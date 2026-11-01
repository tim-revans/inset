//! Where `#[defensive("reason")]` may go, as `syntax.md` says: on a `match`
//! arm or a `failure` arm, as the first line of the block of an `if` or
//! `else` branch or of a `let ... else`, marking that branch, or on a
//! statement with a `?`, marking its error path. Each place is a branch
//! coverage counts, so a mark anywhere else would excuse nothing, and it is
//! refused rather than ignored. A closure's code is not counted, so a mark
//! inside one is refused too. The parser keeps the attribute off items other
//! than functions and components, and `items` refuses it on those.

use inset_syntax::Diagnostic;
use inset_syntax::ast::{Block, Defensive, Expr, ExprKind, Stmt};

/// Checks every `#[defensive]` in a function's body.
pub(crate) fn check(body: &Block, diagnostics: &mut Vec<Diagnostic>) {
    Walk {
        diagnostics,
        in_closure: false,
    }
    .block(body, false);
}

struct Walk<'d> {
    diagnostics: &'d mut Vec<Diagnostic>,
    in_closure: bool,
}

impl Walk<'_> {
    /// A block, which is a branch of its own when it is an `if` or `else`
    /// block or a `let ... else` block.
    fn block(&mut self, block: &Block, branch: bool) {
        for (i, stmt) in block.stmts.iter().enumerate() {
            if let Some(defensive) = stmt.defensive() {
                let marks_branch = branch && i == 0;
                if self.in_closure {
                    self.in_closure_error(defensive);
                } else if !marks_branch && !tries(stmt) {
                    self.diagnostics.push(
                        Diagnostic::new(
                            "`#[defensive]` marks no branch here",
                            defensive.span,
                            "this statement is not a branch, and has no `?`",
                        )
                        .with_help(
                            "put it on the first line of the `if`, `else` or `let ... else` \
                             block that no requirement can reach, on a `match` arm, or on a \
                             statement whose `?` no requirement can make fail",
                        ),
                    );
                }
            }
            match stmt {
                Stmt::Let {
                    init, else_block, ..
                } => {
                    if let Some(init) = init {
                        self.expr(init);
                    }
                    if let Some(otherwise) = else_block {
                        self.block(otherwise, true);
                    }
                }
                Stmt::Expr { expr, .. } => self.expr(expr),
            }
        }
    }

    fn in_closure_error(&mut self, defensive: &Defensive) {
        self.diagnostics.push(
            Diagnostic::new(
                "`#[defensive]` cannot go inside a closure",
                defensive.span,
                "coverage does not count the branches of a closure",
            )
            .with_help("mark the step that calls the closure instead, if it has a `?`"),
        );
    }

    fn expr(&mut self, expr: &Expr) {
        let each = |walk: &mut Self, exprs: &mut dyn Iterator<Item = &Expr>| {
            for e in exprs {
                walk.expr(e);
            }
        };
        match &expr.kind {
            ExprKind::Lit(_) | ExprKind::Path(_) | ExprKind::Move(_) => {}
            ExprKind::Closure(closure) => {
                let outer = std::mem::replace(&mut self.in_closure, true);
                self.expr(&closure.body);
                self.in_closure = outer;
            }
            ExprKind::Call { callee, args } => {
                self.expr(callee);
                each(self, &mut args.iter());
            }
            ExprKind::MethodCall { receiver, args, .. } => {
                self.expr(receiver);
                each(self, &mut args.iter());
            }
            ExprKind::Macro { args, .. } | ExprKind::Tuple(args) | ExprKind::Array(args) => {
                each(self, &mut args.iter());
            }
            ExprKind::Field { base, .. }
            | ExprKind::Unary { operand: base, .. }
            | ExprKind::Ref { operand: base, .. }
            | ExprKind::Cast { expr: base, .. }
            | ExprKind::Paren(base)
            | ExprKind::Await(base)
            | ExprKind::Try(base)
            | ExprKind::Send(base)
            | ExprKind::Let { expr: base, .. } => self.expr(base),
            ExprKind::Index { base, index: other }
            | ExprKind::Binary {
                lhs: base,
                rhs: other,
                ..
            }
            | ExprKind::Assign {
                lhs: base,
                rhs: other,
                ..
            } => {
                self.expr(base);
                self.expr(other);
            }
            ExprKind::Struct { fields, rest, .. } => {
                each(self, &mut fields.iter().filter_map(|f| f.value.as_ref()));
                each(self, &mut rest.iter().map(|r| &**r));
            }
            ExprKind::Range { start, end, .. } => {
                each(self, &mut start.iter().chain(end.iter()).map(|e| &**e));
            }
            ExprKind::Return(value) => each(self, &mut value.iter().map(|v| &**v)),
            ExprKind::Block(block) => self.block(block, false),
            ExprKind::If {
                cond,
                then_block,
                else_branch,
            } => {
                self.expr(cond);
                self.block(then_block, true);
                match else_branch.as_deref() {
                    Some(Expr {
                        kind: ExprKind::Block(block),
                        ..
                    }) => self.block(block, true),
                    Some(other) => self.expr(other),
                    None => {}
                }
            }
            ExprKind::Match { scrutinee, arms } => {
                self.expr(scrutinee);
                for arm in arms {
                    if let Some(defensive) = &arm.defensive
                        && self.in_closure
                    {
                        self.in_closure_error(defensive);
                    }
                    each(self, &mut arm.guard.iter());
                    self.expr(&arm.body);
                }
            }
            ExprKind::For { iter, body, .. } | ExprKind::Fork { iter, body, .. } => {
                self.expr(iter);
                self.block(body, false);
            }
            ExprKind::While { cond, body } => {
                self.expr(cond);
                self.block(body, false);
            }
        }
    }
}

