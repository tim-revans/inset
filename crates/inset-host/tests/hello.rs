//! Hello HTTP's hand-written target Rust, run in the host with fakes.

mod common;

use common::{fake, load, text};
use inset_host::{GrantValue, Granted, SettingType, resolve_settings};

#[test]
fn visitors_are_greeted_by_the_name_in_the_web_address() {
    let program = load("hello-http");
    let mut running = program.start(&fake()).unwrap();
    assert_eq!(
        text(&mut running, "GET", "/hello/Ada"),
        (200, "Hello, Ada!".into())
    );
    assert_eq!(
        text(&mut running, "GET", "/hello/Ada%20Lovelace"),
        (200, "Hello, Ada Lovelace!".into())
    );
    assert_eq!(text(&mut running, "GET", "/hello"), (404, String::new()));
    assert_eq!(
        text(&mut running, "POST", "/hello/Ada"),
        (404, String::new())
    );
}

#[test]
fn a_reply_is_utf8_text() {
    let program = load("hello-http");
    let mut running = program.start(&fake()).unwrap();
    let response = common::request(&mut running, "GET", "/hello/Ada").unwrap();
    assert_eq!(
        response.content_type.as_deref(),
        Some("text/plain; charset=utf-8")
    );
}

#[test]
fn the_manifest_lists_settings_and_grants() {
    let program = load("hello-http");
    let manifest = program.manifest();
    assert_eq!(manifest.app, "HelloHttp");
    let [port] = &manifest.settings[..] else {
        panic!("one setting")
    };
    assert_eq!((&*port.name, port.default.as_str()), ("port", "8080"));
    assert!(matches!(port.type_, SettingType::U16));
    let [http] = &manifest.grants[..] else {
        panic!("one grant")
    };
    assert_eq!((&*http.name, &*http.capability), ("http", "HttpServer"));
    assert!(matches!(&http.fields[0].value, GrantValue::Setting(s) if s == "port"));
}

#[test]
fn a_setting_changes_its_grant() {
    let program = load("hello-http");
    let running = program.start(&fake()).unwrap();
    assert_eq!(running.grants()["http"], Granted::HttpServer { port: 8080 });
    let mut options = fake();
    options.settings = vec![("port".into(), "9090".into())];
    let running = program.start(&options).unwrap();
    assert_eq!(running.grants()["http"], Granted::HttpServer { port: 9090 });
}

#[test]
fn a_setting_must_exist_and_fit_its_type() {
    let program = load("hello-http");
    let set = |name: &str, value: &str| {
        resolve_settings(program.manifest(), &[(name.into(), value.into())])
            .map(|_| ())
            .map_err(|e| e.to_string())
    };
    assert_eq!(set("port", "65535"), Ok(()));
    assert_eq!(
        set("port", "65536"),
        Err("setting `port` is a u16, so it cannot be `65536`".into())
    );
    assert_eq!(
        set("host", "x"),
        Err("HelloHttp has no setting `host`".into())
    );
}
