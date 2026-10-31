//! `inset test` runs a program's requirements against what it builds, and
//! reports each by its sentence, as text or as JSON.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Mutex;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `inset` building in a target directory apart from the one `cargo test`
/// holds locked, shared with the other tests that build programs.
fn inset(args: &[&str], dir: &Path) -> Command {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map_or_else(|| repo().join("target"), PathBuf::from)
        .join("wasm-tests");
    let mut command = Command::new(env!("CARGO_BIN_EXE_inset"));
    command.arg(args[0]).arg(dir).args(&args[1..]);
    command.env("CARGO_TARGET_DIR", target);
    command
}

/// A copy of the program in `dir`, relative to the repository, so building
/// it writes nothing into the source tree.
fn copy(dir: &str, name: &str) -> PathBuf {
    let to = std::env::temp_dir().join(format!("inset-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&to);
    for sub in ["src", "requirements", "web"] {
        let from = repo().join(dir).join(sub);
        if from.is_dir() {
            copy_dir(&from, &to.join(sub));
        }
    }
    to
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let path = entry.unwrap().path();
        let target = to.join(path.file_name().unwrap());
        if path.is_dir() {
            copy_dir(&path, &target);
        } else {
            std::fs::copy(&path, target).unwrap();
        }
    }
}

/// Runs `inset`, one at a time. Programs built from copies of one example
/// share a package name, and so the file Cargo writes the component to.
fn run(command: &mut Command) -> Output {
    static BUILDING: Mutex<()> = Mutex::new(());
    let _building = BUILDING.lock().unwrap_or_else(|e| e.into_inner());
    command.output().expect("run inset")
}

/// Standard output, with each line number in the generated Rust that a
/// panic reports replaced, so a change to the generated code does not
/// change the snapshot.
fn stdout(out: &Output) -> String {
    let text = String::from_utf8_lossy(&out.stdout);
    let mut parts = text.split("src/lib.rs:");
    let mut cleaned = parts.next().unwrap_or_default().to_string();
    for part in parts {
        cleaned.push_str("src/lib.rs:N");
        cleaned.push_str(part.trim_start_matches(|c: char| c.is_ascii_digit()));
    }
    cleaned
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn hello_http_meets_its_requirement() {
    let dir = copy("examples/hello-http", "hello");
    let out = run(&mut inset(&["test"], &dir));
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "pass  Visitors are greeted by the name in the web address\n\n1 requirement passed\n"
    );
}

#[test]
fn an_error_enum_gives_each_requirement_its_status() {
    let dir = copy("crates/inset-host/tests/programs/types", "types");
    let out = run(&mut inset(&["test"], &dir));
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    insta::assert_snapshot!("types_text", stdout(&out));
}

#[test]
fn a_broken_hello_http_fails_its_requirement() {
    let dir = copy("examples/hello-http", "broken-hello");
    let main = dir.join("src/main.inset");
    let source = std::fs::read_to_string(&main).unwrap();
    std::fs::write(&main, source.replace("Hello, {name}!", "Hi, {name}!")).unwrap();
    let out = run(&mut inset(&["test"], &dir));
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(!out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert_eq!(
        stdout(&out),
        "\
FAIL  Visitors are greeted by the name in the web address
      requirements/hello.req.inset:3
      then HelloHttp replies OK with \"Hello, Ada!\"
      but the reply was OK with \"Hi, Ada!\"

1 requirement failed
"
    );
}

#[test]
fn failures_name_the_step_and_what_happened() {
    let dir = copy("crates/inset-cli/tests/programs/greetings", "greetings");
    let text = run(&mut inset(&["test"], &dir));
    let json = run(&mut inset(&["test", "--json"], &dir));
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(text.status.code(), Some(1), "{}", stderr(&text));
    assert_eq!(json.status.code(), Some(1), "{}", stderr(&json));
    insta::assert_snapshot!("greetings_text", stdout(&text));
    insta::assert_snapshot!("greetings_json", stdout(&json));
}

#[test]
fn a_program_with_errors_is_not_tested() {
    let dir = copy("examples/hello-http", "refused");
    let path = dir.join("requirements/hello.req.inset");
    let source = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, source.replace("/hello/Ada", "/goodbye/Ada")).unwrap();
    let out = run(&mut inset(&["test"], &dir));
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
    assert!(
        stderr(&out).contains("no route of `HelloHttp` takes `GET /goodbye/Ada`"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn facts_set_up_and_check_what_tables_hold() {
    for (program, name) in [
        ("crates/inset-host/tests/programs/stock-room", "stock-room"),
        ("crates/inset-host/tests/programs/ordered", "ordered"),
    ] {
        let dir = copy(program, name);
        let out = run(&mut inset(&["test"], &dir));
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
        insta::assert_snapshot!(format!("{}_text", name.replace('-', "_")), stdout(&out));
    }
}

/// A copy of the stock room whose handlers store the wrong thing, while
/// replying as they should, so only the facts and checks after the replies
/// find it out.
#[test]
fn a_fact_that_fails_shows_both_values() {
    let dir = copy("crates/inset-host/tests/programs/stock-room", "broken-room");
    let main = dir.join("src/main.inset");
    let source = std::fs::read_to_string(&main).unwrap();
    let broken = source
        .replace(
            "Item { on_hand: 0, price, place",
            "Item { on_hand: 1, price, place",
        )
        .replace("Place::Shelf(shelf);", "Place::Shelf(shelf + 1);")
        .replace("self.items.remove(&sku)", "self.items.get(&sku)");
    assert_ne!(source, broken);
    std::fs::write(&main, broken).unwrap();
    let text = run(&mut inset(&["test"], &dir));
    let json = run(&mut inset(&["test", "--json"], &dir));
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(text.status.code(), Some(1), "{}", stderr(&text));
    insta::assert_snapshot!("broken_room_text", stdout(&text));
    insta::assert_snapshot!("broken_room_json", stdout(&json));
}

/// Each row of the Stock tracker's examples table is reported on a line of
/// its own, under its requirement, and in the JSON report.
#[test]
fn the_stock_tracker_meets_its_requirements_row_by_row() {
    let dir = copy("examples/stock-tracker", "stock-tracker");
    let text = run(&mut inset(&["test"], &dir));
    let json = run(&mut inset(&["test", "--json"], &dir));
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(text.status.success(), "{}{}", stdout(&text), stderr(&text));
    insta::assert_snapshot!("stock_tracker_text", stdout(&text));
    insta::assert_snapshot!("stock_tracker_json", stdout(&json));
}

/// A copy of the Stock tracker that reorders only below the reorder level,
/// not at it, which one row of the examples table finds out.
#[test]
fn a_failing_row_is_reported_on_its_own() {
    let dir = copy("examples/stock-tracker", "broken-stock-tracker");
    let path = dir.join("src/warehouse.inset");
    let source = std::fs::read_to_string(&path).unwrap();
    let broken = source.replace("if on_hand > reorder_level", "if on_hand >= reorder_level");
    assert_ne!(source, broken);
    std::fs::write(&path, broken).unwrap();
    let text = run(&mut inset(&["test"], &dir));
    let json = run(&mut inset(&["test", "--json"], &dir));
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(text.status.code(), Some(1), "{}", stderr(&text));
    insta::assert_snapshot!("broken_stock_tracker_text", stdout(&text));
    insta::assert_snapshot!("broken_stock_tracker_json", stdout(&json));
}

/// A child that fails is restarted with its own children, and the
/// requirement sees it start again from nothing.
#[test]
fn a_restarted_child_starts_again_from_nothing() {
    let dir = copy("crates/inset-host/tests/programs/restart", "restart");
    let out = run(&mut inset(&["test"], &dir));
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert_eq!(
        stdout(&out),
        "pass  A tally that fails starts again from nothing, with its part\n\n1 requirement passed\n"
    );
}

/// Requirements send JSON bodies written as values and compare JSON
/// replies as values. They pass only if the test runner writes and reads
/// JSON as the program's own serde derives do.
#[test]
fn json_bodies_and_replies_are_values() {
    let dir = copy("crates/inset-host/tests/programs/catalogue", "catalogue");
    let out = run(&mut inset(&["test"], &dir));
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    insta::assert_snapshot!("catalogue_text", stdout(&out));
}

/// A copy of the catalogue that lists every product when asked for parts,
/// so a JSON reply differs from the value a requirement expects.
#[test]
fn a_json_reply_that_differs_shows_both_values() {
    let dir = copy(
        "crates/inset-host/tests/programs/catalogue",
        "broken-catalogue",
    );
    let main = dir.join("src/main.inset");
    let source = std::fs::read_to_string(&main).unwrap();
    let broken = source.replace("parts != Some(true) ||", "parts != Some(false) ||");
    assert_ne!(source, broken);
    std::fs::write(&main, broken).unwrap();
    let text = run(&mut inset(&["test"], &dir));
    let json = run(&mut inset(&["test", "--json"], &dir));
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(text.status.code(), Some(1), "{}", stderr(&text));
    insta::assert_snapshot!("broken_catalogue_text", stdout(&text));
    insta::assert_snapshot!("broken_catalogue_json", stdout(&json));
}
