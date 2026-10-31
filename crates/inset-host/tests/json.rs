//! JSON bodies, query parameters and the front end's files, compiled from
//! `tests/programs/catalogue` and run in the host with fakes.

mod common;

use common::{compile, fake, request};
use inset_host::Request;

#[test]
fn a_json_body_is_read_and_a_json_reply_written_as_serde_does() {
    let program = compile("crates/inset-host/tests/programs/catalogue");
    let mut running = program.start(&fake()).unwrap();
    let body = r#"{"displayName":"M6 kit","net":2.5,"kind":{"kit":[4,"M6"]},"sizes":[[6,"M6"]]}"#;
    let put = |body: &str| Request {
        method: "PUT".into(),
        path: "/products/kit-1".into(),
        body: body.as_bytes().to_vec(),
    };
    let reply = running.request("http", &put(body)).unwrap();
    assert_eq!(reply.status, 200);
    assert_eq!(reply.content_type.as_deref(), Some("application/json"));
    // serde writes a missing `Option` as `null`, and a number read as a
    // `Decimal` back as text.
    assert_eq!(
        String::from_utf8(reply.body).unwrap(),
        r#"{"displayName":"M6 kit","net":"2.5","kind":{"kit":[4,"M6"]},"note":null,"sizes":[[6,"M6"]]}"#
    );
    for bad in ["", "[]", r#"{"displayName":"x"}"#, "{\"displayName\":\"x\""] {
        assert_eq!(
            running.request("http", &put(bad)).unwrap().status,
            400,
            "{bad}"
        );
    }
    let listed = request(&mut running, "GET", "/products?limit=1&parts=false").unwrap();
    assert_eq!(listed.status, 200);
    assert!(
        String::from_utf8(listed.body)
            .unwrap()
            .starts_with(r#"[{"sku":"kit-1","product":{"#)
    );
}

#[test]
fn query_parameters_are_optional_only_when_their_type_is_an_option() {
    let program = compile("crates/inset-host/tests/programs/catalogue");
    let mut running = program.start(&fake()).unwrap();
    let status = |running: &mut inset_host::Running<'_>, path: &str| {
        request(running, "GET", path).unwrap().status
    };
    assert_eq!(status(&mut running, "/products"), 200);
    assert_eq!(status(&mut running, "/products?unknown=1"), 200);
    assert_eq!(status(&mut running, "/products?limit=x"), 400);
    assert_eq!(status(&mut running, "/products?limit"), 400);
    assert_eq!(status(&mut running, "/search"), 400);
    assert_eq!(status(&mut running, "/search?name=M6%20bolt"), 200);
}

#[test]
fn the_front_end_is_served_for_a_get_no_route_takes() {
    let program = compile("crates/inset-host/tests/programs/catalogue");
    let mut running = program.start(&fake()).unwrap();
    let page = request(&mut running, "GET", "/").unwrap();
    assert_eq!(page.status, 200);
    assert_eq!(
        page.content_type.as_deref(),
        Some("text/html; charset=utf-8")
    );
    let file = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/programs/catalogue/web/index.html"),
    )
    .unwrap();
    assert_eq!(page.body, file);
    let style = request(&mut running, "GET", "/css/page.css?v=1").unwrap();
    assert_eq!(style.status, 200);
    assert_eq!(
        style.content_type.as_deref(),
        Some("text/css; charset=utf-8")
    );
    let same = request(&mut running, "GET", "/index.html").unwrap();
    assert_eq!(same.body, file);
    // Only a `GET`, only a file that is there, and never the files'
    // directory itself.
    for (method, path) in [
        ("POST", "/"),
        ("GET", "/missing.js"),
        ("GET", "/css"),
        ("GET", "/../src/main.inset"),
    ] {
        assert_eq!(
            request(&mut running, method, path).unwrap().status,
            404,
            "{method} {path}"
        );
    }
}
