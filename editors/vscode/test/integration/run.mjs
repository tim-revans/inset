// Integration tests: starts VS Code with the extension, opens Hello HTTP,
// and runs `suite.js` inside it, which opens the panels and drives them.
//
// `INSET_COMMAND` must name an `inset` binary built from this repository,
// which `cargo xtask ci` does. VS Code is downloaded once, at a pinned
// version, into `vscode-test/` under the Cargo target directory. It needs a
// display: `cargo xtask ci` runs this under `xvfb-run` when it is installed.

import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { runTests } from "@vscode/test-electron";

const VSCODE_VERSION = "1.105.1";

const extensionDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const repoDir = path.resolve(extensionDir, "../..");
const inset = process.env.INSET_COMMAND;
if (!inset) {
  console.error("Set INSET_COMMAND to the `inset` binary to test with.");
  process.exit(1);
}
const targetDir = process.env.CARGO_TARGET_DIR || path.join(repoDir, "target");

// A user data directory of its own, so the tests see none of your settings
// and the extension starts the `inset` under test.
const userDataDir = fs.mkdtempSync(path.join(os.tmpdir(), "inset-vscode-"));
fs.mkdirSync(path.join(userDataDir, "User"));
fs.writeFileSync(
  path.join(userDataDir, "User", "settings.json"),
  JSON.stringify({
    "inset.server.path": path.resolve(inset),
    "workbench.colorTheme": "Default Dark Modern",
    "workbench.startupEditor": "none",
    "extensions.ignoreRecommendations": true,
  }),
);

try {
  await runTests({
    version: VSCODE_VERSION,
    cachePath: path.join(targetDir, "vscode-test"),
    extensionDevelopmentPath: extensionDir,
    extensionTestsPath: path.join(extensionDir, "test", "integration", "suite.js"),
    extensionTestsEnv: { INSET_REPO: repoDir, INSET_COMMAND: path.resolve(inset) },
    launchArgs: [
      path.join(repoDir, "examples", "hello-http"),
      `--user-data-dir=${userDataDir}`,
      `--extensions-dir=${path.join(userDataDir, "extensions")}`,
      "--disable-extensions",
      "--disable-workspace-trust",
      "--disable-gpu",
      // Otherwise VS Code takes its environment from a login shell, which
      // in the dev container image has no Rust toolchain on PATH, so
      // `inset test` could not build.
      "--force-disable-user-env",
      // Electron's sandbox refuses to start as root, which CI runs as.
      "--no-sandbox",
    ],
  });
} catch (error) {
  console.error(`Integration tests failed: ${error}`);
  process.exitCode = 1;
} finally {
  fs.rmSync(userDataDir, { recursive: true, force: true });
}
