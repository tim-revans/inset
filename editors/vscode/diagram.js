// The diagram panel: the picture `inset diagram` draws of the program being
// edited, beside the code. The language server draws it, through its
// `inset/diagram` request, so the picture comes from the compiler's own
// renderer. Clicking an element opens its source, and the panel redraws
// whenever an Inset file is saved. With coverage on, the picture marks what
// the requirements reached in their last run, which writes the coverage
// the panel reads, and the panel redraws after each run.

const vscode = require("vscode");
const { programRoot } = require("./program");

/** @typedef {import("vscode-languageclient/node").LanguageClient} LanguageClient */

/**
 * What the server answers.
 * @typedef {{
 *   app?: string,
 *   svg?: string,
 *   functions: string[],
 *   sources: Record<string, { uri: string, range: any }>,
 *   problems: { message: string, location?: { uri: string, range: any } }[],
 * }} Picture
 */

class DiagramPanel {
  /**
   * @param {vscode.ExtensionContext} context
   * @param {() => LanguageClient | undefined} client
   */
  constructor(context, client) {
    this.context = context;
    this.client = client;
    /** @type {vscode.WebviewPanel | undefined} */
    this.panel = undefined;
    /** A document of the program shown. @type {vscode.Uri | undefined} */
    this.target = undefined;
    /** The function whose flowchart is shown, or the components when null. */
    this.flowchart = null;
    this.expandAll = false;
    /** Whether the picture marks what the last run of the requirements reached. */
    this.coverage = false;
    /** @type {Picture | undefined} */
    this.picture = undefined;
    /** Fires with the IDs the webview drew, once it has drawn a picture. */
    this.shown = new vscode.EventEmitter();
    /** The SVG last sent to the webview. @type {string | undefined} */
    this.svg = undefined;

    context.subscriptions.push(
      this.shown,
      vscode.commands.registerCommand("inset.showDiagram", () => this.show()),
      vscode.workspace.onDidSaveTextDocument((document) => {
        if (document.uri.path.endsWith(".inset")) {
          this.refresh();
        }
      }),
      vscode.window.onDidChangeActiveTextEditor((editor) => {
        if (editor && this.panel && isInset(editor.document)) {
          this.follow(editor.document.uri);
        }
      }),
      vscode.window.onDidChangeActiveColorTheme(() => this.refresh()),
    );
  }

  /** Opens the panel beside the active editor, for the program it belongs to. */
  async show() {
    const editor = vscode.window.activeTextEditor;
    if (editor && isInset(editor.document)) {
      this.follow(editor.document.uri);
    }
    if (!this.target) {
      vscode.window.showInformationMessage("Inset: open a file of an Inset program to see its diagram.");
      return;
    }
    if (this.panel) {
      this.panel.reveal(undefined, true);
      return;
    }
    const media = vscode.Uri.joinPath(this.context.extensionUri, "media");
    this.panel = vscode.window.createWebviewPanel(
      "inset.diagram",
      "Inset diagram",
      { viewColumn: vscode.ViewColumn.Beside, preserveFocus: true },
      { enableScripts: true, localResourceRoots: [media] },
    );
    this.panel.iconPath = vscode.Uri.joinPath(this.context.extensionUri, "icons", "inset.svg");
    this.panel.webview.html = html(this.panel.webview, media);
    this.panel.webview.onDidReceiveMessage((message) => this.receive(message));
    this.panel.onDidDispose(() => {
      this.panel = undefined;
      this.svg = undefined;
    });
  }

  /** Shows the program a document belongs to, if it is not the one shown. */
  follow(uri) {
    const root = programRoot(uri);
    if (!root) {
      return;
    }
    const before = this.target && programRoot(this.target);
    this.target = uri;
    if (root !== before) {
      this.flowchart = null;
      if (this.panel) {
        this.refresh();
      }
    }
  }

