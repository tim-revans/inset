// Requirements in VS Code's Test Explorer, and in a panel of their own that
// lists each requirement's sentence with whether it last passed.
//
// The requirements are found in each program's `requirements/` files and
// read through the language server's outline, so the extension does not
// parse Inset itself. They run with `inset test <program> --json
// --coverage`, which builds the program and runs every requirement in it,
// and writes what they reached for the diagram panel's coverage view.
// Code marked `#[defensive]` that a requirement reached fails the program. So running one
// requirement runs, and reports, all of its program's requirements: each
// starts the program afresh, so the others' results are as good as a run of
// their own.

const { spawn } = require("node:child_process");
const path = require("node:path");
const vscode = require("vscode");
const { programRoot } = require("./program");

/**
 * A step that did not hold, as `inset test --json` prints it.
 * @typedef {{ line: number, step: string, message: string, expected?: string, actual?: string }} Failure
 */

/**
 * What `inset test --json` prints for one requirement. A requirement with
 * an examples table has a result for each row, and fails if any row does.
 * A property has how many generated cases held, and if it failed, the
 * smallest input found that breaks it.
 * @typedef {{
 *   sentence: string,
 *   file: string,
 *   line: number,
 *   result: "pass" | "fail",
 *   failure?: Failure,
 *   rows?: { cells: string[], line: number, result: "pass" | "fail", failure?: Failure }[],
 *   cases?: number,
 *   inputs?: { name: string, value: string }[],
 * }} Outcome
 */

/**
 * Code marked `#[defensive]` that a requirement reached, as `inset test --json` prints it.
 * @typedef {{ id: string, at: string, reason: string, requirements: string[] }} Defensive
 */

/**
 * The last result of a requirement, for the panel.
 * @typedef {{ result: "pass" | "fail" | "error", detail?: string, location?: vscode.Location }} Result
 */

class Requirements {
  /**
   * @param {vscode.ExtensionContext} context
   * @param {() => string} command The `inset` command.
   */
  constructor(context, command) {
    this.command = command;
    this.controller = vscode.tests.createTestController("inset", "Inset requirements");
    this.controller.refreshHandler = () => this.discover();
    this.controller.createRunProfile(
      "Run",
      vscode.TestRunProfileKind.Run,
      (request, token) => this.run(request, token),
      true,
    );
    /** Results by requirement item ID. @type {Map<string, Result>} */
    this.results = new Map();
    /** Everything the runs have printed, for the integration tests to show when one fails. */
    this.output = "";
    /** Fires when the list or a result changes. */
    this.changed = new vscode.EventEmitter();
    /** Fires with a program's root when its requirements have run. */
    this.ran = new vscode.EventEmitter();
    this.tree = new RequirementsTree(this);
    // The panel shows only in a workspace with requirements.
    this.changed.event(() =>
      vscode.commands.executeCommand("setContext", "inset.hasRequirements", this.controller.items.size > 0),
    );

    const watcher = vscode.workspace.createFileSystemWatcher("**/requirements/**/*.req.inset");
    context.subscriptions.push(
      this.controller,
      this.changed,
      this.ran,
      watcher,
      watcher.onDidCreate(() => this.discoverSoon()),
      watcher.onDidChange(() => this.discoverSoon()),
      watcher.onDidDelete(() => this.discoverSoon()),
      vscode.workspace.onDidSaveTextDocument((document) => {
        if (document.languageId === "inset-req") {
          this.discoverSoon();
        }
      }),
      vscode.workspace.onDidChangeConfiguration((event) => {
        if (event.affectsConfiguration("inset.requirements.exclude")) {
          this.discoverSoon();
        }
      }),
      vscode.window.registerTreeDataProvider("inset.requirements", this.tree),
      vscode.commands.registerCommand("inset.runRequirements", () => this.runAll()),
      vscode.commands.registerCommand("inset.refreshRequirements", () => this.discover()),
    );
  }

  /** Finds the requirements again shortly, once a burst of file changes is over. */
  discoverSoon() {
    clearTimeout(this.timer);
    this.timer = setTimeout(() => this.discover(), 300);
  }

  /**
   * Finds every program's requirements in the workspace, leaving out build
   * output and the files matched by `inset.requirements.exclude`.
   */
  async discover() {
    const excluded = ["**/node_modules/**", "**/target/**"].concat(
      vscode.workspace.getConfiguration("inset").get("requirements.exclude") ?? [],
    );
    const files = await vscode.workspace.findFiles("**/requirements/**/*.req.inset", `{${excluded.join(",")}}`);
    const found = new Set();
    for (const uri of files) {
      const root = programRoot(uri);
      if (root) {
        found.add(root);
        await this.discoverFile(uri);
      }
    }
    this.controller.items.forEach((program) => {
      if (!found.has(program.id)) {
        this.controller.items.delete(program.id);
      } else {
        program.children.forEach((item) => {
          if (!files.some((uri) => uri.toString() === item.uri?.toString())) {
            program.children.delete(item.id);
          }
        });
      }
    });
    this.changed.fire();
  }

