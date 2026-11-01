//! External components, compiled from `tests/programs/externals`: `Counter`
//! sends messages to `WordCounter`, which is written in Rust and runs as a
//! WebAssembly component of its own.

mod common;

use common::{compile, text};
use inset_host::{Mode, Options, Report, Request, Running};

fn post(running: &mut Running<'_>, path: &str, body: &str) -> (u16, String) {
    let request = Request {
        method: "POST".into(),
        path: path.into(),
        body: body.as_bytes().to_vec(),
    };
    let response = running.request("http", &request).expect("the program runs");
    (
        response.status,
        String::from_utf8(response.body).expect("UTF-8"),
    )
}

fn options(working_dir: &std::path::Path) -> Options {
    Options {
        settings: Vec::new(),
        mode: Mode::Fake,
        working_dir: working_dir.to_path_buf(),
    }
}

#[test]
fn values_are_copied_into_and_out_of_the_external_component() {
    let program = compile("crates/inset-host/tests/programs/externals");
    let mut running = program.start(&options(std::path::Path::new("."))).unwrap();
    assert_eq!(
        post(&mut running, "/words", "a long word"),
        (
            200,
            "Word { text: \"a\", length: Short, after: None }\n\
             Word { text: \"long\", length: Long(4), after: Some(\"a\") }\n\
             Word { text: \"word\", length: Long(4), after: Some(\"long\") }\n"
                .into()
        )
    );
}

#[test]
fn a_panic_in_the_external_component_restarts_it() {
    let program = compile("crates/inset-host/tests/programs/externals");
    let mut running = program.start(&options(std::path::Path::new("."))).unwrap();
    assert_eq!(post(&mut running, "/count", "one two"), (200, "2".into()));
    assert_eq!(
        post(&mut running, "/count", "three panic"),
        (500, String::new())
    );
    // The parent's `Child(words) => restart` started it afresh.
    assert_eq!(post(&mut running, "/count", "four"), (200, "1".into()));
    let reports = running.reports();
    assert_eq!(reports.len(), 2, "{reports:?}");
    assert_eq!(reports[0].component, "Tally/Counter/words");
    assert_eq!(reports[0].failure, "Panic");
    assert_eq!(reports[0].restarted.as_deref(), Some("Tally/Counter/words"));
    assert!(
        reports[0].message.contains("asked to panic")
            && reports[0].message.ends_with("src/words.rs:16:13"),
        "the panic's own message and location: {}",
        reports[0].message
    );
    assert_eq!(
        reports[1],
        Report {
            component: "Tally/Counter".into(),
            failure: "Message(words)".into(),
            message: "Tally/Counter/words failed".into(),
            restarted: None,
        }
    );
}

#[test]
fn each_start_of_the_program_starts_its_external_components_afresh() {
    let program = compile("crates/inset-host/tests/programs/externals");
    let mut first = program.start(&options(std::path::Path::new("."))).unwrap();
    assert_eq!(post(&mut first, "/count", "one two"), (200, "2".into()));
    let mut second = program.start(&options(std::path::Path::new("."))).unwrap();
    assert_eq!(post(&mut second, "/count", "one"), (200, "1".into()));
    assert_eq!(post(&mut first, "/count", "three"), (200, "3".into()));
}

#[test]
fn the_host_refuses_an_external_component_that_opens_a_file() {
    let dir = std::env::temp_dir().join(format!("inset-externals-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("secret.txt"), "the secret").unwrap();
    let program = compile("crates/inset-host/tests/programs/externals");
    let mut running = program.start(&options(&dir)).unwrap();
    let reply = text(&mut running, "GET", "/file");
    std::fs::remove_dir_all(&dir).unwrap();
    // The file is there, but nothing is opened for the component, so it
    // finds no files at all.
    assert_eq!(
        reply,
        (
            200,
            "refused: No such file or directory (os error 44)".into()
        )
    );
}
