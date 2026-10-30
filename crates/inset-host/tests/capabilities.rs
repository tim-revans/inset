//! A program gets only what it is granted. These programs are written by
//! hand to reach past their grants, as Rust in an external component might.

mod common;

use common::{build, fake, load, text};
use inset_host::Program;

#[test]
fn calls_to_ungranted_capabilities_are_refused() {
    let program = load("fixture-sandbox");
    let mut running = program.start(&fake()).unwrap();
    let mut probe = |path| text(&mut running, "GET", path).1;
    // No directory is opened for the program, so it finds no files at all.
    let no_files = "refused: No such file or directory (os error 44)";
    assert_eq!(probe("/file"), no_files);
    assert_eq!(probe("/directory"), no_files);
    let no_network = "refused: Permission denied (os error 2)";
    assert_eq!(probe("/connect"), no_network);
    assert_eq!(probe("/listen"), no_network);
    assert_eq!(probe("/environment"), "0 variables");
    assert_eq!(
        probe("/table"),
        "refused: no table was granted as `secrets`"
    );
}

#[test]
fn an_import_the_host_does_not_provide_fails_at_instantiation() {
    let path = build("fixture-web-client");
    let error = Program::load(&path)
        .err()
        .expect("the program is refused")
        .to_string();
    assert!(
        error.contains("component imports instance `wasi:http/"),
        "{error}"
    );
}
