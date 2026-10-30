//! Pins the syntax tree of Hello HTTP, the example the parser covers.

use std::path::Path;

use inset_syntax::{parse_code, parse_requirements};

fn read(path: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/hello-http")
        .join(path);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn code() {
    let parsed = parse_code(&read("src/main.inset"));
    assert_eq!(parsed.diagnostics, []);
    insta::assert_debug_snapshot!(parsed.ast);
}

#[test]
fn requirements() {
    let parsed = parse_requirements(&read("requirements/hello.req.inset"));
    assert_eq!(parsed.diagnostics, []);
    insta::assert_debug_snapshot!(parsed.ast);
}
