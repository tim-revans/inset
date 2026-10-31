//! The JSON form of a value as serde writes it: the form of a `Json<T>`
//! body or reply.
//!
//! The built program reads and writes JSON bodies with serde's derives and
//! `serde_json`. The test runner sends a body written as a value in a
//! requirement, and compares a JSON reply as a value, without running the
//! program's code, so it needs the same form. This module writes and reads
//! it as serde's derives do with `serde_json`, for the types a JSON body
//! may hold (syntax.md, "Routes"):
//!
//! | Type | JSON |
//! | --- | --- |
//! | `()` | `null` |
//! | `bool`, `String`, an integer | a JSON boolean, string or number |
//! | `char` | a string of one character |
//! | A float | a number, or `null` for one that is not finite |
//! | `Decimal` | a string, keeping its scale: `"0.10"`. A number is read too |
//! | A tuple, `Vec`, `VecDeque` or set | a list |
//! | A map with `String` keys | an object |
//! | `Option<T>` | `null` for `None`, and `Some(x)` as `x` |
//! | `Result<T, E>` | `{"Ok": x}` or `{"Err": e}` |
//! | A struct | an object of its fields, by their JSON names. A missing `Option` field reads as `None`, and an unknown field is ignored |
//! | An enum | a unit variant's name, `{"Name": x}` for a variant holding one value, or `{"Name": [fields]}` |
//!
//! Field and variant names are each type's [`DataShape::json`] names.

use serde_json::{Map, Number, Value as Json};

use crate::{DataKind, DataShape, Decimal, Shape, Types, Value, json, key};

/// A value's JSON form, given its shape, which names the enum of each
/// variant and so its JSON names. Fails only for a map whose keys are not
/// text, which serde cannot write as JSON either.
pub fn encode(value: &Value, shape: &Shape, types: &Types) -> Result<Json, String> {
    Ok(match (value, shape) {
        (Value::Unit | Value::Option(None), _) => Json::Null,
        (Value::Bool(b), _) => Json::Bool(*b),
        (Value::Char(c), _) => Json::String(c.to_string()),
        (Value::String(s), _) => Json::String(s.clone()),
        (Value::Int(n), _) => {
            if let Ok(n) = i64::try_from(*n) {
                Json::Number(n.into())
            } else if let Ok(n) = u64::try_from(*n) {
                Json::Number(n.into())
            } else {
                return Err(format!("{n} is too large for a JSON number"));
            }
        }
        (Value::Float(x), _) => Number::from_f64(*x).map_or(Json::Null, Json::Number),
        (Value::Decimal(d), _) => json::from_decimal(d),
        (Value::Tuple(items) | Value::List(items) | Value::Set(items), _) => {
            let shapes: Vec<&Shape> = match shape {
                Shape::Tuple(shapes) => shapes.iter().collect(),
                Shape::List(item) | Shape::Set(item) => vec![item; items.len()],
                _ => return Err(format!("{value:?} is not a `{shape:?}`")),
            };
            Json::Array(
                items
                    .iter()
                    .zip(shapes)
                    .map(|(i, s)| encode(i, s, types))
                    .collect::<Result<_, _>>()?,
            )
        }
        (Value::Map(entries), Shape::Map(_, v_shape)) => {
            let mut object = Map::new();
            for (k, v) in entries {
                let Value::String(k) = k else {
                    return Err(format!("a JSON object's keys are text, not {k:?}"));
                };
                object.insert(k.clone(), encode(v, v_shape, types)?);
            }
            Json::Object(object)
        }
        (Value::Option(Some(inner)), Shape::Option(s)) => encode(inner, s, types)?,
        (Value::Result(result), Shape::Result(ok, err)) => match result {
            Ok(inner) => json::result(true, encode(inner, ok, types)?),
            Err(inner) => json::result(false, encode(inner, err, types)?),
        },
        (Value::Struct { fields, .. }, Shape::Data(id)) => {
            let data = types.get(*id);
            let DataKind::Struct(shapes) = &data.kind else {
                return Err(format!("`{}` is not a struct", data.name));
            };
            let mut object = Map::new();
            for (field, v) in fields {
                let Some(i) = shapes.iter().position(|(n, _)| n == field) else {
                    return Err(format!("`{}` has no field `{field}`", data.name));
                };
                object.insert(json_name(data, field, i), encode(v, &shapes[i].1, types)?);
            }
            Json::Object(object)
        }
        (Value::Variant { name, fields }, Shape::Data(id)) => {
            let data = types.get(*id);
            let DataKind::Enum(variants) = &data.kind else {
                return Err(format!("`{}` is not an enum", data.name));
            };
            let Some(i) = variants.iter().position(|(n, _)| n == name) else {
                return Err(format!("`{}` has no variant `{name}`", data.name));
            };
            let wire = json_name(data, name, i);
            let mut held: Vec<Json> = fields
                .iter()
                .zip(&variants[i].1)
                .map(|(f, s)| encode(f, s, types))
                .collect::<Result<_, _>>()?;
            if held.is_empty() {
                Json::String(wire)
            } else {
                let held = if held.len() == 1 {
                    held.remove(0)
                } else {
                    Json::Array(held)
                };
                let mut object = Map::new();
                object.insert(wire, held);
                Json::Object(object)
            }
        }
        _ => return Err(format!("{value:?} is not a `{shape:?}`")),
    })
}

