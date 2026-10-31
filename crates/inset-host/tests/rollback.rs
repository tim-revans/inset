//! A failed handler is rolled back: its component's fields are restored, its
//! table writes undone and the messages it sent dropped, whether a capability
//! call failed, it panicked or it overflowed.

mod common;

use common::{fake, load, request, text};
use inset_host::{Report, Running};

fn state(running: &mut Running<'_>) -> (String, String, String) {
    (
        text(running, "GET", "/hits").1,
        text(running, "GET", "/log").1,
        text(running, "GET", "/audit").1,
    )
}

fn report(component: &str, failure: &str, message: &str) -> Report {
    Report {
        component: component.into(),
        failure: failure.into(),
        message: message.into(),
    }
}

#[test]
fn a_handler_that_completes_changes_everything() {
    let program = load("fixture-spike");
    let mut running = program.start(&fake()).unwrap();
    assert_eq!(text(&mut running, "POST", "/count/a"), (200, "a 1".into()));
    assert_eq!(text(&mut running, "POST", "/count/a"), (200, "a 2".into()));
    assert_eq!(text(&mut running, "GET", "/count/a"), (200, "2".into()));
    assert_eq!(
        running.table("counts").unwrap().get(b"a"),
        Ok(Some(b"2".to_vec()))
    );
    assert_eq!(
        state(&mut running),
        ("2".into(), "a 1,a 2".into(), String::new())
    );
}

#[test]
fn a_failed_capability_call_rolls_back_without_a_trap() {
    let program = load("fixture-spike");
    let mut running = program.start(&fake()).unwrap();
    text(&mut running, "POST", "/count/a");
    running.table("counts").unwrap().fails = true;
    assert_eq!(text(&mut running, "POST", "/count/a"), (500, String::new()));
    running.table("counts").unwrap().fails = false;
    assert_eq!(
        state(&mut running),
        ("1".into(), "a 1".into(), String::new())
    );
    assert_eq!(
        running.reports(),
        [report(
            "Spike/Front",
            "Capability(counts)",
            "the table failed, as the test asked"
        )]
    );
}

#[test]
fn a_panic_rolls_back_and_the_program_carries_on() {
    let program = load("fixture-spike");
    let mut running = program.start(&fake()).unwrap();
    text(&mut running, "POST", "/count/a");
    assert_eq!(
        text(&mut running, "POST", "/count/panic"),
        (500, String::new())
    );
    assert_eq!(
        state(&mut running),
        ("1".into(), "a 1".into(), String::new())
    );
    assert_eq!(running.table("counts").unwrap().get(b"panic"), Ok(None));
    assert_eq!(text(&mut running, "POST", "/count/a"), (200, "a 2".into()));
    let [panic] = running.reports() else {
        panic!("one report: {:?}", running.reports())
    };
    assert_eq!(
        (&*panic.component, &*panic.failure),
        ("Spike/Front", "Panic")
    );
    assert!(
        panic.message.starts_with("asked to panic at "),
        "{}",
        panic.message
    );
}

#[test]
fn an_overflow_is_a_failure_of_its_own() {
    let program = load("fixture-spike");
    let mut running = program.start(&fake()).unwrap();
    assert_eq!(
        text(&mut running, "POST", "/count/overflow"),
        (500, String::new())
    );
    assert_eq!(
        state(&mut running),
        ("0".into(), String::new(), String::new())
    );
    let [overflow] = running.reports() else {
        panic!("one report: {:?}", running.reports())
    };
    assert_eq!(overflow.failure, "Overflow");
    assert!(
        overflow.message.starts_with("attempt to add with overflow"),
        "{}",
        overflow.message
    );
}

#[test]
fn a_panic_in_a_child_rolls_back_the_handlers_awaiting_it() {
    let program = load("fixture-spike");
    let mut running = program.start(&fake()).unwrap();
    text(&mut running, "POST", "/count/a");
    assert_eq!(
        text(&mut running, "POST", "/count/audit"),
        (500, String::new())
    );
    assert_eq!(
        state(&mut running),
        ("1".into(), "a 1".into(), String::new())
    );
    let [audit] = running.reports() else {
        panic!("one report: {:?}", running.reports())
    };
    assert_eq!(
        (&*audit.component, &*audit.failure),
        ("Spike/Front/audit", "Panic")
    );
}

#[test]
fn a_panic_nothing_responds_to_stops_the_program() {
    let program = load("fixture-spike");
    let mut running = program.start(&fake()).unwrap();
    let Err(stopped) = request(&mut running, "POST", "/count/log") else {
        panic!("the program stops")
    };
    assert_eq!(
        (&*stopped.component, &*stopped.failure),
        ("Spike/Front/log", "Panic")
    );
    assert!(
        stopped
            .message
            .starts_with("the log refuses to check itself at "),
        "{}",
        stopped.message
    );
    assert_eq!(request(&mut running, "GET", "/hits"), Err(stopped));
}
