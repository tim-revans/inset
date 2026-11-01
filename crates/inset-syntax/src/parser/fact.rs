//! A requirement's fact or check, read with the code dialect's parser,
//! because the values and conditions in them are Rust expressions.
//!
//! - `name[key] is value`
//! - `name is empty`
//! - `name fails`
//! - `name["METHOD /path"] replies Status`, optionally `with body`
//! - any other expression is a check, allowed after `then` only.

use super::{PResult, Parser};
use crate::ast::{Expr, ExprKind, Fact, FactKind, Input};
use crate::lexer::{Keyword as Kw, Punct as P, TokenKind as T};
use crate::{Diagnostic, Ident, Span};

/// A step's fact or check.
pub(crate) enum StepFact {
    Fact(Box<Fact>),
    Check(Expr),
}

impl Parser<'_> {
    /// A step's body written as a value, after `with`: one expression.
    pub(crate) fn parse_step_value(&mut self) -> PResult<Expr> {
        let value = self.parse_expr()?;
        if *self.kind() != T::Eof {
            let span = self.span();
            return Err(self.report(Diagnostic::new(
                "unexpected text after the body",
                span,
                "the body is complete before this",
            )));
        }
        Ok(value)
    }

    pub(crate) fn parse_step_fact(&mut self, allow_check: bool) -> PResult<StepFact> {
        let start = self.span().start;
        let lhs = self.parse_expr()?;
        let word = match self.kind() {
            T::Ident(word) if word == "is" || word == "fails" || word == "replies" => {
                Some(word.clone())
            }
            _ => None,
        };
        match word.as_deref() {
            Some("replies") => {
                let ExprKind::Index { base, index } = &lhs.kind else {
                    return Err(self.report(
                        Diagnostic::new(
                            "expected a call after the client's name",
                            lhs.span,
                            "`replies` states how a call is answered",
                        )
                        .with_help("write `name[\"GET /path\"] replies OK with \"...\"`"),
                    ));
                };
                let grant = self.grant_name(base, "a fact")?;
                let key = index.clone();
                self.bump();
                let status = match self.kind() {
                    T::Ident(name) if name.starts_with(|c: char| c.is_ascii_uppercase()) => {
                        let ident = Ident::new(name.clone(), self.span());
                        self.bump();
                        ident
                    }
                    _ => return Err(self.expected("a status after `replies`, such as `OK`")),
                };
                let body = if matches!(self.kind(), T::Ident(w) if w == "with") {
                    self.bump();
                    Some(Box::new(self.parse_expr()?))
                } else {
                    None
                };
                self.expect_end()?;
                Ok(StepFact::Fact(Box::new(Fact {
                    grant,
                    kind: FactKind::Replies { key, status, body },
                    span: Span::new(start, self.prev_span().end),
                })))
            }
            Some("fails") => {
                let grant = self.grant_name(&lhs, "`fails`")?;
                self.bump();
                self.expect_end()?;
                Ok(StepFact::Fact(Box::new(Fact {
                    grant,
                    kind: FactKind::Fails,
                    span: Span::new(start, self.prev_span().end),
                })))
            }
            Some(_) => {
                let entry = match &lhs.kind {
                    ExprKind::Index { base, index } => Some((base, index)),
                    _ => None,
                };
                let grant = match entry {
                    Some((base, _)) => self.grant_name(base, "a fact")?,
                    None => self.grant_name(&lhs, "a fact")?,
                };
                self.bump();
                let empty =
                    matches!(self.kind(), T::Ident(w) if w == "empty") && *self.nth(1) == T::Eof;
                let kind = match (entry, empty) {
                    (None, true) => {
                        self.bump();
                        FactKind::Empty
                    }
                    (Some(_), true) => {
                        let span = self.span();
                        return Err(self.report(
                            Diagnostic::new(
                                "an entry is not empty",
                                span,
                                "`is empty` is stated about a whole table",
                            )
                            .with_help(format!(
                                "write `{} is empty`, or give the value under the key",
                                grant.name
                            )),
                        ));
                    }
                    (None, false) => return Err(self.not_an_entry(&lhs)),
                    (Some((_, index)), false) => {
                        let key = index.clone();
                        let value = Box::new(self.parse_expr()?);
                        self.expect_end()?;
                        FactKind::Entry { key, value }
                    }
                };
                Ok(StepFact::Fact(Box::new(Fact {
                    grant,
                    kind,
                    span: Span::new(start, self.prev_span().end),
                })))
            }
            None if *self.kind() == T::Eof => {
                if allow_check {
                    Ok(StepFact::Check(lhs))
                } else {
                    let span = lhs.span;
                    Err(self.report(
                        Diagnostic::new(
                            "expected a fact after `given`",
                            span,
                            "a `given` step states what a capability holds",
                        )
                        .with_help(
                            "write `name[key] is value`, `name is empty` or `name fails`; a \
                             condition to check goes after `then`",
                        ),
                    ))
                }
            }
            None => {
                let found = self.kind().to_string();
                let span = self.span();
                let help = if allow_check {
                    "a fact is `name[key] is value`, `name is empty` or `name fails`, and \
                     anything else after `then` is a Rust condition"
                } else {
                    "a fact is `name[key] is value`, `name is empty` or `name fails`"
                };
                Err(self.report(
                    Diagnostic::new(
                        format!("expected `is` or `fails`, found {found}"),
                        span,
                        "the fact goes on here",
                    )
                    .with_help(help),
                ))
            }
        }
    }

    /// The grant a fact names: a plain name.
    fn grant_name(&mut self, expr: &Expr, what: &str) -> PResult<Ident> {
        if let ExprKind::Path(path) = &expr.kind
            && let [only] = path.segments.as_slice()
            && only.generics.is_empty()
        {
            return Ok(only.ident.clone());
        }
        Err(self.report(
            Diagnostic::new(
                format!("{what} is stated about a capability, by the name of its grant"),
                expr.span,
                "not the name of a grant",
            )
            .with_help("name the capability as the app's `grant` does, such as `items`"),
        ))
    }

    fn not_an_entry(&mut self, lhs: &Expr) -> super::Reported {
        self.report(
            Diagnostic::new(
                "expected a key after the capability's name",
                lhs.span,
                "`is` states the value under a key",
            )
            .with_help("write `name[key] is value`, or `name is empty`"),
        )
    }

    fn expect_end(&mut self) -> PResult<()> {
        if *self.kind() == T::Eof {
            return Ok(());
        }
        let span = self.span();
        let label = if self.at(P::Comma) {
            "a step states one fact"
        } else {
            "the fact is complete before this"
        };
        Err(self.report(Diagnostic::new(
            "unexpected text at the end of the step",
            span,
            label,
        )))
    }
}

/// A property's inputs and its `where` condition.
pub(crate) type ForAllParts = (Vec<Input>, Option<Expr>);

impl Parser<'_> {
    /// A property's `for all name: Type, ... where condition`, after the
    /// words `for all`.
    pub(crate) fn parse_for_all(&mut self) -> PResult<ForAllParts> {
        let mut inputs = Vec::new();
        loop {
            let name = self.parse_ident("an input's name")?;
            self.expect(P::Colon, "the input's name")?;
            let ty = self.parse_type()?;
            inputs.push(Input { name, ty });
            if !self.eat(P::Comma) {
                break;
            }
        }
        let condition = if self.eat_kw(Kw::Where) {
            Some(self.parse_expr()?)
        } else {
            None
        };
        if *self.kind() != T::Eof {
            return Err(self.expected("`where`, `,` or the end of the line"));
        }
        Ok((inputs, condition))
    }
}
