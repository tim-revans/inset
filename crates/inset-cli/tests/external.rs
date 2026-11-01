//! External components through the command line: CSV import's `CsvReader`
//! built from its Rust file, rustc's error for a file that does not match
//! its declaration, and `inset.lock`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Mutex;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `inset` building in a target directory apart from the one `cargo test`
/// holds locked, shared with the other tests that build programs.
fn inset(args: &[&str], dir: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_inset"));
    command.arg(args[0]).arg(dir).args(&args[1..]);
    command.env("CARGO_TARGET_DIR", target());
    command
}

fn target() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map_or_else(|| repo().join("target"), PathBuf::from)
        .join("wasm-tests")
}

/// Runs `inset`, one at a time: every copy of CSV import has the same
/// package names, and so the same files in the shared target directory.
fn run(command: &mut Command) -> Output {
    static BUILDING: Mutex<()> = Mutex::new(());
    let _building = BUILDING.lock().unwrap_or_else(|e| e.into_inner());
    command.output().expect("run inset")
}

/// A copy of CSV import, with no `inset.lock` unless `locked`.
fn csv_import(name: &str, locked: bool) -> PathBuf {
    let example = repo().join("examples/csv-import");
    let dir = std::env::temp_dir().join(format!("inset-external-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for sub in ["src", "requirements"] {
        std::fs::create_dir_all(dir.join(sub)).unwrap();
        for entry in std::fs::read_dir(example.join(sub)).unwrap() {
            let path = entry.unwrap().path();
            std::fs::copy(&path, dir.join(sub).join(path.file_name().unwrap())).unwrap();
        }
    }
    if locked {
        std::fs::copy(example.join("inset.lock"), dir.join("inset.lock")).unwrap();
    }
    dir
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn a_rust_file_that_does_not_match_its_declaration_gets_rustcs_error() {
    let dir = csv_import("mismatch", true);
    let reader = dir.join("src/csv_reader.rs");
    let text = std::fs::read_to_string(&reader).unwrap();
    std::fs::write(
        &reader,
        text.replace("fn read(&self, text: &str)", "fn read(&self, text: String)"),
    )
    .unwrap();
    let out = run(&mut inset(&["build"], &dir));
    assert!(!out.status.success());
    let shown = stderr(&out).replace(&format!("{}/", dir.display()), "");
    insta::assert_snapshot!(shown);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The version of `name` a lock file holds.
fn locked_version(lock: &str, name: &str) -> String {
    let entry = format!("name = \"{name}\"\nversion = \"");
    let start = lock
        .find(&entry)
        .unwrap_or_else(|| panic!("{name} is locked"))
        + entry.len();
    lock[start..].split('"').next().unwrap().to_string()
}

#[test]
fn inset_lock_is_written_once_and_honoured() {
    let dir = csv_import("lock", false);
    let out = run(&mut inset(&["build"], &dir));
    assert!(out.status.success(), "{}", stderr(&out));
    let first = std::fs::read_to_string(dir.join("inset.lock")).unwrap();
    // The example's own lock is what a first build writes.
    let committed = std::fs::read_to_string(repo().join("examples/csv-import/inset.lock")).unwrap();
    assert_eq!(first, committed);
    let out = run(&mut inset(&["build"], &dir));
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        std::fs::read_to_string(dir.join("inset.lock")).unwrap(),
        first
    );

    // Pin an older `csv` than the newest 1.x, as a person updating the lock
    // would, and build again: the build keeps it.
    let newest = locked_version(&first, "csv");
    let older = "1.3.0";
    assert_ne!(newest, older);
    let externals = dir.join("target/inset/externals");
    let status = Command::new(env!("CARGO"))
        .args(["update", "--quiet", "--package", "csv", "--precise", older])
        .current_dir(&externals)
        .status()
        .unwrap();
    assert!(status.success());
    std::fs::copy(externals.join("Cargo.lock"), dir.join("inset.lock")).unwrap();
    let out = run(&mut inset(&["build"], &dir));
    assert!(out.status.success(), "{}", stderr(&out));
    let lock = std::fs::read_to_string(dir.join("inset.lock")).unwrap();
    assert_eq!(locked_version(&lock, "csv"), older);
    let used = std::fs::read_to_string(externals.join("Cargo.lock")).unwrap();
    assert_eq!(locked_version(&used, "csv"), older);
    std::fs::remove_dir_all(&dir).unwrap();
}
