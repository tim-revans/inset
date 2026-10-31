//! The stored form of a value: JSON, chosen by the compiler.
//!
//! | Type | Stored as |
//! | --- | --- |
//! | `()` | `null` |
//! | `bool`, `String` | a JSON boolean or string |
//! | `char` | a string of one character |
//! | An integer | a number, or a string for one that does not fit 64 bits |
//! | A float | a number, or `"NaN"`, `"inf"` or `"-inf"` |
//! | `Decimal` | a string, keeping its scale: `"0.10"` |
//! | A tuple, `Vec`, `VecDeque` or set | a list |
//! | A map | a list of `[key, value]` pairs |
//! | `Option<T>` | `null` for `None`, and `Some(x)` as `x` would be, or as `[x]` when `T` itself can be `null` |
//! | `Result<T, E>` | `{"Ok": x}` or `{"Err": e}` |
//! | A struct | an object of its fields |
//! | An enum | a unit variant's name, or `{"Name": [fields]}` |
//!
//! The functions here that take or give one JSON value are the conventions
//! themselves, which the runtime's `Stored` implementations call, so the two
//! sides cannot drift apart.

use serde_json::{Map, Number, Value as Json};

use crate::{DataKind, Decimal, Shape, Types, Value, key};

/// An integer.
pub fn from_int(n: i128) -> Json {
    if let Ok(n) = i64::try_from(n) {
        Json::Number(n.into())
    } else if let Ok(n) = u64::try_from(n) {
        Json::Number(n.into())
    } else {
        Json::String(n.to_string())
    }
}

/// A `u128`, which may not fit an `i128`.
pub fn from_u128(n: u128) -> Json {
    match u64::try_from(n) {
        Ok(n) => Json::Number(n.into()),
        Err(_) => Json::String(n.to_string()),
    }
}

/// An integer, from a number or from the string a wide one is stored as.
pub fn to_int(json: &Json) -> Option<i128> {
    match json {
        Json::Number(n) => n
            .as_i64()
            .map(i128::from)
            .or_else(|| n.as_u64().map(i128::from)),
        Json::String(s) => s.parse().ok(),
        _ => None,
    }
}

pub fn to_u128(json: &Json) -> Option<u128> {
    match json {
        Json::Number(n) => n.as_u64().map(u128::from),
        Json::String(s) => s.parse().ok(),
        _ => None,
    }
}

pub fn from_float(x: f64) -> Json {
    match Number::from_f64(x) {
        Some(n) => Json::Number(n),
        None if x.is_nan() => Json::String("NaN".into()),
        None if x > 0.0 => Json::String("inf".into()),
        None => Json::String("-inf".into()),
    }
}

pub fn to_float(json: &Json) -> Option<f64> {
    match json {
        Json::Number(n) => n.as_f64(),
        Json::String(s) => match s.as_str() {
            "NaN" => Some(f64::NAN),
            "inf" => Some(f64::INFINITY),
            "-inf" => Some(f64::NEG_INFINITY),
            _ => None,
        },
        _ => None,
    }
}

pub fn from_decimal(d: &Decimal) -> Json {
    Json::String(d.to_string())
}

pub fn to_decimal(json: &Json) -> Option<Decimal> {
    match json {
        Json::String(s) => s.parse().ok(),
        _ => None,
    }
}

pub fn from_char(c: char) -> Json {
    Json::String(c.to_string())
}

pub fn to_char(json: &Json) -> Option<char> {
    let s = json.as_str()?;
    let mut chars = s.chars();
    let c = chars.next()?;
    chars.next().is_none().then_some(c)
}

/// `Some(x)`, given `x` stored. `nullable` says whether the type inside the
/// `Option` can itself be stored as `null`, in which case `x` is wrapped in
/// a list so `Some(None)` stays apart from `None`.
pub fn some(inner: Json, nullable: bool) -> Json {
    if nullable {
        Json::Array(vec![inner])
    } else {
        inner
    }
}

/// What `Some` holds, or `None` for `None`, undoing [`some`].
pub fn unwrap_some(json: &Json, nullable: bool) -> Result<Option<&Json>, String> {
    match json {
        Json::Null => Ok(None),
        Json::Array(items) if nullable => match items.as_slice() {
            [inner] => Ok(Some(inner)),
            _ => Err(format!("expected `[value]` for `Some`, found {json}")),
        },
        _ if nullable => Err(format!("expected `null` or `[value]`, found {json}")),
        _ => Ok(Some(json)),
    }
}

