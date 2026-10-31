//! The code dialect's recursive-descent parser.
//!
//! Each `parse_*` method reads one construct. On an error it reports a
//! diagnostic and returns `Err(Reported)`, and the nearest list being read
//! (items, members of an app or component, or statements of a block)
//! skips to where the next entry starts and carries on.

mod expr;
mod fact;
mod items;

pub(crate) use fact::StepFact;

use crate::lexer::{Keyword as Kw, Punct as P, Token, TokenKind as T};
use crate::{Diagnostic, Span};

/// An error was reported as a diagnostic.
pub(crate) struct Reported;

pub(crate) type PResult<T> = Result<T, Reported>;

/// How deeply expressions, types, patterns and blocks may nest. Deeper input
/// is reported rather than risking the stack.
const MAX_DEPTH: usize = 64;

pub(crate) struct Parser<'a> {
    source: &'a str,
    /// Where the source starts among the offsets of a whole program.
    base: usize,
    tokens: Vec<Token>,
    pos: usize,
    depth: usize,
    /// Where the most recent error was raised, which recovery skips past.
    error_pos: usize,
    /// Each error, with the offset of the token the parser was at when it
    /// found it.
    pub(crate) diagnostics: Vec<(usize, Diagnostic)>,
}

/// What ends a list's entry, for skipping past one that failed to parse.
#[derive(Clone, Copy)]
enum List {
    Items,
    Members,
    Stmts,
    /// The arms of a `failure` block.
    Arms,
}

