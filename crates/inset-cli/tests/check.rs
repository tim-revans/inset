//! `inset check` passes a sound program silently and fails with
//! diagnostics for a broken one.

use std::path::Path;
use std::process::Command;

fn inset_check(dir: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_inset"))
        .arg("check")
        .arg(dir)
        .output()
        .expect("run inset")
}

#[test]
fn checks_hello_http() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/hello-http");
    let out = inset_check(&dir);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stdout.is_empty() && out.stderr.is_empty());
}

#[test]
fn reports_errors() {
    let dir = std::env::temp_dir().join(format!("inset-cli-check-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("requirements")).unwrap();
    std::fs::write(
        dir.join("src/main.inset"),
        "app Hello {\n    start Greeter {};\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("requirements/hello.req.inset"),
        "requirement \"Visitors are greeted\"\n    when Hello receives GET /hello\n    then Hello replies OK\n",
    )
    .unwrap();
    let out = inset_check(&dir);
    std::fs::remove_dir_all(&dir).unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(stderr.contains("no component `Greeter`"), "{stderr}");
    assert!(stderr.contains("src/main.inset:2:11"), "{stderr}");
    assert!(stderr.contains("`Hello` has no routes"), "{stderr}");
}

#[test]
fn reports_a_missing_program() {
    let out = inset_check(Path::new("no-such-program"));
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("has no src/main.inset"));
}
