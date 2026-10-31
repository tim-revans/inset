//! A requirement's check: a Rust condition on what the fake tables hold,
//! such as `products["bolt-m6"].gross >= dec!(0.12)`. The checker types it
//! and resolves each part, and the test runner evaluates it here.

use std::cmp::Ordering;

use crate::{Shape, Types, Value, json};

/// Reads the stored value a table holds under a stored key: the table's
/// grant, then the key.
pub type Lookup<'r> = dyn FnMut(&str, &[u8]) -> Option<Vec<u8>> + 'r;

/// A binary operator in a check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

/// A check, with its names resolved and its values typed.
#[derive(Clone, Debug, PartialEq)]
pub enum Check {
    Value(Value),
    /// `name[key]`: the value the table granted as `grant` holds under the key.
    Entry {
        grant: String,
        /// The key's stored form.
        key: Vec<u8>,
        /// The key, for a failure to name.
        key_value: Value,
        /// The shape of the table's values.
        shape: Shape,
    },
    Field(Box<Check>, String),
    /// `.0`, `.1` and so on, on a tuple.
    TupleField(Box<Check>, usize),
    Not(Box<Check>),
    Neg(Box<Check>),
    Binary(Op, Box<Check>, Box<Check>),
    /// `.len()` of text or a collection.
    Len(Box<Check>),
    /// `.is_empty()` of text or a collection.
    IsEmpty(Box<Check>),
}

/// Why a check could not be evaluated.
#[derive(Clone, Debug, PartialEq)]
pub enum CheckError {
    /// The table holds nothing under a key the check reads.
    Missing { grant: String, key: Value },
    /// Anything else, such as dividing by zero or a stored value that does
    /// not fit its type.
    Failed(String),
}

impl Check {
    /// Evaluates the check. `read` gives the stored value a table holds
    /// under a stored key.
    pub fn eval(&self, read: &mut Lookup<'_>, types: &Types) -> Result<Value, CheckError> {
        let failed = |message: String| CheckError::Failed(message);
        Ok(match self {
            Check::Value(value) => value.clone(),
            Check::Entry {
                grant,
                key,
                key_value,
                shape,
            } => {
                let Some(stored) = read(grant, key) else {
                    return Err(CheckError::Missing {
                        grant: grant.clone(),
                        key: key_value.clone(),
                    });
                };
                let stored: serde_json::Value = serde_json::from_slice(&stored).map_err(|e| {
                    failed(format!("{grant}[{key_value}] is not stored as JSON: {e}"))
                })?;
                json::decode(&stored, shape, types).map_err(|e| {
                    failed(format!("{grant}[{key_value}] does not fit its type: {e}"))
                })?
            }
            Check::Field(base, name) => {
                let base = base.eval(read, types)?;
                base.field(name)
                    .cloned()
                    .ok_or_else(|| failed(format!("{base} has no field `{name}`")))?
            }
            Check::TupleField(base, index) => match base.eval(read, types)? {
                Value::Tuple(mut items) if *index < items.len() => items.swap_remove(*index),
                other => return Err(failed(format!("{other} has no field `{index}`"))),
            },
            Check::Not(inner) => match inner.eval(read, types)? {
                Value::Bool(b) => Value::Bool(!b),
                other => return Err(failed(format!("`!` needs a bool, not {other}"))),
            },
            Check::Neg(inner) => match inner.eval(read, types)? {
                Value::Int(n) => Value::Int(-n),
                Value::Float(x) => Value::Float(-x),
                Value::Decimal(d) => Value::Decimal(-d),
                other => return Err(failed(format!("`-` needs a number, not {other}"))),
            },
            Check::Len(inner) | Check::IsEmpty(inner) => {
                let len = match inner.eval(read, types)? {
                    Value::String(s) => s.len(),
                    Value::List(items) | Value::Set(items) => items.len(),
                    Value::Map(entries) => entries.len(),
                    other => return Err(failed(format!("{other} has no length"))),
                };
                if matches!(self, Check::Len(_)) {
                    Value::Int(len as i128)
                } else {
                    Value::Bool(len == 0)
                }
            }
            Check::Binary(Op::And, lhs, rhs) => match lhs.eval(read, types)? {
                Value::Bool(false) => Value::Bool(false),
                Value::Bool(true) => rhs.eval(read, types)?,
                other => return Err(failed(format!("`&&` needs a bool, not {other}"))),
            },
            Check::Binary(Op::Or, lhs, rhs) => match lhs.eval(read, types)? {
                Value::Bool(true) => Value::Bool(true),
                Value::Bool(false) => rhs.eval(read, types)?,
                other => return Err(failed(format!("`||` needs a bool, not {other}"))),
            },
            Check::Binary(op, lhs, rhs) => {
                let lhs = lhs.eval(read, types)?;
                let rhs = rhs.eval(read, types)?;
                binary(*op, &lhs, &rhs).map_err(failed)?
            }
        })
    }
}

