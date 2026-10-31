# Inset

Inset is a programming language written by AI and reviewed by humans. Reviewers don't read the source. They review three generated artefacts:

- **Diagrams** of components and the data flowing between them, generated from the code, to judge structure.
- **Requirement tests** that open with the requirement in plain English, to judge correctness.
- **Coverage overlaid on the diagrams**, to find code that no requirement asked for.

The compiler is written in Rust and emits Rust. That Rust builds to WebAssembly components, which run on a host such as Wasmtime and get only the capabilities they are granted.

## Quickstart

You need [rustup](https://rustup.rs). These five commands install the `inset` command and review Hello HTTP the way Inset intends: by its requirements and its diagram, not its source.

```sh
cargo install --path crates/inset-cli                # put `inset` on PATH
inset check examples/hello-http                      # check the program; silent when it is sound
inset test examples/hello-http                       # run its requirements and list each with pass or fail
inset diagram examples/hello-http -o hello-http.svg  # draw its component diagram; open the SVG in a browser
inset run examples/hello-http                        # serve it, then open http://localhost:8080/hello/Ada
```

`inset test --json` prints the test report as JSON. `inset diagram --json` prints the diagram data. `inset diagram --flowchart HelloHttp/Greeter/fn:hello` draws one function's flowchart instead. `inset contract examples/hello-http` prints the program's API contract as OpenAPI 3.1, or with `--typescript` as TypeScript types for a front end. `inset build` writes both beside the component.

## Repository layout

| Directory | Contents |
| --- | --- |
| [`crates/`](crates/) | The compiler, the `inset` command, the language server, and the runtime and WebAssembly host that built programs run on |
| [`docs/language/`](docs/language/) | The language specification: syntax, diagrams and their data format, and the runtime |
| [`examples/`](examples/) | Example programs, each with its expected diagram data and pictures in `expected/` |
| [`editors/vscode/`](editors/vscode/) | The VS Code extension, which highlights Inset, starts the language server, and shows a program's diagram and requirements |
| [`xtask/`](xtask/) | Repository tasks run as `cargo xtask`, such as the CI gate and end-to-end runs |
| [`.devcontainer/`](.devcontainer/) | The dev container that CI also builds in |

## Building

You need [rustup](https://rustup.rs). `rust-toolchain.toml` selects the toolchain and installs the `wasm32-wasip2` target. The editor tests and packaging also need [Node.js](https://nodejs.org) 22 or later with npm, and the editor integration tests need a display or `xvfb-run`.

```sh
cargo xtask ci                 # format check, clippy, tests, editor tests, front-end type check
cargo xtask editor             # package the VS Code extension
cargo install --path crates/inset-cli   # put `inset` on PATH, for the editor
cargo xtask e2e hello          # build Hello HTTP, serve it and request a greeting
```

Or open the repo in its dev container (`.devcontainer/`), which any [Dev Container](https://containers.dev) client supports. In VS Code, a new dev container installs the `inset` command and the Inset extension, so Inset files are highlighted and checked from the start. CI runs `cargo xtask ci` in the same image.

See `AGENTS.md` for contribution rules.

## Licence

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT licence](LICENSE-MIT), at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this project, as defined in the Apache-2.0 licence, shall be dual licensed as above, without any additional terms or conditions.
