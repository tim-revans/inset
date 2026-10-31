# Inset for VS Code

Syntax highlighting for Inset code (`*.inset`) and requirement tests (`*.req.inset`), and for ```` ```inset ```` and ```` ```inset-req ```` blocks in Markdown. The extension also starts the Inset language server, which reports problems as you type and adds hover, go-to-definition, and an outline. Two panels let you review a program without reading its source: its diagram, and its requirements with their results.

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

## Reviewing a program

- Diagram: run **Inset: Show the diagram**, or press the diagram button at the top of an Inset file, to open the program's diagram beside the code. The picture is the one `inset diagram` draws, in a palette that follows your colour theme. Clicking an element opens its source in the editor beside the panel. Choose a function's flowchart in the **Show** list, or double-click the function, to see the flowchart. The panel redraws whenever you save an Inset file. Tick **Coverage** to mark what the requirements reached when they last ran from the Test Explorer: `✓` reached, `✗` not reached, `~` excused by `#[defensive]`, and `!` defensive yet reached. The panel redraws after each run. A program with problems gets them listed in the panel instead, and clicking one opens it.
- Requirements: the **Inset requirements** panel in the Explorer lists each program's requirements by their sentences, with whether each passed when last run. Under a failed requirement, the panel shows the step that did not hold and why. Clicking the step opens it in the editor. The panel's run button runs every requirement.
- Test Explorer: the same requirements appear in VS Code's Test Explorer, with a run button beside each `requirement` line. A requirement runs with `inset test`, which builds the program and runs all of its requirements, so running one reports every requirement in its program. If a program cannot be built, the program shows the error once, and each of its requirements points to that error. If a requirement reaches code marked `#[defensive]`, the program fails, with the code, its reason and the requirement.

To leave requirements files out of both lists, such as test fixtures, set `inset.requirements.exclude` to glob patterns. This repository's `.vscode/settings.json` leaves out the compiler's fixtures under `crates/`, because some of their requirements fail on purpose.

The diagram and the list of requirements come from the language server, so both need the `inset` command.

Highlighting works without the server. If the server cannot start, the extension shows a warning that says how to install the `inset` command.

## Tests

`npm test` tokenizes every example program and every Inset block in `docs/language/` and compares the result with `test/snapshots/`. It fails on any token the grammars leave unscoped. After a deliberate grammar or syntax change, run `npm test -- --update` and check the snapshot diff.

The grammars must change with the syntax: see `docs/language/syntax.md`.

`npm run test:integration` starts VS Code on this repository with this extension and opens each panel. The tests click `Greeter` in Hello HTTP's diagram and run every example's requirements from the Test Explorer. `INSET_COMMAND` names the `inset` binary under test. The first run downloads a pinned VS Code into the `vscode-test` directory under the Cargo target directory. `cargo xtask ci` runs the integration tests under `xvfb-run` when it is installed, as in CI, and on your desktop otherwise.
