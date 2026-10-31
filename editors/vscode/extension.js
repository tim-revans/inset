// Starts the Inset language server, `inset lsp`, for Inset code and
// requirements. The server shows what `inset check` reports as you type, and
// gives hover, go-to-definition and the outline. Highlighting comes from the
// grammars alone, so it works without the server.
//
// The review panels are in modules of their own: the diagram in
// `diagram.js`, and the requirements, in a panel and in the Test Explorer,
// in `requirements.js`.

const vscode = require("vscode");
const { LanguageClient } = require("vscode-languageclient/node");
const { DiagramPanel } = require("./diagram");
const { Requirements } = require("./requirements");

/** @type {LanguageClient | undefined} */
let client;

/** @type {DiagramPanel | undefined} */
let diagram;

/** @type {Requirements | undefined} */
let requirements;

/** The `inset` command, which serves the language and runs requirements. */
function insetCommand() {
  return vscode.workspace.getConfiguration("inset").get("server.path") || "inset";
}

async function start() {
  const command = insetCommand();
  client = new LanguageClient(
    "inset",
    "Inset",
    { command, args: ["lsp"] },
    {
      documentSelector: [
        { scheme: "file", language: "inset" },
        { scheme: "file", language: "inset-req" },
        { scheme: "untitled", language: "inset" },
        { scheme: "untitled", language: "inset-req" },
      ],
      // A program is checked as a whole, so a change to a file that is not
      // open, such as a requirements file edited elsewhere, is checked too.
      synchronize: {
        fileEvents: vscode.workspace.createFileSystemWatcher("**/*.inset"),
      },
    },
  );
  try {
    await client.start();
  } catch (error) {
    client = undefined;
    vscode.window.showWarningMessage(
      `Inset: could not start the language server with \`${command} lsp\` (${error.message}). ` +
        "Install the `inset` command with `cargo install --path crates/inset-cli`, " +
        "or set `inset.server.path` to it.",
    );
    return;
  }
  // The requirements are read through the server, and the diagram drawn by it.
  requirements?.discover();
  diagram?.refresh();
}

async function stop() {
  const running = client;
  client = undefined;
  if (running) {
    await running.stop();
  }
}

/**
 * Returns the panels, which the extension's integration tests drive.
 * @param {vscode.ExtensionContext} context
 */
async function activate(context) {
  diagram = new DiagramPanel(context, () => client);
  requirements = new Requirements(context, insetCommand);
  context.subscriptions.push(requirements.ran.event((root) => diagram?.ran(root)));
  context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration(async (event) => {
      if (event.affectsConfiguration("inset.server.path")) {
        await stop();
        await start();
      }
    }),
    vscode.commands.registerCommand("inset.restartServer", async () => {
      await stop();
      await start();
    }),
  );
  await start();
  return { diagram, requirements };
}

function deactivate() {
  return stop();
}

module.exports = { activate, deactivate };