  /**
   * Reads one requirements file's requirements from the language server's
   * outline of it, and puts them under their program.
   * @param {vscode.Uri} uri
   */
  async discoverFile(uri) {
    const root = programRoot(uri);
    if (!root) {
      return;
    }
    /** @type {vscode.DocumentSymbol[] | undefined} */
    let symbols;
    try {
      symbols = await vscode.commands.executeCommand("vscode.executeDocumentSymbolProvider", uri);
    } catch {
      symbols = undefined;
    }
    const program = this.program(root);
    program.children.forEach((item) => {
      if (item.uri?.toString() === uri.toString()) {
        program.children.delete(item.id);
      }
    });
    for (const symbol of symbols ?? []) {
      if (symbol.detail !== "requirement") {
        continue;
      }
      const item = this.controller.createTestItem(requirementId(root, symbol.name), symbol.name, uri);
      item.range = symbol.range;
      // Each row of an examples table is a case of its own.
      (symbol.children ?? [])
        .filter((child) => child.detail === "examples row")
        .forEach((child, index) => {
          const row = this.controller.createTestItem(rowId(item.id, index), child.name, uri);
          row.range = child.range;
          item.children.add(row);
        });
      program.children.add(item);
    }
    this.changed.fire();
  }

  /** The item for a program, made if it is new. @param {string} root */
  program(root) {
    let item = this.controller.items.get(root);
    if (!item) {
      const folder = vscode.workspace.getWorkspaceFolder(vscode.Uri.file(root));
      const label = folder ? path.relative(folder.uri.fsPath, root) || folder.name : path.basename(root);
      item = this.controller.createTestItem(root, label, vscode.Uri.file(root));
      this.controller.items.add(item);
    }
    return item;
  }

  /** Runs every program's requirements. */
  runAll() {
    const cancel = new vscode.CancellationTokenSource();
    return this.run(new vscode.TestRunRequest(), cancel.token).finally(() => cancel.dispose());
  }

  /**
   * @param {vscode.TestRunRequest} request
   * @param {vscode.CancellationToken} token
   */
  async run(request, token) {
    const run = this.controller.createTestRun(request);
    /** The programs to run, by directory. @type {Set<string>} */
    const roots = new Set();
    const requested = request.include ?? [];
    if (requested.length === 0) {
      this.controller.items.forEach((program) => roots.add(program.id));
    }
    for (const item of requested) {
      let program = item;
      while (program.parent) {
        program = program.parent;
      }
      roots.add(program.id);
    }
    try {
      for (const root of roots) {
        if (token.isCancellationRequested) {
          break;
        }
        await this.runProgram(root, run, token);
      }
    } finally {
      run.end();
      this.changed.fire();
    }
  }

