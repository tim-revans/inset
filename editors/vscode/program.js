// Finding the Inset program a file belongs to, by the rule the language
// server uses: the nearest directory above the file whose `src/` or
// `requirements/` holds it, and which has a `src/main.inset`.

const fs = require("node:fs");
const path = require("node:path");

/**
 * The program's directory, or undefined for a file outside any program.
 * @param {import("vscode").Uri} uri
 */
function programRoot(uri) {
  if (uri.scheme !== "file") {
    return undefined;
  }
  const file = uri.fsPath;
  let dir = path.dirname(file);
  while (true) {
    const top = path.relative(dir, file).split(path.sep)[0];
    if ((top === "src" || top === "requirements") && fs.existsSync(path.join(dir, "src", "main.inset"))) {
      return dir;
    }
    const parent = path.dirname(dir);
    if (parent === dir) {
      return undefined;
    }
    dir = parent;
  }
}

module.exports = { programRoot };
