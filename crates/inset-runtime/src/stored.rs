//! How values are stored: in a table, and in the state a component saves
//! before a handler that may be rolled back.
//!
//! The stored form is defined in the `inset-value` crate, which the test
//! runner also uses to set up and read fake tables, so both agree on it.
//! [`Stored`] puts a value in that form and reads it back. The compiler
//! implements it for each struct and enum a program declares, so a program
//! needs no derive to store its types, and serde's attributes, which say how
//! a JSON body looks, cannot change what is stored. [`Key`] and [`KeyType`]
//! give a table's keys their order-preserving form.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::hash::Hash;
use std::rc::Rc;
use std::sync::{Arc, Mutex, RwLock};

use inset_value::json;
pub use inset_value::key::Reader as KeyReader;
use inset_value::key::{self as encode};
use rust_decimal::Decimal;
use serde_json::Value as Json;

/// A value in its stored form, JSON.
pub trait Stored: Sized {
    /// Whether the stored form can be `null`, so `Some` of it must be told
    /// apart from `None`.
    const NULLABLE: bool = false;

    fn to_json(&self) -> Json;

    fn from_json(json: &Json) -> Result<Self, String>;
}

/// A key, or a borrowed form of one such as `&str`, in its stored form.
pub trait Key {
    /// Writes the key. `whole` says it is the whole key, not a part of a tuple.
    fn write_key(&self, out: &mut Vec<u8>, whole: bool);
}

/// A type a table's keys can have.
pub trait KeyType: Key + Stored {
    fn read_key(reader: &mut KeyReader<'_>, whole: bool) -> Result<Self, String>;
}

/// The stored form of a whole key.
pub fn key_bytes<K: Key + ?Sized>(key: &K) -> Vec<u8> {
    let mut out = Vec::new();
    key.write_key(&mut out, true);
    out
}

/// Reads a whole stored key.
pub fn read_key<K: KeyType>(bytes: &[u8]) -> Result<K, String> {
    let mut reader = KeyReader::new(bytes);
    let key = K::read_key(&mut reader, true)?;
    reader.finish()?;
    Ok(key)
}

fn expected(what: &str, json: &Json) -> String {
    format!("expected {what}, found {json}")
}

macro_rules! ints {
    ($($t:ty: $signed:literal),*) => {$(
        impl Stored for $t {
            fn to_json(&self) -> Json {
                json::from_int(i128::from(*self))
            }

            fn from_json(j: &Json) -> Result<Self, String> {
                let n = json::to_int(j).ok_or_else(|| expected("an integer", j))?;
                <$t>::try_from(n).map_err(|_| format!("{n} does not fit a `{}`", stringify!($t)))
            }
        }

        impl Key for $t {
            fn write_key(&self, out: &mut Vec<u8>, _whole: bool) {
                if $signed {
                    encode::write_int(out, i128::from(*self) , <$t>::BITS as u8);
                } else {
                    encode::write_uint(out, i128::from(*self) as u128, <$t>::BITS as u8);
                }
            }
        }

        impl KeyType for $t {
            fn read_key(reader: &mut KeyReader<'_>, _whole: bool) -> Result<Self, String> {
                let n = if $signed {
                    reader.read_int(<$t>::BITS as u8)?
                } else {
                    reader.read_uint(<$t>::BITS as u8)? as i128
                };
                <$t>::try_from(n).map_err(|_| format!("{n} does not fit a `{}`", stringify!($t)))
            }
        }
    )*};
}

ints!(u8: false, u16: false, u32: false, u64: false, i8: true, i16: true, i32: true, i64: true, i128: true);

macro_rules! sized_ints {
    ($($t:ty => $as:ty),*) => {$(
        impl Stored for $t {
            fn to_json(&self) -> Json {
                (*self as $as).to_json()
            }

            fn from_json(j: &Json) -> Result<Self, String> {
                let n = <$as>::from_json(j)?;
                <$t>::try_from(n).map_err(|_| format!("{n} does not fit a `{}`", stringify!($t)))
            }
        }

        impl Key for $t {
            fn write_key(&self, out: &mut Vec<u8>, whole: bool) {
                (*self as $as).write_key(out, whole);
            }
        }

        impl KeyType for $t {
            fn read_key(reader: &mut KeyReader<'_>, whole: bool) -> Result<Self, String> {
                let n = <$as>::read_key(reader, whole)?;
                <$t>::try_from(n).map_err(|_| format!("{n} does not fit a `{}`", stringify!($t)))
            }
        }
    )*};
}

// `usize` and `isize` are stored as 64 bits wide on every target.
sized_ints!(usize => u64, isize => i64);

impl Stored for u128 {
    fn to_json(&self) -> Json {
        json::from_u128(*self)
    }

