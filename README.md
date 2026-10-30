# Inset

Inset is a programming language written by AI and reviewed by humans. Reviewers don't read the source. They review three generated artefacts:

- **Diagrams** of components and the data flowing between them, generated from the code, to judge structure.
- **Requirement tests** that open with the requirement in plain English, to judge correctness.
- **Coverage overlaid on the diagrams**, to find code that no requirement asked for.

The compiler is written in Rust and emits Rust. That Rust builds to WebAssembly components, which run on a host such as Wasmtime and get only the capabilities they are granted.

## Status

The syntax is being designed through example programs, the diagram data format is defined, and the runtime that built programs run on exists. The compiler has its parser, which reads Hello HTTP, but nothing compiles from Inset source yet.

- [`docs/language/syntax.md`](docs/language/syntax.md) is the syntax reference.
- [`crates/inset-syntax/`](crates/inset-syntax/) parses both dialects into a syntax tree, with diagnostics for malformed input: `cargo run -p inset-cli -- parse examples/hello-http/src/main.inset` prints the tree.
- [`docs/language/diagrams.md`](docs/language/diagrams.md) describes the generated diagrams and their data format, whose JSON Schema is [`diagram.schema.json`](docs/language/diagram.schema.json).
- [`crates/inset-diagram/`](crates/inset-diagram/) reads diagram data and draws it as SVG: `cargo run -p inset-diagram -- render examples/stock-tracker/expected/diagram.json`.
- [`docs/language/runtime.md`](docs/language/runtime.md) describes how a built program runs: the Rust the compiler will emit, [`crates/inset-runtime/`](crates/inset-runtime/), which that Rust links, and [`crates/inset-host/`](crates/inset-host/), the WebAssembly host that runs it with only the capabilities it is granted. `cargo xtask e2e hello-spike` serves Hello HTTP from its hand-written Rust and asks it for a greeting.
- [`examples/`](examples/) holds the example programs that pin the syntax down, each with its expected diagram data and pictures in `expected/`.
- [`editors/vscode/`](editors/vscode/) is a VS Code extension that highlights Inset code and requirements. `cargo xtask editor` packages it.

## Building

You need [rustup](https://rustup.rs). `rust-toolchain.toml` selects the toolchain and installs the `wasm32-wasip2` target. The editor grammar tests and packaging also need [Node.js](https://nodejs.org) 22 or later with npm.

```sh
cargo xtask ci                 # format check, clippy, tests, editor grammar tests
cargo xtask editor             # package the VS Code extension
cargo xtask e2e hello-spike    # serve Hello HTTP in the host and request a greeting
```

Or open the repo in its dev container (`.devcontainer/`), which any [Dev Container](https://containers.dev) client supports. In VS Code, a new dev container builds and installs the Inset extension, so Inset files are highlighted from the start. CI runs `cargo xtask ci` in the same image.

See `AGENTS.md` for contribution rules.

## Licence

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT licence](LICENSE-MIT), at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this project, as defined in the Apache-2.0 licence, shall be dual licensed as above, without any additional terms or conditions.
