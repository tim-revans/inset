//! `inset diagram` draws a program's diagram from its source.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn inset(dir: &Path, options: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_inset"))
        .arg("diagram")
        .arg(dir)
        .args(options)
        .output()
        .expect("run inset")
}

fn hello() -> PathBuf {
    repo().join("examples/hello-http")
}

fn expected(file: &str) -> String {
    std::fs::read_to_string(hello().join("expected").join(file)).unwrap()
}

fn stdout(out: &Output) -> String {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn hello_http_is_drawn_as_expected() {
    assert_eq!(stdout(&inset(&hello(), &[])), expected("diagram.svg"));
    assert_eq!(
        stdout(&inset(
            &hello(),
            &["--flowchart", "HelloHttp/Greeter/fn:hello"]
        )),
        expected("hello.svg")
    );
}

#[test]
fn json_is_the_diagram_data() {
    let emitted: serde_json::Value =
        serde_json::from_str(&stdout(&inset(&hello(), &["--json"]))).unwrap();
    let expected: serde_json::Value = serde_json::from_str(&expected("diagram.json")).unwrap();
    assert_eq!(emitted, expected);
}

#[test]
fn view_options_are_checked() {
    let out = inset(&hello(), &["--hide", "colours"]);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("unknown label `colours`"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