/// Reads a value of the given shape from its JSON form, as serde's derive
/// would. The error says what did not fit.
pub fn decode(json: &Json, shape: &Shape, types: &Types) -> Result<Value, String> {
    let expected = |what: &str| format!("expected {what}, found {json}");
    Ok(match shape {
        Shape::Unit => match json {
            Json::Null => Value::Unit,
            _ => return Err(expected("`null`")),
        },
        Shape::Bool => Value::Bool(json.as_bool().ok_or_else(|| expected("a bool"))?),
        Shape::Char => Value::Char(json::to_char(json).ok_or_else(|| expected("a char"))?),
        Shape::String => Value::String(
            json.as_str()
                .ok_or_else(|| expected("a string"))?
                .to_string(),
        ),
        Shape::Int { signed, bits } => {
            let n = match json {
                Json::Number(n) => n
                    .as_i64()
                    .map(i128::from)
                    .or_else(|| n.as_u64().map(i128::from)),
                _ => None,
            }
            .ok_or_else(|| expected("an integer"))?;
            if !key::int_fits(n, *signed, *bits) {
                let ty = format!("{}{bits}", if *signed { 'i' } else { 'u' });
                return Err(format!("{n} does not fit a `{ty}`"));
            }
            Value::Int(n)
        }
        Shape::Float => Value::Float(json.as_f64().ok_or_else(|| expected("a number"))?),
        Shape::Decimal => {
            let text = match json {
                Json::String(s) => s.clone(),
                Json::Number(n) => n.to_string(),
                _ => return Err(expected("a decimal")),
            };
            let decimal = text
                .parse::<Decimal>()
                .or_else(|_| Decimal::from_scientific(&text))
                .map_err(|_| expected("a decimal"))?;
            Value::Decimal(decimal)
        }
        Shape::Tuple(shapes) => {
            let items = json.as_array().ok_or_else(|| expected("a list"))?;
            if items.len() != shapes.len() {
                return Err(format!("expected a list of {}, found {json}", shapes.len()));
            }
            Value::Tuple(
                items
                    .iter()
                    .zip(shapes)
                    .map(|(item, shape)| decode(item, shape, types))
                    .collect::<Result<_, _>>()?,
            )
        }
        Shape::List(item) | Shape::Set(item) => {
            let items = json
                .as_array()
                .ok_or_else(|| expected("a list"))?
                .iter()
                .map(|i| decode(i, item, types))
                .collect::<Result<_, _>>()?;
            if matches!(shape, Shape::List(_)) {
                Value::List(items)
            } else {
                Value::Set(items)
            }
        }
        Shape::Map(key, value) => {
            let object = json.as_object().ok_or_else(|| expected("an object"))?;
            if **key != Shape::String {
                return Err("a JSON object's keys are text".into());
            }
            Value::Map(
                object
                    .iter()
                    .map(|(k, v)| Ok((Value::String(k.clone()), decode(v, value, types)?)))
                    .collect::<Result<_, String>>()?,
            )
        }
        Shape::Option(inner) => match json {
            Json::Null => Value::Option(None),
            _ => Value::Option(Some(Box::new(decode(json, inner, types)?))),
        },
        Shape::Result(ok, err) => {
            let (is_ok, inner) = json::to_result(json)?;
            if is_ok {
                Value::Result(Ok(Box::new(decode(inner, ok, types)?)))
            } else {
                Value::Result(Err(Box::new(decode(inner, err, types)?)))
            }
        }
        Shape::Data(id) => {
            let data = types.get(*id);
            match &data.kind {
                DataKind::Struct(fields) => {
                    let object = json
                        .as_object()
                        .ok_or_else(|| expected(&format!("a `{}`", data.name)))?;
                    let mut values = Vec::new();
                    for (i, (name, shape)) in fields.iter().enumerate() {
                        let wire = json_name(data, name, i);
                        let value = match object.get(&wire) {
                            Some(field) => decode(field, shape, types).map_err(|e| {
                                format!("in field `{wire}` of `{}`: {e}", data.name)
                            })?,
                            // serde's derive reads a missing `Option` as `None`.
                            None if matches!(shape, Shape::Option(_)) => Value::Option(None),
                            None => {
                                return Err(format!("missing field `{wire}` of `{}`", data.name));
                            }
                        };
                        values.push((name.clone(), value));
                    }
                    Value::Struct {
                        name: data.name.clone(),
                        fields: values,
                    }
                }
                DataKind::Enum(variants) => {
                    let (wire, held) = match json {
                        Json::String(name) => (name.as_str(), None),
                        Json::Object(object) if object.len() == 1 => {
                            let (name, held) = object.iter().next().expect("one entry");
                            (name.as_str(), Some(held))
                        }
                        _ => return Err(expected(&format!("a `{}`", data.name))),
                    };
                    let Some((name, shapes)) = variants
                        .iter()
                        .enumerate()
                        .find(|(i, (n, _))| json_name(data, n, *i) == wire)
                        .map(|(_, v)| v)
                    else {
                        return Err(format!("`{}` has no variant `{wire}`", data.name));
                    };
                    let fields = match (shapes.as_slice(), held) {
                        ([], None) => Vec::new(),
                        ([shape], Some(held)) => vec![decode(held, shape, types)?],
                        (shapes, Some(Json::Array(items))) if shapes.len() == items.len() => items
                            .iter()
                            .zip(shapes)
                            .map(|(j, s)| decode(j, s, types))
                            .collect::<Result<_, _>>()?,
                        _ => {
                            return Err(format!(
                                "`{}::{name}` holds {}, found {json}",
                                data.name,
                                shapes.len()
                            ));
                        }
                    };
                    Value::Variant {
                        name: name.clone(),
                        fields,
                    }
                }
            }
        }
    })
}

