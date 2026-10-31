//! A program of two modules, with structs, enums, `match` and `for`: its
//! generated Rust, pinned. Each Inset module is a Rust module of the same
//! name, so paths and privacy mean the same in both.

use std::path::Path;

use inset_check::Program;
use inset_codegen::generate;

#[test]
fn types_and_modules_rust() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../inset-host/tests/programs/types");
    let program = Program::load(&dir).expect("load the types program");
    let generated = generate(&program).expect("the types program compiles");
    assert_eq!(generated.package, "shelf");
    insta::assert_snapshot!(generated.source);
}