/// An enum variant: its name alone, or an object from its name to its fields.
pub fn variant(name: &str, fields: Vec<Json>) -> Json {
    if fields.is_empty() {
        Json::String(name.into())
    } else {
        let mut object = Map::new();
        object.insert(name.into(), Json::Array(fields));
        Json::Object(object)
    }
}

/// The name of a stored variant, and its fields.
pub fn to_variant(json: &Json) -> Result<(&str, &[Json]), String> {
    match json {
        Json::String(name) => Ok((name, &[])),
        Json::Object(object) if object.len() == 1 => {
            let (name, fields) = object.iter().next().expect("one entry");
            match fields {
                Json::Array(fields) => Ok((name, fields)),
                _ => Err(format!(
                    "expected a list of the fields of `{name}`, found {fields}"
                )),
            }
        }
        _ => Err(format!("expected an enum variant, found {json}")),
    }
}

/// `Ok(x)` or `Err(e)`.
pub fn result(ok: bool, inner: Json) -> Json {
    let mut object = Map::new();
    object.insert(if ok { "Ok" } else { "Err" }.into(), inner);
    Json::Object(object)
}

/// Whether a stored result is `Ok`, and what it holds.
pub fn to_result(json: &Json) -> Result<(bool, &Json), String> {
    match json {
        Json::Object(object) if object.len() == 1 => match object.iter().next() {
            Some((key, inner)) if key == "Ok" => Ok((true, inner)),
            Some((key, inner)) if key == "Err" => Ok((false, inner)),
            _ => Err(format!("expected `Ok` or `Err`, found {json}")),
        },
        _ => Err(format!("expected `Ok` or `Err`, found {json}")),
    }
}

/// Whether a value of this shape can be stored as `null`.
pub fn nullable(shape: &Shape) -> bool {
    matches!(shape, Shape::Unit | Shape::Option(_))
}

/// The stored form of a value.
pub fn encode(value: &Value) -> Json {
    match value {
        Value::Unit => Json::Null,
        Value::Bool(b) => Json::Bool(*b),
        Value::Char(c) => from_char(*c),
        Value::String(s) => Json::String(s.clone()),
        Value::Int(n) => from_int(*n),
        Value::Float(x) => from_float(*x),
        Value::Decimal(d) => from_decimal(d),
        Value::Tuple(items) | Value::List(items) | Value::Set(items) => {
            Json::Array(items.iter().map(encode).collect())
        }
        Value::Map(entries) => Json::Array(
            entries
                .iter()
                .map(|(k, v)| Json::Array(vec![encode(k), encode(v)]))
                .collect(),
        ),
        Value::Option(None) => Json::Null,
        Value::Option(Some(inner)) => some(
            encode(inner),
            matches!(**inner, Value::Unit | Value::Option(_)),
        ),
        Value::Result(Ok(inner)) => result(true, encode(inner)),
        Value::Result(Err(inner)) => result(false, encode(inner)),
        Value::Struct { fields, .. } => Json::Object(
            fields
                .iter()
                .map(|(name, value)| (name.clone(), encode(value)))
                .collect(),
        ),
        Value::Variant { name, fields } => variant(name, fields.iter().map(encode).collect()),
    }
}

