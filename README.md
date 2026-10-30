# Inset

Inset is a programming language written by AI and reviewed by humans. Reviewers don't read the source. They review three generated artefacts:

- **Diagrams** of components and the data flowing between them, generated from the code, to judge structure.
- **Requirement tests** that open with the requirement in plain English, to judge correctness.
- **Coverage overlaid on the diagrams**, to find code that no requirement asked for.

The compiler is written in Rust and emits Rust. That Rust builds to WebAssembly components, which run on a host such as Wasmtime and get only the capabilities they are granted.

## Status

Early scaffolding. Nothing compiles yet.

## Building

You need [rustup](https://rustup.rs). `rust-toolchain.toml` selects the toolchain and installs the `wasm32-wasip2` target.

```sh
cargo xtask ci    # format check, clippy, tests
```

Or open the repo in its dev container (`.devcontainer/`), which any [Dev Container](https://containers.dev) client supports. CI runs `cargo xtask ci` in the same image.

See `AGENTS.md` for contribution rules.

## Licence

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT licence](LICENSE-MIT), at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this project, as defined in the Apache-2.0 licence, shall be dual licensed as above, without any additional terms or conditions.
