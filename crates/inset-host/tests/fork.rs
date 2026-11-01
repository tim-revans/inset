//! `fork for`, compiled from `tests/programs/fork`: branches that await a
//! child's messages run concurrently on the one thread, and every fork
//! gathers its values in the order of its collection.

mod common;

use common::{compile, fake, text};
use inset_host::{Request, Running};

fn ask(running: &mut Running<'_>, names: &str) -> (u16, String) {
    let request = Request {
        method: "POST".into(),
        path: "/ask".into(),
        body: names.as_bytes().to_vec(),
    };
    let response = running.request("http", &request).expect("running");
    (response.status, String::from_utf8(response.body).unwrap())
}

#[test]
fn branches_that_await_run_concurrently_and_keep_their_order() {
    let program = compile("crates/inset-host/tests/programs/fork");
    let mut running = program.start(&fake()).unwrap();
    // Every branch sends its first message before any sends its second, so
    // the clerk hears every question before any thanks.
    assert_eq!(
        ask(&mut running, "ada, bob, cy"),
        (200, "ada=1,bob=2,cy=3".into())
    );
    assert_eq!(
        text(&mut running, "GET", "/notes"),
        (
            200,
            "ada asks,bob asks,cy asks,ada thanks,bob thanks,cy thanks".into()
        )
    );
}

#[test]
fn a_fork_over_nothing_gives_nothing() {
    let program = compile("crates/inset-host/tests/programs/fork");
    let mut running = program.start(&fake()).unwrap();
    assert_eq!(ask(&mut running, ""), (200, String::new()));
    assert_eq!(text(&mut running, "POST", "/count/0"), (200, "0".into()));
    assert_eq!(text(&mut running, "POST", "/count/7"), (200, "7".into()));
}
