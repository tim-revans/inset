//! `inset parse` prints a syntax tree for a good file and fails with
//! diagnostics for a bad one.

use std::path::Path;
use std::process::Command;

fn inset_parse(path: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_inset"))
        .arg("parse")
        .arg(path)
        .output()
        .expect("run inset")
}

fn example(path: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/hello-http")
        .join(path)
}

#[test]
fn parses_hello_http() {
    for (path, root) in [
        ("src/main.inset", "Module {"),
        ("requirements/hello.req.inset", "RequirementsFile {"),
    ] {
        let out = inset_parse(&example(path));
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "{path}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(stdout.starts_with(root), "{path}: {stdout}");
        assert!(out.stderr.is_empty());
    }
}

#[test]
fn reports_errors() {
    let dir = std::env::temp_dir().join(format!("inset-cli-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("main.inset");
    std::fs::write(&file, "app HelloHttp {\n    setting port: u16 = 8080\n}\n").unwrap();
    let out = inset_parse(&file);
    std::fs::remove_dir_all(&dir).unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(
        stderr.contains("expected `;` after the setting"),
        "{stderr}"
    );
    assert!(stderr.contains("main.inset:2:25"), "{stderr}");
}