impl<'a> Parser<'a> {
    pub(crate) fn new(source: &'a str, base: usize, tokens: Vec<Token>) -> Parser<'a> {
        Parser {
            source,
            base,
            tokens,
            pos: 0,
            depth: 0,
            error_pos: 0,
            diagnostics: Vec::new(),
        }
    }

    // ---- Tokens ----------------------------------------------------------

    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn kind(&self) -> &T {
        &self.peek().kind
    }

    fn nth(&self, n: usize) -> &T {
        let last = self.tokens.len() - 1;
        &self.tokens[(self.pos + n).min(last)].kind
    }

    fn span(&self) -> Span {
        self.peek().span
    }

    fn prev_span(&self) -> Span {
        if self.pos == 0 {
            Span::new(self.base, self.base)
        } else {
            self.tokens[self.pos - 1].span
        }
    }

    fn bump(&mut self) -> Token {
        let token = self.peek().clone();
        if token.kind != T::Eof {
            self.pos += 1;
        }
        token
    }

    fn at(&self, p: P) -> bool {
        *self.kind() == T::Punct(p)
    }

    fn at_kw(&self, kw: Kw) -> bool {
        *self.kind() == T::Keyword(kw)
    }

    fn eat(&mut self, p: P) -> bool {
        let found = self.at(p);
        if found {
            self.bump();
        }
        found
    }

    fn eat_kw(&mut self, kw: Kw) -> bool {
        let found = self.at_kw(kw);
        if found {
            self.bump();
        }
        found
    }

    /// Takes the first character of a joined token such as `&&` or `>>=`,
    /// leaving the rest, so `&&x` can be read as `& &x` and `Vec<Vec<u8>>`
    /// can close two generic lists.
    fn split_first(&mut self, first: P, rest: P) {
        let token = &mut self.tokens[self.pos];
        let start = token.span.start;
        token.kind = T::Punct(rest);
        token.span.start += first.as_str().len();
        self.tokens.insert(
            self.pos,
            Token {
                kind: T::Punct(first),
                span: Span::new(start, start + first.as_str().len()),
            },
        );
    }

    /// Eats `>`, splitting `>>`, `>=` and `>>=` where they close generics.
    fn eat_gt(&mut self) -> bool {
        match self.kind() {
            T::Punct(P::Shr) => self.split_first(P::Gt, P::Gt),
            T::Punct(P::Ge) => self.split_first(P::Gt, P::Eq),
            T::Punct(P::ShrEq) => self.split_first(P::Gt, P::Ge),
            _ => {}
        }
        self.eat(P::Gt)
    }

    /// Splits `&&` so a reference to a reference reads as two `&`.
    fn split_and_and(&mut self) {
        if self.at(P::AndAnd) {
            self.split_first(P::And, P::And);
        }
    }

    // ---- Errors ----------------------------------------------------------

    fn report(&mut self, diagnostic: Diagnostic) -> Reported {
        self.error_pos = self.pos;
        // One mistake often confuses the parser at the same spot more than
        // once, such as an unclosed `{` at the end of the file. Only the
        // first is useful.
        let repeated = self
            .diagnostics
            .last()
            .is_some_and(|(_, last)| last.span == diagnostic.span);
        if !repeated {
            self.diagnostics.push((self.span().start, diagnostic));
        }
        Reported
    }

    /// Reports that `what` was expected where the next token is.
    fn expected(&mut self, what: &str) -> Reported {
        let found = self.kind().to_string();
        let span = self.span();
        self.report(Diagnostic::new(
            format!("expected {what}, found {found}"),
            span,
            format!("expected {what}"),
        ))
    }

    /// Expects punctuation that ends or separates something, such as `;`.
    /// When the next token is on a later line, the label goes after the
    /// previous token, which is where the punctuation is missing.
    fn expect(&mut self, p: P, after: &str) -> PResult<Span> {
        if self.at(p) {
            return Ok(self.bump().span);
        }
        let prev = self.prev_span();
        let found = self.kind().to_string();
        let (span, label) = if self.pos > 0 && self.starts_later_line(prev) {
            (prev, format!("expected `{}` after this", p.as_str()))
        } else {
            (self.span(), format!("expected `{}`", p.as_str()))
        };
        Err(self.report(Diagnostic::new(
            format!("expected `{}` after {after}, found {found}", p.as_str()),
            span,
            label,
        )))
    }

    /// Whether the next token starts on a line after the one `prev` ends on.
    fn starts_later_line(&self, prev: Span) -> bool {
        self.source
            .get(prev.end - self.base..self.span().start - self.base)
            .is_some_and(|gap| gap.contains('\n'))
    }

    fn unsupported(&mut self, what: &str, span: Span) -> Reported {
        self.report(Diagnostic::new(
            format!("{what} is not supported yet"),
            span,
            "the parser does not handle this yet",
        ))
    }

    /// Expects a closing delimiter, reporting the opening one when the file
    /// ends first.
    fn expect_close(&mut self, close: P, open: Span, after: &str) -> PResult<Span> {
        if *self.kind() == T::Eof {
            let open_text = match close {
                P::RBrace => "{",
                P::RParen => "(",
                _ => "[",
            };
            return Err(self.report(Diagnostic::new(
                format!("unclosed `{open_text}`"),
                open,
                format!("this `{open_text}` has no matching `{}`", close.as_str()),
            )));
        }
        self.expect(close, after)
    }

    fn enter(&mut self) -> PResult<()> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            let span = self.span();
            return Err(self.report(
                Diagnostic::new(
                    "the code is nested too deeply",
                    span,
                    format!("this is more than {MAX_DEPTH} levels deep"),
                )
                .with_help("move the inner part into a function of its own"),
            ));
        }
        Ok(())
    }

    fn leave(&mut self) {
        self.depth -= 1;
    }

    fn nested<R>(&mut self, f: impl FnOnce(&mut Parser<'a>) -> PResult<R>) -> PResult<R> {
        let entered = self.enter();
        let result = entered.and_then(|()| f(self));
        self.leave();
        result
    }

    /// Skips the rest of a list entry that failed to parse, from its first
    /// token at `start`. Brackets are skipped whole. It stops after a `;` or
    /// a closing `}` at the entry's level, before a bracket that closes the
    /// enclosing list, and before a token that starts a new entry once past
    /// the error.
    fn recover(&mut self, start: usize, list: List) {
        self.pos = start;
        let mut depth = 0usize;
        loop {
            let past_error = self.pos >= self.error_pos && self.pos > start;
            match self.kind() {
                T::Eof => return,
                T::Punct(P::LBrace | P::LParen | P::LBracket) => depth += 1,
                T::Punct(P::RBrace | P::RParen | P::RBracket) if depth == 0 => {
                    // A stray `)` or `]` is skipped, so the list moves on.
                    // A `}` closes the list, which then ends.
                    if self.pos == start && !self.at(P::RBrace) {
                        self.bump();
                    }
                    return;
                }
                T::Punct(P::RBrace) if depth == 1 && past_error => {
                    self.bump();
                    self.eat(P::Semi);
                    return;
                }
                T::Punct(P::RBrace | P::RParen | P::RBracket) => depth -= 1,
                T::Punct(P::Comma) if depth == 0 && matches!(list, List::Arms) => {
                    self.bump();
                    return;
                }
                T::Punct(P::Semi) if depth == 0 => {
                    self.bump();
                    return;
                }
                kind if depth == 0 && past_error && starts_entry(kind, list) => return,
                _ => {}
            }
            self.bump();
        }
    }
}

/// Whether a token can start a new entry of the list, for recovery.
fn starts_entry(kind: &T, list: List) -> bool {
    match list {
        List::Items => matches!(
            kind,
            T::Doc(_)
                | T::Punct(P::Pound)
                | T::Keyword(
                    Kw::Pub
                        | Kw::App
                        | Kw::Component
                        | Kw::Fn
                        | Kw::Async
                        | Kw::Struct
                        | Kw::Enum
                        | Kw::Extern
                        | Kw::Mod
                        | Kw::Use
                )
        ),
        List::Members => matches!(
            kind,
            T::Doc(_)
                | T::Punct(P::Pound)
                | T::Keyword(
                    Kw::Pub
                        | Kw::Fn
                        | Kw::Async
                        | Kw::Setting
                        | Kw::Grant
                        | Kw::Start
                        | Kw::Failure
                )
        ),
        List::Stmts => matches!(kind, T::Keyword(Kw::Let)),
        List::Arms => matches!(kind, T::Doc(_)),
    }
}