  /** Asks the server for the picture again, and shows it. */
  async refresh() {
    const client = this.client();
    if (!this.panel || !this.target) {
      return;
    }
    if (!client || !client.isRunning()) {
      this.post({ type: "message", text: "The Inset language server is not running, so there is no diagram." });
      return;
    }
    const light = [vscode.ColorThemeKind.Light, vscode.ColorThemeKind.HighContrastLight].includes(
      vscode.window.activeColorTheme.kind,
    );
    const options = ["--theme", light ? "light" : "dark"];
    if (this.flowchart) {
      options.push("--flowchart", this.flowchart);
    } else if (this.expandAll) {
      options.push("--expand-all");
    }
    if (this.coverage) {
      options.push("--coverage");
    }
    /** @type {Picture | null} */
    let picture;
    try {
      picture = await client.sendRequest("inset/diagram", {
        textDocument: { uri: this.target.toString() },
        options,
      });
    } catch (error) {
      this.post({ type: "message", text: `The diagram could not be drawn: ${error.message}` });
      return;
    }
    if (!picture) {
      this.post({ type: "message", text: "This file is not part of an Inset program." });
      return;
    }
    if (this.flowchart && !picture.functions.includes(this.flowchart)) {
      // The function is gone, so show the components instead.
      this.flowchart = null;
      return this.refresh();
    }
    this.picture = picture;
    this.svg = picture.svg;
    this.panel.title = picture.app ? `${picture.app} diagram` : "Inset diagram";
    this.post({
      type: "picture",
      svg: picture.svg ?? null,
      app: picture.app ?? null,
      functions: picture.functions,
      flowchart: this.flowchart,
      expandAll: this.expandAll,
      coverage: this.coverage,
      clickable: Object.keys(picture.sources),
      problems: picture.problems.map((p) => p.message),
    });
  }

  post(message) {
    this.panel?.webview.postMessage(message);
  }

  async receive(message) {
    switch (message.type) {
      case "ready":
        return this.refresh();
      case "view":
        this.flowchart = message.flowchart || null;
        this.expandAll = Boolean(message.expandAll);
        this.coverage = Boolean(message.coverage);
        return this.refresh();
      case "open":
        return this.open(this.picture?.sources[message.id]);
      case "problem":
        return this.open(this.picture?.problems[message.index]?.location);
      case "shown":
        this.shown.fire(message.ids);
        return;
    }
  }

  /** Opens a location in the editor beside the panel, keeping focus on the panel. */
  async open(location) {
    if (!location) {
      return;
    }
    const uri = vscode.Uri.parse(location.uri);
    const { start, end } = location.range;
    const range = new vscode.Range(start.line, start.character, end.line, end.character);
    const viewColumn =
      this.panel?.viewColumn === vscode.ViewColumn.One ? vscode.ViewColumn.Two : vscode.ViewColumn.One;
    await vscode.window.showTextDocument(uri, { viewColumn, selection: range, preserveFocus: true });
  }

  /** Redraws after the requirements of the program shown have run, when coverage is shown. */
  ran(root) {
    if (this.coverage && this.target && programRoot(this.target) === root) {
      this.refresh();
    }
  }

  /** For tests: shows or hides coverage, as the reader's checkbox does. */
  showCoverage(on) {
    this.coverage = on;
    return this.refresh();
  }

  /** For tests: clicks the element with an ID in the webview, as a reader would. */
  click(id) {
    this.post({ type: "click", id });
  }
}

/** @param {vscode.TextDocument} document */
function isInset(document) {
  return document.languageId === "inset" || document.languageId === "inset-req";
}

/**
 * @param {vscode.Webview} webview
 * @param {vscode.Uri} media
 */
function html(webview, media) {
  const nonce = [...Array(32)].map(() => Math.floor(Math.random() * 36).toString(36)).join("");
  const script = webview.asWebviewUri(vscode.Uri.joinPath(media, "diagram.js"));
  const style = webview.asWebviewUri(vscode.Uri.joinPath(media, "diagram.css"));
  return `<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src ${webview.cspSource} 'unsafe-inline'; script-src 'nonce-${nonce}';">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<link rel="stylesheet" href="${style}">
<title>Inset diagram</title>
</head>
<body>
<div class="toolbar">
  <label>Show <select id="view"><option value="">Components</option></select></label>
  <label><input type="checkbox" id="expand-all"> Expand every component</label>
  <label title="What the requirements reached when they last ran"><input type="checkbox" id="coverage"> Coverage</label>
</div>
<p id="message" hidden></p>
<ul id="problems" hidden></ul>
<div id="picture"></div>
<script nonce="${nonce}" src="${script}"></script>
</body>
</html>`;
}

module.exports = { DiagramPanel };
