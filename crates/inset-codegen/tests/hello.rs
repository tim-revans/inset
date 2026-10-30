//! Hello HTTP's generated Rust and Cargo manifest, pinned.

use std::path::Path;

use inset_check::Program;
use inset_codegen::{cargo_toml, generate};

fn hello() -> Program {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/hello-http");
    Program::load(&dir).expect("load Hello HTTP")
}

#[test]
fn hello_http_rust() {
    let generated = generate(&hello()).expect("Hello HTTP compiles");
    assert_eq!(generated.package, "hello-http");
    insta::assert_snapshot!(generated.source);
}

#[test]
fn hello_http_manifest() {
    insta::assert_snapshot!(cargo_toml(
        "hello-http",
        Path::new("/inset/crates/inset-runtime")
    ));
}
