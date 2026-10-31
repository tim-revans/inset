//! TypeScript types for a front end: a `.d.ts` file.
//!
//! - Each data type sent as JSON is an exported `interface` for a struct,
//!   or a `type` for an enum: a union of its variants as serde writes them.
//!   Numbers are `number`, and a `Decimal` is a `string`, as serde writes it.
//!   An `Option` is `T | null`, because serde writes `None` as `null`.
//! - `Routes` maps each route, keyed by its method and its path as its
//!   attribute writes it, to what it takes and replies: `path` and `query`
//!   parameters, a `body`, the `reply` on success, and every `status` it can
//!   reply with. A text body or reply is a `string`.
//!
//! A front end imports the types and annotates what it sends and reads with
//! them, from TypeScript or from JavaScript's JSDoc, so `tsc` reports a use
//! that no longer fits the program.

use std::fmt::Write;

use inset_check::{Api, ApiBody, ApiParam, ApiSegment};
use inset_value::{DataKind, Shape, Types};

use crate::{json_types, route_keys};

/// The program's API as TypeScript declarations.
pub fn typescript(api: &Api) -> Result<String, String> {
    let keys = route_keys(api)?;
    let types = json_types(api)?;
    let mut out = format!(
        "// The API contract of the Inset app `{}`, written by `inset contract`.\n\
         // It is written again on every build: change the Inset source, not this file.\n",
        api.app
    );
    for id in types.values() {
        out.push('\n');
        data(&mut out, api, *id);
    }
    out.push('\n');
    comment(
        &mut out,
        "",
        Some(
            "Every route, by its method and its path as its attribute writes it: what it\n\
             takes, what it replies on success, and every status it can reply with.",
        ),
    );
    out.push_str("export interface Routes {\n");
    for (route, key) in api.routes.iter().zip(&keys) {
        comment(&mut out, "  ", route.doc.as_deref());
        let _ = writeln!(out, "  {}: {{", quote(key));
        let path: Vec<&ApiParam> = route
            .path
            .iter()
            .filter_map(|s| match s {
                ApiSegment::Param(p) => Some(p),
                ApiSegment::Literal(_) => None,
            })
            .collect();
        if !path.is_empty() {
            let _ = writeln!(out, "    path: {};", params(&path, &api.types));
        }
        if !route.query.is_empty() {
            let query: Vec<&ApiParam> = route.query.iter().collect();
            let _ = writeln!(out, "    query: {};", params(&query, &api.types));
        }
        if let Some(body) = &route.body {
            let _ = writeln!(out, "    body: {};", body_type(body, &api.types));
        }
        let _ = writeln!(out, "    reply: {};", body_type(&route.reply, &api.types));
        let mut statuses = vec![200];
        if route.not_found {
            statuses.push(404);
        }
        statuses.extend(route.errors.iter().map(|e| e.code));
        if route.body.is_some() || !route.query.is_empty() {
            statuses.push(400);
        }
        statuses.push(500);
        statuses.sort_unstable();
        statuses.dedup();
        let statuses: Vec<String> = statuses.iter().map(u16::to_string).collect();
        let _ = writeln!(out, "    status: {};", statuses.join(" | "));
        out.push_str("  };\n");
    }
    out.push_str("}\n");
    Ok(out)
}

fn data(out: &mut String, api: &Api, id: usize) {
    let data = api.types.get(id);
    let docs = &api.docs[id];
    match &data.kind {
        DataKind::Struct(fields) => {
            comment(out, "", docs.doc.as_deref());
            let _ = writeln!(out, "export interface {} {{", data.name);
            for (i, (_, shape)) in fields.iter().enumerate() {
                comment(out, "  ", docs.members.get(i).cloned().flatten().as_deref());
                let _ = writeln!(
                    out,
                    "  {}: {};",
                    property(&data.json[i]),
                    ts(shape, &api.types)
                );
            }
            out.push_str("}\n");
        }
        DataKind::Enum(variants) => {
            // A union has nowhere to hang each variant's doc comment, so
            // they are listed in the type's.
            let mut doc = docs.doc.clone().unwrap_or_default();
            for (i, _) in variants.iter().enumerate() {
                if let Some(variant) = docs.members.get(i).cloned().flatten() {
                    if !doc.is_empty() {
                        doc.push('\n');
                    }
                    let _ = write!(doc, "- `{}`: {variant}", data.json[i]);
                }
            }
            comment(out, "", (!doc.is_empty()).then_some(doc.as_str()));
            let union: Vec<String> = variants
                .iter()
                .enumerate()
                .map(|(i, (_, shapes))| {
                    let name = &data.json[i];
                    match shapes.as_slice() {
                        [] => quote(name),
                        [one] => format!("{{ {}: {} }}", property(name), ts(one, &api.types)),
                        many => format!(
                            "{{ {}: {} }}",
                            property(name),
                            ts(&Shape::Tuple(many.to_vec()), &api.types)
                        ),
                    }
                })
                .collect();
            let _ = writeln!(out, "export type {} = {};", data.name, union.join(" | "));
        }
    }
}

fn params(params: &[&ApiParam], types: &Types) -> String {
    let fields: Vec<String> = params
        .iter()
        .map(|p| {
            let optional = if p.required { "" } else { "?" };
            format!("{}{optional}: {}", property(&p.name), ts(&p.shape, types))
        })
        .collect();
    format!("{{ {} }}", fields.join("; "))
}

fn body_type(body: &ApiBody, types: &Types) -> String {
    match body {
        ApiBody::Json(shape) => ts(shape, types),
        ApiBody::Text(_) => "string".to_string(),
    }
}

/// A value's TypeScript type, as JSON holds it.
fn ts(shape: &Shape, types: &Types) -> String {
    match shape {
        Shape::Unit => "null".into(),
        Shape::Bool => "boolean".into(),
        Shape::Char | Shape::String | Shape::Decimal => "string".into(),
        Shape::Int { .. } | Shape::Float => "number".into(),
        Shape::Tuple(items) => {
            let items: Vec<String> = items.iter().map(|s| ts(s, types)).collect();
            format!("[{}]", items.join(", "))
        }
        Shape::List(item) | Shape::Set(item) => {
            let item = ts(item, types);
            if item.contains(' ') {
                format!("Array<{item}>")
            } else {
                format!("{item}[]")
            }
        }
        Shape::Map(_, value) => format!("{{ [key: string]: {} }}", ts(value, types)),
        Shape::Option(inner) => format!("{} | null", ts(inner, types)),
        Shape::Result(ok, err) => format!(
            "{{ Ok: {} }} | {{ Err: {} }}",
            ts(ok, types),
            ts(err, types)
        ),
        Shape::Data(id) => types.get(*id).name.clone(),
    }
}

/// A property's name, quoted unless it is an identifier.
fn property(name: &str) -> String {
    let identifier = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$');
    if identifier {
        name.to_string()
    } else {
        quote(name)
    }
}

fn quote(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// A JSDoc comment, so editors show the Inset doc comment on hover.
fn comment(out: &mut String, indent: &str, doc: Option<&str>) {
    let Some(doc) = doc else {
        return;
    };
    let doc = doc.replace("*/", "*\\/");
    let lines: Vec<&str> = doc.lines().collect();
    if let [line] = lines.as_slice() {
        let _ = writeln!(out, "{indent}/** {line} */");
        return;
    }
    let _ = writeln!(out, "{indent}/**");
    for line in lines {
        if line.is_empty() {
            let _ = writeln!(out, "{indent} *");
        } else {
            let _ = writeln!(out, "{indent} * {line}");
        }
    }
    let _ = writeln!(out, "{indent} */");
}