    fn from_json(j: &Json) -> Result<Self, String> {
        json::to_u128(j).ok_or_else(|| expected("an integer", j))
    }
}

impl Key for u128 {
    fn write_key(&self, out: &mut Vec<u8>, _whole: bool) {
        encode::write_uint(out, *self, 128);
    }
}

impl KeyType for u128 {
    fn read_key(reader: &mut KeyReader<'_>, _whole: bool) -> Result<Self, String> {
        reader.read_uint(128)
    }
}

macro_rules! floats {
    ($($t:ty),*) => {$(
        impl Stored for $t {
            fn to_json(&self) -> Json {
                json::from_float(f64::from(*self))
            }

            fn from_json(j: &Json) -> Result<Self, String> {
                json::to_float(j).map(|x| x as $t).ok_or_else(|| expected("a number", j))
            }
        }
    )*};
}

floats!(f32, f64);

impl Stored for bool {
    fn to_json(&self) -> Json {
        Json::Bool(*self)
    }

    fn from_json(j: &Json) -> Result<Self, String> {
        j.as_bool().ok_or_else(|| expected("a bool", j))
    }
}

impl Key for bool {
    fn write_key(&self, out: &mut Vec<u8>, _whole: bool) {
        encode::write_bool(out, *self);
    }
}

impl KeyType for bool {
    fn read_key(reader: &mut KeyReader<'_>, _whole: bool) -> Result<Self, String> {
        reader.read_bool()
    }
}

impl Stored for char {
    fn to_json(&self) -> Json {
        json::from_char(*self)
    }

    fn from_json(j: &Json) -> Result<Self, String> {
        json::to_char(j).ok_or_else(|| expected("a char", j))
    }
}

impl Key for char {
    fn write_key(&self, out: &mut Vec<u8>, _whole: bool) {
        encode::write_char(out, *self);
    }
}

impl KeyType for char {
    fn read_key(reader: &mut KeyReader<'_>, _whole: bool) -> Result<Self, String> {
        reader.read_char()
    }
}

impl Stored for String {
    fn to_json(&self) -> Json {
        Json::String(self.clone())
    }

    fn from_json(j: &Json) -> Result<Self, String> {
        j.as_str()
            .map(str::to_string)
            .ok_or_else(|| expected("a string", j))
    }
}

impl Key for str {
    fn write_key(&self, out: &mut Vec<u8>, whole: bool) {
        encode::write_str(out, self, whole);
    }
}

impl Key for String {
    fn write_key(&self, out: &mut Vec<u8>, whole: bool) {
        self.as_str().write_key(out, whole);
    }
}

impl KeyType for String {
    fn read_key(reader: &mut KeyReader<'_>, whole: bool) -> Result<Self, String> {
        reader.read_str(whole)
    }
}

impl<K: Key + ?Sized> Key for &K {
    fn write_key(&self, out: &mut Vec<u8>, whole: bool) {
        (**self).write_key(out, whole);
    }
}

impl Stored for () {
    const NULLABLE: bool = true;

    fn to_json(&self) -> Json {
        Json::Null
    }

    fn from_json(j: &Json) -> Result<Self, String> {
        match j {
            Json::Null => Ok(()),
            _ => Err(expected("`null`", j)),
        }
    }
}

impl Stored for Decimal {
    fn to_json(&self) -> Json {
        json::from_decimal(self)
    }

    fn from_json(j: &Json) -> Result<Self, String> {
        json::to_decimal(j).ok_or_else(|| expected("a decimal", j))
    }
}

impl<T: Stored> Stored for Option<T> {
    const NULLABLE: bool = true;

    fn to_json(&self) -> Json {
        match self {
            None => Json::Null,
            Some(inner) => json::some(inner.to_json(), T::NULLABLE),
        }
    }

    fn from_json(j: &Json) -> Result<Self, String> {
        json::unwrap_some(j, T::NULLABLE)?
            .map(T::from_json)
            .transpose()
    }
}

impl<T: Stored, E: Stored> Stored for Result<T, E> {
    fn to_json(&self) -> Json {
        match self {
            Ok(inner) => json::result(true, inner.to_json()),
            Err(inner) => json::result(false, inner.to_json()),
        }
    }

