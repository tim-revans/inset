//! The stored form of a table's key: bytes whose order is the key's order.
//!
//! Comparing two encoded keys byte by byte, as SQLite compares a `BLOB` and
//! as a `BTreeMap<Vec<u8>, _>` does, gives the same answer as comparing the
//! keys in Rust. So a table returns its entries in key order without the
//! store knowing the key's type.
//!
//! - An unsigned integer is its bytes, most significant first, at its full
//!   width. A signed one has its sign bit flipped first, so negative numbers
//!   come before positive ones.
//! - `bool` is one byte, `0` or `1`, and `char` is its code point as a `u32`.
//! - A `String` that is the whole key is its UTF-8 bytes, whose byte order
//!   is Rust's order of strings, so a text key reads as itself in the store.
//!   A `String` inside a tuple has each `0` byte written as `0, 255`, and
//!   ends with `0, 0`, so a shorter string comes before a longer one that
//!   starts with it, as in Rust.
//! - A tuple is its members in turn.

use crate::{Shape, Value};

/// Whether values of this shape can be a table's key.
pub fn is_key(shape: &Shape) -> bool {
    match shape {
        Shape::Bool | Shape::Char | Shape::String | Shape::Int { .. } => true,
        Shape::Tuple(items) => items.iter().all(is_key),
        _ => false,
    }
}

/// Whether `n` fits an integer type of this width.
pub fn int_fits(n: i128, signed: bool, bits: u8) -> bool {
    if signed {
        bits >= 128 || (-(1i128 << (bits - 1))..(1i128 << (bits - 1))).contains(&n)
    } else {
        n >= 0 && (bits >= 128 || n < (1i128 << bits))
    }
}

pub fn write_uint(out: &mut Vec<u8>, n: u128, bits: u8) {
    let bytes = n.to_be_bytes();
    out.extend_from_slice(&bytes[16 - usize::from(bits / 8)..]);
}

pub fn write_int(out: &mut Vec<u8>, n: i128, bits: u8) {
    // Flipping the sign bit at this width orders negative numbers first.
    let flipped = (n as u128) ^ (1u128 << (bits - 1));
    let mask = if bits >= 128 {
        u128::MAX
    } else {
        (1u128 << bits) - 1
    };
    write_uint(out, flipped & mask, bits);
}

pub fn write_bool(out: &mut Vec<u8>, b: bool) {
    out.push(u8::from(b));
}

pub fn write_char(out: &mut Vec<u8>, c: char) {
    write_uint(out, u128::from(u32::from(c)), 32);
}

/// A string: as it is when it is the `whole` key, or escaped and ended
/// inside a tuple.
pub fn write_str(out: &mut Vec<u8>, s: &str, whole: bool) {
    if whole {
        out.extend_from_slice(s.as_bytes());
        return;
    }
    for &byte in s.as_bytes() {
        out.push(byte);
        if byte == 0 {
            out.push(255);
        }
    }
    out.extend_from_slice(&[0, 0]);
}

/// Reads an encoded key back, a part at a time.
pub struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Reader<'a> {
        Reader(bytes)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.0.len() < n {
            return Err("the stored key ends too soon".into());
        }
        let (taken, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(taken)
    }

    pub fn read_uint(&mut self, bits: u8) -> Result<u128, String> {
        let bytes = self.take(usize::from(bits / 8))?;
        Ok(bytes.iter().fold(0u128, |n, &b| (n << 8) | u128::from(b)))
    }

    pub fn read_int(&mut self, bits: u8) -> Result<i128, String> {
        let flipped = self.read_uint(bits)? ^ (1u128 << (bits - 1));
        // Sign-extend from `bits` to 128.
        let shift = 128 - u32::from(bits);
        Ok(((flipped << shift) as i128) >> shift)
    }

    pub fn read_bool(&mut self) -> Result<bool, String> {
        match self.take(1)? {
            [0] => Ok(false),
            [1] => Ok(true),
            _ => Err("a stored `bool` key is not 0 or 1".into()),
        }
    }

    pub fn read_char(&mut self) -> Result<char, String> {
        let n = self.read_uint(32)?;
        u32::try_from(n)
            .ok()
            .and_then(char::from_u32)
            .ok_or_else(|| format!("{n} is not a `char`"))
    }

    pub fn read_str(&mut self, whole: bool) -> Result<String, String> {
        let bytes = if whole {
            self.take(self.0.len())?.to_vec()
        } else {
            let mut bytes = Vec::new();
            loop {
                match self.take(1)? {
                    [0] => match self.take(1)? {
                        [0] => break,
                        [255] => bytes.push(0),
                        _ => return Err("a stored text key is not escaped".into()),
                    },
                    [byte] => bytes.push(*byte),
                    _ => unreachable!("one byte was taken"),
                }
            }
            bytes
        };
        String::from_utf8(bytes).map_err(|_| "a stored text key is not UTF-8".into())
    }

    /// Checks that the whole key was read.
    pub fn finish(self) -> Result<(), String> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err("the stored key is longer than its type".into())
        }
    }
}

