//! Tables, compiled from `tests/programs/stock-room` and
//! `tests/programs/ordered`: every table method, rollback of a handler's
//! writes and its component's state, and real SQLite files that last from
//! one run to the next.

mod common;

use std::path::{Path, PathBuf};

use common::{compile, fake, request, text};
use inset_host::{Mode, Options, Program, Request, Running};

const BOLT: &[u8] = br#"{"on_hand":3,"place":"Back","price":"0.10"}"#;

fn send(running: &mut Running<'_>, method: &str, path: &str, body: &str) -> (u16, String) {
    let request = Request {
        method: method.into(),
        path: path.into(),
        body: body.as_bytes().to_vec(),
    };
    let response = running.request("http", &request).expect("the program runs");
    (response.status, String::from_utf8(response.body).unwrap())
}

fn stored(running: &Running<'_>, table: &str, key: &[u8]) -> Option<String> {
    let value = running.table(table).unwrap().get(key).unwrap()?;
    Some(String::from_utf8(value).unwrap())
}

#[test]
fn a_handler_reads_writes_and_removes_entries() {
    let program = compile("crates/inset-host/tests/programs/stock-room");
    let mut running = program.start(&fake()).unwrap();
    assert_eq!(
        send(&mut running, "PUT", "/items/nut-m6", "0.05"),
        (200, "stocked".into())
    );
    running
        .table("items")
        .unwrap()
        .set(b"bolt-m6", BOLT)
        .unwrap();
    assert_eq!(
        send(&mut running, "POST", "/items/nut-m6/receipts", "20"),
        (200, "20".into())
    );
    assert_eq!(
        send(&mut running, "POST", "/items/bolt-m6/shelf", "4"),
        (200, "shelf 4".into())
    );
    assert_eq!(
        stored(&running, "items", b"bolt-m6").as_deref(),
        Some(r#"{"on_hand":3,"place":{"Shelf":[4]},"price":"0.10"}"#)
    );
    assert_eq!(
        text(&mut running, "GET", "/items"),
        (200, "bolt-m6 3 0.30\nnut-m6 20 1.00\n".into())
    );
    assert_eq!(
        send(&mut running, "DELETE", "/items/bolt-m6", ""),
        (200, "3".into())
    );
    assert_eq!(stored(&running, "items", b"bolt-m6"), None);
    assert_eq!(
        text(&mut running, "GET", "/items/bolt-m6"),
        (404, String::new())
    );
    assert_eq!(text(&mut running, "GET", "/writes"), (200, "4".into()));
}

#[test]
fn a_failed_table_call_rolls_back_the_writes_and_the_state() {
    let program = compile("crates/inset-host/tests/programs/stock-room");
    let mut running = program.start(&fake()).unwrap();
    running
        .table("items")
        .unwrap()
        .set(b"bolt-m6", BOLT)
        .unwrap();
    running.table("items").unwrap().fails = true;
    assert_eq!(
        send(&mut running, "POST", "/items/bolt-m6/receipts", "20"),
        (500, String::new())
    );
    assert_eq!(
        stored(&running, "items", b"bolt-m6").as_deref(),
        Some(std::str::from_utf8(BOLT).unwrap())
    );
    assert_eq!(text(&mut running, "GET", "/writes"), (200, "0".into()));
    let [report] = running.reports() else {
        panic!("one report: {:?}", running.reports())
    };
    assert_eq!(
        (report.component.as_str(), report.failure.as_str()),
        ("StockRoom/Room", "Capability(items)")
    );
}

#[test]
fn an_overflow_after_a_write_rolls_the_write_back() {
    let program = compile("crates/inset-host/tests/programs/stock-room");
    let mut running = program.start(&fake()).unwrap();
    running
        .table("items")
        .unwrap()
        .set(b"bolt-m6", BOLT)
        .unwrap();
    running
        .table("items")
        .unwrap()
        .set(
            b"nut-m6",
            br#"{"on_hand":4294967295,"place":"Back","price":"0.05"}"#,
        )
        .unwrap();
    assert_eq!(
        send(&mut running, "POST", "/items/bolt-m6/transfers/nut-m6", "1"),
        (500, String::new())
    );
    assert_eq!(
        stored(&running, "items", b"bolt-m6").as_deref(),
        Some(std::str::from_utf8(BOLT).unwrap())
    );
    assert_eq!(text(&mut running, "GET", "/writes"), (200, "0".into()));
    assert_eq!(running.reports()[0].failure, "Overflow");
}

/// Options for a real run in `dir`, where the program's database file goes.
fn real(dir: &Path) -> Options {
    Options {
        settings: Vec::new(),
        mode: Mode::Real,
        working_dir: dir.to_path_buf(),
    }
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("inset-storage-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_stored_table_lasts_from_one_run_to_the_next() {
    let program = compile("crates/inset-host/tests/programs/stock-room");
    let dir = temp_dir("lasts");
    {
        let mut running = program.start(&real(&dir)).unwrap();
        send(&mut running, "PUT", "/items/bolt-m6", "0.10");
        send(&mut running, "PUT", "/items/nut-m6", "0.05");
        send(&mut running, "POST", "/items/bolt-m6/receipts", "3");
        send(&mut running, "POST", "/items/nut-m6/receipts", "4294967295");
        // Taking from bolt-m6 is written, then adding to nut-m6 overflows.
        assert_eq!(
            send(&mut running, "POST", "/items/bolt-m6/transfers/nut-m6", "1"),
            (500, String::new())
        );
    }
    assert!(dir.join("stock-room.db").is_file());
    let mut running = program.start(&real(&dir)).unwrap();
    assert_eq!(
        text(&mut running, "GET", "/items"),
        (
            200,
            "bolt-m6 3 0.30\nnut-m6 4294967295 214748364.75\n".into()
        )
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_stored_table_of_another_type_is_refused() {
    let program = compile("crates/inset-host/tests/programs/stock-room");
    let dir = temp_dir("refused");
    {
        let running = program.start(&real(&dir)).unwrap();
        running
            .table("items")
            .unwrap()
            .set(b"bolt-m6", br#"{"on_hand":3,"price":"0.10"}"#)
            .unwrap();
    }
    let Err(error) = program.start(&real(&dir)) else {
        panic!("the program starts")
    };
    assert_eq!(
        error.to_string(),
        format!(
            "cannot use the table `items` in {} for the grant `items: Table<String, Item>`: \
             the value stored under \"bolt-m6\" does not fit: missing field `place` of `Item`",
            dir.join("stock-room.db").display()
        )
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

fn listed(program: &Program, options: &Options, path: &str) -> String {
    let mut running = program.start(options).unwrap();
    let response = request(&mut running, "GET", path).unwrap();
    String::from_utf8(response.body).unwrap()
}

#[test]
fn entries_come_in_key_order() {
    let program = compile("crates/inset-host/tests/programs/ordered");
    let dir = temp_dir("ordered");
    for options in [fake(), real(&dir)] {
        let mut running = program.start(&options).unwrap();
        for n in [
            "7",
            "-1",
            "0",
            "-300",
            "9223372036854775807",
            "-9223372036854775808",
            "300",
        ] {
            send(&mut running, "PUT", &format!("/numbers/{n}"), "x");
        }
        for word in ["b", "ab", "a", "B", "%C3%A9", "a%00"] {
            send(&mut running, "PUT", &format!("/words/{word}"), "1");
        }
        assert_eq!(
            text(&mut running, "GET", "/numbers").1,
            "-9223372036854775808 -300 -1 0 7 300 9223372036854775807"
        );
        assert_eq!(text(&mut running, "GET", "/words").1, "B a a\0 ab b é");
    }
    // The second run reads what the first stored, still in order.
    assert_eq!(
        listed(&program, &real(&dir), "/numbers"),
        "-9223372036854775808 -300 -1 0 7 300 9223372036854775807"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}
