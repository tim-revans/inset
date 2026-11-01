//! Types, paths, patterns, blocks and expressions: the Rust inside Inset.

use super::{List, PResult, Parser, Reported};
use crate::ast::*;
use crate::lexer::{Keyword as Kw, Punct as P, TokenKind as T};
use crate::{Diagnostic, Ident, Span};

impl Parser<'_> {
    // ---- Names, types and paths ------------------------------------------

    pub(super) fn parse_ident(&mut self, what: &str) -> PResult<Ident> {
        match self.kind().clone() {
            T::Ident(name) => Ok(Ident::new(name, self.bump().span)),
            T::Keyword(kw) => Err(self.keyword_as_name(kw, what)),
            _ => Err(self.expected(what)),
        }
    }

    pub(super) fn keyword_as_name(&mut self, kw: Kw, what: &str) -> Reported {
        let span = self.span();
        let help = if kw.is_inset_only() {
            format!(
                "`{}` is a keyword in Inset, though not in Rust; choose another name",
                kw.as_str()
            )
        } else {
            "choose another name".to_string()
        };
        self.report(
            Diagnostic::new(
                format!("expected {what}, found keyword `{}`", kw.as_str()),
                span,
                "this is a keyword, so it cannot be a name",
            )
            .with_help(help),
        )
    }

    pub(super) fn parse_type(&mut self) -> PResult<Type> {
        self.nested(Parser::parse_type_inner)
    }

    pub(super) fn parse_type_inner(&mut self) -> PResult<Type> {
        let start = self.span();
        self.split_and_and();
        match self.kind() {
            T::Punct(P::And) => {
                self.bump();
                let mutable = self.eat_kw(Kw::Mut);
                let inner = self.parse_type()?;
                Ok(Type {
                    span: start.to(inner.span),
                    kind: TypeKind::Ref {
                        mutable,
                        inner: Box::new(inner),
                    },
                })
            }
            T::Punct(P::LParen) => {
                self.bump();
                let mut elems = Vec::new();
                let mut trailing_comma = false;
                while !self.at(P::RParen) {
                    elems.push(self.parse_type()?);
                    trailing_comma = self.eat(P::Comma);
                    if !trailing_comma {
                        break;
                    }
                }
                let end = self.expect_close(P::RParen, start, "the tuple's types")?;
                if elems.len() == 1 && !trailing_comma {
                    return Ok(elems.remove(0));
                }
                Ok(Type {
                    kind: TypeKind::Tuple(elems),
                    span: start.to(end),
                })
            }
            T::Punct(P::LBracket) => {
                self.bump();
                let inner = self.parse_type()?;
                if self.at(P::Semi) {
                    let span = self.span();
                    return Err(self.unsupported("an array type", span));
                }
                let end = self.expect_close(P::RBracket, start, "the slice's type")?;
                Ok(Type {
                    kind: TypeKind::Slice(Box::new(inner)),
                    span: start.to(end),
                })
            }
            T::Punct(P::Underscore) => Ok(Type {
                kind: TypeKind::Infer,
                span: self.bump().span,
            }),
            T::Keyword(Kw::Dyn) => {
                let span = self.span();
                Err(self.report(
                    Diagnostic::new(
                        "Inset has no `dyn Trait`",
                        span,
                        "dynamic dispatch hides which code runs",
                    )
                    .with_help("use an enum or a concrete type"),
                ))
            }
            T::Keyword(Kw::Impl | Kw::Fn) => {
                let span = self.span();
                Err(self.unsupported("this kind of type", span))
            }
            _ => {
                let path = self.parse_path(false)?;
                Ok(Type {
                    span: path.span,
                    kind: TypeKind::Path(path),
                })
            }
        }
    }

    pub(super) fn at_path_start(&self) -> bool {
        matches!(
            self.kind(),
            T::Ident(_) | T::Keyword(Kw::SelfValue | Kw::SelfType | Kw::Super | Kw::Crate)
        )
    }

    /// A path in a type, where generics follow a segment directly, or in an
    /// expression, where they need `::` first, as in `collect::<Vec<_>>`.
    pub(super) fn parse_path(&mut self, in_expr: bool) -> PResult<Path> {
        let start = self.span();
        let mut segments = vec![self.parse_path_segment(in_expr)?];
        while self.at(P::PathSep) && !matches!(self.nth(1), T::Punct(P::Lt)) {
            self.bump();
            segments.push(self.parse_path_segment(in_expr)?);
        }
        Ok(Path {
            segments,
            span: start.to(self.prev_span()),
        })
    }

    pub(super) fn parse_path_segment(&mut self, in_expr: bool) -> PResult<PathSegment> {
        let ident = match self.kind().clone() {
            T::Ident(name) => Ident::new(name, self.bump().span),
            T::Keyword(kw @ (Kw::SelfValue | Kw::SelfType | Kw::Super | Kw::Crate)) => {
                Ident::new(kw.as_str(), self.bump().span)
            }
            _ => return Err(self.expected(if in_expr { "an expression" } else { "a type" })),
        };
        let generics = if in_expr {
            if self.at(P::PathSep) && matches!(self.nth(1), T::Punct(P::Lt)) {
                self.bump();
                self.parse_generic_args()?
            } else {
                Vec::new()
            }
        } else if self.at(P::Lt) {
            self.parse_generic_args()?
        } else {
            Vec::new()
        };
        Ok(PathSegment { ident, generics })
    }

    pub(super) fn parse_generic_args(&mut self) -> PResult<Vec<Type>> {
        let open = self.bump().span;
        let mut args = Vec::new();
        loop {
            if self.eat_gt() {
                return Ok(args);
            }
            args.push(self.parse_type()?);
            if !self.eat(P::Comma) {
                if self.eat_gt() {
                    return Ok(args);
                }
                if *self.kind() == T::Eof {
                    return Err(self.report(Diagnostic::new(
                        "unclosed `<`",
                        open,
                        "this `<` has no matching `>`",
                    )));
                }
                return Err(self.expected("`,` or `>`"));
            }
        }
    }

    // ---- Patterns --------------------------------------------------------

    pub(super) fn parse_pattern(&mut self) -> PResult<Pattern> {
        self.nested(Parser::parse_pattern_inner)
    }

    /// A pattern that may have several alternatives, `a | b`, as in a
    /// `match` arm or a `let`. A closure's parameters cannot, since `|`
    /// ends them.
    pub(super) fn parse_pattern_top(&mut self) -> PResult<Pattern> {
        let first = self.parse_pattern()?;
        if !self.at(P::Or) {
            return Ok(first);
        }
        let mut alternatives = vec![first];
        while self.eat(P::Or) {
            alternatives.push(self.parse_pattern()?);
        }
        let span = alternatives[0]
            .span
            .to(alternatives[alternatives.len() - 1].span);
        Ok(Pattern {
            kind: PatternKind::Or(alternatives),
            span,
        })
    }

    /// `{ name, name: pattern, .. }` after a struct pattern's path.
    fn parse_struct_pattern(&mut self, path: Path) -> PResult<PatternKind> {
        let open = self.bump().span;
        let mut fields = Vec::new();
        let mut rest = false;
        while !self.at(P::RBrace) {
            if self.eat(P::DotDot) {
                rest = true;
                break;
            }
            let start = self.span();
            let mutable = self.eat_kw(Kw::Mut);
            let name = self.parse_ident("a field name")?;
            let pattern = if !mutable && self.eat(P::Colon) {
                Some(self.parse_pattern_top()?)
            } else if mutable {
                Some(Pattern {
                    span: start.to(name.span),
                    kind: PatternKind::Binding {
                        mutable: true,
                        name: name.clone(),
                    },
                })
            } else {
                None
            };
            fields.push(FieldPattern {
                span: start.to(self.prev_span()),
                name,
                pattern,
            });
            if !self.eat(P::Comma) {
                break;
            }
        }
        self.expect_close(P::RBrace, open, "the struct pattern")?;
        Ok(PatternKind::Struct { path, fields, rest })
    }

    pub(super) fn parse_pattern_inner(&mut self) -> PResult<Pattern> {
        let start = self.span();
        let kind = match self.kind().clone() {
            T::Punct(P::Underscore) => {
                self.bump();
                PatternKind::Wild
            }
            T::Keyword(Kw::Mut) => {
                self.bump();
                PatternKind::Binding {
                    mutable: true,
                    name: self.parse_ident("a name")?,
                }
            }
            T::Punct(P::LParen) => {
                self.bump();
                let mut elems = Vec::new();
                while !self.at(P::RParen) {
                    elems.push(self.parse_pattern_top()?);
                    if !self.eat(P::Comma) {
                        break;
                    }
                }
                self.expect_close(P::RParen, start, "the tuple pattern")?;
                PatternKind::Tuple(elems)
            }
            T::Int(_) | T::Float(_) | T::Str(_) | T::Char(_) | T::Keyword(Kw::True | Kw::False) => {
                PatternKind::Lit(self.parse_lit())
            }
            T::Punct(P::Minus) if matches!(self.nth(1), T::Int(_) | T::Float(_)) => {
                self.bump();
                let mut lit = self.parse_lit();
                if let LitKind::Int(text) | LitKind::Float(text) = &mut lit.kind {
                    text.insert(0, '-');
                }
                lit.span = start.to(lit.span);
                PatternKind::Lit(lit)
            }
            _ if self.at_path_start() => {
                let path = self.parse_path(true)?;
                if self.at(P::LParen) {
                    let open = self.bump().span;
                    let mut elems = Vec::new();
                    while !self.at(P::RParen) {
                        elems.push(self.parse_pattern_top()?);
                        if !self.eat(P::Comma) {
                            break;
                        }
                    }
                    self.expect_close(P::RParen, open, "the pattern")?;
                    PatternKind::TupleStruct { path, elems }
                } else if self.at(P::LBrace) {
                    self.parse_struct_pattern(path)?
                } else if let [segment] = path.segments.as_slice()
                    && segment.generics.is_empty()
                {
                    // A lone name binds, as in Rust's parser. Whether it is
                    // really a unit variant such as `None` is resolved later.
                    PatternKind::Binding {
                        mutable: false,
                        name: segment.ident.clone(),
                    }
                } else {
                    PatternKind::Path(path)
                }
            }
            T::Keyword(kw) => return Err(self.keyword_as_name(kw, "a pattern")),
            _ => return Err(self.expected("a pattern")),
        };
        Ok(Pattern {
            kind,
            span: start.to(self.prev_span()),
        })
    }

    // ---- Blocks and statements -------------------------------------------

    pub(super) fn parse_block(&mut self) -> PResult<Block> {
        self.nested(Parser::parse_block_inner)
    }

    pub(super) fn parse_block_inner(&mut self) -> PResult<Block> {
        let open = self.expect(P::LBrace, "the block")?;
        let mut stmts = Vec::new();
        while !self.at(P::RBrace) && *self.kind() != T::Eof {
            if self.eat(P::Semi) {
                continue;
            }
            let start = self.pos;
            match self.parse_stmt() {
                Ok(stmt) => stmts.push(stmt),
                Err(Reported) => self.recover(start, List::Stmts),
            }
        }
        let close = self.expect_close(P::RBrace, open, "the block")?;
        Ok(Block {
            stmts,
            span: open.to(close),
        })
    }

    pub(super) fn parse_stmt(&mut self) -> PResult<Stmt> {
        let defensive = self.parse_defensive("a statement")?;
        let mut stmt = self.parse_bare_stmt()?;
        match &mut stmt {
            Stmt::Let { defensive: d, .. } | Stmt::Expr { defensive: d, .. } => *d = defensive,
        }
        Ok(stmt)
    }

    fn parse_bare_stmt(&mut self) -> PResult<Stmt> {
        match self.kind() {
            T::Keyword(Kw::Let) => return self.parse_let(),
            T::Doc(_) => {
                let span = self.span();
                return Err(self.report(
                    Diagnostic::new(
                        "a doc comment cannot go inside a function",
                        span,
                        "doc comments describe items, fields and functions",
                    )
                    .with_help("use `//` for a comment here"),
                ));
            }
            T::Keyword(
                kw @ (Kw::Fn | Kw::Struct | Kw::Enum | Kw::Use | Kw::Impl | Kw::Mod | Kw::Const),
            ) => {
                let what = format!("`{}` inside a function", kw.as_str());
                let span = self.span();
                return Err(self.unsupported(&what, span));
            }
            T::Keyword(kw @ (Kw::Static | Kw::Trait | Kw::Unsafe)) => {
                let kw = *kw;
                return Err(self.left_out(kw));
            }
            T::Ident(name) if name == "macro_rules" && matches!(self.nth(1), T::Punct(P::Not)) => {
                return Err(self.macro_rules());
            }
            _ => {}
        }
        // An `if` or a block ends its statement without `;`, as in Rust.
        let block_like = matches!(
            self.kind(),
            T::Keyword(Kw::If | Kw::Match | Kw::For | Kw::While) | T::Punct(P::LBrace)
        );
        let expr = if block_like {
            self.nested(|p| p.parse_primary(false))?
        } else {
            self.parse_expr()?
        };
        let semi = if self.eat(P::Semi) {
            true
        } else if block_like || self.at(P::RBrace) {
            false
        } else {
            self.expect(P::Semi, "the statement")?;
            true
        };
        Ok(Stmt::Expr {
            expr,
            semi,
            defensive: None,
        })
    }

    pub(super) fn parse_let(&mut self) -> PResult<Stmt> {
        let start = self.bump().span;
        let pattern = self.parse_pattern_top()?;
        let ty = if self.eat(P::Colon) {
            Some(self.parse_type()?)
        } else {
            None
        };
        let mut init = None;
        let mut else_block = None;
        if self.eat(P::Eq) {
            init = Some(self.parse_expr()?);
            if self.eat_kw(Kw::Else) {
                else_block = Some(self.parse_block()?);
            }
        }
        let end = self.expect(P::Semi, "the `let` statement")?;
        Ok(Stmt::Let {
            pattern,
            ty,
            init,
            else_block,
            defensive: None,
            span: start.to(end),
        })
    }

    // ---- Expressions -----------------------------------------------------

    pub(super) fn parse_expr(&mut self) -> PResult<Expr> {
        self.parse_expr_bp(0, false)
    }

    /// An expression where a struct literal is not allowed, such as an `if`
    /// condition, where `x {` starts the block.
    pub(super) fn parse_expr_no_struct(&mut self) -> PResult<Expr> {
        self.parse_expr_bp(0, true)
    }

    /// Precedence climbing over Rust's operators. `min_bp` is the weakest
    /// binding power the caller accepts.
    pub(super) fn parse_expr_bp(&mut self, min_bp: u8, no_struct: bool) -> PResult<Expr> {
        self.nested(|p| p.parse_expr_bp_inner(min_bp, no_struct))
    }

    pub(super) fn parse_expr_bp_inner(&mut self, min_bp: u8, no_struct: bool) -> PResult<Expr> {
        let mut lhs = self.parse_prefix(no_struct)?;
        let mut compared: Option<Span> = None;
        loop {
            if self.at_kw(Kw::As) {
                if CAST_BP < min_bp {
                    break;
                }
                self.bump();
                let ty = self.parse_type()?;
                lhs = Expr {
                    span: lhs.span.to(ty.span),
                    kind: ExprKind::Cast {
                        expr: Box::new(lhs),
                        ty,
                    },
                };
                continue;
            }
            if self.at(P::DotDot) || self.at(P::DotDotEq) {
                if RANGE_BP < min_bp {
                    break;
                }
                let inclusive = self.at(P::DotDotEq);
                self.bump();
                let end = if self.at_range_end(no_struct) {
                    None
                } else {
                    Some(Box::new(self.parse_expr_bp(RANGE_BP + 1, no_struct)?))
                };
                lhs = Expr {
                    span: lhs.span.to(self.prev_span()),
                    kind: ExprKind::Range {
                        start: Some(Box::new(lhs)),
                        end,
                        inclusive,
                    },
                };
                continue;
            }
            let Some((op, left_bp, right_bp)) = infix(self.kind()) else {
                break;
            };
            if left_bp < min_bp {
                break;
            }
            let op_span = self.bump().span;
            if let Infix::Binary(op) = op
                && is_comparison(op)
            {
                if let Some(first) = compared {
                    return Err(self.report(
                        Diagnostic::new(
                            "comparison operators cannot be chained",
                            first.to(op_span),
                            "the second comparison would compare the first one's `bool`",
                        )
                        .with_help("join two comparisons with `&&`, such as `a < b && b < c`"),
                    ));
                }
                compared = Some(op_span);
            }
            let rhs = self.parse_expr_bp(right_bp, no_struct)?;
            let span = lhs.span.to(rhs.span);
            let kind = match op {
                Infix::Binary(op) => ExprKind::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                Infix::Assign(op) => ExprKind::Assign {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
            };
            lhs = Expr { kind, span };
        }
        Ok(lhs)
    }

    pub(super) fn parse_prefix(&mut self, no_struct: bool) -> PResult<Expr> {
        let start = self.span();
        self.split_and_and();
        let unary = match self.kind() {
            T::Punct(P::Minus) => Some(UnaryOp::Neg),
            T::Punct(P::Not) => Some(UnaryOp::Not),
            T::Punct(P::Star) => Some(UnaryOp::Deref),
            _ => None,
        };
        if let Some(op) = unary {
            self.bump();
            let operand = self.parse_expr_bp(UNARY_BP, no_struct)?;
            return Ok(Expr {
                span: start.to(operand.span),
                kind: ExprKind::Unary {
                    op,
                    operand: Box::new(operand),
                },
            });
        }
        match self.kind() {
            T::Punct(P::And) => {
                self.bump();
                let mutable = self.eat_kw(Kw::Mut);
                let operand = self.parse_expr_bp(UNARY_BP, no_struct)?;
                Ok(Expr {
                    span: start.to(operand.span),
                    kind: ExprKind::Ref {
                        mutable,
                        operand: Box::new(operand),
                    },
                })
            }
            T::Keyword(Kw::Send) => {
                self.bump();
                let message = self.parse_expr_bp(UNARY_BP, no_struct)?;
                Ok(Expr {
                    span: start.to(message.span),
                    kind: ExprKind::Send(Box::new(message)),
                })
            }
            T::Punct(P::DotDot | P::DotDotEq) => {
                let inclusive = self.at(P::DotDotEq);
                self.bump();
                let end = if self.at_range_end(no_struct) {
                    None
                } else {
                    Some(Box::new(self.parse_expr_bp(RANGE_BP + 1, no_struct)?))
                };
                Ok(Expr {
                    span: start.to(self.prev_span()),
                    kind: ExprKind::Range {
                        start: None,
                        end,
                        inclusive,
                    },
                })
            }
            T::Keyword(Kw::Return) => {
                self.bump();
                let value = if matches!(
                    self.kind(),
                    T::Punct(P::Semi | P::RBrace | P::RParen | P::Comma) | T::Eof
                ) {
                    None
                } else {
                    Some(Box::new(self.parse_expr_bp(0, no_struct)?))
                };
                Ok(Expr {
                    span: start.to(self.prev_span()),
                    kind: ExprKind::Return(value),
                })
            }
            _ => {
                let primary = self.parse_primary(no_struct)?;
                self.parse_postfix(primary)
            }
        }
    }

    pub(super) fn parse_postfix(&mut self, mut expr: Expr) -> PResult<Expr> {
        loop {
            let start = expr.span;
            let kind = match self.kind() {
                T::Punct(P::Question) => {
                    self.bump();
                    ExprKind::Try(Box::new(expr))
                }
                T::Punct(P::LParen) => {
                    let open = self.bump().span;
                    let args = self.parse_args(P::RParen, open)?;
                    ExprKind::Call {
                        callee: Box::new(expr),
                        args,
                    }
                }
                T::Punct(P::LBracket) => {
                    let open = self.bump().span;
                    let index = self.parse_expr()?;
                    self.expect_close(P::RBracket, open, "the index")?;
                    ExprKind::Index {
                        base: Box::new(expr),
                        index: Box::new(index),
                    }
                }
                T::Punct(P::Dot) => {
                    self.bump();
                    match self.kind().clone() {
                        T::Keyword(Kw::Await) => {
                            self.bump();
                            ExprKind::Await(Box::new(expr))
                        }
                        T::Int(text) if text.chars().all(|c| c.is_ascii_digit()) => {
                            let name = Ident::new(text, self.bump().span);
                            ExprKind::Field {
                                base: Box::new(expr),
                                name,
                            }
                        }
                        T::Ident(_) => {
                            let method = self.parse_path_segment(true)?;
                            if self.at(P::LParen) {
                                let open = self.bump().span;
                                let args = self.parse_args(P::RParen, open)?;
                                ExprKind::MethodCall {
                                    receiver: Box::new(expr),
                                    method,
                                    args,
                                }
                            } else if !method.generics.is_empty() {
                                return Err(self.expected("`(` and the method's arguments"));
                            } else {
                                ExprKind::Field {
                                    base: Box::new(expr),
                                    name: method.ident,
                                }
                            }
                        }
                        _ => return Err(self.expected("a field, a method or `await`")),
                    }
                }
                _ => return Ok(expr),
            };
            expr = Expr {
                kind,
                span: start.to(self.prev_span()),
            };
        }
    }

    /// Comma-separated expressions up to and including `close`.
    pub(super) fn parse_args(&mut self, close: P, open: Span) -> PResult<Vec<Expr>> {
        let mut args = Vec::new();
        while !self.at(close) {
            args.push(self.parse_expr()?);
            if !self.eat(P::Comma) {
                break;
            }
        }
        self.expect_close(close, open, "the arguments")?;
        Ok(args)
    }

    pub(super) fn parse_lit(&mut self) -> Lit {
        let token = self.bump();
        let kind = match token.kind {
            T::Int(text) => LitKind::Int(text),
            T::Float(text) => LitKind::Float(text),
            T::Str(value) => LitKind::Str(value),
            T::Char(c) => LitKind::Char(c),
            T::Keyword(Kw::True) => LitKind::Bool(true),
            _ => LitKind::Bool(false),
        };
        Lit {
            kind,
            span: token.span,
        }
    }

    pub(super) fn parse_primary(&mut self, no_struct: bool) -> PResult<Expr> {
        let start = self.span();
        let kind = match self.kind().clone() {
            T::Int(_) | T::Float(_) | T::Str(_) | T::Char(_) | T::Keyword(Kw::True | Kw::False) => {
                ExprKind::Lit(self.parse_lit())
            }
            T::Punct(P::LParen) => {
                self.bump();
                let mut elems = Vec::new();
                let mut trailing_comma = false;
                while !self.at(P::RParen) {
                    elems.push(self.parse_expr()?);
                    trailing_comma = self.eat(P::Comma);
                    if !trailing_comma {
                        break;
                    }
                }
                self.expect_close(P::RParen, start, "the parenthesised expression")?;
                if elems.len() == 1 && !trailing_comma {
                    ExprKind::Paren(Box::new(elems.remove(0)))
                } else {
                    ExprKind::Tuple(elems)
                }
            }
            T::Punct(P::LBracket) => {
                self.bump();
                let mut elems = Vec::new();
                while !self.at(P::RBracket) {
                    elems.push(self.parse_expr()?);
                    if self.at(P::Semi) {
                        let span = self.span();
                        return Err(self.unsupported("`[value; count]`", span));
                    }
                    if !self.eat(P::Comma) {
                        break;
                    }
                }
                self.expect_close(P::RBracket, start, "the array")?;
                ExprKind::Array(elems)
            }
            T::Punct(P::LBrace) => ExprKind::Block(self.parse_block()?),
            T::Keyword(Kw::If) => return self.parse_if(),
            T::Keyword(Kw::Match) => return self.parse_match(),
            T::Keyword(Kw::For) => return self.parse_for(),
            T::Keyword(Kw::While) => return self.parse_while(),
            T::Keyword(Kw::Fork) => return self.parse_fork(),
            T::Keyword(Kw::Let) => {
                return Err(self.report(
                    Diagnostic::new(
                        "`let` here is not a statement",
                        start,
                        "`let` inside an expression is only allowed as the condition of an `if` or `while`",
                    )
                    .with_help("move the `let` to a statement of its own"),
                ));
            }
            T::Keyword(kw @ (Kw::Loop | Kw::Break | Kw::Continue | Kw::Async)) => {
                let what = format!("`{}`", kw.as_str());
                return Err(self.unsupported(&what, start));
            }
            T::Punct(P::Or | P::OrOr) => return self.parse_closure(start, false, no_struct),
            T::Keyword(Kw::Move) => {
                self.bump();
                if self.at(P::Or) || self.at(P::OrOr) {
                    return self.parse_closure(start, true, no_struct);
                }
                ExprKind::Move(self.parse_place()?)
            }
            T::Keyword(Kw::Unsafe) => return Err(self.left_out(Kw::Unsafe)),
            T::Keyword(kw) if kw.is_inset_only() => {
                return Err(self.keyword_as_name(kw, "an expression"));
            }
            _ if self.at_path_start() => {
                let path = self.parse_path(true)?;
                if self.at(P::Not) {
                    return self.parse_macro(path);
                }
                if self.at(P::LBrace) && !no_struct {
                    return self.parse_struct_lit(path);
                }
                ExprKind::Path(path)
            }
            _ => return Err(self.expected("an expression")),
        };
        Ok(Expr {
            kind,
            span: start.to(self.prev_span()),
        })
    }

    /// `|params| body`, from its first `|`, or `||` when it has none.
    /// `start` is where it began, at `move` if it has one.
    pub(super) fn parse_closure(
        &mut self,
        start: Span,
        is_move: bool,
        no_struct: bool,
    ) -> PResult<Expr> {
        let mut params = Vec::new();
        if !self.eat(P::OrOr) {
            self.bump();
            while !self.at(P::Or) {
                let pattern = self.parse_pattern()?;
                let ty = if self.eat(P::Colon) {
                    Some(self.parse_type()?)
                } else {
                    None
                };
                params.push(ClosureParam {
                    span: pattern
                        .span
                        .to(ty.as_ref().map_or(pattern.span, |t| t.span)),
                    pattern,
                    ty,
                });
                if !self.eat(P::Comma) {
                    break;
                }
            }
            self.expect(P::Or, "the closure's parameters")?;
        }
        let ret = if self.eat(P::Arrow) {
            Some(self.parse_type()?)
        } else {
            None
        };
        // As in Rust, a closure with a return type has a block for its body.
        let body = if ret.is_some() {
            if !self.at(P::LBrace) {
                return Err(self.expected("`{` to start the closure's body"));
            }
            let block = self.parse_block()?;
            Expr {
                span: block.span,
                kind: ExprKind::Block(block),
            }
        } else {
            self.parse_expr_bp(0, no_struct)?
        };
        Ok(Expr {
            span: start.to(body.span),
            kind: ExprKind::Closure(Closure {
                is_move,
                params,
                ret,
                body: Box::new(body),
            }),
        })
    }

    pub(super) fn parse_macro(&mut self, name: Path) -> PResult<Expr> {
        self.bump();
        let (close, what) = match self.kind() {
            T::Punct(P::LParen) => (P::RParen, "the macro's arguments"),
            T::Punct(P::LBracket) => (P::RBracket, "the macro's arguments"),
            _ => return Err(self.expected("`(` or `[` after the macro's name")),
        };
        let open = self.bump().span;
        let mut args = Vec::new();
        while !self.at(close) {
            args.push(self.parse_expr()?);
            if self.at(P::Semi) {
                let span = self.span();
                return Err(self.unsupported("`;` in a macro's arguments", span));
            }
            if !self.eat(P::Comma) {
                break;
            }
        }
        let end = self.expect_close(close, open, what)?;
        Ok(Expr {
            span: name.span.to(end),
            kind: ExprKind::Macro { name, args },
        })
    }

    pub(super) fn parse_struct_lit(&mut self, path: Path) -> PResult<Expr> {
        let open = self.bump().span;
        let mut fields = Vec::new();
        let mut rest = None;
        while !self.at(P::RBrace) {
            if self.eat(P::DotDot) {
                rest = Some(Box::new(self.parse_expr()?));
                break;
            }
            fields.push(self.parse_field_init()?);
            if !self.eat(P::Comma) {
                break;
            }
        }
        let end = self.expect_close(P::RBrace, open, "the struct's fields")?;
        Ok(Expr {
            span: path.span.to(end),
            kind: ExprKind::Struct { path, fields, rest },
        })
    }

    /// The condition of an `if` or `while`: an expression, or `let pattern = value`.
    pub(super) fn parse_cond(&mut self) -> PResult<Expr> {
        if !self.at_kw(Kw::Let) {
            return self.parse_expr_no_struct();
        }
        let start = self.bump().span;
        let pattern = self.parse_pattern_top()?;
        self.expect(P::Eq, "the pattern")?;
        // `&&` binds looser than the value, so a chain of conditions is seen.
        let expr = self.parse_expr_bp(6, true)?;
        if self.at(P::AndAnd) {
            let span = self.span();
            return Err(self.unsupported("a `let` chain", span));
        }
        Ok(Expr {
            span: start.to(expr.span),
            kind: ExprKind::Let {
                pattern,
                expr: Box::new(expr),
            },
        })
    }

    /// Whether a range ends here, with no end value: `a..` or `..`.
    pub(super) fn at_range_end(&self, no_struct: bool) -> bool {
        matches!(
            self.kind(),
            T::Punct(P::RParen | P::RBracket | P::RBrace | P::Comma | P::Semi | P::FatArrow)
                | T::Eof
        ) || (no_struct && self.at(P::LBrace))
    }

    pub(super) fn parse_match(&mut self) -> PResult<Expr> {
        let start = self.bump().span;
        let scrutinee = self.parse_expr_no_struct()?;
        if !self.at(P::LBrace) {
            return Err(self.expected("`{` to start the `match` arms"));
        }
        let open = self.bump().span;
        let mut arms = Vec::new();
        while !self.at(P::RBrace) && *self.kind() != T::Eof {
            let defensive = self.parse_defensive("a `match` arm")?;
            let pattern = self.parse_pattern_top()?;
            let guard = if self.eat_kw(Kw::If) {
                Some(self.parse_expr()?)
            } else {
                None
            };
            self.expect(P::FatArrow, "the arm's pattern")?;
            let body = if self.at(P::LBrace) {
                let block = self.parse_block()?;
                Expr {
                    span: block.span,
                    kind: ExprKind::Block(block),
                }
            } else {
                self.parse_expr()?
            };
            let block_like = matches!(
                body.kind,
                ExprKind::Block(_)
                    | ExprKind::If { .. }
                    | ExprKind::Match { .. }
                    | ExprKind::For { .. }
                    | ExprKind::While { .. }
            );
            arms.push(MatchArm {
                span: defensive
                    .as_ref()
                    .map_or(pattern.span, |d| d.span)
                    .to(body.span),
                pattern,
                guard,
                body,
                defensive,
            });
            if !self.eat(P::Comma) && !block_like && !self.at(P::RBrace) {
                self.expect(P::Comma, "the arm")?;
            }
        }
        let end = self.expect_close(P::RBrace, open, "the `match` arms")?;
        Ok(Expr {
            span: start.to(end),
            kind: ExprKind::Match {
                scrutinee: Box::new(scrutinee),
                arms,
            },
        })
    }

    pub(super) fn parse_for(&mut self) -> PResult<Expr> {
        let start = self.bump().span;
        let pattern = self.parse_pattern_top()?;
        if !self.eat_kw(Kw::In) {
            return Err(self.expected("`in` after the `for` loop's pattern"));
        }
        let iter = self.parse_expr_no_struct()?;
        if !self.at(P::LBrace) {
            return Err(self.expected("`{` to start the loop's body"));
        }
        let body = self.parse_block()?;
        Ok(Expr {
            span: start.to(body.span),
            kind: ExprKind::For {
                pattern,
                iter: Box::new(iter),
                body,
            },
        })
    }

    /// `fork for pattern in iter { body }`. `fork { a, b }` is Draft.
    fn parse_fork(&mut self) -> PResult<Expr> {
        let start = self.bump().span;
        if !self.at_kw(Kw::For) {
            if self.at(P::LBrace) {
                let span = start.to(self.span());
                return Err(self.unsupported("`fork { ... }`", span));
            }
            return Err(self.expected("`for` after `fork`"));
        }
        let Expr { kind, span } = self.parse_for()?;
        let ExprKind::For {
            pattern,
            iter,
            body,
        } = kind
        else {
            unreachable!("`parse_for` reads a `for` loop");
        };
        Ok(Expr {
            span: start.to(span),
            kind: ExprKind::Fork {
                pattern,
                iter,
                body,
            },
        })
    }

    pub(super) fn parse_while(&mut self) -> PResult<Expr> {
        let start = self.bump().span;
        let cond = self.parse_cond()?;
        if !self.at(P::LBrace) {
            return Err(self.expected("`{` to start the loop's body"));
        }
        let body = self.parse_block()?;
        Ok(Expr {
            span: start.to(body.span),
            kind: ExprKind::While {
                cond: Box::new(cond),
                body,
            },
        })
    }

    pub(super) fn parse_if(&mut self) -> PResult<Expr> {
        let start = self.bump().span;
        let cond = self.parse_cond()?;
        if !self.at(P::LBrace) {
            return Err(self.expected("`{` to start the `if` block"));
        }
        let then_block = self.parse_block()?;
        let else_branch = if self.eat_kw(Kw::Else) {
            let branch = if self.at_kw(Kw::If) {
                self.nested(Parser::parse_if)?
            } else if self.at(P::LBrace) {
                let block = self.parse_block()?;
                Expr {
                    span: block.span,
                    kind: ExprKind::Block(block),
                }
            } else {
                return Err(self.expected("`{` or `if` after `else`"));
            };
            Some(Box::new(branch))
        } else {
            None
        };
        Ok(Expr {
            span: start.to(self.prev_span()),
            kind: ExprKind::If {
                cond: Box::new(cond),
                then_block,
                else_branch,
            },
        })
    }
}

