//! The API contracts of the example programs: pinned by snapshots, the same
//! on every run, and valid OpenAPI 3.1.
//!
//! `tests/openapi/` holds the OpenAPI Initiative's published JSON Schemas
//! for OpenAPI 3.1 (`schema-base` from 2022-10-07, which checks each Schema
//! Object against the OpenAPI dialect of JSON Schema, with the dialect and
//! its vocabulary), copied from <https://spec.openapis.org/oas/3.1/>, so
//! the test needs no network.

use std::path::Path;

use inset_check::{Api, Program};

const EXAMPLES: [&str; 2] = ["hello-http", "stock-tracker"];

fn api(example: &str) -> Api {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(example);
    let program = Program::load(&dir).expect("load example");
    inset_check::api(&program).unwrap_or_else(|reports| panic!("{example}: {reports:?}"))
}

#[test]
fn openapi_documents_are_pinned() {
    for example in EXAMPLES {
        let document = inset_contract::openapi(&api(example)).unwrap();
        insta::assert_snapshot!(format!("{example}.openapi.json"), document);
    }
}

#[test]
fn typescript_is_pinned() {
    for example in EXAMPLES {
        let types = inset_contract::typescript(&api(example)).unwrap();
        insta::assert_snapshot!(format!("{example}.d.ts"), types);
    }
}

#[test]
fn the_contract_is_the_same_every_time() {
    for example in EXAMPLES {
        let (a, b) = (api(example), api(example));
        assert_eq!(
            inset_contract::openapi(&a).unwrap(),
            inset_contract::openapi(&b).unwrap()
        );
        assert_eq!(
            inset_contract::typescript(&a).unwrap(),
            inset_contract::typescript(&b).unwrap()
        );
    }
}

#[test]
fn openapi_documents_are_valid_openapi_3_1() {
    let schemas = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/openapi");
    let read = |name: &str| -> serde_json::Value {
        let text = std::fs::read_to_string(schemas.join(name)).expect("read schema");
        serde_json::from_str(&text).expect("schema is JSON")
    };
    let base = read("schema-base.json");
    let registry = jsonschema::Registry::new()
        .add(
            "https://spec.openapis.org/oas/3.1/schema/2022-10-07",
            read("schema.json"),
        )
        .and_then(|r| {
            r.add(
                "https://spec.openapis.org/oas/3.1/dialect/base",
                read("dialect-base.json"),
            )
        })
        .and_then(|r| {
            r.add(
                "https://spec.openapis.org/oas/3.1/meta/base",
                read("meta-base.json"),
            )
        })
        .and_then(|r| r.prepare())
        .expect("the OpenAPI schemas load");
    let validator = jsonschema::options()
        .with_registry(&registry)
        .build(&base)
        .expect("the OpenAPI schema compiles");
    for example in EXAMPLES {
        let document = inset_contract::openapi(&api(example)).unwrap();
        let json: serde_json::Value =
            serde_json::from_str(&document).expect("the document is JSON");
        let errors: Vec<String> = validator
            .iter_errors(&json)
            .map(|e| format!("{e} at {}", e.instance_path()))
            .collect();
        assert!(errors.is_empty(), "{example}:\n{}", errors.join("\n"));
    }
    // The validator rejects a document that is not OpenAPI, and one whose
    // Schema Object is not JSON Schema.
    let broken = serde_json::json!({"openapi": "3.1.0", "info": {"title": "x"}, "paths": {}});
    assert!(
        !validator.is_valid(&broken),
        "a document without `info.version` passed"
    );
    let broken = serde_json::json!({
        "openapi": "3.1.0",
        "info": {"title": "x", "version": "0"},
        "components": {"schemas": {"X": {"type": 5}}}
    });
    assert!(
        !validator.is_valid(&broken),
        "a schema with `\"type\": 5` passed"
    );
}
