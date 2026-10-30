# Inset for VS Code

Syntax highlighting for Inset code (`*.inset`) and requirement tests (`*.req.inset`), and for ```` ```inset ```` and ```` ```inset-req ```` blocks in Markdown.

## Install

From the repository root:

```sh
cargo xtask editor
code --install-extension target/vscode/inset.vsix
```

`cargo xtask editor` needs Node.js and npm. It prints where it wrote the `.vsix`, which is under `$CARGO_TARGET_DIR` when that is set.

## What it highlights

- Code: Inset's keywords (`app`, `component`, `setting`, `grant`, `start`, `failure`), route attributes with their HTTP method, `{param}` segments and `via` capability, capability types, and boundary crossings: `move` and `&` where a value crosses into another component, `send` and `fork`.
- Requirements: the requirement sentence, as a bold heading so it stands out from the steps below it; the step keywords (`given`, `when`, `then`, `and`) and the dialect's fixed words (`receives`, `replies`, `with`, `is`, `empty`, `fails`); paths, statuses, `<placeholders>`, `"""` text blocks and `examples` tables, whose headings share the colour of the placeholders they name.

Whatever the colour theme, the requirement sentence is the only bold text in a requirements file, boundary crossings, `send` and `fork` are bold in code, and capabilities are underlined. These come from `editor.tokenColorCustomizations` defaults, so your own setting of it replaces them.

## Grammar tests

`npm test` tokenizes every example program and every Inset block in `docs/language/` and compares the result with `test/snapshots/`. It fails on any token the grammars leave unscoped. After a deliberate grammar or syntax change, run `npm test -- --update` and check the snapshot diff.

The grammars must change with the syntax: see `docs/language/syntax.md`.
