# Agent rules

Read this before changing anything in the repo.

## Before you finish

- Run `cargo xtask ci`. Work is not done until it passes. It runs format check, clippy with warnings denied, and every test.
- If a snapshot test (`insta`) changes, explain in the commit or PR why the new output is correct. Do not bulk-accept snapshots.

## Code

- Write idiomatic Rust. Any `unsafe` needs a written justification in the crate's `//!` docs.
- Put the reason for a non-obvious decision next to what it explains: the crate's `//!` docs, or the relevant page in `docs/language/`.
- Keep `docs/language/` in step with the compiler. It is the language specification.
- Change the grammars in `editors/vscode/` with the syntax, in the same commit.
- Compiler output (generated Rust, diagram JSON, API contracts) must be deterministic. Pin it with tests.

## Never commit

- Secrets or credentials.
- Local agent configuration, notes or plans. These are git-ignored; keep it that way.
- Attribution to an AI tool or vendor, in files, commit messages or PR descriptions. No co-author trailers, session links or "generated with" lines. Inset is for AI authors in general, not any one product.
