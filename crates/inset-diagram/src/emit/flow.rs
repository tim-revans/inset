//! Flowcharts: a function's body as steps, following the table in
//! `docs/language/diagrams.md`.
//!
//! A step is one statement, so everything inside it that is not a branch
//! must be drawn as part of it. A closure is opaque, because it cannot send,
//! await or call a capability. An `if` or `return` inside an expression, or
//! a block, would hide a branch inside a step, so they cannot be drawn yet.

use inset_syntax::Span;
use inset_syntax::ast::{self, Block, Expr, ExprKind, Stmt};

use super::{Emitter, Scope};
use crate::model::{Branch, Exit, Step};

/// What happens to the final value of a block.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Value {
    /// It is the function's value, drawn as a return.
    Returned,
    /// It is thrown away, as in a function that returns nothing.
    Discarded,
    /// It is assigned by a `let`, and drawn only if it does something.
    Assigned,
}

/// The function a flowchart is drawn for.
struct Context<'s> {
    returns_option: bool,
    scope: Option<&'s Scope<'s>>,
}

/// What a step uses, found by walking its expressions.
#[derive(Default)]
struct Uses {
    links: Vec<String>,
    /// How many `?` the step has outside closures.
    tries: usize,
}

impl Emitter<'_> {
    pub(super) fn flowchart(
        &mut self,
        id: &str,
        function: &ast::Function,
        scope: Option<&Scope<'_>>,
    ) -> Vec<Step> {
        let returns_option = function.ret.as_ref().is_some_and(|ty| match &ty.kind {
            ast::TypeKind::Path(p) => p.segments.last().is_some_and(|s| s.ident.name == "Option"),
            _ => false,
        });
        let cx = Context {
            returns_option,
            scope,
        };
        let value = match function.ret {
            Some(_) => Value::Returned,
            None => Value::Discarded,
        };
        self.block(&cx, id, &function.body, value)
    }

    fn block(&mut self, cx: &Context<'_>, prefix: &str, block: &Block, value: Value) -> Vec<Step> {
        let mut steps = Vec::new();
        let count = block.stmts.len();
        for (i, stmt) in block.stmts.iter().enumerate() {
            let id = format!("{prefix}/{}", steps.len());
            let span = match stmt {
                Stmt::Let { span, .. } => *span,
                Stmt::Expr { expr, .. } => expr.span,
            };
            let step = match stmt {
                Stmt::Let {
                    pattern,
                    init,
                    else_block: Some(otherwise),
                    span,
                    ..
                } => {
                    let uses = self.uses(cx, init.as_ref());
                    self.no_exit(&uses, *span, "in a `let ... else`");
                    self.locate(&format!("{id}/b0"), pattern.span);
                    self.locate(&format!("{id}/b1"), otherwise.span);
                    let branches = vec![
                        Branch {
                            id: format!("{id}/b0"),
                            label: self.text(pattern.span),
                            steps: Vec::new(),
                            defensive: None,
                        },
                        Branch {
                            id: format!("{id}/b1"),
                            label: "else".into(),
                            steps: self.block(cx, &format!("{id}/b1"), otherwise, Value::Discarded),
                            defensive: None,
                        },
                    ];
                    Some(Step::Decision {
                        text: self.text(Span::new(span.start, otherwise.span.start)),
                        id,
                        links: uses.links,
                        branches,
                    })
                }
                Stmt::Let {
                    init:
                        Some(
                            init @ Expr {
                                kind: ExprKind::If { then_block, .. },
                                ..
                            },
                        ),
                    span,
                    ..
                } => {
                    let head = Span::new(span.start, then_block.span.start);
                    Some(self.decision(cx, id, head, init, Value::Assigned))
                }
                Stmt::Let { init, span, .. } => {
                    let uses = self.uses(cx, init.as_ref());
                    Some(self.action(cx, id, *span, uses))
                }
                Stmt::Expr { expr, semi } => {
                    let last = i + 1 == count && !semi;
                    let value = if last { value } else { Value::Discarded };
                    self.statement(cx, id, expr, last, value)
                }
            };
            if let Some(step) = step {
                // A decision has already located itself at its head.
                self.sources.entry(step.id().to_string()).or_insert(span);
                steps.push(step);
            }
        }
        steps
    }

    fn statement(
        &mut self,
        cx: &Context<'_>,
        id: String,
        expr: &Expr,
        last: bool,
        value: Value,
    ) -> Option<Step> {
        match &expr.kind {
            ExprKind::If { then_block, .. } => {
                let head = Span::new(expr.span.start, then_block.span.start);
                Some(self.decision(cx, id, head, expr, value))
            }
            ExprKind::Return(returned) => {
                let uses = self.uses(cx, returned.as_deref());
                self.no_exit(&uses, expr.span, "in a `return`");
                Some(Step::Return {
                    id,
                    text: returned
                        .as_ref()
                        .map_or_else(|| "()".into(), |r| self.text(r.span)),
                    links: uses.links,
                })
            }
            ExprKind::Block(_) => {
                self.unsupported("a block in a body", expr.span, "this block");
                None
            }
            _ if last && value == Value::Returned => {
                let uses = self.uses(cx, Some(expr));
                self.no_exit(&uses, expr.span, "in the function's value");
                Some(Step::Return {
                    id,
                    text: self.text(expr.span),
                    links: uses.links,
                })
            }
            ExprKind::Path(_) | ExprKind::Lit(_) if last && value == Value::Assigned => None,
            _ => {
                let uses = self.uses(cx, Some(expr));
                Some(self.action(cx, id, expr.span, uses))
            }
        }
    }

    fn action(&mut self, cx: &Context<'_>, id: String, span: Span, uses: Uses) -> Step {
        let exit = (uses.tries > 0).then(|| {
            self.locate(&format!("{id}/err"), span);
            if !cx.returns_option {
                self.unsupported(
                    "`?` in a function that does not return an `Option`",
                    span,
                    "returns early here",
                );
            }
            Exit {
                id: format!("{id}/err"),
                text: "None".into(),
                defensive: None,
            }
        });
        let text = self.text(span);
        Step::Action {
            text: text
                .strip_suffix(';')
                .unwrap_or(&text)
                .trim_end()
                .to_string(),
            id,
            links: uses.links,
            exit,
        }
    }

    /// An `if` whose head, `if condition` or `let name = if condition`, is
    /// `head`. The branches are `yes` and `no`, and an `else if` is a
    /// decision of its own in the `no` branch.
    fn decision(
        &mut self,
        cx: &Context<'_>,
        id: String,
        head: Span,
        expr: &Expr,
        value: Value,
    ) -> Step {
        let ExprKind::If {
            cond,
            then_block,
            else_branch,
        } = &expr.kind
        else {
            unreachable!("a decision is drawn for an `if`");
        };
        let uses = self.uses(cx, Some(cond));
        self.no_exit(&uses, cond.span, "in a condition");
        let yes = format!("{id}/b0");
        let no = format!("{id}/b1");
        self.locate(&id, head);
        self.locate(&yes, then_block.span);
        self.locate(&no, else_branch.as_ref().map_or(head, |e| e.span));
        let no_steps = match else_branch.as_deref() {
            None => Vec::new(),
            Some(Expr {
                kind: ExprKind::Block(block),
                ..
            }) => self.block(cx, &no, block, value),
            Some(nested @ Expr { kind, span }) => {
                let ExprKind::If { then_block, .. } = kind else {
                    unreachable!("`else` is followed by a block or an `if`");
                };
                let head = Span::new(span.start, then_block.span.start);
                vec![self.decision(cx, format!("{no}/0"), head, nested, value)]
            }
        };
        Step::Decision {
            text: self.text(head),
            links: uses.links,
            branches: vec![
                Branch {
                    steps: self.block(cx, &yes, then_block, value),
                    id: yes,
                    label: "yes".into(),
                    defensive: None,
                },
                Branch {
                    id: no,
                    label: "no".into(),
                    steps: no_steps,
                    defensive: None,
                },
            ],
            id,
        }
    }

    fn no_exit(&mut self, uses: &Uses, span: Span, place: &str) {
        if uses.tries > 0 {
            self.unsupported(&format!("`?` {place}"), span, "returns early here");
        }
    }

    fn uses(&mut self, cx: &Context<'_>, expr: Option<&Expr>) -> Uses {
        let mut uses = Uses::default();
        if let Some(expr) = expr {
            self.walk(cx, expr, &mut uses, false);
        }
        uses
    }

    /// Finds the functions and capabilities an expression uses, and what in
    /// it cannot be drawn as part of one step.
    fn walk(&mut self, cx: &Context<'_>, expr: &Expr, uses: &mut Uses, in_closure: bool) {
        let link = |uses: &mut Uses, id: String| {
            if !uses.links.contains(&id) {
                uses.links.push(id);
            }
        };
        match &expr.kind {
            ExprKind::Lit(_) | ExprKind::Path(_) => {}
            ExprKind::Call { callee, args } => {
                if let Some(id) = self.callee(cx, callee) {
                    link(uses, id);
                }
                self.walk(cx, callee, uses, in_closure);
                for arg in args {
                    self.walk(cx, arg, uses, in_closure);
                }
            }
            ExprKind::MethodCall {
                receiver,
                method,
                args,
            } => {
                if let Some(edge) = self.capability_call(cx, receiver, &method.ident.name) {
                    link(uses, edge);
                }
                self.walk(cx, receiver, uses, in_closure);
                for arg in args {
                    self.walk(cx, arg, uses, in_closure);
                }
            }
            ExprKind::Macro { args, .. } | ExprKind::Tuple(args) | ExprKind::Array(args) => {
                for arg in args {
                    self.walk(cx, arg, uses, in_closure);
                }
            }
            ExprKind::Field { base, .. }
            | ExprKind::Unary { operand: base, .. }
            | ExprKind::Ref { operand: base, .. }
            | ExprKind::Cast { expr: base, .. }
            | ExprKind::Paren(base) => self.walk(cx, base, uses, in_closure),
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
                self.walk(cx, base, uses, in_closure);
                self.walk(cx, other, uses, in_closure);
            }
            ExprKind::Struct { fields, rest, .. } => {
                for value in fields.iter().filter_map(|f| f.value.as_ref()) {
                    self.walk(cx, value, uses, in_closure);
                }
                if let Some(rest) = rest {
                    self.walk(cx, rest, uses, in_closure);
                }
            }
            ExprKind::Try(inner) => {
                if !in_closure {
                    uses.tries += 1;
                }
                self.walk(cx, inner, uses, in_closure);
            }
            ExprKind::Closure(closure) => self.walk(cx, &closure.body, uses, true),
            ExprKind::Await(_) => self.unsupported("`.await`", expr.span, "awaited here"),
            ExprKind::Send(_) => self.unsupported("`send`", expr.span, "sent here"),
            ExprKind::Move(_) => {
                self.unsupported("`move` across a boundary", expr.span, "moved here");
            }
            ExprKind::If {
                cond,
                then_block,
                else_branch,
            } if in_closure => {
                self.walk(cx, cond, uses, true);
                self.walk_block(cx, then_block, uses);
                if let Some(other) = else_branch {
                    self.walk(cx, other, uses, true);
                }
            }
            ExprKind::Return(value) if in_closure => {
                if let Some(value) = value {
                    self.walk(cx, value, uses, true);
                }
            }
            ExprKind::Block(block) if in_closure => self.walk_block(cx, block, uses),
            ExprKind::If { .. } => {
                self.unsupported("an `if` inside an expression", expr.span, "this `if`");
            }
            ExprKind::Return(_) => {
                self.unsupported(
                    "a `return` inside an expression",
                    expr.span,
                    "this `return`",
                );
            }
            ExprKind::Block(_) => {
                self.unsupported("a block inside an expression", expr.span, "this block");
            }
        }
    }

    /// Walks a block inside a closure, for the links it makes.
    fn walk_block(&mut self, cx: &Context<'_>, block: &Block, uses: &mut Uses) {
        for stmt in &block.stmts {
            match stmt {
                Stmt::Let {
                    init, else_block, ..
                } => {
                    if let Some(init) = init {
                        self.walk(cx, init, uses, true);
                    }
                    if let Some(block) = else_block {
                        self.walk_block(cx, block, uses);
                    }
                }
                Stmt::Expr { expr, .. } => self.walk(cx, expr, uses, true),
            }
        }
    }

    /// The function a call names: an algorithm at the top level, or one of
    /// the component's own, as `Self::name`.
    fn callee(&self, cx: &Context<'_>, callee: &Expr) -> Option<String> {
        let ExprKind::Path(path) = &callee.kind else {
            return None;
        };
        let names: Vec<&str> = path
            .segments
            .iter()
            .map(|s| s.ident.name.as_str())
            .collect();
        match names.as_slice() {
            [name] if self.algorithms.contains(name) => Some(format!("{}/fn:{name}", self.app)),
            ["Self", name] => {
                let scope = cx.scope?;
                scope
                    .definition
                    .functions
                    .iter()
                    .any(|f| f.name.name == *name)
                    .then(|| format!("{}/fn:{name}", scope.component))
            }
            _ => None,
        }
    }

    /// The edge a call of `method` on `receiver` uses, when the receiver is
    /// a capability field, `self.name`. The method is recorded as one of
    /// the capability's operations.
    fn capability_call(
        &mut self,
        cx: &Context<'_>,
        receiver: &Expr,
        method: &str,
    ) -> Option<String> {
        let ExprKind::Field { base, name } = &receiver.kind else {
            return None;
        };
        let ExprKind::Path(path) = &base.kind else {
            return None;
        };
        if !matches!(path.segments.as_slice(), [s] if s.ident.name == "self") {
            return None;
        }
        let field = format!("{}/field:{}", cx.scope?.component, name.name);
        let operations = self.operations.get_mut(&field)?;
        operations.insert(method.to_string());
        let mut grant = &field;
        while let Some(next) = self.moved_from.get(grant) {
            grant = next;
        }
        Some(format!("{field}->{grant}"))
    }
}