    fn from_json(j: &Json) -> Result<Self, String> {
        match json::to_result(j)? {
            (true, inner) => Ok(Ok(T::from_json(inner)?)),
            (false, inner) => Ok(Err(E::from_json(inner)?)),
        }
    }
}

fn list<T: Stored>(j: &Json) -> Result<impl Iterator<Item = Result<T, String>>, String> {
    let items = j.as_array().ok_or_else(|| expected("a list", j))?;
    Ok(items.iter().map(T::from_json))
}

macro_rules! lists {
    ($($c:ident<$t:ident $(: $($bound:path)+)?>),*) => {$(
        impl<$t: Stored $($(+ $bound)+)?> Stored for $c<$t> {
            fn to_json(&self) -> Json {
                Json::Array(self.iter().map(Stored::to_json).collect())
            }

            fn from_json(j: &Json) -> Result<Self, String> {
                list(j)?.collect()
            }
        }
    )*};
}

lists!(Vec<T>, VecDeque<T>, BTreeSet<T: Ord>, HashSet<T: Eq Hash>);

fn pairs<K: Stored, V: Stored>(
    j: &Json,
) -> Result<impl Iterator<Item = Result<(K, V), String>>, String> {
    let entries = j.as_array().ok_or_else(|| expected("a list of pairs", j))?;
    Ok(entries
        .iter()
        .map(|entry| match entry.as_array().map(Vec::as_slice) {
            Some([k, v]) => Ok((K::from_json(k)?, V::from_json(v)?)),
            _ => Err(expected("a `[key, value]` pair", entry)),
        }))
}

macro_rules! maps {
    ($($c:ident<$k:ident: $($bound:path)+>),*) => {$(
        impl<$k: Stored $(+ $bound)+, V: Stored> Stored for $c<$k, V> {
            fn to_json(&self) -> Json {
                Json::Array(
                    self.iter()
                        .map(|(k, v)| Json::Array(vec![k.to_json(), v.to_json()]))
                        .collect(),
                )
            }

            fn from_json(j: &Json) -> Result<Self, String> {
                pairs(j)?.collect()
            }
        }
    )*};
}

maps!(BTreeMap<K: Ord>, HashMap<K: Eq Hash>);

impl<T: Stored, const N: usize> Stored for [T; N] {
    fn to_json(&self) -> Json {
        Json::Array(self.iter().map(Stored::to_json).collect())
    }

    fn from_json(j: &Json) -> Result<Self, String> {
        let items: Vec<T> = list(j)?.collect::<Result<_, _>>()?;
        let len = items.len();
        items
            .try_into()
            .map_err(|_| format!("expected a list of {N}, found {len}"))
    }
}

macro_rules! wrappers {
    ($($w:ident),*) => {$(
        impl<T: Stored> Stored for $w<T> {
            const NULLABLE: bool = T::NULLABLE;

            fn to_json(&self) -> Json {
                (**self).to_json()
            }

            fn from_json(j: &Json) -> Result<Self, String> {
                T::from_json(j).map($w::new)
            }
        }
    )*};
}

wrappers!(Box, Rc, Arc);

impl<T: Stored> Stored for RefCell<T> {
    const NULLABLE: bool = T::NULLABLE;

    fn to_json(&self) -> Json {
        self.borrow().to_json()
    }

    fn from_json(j: &Json) -> Result<Self, String> {
        T::from_json(j).map(RefCell::new)
    }
}

impl<T: Stored> Stored for Mutex<T> {
    const NULLABLE: bool = T::NULLABLE;

    fn to_json(&self) -> Json {
        self.lock().unwrap_or_else(|e| e.into_inner()).to_json()
    }

    fn from_json(j: &Json) -> Result<Self, String> {
        T::from_json(j).map(Mutex::new)
    }
}

impl<T: Stored> Stored for RwLock<T> {
    const NULLABLE: bool = T::NULLABLE;

    fn to_json(&self) -> Json {
        self.read().unwrap_or_else(|e| e.into_inner()).to_json()
    }

    fn from_json(j: &Json) -> Result<Self, String> {
        T::from_json(j).map(RwLock::new)
    }
}

impl<T: Stored + Copy> Stored for Cell<T> {
    const NULLABLE: bool = T::NULLABLE;

    fn to_json(&self) -> Json {
        self.get().to_json()
    }

    fn from_json(j: &Json) -> Result<Self, String> {
        T::from_json(j).map(Cell::new)
    }
}

macro_rules! tuples {
    ($(($($t:ident $i:tt),+)),*) => {$(
        impl<$($t: Stored),+> Stored for ($($t,)+) {
            fn to_json(&self) -> Json {
                Json::Array(vec![$(self.$i.to_json()),+])
            }

            fn from_json(j: &Json) -> Result<Self, String> {
                let items = j.as_array().ok_or_else(|| expected("a tuple", j))?;
                let len = [$($i),+].len();
                if items.len() != len {
                    return Err(format!("expected a tuple of {len}, found {j}"));
                }
                Ok(($($t::from_json(&items[$i])?,)+))
            }
        }

        impl<$($t: Key),+> Key for ($($t,)+) {
            fn write_key(&self, out: &mut Vec<u8>, _whole: bool) {
                $(self.$i.write_key(out, false);)+
            }
        }

        impl<$($t: KeyType),+> KeyType for ($($t,)+) {
            fn read_key(reader: &mut KeyReader<'_>, _whole: bool) -> Result<Self, String> {
                Ok(($($t::read_key(reader, false)?,)+))
            }
        }
    )*};
}

tuples!(
    (A 0),
    (A 0, B 1),
    (A 0, B 1, C 2),
    (A 0, B 1, C 2, D 3),
    (A 0, B 1, C 2, D 3, E 4),
    (A 0, B 1, C 2, D 3, E 4, F 5),
    (A 0, B 1, C 2, D 3, E 4, F 5, G 6),
    (A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7)
);

/// The fields of a component that hold state, saved before a handler that
/// may be rolled back. The compiler emits `save` and `restore` with it.
pub struct Saved(Vec<Json>);

impl Saved {
    pub fn new(fields: Vec<Json>) -> Saved {
        Saved(fields)
    }

