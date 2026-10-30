// Grammar snapshot tests.
//
// Tokenizes every example program and every ```inset and ```inset-req block in
// docs/language/ with the extension's grammars, the same way VS Code does, and
// compares the scopes with the snapshots in test/snapshots/. Also fails on any
// token the grammars leave unscoped, and checks that the Markdown injection
// highlights each fenced block exactly as its own grammar does.
//
// Run with `--update` to rewrite the snapshots after a deliberate change.

import fs from "node:fs";
import path from "node:path";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import vsctm from "vscode-textmate";
import oniguruma from "vscode-oniguruma";

const require = createRequire(import.meta.url);
const extensionDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const repoDir = path.resolve(extensionDir, "../..");
const snapshotDir = path.join(extensionDir, "test", "snapshots");
const update = process.argv.includes("--update");

const CODE = "source.inset";
const REQUIREMENTS = "source.inset-req";
const MARKDOWN = "text.html.markdown";
const FENCE_SCOPES = { inset: CODE, "inset-req": REQUIREMENTS };

const grammarFiles = {
  [CODE]: "syntaxes/inset.tmLanguage.json",
  [REQUIREMENTS]: "syntaxes/inset-req.tmLanguage.json",
  "markdown.inset.codeblock": "syntaxes/inset-markdown.tmLanguage.json",
};

async function loadRegistry() {
  const wasm = fs.readFileSync(require.resolve("vscode-oniguruma/release/onig.wasm"));
  await oniguruma.loadWASM(wasm.buffer);
  return new vsctm.Registry({
    onigLib: Promise.resolve({
      createOnigScanner: (sources) => new oniguruma.OnigScanner(sources),
      createOnigString: (text) => new oniguruma.OnigString(text),
    }),
    async loadGrammar(scopeName) {
      if (scopeName === MARKDOWN) {
        // Stands in for VS Code's Markdown grammar, so only the injection
        // decides how fenced blocks are highlighted.
        return vsctm.parseRawGrammar(JSON.stringify({ scopeName, patterns: [] }), "markdown.json");
      }
      const file = grammarFiles[scopeName];
      if (!file) return null;
      const fullPath = path.join(extensionDir, file);
      return vsctm.parseRawGrammar(fs.readFileSync(fullPath, "utf8"), fullPath);
    },
    getInjections: (scopeName) => (scopeName === MARKDOWN ? ["markdown.inset.codeblock"] : undefined),
  });
}

/** Tokenizes `lines`, returning each line's tokens as { start, end, scopes }. */
function tokenize(grammar, lines) {
  let state = vsctm.INITIAL;
  return lines.map((line) => {
    const result = grammar.tokenizeLine(line, state);
    state = result.ruleStack;
    return result.tokens.map((token) => ({
      start: token.startIndex,
      end: Math.min(token.endIndex, line.length),
      scopes: token.scopes,
    }));
  });
}

/** Renders tokens in the format of vscode-tmgrammar-snap: each line, then a caret line per token. */
function render(lines, tokens) {
  const out = [];
  lines.forEach((line, i) => {
    out.push(">" + line);
    for (const token of tokens[i]) {
      if (line.slice(token.start, token.end).trim() === "") continue;
      const carets = " ".repeat(token.start) + "^".repeat(token.end - token.start);
      out.push("#" + carets + " " + token.scopes.slice(1).join(" "));
    }
  });
  return out.join("\n") + "\n";
}

/** Every non-blank token needs a scope of its own, beyond the grammar's root scope. */
function unscoped(name, firstLine, lines, tokens) {
  const found = [];
  lines.forEach((line, i) => {
    for (const token of tokens[i]) {
      const text = line.slice(token.start, token.end);
      if (text.trim() !== "" && token.scopes.length === 1) {
        found.push(`${name}:${firstLine + i}:${token.start + 1}: unscoped token ${JSON.stringify(text)}`);
      }
    }
  });
  return found;
}

function walk(dir) {
  return fs
    .readdirSync(dir, { withFileTypes: true })
    .flatMap((entry) => {
      const full = path.join(dir, entry.name);
      return entry.isDirectory() ? walk(full) : [full];
    })
    .sort();
}

function lines(text) {
  return text.replace(/\n$/, "").split("\n");
}

