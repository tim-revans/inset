//! Every example that checks clean builds to a WebAssembly component, and
//! `rustc` has nothing to say about its Rust. An error or a warning in
//! generated Rust is a compiler bug.

use std::path::{Path, PathBuf};

use inset_check::{Program, check};
use inset_codegen::build;

#[test]
fn every_checked_example_builds_without_rust_errors_or_warnings() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    // A target directory apart from the one `cargo test` holds locked.
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map_or_else(|| repo.join("target"), PathBuf::from)
        .join("wasm-tests");
    let mut examples: Vec<_> = std::fs::read_dir(repo.join("examples"))
        .expect("read examples")
        .map(|e| e.expect("read examples").path())
        .collect();
    examples.sort();
    let mut built = Vec::new();
    for dir in examples {
        let program = Program::load(&dir).expect("load example");
        if !check(&program).is_empty() {
            continue;
        }
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        let out = target.join("inset").join(&name);
        let result =
            build(&program, &out, Some(&target)).unwrap_or_else(|e| panic!("{name} builds: {e}"));
        assert!(result.component.is_file(), "{name}: no component");
        assert_eq!(result.warnings, "", "{name}: rustc warned");
        built.push(name);
    }
    // The examples the compiler covers so far. Another example joins this
    // list when it checks clean.
    assert_eq!(built, ["hello-http", "stock-tracker"]);
}