/// Whether a statement has a `?` outside any closure: an error path that
/// a mark on the statement excuses.
fn tries(stmt: &Stmt) -> bool {
    fn expr(e: &Expr) -> bool {
        let mut found = false;
        visit(e, &mut found);
        found
    }
    fn visit(e: &Expr, found: &mut bool) {
        match &e.kind {
            ExprKind::Try(_) => *found = true,
            ExprKind::Closure(_) | ExprKind::Lit(_) | ExprKind::Path(_) | ExprKind::Move(_) => {}
            ExprKind::Call { callee, args } => {
                visit(callee, found);
                args.iter().for_each(|a| visit(a, found));
            }
            ExprKind::MethodCall { receiver, args, .. } => {
                visit(receiver, found);
                args.iter().for_each(|a| visit(a, found));
            }
            ExprKind::Macro { args, .. } | ExprKind::Tuple(args) | ExprKind::Array(args) => {
                args.iter().for_each(|a| visit(a, found));
            }
            ExprKind::Field { base, .. }
            | ExprKind::Unary { operand: base, .. }
            | ExprKind::Ref { operand: base, .. }
            | ExprKind::Cast { expr: base, .. }
            | ExprKind::Paren(base)
            | ExprKind::Await(base)
            | ExprKind::Send(base)
            | ExprKind::Let { expr: base, .. } => visit(base, found),
            ExprKind::Index { base, index: other }
            | ExprKind::Binary {
                lhs: base,
                rhs: other,
                ..
            }
            | ExprKind::Assign {
                lhs: base,
                rhs: other,
                ..
            } => {
                visit(base, found);
                visit(other, found);
            }
            ExprKind::Struct { fields, rest, .. } => {
                for value in fields.iter().filter_map(|f| f.value.as_ref()) {
                    visit(value, found);
                }
                if let Some(rest) = rest {
                    visit(rest, found);
                }
            }
            ExprKind::Range { start, end, .. } => {
                for part in [start, end].into_iter().flatten() {
                    visit(part, found);
                }
            }
            ExprKind::Return(value) => {
                if let Some(value) = value {
                    visit(value, found);
                }
            }
            ExprKind::Fork { iter, .. } => visit(iter, found),
            // The `?` in a nested block is its own statement's.
            ExprKind::Block(_)
            | ExprKind::If { .. }
            | ExprKind::Match { .. }
            | ExprKind::For { .. }
            | ExprKind::While { .. } => {}
        }
    }
    match stmt {
        Stmt::Let { init, .. } => init.as_ref().is_some_and(expr),
        Stmt::Expr { expr: e, .. } => expr(e),
    }
}
