//! Coverage: the emitted code marks each element coverage counts as it
//! passes it, by the element's diagram ID, so `inset test` can tell which
//! requirement reached what (see `inset_runtime::coverage`).
//!
//! The elements are those `docs/language/diagrams.md` gives an ID to and
//! `syntax.md` counts: entering a function, each branch of a decision, a
//! loop's body and its skip, the error path of a `?`, and each arm of a
//! `failure` block. A function body is walked here exactly as
//! `inset-diagram` walks it to draw the flowchart, so the IDs agree: a step's
//! index counts only the statements the flowchart draws, and a construct the
//! flowchart cannot draw yet, such as an `if` inside an expression, is
//! emitted as written, unmarked. A test checks that every element the diagram
//! of each example and test program has is marked, and nothing else.
//!
//! A component's elements are listed by their IDs relative to a running
//! component, in `Component::BRANCHES`, because a component type started
//! twice runs as two components with IDs of their own. A top-level
//! algorithm's elements are listed by their full IDs, for the app.
//!
//! Marking a branch adds one statement at the start of its block. A loop's
//! skip needs to know whether the body ran, so a loop is wrapped in a block
//! with a flag. The error path of a `?` is marked by passing the value to
//! `coverage::exit`, which returns it unchanged. None of this changes what
//! the code does.

use inset_syntax::ast::{Block, Expr, ExprKind, Function, Stmt};
use proc_macro2::{Literal, TokenStream};
use quote::quote;

use crate::Emitter;
use crate::body::pattern;

/// Whose elements the function being emitted marks.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Owner {
    /// A component's, by IDs relative to the running component.
    Component,
    /// The app's, for a top-level algorithm.
    Algorithms,
}

/// What happens to the final value of a block, as the flowchart decides
/// whether to draw it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Value {
    Returned,
    Discarded,
    Assigned,
}

/// The elements coverage counts, as `inset build` emitted them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Elements {
    /// Each component type's, by its Rust path such as
    /// `crate::warehouse::Inventory`, relative to a running component.
    pub components: Vec<(String, Vec<String>)>,
    /// The top-level algorithms', by their full IDs.
    pub algorithms: Vec<String>,
}