  /**
   * @param {string} root
   * @param {vscode.TestRun} run
   * @param {vscode.CancellationToken} token
   */
  async runProgram(root, run, token) {
    const program = this.controller.items.get(root);
    if (!program) {
      return;
    }
    /** @type {vscode.TestItem[]} */
    const items = [];
    program.children.forEach((item) => {
      items.push(item);
      run.started(item);
      item.children.forEach((row) => {
        items.push(row);
        run.started(row);
      });
    });
    const label = program.label;
    const print = (text) => {
      run.appendOutput(text.replace(/\r?\n/g, "\r\n"));
      this.output += text;
    };
    print(`inset test ${label}\n`);
    const { code, stdout, stderr } = await execute(this.command(), ["test", root, "--json", "--coverage"], root, token);
    this.ran.fire(root);
    if (stderr) {
      print(stderr);
    }
    /** @type {{ requirements: Outcome[], defensive?: Defensive[], routes?: { route: string, holds: string[] }[] } | undefined} */
    let report;
    try {
      report = JSON.parse(stdout);
    } catch {
      report = undefined;
    }
    if (!report) {
      // The program did not build, or `inset` did not start. The program
      // gets what `inset test` said, once, and each requirement a line
      // pointing there.
      const text = stderr.trim() || `\`inset test\` stopped with ${code} and printed no report.`;
      run.errored(program, new vscode.TestMessage(text));
      this.results.set(program.id, { result: "error", detail: text });
      const pointer = `${label} could not be built, so its requirements did not run. See the error on ${label}.`;
      for (const item of items) {
        run.errored(item, new vscode.TestMessage(pointer));
        this.results.set(item.id, { result: "error", detail: pointer });
      }
      return;
    }
    this.results.delete(program.id);
    const defensive = report.defensive ?? [];
    if (defensive.length > 0) {
      const text = defensive
        .map((d) => `Defensive code was reached: ${d.id}\n${d.at}\n#[defensive(${JSON.stringify(d.reason)})]\nbut ${d.requirements.map((r) => JSON.stringify(r)).join(" and ")} reached it`)
        .join("\n\n");
      print(`FAIL  ${text.replace(/\n/g, "\n      ")}\n`);
      run.failed(program, new vscode.TestMessage(text));
      this.results.set(program.id, { result: "fail", detail: text });
    }
    for (const outcome of report.requirements) {
      const item = program.children.get(requirementId(root, outcome.sentence));
      if (!item) {
        continue;
      }
      print(`${outcome.result === "fail" ? "FAIL" : "pass"}  ${outcome.sentence}\n`);
      const property = propertyLine(outcome);
      if (property) {
        print(`      ${property}\n`);
      }
      const uri = vscode.Uri.file(path.join(root, outcome.file));
      const rows = outcome.rows ?? [];
      rows.forEach((row, index) => {
        const rowItem = item.children.get(rowId(item.id, index));
        print(`      ${row.result === "fail" ? "FAIL" : "pass"}  | ${row.cells.join(" | ")} |\n`);
        if (rowItem) {
          this.report(run, rowItem, uri, row.failure);
        }
      });
      if (rows.length > 0) {
        const failed = rows.filter((row) => row.result === "fail").length;
        if (failed === 0) {
          run.passed(item);
          this.results.set(item.id, { result: "pass" });
        } else {
          const detail = `${failed} of ${rows.length} rows of the examples table failed`;
          run.failed(item, new vscode.TestMessage(detail));
          this.results.set(item.id, { result: "fail", detail });
        }
        continue;
      }
      this.report(run, item, uri, outcome.failure, outcome.inputs?.length ? property : undefined);
    }
    print(sideBySide(report.routes ?? []));
  }

  /**
   * Reports whether a requirement or an examples row passed, and for a
   * failure the step that did not hold.
   * @param {vscode.TestRun} run
   * @param {vscode.TestItem} item
   * @param {vscode.Uri} uri The requirements file.
   * @param {Failure | undefined} failure
   * @param {string} [inputs] For a property, the input it fails for.
   */
  report(run, item, uri, failure, inputs) {
    if (!failure) {
      run.passed(item);
      this.results.set(item.id, { result: "pass" });
      return;
    }
    const location = new vscode.Location(uri, new vscode.Position(failure.line - 1, 0));
    const detail = `${inputs ? `${inputs}\n` : ""}${failure.step.trim()}\n${failure.message}`;
    // A fact that compared two values shows them side by side.
    const message =
      failure.expected !== undefined && failure.actual !== undefined
        ? vscode.TestMessage.diff(detail, failure.expected, failure.actual)
        : new vscode.TestMessage(detail);
    message.location = location;
    run.failed(item, message);
    this.results.set(item.id, { result: "fail", detail, location });
  }
}

/**
 * Which routes' requests can run at the same time, as the table `inset test`
 * prints: `yes` where two routes hold no component in common.
 * @param {{ route: string, holds: string[] }[]} routes
 */
function sideBySide(routes) {
  if (routes.length < 2) {
    return "";
  }
  const width = Math.max(...routes.map((r) => [...r.route].length));
  const cell = (/** @type {string} */ text) => text.padStart(4);
  let out = "\nrequests that can run at the same time\n";
  out += `    ${"".padEnd(width)}${routes.map((_, i) => cell(String(i + 1))).join("")}\n`;
  routes.forEach((a, i) => {
    const cells = routes.map((b, j) => {
      if (i === j) return cell("·");
      return cell(a.holds.some((c) => b.holds.includes(c)) ? "no" : "yes");
    });
    out += `${String(i + 1).padStart(2)} ${a.route.padEnd(width)} ${cells.join("")}\n`;
  });
  return out;
}

/**
 * For a property, the line `inset test` prints under its sentence: how many
 * cases held, or the smallest input found that breaks it.
 * @param {Outcome} outcome
 */
function propertyLine(outcome) {
  if (outcome.cases === undefined) {
    return undefined;
  }
  if (outcome.inputs?.length) {
    const inputs = outcome.inputs.map((input) => `${input.name} = ${input.value}`).join(", ");
    return `fails for ${inputs}, the smallest failing input found`;
  }
  return outcome.result === "pass" ? `held in ${outcome.cases} ${outcome.cases === 1 ? "case" : "cases"}` : undefined;
}

/** A requirement's item ID: its program and sentence, which is unique in the program. */
function requirementId(root, sentence) {
  return `${root}\n${sentence}`;
}

/** An examples row's item ID: its requirement's, and its place in the table. */
function rowId(requirement, index) {
  return `${requirement}\n${index}`;
}

