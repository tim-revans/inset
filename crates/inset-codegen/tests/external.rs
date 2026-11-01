//! Pins what the compiler writes for an external component: its WIT world,
//! and the Rust adapter's `interface` crate, for the `externals` test
//! program's `WordCounter`, whose messages carry a struct, an enum with a
//! value in one variant, an `Option`, a `Result` and lent text.

use std::path::Path;

use inset_check::Program;

#[test]
fn word_counter() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../inset-host/tests/programs/externals");
    let program = Program::load(&dir).unwrap();
    let generated = inset_codegen::generate(&program).expect("the program compiles");
    let [external] = generated.externals.as_slice() else {
        panic!("one external component");
    };
    insta::assert_snapshot!("word_counter_wit", inset_codegen::world(external));
    insta::assert_snapshot!(
        "word_counter_interface",
        inset_codegen::interface_lib(external)
    );
}
