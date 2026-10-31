//! The typed API contract of an Inset program, which keeps a front end from
//! drifting from the back end it calls.
//!
//! The contract is written from the routes as the checker resolved them
//! ([`inset_check::Api`]), in two forms:
//!
//! - [`openapi`]: an OpenAPI 3.1 document, for any client or tool;
//! - [`typescript`]: TypeScript types for a front end, which `tsc` checks
//!   it against. A field renamed in the Inset source is then an error in the
//!   front end, not a request that quietly fails.
//!
//! Both describe JSON as serde writes it: a field's or a variant's name is
//! its JSON name, after any `#[serde(rename_all)]` or `#[serde(rename)]`,
//! the same names `inset test` sends and reads (`inset_value::serde`).
//!
//! Both are deterministic: routes in the order the program declares them,
//! data types by name, and the same text for the same program every time.
//! Two data types with the same name in different modules would take the
//! same name in the contract, so a program whose JSON uses both is refused.
//!
//! An app with more than one `HttpServer` has one contract for all of its
//! routes, keyed by method and path, so two servers with the same route
//! are refused too.

mod json;
mod openapi;
mod typescript;

use std::collections::BTreeMap;

pub use inset_check::Api;
use inset_value::{DataKind, Shape, Types};

pub use openapi::openapi;
pub use typescript::typescript;

/// The program's data types that its JSON bodies and replies hold, by
/// name, with their index in [`Api::types`]. Fails if two share a name.
fn json_types(api: &Api) -> Result<BTreeMap<String, usize>, String> {
    let mut found = Vec::new();
    for route in &api.routes {
        for shape in route
            .body
            .iter()
            .chain(std::iter::once(&route.reply))
            .filter_map(|b| match b {
                inset_check::ApiBody::Json(shape) => Some(shape),
                inset_check::ApiBody::Text(_) => None,
            })
        {
            reach(shape, &api.types, &mut found);
        }
    }
    let mut by_name = BTreeMap::new();
    for id in found {
        let name = api.types.get(id).name.clone();
        if let Some(other) = by_name.insert(name.clone(), id)
            && other != id
        {
            return Err(format!(
                "two data types named `{name}` are sent as JSON, and the contract names each by its name; rename one"
            ));
        }
    }
    Ok(by_name)
}

fn reach(shape: &Shape, types: &Types, found: &mut Vec<usize>) {
    match shape {
        Shape::Tuple(items) => items.iter().for_each(|s| reach(s, types, found)),
        Shape::List(s) | Shape::Set(s) | Shape::Option(s) => reach(s, types, found),
        Shape::Map(k, v) | Shape::Result(k, v) => {
            reach(k, types, found);
            reach(v, types, found);
        }
        Shape::Data(id) if !found.contains(id) => {
            found.push(*id);
            match &types.get(*id).kind {
                DataKind::Struct(fields) => fields.iter().for_each(|(_, s)| reach(s, types, found)),
                DataKind::Enum(variants) => variants
                    .iter()
                    .flat_map(|(_, s)| s)
                    .for_each(|s| reach(s, types, found)),
            }
        }
        _ => {}
    }
}

/// Each route's key in the contract, `METHOD template`. Fails if two
/// routes of different servers share one.
fn route_keys(api: &Api) -> Result<Vec<String>, String> {
    let mut keys: Vec<String> = Vec::new();
    for route in &api.routes {
        let key = format!("{} {}", route.method.as_str(), route.template);
        if keys.contains(&key) {
            return Err(format!(
                "two servers both have the route `{key}`, and one contract holds every route; give one another path"
            ));
        }
        keys.push(key);
    }
    Ok(keys)
}

/// The route's path without its query, as OpenAPI writes it.
fn path_of(template: &str) -> &str {
    template.split('?').next().unwrap_or(template)
}
