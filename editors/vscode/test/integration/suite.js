// Runs inside VS Code, started by `run.mjs` on the repository. Opens each
// panel and does what a reviewer would: clicks `Greeter` in Hello HTTP's
// diagram, runs every requirement from the Test Explorer, and then turns on
// the diagram's coverage.

const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");
const path = require("node:path");
const vscode = require("vscode");

const SENTENCE = "Visitors are greeted by the name in the web address";

/** Waits until `check` returns something truthy, and returns it. */
async function until(what, check, seconds = 60) {
  const deadline = Date.now() + seconds * 1000;
  while (Date.now() < deadline) {
    const value = await check();
    if (value) {
      return value;
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`timed out waiting for ${what}`);
}

/** The diagram panel shows the picture `inset diagram` draws, and clicking `Greeter` opens it. */
async function diagramPanel(api, example) {
  const main = path.join(example, "src", "main.inset");
  await vscode.window.showTextDocument(vscode.Uri.file(main));
  let shown = false;
  const listener = api.diagram.shown.event((ids) => {
    shown ||= ids.includes("HelloHttp/Greeter");
  });
  await vscode.commands.executeCommand("inset.showDiagram");
  await until("the diagram to be shown", () => shown);
  listener.dispose();

  const theme = vscode.window.activeColorTheme.kind === vscode.ColorThemeKind.Light ? "light" : "dark";
  const drawn = execFileSync(process.env.INSET_COMMAND, ["diagram", example, "--theme", theme], {
    encoding: "utf8",
  });
  assert.equal(api.diagram.svg, drawn);
  // What "Side by side" shades when the route is picked.
  assert.deepEqual(api.diagram.picture.routes, [
    { handler: "HelloHttp/Greeter/fn:hello", route: "GET /hello/{name}", holds: ["HelloHttp/Greeter"] },
  ]);

  api.diagram.click("HelloHttp/Greeter");
  const editor = await until("main.inset to open at Greeter", () =>
    vscode.window.visibleTextEditors.find(
      (e) => e.document.uri.fsPath === main && e.selection.start.line === 9,
    ),
  );
  assert.equal(editor.document.lineAt(9).text, "component Greeter {");
  assert.equal(editor.selection.start.character, "component ".length);
}

/**
 * The Test Explorer lists the examples' requirements by their sentences,
 * leaving out the compiler's test fixtures as the repository's settings ask,
 * and runs them. Hello HTTP's requirement passes. A program the compiler
 * cannot build yet has that one error, and its requirements point to it.
 */
async function testExplorer(api) {
  const item = await until("the requirement to be found", () => {
    let found;
    api.requirements.controller.items.forEach((program) =>
      program.children.forEach((child) => {
        if (child.label === SENTENCE) {
          found = child;
        }
      }),
    );
    return found;
  });
  assert.equal(item.uri.path.endsWith("requirements/hello.req.inset"), true);
  assert.equal(item.range.start.line, 0);
  await vscode.commands.executeCommand("vscode.revealTestInExplorer", item);
  const programs = [];
  api.requirements.controller.items.forEach((program) => programs.push(program));
  assert.deepEqual(programs.map((p) => p.label).sort(), [
    "examples/csv-import",
    "examples/hello-http",
    "examples/stock-tracker",
  ]);
  const requirements = programs.flatMap((program) => {
    const children = [];
    program.children.forEach((child) => children.push(child));
    return children;
  });

  await vscode.commands.executeCommand("testing.runAll");
  // Building a program the first time compiles its dependencies.
  const results = api.requirements.results;
  await until("every requirement to run", () => requirements.every((r) => results.has(r.id)), 900).catch(
    (error) => {
      throw new Error(`${error.message}; the runs printed:\n${api.requirements.output}`);
    },
  );
  assert.equal(results.get(item.id).result, "pass", results.get(item.id).detail);
  // Every example builds, and every requirement and examples row passes.
  for (const program of programs) {
    assert.equal(results.get(program.id), undefined, `${program.label}: ${results.get(program.id)?.detail}`);
    program.children.forEach((child) => {
      assert.equal(results.get(child.id).result, "pass", child.label);
      child.children.forEach((row) => assert.equal(results.get(row.id).result, "pass", row.label));
    });
  }
  // The Stock tracker's routes all wait for each other, as the table says.
  assert.match(api.requirements.output, /requests that can run at the same time\n.*\n 1 GET \/items\/\{sku\} +· +no/);
  // A property reports how many generated cases held.
  assert.match(api.requirements.output, /pass {2}The gross price is never below the net price\n {6}held in 100 cases\n/);
}

/**
 * After the run, the diagram panel's coverage shows what the requirements
 * reached: everything in Hello HTTP, as `inset diagram --coverage` draws it.
 */
async function coverage(api, example) {
  await api.diagram.showCoverage(true);
  const theme = vscode.window.activeColorTheme.kind === vscode.ColorThemeKind.Light ? "light" : "dark";
  const drawn = execFileSync(process.env.INSET_COMMAND, ["diagram", example, "--theme", theme, "--coverage"], {
    encoding: "utf8",
  });
  assert.equal(api.diagram.svg, drawn);
  assert.match(drawn, /class="cov-covered"/);
  // Nothing is shaded as not reached: a cross shows only in the key.
  assert.doesNotMatch(drawn, /class="band"/);
  await api.diagram.showCoverage(false);
}

/**
 * The requirements panel lists the sentence as passed, and every Stock
 * tracker requirement too, with each row of its examples table under it,
 * and every CSV import requirement.
 */
async function requirementsPanel(api) {
  await vscode.commands.executeCommand("inset.requirements.focus");
  const tree = api.requirements.tree;
  const program = tree.getChildren().find((p) => p.label === "examples/hello-http");
  const [requirement] = tree.getChildren(program);
  const shown = tree.getTreeItem(requirement);
  assert.equal(shown.label, SENTENCE);
  assert.equal(shown.description, "passed");
  assert.equal(shown.iconPath.id, "pass");

  const stock = tree.getChildren().find((p) => p.label === "examples/stock-tracker");
  assert.equal(tree.getTreeItem(stock).description, "");
  const stockRequirements = tree.getChildren(stock);
  assert.equal(stockRequirements.length, 13);
  for (const requirement of stockRequirements) {
    assert.equal(tree.getTreeItem(requirement).description, "passed", requirement.label);
  }
  const outline = stockRequirements.find((r) => r.label.startsWith("Stock that falls"));
  const rows = tree.getChildren(outline);
  assert.deepEqual(
    rows.map((row) => row.label),
    ['| 12 | 50 | 11 | "" |', '| 11 | 50 | 10 | "bolt-m6 40\\n" |', '| 1 | 50 | 0 | "bolt-m6 50\\n" |', '| 11 | 5 | 10 | "" |'],
  );
  for (const row of rows) {
    assert.equal(tree.getTreeItem(row).description, "passed", row.label);
  }

  // CSV import, with its two properties.
  const csv = tree.getChildren().find((p) => p.label === "examples/csv-import");
  const csvRequirements = tree.getChildren(csv);
  assert.equal(csvRequirements.length, 7);
  for (const requirement of csvRequirements) {
    assert.equal(tree.getTreeItem(requirement).description, "passed", requirement.label);
  }
}

exports.run = async function () {
  const example = path.join(process.env.INSET_REPO, "examples", "hello-http");
  const api = await vscode.extensions.getExtension("inset.inset").activate();
  for (const test of [diagramPanel, testExplorer, coverage, requirementsPanel]) {
    console.log(`integration: ${test.name}`);
    await test(api, example);
  }
  console.log("integration: all passed");
};
