//! The OpenAPI 3.1 document.
//!
//! - Each route is an operation under its path, without the query, whose
//!   parameters go in `parameters`. Its `operationId` is its handler's
//!   name, or `Component.handler` when two components share a handler name.
//! - A JSON body or reply is `application/json` with the schema of its type.
//!   A text body or reply is `text/plain`. A text body that is converted to
//!   a number or a `bool` has a `pattern` saying what text converts.
//! - The responses are the ones `inset check` lets a requirement expect:
//!   `200`, `404` for an `Option`, each status of the error enum, `400` for
//!   a route that reads a body or a query, and `500` for any route, which a
//!   failure that a `failure` block rolls back replies.
//! - Each data type sent as JSON is a schema under `components`. A field
//!   whose type is an `Option` is not required, as serde reads it as `None`
//!   when it is missing, and is `null` when serde writes `None`.

use inset_check::{Api, ApiBody, ApiParam, ApiRoute, ApiSegment};
use inset_value::{DataKind, Shape, Types};

use crate::json::J;
use crate::{json_types, path_of, route_keys};

const DECIMAL: &str = "^-?[0-9]+(\\.[0-9]+)?$";

/// The program's API as an OpenAPI 3.1 document, as JSON.
pub fn openapi(api: &Api) -> Result<String, String> {
    route_keys(api)?;
    let types = json_types(api)?;
    let mut info = J::obj([("title", J::str(&api.app)), ("version", J::str("0.0.0"))]);
    if let Some(doc) = &api.doc {
        info = info.with("description", J::str(doc));
    }
    let mut paths: Vec<(String, J)> = Vec::new();
    for route in &api.routes {
        let path = path_of(&route.template).to_string();
        let method = route.method.as_str().to_ascii_lowercase();
        let operation = operation(api, route);
        match paths.iter_mut().find(|(p, _)| *p == path) {
            Some((_, item)) => *item = std::mem::replace(item, J::Null).with(&method, operation),
            None => paths.push((path, J::Obj(vec![(method, operation)]))),
        }
    }
    let mut document = J::obj([
        ("openapi", J::str("3.1.0")),
        ("info", info),
        ("paths", J::Obj(paths)),
    ]);
    if !types.is_empty() {
        let schemas = types
            .iter()
            .map(|(name, id)| (name.clone(), data_schema(api, *id)))
            .collect();
        document = document.with("components", J::obj([("schemas", J::Obj(schemas))]));
    }
    Ok(document.write())
}

fn operation(api: &Api, route: &ApiRoute) -> J {
    let shared = api
        .routes
        .iter()
        .filter(|r| r.handler == route.handler)
        .count()
        > 1;
    let id = if shared {
        format!("{}.{}", route.component, route.handler)
    } else {
        route.handler.clone()
    };
    let mut op = J::obj([("operationId", J::str(id))]);
    if let Some(doc) = &route.doc {
        op = op.with("description", J::str(doc));
    }
    let mut parameters = Vec::new();
    for segment in &route.path {
        if let ApiSegment::Param(param) = segment {
            parameters.push(parameter(param, "path", &api.types));
        }
    }
    for param in &route.query {
        parameters.push(parameter(param, "query", &api.types));
    }
    if !parameters.is_empty() {
        op = op.with("parameters", J::Arr(parameters));
    }
    if let Some(body) = &route.body {
        op = op.with(
            "requestBody",
            J::obj([
                ("required", J::Bool(true)),
                ("content", content(body, &api.types)),
            ]),
        );
    }
    let mut responses: Vec<(u16, J)> = vec![(
        200,
        J::obj([
            ("description", J::str("OK")),
            ("content", content(&route.reply, &api.types)),
        ]),
    )];
    let mut add = |code: u16, description: String, text: bool| match responses
        .iter_mut()
        .find(|(c, _)| *c == code)
    {
        Some((_, response)) => {
            if let J::Obj(entries) = response
                && let Some((_, J::Str(existing))) =
                    entries.iter_mut().find(|(k, _)| k == "description")
            {
                existing.push('\n');
                existing.push_str(&description);
            }
        }
        None => {
            let mut response = J::obj([("description", J::str(description))]);
            if text {
                response = response.with(
                    "content",
                    content(&ApiBody::Text(Shape::String), &api.types),
                );
            }
            responses.push((code, response));
        }
    };
    if route.not_found {
        add(404, "Not found: the handler replied `None`.".into(), false);
    }
    for error in &route.errors {
        let mut description = format!("`{}`", error.variant);
        if let Some(doc) = &error.doc {
            description.push_str(": ");
            description.push_str(doc);
        }
        add(error.code, description, error.text);
    }
    if route.body.is_some() || !route.query.is_empty() {
        let what = match (route.body.is_some(), route.query.is_empty()) {
            (true, true) => "The body could not be read.",
            (false, false) => "A query parameter was missing or could not be read.",
            _ => {
                "The body could not be read, or a query parameter was missing or could not be read."
            }
        };
        add(400, what.into(), false);
    }
    add(
        500,
        "A failure ended the handler, and a `failure` block rolled it back.".into(),
        false,
    );
    responses.sort_by_key(|(code, _)| *code);
    op.with(
        "responses",
        J::Obj(
            responses
                .into_iter()
                .map(|(code, r)| (code.to_string(), r))
                .collect(),
        ),
    )
}

fn parameter(param: &ApiParam, location: &str, types: &Types) -> J {
    J::obj([
        ("name", J::str(&param.name)),
        ("in", J::str(location)),
        ("required", J::Bool(param.required)),
        ("schema", schema(&param.shape, types)),
    ])
}

