// Starts the Inset language server, `inset lsp`, for Inset code and
// requirements. The server shows what `inset check` reports as you type, and
// gives hover, go-to-definition and the outline. Highlighting comes from the
// grammars alone, so it works without the server.

const vscode = require("vscode");
const { LanguageClient } = require("vscode-languageclient/node");

/** @type {LanguageClient | undefined} */
let client;

async function start() {
  const command = vscode.workspace.getConfiguration("inset").get("server.path") || "inset";
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
  }
}

async function stop() {
  const running = client;
  client = undefined;
  if (running) {
    await running.stop();
  }
}

/** @param {vscode.ExtensionContext} context */
async function activate(context) {
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
}

function deactivate() {
  return stop();
}

module.exports = { activate, deactivate };
