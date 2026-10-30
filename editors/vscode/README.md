# Inset for VS Code

Syntax highlighting for Inset code (`*.inset`) and requirement tests (`*.req.inset`), and for ```` ```inset ```` and ```` ```inset-req ```` blocks in Markdown. The extension also starts the Inset language server, which reports problems as you type and adds hover, go-to-definition, and an outline.

## Install

From the repository root:

```sh
cargo install --path crates/inset-cli
cargo xtask editor
code --install-extension target/vscode/inset.vsix
```

The first command puts `inset` on `PATH`, and the extension runs `inset lsp` to start the server. To use another build, set `inset.server.path` to its path. The repository's dev container runs all three commands when it is created. After changing the extension or the compiler, run them again and reload the window, or run **Inset: Restart the language server** after reinstalling only the command.

`cargo xtask editor` needs Node.js and npm. It prints where it wrote the `.vsix`, which is under `$CARGO_TARGET_DIR` when that is set.

## What it highlights

- Code: Inset's keywords (`app`, `component`, `setting`, `grant`, `start`, `failure`), route attributes with their HTTP method, `{param}` segments and `via` capability, capability types, and boundary crossings: `move` and `&` where a value crosses into another component, `send` and `fork`.
- Requirements: the requirement sentence, as a bold heading so it stands out from the steps below it; the step keywords (`given`, `when`, `then`, `and`) and the dialect's fixed words (`receives`, `replies`, `with`, `is`, `empty`, `fails`); paths, statuses, `<placeholders>`, `"""` text blocks and `examples` tables, whose headings share the colour of the placeholders they name.

Whatever the colour theme, the requirement sentence is the only bold text in a requirements file, boundary crossings, `send` and `fork` are bold in code, and capabilities are underlined. These come from `editor.tokenColorCustomizations` defaults, so your own setting of it replaces them.

## What the language server adds

- Diagnostics: the problems `inset check` reports, with the same message, label and help, updated as you type. A program is checked as a whole, so renaming a route in `src/main.inset` marks the requirement steps that reach it. A `.inset` file outside a program, which has no `src/main.inset` above it, gets its parse errors only.
- Hover: the declaration, doc comment and type of a name; for a component, the capabilities it holds, the routes it serves and the messages it takes.
- Go to definition: a requirement step leads to the handler of its route, the app's name in a step leads to the app, and a name in code leads to its declaration.
- Outline: the app with its settings, grants, and children; each component with its fields, children, and handlers; and each requirement by its sentence.

Highlighting works without the server. If the server cannot start, the extension shows a warning that says how to install the `inset` command.

## Grammar tests

`npm test` tokenizes every example program and every Inset block in `docs/language/` and compares the result with `test/snapshots/`. It fails on any token the grammars leave unscoped. After a deliberate grammar or syntax change, run `npm test -- --update` and check the snapshot diff.

The grammars must change with the syntax: see `docs/language/syntax.md`.