/// The stored form of a key of this shape.
pub fn encode(value: &Value, shape: &Shape) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    write(&mut out, value, shape, true)?;
    Ok(out)
}

fn write(out: &mut Vec<u8>, value: &Value, shape: &Shape, whole: bool) -> Result<(), String> {
    match (value, shape) {
        (Value::Bool(b), Shape::Bool) => write_bool(out, *b),
        (Value::Char(c), Shape::Char) => write_char(out, *c),
        (Value::String(s), Shape::String) => write_str(out, s, whole),
        (Value::Int(n), Shape::Int { signed, bits }) => {
            if !int_fits(*n, *signed, *bits) {
                return Err(format!("{n} does not fit the key's type"));
            }
            if *signed {
                write_int(out, *n, *bits);
            } else {
                write_uint(out, *n as u128, *bits);
            }
        }
        (Value::Tuple(items), Shape::Tuple(shapes)) if items.len() == shapes.len() => {
            for (item, shape) in items.iter().zip(shapes) {
                write(out, item, shape, false)?;
            }
        }
        _ => return Err(format!("{value} is not a key of this type")),
    }
    Ok(())
}

/// Reads a stored key of this shape.
pub fn decode(bytes: &[u8], shape: &Shape) -> Result<Value, String> {
    let mut reader = Reader::new(bytes);
    let value = read(&mut reader, shape, true)?;
    reader.finish()?;
    Ok(value)
}

fn read(reader: &mut Reader<'_>, shape: &Shape, whole: bool) -> Result<Value, String> {
    Ok(match shape {
        Shape::Bool => Value::Bool(reader.read_bool()?),
        Shape::Char => Value::Char(reader.read_char()?),
        Shape::String => Value::String(reader.read_str(whole)?),
        Shape::Int { signed: true, bits } => Value::Int(reader.read_int(*bits)?),
        Shape::Int {
            signed: false,
            bits,
        } => {
            let n = reader.read_uint(*bits)?;
            Value::Int(i128::try_from(n).map_err(|_| format!("{n} is too large to show"))?)
        }
        Shape::Tuple(shapes) => Value::Tuple(
            shapes
                .iter()
                .map(|s| read(reader, s, false))
                .collect::<Result<_, _>>()?,
        ),
        _ => return Err("this type cannot be a key".into()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(values: &[Value], shape: &Shape) -> Vec<String> {
        let mut keys: Vec<Vec<u8>> = values.iter().map(|v| encode(v, shape).unwrap()).collect();
        keys.sort();
        keys.iter()
            .map(|k| decode(k, shape).unwrap().to_string())
            .collect()
    }

    #[test]
    fn signed_keys_sort_negative_first() {
        let shape = Shape::Int {
            signed: true,
            bits: 32,
        };
        let values: Vec<Value> = [7, -1, 0, i32::MIN as i128, -300, i32::MAX as i128, 300]
            .into_iter()
            .map(Value::Int)
            .collect();
        assert_eq!(
            sorted(&values, &shape),
            ["-2147483648", "-300", "-1", "0", "7", "300", "2147483647"]
        );
    }

    #[test]
    fn text_keys_sort_as_rust_sorts_strings() {
        let shape = Shape::String;
        let values: Vec<Value> = ["b", "", "ab", "a", "a\0", "é", "Z"]
            .into_iter()
            .map(|s| Value::String(s.into()))
            .collect();
        assert_eq!(
            sorted(&values, &shape),
            [
                r#""""#, r#""Z""#, r#""a""#, r#""a\0""#, r#""ab""#, r#""b""#, r#""é""#
            ]
        );
        assert_eq!(
            encode(&Value::String("bolt".into()), &shape).unwrap(),
            b"bolt"
        );
    }

    #[test]
    fn tuple_keys_sort_by_each_member_in_turn() {
        let shape = Shape::Tuple(vec![
            Shape::String,
            Shape::Int {
                signed: true,
                bits: 8,
            },
        ]);
        let pair = |s: &str, n: i128| Value::Tuple(vec![Value::String(s.into()), Value::Int(n)]);
        let values = [
            pair("a", 5),
            pair("", 9),
            pair("a\0", -1),
            pair("a", -5),
            pair("ab", 0),
        ];
        assert_eq!(
            sorted(&values, &shape),
            [
                r#"("", 9)"#,
                r#"("a", -5)"#,
                r#"("a", 5)"#,
                r#"("a\0", -1)"#,
                r#"("ab", 0)"#
            ]
        );
    }

    #[test]
    fn wide_integers_keep_their_order() {
        let shape = Shape::Int {
            signed: true,
            bits: 128,
        };
        let values: Vec<Value> = [i128::MAX, -1, i128::MIN, 0]
            .into_iter()
            .map(Value::Int)
            .collect();
        assert_eq!(
            sorted(&values, &shape),
            [
                i128::MIN.to_string(),
                "-1".into(),
                "0".into(),
                i128::MAX.to_string()
            ]
        );
    }
}