/// The JSON name of the field or variant `name`, the `i`th of `data`.
fn json_name(data: &DataShape, name: &str, i: usize) -> String {
    data.json
        .get(i)
        .cloned()
        .unwrap_or_else(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn types() -> Types {
        Types(vec![
            DataShape {
                name: "Item".into(),
                kind: DataKind::Struct(vec![
                    (
                        "on_hand".into(),
                        Shape::Int {
                            signed: false,
                            bits: 32,
                        },
                    ),
                    ("net".into(), Shape::Decimal),
                    ("place".into(), Shape::Data(1)),
                    ("note".into(), Shape::Option(Box::new(Shape::String))),
                ]),
                json: vec!["onHand".into(), "net".into(), "place".into(), "note".into()],
            },
            DataShape {
                name: "Place".into(),
                kind: DataKind::Enum(vec![
                    ("Back".into(), vec![]),
                    ("Shelf".into(), vec![Shape::String]),
                    (
                        "Bin".into(),
                        vec![
                            Shape::String,
                            Shape::Int {
                                signed: false,
                                bits: 8,
                            },
                        ],
                    ),
                ]),
                json: vec!["back".into(), "shelf".into(), "bin".into()],
            },
        ])
    }

    fn item(place: Value) -> Value {
        Value::Struct {
            name: "Item".into(),
            fields: vec![
                ("on_hand".into(), Value::Int(40)),
                ("net".into(), Value::Decimal("0.10".parse().unwrap())),
                ("place".into(), place),
                ("note".into(), Value::Option(None)),
            ],
        }
    }

    #[test]
    fn a_value_is_written_as_serde_writes_it_and_read_back() {
        let types = types();
        for (place, expected) in [
            (
                Value::Variant {
                    name: "Back".into(),
                    fields: vec![],
                },
                r#""back""#,
            ),
            (
                Value::Variant {
                    name: "Shelf".into(),
                    fields: vec![Value::String("A3".into())],
                },
                r#"{"shelf":"A3"}"#,
            ),
            (
                Value::Variant {
                    name: "Bin".into(),
                    fields: vec![Value::String("A".into()), Value::Int(3)],
                },
                r#"{"bin":["A",3]}"#,
            ),
        ] {
            let value = item(place);
            let json = encode(&value, &Shape::Data(0), &types).unwrap();
            assert_eq!(
                json.to_string(),
                format!(r#"{{"net":"0.10","note":null,"onHand":40,"place":{expected}}}"#)
            );
            let back = decode(&json, &Shape::Data(0), &types).unwrap();
            assert_eq!(back.to_string(), value.to_string());
        }
    }

    #[test]
    fn reading_follows_serdes_derive() {
        let types = types();
        // A missing `Option` is `None`, an unknown field is ignored, and a
        // decimal may be a number.
        let json: Json =
            serde_json::from_str(r#"{"onHand":1,"net":0.5,"place":"back","extra":true}"#).unwrap();
        let value = decode(&json, &Shape::Data(0), &types).unwrap();
        assert_eq!(
            value.to_string(),
            "Item { on_hand: 1, net: 0.5, place: Back, note: None }"
        );
        let json: Json = serde_json::from_str(r#"{"net":"1","place":"back"}"#).unwrap();
        assert_eq!(
            decode(&json, &Shape::Data(0), &types).unwrap_err(),
            "missing field `onHand` of `Item`"
        );
        let json: Json = serde_json::from_str(r#"{"onHand":1,"net":"1","place":"Back"}"#).unwrap();
        assert_eq!(
            decode(&json, &Shape::Data(0), &types).unwrap_err(),
            "in field `place` of `Item`: `Place` has no variant `Back`"
        );
    }
}