fn content(body: &ApiBody, types: &Types) -> J {
    let (media, schema) = match body {
        ApiBody::Json(shape) => ("application/json", schema(shape, types)),
        ApiBody::Text(shape) => ("text/plain; charset=utf-8", text_schema(shape)),
    };
    J::Obj(vec![(media.to_string(), J::obj([("schema", schema)]))])
}

/// A text body's schema: text, with the pattern of what converts to its type.
fn text_schema(shape: &Shape) -> J {
    let pattern = match shape {
        Shape::Int { signed: true, .. } => Some("^-?[0-9]+$"),
        Shape::Int { signed: false, .. } => Some("^[0-9]+$"),
        Shape::Bool => Some("^(true|false)$"),
        Shape::Decimal => Some(DECIMAL),
        _ => None,
    };
    let mut schema = J::obj([("type", J::str("string"))]);
    if let Some(pattern) = pattern {
        schema = schema.with("pattern", J::str(pattern));
    }
    if *shape == Shape::Char {
        schema = schema
            .with("minLength", J::num(1))
            .with("maxLength", J::num(1));
    }
    schema
}

/// The schema of a value in JSON.
fn schema(shape: &Shape, types: &Types) -> J {
    let ty = |name: &str| J::obj([("type", J::str(name))]);
    match shape {
        Shape::Unit => ty("null"),
        Shape::Bool => ty("boolean"),
        Shape::Char => ty("string")
            .with("minLength", J::num(1))
            .with("maxLength", J::num(1)),
        Shape::String => ty("string"),
        Shape::Int { signed, bits } => {
            let mut schema = ty("integer");
            match (signed, bits) {
                (true, 32) => schema = schema.with("format", J::str("int32")),
                (true, 64) => schema = schema.with("format", J::str("int64")),
                _ => {}
            }
            if *bits <= 32 {
                let (min, max) = if *signed {
                    (-(1i64 << (bits - 1)), (1i64 << (bits - 1)) - 1)
                } else {
                    (0, (1i64 << bits) - 1)
                };
                schema = schema
                    .with("minimum", J::num(min))
                    .with("maximum", J::num(max));
            } else if !signed {
                schema = schema.with("minimum", J::num(0));
            }
            schema
        }
        Shape::Float => ty("number"),
        Shape::Decimal => ty("string").with("pattern", J::str(DECIMAL)),
        Shape::Tuple(items) => ty("array")
            .with(
                "prefixItems",
                J::Arr(items.iter().map(|s| schema(s, types)).collect()),
            )
            .with("minItems", J::num(items.len()))
            .with("maxItems", J::num(items.len())),
        Shape::List(item) => ty("array").with("items", schema(item, types)),
        Shape::Set(item) => ty("array")
            .with("items", schema(item, types))
            .with("uniqueItems", J::Bool(true)),
        Shape::Map(_, value) => ty("object").with("additionalProperties", schema(value, types)),
        Shape::Option(inner) => J::obj([("anyOf", J::Arr(vec![schema(inner, types), ty("null")]))]),
        Shape::Result(ok, err) => J::obj([(
            "oneOf",
            J::Arr(vec![
                tagged("Ok", schema(ok, types)),
                tagged("Err", schema(err, types)),
            ]),
        )]),
        Shape::Data(id) => J::obj([(
            "$ref",
            J::str(format!("#/components/schemas/{}", types.get(*id).name)),
        )]),
    }
}

/// `{"name": value}`, as serde writes a `Result` and an enum's variant
/// that holds values.
fn tagged(name: &str, value: J) -> J {
    J::obj([
        ("type", J::str("object")),
        ("properties", J::Obj(vec![(name.to_string(), value)])),
        ("required", J::Arr(vec![J::str(name)])),
        ("additionalProperties", J::Bool(false)),
    ])
}

fn data_schema(api: &Api, id: usize) -> J {
    let data = api.types.get(id);
    let docs = &api.docs[id];
    let described = |schema: J, doc: &Option<String>| match doc {
        Some(doc) => schema.with("description", J::str(doc)),
        None => schema,
    };
    let schema = match &data.kind {
        DataKind::Struct(fields) => {
            let mut properties = Vec::new();
            let mut required = Vec::new();
            for (i, (_, shape)) in fields.iter().enumerate() {
                let name = data.json[i].clone();
                if !matches!(shape, Shape::Option(_)) {
                    required.push(J::str(&name));
                }
                let doc = docs.members.get(i).cloned().flatten();
                properties.push((name, described(schema(shape, &api.types), &doc)));
            }
            let mut schema = J::obj([
                ("type", J::str("object")),
                ("properties", J::Obj(properties)),
            ]);
            if !required.is_empty() {
                schema = schema.with("required", J::Arr(required));
            }
            schema
        }
        DataKind::Enum(variants) => {
            let one_of = variants
                .iter()
                .enumerate()
                .map(|(i, (_, shapes))| {
                    let name = &data.json[i];
                    let variant = match shapes.as_slice() {
                        [] => J::obj([("const", J::str(name))]),
                        [one] => tagged(name, schema(one, &api.types)),
                        many => tagged(name, schema(&Shape::Tuple(many.to_vec()), &api.types)),
                    };
                    described(variant, &docs.members.get(i).cloned().flatten())
                })
                .collect();
            J::obj([("oneOf", J::Arr(one_of))])
        }
    };
    described(schema, &docs.doc)
}
