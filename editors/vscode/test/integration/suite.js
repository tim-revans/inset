// Runs inside VS Code, started by `run.mjs` on Hello HTTP. Opens each panel
// and does what a reviewer would: clicks `Greeter` in the diagram, and runs
// the requirement from the Test Explorer.

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

  api.diagram.click("HelloHttp/Greeter");
  const editor = await until("main.inset to open at Greeter", () =>
    vscode.window.visibleTextEditors.find(
      (e) => e.document.uri.fsPath === main && e.selection.start.line === 9,
    ),
  );
  assert.equal(editor.document.lineAt(9).text, "component Greeter {");
  assert.equal(editor.selection.start.character, "component ".length);
}

/** The Test Explorer lists the requirement by its sentence, and runs it. */
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
  await vscode.commands.executeCommand("testing.runAll");
  // Building the program the first time compiles its dependencies.
  const result = await until("the requirement to run", () => api.requirements.results.get(item.id), 900).catch(
    (error) => {
      throw new Error(`${error.message}; the runs printed:\n${api.requirements.output}`);
    },
  );
  assert.equal(result.result, "pass", result.detail);
}

/** The requirements panel lists the sentence as passed. */
async function requirementsPanel(api) {
  await vscode.commands.executeCommand("inset.requirements.focus");
  const tree = api.requirements.tree;
  const [program] = tree.getChildren();
  const [requirement] = tree.getChildren(program);
  const shown = tree.getTreeItem(requirement);
  assert.equal(shown.label, SENTENCE);
  assert.equal(shown.description, "passed");
  assert.equal(shown.iconPath.id, "pass");
}

exports.run = async function () {
  const example = path.join(process.env.INSET_REPO, "examples", "hello-http");
  const api = await vscode.extensions.getExtension("inset.inset").activate();
  for (const test of [diagramPanel, testExplorer, requirementsPanel]) {
    console.log(`integration: ${test.name}`);
    await test(api, example);
  }
  console.log("integration: all passed");
};