enum Infix {
    Binary(BinaryOp),
    Assign(Option<BinaryOp>),
}

/// Below `||`, above assignment, as in Rust.
const RANGE_BP: u8 = 2;
const CAST_BP: u8 = 21;
const UNARY_BP: u8 = 23;

/// Rust's binary operators with their left and right binding powers. A
/// higher power binds tighter; right below left makes an operator
/// right-associative.
fn infix(kind: &T) -> Option<(Infix, u8, u8)> {
    use BinaryOp as B;
    let T::Punct(p) = kind else {
        return None;
    };
    let binary = |op, bp: u8| Some((Infix::Binary(op), bp, bp + 1));
    let assign = |op| Some((Infix::Assign(op), 2, 1));
    match p {
        P::Eq => assign(None),
        P::PlusEq => assign(Some(B::Add)),
        P::MinusEq => assign(Some(B::Sub)),
        P::StarEq => assign(Some(B::Mul)),
        P::SlashEq => assign(Some(B::Div)),
        P::PercentEq => assign(Some(B::Rem)),
        P::CaretEq => assign(Some(B::BitXor)),
        P::AndEq => assign(Some(B::BitAnd)),
        P::OrEq => assign(Some(B::BitOr)),
        P::ShlEq => assign(Some(B::Shl)),
        P::ShrEq => assign(Some(B::Shr)),
        P::OrOr => binary(B::Or, 3),
        P::AndAnd => binary(B::And, 5),
        P::EqEq => binary(B::Eq, 7),
        P::Ne => binary(B::Ne, 7),
        P::Lt => binary(B::Lt, 7),
        P::Le => binary(B::Le, 7),
        P::Gt => binary(B::Gt, 7),
        P::Ge => binary(B::Ge, 7),
        P::Or => binary(B::BitOr, 9),
        P::Caret => binary(B::BitXor, 11),
        P::And => binary(B::BitAnd, 13),
        P::Shl => binary(B::Shl, 15),
        P::Shr => binary(B::Shr, 15),
        P::Plus => binary(B::Add, 17),
        P::Minus => binary(B::Sub, 17),
        P::Star => binary(B::Mul, 19),
        P::Slash => binary(B::Div, 19),
        P::Percent => binary(B::Rem, 19),
        _ => None,
    }
}

fn is_comparison(op: BinaryOp) -> bool {
    use BinaryOp as B;
    matches!(op, B::Eq | B::Ne | B::Lt | B::Le | B::Gt | B::Ge)
}
