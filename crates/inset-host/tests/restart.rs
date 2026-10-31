//! Restarting a child and a failure nothing responds to, compiled from
//! `tests/programs/restart`. `Front` restarts `tally`, which starts `part`,
//! whenever the tally fails.

mod common;

use common::{compile, fake, request, text};
use inset_host::{Report, Running, Stopped};

fn counts(running: &mut Running<'_>) -> (u16, String) {
    text(running, "GET", "/counts")
}

#[test]
fn a_restart_resets_the_child_and_its_children() {
    let program = compile("crates/inset-host/tests/programs/restart");
    let mut running = program.start(&fake()).unwrap();
    assert_eq!(text(&mut running, "POST", "/add/2"), (200, "2 1".into()));
    assert_eq!(text(&mut running, "POST", "/add/3"), (200, "5 2".into()));
    running.table("marks").unwrap().fails = true;
    assert_eq!(text(&mut running, "POST", "/add/4"), (500, String::new()));
    running.table("marks").unwrap().fails = false;
    // The tally and its part are as they started, and the program carries on.
    assert_eq!(counts(&mut running), (200, "0 0".into()));
    assert_eq!(text(&mut running, "POST", "/add/1"), (200, "1 1".into()));
    // The announcements from before the restart were delivered, and the
    // failed handler's was not.
    assert_eq!(
        text(&mut running, "GET", "/notes"),
        (200, "added 2,added 3,added 1".into())
    );
    assert_eq!(
        running.reports(),
        [
            Report {
                component: "Counters/Front/tally".into(),
                failure: "Capability(marks)".into(),
                message: "the table failed, as the test asked".into(),
                restarted: Some("Counters/Front/tally".into()),
            },
            Report {
                component: "Counters/Front".into(),
                failure: "Message(tally)".into(),
                message: "Counters/Front/tally failed".into(),
                restarted: None,
            },
        ]
    );
    assert_eq!(
        running.reports()[0].to_string(),
        "Counters/Front/tally restarted after Capability(marks): the table failed, as the test asked"
    );
}

#[test]
fn a_panic_restarts_the_child_in_a_fresh_instance() {
    let program = compile("crates/inset-host/tests/programs/restart");
    let mut running = program.start(&fake()).unwrap();
    assert_eq!(text(&mut running, "POST", "/add/2"), (200, "2 1".into()));
    assert_eq!(text(&mut running, "POST", "/add/13"), (500, String::new()));
    assert_eq!(counts(&mut running), (200, "0 0".into()));
    assert_eq!(text(&mut running, "GET", "/notes"), (200, "added 2".into()));
    // The table write of the handler that panicked is undone.
    assert_eq!(running.table("marks").unwrap().get(b"13"), Ok(None));
    let [panic] = running.reports() else {
        panic!("one report: {:?}", running.reports())
    };
    assert_eq!(
        (
            &*panic.component,
            &*panic.failure,
            panic.restarted.as_deref()
        ),
        (
            "Counters/Front/tally",
            "Panic",
            Some("Counters/Front/tally")
        )
    );
}

#[test]
fn a_message_waiting_in_a_mailbox_survives_a_panic() {
    let program = compile("crates/inset-host/tests/programs/restart");
    let mut running = program.start(&fake()).unwrap();
    assert_eq!(
        text(&mut running, "POST", "/announce"),
        (500, String::new())
    );
    // The tally's handler completed, so its announcement stands, though the
    // handler awaiting it panicked before the announcement was handled.
    assert_eq!(
        text(&mut running, "GET", "/notes"),
        (200, "announced".into())
    );
    let [panic] = running.reports() else {
        panic!("one report: {:?}", running.reports())
    };
    assert_eq!(
        (&*panic.component, &*panic.failure),
        ("Counters/Front", "Panic")
    );
}

#[test]
fn a_failure_nothing_responds_to_stops_the_program_and_names_where() {
    let program = compile("crates/inset-host/tests/programs/restart");
    let mut running = program.start(&fake()).unwrap();
    assert_eq!(
        text(&mut running, "POST", "/record/1"),
        (200, "recorded".into())
    );
    running.table("ledger").unwrap().fails = true;
    let stopped = Stopped {
        component: "Counters/Front".into(),
        failure: "Capability(ledger)".into(),
        message: "the table failed, as the test asked".into(),
    };
    assert_eq!(
        request(&mut running, "POST", "/record/2"),
        Err(stopped.clone())
    );
    assert_eq!(
        inset_host::describe(&stopped),
        "Counters/Front failed with Capability(ledger): the table failed, as the test asked"
    );
    // A stopped program answers nothing more.
    assert_eq!(request(&mut running, "GET", "/counts"), Err(stopped));
}