impl Emitter<'_> {
    /// Lists an element, and returns the statement that marks it.
    fn mark(&mut self, id: String) -> TokenStream {
        let index = Literal::usize_unsuffixed(self.list(id));
        match self.owner {
            Owner::Component => quote!(::inset_runtime::coverage::branch::<Self>(#index);),
            Owner::Algorithms => quote!(::inset_runtime::coverage::algorithm(#index);),
        }
    }

    /// Lists an element, and returns its index.
    pub(crate) fn list(&mut self, id: String) -> usize {
        let list = match self.owner {
            Owner::Component => &mut self.branches,
            Owner::Algorithms => &mut self.elements.algorithms,
        };
        list.push(id);
        list.len() - 1
    }

    /// `expr?` with its error path marked, when a step's `?` is being emitted.
    pub(crate) fn tried(&mut self, base: TokenStream) -> TokenStream {
        match self.exit {
            Some(index) => {
                let index = Literal::usize_unsuffixed(index);
                match self.owner {
                    Owner::Component => {
                        quote!(::inset_runtime::coverage::exit::<Self, _>(#base, #index)?)
                    }
                    Owner::Algorithms => {
                        quote!(::inset_runtime::coverage::algorithm_exit(#base, #index)?)
                    }
                }
            }
            None => quote!(#base?),
        }
    }

    /// A function's body, marking its entry and every branch in it. `id` is
    /// the function's ID: relative for a component's, and full for an
    /// algorithm's.
    pub(crate) fn covered_body(&mut self, id: &str, function: &Function) -> TokenStream {
        let entry = self.mark(id.to_string());
        let value = match function.ret {
            Some(_) => Value::Returned,
            None => Value::Discarded,
        };
        self.covered_block(id, &function.body, value, entry)
    }

    /// A block whose steps have IDs under `prefix`, with `lead` first.
    fn covered_block(
        &mut self,
        prefix: &str,
        block: &Block,
        value: Value,
        lead: TokenStream,
    ) -> TokenStream {
        let mut steps = 0;
        let count = block.stmts.len();
        let mut out = vec![lead];
        for (i, stmt) in block.stmts.iter().enumerate() {
            let id = format!("{prefix}/{steps}");
            let (tokens, drawn) = match stmt {
                Stmt::Let {
                    pattern: p,
                    ty,
                    init: Some(init),
                    else_block: Some(otherwise),
                    ..
                } => {
                    let p = pattern(p);
                    let ty = ty.as_ref().map(|t| {
                        let t = self.ty(t);
                        quote!(: #t)
                    });
                    let init = self.expr(init);
                    let matched = self.mark(format!("{id}/b0"));
                    let lead = self.mark(format!("{id}/b1"));
                    let otherwise =
                        self.covered_block(&format!("{id}/b1"), otherwise, Value::Discarded, lead);
                    (quote!(let #p #ty = #init else #otherwise; #matched), true)
                }
                Stmt::Let {
                    pattern: p,
                    ty,
                    init:
                        Some(
                            init @ Expr {
                                kind: ExprKind::If { .. } | ExprKind::Match { .. },
                                ..
                            },
                        ),
                    else_block: None,
                    ..
                } => {
                    let p = pattern(p);
                    let ty = ty.as_ref().map(|t| {
                        let t = self.ty(t);
                        quote!(: #t)
                    });
                    let init = self.decision(&id, init, Value::Assigned);
                    (quote!(let #p #ty = #init;), true)
                }
                Stmt::Let { init, .. } => {
                    let exit = init.as_ref().is_some_and(tries);
                    (self.action(&id, exit, |e| e.stmt(stmt)), true)
                }
                Stmt::Expr { expr, semi, .. } => {
                    let last = i + 1 == count && !semi;
                    let value = if last { value } else { Value::Discarded };
                    let (tokens, drawn) = self.statement(&id, expr, last, value);
                    let semi = semi.then(|| quote!(;));
                    (quote!(#tokens #semi), drawn)
                }
            };
            if drawn {
                steps += 1;
            }
            out.push(tokens);
        }
        quote!({ #(#out)* })
    }

    /// A statement, and whether the flowchart draws it as a step.
    fn statement(
        &mut self,
        id: &str,
        expr: &Expr,
        last: bool,
        value: Value,
    ) -> (TokenStream, bool) {
        match &expr.kind {
            ExprKind::If { .. } | ExprKind::Match { .. } => (self.decision(id, expr, value), true),
            ExprKind::For {
                pattern: p,
                iter,
                body,
            } => {
                let p = pattern(p);
                let iter = self.expr(iter);
                let head = quote!(for #p in #iter);
                (self.covered_loop(id, head, body), true)
            }
            ExprKind::While { cond, body } => {
                let cond = self.expr(cond);
                (self.covered_loop(id, quote!(while #cond), body), true)
            }
            // The flowchart cannot draw a block in a body yet.
            ExprKind::Block(_) => (self.expr(expr), false),
            ExprKind::Return(_) => (self.expr(expr), true),
            _ if last && value == Value::Returned => (self.expr(expr), true),
            ExprKind::Path(_) | ExprKind::Lit(_) if last && value == Value::Assigned => {
                (self.expr(expr), false)
            }
            _ => (self.action(id, tries(expr), |e| e.expr(expr)), true),
        }
    }

    /// A step that runs and carries on, with the error path of its `?`
    /// marked, if it has one.
    fn action(
        &mut self,
        id: &str,
        exit: bool,
        emit: impl FnOnce(&mut Self) -> TokenStream,
    ) -> TokenStream {
        let outer = self.exit.take();
        if exit {
            self.exit = Some(self.list(format!("{id}/err")));
        }
        let tokens = emit(self);
        self.exit = outer;
        tokens
    }

    /// An `if` or a `match`, with each of its branches marked.
    fn decision(&mut self, id: &str, expr: &Expr, value: Value) -> TokenStream {
        match &expr.kind {
            ExprKind::If {
                cond,
                then_block,
                else_branch,
            } => {
                let cond = self.expr(cond);
                let yes = format!("{id}/b0");
                let no = format!("{id}/b1");
                let lead = self.mark(yes.clone());
                let then_block = self.covered_block(&yes, then_block, value, lead);
                let lead = self.mark(no.clone());
                let otherwise = match else_branch.as_deref() {
                    None => quote!({ #lead }),
                    Some(Expr {
                        kind: ExprKind::Block(block),
                        ..
                    }) => self.covered_block(&no, block, value, lead),
                    Some(nested) => {
                        let nested = self.decision(&format!("{no}/0"), nested, value);
                        quote!({ #lead #nested })
                    }
                };
                quote!(if #cond #then_block else #otherwise)
            }
            ExprKind::Match { scrutinee, arms } => {
                let scrutinee = self.expr(scrutinee);
                let arms: Vec<_> = arms
                    .iter()
                    .enumerate()
                    .map(|(i, arm)| {
                        let branch = format!("{id}/b{i}");
                        let pat = pattern(&arm.pattern);
                        let guard = arm.guard.as_ref().map(|g| {
                            let g = self.expr(g);
                            quote!(if #g)
                        });
                        let lead = self.mark(branch.clone());
                        let body = match &arm.body.kind {
                            ExprKind::Block(block) => {
                                self.covered_block(&branch, block, value, lead)
                            }
                            _ => {
                                let (body, _) =
                                    self.statement(&format!("{branch}/0"), &arm.body, true, value);
                                quote!({ #lead #body })
                            }
                        };
                        quote!(#pat #guard => #body,)
                    })
                    .collect();
                quote!(match #scrutinee { #(#arms)* })
            }
            _ => unreachable!("a decision is an `if` or a `match`"),
        }
    }

    /// A loop, with its body and its skip marked. The skip is the loop
    /// running its body no times, which only the end of the loop can tell.
    fn covered_loop(&mut self, id: &str, head: TokenStream, body: &Block) -> TokenStream {
        let ran = quote!(__inset_ran);
        let mark = self.mark(format!("{id}/body"));
        let lead = quote!(#ran = true; #mark);
        let body = self.covered_block(&format!("{id}/body"), body, Value::Discarded, lead);
        let skip = self.mark(format!("{id}/skip"));
        quote!({
            let mut #ran = false;
            #head #body
            if !#ran {
                #skip
            }
        })
    }
}

/// Whether an expression has a `?` outside any closure, which the
/// flowchart draws as the step's error path.
fn tries(expr: &Expr) -> bool {
    let block = |block: &Block| {
        block.stmts.iter().any(|stmt| match stmt {
            Stmt::Let {
                init, else_block, ..
            } => {
                init.as_ref().is_some_and(tries)
                    || else_block.as_ref().is_some_and(|b| {
                        b.stmts
                            .iter()
                            .any(|s| matches!(s, Stmt::Expr { expr, .. } if tries(expr)))
                    })
            }
            Stmt::Expr { expr, .. } => tries(expr),
        })
    };
    match &expr.kind {
        ExprKind::Try(_) => true,
        ExprKind::Lit(_) | ExprKind::Path(_) | ExprKind::Move(_) | ExprKind::Closure(_) => false,
        ExprKind::Call { callee, args } => tries(callee) || args.iter().any(tries),
        ExprKind::MethodCall { receiver, args, .. } => tries(receiver) || args.iter().any(tries),
        ExprKind::Macro { args, .. } | ExprKind::Tuple(args) | ExprKind::Array(args) => {
            args.iter().any(tries)
        }
        ExprKind::Field { base, .. }
        | ExprKind::Unary { operand: base, .. }
        | ExprKind::Ref { operand: base, .. }
        | ExprKind::Cast { expr: base, .. }
        | ExprKind::Paren(base)
        | ExprKind::Await(base)
        | ExprKind::Send(base)
        | ExprKind::Let { expr: base, .. } => tries(base),
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
        } => tries(base) || tries(other),
        ExprKind::Struct { fields, rest, .. } => {
            fields
                .iter()
                .filter_map(|field| field.value.as_ref())
                .any(tries)
                || rest.as_deref().is_some_and(tries)
        }
        ExprKind::Range { start, end, .. } => {
            start.as_deref().is_some_and(tries) || end.as_deref().is_some_and(tries)
        }
        ExprKind::Return(value) => value.as_deref().is_some_and(tries),
        ExprKind::Block(b) => block(b),
        ExprKind::If {
            cond,
            then_block,
            else_branch,
        } => tries(cond) || block(then_block) || else_branch.as_deref().is_some_and(tries),
        ExprKind::Match { scrutinee, arms } => {
            tries(scrutinee)
                || arms
                    .iter()
                    .any(|arm| arm.guard.as_ref().is_some_and(tries) || tries(&arm.body))
        }
        ExprKind::For { iter, body, .. } => tries(iter) || block(body),
        ExprKind::While { cond, body } => tries(cond) || block(body),
    }
}
