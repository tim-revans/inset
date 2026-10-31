//! Inset values outside a running program: how a table stores them, and how
//! requirement tests write, read and compare them.
//!
//! A `Table<K, V>` keeps each value in a **stored form** that the compiler
//! chooses, not serde's, so a program needs no derive to store a type, and
//! a serde attribute meant for a JSON body cannot change what is on disk
//! (syntax.md, "Types"). Two parts of Inset must agree on that form: the
//! runtime, which stores values from inside the WebAssembly program through
//! code the compiler generates for each type, and `inset test`, which sets up
//! and reads a fake table from requirement facts without running the
//! program's code. This crate is the one place the form is defined, and the
//! runtime and the test runner both use it.
//!
//! - **Values** are stored as JSON ([`json`]), so a stored table can be read
//!   by a person or another tool. A struct is an object of its fields, an
//!   enum's unit variant is its name, and a tuple variant is an object from
//!   its name to a list of its fields. A `Decimal` is a string, so it keeps
//!   its scale exactly, as `"0.10"`.
//! - **Keys** are stored in an order-preserving binary form ([`key`]), so
//!   comparing two stored keys byte by byte gives Rust's order of the keys.
//!   A database sorts them without knowing their type, and `entries` returns
//!   them in key order. A key is an integer, `bool`, `char`, `String`, or a
//!   tuple of these; a `String` key alone is stored as its UTF-8 bytes.
//!
//! The test runner works with [`Value`], a value of any Inset data type with
//! its [`Shape`]. A value compares as the type's derived `PartialEq` would,
//! and prints as `{:?}` would, which is what `assert_eq!` shows when a fact
//! fails. A requirement's check is a [`Check`], evaluated against what the
//! fake tables hold.
//!
//! A JSON body is not stored, so it does not use the stored form: it is the
//! program's own serde form, which [`serde`] writes and reads the way
//! `serde_json` and serde's derives do, so a requirement can send a body
//! written as a value and compare a JSON reply as one.

mod check;
pub mod json;
pub mod key;
pub mod rename;
pub mod serde;

use std::cmp::Ordering;
use std::fmt;

pub use check::{Check, CheckError, Lookup, Op, symbol};
pub use rust_decimal::Decimal;

/// What a value's type is, as far as storing, comparing and printing it go.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Unit,
    Bool,
    Char,
    String,
    /// An integer type: whether it is signed, and its width in bits.
    /// `usize` and `isize` are 64 bits wide, as on every target Inset builds for.
    Int {
        signed: bool,
        bits: u8,
    },
    Float,
    Decimal,
    Tuple(Vec<Shape>),
    /// `Vec` and `VecDeque`.
    List(Box<Shape>),
    /// `HashSet` and `BTreeSet`.
    Set(Box<Shape>),
    /// `HashMap` and `BTreeMap`.
    Map(Box<Shape>, Box<Shape>),
    Option(Box<Shape>),
    Result(Box<Shape>, Box<Shape>),
    /// A struct or enum the program declares, by its index in [`Types`].
    Data(usize),
}

/// The structs and enums a program declares, which [`Shape::Data`] refers
/// to. A data type may contain itself, through a `Vec` or an `Option`, so
/// shapes refer to it by index rather than containing it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Types(pub Vec<DataShape>);

#[derive(Clone, Debug, PartialEq)]
pub struct DataShape {
    pub name: String,
    pub kind: DataKind,
    /// The name of each field or variant, in order, in the type's JSON form
    /// as serde writes it ([`serde`]): the declared name, unless a
    /// `#[serde(rename_all)]` or `#[serde(rename)]` changes it.
    pub json: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DataKind {
    /// Its fields, in the order they are declared.
    Struct(Vec<(String, Shape)>),
    /// Its variants, each with the types it holds.
    Enum(Vec<(String, Vec<Shape>)>),
}

impl Types {
    pub fn get(&self, id: usize) -> &DataShape {
        &self.0[id]
    }
}

/// A value of an Inset data type.
#[derive(Clone, Debug)]
pub enum Value {
    Unit,
    Bool(bool),
    Char(char),
    String(String),
    /// Any integer. `u128` values above `i128::MAX` cannot be written in a
    /// requirement, and are not represented.
    Int(i128),
    Float(f64),
    Decimal(Decimal),
    Tuple(Vec<Value>),
    List(Vec<Value>),
    Set(Vec<Value>),
    Map(Vec<(Value, Value)>),
    Option(Option<Box<Value>>),
    Result(Result<Box<Value>, Box<Value>>),
    Struct {
        name: String,
        fields: Vec<(String, Value)>,
    },
    Variant {
        name: String,
        fields: Vec<Value>,
    },
}

impl Value {
    /// The field `name` of a struct.
    pub fn field(&self, name: &str) -> Option<&Value> {
        match self {
            Value::Struct { fields, .. } => fields.iter().find(|(n, _)| n == name).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Ordering, for the comparison operators in a check: numbers, text,
    /// `char` and `bool`, as Rust orders them. `None` for any other value,
    /// and for a float that is NaN.
    pub fn compare(&self, other: &Value) -> Option<Ordering> {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => Some(a.cmp(b)),
            (Value::Float(a), Value::Float(b)) => a.partial_cmp(b),
            (Value::Decimal(a), Value::Decimal(b)) => Some(a.cmp(b)),
            (Value::String(a), Value::String(b)) => Some(a.cmp(b)),
            (Value::Char(a), Value::Char(b)) => Some(a.cmp(b)),
            (Value::Bool(a), Value::Bool(b)) => Some(a.cmp(b)),
            _ => None,
        }
    }
}

/// Equality as a derived `PartialEq` has it: field by field, with a
/// `Decimal` equal to another of the same number whatever their scales, as
/// in `rust_decimal`, and a set or map equal to another with the same
/// entries in any order.
impl PartialEq for Value {
    fn eq(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Unit, Value::Unit) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Char(a), Value::Char(b)) => a == b,
            (Value::String(a), Value::String(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::Decimal(a), Value::Decimal(b)) => a == b,
            (Value::Tuple(a), Value::Tuple(b)) | (Value::List(a), Value::List(b)) => a == b,
            (Value::Set(a), Value::Set(b)) => same_entries(a, b),
            (Value::Map(a), Value::Map(b)) => same_entries(a, b),
            (Value::Option(a), Value::Option(b)) => a == b,
            (Value::Result(a), Value::Result(b)) => a == b,
            (
                Value::Struct { name, fields },
                Value::Struct {
                    name: other_name,
                    fields: other_fields,
                },
            ) => name == other_name && fields == other_fields,
            (
                Value::Variant { name, fields },
                Value::Variant {
                    name: other_name,
                    fields: other_fields,
                },
            ) => name == other_name && fields == other_fields,
            _ => false,
        }
    }
}