    /// The saved fields, to restore one at a time in the order they were saved.
    pub fn fields(self) -> SavedFields {
        SavedFields(self.0.into_iter())
    }
}

impl Stored for Saved {
    fn to_json(&self) -> Json {
        Json::Array(self.0.clone())
    }

    fn from_json(j: &Json) -> Result<Self, String> {
        Ok(Saved(
            j.as_array().ok_or_else(|| expected("a list", j))?.clone(),
        ))
    }
}

pub struct SavedFields(std::vec::IntoIter<Json>);

impl SavedFields {
    /// Restores the next saved field into `field`.
    pub fn restore<T: Stored>(&mut self, field: &mut T) {
        let saved = self.0.next().expect("each field was saved");
        // The runtime restores only state it saved from the same program.
        *field = T::from_json(&saved).expect("a saved field restores");
    }
}

/// Stores a struct or enum, for the `Stored` implementations the compiler
/// emits: a struct's fields by name, and an enum's variants.
pub mod data {
    use serde_json::{Map, Value as Json};

    use super::Stored;

    pub fn object(fields: Vec<(&str, Json)>) -> Json {
        Json::Object(
            fields
                .into_iter()
                .map(|(name, value)| (name.to_string(), value))
                .collect(),
        )
    }

    /// The fields of a stored struct, read by name.
    pub struct Fields<'j> {
        name: &'static str,
        object: &'j Map<String, Json>,
    }

    pub fn fields<'j>(name: &'static str, j: &'j Json) -> Result<Fields<'j>, String> {
        match j {
            Json::Object(object) => Ok(Fields { name, object }),
            _ => Err(format!("expected a `{name}`, found {j}")),
        }
    }

    impl Fields<'_> {
        pub fn get<T: Stored>(&self, field: &str) -> Result<T, String> {
            let value = self
                .object
                .get(field)
                .ok_or_else(|| format!("missing field `{field}` of `{}`", self.name))?;
            T::from_json(value).map_err(|e| format!("in field `{field}` of `{}`: {e}", self.name))
        }
    }

    pub fn variant(name: &str, fields: Vec<Json>) -> Json {
        inset_value::json::variant(name, fields)
    }

    /// A stored variant's name and fields.
    pub fn to_variant(j: &Json) -> Result<(&str, &[Json]), String> {
        inset_value::json::to_variant(j)
    }

    /// Checks that a stored variant holds `count` fields.
    pub fn arity(variant: &str, fields: &[Json], count: usize) -> Result<(), String> {
        if fields.len() == count {
            Ok(())
        } else {
            Err(format!(
                "`{variant}` holds {count}, but {} are stored",
                fields.len()
            ))
        }
    }

    pub fn unknown_variant<T>(enum_name: &str, variant: &str) -> Result<T, String> {
        Err(format!("`{enum_name}` has no variant `{variant}`"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_agree_with_inset_value() {
        use inset_value::{Shape, Value};
        let int = |bits| Shape::Int { signed: true, bits };
        assert_eq!(
            key_bytes(&-5i32),
            encode::encode(&Value::Int(-5), &int(32)).unwrap()
        );
        assert_eq!(
            key_bytes(&("a\0b".to_string(), 7u8)),
            encode::encode(
                &Value::Tuple(vec![Value::String("a\0b".into()), Value::Int(7)]),
                &Shape::Tuple(vec![
                    Shape::String,
                    Shape::Int {
                        signed: false,
                        bits: 8
                    }
                ])
            )
            .unwrap()
        );
        assert_eq!(key_bytes("bolt-m6"), b"bolt-m6");
        assert_eq!(
            read_key::<(String, u8)>(&key_bytes(&("x".to_string(), 3u8))),
            Ok(("x".into(), 3))
        );
        assert_eq!(read_key::<i64>(&key_bytes(&i64::MIN)), Ok(i64::MIN));
    }

    #[test]
    fn options_round_trip() {
        let values: [Option<Option<u8>>; 3] = [None, Some(None), Some(Some(4))];
        for value in values {
            assert_eq!(Option::<Option<u8>>::from_json(&value.to_json()), Ok(value));
        }
    }
}
