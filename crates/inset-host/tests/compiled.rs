//! A program using every construct the compiler emits so far, compiled from
//! `tests/programs/routes` and run in the host with fakes.

mod common;

use common::{compile, fake, text};
use inset_host::{GrantValue, Request, SettingType};

#[test]
fn settings_and_grants_reach_the_manifest() {
    let program = compile("crates/inset-host/tests/programs/routes");
    let manifest = program.manifest();
    assert_eq!(manifest.app, "Routes");
    let settings: Vec<_> = manifest
        .settings
        .iter()
        .map(|s| (s.name.as_str(), s.type_, s.default.as_str()))
        .collect();
    assert_eq!(
        settings,
        [
            ("port", SettingType::U16, "8080"),
            ("admin_port", SettingType::U16, "8081"),
            ("greeting", SettingType::String, "Hello"),
            ("verbose", SettingType::Bool, "false"),
            ("offset", SettingType::I32, "-3"),
        ]
    );
    let grants: Vec<_> = manifest
        .grants
        .iter()
        .map(|g| (g.name.as_str(), g.capability.as_str(), &g.fields[0].value))
        .collect();
    assert_eq!(
        grants,
        [
            ("http", "HttpServer", &GrantValue::Setting("port".into())),
            (
                "ops",
                "HttpServer",
                &GrantValue::Setting("admin_port".into())
            ),
        ]
    );
}

#[test]
fn routes_convert_their_parameters_and_bodies() {
    let program = compile("crates/inset-host/tests/programs/routes");
    let mut running = program.start(&fake()).unwrap();
    assert_eq!(text(&mut running, "GET", "/add/2/40"), (200, "42".into()));
    assert_eq!(text(&mut running, "GET", "/add/2/x"), (404, String::new()));
    assert_eq!(text(&mut running, "GET", "/find/7"), (200, "7".into()));
    assert_eq!(text(&mut running, "GET", "/find/0"), (200, "zero".into()));
    assert_eq!(text(&mut running, "GET", "/find/-1"), (404, String::new()));
    assert_eq!(
        text(&mut running, "GET", "/words/ab,c,def"),
        (200, "3 [2, 1, 3] true".into())
    );
    let mut post = |body: &str| {
        let request = Request {
            method: "POST".into(),
            path: "/double/twice".into(),
            body: body.as_bytes().to_vec(),
        };
        let response = running.request("http", &request).unwrap();
        (response.status, String::from_utf8(response.body).unwrap())
    };
    assert_eq!(post("21"), (200, "twice: 42".into()));
    assert_eq!(post("many"), (400, String::new()));
}

#[test]
fn each_server_serves_its_own_routes() {
    let program = compile("crates/inset-host/tests/programs/routes");
    let mut running = program.start(&fake()).unwrap();
    let mut get = |server: &str, path: &str| {
        let request = Request {
            method: "GET".into(),
            path: path.into(),
            body: Vec::new(),
        };
        running.request(server, &request).unwrap().status
    };
    assert_eq!(get("ops", "/status"), 200);
    assert_eq!(get("http", "/status"), 404);
    assert_eq!(get("ops", "/add/1/2"), 404);
}

#[test]
fn overflow_stops_a_program_without_a_failure_block() {
    let program = compile("crates/inset-host/tests/programs/routes");
    let mut running = program.start(&fake()).unwrap();
    let request = Request {
        method: "POST".into(),
        path: "/double/big".into(),
        body: u64::MAX.to_string().into_bytes(),
    };
    let stopped = running.request("http", &request).unwrap_err();
    assert_eq!(
        (stopped.component.as_str(), stopped.failure.as_str()),
        ("Routes/Front", "Overflow")
    );
}