fn binary(op: Op, lhs: &Value, rhs: &Value) -> Result<Value, String> {
    let order = || {
        lhs.compare(rhs)
            .ok_or_else(|| format!("{lhs} and {rhs} cannot be ordered"))
    };
    let overflow = || format!("{lhs} {} {rhs} overflows", symbol(op));
    Ok(match op {
        Op::Eq => Value::Bool(lhs == rhs),
        Op::Ne => Value::Bool(lhs != rhs),
        Op::Lt => Value::Bool(order()? == Ordering::Less),
        Op::Le => Value::Bool(order()? != Ordering::Greater),
        Op::Gt => Value::Bool(order()? == Ordering::Greater),
        Op::Ge => Value::Bool(order()? != Ordering::Less),
        Op::And | Op::Or => unreachable!("evaluated lazily"),
        Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Rem => match (lhs, rhs) {
            (Value::Int(a), Value::Int(b)) => Value::Int(
                match op {
                    Op::Add => a.checked_add(*b),
                    Op::Sub => a.checked_sub(*b),
                    Op::Mul => a.checked_mul(*b),
                    Op::Div => a.checked_div(*b),
                    _ => a.checked_rem(*b),
                }
                .ok_or_else(overflow)?,
            ),
            (Value::Decimal(a), Value::Decimal(b)) => Value::Decimal(
                match op {
                    Op::Add => a.checked_add(*b),
                    Op::Sub => a.checked_sub(*b),
                    Op::Mul => a.checked_mul(*b),
                    Op::Div => a.checked_div(*b),
                    _ => a.checked_rem(*b),
                }
                .ok_or_else(overflow)?,
            ),
            (Value::Float(a), Value::Float(b)) => Value::Float(match op {
                Op::Add => a + b,
                Op::Sub => a - b,
                Op::Mul => a * b,
                Op::Div => a / b,
                _ => a % b,
            }),
            (Value::String(a), Value::String(b)) if op == Op::Add => {
                Value::String(format!("{a}{b}"))
            }
            _ => {
                return Err(format!("`{}` cannot combine {lhs} and {rhs}", symbol(op)));
            }
        },
    })
}

/// How an operator is written.
pub fn symbol(op: Op) -> &'static str {
    match op {
        Op::Eq => "==",
        Op::Ne => "!=",
        Op::Lt => "<",
        Op::Le => "<=",
        Op::Gt => ">",
        Op::Ge => ">=",
        Op::And => "&&",
        Op::Or => "||",
        Op::Add => "+",
        Op::Sub => "-",
        Op::Mul => "*",
        Op::Div => "/",
        Op::Rem => "%",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_check_reads_an_entry_and_compares_a_field() {
        let types = Types(vec![crate::DataShape {
            name: "Product".into(),
            kind: crate::DataKind::Struct(vec![("gross".into(), Shape::Decimal)]),
        }]);
        let entry = Check::Entry {
            grant: "products".into(),
            key: b"bolt-m6".to_vec(),
            key_value: Value::String("bolt-m6".into()),
            shape: Shape::Data(0),
        };
        let gross = Check::Field(Box::new(entry), "gross".into());
        let check = |op, n: &str| {
            Check::Binary(
                op,
                Box::new(gross.clone()),
                Box::new(Check::Value(Value::Decimal(n.parse().unwrap()))),
            )
        };
        let mut read = |grant: &str, key: &[u8]| {
            (grant == "products" && key == b"bolt-m6").then(|| br#"{"gross":"0.12"}"#.to_vec())
        };
        assert_eq!(
            check(Op::Ge, "0.10").eval(&mut read, &types),
            Ok(Value::Bool(true))
        );
        assert_eq!(
            check(Op::Eq, "0.120").eval(&mut read, &types),
            Ok(Value::Bool(true))
        );
        assert_eq!(
            check(Op::Lt, "0.12").eval(&mut read, &types),
            Ok(Value::Bool(false))
        );
        let mut empty = |_: &str, _: &[u8]| None;
        assert_eq!(
            check(Op::Ge, "0").eval(&mut empty, &types),
            Err(CheckError::Missing {
                grant: "products".into(),
                key: Value::String("bolt-m6".into())
            })
        );
    }
}