/// Whether two collections hold the same entries, in any order.
fn same_entries<T: PartialEq>(a: &[T], b: &[T]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut unmatched: Vec<&T> = b.iter().collect();
    a.iter()
        .all(|x| match unmatched.iter().position(|y| *y == x) {
            Some(i) => {
                unmatched.swap_remove(i);
                true
            }
            None => false,
        })
}

/// The value as Rust's `{:?}` prints it, which is what `assert_eq!` shows:
/// `Item { on_hand: 35, reorder_level: 10 }`, `"bolt-m6"`, `Some(0.10)`.
impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fn list<'v>(
            f: &mut fmt::Formatter<'_>,
            items: impl IntoIterator<Item = &'v Value>,
        ) -> fmt::Result {
            for (i, item) in items.into_iter().enumerate() {
                if i > 0 {
                    f.write_str(", ")?;
                }
                write!(f, "{item}")?;
            }
            Ok(())
        }
        match self {
            Value::Unit => f.write_str("()"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Char(c) => write!(f, "{c:?}"),
            Value::String(s) => write!(f, "{s:?}"),
            Value::Int(n) => write!(f, "{n}"),
            Value::Float(x) => write!(f, "{x:?}"),
            Value::Decimal(d) => write!(f, "{d}"),
            Value::Tuple(items) => {
                f.write_str("(")?;
                list(f, items)?;
                if items.len() == 1 {
                    f.write_str(",")?;
                }
                f.write_str(")")
            }
            Value::List(items) => {
                f.write_str("[")?;
                list(f, items)?;
                f.write_str("]")
            }
            Value::Set(items) => {
                f.write_str("{")?;
                list(f, items)?;
                f.write_str("}")
            }
            Value::Map(entries) => {
                f.write_str("{")?;
                for (i, (k, v)) in entries.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{k}: {v}")?;
                }
                f.write_str("}")
            }
            Value::Option(None) => f.write_str("None"),
            Value::Option(Some(v)) => write!(f, "Some({v})"),
            Value::Result(Ok(v)) => write!(f, "Ok({v})"),
            Value::Result(Err(e)) => write!(f, "Err({e})"),
            Value::Struct { name, fields } => {
                write!(f, "{name}")?;
                if fields.is_empty() {
                    return Ok(());
                }
                f.write_str(" { ")?;
                for (i, (field, value)) in fields.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{field}: {value}")?;
                }
                f.write_str(" }")
            }
            Value::Variant { name, fields } => {
                f.write_str(name)?;
                if fields.is_empty() {
                    return Ok(());
                }
                f.write_str("(")?;
                list(f, fields)?;
                f.write_str(")")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_print_as_debug_does() {
        let item = Value::Struct {
            name: "Item".into(),
            fields: vec![
                ("sku".into(), Value::String("bolt-m6".into())),
                ("net".into(), Value::Decimal("0.10".parse().unwrap())),
                (
                    "tags".into(),
                    Value::List(vec![Value::Char('a'), Value::Tuple(vec![Value::Int(1)])]),
                ),
                (
                    "state".into(),
                    Value::Variant {
                        name: "Held".into(),
                        fields: vec![Value::Option(None)],
                    },
                ),
            ],
        };
        assert_eq!(
            item.to_string(),
            r#"Item { sku: "bolt-m6", net: 0.10, tags: ['a', (1,)], state: Held(None) }"#
        );
    }

    #[test]
    fn decimals_are_equal_whatever_their_scale_and_sets_in_any_order() {
        let d = |s: &str| Value::Decimal(s.parse().unwrap());
        assert_eq!(d("0.10"), d("0.1"));
        assert_ne!(d("0.10"), d("0.11"));
        assert_eq!(
            Value::Set(vec![Value::Int(1), Value::Int(2)]),
            Value::Set(vec![Value::Int(2), Value::Int(1)])
        );
        assert_ne!(
            Value::List(vec![Value::Int(1), Value::Int(2)]),
            Value::List(vec![Value::Int(2), Value::Int(1)])
        );
    }
}