/** The fenced Inset blocks in a Markdown file, each with the heading it sits under. */
function fencedBlocks(text) {
  const blocks = [];
  let heading = "";
  let open = null;
  lines(text).forEach((line, i) => {
    if (open) {
      if (line.trim() === open.fence) {
        blocks.push(open);
        open = null;
      } else {
        open.lines.push(line);
      }
      return;
    }
    const headingMatch = /^#+\s+(.*)$/.exec(line);
    if (headingMatch) heading = headingMatch[1];
    const fence = /^(`{3,}|~{3,})\s*(\S+)?\s*$/.exec(line);
    if (fence && FENCE_SCOPES[fence[2]]) {
      open = { fence: fence[1], language: fence[2], heading, firstLine: i + 2, lines: [] };
    } else if (fence) {
      open = { fence: fence[1], language: null, heading, firstLine: i + 2, lines: [] };
    }
  });
  return blocks.filter((block) => block.language);
}

async function main() {
  const registry = await loadRegistry();
  const grammars = {};
  for (const scope of [CODE, REQUIREMENTS, MARKDOWN]) {
    grammars[scope] = await registry.loadGrammar(scope);
  }

  const failures = [];
  const snapshots = new Map();

  for (const file of walk(path.join(repoDir, "examples")).filter((f) => f.endsWith(".inset"))) {
    const rel = path.relative(repoDir, file);
    const scope = file.endsWith(".req.inset") ? REQUIREMENTS : CODE;
    const source = lines(fs.readFileSync(file, "utf8"));
    const tokens = tokenize(grammars[scope], source);
    failures.push(...unscoped(rel, 1, source, tokens));
    snapshots.set(rel + ".snap", render(source, tokens));
  }

  for (const file of walk(path.join(repoDir, "docs", "language")).filter((f) => f.endsWith(".md"))) {
    const rel = path.relative(repoDir, file);
    const text = fs.readFileSync(file, "utf8");
    const blocks = fencedBlocks(text);
    if (blocks.length === 0) continue;

    const markdownTokens = tokenize(grammars[MARKDOWN], lines(text));
    const parts = [];
    for (const block of blocks) {
      const tokens = tokenize(grammars[FENCE_SCOPES[block.language]], block.lines);
      failures.push(...unscoped(rel, block.firstLine, block.lines, tokens));
      parts.push(`## ${block.heading} (${block.language})\n` + render(block.lines, tokens));
      failures.push(...compareInjected(rel, block, tokens, markdownTokens));
    }
    snapshots.set(rel + ".snap", parts.join("\n"));
  }

  failures.push(...compareSnapshots(snapshots));

  if (failures.length > 0) {
    for (const failure of failures) console.error(failure);
    console.error(`\n${failures.length} grammar test failure(s).`);
    process.exit(1);
  }
  console.log(`Grammar tests passed: ${snapshots.size} snapshot file(s).`);
}

/** A fenced block in Markdown must get the same tokens as the block on its own. */
function compareInjected(rel, block, tokens, markdownTokens) {
  const embedded = `meta.embedded.block.${block.language}`;
  const failures = [];
  block.lines.forEach((line, i) => {
    const lineNumber = block.firstLine + i;
    const inMarkdown = markdownTokens[lineNumber - 1]
      .filter((token) => line.slice(token.start, token.end).trim() !== "")
      .map((token) => {
        const at = token.scopes.indexOf(embedded);
        return at === -1 ? null : `${token.start}-${token.end} ${token.scopes.slice(at + 1).join(" ")}`;
      });
    const alone = tokens[i]
      .filter((token) => line.slice(token.start, token.end).trim() !== "")
      .map((token) => `${token.start}-${token.end} ${token.scopes.slice(1).join(" ")}`);
    if (inMarkdown.join("\n") !== alone.join("\n")) {
      failures.push(`${rel}:${lineNumber}: highlighted differently in Markdown than in a ${block.language} file`);
    }
  });
  return failures;
}

function compareSnapshots(snapshots) {
  const failures = [];
  const existing = fs.existsSync(snapshotDir)
    ? walk(snapshotDir).map((file) => path.relative(snapshotDir, file))
    : [];
  for (const [rel, actual] of snapshots) {
    const file = path.join(snapshotDir, rel);
    const expected = fs.existsSync(file) ? fs.readFileSync(file, "utf8") : null;
    if (expected === actual) continue;
    if (update) {
      fs.mkdirSync(path.dirname(file), { recursive: true });
      fs.writeFileSync(file, actual);
      console.log(`updated ${path.relative(extensionDir, file)}`);
    } else if (expected === null) {
      failures.push(`${rel}: no snapshot (run \`npm test -- --update\` in editors/vscode)`);
    } else {
      failures.push(`${rel}: snapshot differs\n${firstDifference(expected, actual)}`);
    }
  }
  for (const rel of existing.filter((rel) => !snapshots.has(rel))) {
    if (update) {
      fs.rmSync(path.join(snapshotDir, rel));
      console.log(`removed stale ${rel}`);
    } else {
      failures.push(`${rel}: stale snapshot, its source no longer exists`);
    }
  }
  return failures;
}

function firstDifference(expected, actual) {
  const a = expected.split("\n");
  const b = actual.split("\n");
  for (let i = 0; i < Math.max(a.length, b.length); i++) {
    if (a[i] !== b[i]) {
      return `  line ${i + 1}\n  - ${a[i] ?? "(end of file)"}\n  + ${b[i] ?? "(end of file)"}`;
    }
  }
  return "";
}

await main();
