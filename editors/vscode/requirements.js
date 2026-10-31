// Requirements in VS Code's Test Explorer, and in a panel of their own that
// lists each requirement's sentence with whether it last passed.
//
// The requirements are found in each program's `requirements/` files and
// read through the language server's outline, so the extension does not
// parse Inset itself. They run with `inset test <program> --json`, which
// builds the program and runs every requirement in it. So running one
// requirement runs, and reports, all of its program's requirements: each
// starts the program afresh, so the others' results are as good as a run of
// their own.

const { spawn } = require("node:child_process");
const path = require("node:path");
const vscode = require("vscode");
const { programRoot } = require("./program");

/**
 * What `inset test --json` prints for one requirement.
 * @typedef {{
 *   sentence: string,
 *   file: string,
 *   line: number,
 *   result: "pass" | "fail",
 *   failure?: { line: number, step: string, message: string },
 * }} Outcome
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
    this.tree = new RequirementsTree(this);
    // The panel shows only in a workspace with requirements.
    this.changed.event(() =>
      vscode.commands.executeCommand("setContext", "inset.hasRequirements", this.controller.items.size > 0),
    );

    const watcher = vscode.workspace.createFileSystemWatcher("**/requirements/**/*.req.inset");
    context.subscriptions.push(
      this.controller,
      this.changed,
      watcher,
      watcher.onDidCreate((uri) => this.discoverFile(uri)),
      watcher.onDidChange((uri) => this.discoverFile(uri)),
      watcher.onDidDelete((uri) => this.forget(uri)),
      vscode.workspace.onDidSaveTextDocument((document) => {
        if (document.languageId === "inset-req") {
          this.discoverFile(document.uri);
        }
      }),
      vscode.window.registerTreeDataProvider("inset.requirements", this.tree),
      vscode.commands.registerCommand("inset.runRequirements", () => this.runAll()),
      vscode.commands.registerCommand("inset.refreshRequirements", () => this.discover()),
    );
  }

  /** Finds every program's requirements in the workspace. */
  async discover() {
    const files = await vscode.workspace.findFiles(
      "**/requirements/**/*.req.inset",
      "{**/node_modules/**,**/target/**}",
    );
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
      program.children.add(item);
    }
    this.changed.fire();
  }

  /** @param {vscode.Uri} uri */
  forget(uri) {
    const root = programRoot(uri);
    const program = root && this.controller.items.get(root);
    program?.children.forEach((item) => {
      if (item.uri?.toString() === uri.toString()) {
        program.children.delete(item.id);
      }
    });
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
      roots.add(item.parent ? item.parent.id : item.id);
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
    });
    const label = program.label;
    const print = (text) => {
      run.appendOutput(text.replace(/\r?\n/g, "\r\n"));
      this.output += text;
    };
    print(`inset test ${label}\n`);
    const { code, stdout, stderr } = await execute(this.command(), ["test", root, "--json"], root, token);
    if (stderr) {
      print(stderr);
    }
    /** @type {{ requirements: Outcome[] } | undefined} */
    let report;
    try {
      report = JSON.parse(stdout);
    } catch {
      report = undefined;
    }
    if (!report) {
      // The program did not build, or `inset` did not start: every
      // requirement is in error, with what `inset test` said.
      const text = stderr.trim() || `\`inset test\` stopped with ${code} and printed no report.`;
      for (const item of items) {
        run.errored(item, new vscode.TestMessage(text));
        this.results.set(item.id, { result: "error", detail: text });
      }
      return;
    }
    for (const outcome of report.requirements) {
      const item = program.children.get(requirementId(root, outcome.sentence));
      if (!item) {
        continue;
      }
      const failure = outcome.failure;
      print(`${failure ? "FAIL" : "pass"}  ${outcome.sentence}\n`);
      if (!failure) {
        run.passed(item);
        this.results.set(item.id, { result: "pass" });
        continue;
      }
      const uri = vscode.Uri.file(path.join(root, outcome.file));
      const location = new vscode.Location(uri, new vscode.Position(failure.line - 1, 0));
      const detail = `${failure.step.trim()}\n${failure.message}`;
      const message = new vscode.TestMessage(detail);
      message.location = location;
      run.failed(item, message);
      this.results.set(item.id, { result: "fail", detail, location });
    }
  }
}

/** A requirement's item ID: its program and sentence, which is unique in the program. */
function requirementId(root, sentence) {
  return `${root}\n${sentence}`;
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
 * and its last result, and for a failure the step that did not hold.
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
      if (node.parent) {
        const result = this.requirements.results.get(node.id);
        return result && result.result !== "pass" ? [result] : [];
      }
    }
    return children.sort(byPosition);
  }

  /** @param {vscode.TestItem | Result} node */
  getTreeItem(node) {
    if (!("children" in node)) {
      // The step that failed, or why the program could not run.
      const tree = new vscode.TreeItem(node.detail?.split("\n").join(" — ") ?? "");
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
      tree.contextValue = "program";
      return tree;
    }
    const result = this.requirements.results.get(node.id);
    const state = result ? vscode.TreeItemCollapsibleState.Expanded : vscode.TreeItemCollapsibleState.None;
    const tree = new vscode.TreeItem(node.label, result?.result === "pass" ? undefined : state);
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
