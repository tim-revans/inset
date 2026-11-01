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
use crate::model::{Arg, Branch, Edge, Exit, FieldKind, Pass, Step};

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
    /// The function's ID, where its message and emit edges start.
    function: &'s str,
    /// What the error path of a `?` returns: `None` in a function that
    /// returns an `Option`, `Err(error)` in one that returns a `Result`.
    exit: Option<&'static str>,
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
        let exit = function.ret.as_ref().and_then(|ty| match &ty.kind {
            ast::TypeKind::Path(p) => match p.segments.last()?.ident.name.as_str() {
                "Option" => Some("None"),
                "Result" => Some("Err(error)"),
                _ => None,
            },
            _ => None,
        });
        let cx = Context {
            function: id,
            exit,
            scope,
        };
        let value = match function.ret {
            Some(_) => Value::Returned,
            None => Value::Discarded,
        };
        self.block(&cx, id, &function.body, value, false)
    }

    /// A block's steps, with IDs under `prefix`. In a `branch` block, a
    /// `#[defensive]` on the first statement marks the branch, and elsewhere
    /// it marks the statement's error path.
    fn block(
        &mut self,
        cx: &Context<'_>,
        prefix: &str,
        block: &Block,
        value: Value,
        branch: bool,
    ) -> Vec<Step> {
        let mut steps = Vec::new();
        let count = block.stmts.len();
        for (i, stmt) in block.stmts.iter().enumerate() {
            let id = format!("{prefix}/{}", steps.len());
            self.excused = stmt
                .defensive()
                .filter(|_| !(branch && i == 0))
                .map(|d| d.reason.value.clone());
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
                            steps: self.block(
                                cx,
                                &format!("{id}/b1"),
                                otherwise,
                                Value::Discarded,
                                true,
                            ),
                            defensive: marked(otherwise),
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
                Stmt::Let {
                    init:
                        Some(
                            init @ Expr {
                                kind: ExprKind::Match { scrutinee, .. },
                                ..
                            },
                        ),
                    span,
                    ..
                } => {
                    let head = Span::new(span.start, scrutinee.span.end);
                    Some(self.match_decision(cx, id, head, init, Value::Assigned))
                }
                Stmt::Let {
                    init:
                        Some(Expr {
                            kind: ExprKind::Fork { iter, body, .. },
                            ..
                        }),
                    else_block: None,
                    span,
                    ..
                } => {
                    let head = Span::new(span.start, body.span.start);
                    Some(self.fork_step(cx, id, head, iter, body))
                }
                Stmt::Let { init, span, .. } => {
                    let uses = self.uses(cx, init.as_ref());
                    Some(self.action(cx, id, *span, uses))
                }
                Stmt::Expr { expr, semi, .. } => {
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
            ExprKind::Match { scrutinee, .. } => {
                let head = Span::new(expr.span.start, scrutinee.span.end);
                Some(self.match_decision(cx, id, head, expr, value))
            }
            ExprKind::For { iter, body, .. } => {
                let head = Span::new(expr.span.start, body.span.start);
                let uses = self.uses(cx, Some(iter));
                self.no_exit(&uses, iter.span, "in a loop's head");
                Some(self.loop_step(cx, id, head, uses, body, "each"))
            }
            ExprKind::Fork { iter, body, .. } => {
                let head = Span::new(expr.span.start, body.span.start);
                Some(self.fork_step(cx, id, head, iter, body))
            }
            ExprKind::While { cond, body } => {
                let head = Span::new(expr.span.start, body.span.start);
                let uses = self.uses(cx, Some(cond));
                self.no_exit(&uses, cond.span, "in a loop's head");
                Some(self.loop_step(cx, id, head, uses, body, "repeat"))
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
            if cx.exit.is_none() {
                self.unsupported(
                    "`?` in a function that returns neither an `Option` nor a `Result`",
                    span,
                    "returns early here",
                );
            }
            Exit {
                id: format!("{id}/err"),
                text: cx.exit.unwrap_or("None").into(),
                defensive: self.excused.take(),
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
        let no_marked = match else_branch.as_deref() {
            Some(Expr {
                kind: ExprKind::Block(block),
                ..
            }) => marked(block),
            _ => None,
        };
        let no_steps = match else_branch.as_deref() {
            None => Vec::new(),
            Some(Expr {
                kind: ExprKind::Block(block),
                ..
            }) => self.block(cx, &no, block, value, true),
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
                    steps: self.block(cx, &yes, then_block, value, true),
                    id: yes,
                    label: "yes".into(),
                    defensive: marked(then_block),
                },
                Branch {
                    id: no,
                    label: "no".into(),
                    steps: no_steps,
                    defensive: no_marked,
                },
            ],
            id,
        }
    }

    /// A `match`, whose head, `match value` or `let name = match value`, is
    /// `head`. Each arm is a branch, labelled with its pattern and guard.
    fn match_decision(
        &mut self,
        cx: &Context<'_>,
        id: String,
        head: Span,
        expr: &Expr,
        value: Value,
    ) -> Step {
        let ExprKind::Match { scrutinee, arms } = &expr.kind else {
            unreachable!("a match decision is drawn for a `match`");
        };
        let uses = self.uses(cx, Some(scrutinee));
        self.no_exit(&uses, scrutinee.span, "in a `match`'s value");
        self.locate(&id, head);
        let mut branches = Vec::new();
        for (i, arm) in arms.iter().enumerate() {
            let branch = format!("{id}/b{i}");
            let label_end = arm.guard.as_ref().map_or(arm.pattern.span, |g| g.span);
            if let Some(guard) = &arm.guard {
                let uses = self.uses(cx, Some(guard));
                self.no_exit(&uses, guard.span, "in a `match` guard");
            }
            self.locate(&branch, arm.body.span);
            let steps = self.arm(cx, &branch, &arm.body, value);
            branches.push(Branch {
                label: self.text(Span::new(arm.pattern.span.start, label_end.end)),
                id: branch,
                steps,
                defensive: arm.defensive.as_ref().map(|d| d.reason.value.clone()),
            });
        }
        Step::Decision {
            text: self.text(head),
            id,
            links: uses.links,
            branches,
        }
    }

    /// The steps of a `match` arm: its block, or its expression as the
    /// block's final value.
    fn arm(&mut self, cx: &Context<'_>, prefix: &str, body: &Expr, value: Value) -> Vec<Step> {
        if let ExprKind::Block(block) = &body.kind {
            return self.block(cx, prefix, block, value, false);
        }
        let id = format!("{prefix}/0");
        let step = self.statement(cx, id, body, true, value);
        step.into_iter()
            .inspect(|step| {
                self.sources
                    .entry(step.id().to_string())
                    .or_insert(body.span);
            })
            .collect()
    }

    /// A `for` or `while` loop: its head, its body, and the way past it
    /// that runs the body no times.
    fn loop_step(
        &mut self,
        cx: &Context<'_>,
        id: String,
        head: Span,
        uses: Uses,
        body: &Block,
        label: &str,
    ) -> Step {
        let body_id = format!("{id}/body");
        let skip_id = format!("{id}/skip");
        self.locate(&id, head);
        self.locate(&body_id, body.span);
        self.locate(&skip_id, head);
        Step::Loop {
            text: self.text(head),
            links: uses.links,
            concurrent: false,
            body: Branch {
                steps: self.block(cx, &body_id, body, Value::Discarded, false),
                id: body_id,
                label: label.into(),
                defensive: None,
            },
            skip: Branch {
                id: skip_id,
                label: "none".into(),
                steps: Vec::new(),
                defensive: None,
            },
            id,
        }
    }

    /// A `fork for`: a loop whose bodies run concurrently and are joined.
    /// The body's final value is the branch's, drawn if it does something.
    fn fork_step(
        &mut self,
        cx: &Context<'_>,
        id: String,
        head: Span,
        iter: &Expr,
        body: &Block,
    ) -> Step {
        let uses = self.uses(cx, Some(iter));
        self.no_exit(&uses, iter.span, "in a fork's head");
        let body_id = format!("{id}/body");
        let skip_id = format!("{id}/skip");
        self.locate(&id, head);
        self.locate(&body_id, body.span);
        self.locate(&skip_id, head);
        Step::Loop {
            text: self.text(head),
            links: uses.links,
            concurrent: true,
            body: Branch {
                steps: self.block(cx, &body_id, body, Value::Assigned, false),
                id: body_id,
                label: "each".into(),
                defensive: None,
            },
            skip: Branch {
                id: skip_id,
                label: "none".into(),
                steps: Vec::new(),
                defensive: None,
            },
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
            ExprKind::Let { expr: inner, .. } => self.walk(cx, inner, uses, in_closure),
            ExprKind::Range { start, end, .. } => {
                for part in [start, end].into_iter().flatten() {
                    self.walk(cx, part, uses, in_closure);
                }
            }
            ExprKind::Match { scrutinee, arms } if in_closure => {
                self.walk(cx, scrutinee, uses, true);
                for arm in arms {
                    if let Some(guard) = &arm.guard {
                        self.walk(cx, guard, uses, true);
                    }
                    self.walk(cx, &arm.body, uses, true);
                }
            }
            ExprKind::For { iter, body, .. } | ExprKind::Fork { iter, body, .. } if in_closure => {
                self.walk(cx, iter, uses, true);
                self.walk_block(cx, body, uses);
            }
            ExprKind::While { cond, body } if in_closure => {
                self.walk(cx, cond, uses, true);
                self.walk_block(cx, body, uses);
            }
            ExprKind::Match { .. } => {
                self.unsupported("a `match` inside an expression", expr.span, "this `match`");
            }
            ExprKind::For { .. } | ExprKind::While { .. } => {
                self.unsupported("a loop inside an expression", expr.span, "this loop");
            }
            ExprKind::Fork { .. } => {
                self.unsupported("a `fork` inside an expression", expr.span, "this `fork`");
            }
            // A capability call is a step linked to the capability's edge,
            // and a message, awaited or sent, a step linked to its arrow.
            ExprKind::Await(inner) if self.calls_a_capability(cx, inner) => {
                self.walk(cx, inner, uses, in_closure);
            }
            ExprKind::Await(inner) => match self.message(cx, inner, false) {
                Some(edge) => {
                    link(uses, edge);
                    self.walk_args(cx, inner, uses, in_closure);
                }
                None => self.unsupported("this `.await`", expr.span, "awaited here"),
            },
            ExprKind::Send(inner) => match self.message(cx, inner, true) {
                Some(edge) => {
                    link(uses, edge);
                    self.walk_args(cx, inner, uses, in_closure);
                }
                None => self.unsupported("this `send`", expr.span, "sent here"),
            },
            ExprKind::Move(_) => {}
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

    /// Walks the arguments of a message, for the links they make.
    fn walk_args(&mut self, cx: &Context<'_>, call: &Expr, uses: &mut Uses, in_closure: bool) {
        if let ExprKind::MethodCall { args, .. } = &call.kind {
            for arg in args {
                self.walk(cx, arg, uses, in_closure);
            }
        }
    }

    /// The edge a message makes, when `call` sends one: a message of a
    /// child or a lent sibling, `self.name.message(...)`, or with `send`,
    /// a value emitted through an outlet, `self.outlet.emit(value)`. The
    /// edge is added, and its ID returned.
    fn message(&mut self, cx: &Context<'_>, call: &Expr, send: bool) -> Option<String> {
        let ExprKind::MethodCall {
            receiver,
            method,
            args,
        } = &call.kind
        else {
            return None;
        };
        let ExprKind::Field { base, name } = &receiver.kind else {
            return None;
        };
        let is_self = matches!(&base.kind, ExprKind::Path(p) if matches!(p.segments.as_slice(), [s] if s.ident.name == "self"));
        let scope = cx.scope?;
        if !is_self {
            return None;
        }
        let method = method.ident.name.as_str();
        let field = scope
            .definition
            .fields
            .iter()
            .find(|f| f.name.name == name.name);
        let is_outlet = field.is_some_and(|f| super::field_kind(&f.ty) == FieldKind::Outlet);
        if send && is_outlet && method == "emit" {
            let [value] = args.as_slice() else {
                return None;
            };
            let to = format!("{}/field:{}", scope.component, name.name);
            let id = self.edge_id(cx.function, &to);
            self.locate(&id, call.span);
            self.emit_edges.push(Edge::Emit {
                id: id.clone(),
                from: cx.function.to_string(),
                to,
                value: self.arg(value),
            });
            return Some(id);
        }
        let (_, target, definition) = scope.handles.iter().find(|(h, ..)| *h == name.name)?;
        let is_message = definition.functions.iter().any(|f| {
            f.name.name == method
                && f.attrs
                    .iter()
                    .any(|a| a.kind == ast::AttributeKind::Message)
        });
        if !is_message {
            return None;
        }
        let to = format!("{target}/fn:{method}");
        let id = self.edge_id(cx.function, &to);
        self.locate(&id, call.span);
        let args = args.iter().map(|arg| self.arg(arg)).collect();
        self.message_edges.push(Edge::Message {
            id: id.clone(),
            from: cx.function.to_string(),
            to,
            send,
            args,
        });
        Some(id)
    }

    /// How a value crosses a boundary, from how it is written.
    fn arg(&self, arg: &Expr) -> Arg {
        let (pass, span) = match &arg.kind {
            ExprKind::Ref { operand, .. } => (Pass::Lend, operand.span),
            ExprKind::Move(place) => (Pass::Move, place.span),
            // The checker allows a bare place only when it is `Copy`.
            ExprKind::Path(_) | ExprKind::Field { .. } => (Pass::Copy, arg.span),
            _ => (Pass::New, arg.span),
        };
        Arg {
            pass,
            text: self.text(span),
        }
    }

    /// `<from>-><to>`, with `#2` and so on added when the two are already joined.
    fn edge_id(&self, from: &str, to: &str) -> String {
        let id = format!("{from}->{to}");
        let taken = |id: &str| {
            self.message_edges
                .iter()
                .chain(&self.emit_edges)
                .any(|e| e.id() == id)
        };
        if !taken(&id) {
            return id;
        }
        (2..)
            .map(|n| format!("{id}#{n}"))
            .find(|id| !taken(id))
            .expect("some number is free")
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
            ["Self", name] => {
                let scope = cx.scope?;
                scope
                    .definition
                    .functions
                    .iter()
                    .any(|f| f.name.name == *name)
                    .then(|| format!("{}/fn:{name}", scope.component))
            }
            _ => {
                let last = &path.segments[path.segments.len() - 1].ident;
                let target = self.refs.get(&last.span)?;
                self.algorithms.get(target).cloned()
            }
        }
    }

    /// Whether an expression calls a method of a capability field.
    fn calls_a_capability(&self, cx: &Context<'_>, expr: &Expr) -> bool {
        let ExprKind::MethodCall { receiver, .. } = &expr.kind else {
            return false;
        };
        let ExprKind::Field { base, name } = &receiver.kind else {
            return false;
        };
        let is_self = matches!(&base.kind, ExprKind::Path(p) if matches!(p.segments.as_slice(), [s] if s.ident.name == "self"));
        is_self
            && cx.scope.is_some_and(|scope| {
                self.operations
                    .contains_key(&format!("{}/field:{}", scope.component, name.name))
            })
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

/// The reason a `#[defensive]` on a branch block's first statement gives.
fn marked(block: &Block) -> Option<String> {
    let first = block.stmts.first()?;
    Some(first.defensive()?.reason.value.clone())
}
