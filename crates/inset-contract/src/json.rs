//! JSON with its keys in the order they are added, written with two
//! spaces of indentation, so the OpenAPI document reads in the order a
//! person expects (`openapi`, `info`, `paths`, `components`) rather than
//! the alphabetical order `serde_json` keeps without its `preserve_order`
//! feature, which would change the order of every other JSON the workspace
//! writes.

use std::fmt::Write;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum J {
    Null,
    Bool(bool),
    /// A number, as written.
    Num(String),
    Str(String),
    Arr(Vec<J>),
    Obj(Vec<(String, J)>),
}

impl J {
    pub(crate) fn str(s: impl Into<String>) -> J {
        J::Str(s.into())
    }

    pub(crate) fn num(n: impl ToString) -> J {
        J::Num(n.to_string())
    }

    pub(crate) fn obj<const N: usize>(entries: [(&str, J); N]) -> J {
        J::Obj(
            entries
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        )
    }

    /// Adds an entry to an object.
    pub(crate) fn with(mut self, key: &str, value: J) -> J {
        if let J::Obj(entries) = &mut self {
            entries.push((key.to_string(), value));
        }
        self
    }

    pub(crate) fn write(&self) -> String {
        let mut out = String::new();
        self.write_at(&mut out, 0);
        out.push('\n');
        out
    }

    fn write_at(&self, out: &mut String, depth: usize) {
        let indent = |out: &mut String, depth: usize| {
            out.push('\n');
            out.push_str(&"  ".repeat(depth));
        };
        match self {
            J::Null => out.push_str("null"),
            J::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            J::Num(n) => out.push_str(n),
            J::Str(s) => quote(out, s),
            J::Arr(items) if items.is_empty() => out.push_str("[]"),
            J::Obj(entries) if entries.is_empty() => out.push_str("{}"),
            J::Arr(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    indent(out, depth + 1);
                    item.write_at(out, depth + 1);
                }
                indent(out, depth);
                out.push(']');
            }
            J::Obj(entries) => {
                out.push('{');
                for (i, (key, value)) in entries.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    indent(out, depth + 1);
                    quote(out, key);
                    out.push_str(": ");
                    value.write_at(out, depth + 1);
                }
                indent(out, depth);
                out.push('}');
            }
        }
    }
}

fn quote(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}