/// Reads a stored value of the given shape. The error says what did not fit.
pub fn decode(json: &Json, shape: &Shape, types: &Types) -> Result<Value, String> {
    let expected = |what: &str| format!("expected {what}, found {json}");
    Ok(match shape {
        Shape::Unit => match json {
            Json::Null => Value::Unit,
            _ => return Err(expected("`null`")),
        },
        Shape::Bool => Value::Bool(json.as_bool().ok_or_else(|| expected("a bool"))?),
        Shape::Char => Value::Char(to_char(json).ok_or_else(|| expected("a char"))?),
        Shape::String => Value::String(
            json.as_str()
                .ok_or_else(|| expected("a string"))?
                .to_string(),
        ),
        Shape::Int { signed, bits } => {
            let n = to_int(json).ok_or_else(|| expected("an integer"))?;
            if !key::int_fits(n, *signed, *bits) {
                let ty = format!("{}{bits}", if *signed { 'i' } else { 'u' });
                return Err(format!("{n} does not fit a `{ty}`"));
            }
            Value::Int(n)
        }
        Shape::Float => Value::Float(to_float(json).ok_or_else(|| expected("a number"))?),
        Shape::Decimal => Value::Decimal(to_decimal(json).ok_or_else(|| expected("a decimal"))?),
        Shape::Tuple(shapes) => {
            let items = json.as_array().ok_or_else(|| expected("a tuple"))?;
            if items.len() != shapes.len() {
                return Err(format!(
                    "expected a tuple of {}, found {json}",
                    shapes.len()
                ));
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
            let items = json.as_array().ok_or_else(|| expected("a list"))?;
            let items = items
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
            let entries = json.as_array().ok_or_else(|| expected("a list of pairs"))?;
            Value::Map(
                entries
                    .iter()
                    .map(|entry| match entry.as_array().map(Vec::as_slice) {
                        Some([k, v]) => Ok((decode(k, key, types)?, decode(v, value, types)?)),
                        _ => Err(format!("expected a `[key, value]` pair, found {entry}")),
                    })
                    .collect::<Result<_, _>>()?,
            )
        }
        Shape::Option(inner) => match unwrap_some(json, nullable(inner))? {
            None => Value::Option(None),
            Some(inner_json) => Value::Option(Some(Box::new(decode(inner_json, inner, types)?))),
        },
        Shape::Result(ok, err) => {
            let (is_ok, inner) = to_result(json)?;
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
                    for (name, shape) in fields {
                        let field = object
                            .get(name)
                            .ok_or_else(|| format!("missing field `{name}` of `{}`", data.name))?;
                        let value = decode(field, shape, types)
                            .map_err(|e| format!("in field `{name}` of `{}`: {e}", data.name))?;
                        values.push((name.clone(), value));
                    }
                    if let Some(extra) =
                        object.keys().find(|k| !fields.iter().any(|(n, _)| n == *k))
                    {
                        return Err(format!("`{}` has no field `{extra}`", data.name));
                    }
                    Value::Struct {
                        name: data.name.clone(),
                        fields: values,
                    }
                }
                DataKind::Enum(variants) => {
                    let (name, stored) = to_variant(json)?;
                    let Some((_, shapes)) = variants.iter().find(|(n, _)| n == name) else {
                        return Err(format!("`{}` has no variant `{name}`", data.name));
                    };
                    if stored.len() != shapes.len() {
                        return Err(format!(
                            "`{}::{name}` holds {}, found {}",
                            data.name,
                            shapes.len(),
                            stored.len()
                        ));
                    }
                    Value::Variant {
                        name: name.to_string(),
                        fields: stored
                            .iter()
                            .zip(shapes)
                            .map(|(j, s)| decode(j, s, types))
                            .collect::<Result<_, _>>()?,
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn types() -> Types {
        Types(vec![
            crate::DataShape {
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
                    ("state".into(), Shape::Data(1)),
                ]),
                json: vec!["on_hand".into(), "net".into(), "state".into()],
            },
            crate::DataShape {
                name: "State".into(),
                kind: DataKind::Enum(vec![
                    ("Stocked".into(), vec![]),
                    ("Held".into(), vec![Shape::String]),
                ]),
                json: vec!["Stocked".into(), "Held".into()],
            },
        ])
    }

    #[test]
    fn a_value_survives_being_stored() {
        let item = Value::Struct {
            name: "Item".into(),
            fields: vec![
                ("on_hand".into(), Value::Int(40)),
                ("net".into(), Value::Decimal("0.10".parse().unwrap())),
                (
                    "state".into(),
                    Value::Variant {
                        name: "Held".into(),
                        fields: vec![Value::String("audit".into())],
                    },
                ),
            ],
        };
        let json = encode(&item);
        assert_eq!(
            json.to_string(),
            r#"{"net":"0.10","on_hand":40,"state":{"Held":["audit"]}}"#
        );
        let back = decode(&json, &Shape::Data(0), &types()).unwrap();
        assert_eq!(back.to_string(), item.to_string());
    }

    #[test]
    fn options_that_hold_options_stay_apart() {
        let shape = Shape::Option(Box::new(Shape::Option(Box::new(Shape::Bool))));
        for value in [
            Value::Option(None),
            Value::Option(Some(Box::new(Value::Option(None)))),
            Value::Option(Some(Box::new(Value::Option(Some(Box::new(Value::Bool(
                true,
            ))))))),
        ] {
            let back = decode(&encode(&value), &shape, &Types::default()).unwrap();
            assert_eq!(back.to_string(), value.to_string());
        }
    }

    #[test]
    fn a_value_that_does_not_fit_says_why() {
        let json: Json =
            serde_json::from_str(r#"{"on_hand":-1,"net":"0.1","state":"Stocked"}"#).unwrap();
        assert_eq!(
            decode(&json, &Shape::Data(0), &types()).unwrap_err(),
            "in field `on_hand` of `Item`: -1 does not fit a `u32`"
        );
        let json: Json = serde_json::from_str(r#"{"net":"0.1","state":"Stocked"}"#).unwrap();
        assert_eq!(
            decode(&json, &Shape::Data(0), &types()).unwrap_err(),
            "missing field `on_hand` of `Item`"
        );
    }
}