/**
 * Runs a command to the end, or until cancelled.
 * @returns {Promise<{ code: number | string, stdout: string, stderr: string }>}
 */
function execute(command, args, cwd, token) {
  return new Promise((resolve) => {
    const child = spawn(command, args, { cwd });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (data) => (stdout += data));
    child.stderr.on("data", (data) => (stderr += data));
    const cancelled = token.onCancellationRequested(() => child.kill());
    child.on("error", (error) => {
      cancelled.dispose();
      resolve({ code: "an error", stdout, stderr: `Could not run \`${command}\`: ${error.message}` });
    });
    child.on("close", (code, signal) => {
      cancelled.dispose();
      resolve({ code: code ?? `signal ${signal}`, stdout, stderr });
    });
  });
}

/**
 * The requirements panel: each program, with each requirement's sentence
 * and its last result, and for a failure the step that did not hold. A
 * requirement with an examples table has its rows under it, each with its
 * own result.
 * @implements {vscode.TreeDataProvider<vscode.TestItem | Result>}
 */
class RequirementsTree {
  /** @param {Requirements} requirements */
  constructor(requirements) {
    this.requirements = requirements;
    this.onDidChangeTreeData = requirements.changed.event;
  }

  /** @param {vscode.TestItem | Result | undefined} node */
  getChildren(node) {
    const children = [];
    if (!node) {
      this.requirements.controller.items.forEach((program) => children.push(program));
    } else if ("children" in node) {
      node.children.forEach((item) => children.push(item));
      const result = this.requirements.results.get(node.id);
      const problem = result && result.result !== "pass" ? [result] : [];
      if (node.parent && children.length > 0) {
        // A requirement with an examples table: its rows, each with its result.
        return children.sort(byPosition);
      }
      if (node.parent) {
        return problem;
      }
      return [...problem, ...children.sort(byPosition)];
    }
    return children.sort(byPosition);
  }

  /** @param {vscode.TestItem | Result} node */
  getTreeItem(node) {
    if (!("children" in node)) {
      // The step that failed, or why the program could not run.
      // A failure's step and why; an error's first line, with the rest in the tooltip.
      const lines = node.detail?.split("\n") ?? [];
      const tree = new vscode.TreeItem((node.result === "fail" ? lines : lines.slice(0, 1)).join(" — "));
      tree.tooltip = node.detail;
      tree.iconPath = new vscode.ThemeIcon("debug-stackframe-dot");
      if (node.location) {
        tree.command = openCommand(node.location.uri, node.location.range);
      }
      return tree;
    }
    if (!node.parent) {
      const tree = new vscode.TreeItem(node.label, vscode.TreeItemCollapsibleState.Expanded);
      tree.iconPath = new vscode.ThemeIcon("package");
      tree.description = this.requirements.results.has(node.id) ? "could not be built" : "";
      tree.contextValue = "program";
      return tree;
    }
    const result = this.requirements.results.get(node.id);
    const hasRows = node.children.size > 0;
    const state =
      result || hasRows ? vscode.TreeItemCollapsibleState.Expanded : vscode.TreeItemCollapsibleState.None;
    const tree = new vscode.TreeItem(node.label, result?.result === "pass" && !hasRows ? undefined : state);
    tree.id = node.id;
    tree.iconPath = icon(result?.result);
    tree.description = { pass: "passed", fail: "failed", error: "could not run" }[result?.result] ?? "";
    tree.tooltip = node.label;
    if (node.uri && node.range) {
      tree.command = openCommand(node.uri, node.range);
    }
    return tree;
  }

  /** @param {vscode.TestItem | Result} node */
  getParent(node) {
    return "children" in node ? node.parent : undefined;
  }
}

/** @param {"pass" | "fail" | "error" | undefined} result */
function icon(result) {
  switch (result) {
    case "pass":
      return new vscode.ThemeIcon("pass", new vscode.ThemeColor("testing.iconPassed"));
    case "fail":
      return new vscode.ThemeIcon("error", new vscode.ThemeColor("testing.iconFailed"));
    case "error":
      return new vscode.ThemeIcon("warning", new vscode.ThemeColor("testing.iconErrored"));
    default:
      return new vscode.ThemeIcon("circle-large-outline");
  }
}

/** Requirements in file order, then by position. */
function byPosition(a, b) {
  const file = (a.uri?.fsPath ?? "").localeCompare(b.uri?.fsPath ?? "");
  return file || (a.range?.start.line ?? 0) - (b.range?.start.line ?? 0) || String(a.label).localeCompare(b.label);
}

function openCommand(uri, range) {
  return {
    command: "vscode.open",
    title: "Open",
    arguments: [uri, { selection: new vscode.Range(range.start, range.start) }],
  };
}

module.exports = { Requirements };
