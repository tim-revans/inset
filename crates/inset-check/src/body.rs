//! Function bodies: names, types, `self`, closures, messages, outlets,
//! capability calls and values crossing component boundaries.

use inset_syntax::ast::*;
use inset_syntax::{Diagnostic, Ident, Span};

use crate::items::handed_down_use;
use crate::names::{Def, Ns};
use crate::types::{FLOATS, INTS, Resolver, Ty, data_ty, fits};
use crate::wiring::{mismatch, mutable_borrow, without_move};
use crate::{Checker, FnKind, Sig, names};

/// How the value of a call is used, which decides whether a message or
/// capability call is allowed.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Use {
    Value,
    Awaited,
    Sent,
}

/// Standard names a path in an expression may start with.
const STD_NAMES: [&str; 19] = [
    "String", "Vec", "Option", "Result", "HashMap", "BTreeMap", "HashSet", "BTreeSet", "VecDeque",
    "Box", "Rc", "Arc", "Cell", "RefCell", "Mutex", "RwLock", "Decimal", "char", "str",
];

/// The macros Inset keeps (syntax.md, "What Inset keeps from Rust").
const MACROS: [&str; 7] = [
    "vec", "format", "matches", "assert", "write", "panic", "dec",
];

impl Checker<'_> {
    pub(crate) fn check_bodies(&mut self) {
        let mut checked = Vec::new();
        for (index, comp) in self.components.iter().enumerate() {
            for sig in &comp.functions {
                checked.push(Body::function(self, Some(index), sig));
            }
        }
        for sig in &self.algorithms {
            checked.push(Body::function(self, None, sig));
        }
        let mut marks = Vec::new();
        for sig in self
            .components
            .iter()
            .flat_map(|c| &c.functions)
            .chain(&self.algorithms)
        {
            crate::defensive::check(&sig.ast.body, &mut marks);
        }
        self.diagnostics.extend(marks);
        for body in checked {
            self.diagnostics.extend(body.diagnostics);
            self.locals.extend(body.locals);
            self.refs.extend(body.refs);
        }
    }
}

/// What checking a function's body found.
pub(crate) struct Checked {
    pub diagnostics: Vec<Diagnostic>,
    /// The parameters and variables it binds, with their types.
    pub locals: Vec<(Ident, Ty)>,
    /// Each name it resolved, with the name of what it refers to.
    pub refs: Vec<(Span, Span)>,
}

/// A variable in scope: its name, its type and where it was bound.
struct Local {
    name: String,
    ty: Ty,
    span: Span,
}

pub(crate) struct Body<'c, 'a> {
    cx: &'c Checker<'a>,
    /// The module the code is in, where its names resolve.
    module: usize,
    comp: Option<usize>,
    /// The function being checked, or `None` for a handoff in a `start`.
    function: Option<&'c Sig<'a>>,
    scopes: Vec<Vec<Local>>,
    /// Inside a closure, which may not wait, send or call a capability.
    in_closure: bool,
    diagnostics: Vec<Diagnostic>,
    locals: Vec<(Ident, Ty)>,
    refs: Vec<(Span, Span)>,
}

impl<'c, 'a> Body<'c, 'a> {
    fn new(cx: &'c Checker<'a>, comp: Option<usize>, function: Option<&'c Sig<'a>>) -> Self {
        let module = match (function, comp) {
            (Some(sig), _) => sig.module,
            (None, Some(comp)) => cx.components[comp].module,
            (None, None) => 0,
        };
        Body {
            cx,
            module,
            comp,
            function,
            scopes: vec![Vec::new()],
            in_closure: false,
            diagnostics: Vec::new(),
            locals: Vec::new(),
            refs: Vec::new(),
        }
    }

    pub(crate) fn function(cx: &'c Checker<'a>, comp: Option<usize>, sig: &'c Sig<'a>) -> Checked {
        let mut body = Body::new(cx, comp, Some(sig));
        for (param, ty) in sig.ast.params.iter().zip(&sig.params) {
            body.bind(&param.pattern, ty.clone());
        }
        let ty = body.block(&sig.ast.body);
        let tail = match sig.ast.body.stmts.last() {
            Some(Stmt::Expr {
                expr, semi: false, ..
            }) => Some(expr),
            _ => None,
        };
        if !fits(&sig.ret, &ty) {
            match tail {
                Some(tail) => body.error(mismatch(
                    tail.span,
                    &sig.ret,
                    &ty,
                    &format!("the reply of `{}`", sig.name()),
                )),
                None => {
                    let end = sig.ast.body.span.end;
                    body.error(
                        Diagnostic::new(
                            format!(
                                "`{}` returns a `{}`, but its body ends without a value",
                                sig.name(),
                                sig.ret
                            ),
                            Span::new(end - 1, end),
                            "the body ends here",
                        )
                        .with_help("end the body with the value to return, without a `;` after it"),
                    );
                }
            }
        }
        Checked {
            diagnostics: body.diagnostics,
            locals: body.locals,
            refs: body.refs,
        }
    }

    /// Checks the value of a handoff in a `start` statement, with the names
    /// of the settings or fields in scope.
    pub(crate) fn start_expr(
        cx: &'c Checker<'a>,
        comp: Option<usize>,
        names: Vec<(&Ident, Ty)>,
        expr: &Expr,
    ) -> (Ty, Vec<Diagnostic>) {
        let mut body = Body::new(cx, comp, None);
        body.scopes = vec![
            names
                .into_iter()
                .map(|(name, ty)| Local {
                    name: name.name.clone(),
                    ty,
                    span: name.span,
                })
                .collect(),
        ];
        let ty = body.expr(expr);
        (ty, body.diagnostics)
    }

    fn error(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    fn comp(&self) -> Option<&'c crate::Comp<'a>> {
        self.comp.map(|i| &self.cx.components[i])
    }

    fn receiver(&self) -> Option<&'a Receiver> {
        self.function.and_then(|f| f.ast.receiver.as_ref())
    }

    fn fn_name(&self) -> &str {
        self.function.map_or("a `start` statement", |f| f.name())
    }

    fn resolve(&mut self, ty: &Type) -> Ty {
        let resolver = Resolver {
            names: &self.cx.names,
            module: self.module,
        };
        resolver.resolve(ty, &mut self.diagnostics)
    }

    // ---- Scopes ------------------------------------------------------------

    fn local(&self, name: &str) -> Option<&Ty> {
        self.find_local(name).map(|l| &l.ty)
    }

    fn find_local(&self, name: &str) -> Option<&Local> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.iter().rev().find(|l| l.name == name))
    }

    /// The type of the variable a name refers to, recording the reference.
    fn use_local(&mut self, name: &Ident) -> Option<Ty> {
        let local = self.find_local(&name.name)?;
        let (ty, span) = (local.ty.clone(), local.span);
        self.refs.push((name.span, span));
        Some(ty)
    }

    fn bind(&mut self, pattern: &Pattern, ty: Ty) {
        self.bind_in(pattern, ty, None);
    }

    /// Binds the names in a pattern matched against a value of type `ty`.
    /// Matching through a reference binds references, as Rust's default
    /// binding mode does: `by_ref` is then whether they are mutable.
    fn bind_in(&mut self, pattern: &Pattern, ty: Ty, by_ref: Option<bool>) {
        if let Ty::Ref { mutable, inner } = &ty
            && !matches!(
                pattern.kind,
                PatternKind::Binding { .. } | PatternKind::Wild
            )
        {
            let mode = Some(by_ref.unwrap_or(*mutable) && *mutable);
            return self.bind_in(pattern, (**inner).clone(), mode);
        }
        match &pattern.kind {
            PatternKind::Wild | PatternKind::Lit(_) => {}
            PatternKind::Binding { name, .. } => {
                if name.name == "None" {
                    return;
                }
                let ty = match by_ref {
                    Some(mutable) if ty.is_known() => Ty::Ref {
                        mutable,
                        inner: Box::new(ty),
                    },
                    Some(_) => Ty::Unknown,
                    None => ty,
                };
                self.locals.push((name.clone(), ty.clone()));
                self.scopes.last_mut().expect("a scope").push(Local {
                    name: name.name.clone(),
                    ty,
                    span: name.span,
                });
            }
            PatternKind::Tuple(elems) => {
                let tys = match ty {
                    Ty::Tuple(tys) if tys.len() == elems.len() => tys,
                    _ => vec![Ty::Unknown; elems.len()],
                };
                for (elem, ty) in elems.iter().zip(tys) {
                    self.bind_in(elem, ty, by_ref);
                }
            }
            PatternKind::TupleStruct { path, elems } => {
                let name = path.segments.last().map_or("", |s| s.ident.name.as_str());
                let inner: Vec<Ty> = match (path.segments.len(), name, &ty) {
                    (1, "Some", Ty::Std("Option", args)) | (1, "Ok", Ty::Std("Result", args)) => {
                        vec![args[0].clone()]
                    }
                    (1, "Err", Ty::Std("Result", args)) => vec![args[1].clone()],
                    (1, "Json", Ty::Std("Json", args)) => vec![args[0].clone()],
                    (1, "Some" | "Ok" | "Err" | "Json", _) => vec![Ty::Unknown],
                    _ => match self.pattern_variant(path, &ty) {
                        Some((data, variant)) => {
                            self.cx.datas[data].variants[variant].fields.clone()
                        }
                        None => Vec::new(),
                    },
                };
                if !inner.is_empty() && inner.len() != elems.len() {
                    self.error(Diagnostic::new(
                        format!(
                            "`{name}` holds {} value{}, but the pattern has {}",
                            inner.len(),
                            if inner.len() == 1 { "" } else { "s" },
                            elems.len()
                        ),
                        pattern.span,
                        format!("expected {}", inner.len()),
                    ));
                }
                for (i, elem) in elems.iter().enumerate() {
                    let ty = inner.get(i).cloned().unwrap_or(Ty::Unknown);
                    self.bind_in(elem, ty, by_ref);
                }
            }
            PatternKind::Path(path) => {
                if let Some((data, variant)) = self.pattern_variant(path, &ty)
                    && !self.cx.datas[data].variants[variant].fields.is_empty()
                {
                    let name = &self.cx.datas[data].variants[variant].name.name;
                    self.error(
                        Diagnostic::new(
                            format!("the variant `{name}` holds values"),
                            path.span,
                            "this pattern names it without them",
                        )
                        .with_help(format!(
                            "write `{}(..)` to match it whatever it holds",
                            self.text_of_path(path)
                        )),
                    );
                }
            }
            PatternKind::Struct { path, fields, rest } => {
                let data = self.data_of_type_path(path);
                let Some(data) = data else {
                    for field in fields {
                        if let Some(p) = &field.pattern {
                            self.bind_in(p, Ty::Unknown, by_ref);
                        } else {
                            self.bind_field_shorthand(&field.name, Ty::Unknown, by_ref);
                        }
                    }
                    return;
                };
                let mut named = Vec::new();
                for field in fields {
                    named.push(field.name.name.as_str());
                    let ty = self.struct_field(data, &field.name).unwrap_or(Ty::Unknown);
                    match &field.pattern {
                        Some(p) => self.bind_in(p, ty, by_ref),
                        None => self.bind_field_shorthand(&field.name, ty, by_ref),
                    }
                }
                if !rest {
                    self.missing_fields(
                        data,
                        &named,
                        path.span,
                        "name each field, or end with `..` to ignore the rest",
                    );
                }
            }
            PatternKind::Or(alternatives) => {
                for (i, alternative) in alternatives.iter().enumerate() {
                    if i == 0 {
                        self.bind_in(alternative, ty.clone(), by_ref);
                    } else {
                        // Each alternative binds the same names, as Rust requires.
                        self.scopes.push(Vec::new());
                        self.bind_in(alternative, ty.clone(), by_ref);
                        self.scopes.pop();
                    }
                }
            }
        }
    }

    fn bind_field_shorthand(&mut self, name: &Ident, ty: Ty, by_ref: Option<bool>) {
        let pattern = Pattern {
            kind: PatternKind::Binding {
                mutable: false,
                name: name.clone(),
            },
            span: name.span,
        };
        self.bind_in(&pattern, ty, by_ref);
    }

    fn text_of_path(&self, path: &Path) -> String {
        path.segments
            .iter()
            .map(|s| s.ident.name.as_str())
            .collect::<Vec<_>>()
            .join("::")
    }

    /// The enum variant a pattern's path names, reporting one that is not.
    fn pattern_variant(&mut self, path: &Path, matched: &Ty) -> Option<(usize, usize)> {
        let idents: Vec<Ident> = path.segments.iter().map(|s| s.ident.clone()).collect();
        let found = match self.cx.names.resolve_in(self.module, &idents, Ns::Value) {
            Ok(Some(Def::Variant(data, variant))) => Some((data, variant)),
            Ok(Some(Def::Std(_))) => return None,
            Err(diagnostic) => {
                self.error(diagnostic);
                return None;
            }
            Ok(_) => None,
        };
        let Some((data, variant)) = found else {
            self.error(cannot_find_type(path));
            return None;
        };
        if let Ty::Data { id, name, .. } = matched
            && *id != data
        {
            self.error(Diagnostic::new(
                format!(
                    "this pattern matches a `{}`, but the value is a `{name}`",
                    self.cx.datas[data].name.name
                ),
                path.span,
                format!("expected a variant of `{name}`"),
            ));
        }
        Some((data, variant))
    }

    /// The struct a path in a type position names, if it names one.
    fn data_of_type_path(&mut self, path: &Path) -> Option<usize> {
        let idents: Vec<Ident> = path.segments.iter().map(|s| s.ident.clone()).collect();
        match self.cx.names.resolve_in(self.module, &idents, Ns::Type) {
            Ok(Some(Def::Data(data))) if !self.cx.datas[data].is_enum => Some(data),
            Ok(Some(Def::Variant(..))) => {
                self.error(Diagnostic::new(
                    "a variant with named fields is not supported yet",
                    path.span,
                    "the checker does not handle this yet",
                ));
                None
            }
            Ok(Some(Def::Std(_))) => None,
            Err(diagnostic) => {
                self.error(diagnostic);
                None
            }
            Ok(_) => {
                self.error(cannot_find_type(path));
                None
            }
        }
    }

    /// The type of a struct's field, recording the reference and reporting
    /// a field that is not there or is private here.
    fn struct_field(&mut self, data: usize, name: &Ident) -> Option<Ty> {
        let info = &self.cx.datas[data];
        let Some(field) = info.field(&name.name) else {
            let fields = names(info.fields.iter().map(|f| f.name.name.as_str()));
            let struct_name = info.name.name.clone();
            self.error(
                Diagnostic::new(
                    format!("`{struct_name}` has no field `{}`", name.name),
                    name.span,
                    format!("not a field of `{struct_name}`"),
                )
                .with_help(if fields.is_empty() {
                    format!("`{struct_name}` has no fields")
                } else {
                    format!("the fields of `{struct_name}` are {fields}")
                }),
            );
            return None;
        };
        self.refs.push((name.span, field.name.span));
        if !self.cx.names.visible(field.vis, self.module) {
            let struct_name = &info.name.name;
            let owner = self.cx.names.modules.list[self.cx.names.datas[data].module].rust_path();
            self.error(
                Diagnostic::new(
                    format!("the field `{}` of `{struct_name}` is private", name.name),
                    name.span,
                    format!("declared in `{owner}` without `pub`"),
                )
                .with_help("make the field `pub(crate)` to use it from anywhere in the program"),
            );
        }
        Some(field.ty.clone())
    }

    fn missing_fields(&mut self, data: usize, named: &[&str], span: Span, help: &str) {
        let info = &self.cx.datas[data];
        let missing: Vec<&str> = info
            .fields
            .iter()
            .map(|f| f.name.name.as_str())
            .filter(|f| !named.contains(f))
            .collect();
        if !missing.is_empty() {
            let message = format!(
                "`{}` is missing {}",
                info.name.name,
                names(missing.iter().copied())
            );
            self.error(Diagnostic::new(message, span, "every field is given").with_help(help));
        }
    }

    // ---- Statements --------------------------------------------------------

    fn block(&mut self, block: &Block) -> Ty {
        self.scopes.push(Vec::new());
        let mut ty = Ty::unit();
        let count = block.stmts.len();
        for (i, stmt) in block.stmts.iter().enumerate() {
            ty = match stmt {
                Stmt::Let {
                    pattern,
                    ty: annotation,
                    init,
                    else_block,
                    ..
                } => {
                    let annotated = annotation.as_ref().map(|t| self.resolve(t));
                    let init_ty = init.as_ref().map(|e| match &annotated {
                        Some(t) => self.expect(e, t, "the variable"),
                        None => self.expr(e),
                    });
                    if let Some(else_block) = else_block {
                        self.block(else_block);
                    }
                    let ty = annotated.or(init_ty).unwrap_or(Ty::Unknown);
                    // A pattern matched through a reference binds references,
                    // which this check does not follow.
                    let ty = if matches!(ty, Ty::Ref { .. })
                        && !matches!(pattern.kind, PatternKind::Binding { .. })
                    {
                        Ty::Unknown
                    } else {
                        ty
                    };
                    self.bind(pattern, ty);
                    Ty::unit()
                }
                Stmt::Expr { expr, semi, .. } => {
                    let ty = self.expr(expr);
                    if !semi && i + 1 == count {
                        ty
                    } else if ty == Ty::Never {
                        Ty::Never
                    } else {
                        Ty::unit()
                    }
                }
            };
        }
        self.scopes.pop();
        ty
    }

    // ---- Expressions -------------------------------------------------------

    /// Checks an expression where a value of type `expected` is wanted.
    fn expect(&mut self, expr: &Expr, expected: &Ty, what: &str) -> Ty {
        let ty = self.expr(expr);
        if !fits(expected, &ty) {
            self.error(mismatch(expr.span, expected, &ty, what));
        }
        ty
    }

    fn expr(&mut self, expr: &Expr) -> Ty {
        self.expr_use(expr, Use::Value).0
    }

    /// Checks an expression used as `usage`. The flag says whether it was a
    /// message, outlet or capability call, which `.await` and `send` need.
    fn expr_use(&mut self, expr: &Expr, usage: Use) -> (Ty, bool) {
        if let ExprKind::MethodCall {
            receiver,
            method,
            args,
        } = &expr.kind
        {
            return self.method_call(expr.span, receiver, &method.ident, args, usage);
        }
        let ty = match &expr.kind {
            ExprKind::Lit(lit) => lit_ty(lit),
            ExprKind::Path(path) => self.path_value(path),
            ExprKind::Macro { name, args } => self.macro_call(name, args),
            ExprKind::Call { callee, args } => self.call(expr.span, callee, args),
            ExprKind::MethodCall { .. } => unreachable!("handled above"),
            ExprKind::Field { base, name } => self.field(base, name),
            ExprKind::Index { base, index } => {
                self.expr(base);
                self.expr(index);
                Ty::Unknown
            }
            ExprKind::Await(inner) => self.await_expr(expr.span, inner),
            ExprKind::Try(inner) => match self.expr(inner) {
                Ty::Std("Option" | "Result", args) => args[0].clone(),
                _ => Ty::Unknown,
            },
            ExprKind::Unary { op, operand } => {
                let ty = self.expr(operand);
                match (op, ty) {
                    (UnaryOp::Deref, Ty::Ref { inner, .. }) => *inner,
                    (UnaryOp::Deref, _) => Ty::Unknown,
                    (_, ty) => ty,
                }
            }
            ExprKind::Ref { mutable, operand } => Ty::Ref {
                mutable: *mutable,
                inner: Box::new(self.expr(operand)),
            },
            ExprKind::Binary { op, lhs, rhs } => self.binary(*op, lhs, rhs),
            ExprKind::Assign { op, lhs, rhs } => {
                self.check_assign_target(lhs);
                let target = self.expr(lhs);
                if op.is_none() {
                    self.expect(rhs, &target, "the target");
                } else {
                    self.expr(rhs);
                }
                Ty::unit()
            }
            ExprKind::Cast { expr, ty } => {
                self.expr(expr);
                self.resolve(ty)
            }
            ExprKind::Struct { path, fields, rest } => {
                self.struct_lit(path, fields, rest.as_deref())
            }
            ExprKind::Tuple(elems) => Ty::Tuple(elems.iter().map(|e| self.expr(e)).collect()),
            ExprKind::Paren(inner) => self.expr(inner),
            ExprKind::Array(elems) => {
                for elem in elems {
                    self.expr(elem);
                }
                Ty::Unknown
            }
            ExprKind::Block(block) => self.block(block),
            ExprKind::If {
                cond,
                then_block,
                else_branch,
            } => {
                self.scopes.push(Vec::new());
                self.cond(cond, "an `if` condition");
                let then_ty = self.block(then_block);
                self.scopes.pop();
                match else_branch {
                    Some(branch) => {
                        let else_ty = self.expr(branch);
                        if then_ty == Ty::Never {
                            else_ty
                        } else {
                            then_ty
                        }
                    }
                    None => Ty::unit(),
                }
            }
            ExprKind::Return(value) => {
                self.return_expr(expr.span, value.as_deref());
                Ty::Never
            }
            ExprKind::Send(inner) => {
                self.send(expr.span, inner);
                Ty::unit()
            }
            ExprKind::Move(place) => {
                self.error(
                    Diagnostic::new(
                        "`move` marks a value crossing a component boundary",
                        expr.span,
                        "nothing crosses a boundary here",
                    )
                    .with_help("remove `move`; inside a component, values move as they do in Rust"),
                );
                self.place(place)
            }
            ExprKind::Closure(closure) => {
                self.closure(closure);
                Ty::Unknown
            }
            ExprKind::Match { scrutinee, arms } => self.match_expr(scrutinee, arms),
            ExprKind::For {
                pattern,
                iter,
                body,
            } => {
                let iter_ty = self.expr(iter);
                self.scopes.push(Vec::new());
                self.bind(pattern, element(&iter_ty));
                self.block(body);
                self.scopes.pop();
                Ty::unit()
            }
            ExprKind::While { cond, body } => {
                self.scopes.push(Vec::new());
                self.cond(cond, "a `while` condition");
                self.block(body);
                self.scopes.pop();
                Ty::unit()
            }
            ExprKind::Let { .. } => {
                self.cond(expr, "a condition");
                Ty::Bool
            }
            ExprKind::Range { start, end, .. } => {
                let start = start.as_ref().map(|e| self.expr(e));
                let end = end.as_ref().map(|e| self.expr(e));
                let elem = match (start, end) {
                    (Some(Ty::Int(None)), Some(end)) => end,
                    (Some(start), _) => start,
                    (None, Some(end)) => end,
                    (None, None) => Ty::Unknown,
                };
                Ty::Std("Range", vec![elem])
            }
        };
        (ty, false)
    }

    fn path_value(&mut self, path: &Path) -> Ty {
        let first = &path.segments[0].ident;
        if path.segments.len() == 1 {
            let name = first.name.as_str();
            if name == "self" {
                self.self_available(first.span);
                return Ty::Unknown;
            }
            if let Some(ty) = self.use_local(first) {
                return ty;
            }
        }
        if first.name == "Self" {
            self.self_path(path);
            return Ty::Unknown;
        }
        let idents: Vec<Ident> = path.segments.iter().map(|s| s.ident.clone()).collect();
        match self.cx.names.resolve_in(self.module, &idents, Ns::Value) {
            Err(diagnostic) => {
                self.error(diagnostic);
                Ty::Unknown
            }
            Ok(Some(Def::Variant(data, variant))) => {
                if self.cx.datas[data].variants[variant].fields.is_empty() {
                    data_ty(&self.cx.names, data)
                } else {
                    Ty::Unknown
                }
            }
            Ok(Some(Def::Algorithm(_) | Def::Std(_))) => Ty::Unknown,
            Ok(Some(def)) => {
                let what = match def {
                    Def::Data(_) => "a type",
                    Def::Component(_) => "a component",
                    Def::Module(_) => "a module",
                    _ => "the app",
                };
                let name = self.text_of_path(path);
                self.error(Diagnostic::new(
                    format!("expected a value, found {what} `{name}`"),
                    path.span,
                    format!("`{name}` is {what}"),
                ));
                Ty::Unknown
            }
            Ok(None) if path.segments.len() == 1 => match first.name.as_str() {
                "None" => Ty::Std("Option", vec![Ty::Unknown]),
                "Some" | "Ok" | "Err" | "Json" => Ty::Unknown,
                _ => {
                    let diagnostic = self.cannot_find(first);
                    self.error(diagnostic);
                    Ty::Unknown
                }
            },
            Ok(None) => {
                self.std_path(path);
                Ty::Unknown
            }
        }
    }

    /// `Self::name`: a function of the component.
    fn self_path(&mut self, path: &Path) {
        let first = &path.segments[0].ident;
        let Some(segment) = path.segments.get(1) else {
            self.error(Diagnostic::new(
                "`Self` is not a value",
                first.span,
                "a component is started, not made in code",
            ));
            return;
        };
        let name = &segment.ident;
        match self.comp() {
            Some(comp) if comp.function(&name.name).is_some() => {
                let target = comp.function(&name.name).expect("found").ast.name.span;
                self.refs.push((name.span, target));
            }
            Some(comp) => {
                let comp_name = comp.name().to_string();
                self.error(Diagnostic::new(
                    format!("`{comp_name}` has no function `{}`", name.name),
                    name.span,
                    format!("not a function of `{comp_name}`"),
                ));
            }
            None => self.error(Diagnostic::new(
                "`Self` names a component, and this is not in one",
                first.span,
                "a function at the top level of a module is called by its name",
            )),
        }
    }

    /// A path of several names that the program does not declare, which must
    /// start with a standard name, such as `String::from`.
    fn std_path(&mut self, path: &Path) {
        let first = &path.segments[0].ident;
        match first.name.as_str() {
            "Instant" | "SystemTime" => self.error(
                Diagnostic::new(format!("Inset has no `{}`", first.name), first.span, "the clock is an effect")
                    .with_help("use a capability the app grants, so the diagram shows it and a test can fake it"),
            ),
            name if STD_NAMES.contains(&name) || INTS.contains(&name) || FLOATS.contains(&name) => {}
            _ => self.error(cannot_find_type(path)),
        }
    }

    fn self_available(&mut self, span: Span) -> bool {
        if self.receiver().is_some() {
            return true;
        }
        let (label, help) = match (self.function, self.comp) {
            (None, _) => (
                "a `start` statement has no `self`".to_string(),
                "a `start` statement runs before the component handles anything; name its fields without `self.`",
            ),
            (Some(_), None) => (
                "a function at the top level of a module has no `self`".to_string(),
                "pass the function what it needs as parameters",
            ),
            (Some(sig), Some(_)) if sig.kind == FnKind::Algorithm => (
                format!("the algorithm `{}` takes no `self`", sig.name()),
                "an algorithm is pure, so it reaches no fields; pass it what it needs as parameters",
            ),
            (Some(sig), Some(_)) => (
                format!("`{}` takes no `self`", sig.name()),
                "take `&self` to read the component's fields, or `&mut self` to change them",
            ),
        };
        self.error(Diagnostic::new("`self` is not available here", span, label).with_help(help));
        false
    }

    fn cannot_find(&self, name: &Ident) -> Diagnostic {
        let diagnostic = Diagnostic::new(
            format!("cannot find `{}` in this scope", name.name),
            name.span,
            "not a variable, parameter or function here",
        );
        let Some(comp) = self.comp() else {
            return diagnostic;
        };
        let take_self = if self.receiver().is_none() && self.function.is_some() {
            ", and take `&self`"
        } else {
            ""
        };
        if comp.field(&name.name).is_some() || comp.child(&name.name).is_some() {
            diagnostic.with_help(format!(
                "`{0}` belongs to `{1}`: write `self.{0}`{take_self}",
                name.name,
                comp.name()
            ))
        } else if comp.function(&name.name).is_some() {
            diagnostic.with_help(format!(
                "call a function of `{}` as `Self::{}`",
                comp.name(),
                name.name
            ))
        } else {
            diagnostic
        }
    }

    fn field(&mut self, base: &Expr, name: &Ident) -> Ty {
        if let ExprKind::Path(path) = &base.kind
            && path.segments.len() == 1
            && path.segments[0].ident.name == "self"
        {
            return self.self_field(base.span, name);
        }
        let base_ty = self.expr(base);
        match (base_ty.deref(), name.name.parse::<usize>()) {
            (Ty::Tuple(elems), Ok(i)) if i < elems.len() => elems[i].clone(),
            (Ty::Std("Json", args), Ok(0)) => args[0].clone(),
            (Ty::Data { id, .. }, Err(_)) if !self.cx.datas[*id].is_enum => {
                self.struct_field(*id, name).unwrap_or(Ty::Unknown)
            }
            _ => Ty::Unknown,
        }
    }

    /// `self.name`: a field or a child of the component.
    fn self_field(&mut self, self_span: Span, name: &Ident) -> Ty {
        if !self.self_available(self_span) {
            return Ty::Unknown;
        }
        let Some(comp) = self.comp() else {
            return Ty::Unknown;
        };
        if let Some(field) = comp.field(&name.name) {
            self.refs.push((name.span, field.decl.name.span));
            if field.handed_down {
                let diagnostic = handed_down_use(comp.name(), &name.name, name.span);
                self.error(diagnostic);
                return Ty::Unknown;
            }
            return field.ty.clone();
        }
        if let Some(start) = comp.start_of(&name.name) {
            let ty = match self.cx.started(start) {
                Some(child) => Ty::Handle(child, self.cx.components[child].name().to_string()),
                None => Ty::Unknown,
            };
            if let Some(binding) = &start.binding {
                self.refs.push((name.span, binding.span));
            }
            return ty;
        }
        let comp_name = comp.name().to_string();
        self.error(
            Diagnostic::new(
                format!("`{comp_name}` has no field or child `{}`", name.name),
                name.span,
                format!("not a field or child of `{comp_name}`"),
            )
            .with_help(format!(
                "a component uses only what it is handed: declare `{0}` as a field of `{comp_name}`, \
                 and have whoever starts `{comp_name}` move a capability into it with `{0}: move ...`",
                name.name
            )),
        );
        Ty::Unknown
    }

    fn place(&mut self, place: &Place) -> Ty {
        let first = &place.parts[0];
        let mut ty = if first.name == "self" {
            match place.parts.get(1) {
                Some(field) => self.self_field(first.span, field),
                None => {
                    self.self_available(first.span);
                    Ty::Unknown
                }
            }
        } else {
            match self.use_local(first) {
                Some(ty) => ty,
                None => {
                    let diagnostic = self.cannot_find(first);
                    self.error(diagnostic);
                    Ty::Unknown
                }
            }
        };
        let skip = if first.name == "self" { 2 } else { 1 };
        if place.parts.len() > skip {
            ty = Ty::Unknown;
        }
        ty
    }

    fn check_assign_target(&mut self, target: &Expr) {
        let mut root = target;
        let mut field = None;
        // Walk down to the root of `self.a.b[i]`, remembering the first field.
        loop {
            match &root.kind {
                ExprKind::Field { base, name } => {
                    field = Some(name);
                    root = base;
                }
                ExprKind::Index { base, .. } => root = base,
                _ => break,
            }
        }
        let is_self = matches!(&root.kind, ExprKind::Path(p) if p.segments.len() == 1 && p.segments[0].ident.name == "self");
        if let (true, Some(field), Some(receiver)) = (is_self, field, self.receiver())
            && !receiver.mutable
        {
            self.error(
                Diagnostic::new(
                    format!(
                        "`{}` takes `&self`, so it cannot change `self.{}`",
                        self.fn_name(),
                        field.name
                    ),
                    target.span,
                    "this changes the component",
                )
                .with_help("take `&mut self` to change the component's fields"),
            );
        }
    }

    fn binary(&mut self, op: BinaryOp, lhs: &Expr, rhs: &Expr) -> Ty {
        use BinaryOp as B;
        match op {
            B::And | B::Or => {
                self.expect(lhs, &Ty::Bool, "an operand of `&&` and `||`");
                self.expect(rhs, &Ty::Bool, "an operand of `&&` and `||`");
                Ty::Bool
            }
            B::Eq | B::Ne | B::Lt | B::Le | B::Gt | B::Ge => {
                let left = self.expr(lhs);
                self.expr(rhs);
                let needs = if matches!(op, B::Eq | B::Ne) {
                    "PartialEq"
                } else {
                    "PartialOrd"
                };
                if let Ty::Data { id, name, .. } = left.deref()
                    && !self.cx.datas[*id].derives(needs)
                {
                    self.error(
                        Diagnostic::new(
                            format!("`{name}` cannot be compared"),
                            lhs.span,
                            format!("`{name}` does not derive `{needs}`"),
                        )
                        .with_help(format!("add `{needs}` to the `#[derive]` of `{name}`")),
                    );
                }
                Ty::Bool
            }
            _ => {
                let left = self.expr(lhs);
                let right = self.expr(rhs);
                match left {
                    Ty::Int(None) | Ty::Float(None) | Ty::Unknown => right,
                    Ty::Ref { inner, .. } => *inner,
                    left => left,
                }
            }
        }
    }

    fn return_expr(&mut self, span: Span, value: Option<&Expr>) {
        let Some(sig) = self.function.filter(|_| !self.in_closure) else {
            if let Some(value) = value {
                self.expr(value);
            }
            return;
        };
        match value {
            Some(value) => {
                self.expect(value, &sig.ret, &format!("the reply of `{}`", sig.name()));
            }
            None if !sig.ret.is_unit() && sig.ret.is_known() => {
                self.error(mismatch(
                    span,
                    &sig.ret,
                    &Ty::unit(),
                    &format!("the reply of `{}`", sig.name()),
                ));
            }
            None => {}
        }
    }

    fn struct_lit(&mut self, path: &Path, fields: &[FieldInit], rest: Option<&Expr>) -> Ty {
        let name = path.segments.last().map_or("", |s| s.ident.name.as_str());
        let special = match name {
            "HttpServer" | "Table" if path.segments.len() == 1 => Some(
                Diagnostic::new(
                    "only an app's `grant` creates a capability",
                    path.span,
                    "this would make a capability no grant declares",
                )
                .with_help(
                    "declare a field for the capability, and have the component's parent move one in",
                ),
            ),
            "Handle" | "Outlet" if path.segments.len() == 1 => Some(Diagnostic::new(
                format!("a `{name}` cannot be made in code"),
                path.span,
                "`start` makes handles, and connects outlets",
            )),
            _ => None,
        };
        let data = match special {
            Some(diagnostic) => {
                self.error(diagnostic);
                None
            }
            None => self.data_of_type_path(path),
        };
        let mut named = Vec::new();
        for field in fields {
            let expected = match data {
                Some(data) => {
                    if named.contains(&field.name.name.as_str()) {
                        self.error(Diagnostic::new(
                            format!("the field `{}` is given twice", field.name.name),
                            field.name.span,
                            "this repeats an earlier field",
                        ));
                    }
                    named.push(field.name.name.as_str());
                    self.struct_field(data, &field.name)
                }
                None => None,
            }
            .unwrap_or(Ty::Unknown);
            let what = format!("the field `{}`", field.name.name);
            match &field.value {
                Some(value) => {
                    self.expect(value, &expected, &what);
                }
                None => {
                    let found = self.path_value(&Path {
                        segments: vec![PathSegment {
                            ident: field.name.clone(),
                            generics: Vec::new(),
                        }],
                        span: field.name.span,
                    });
                    if !fits(&expected, &found) {
                        self.error(mismatch(field.name.span, &expected, &found, &what));
                    }
                }
            }
        }
        let Some(data) = data else {
            if let Some(rest) = rest {
                self.expr(rest);
            }
            return Ty::Unknown;
        };
        let ty = data_ty(&self.cx.names, data);
        match rest {
            Some(rest) => {
                self.expect(rest, &ty, "the rest of the struct");
            }
            None => self.missing_fields(data, &named, path.span, "give each field a value"),
        }
        ty
    }

    /// The condition of an `if` or `while`, binding the names of an
    /// `if let` or `while let` in the current scope.
    fn cond(&mut self, cond: &Expr, what: &str) {
        match &cond.kind {
            ExprKind::Let { pattern, expr } => {
                let ty = self.expr(expr);
                self.bind(pattern, ty);
            }
            _ => {
                self.expect(cond, &Ty::Bool, what);
            }
        }
    }

    fn match_expr(&mut self, scrutinee: &Expr, arms: &[MatchArm]) -> Ty {
        let matched = self.expr(scrutinee);
        let mut result = Ty::Never;
        for arm in arms {
            self.scopes.push(Vec::new());
            self.bind(&arm.pattern, matched.clone());
            if let Some(guard) = &arm.guard {
                self.expect(guard, &Ty::Bool, "a `match` guard");
            }
            let ty = self.expr(&arm.body);
            self.scopes.pop();
            if result == Ty::Never {
                result = ty;
            }
        }
        self.check_covered(scrutinee, &matched, arms);
        result
    }

    /// Reports a `match` that leaves a case out, when that is certain: an
    /// enum variant, `Some`, `None`, `Ok`, `Err`, `true` or `false` that no
    /// arm names, or a number or text matched with no arm for the rest.
    fn check_covered(&mut self, scrutinee: &Expr, matched: &Ty, arms: &[MatchArm]) {
        if arms
            .iter()
            .any(|a| a.guard.is_none() && irrefutable(&a.pattern))
        {
            return;
        }
        let mut mentioned = Vec::new();
        for arm in arms {
            mentioned_names(&arm.pattern, &mut mentioned);
        }
        let cases: Vec<String> = match matched.deref() {
            Ty::Data { id, name, .. } if self.cx.datas[*id].is_enum => self.cx.datas[*id]
                .variants
                .iter()
                .map(|v| format!("{name}::{}", v.name.name))
                .collect(),
            Ty::Std("Option", _) => vec!["None".into(), "Some(_)".into()],
            Ty::Std("Result", _) => vec!["Ok(_)".into(), "Err(_)".into()],
            Ty::Bool => vec!["true".into(), "false".into()],
            Ty::Int(_) | Ty::Float(_) | Ty::String | Ty::Str | Ty::Char | Ty::Decimal => {
                vec!["_".into()]
            }
            _ => return,
        };
        let missing: Vec<&str> = cases
            .iter()
            .map(String::as_str)
            .filter(|case| {
                let last = case
                    .rsplit("::")
                    .next()
                    .unwrap_or(case)
                    .trim_end_matches("(_)");
                *case == "_" || !mentioned.iter().any(|m| m == last)
            })
            .collect();
        if missing.is_empty() {
            return;
        }
        let (label, help) = if missing == ["_"] {
            (
                format!(
                    "a `{}` has more values than these arms name",
                    matched.deref()
                ),
                "add a `_ =>` arm for every other value".to_string(),
            )
        } else {
            (
                format!("no arm for {}", names(missing.iter().copied())),
                format!(
                    "add an arm for {}, or a `_ =>` arm for every other case",
                    names(missing.iter().copied())
                ),
            )
        };
        self.error(
            Diagnostic::new(
                "this `match` does not cover every case",
                scrutinee.span,
                label,
            )
            .with_help(help),
        );
    }

    fn closure(&mut self, closure: &Closure) {
        self.scopes.push(Vec::new());
        for param in &closure.params {
            let ty = param.ty.as_ref().map_or(Ty::Unknown, |t| self.resolve(t));
            self.bind(&param.pattern, ty);
        }
        let outer = self.in_closure;
        self.in_closure = true;
        let ret = closure.ret.as_ref().map(|t| self.resolve(t));
        match ret {
            Some(ret) => {
                self.expect(&closure.body, &ret, "the closure's value");
            }
            None => {
                self.expr(&closure.body);
            }
        }
        self.in_closure = outer;
        self.scopes.pop();
    }

    fn await_expr(&mut self, span: Span, inner: &Expr) -> Ty {
        let is_async = self.function.is_some_and(|f| f.ast.is_async);
        if self.in_closure {
            self.error(
                Diagnostic::new(
                    "a closure cannot `await`",
                    span,
                    "this waits inside a closure",
                )
                .with_help(
                    "await in the function's body, outside the closure, so the flowchart shows \
                         the call as a step of its own",
                ),
            );
        } else if !is_async {
            let help = match self.function {
                Some(sig) if sig.kind == FnKind::Algorithm => {
                    "an algorithm is pure, so it has nothing to wait for".to_string()
                }
                Some(sig) => format!("make the handler `async fn {}`", sig.name()),
                None => "a `start` statement cannot wait".to_string(),
            };
            self.error(
                Diagnostic::new(
                    "`.await` is only allowed in an `async fn`",
                    span,
                    format!("`{}` is not `async`", self.fn_name()),
                )
                .with_help(help),
            );
        }
        let (ty, effect) = self.expr_use(inner, Use::Awaited);
        if effect { ty } else { Ty::Unknown }
    }

    fn send(&mut self, span: Span, inner: &Expr) {
        if self.in_closure {
            self.error(
                Diagnostic::new(
                    "a closure cannot `send`",
                    span,
                    "this sends from inside a closure",
                )
                .with_help(
                    "send from the function's body, outside the closure, so the diagram shows \
                         the message as a step of its own",
                ),
            );
        }
        let (_, effect) = self.expr_use(inner, Use::Sent);
        if !effect {
            self.error(
                Diagnostic::new(
                    "`send` takes a message or an outlet's `emit`",
                    inner.span,
                    "this is not a message",
                )
                .with_help(
                    "write `send self.child.message(value)` or `send self.outlet.emit(value)`",
                ),
            );
        }
    }

    // ---- Calls -------------------------------------------------------------

    fn call(&mut self, span: Span, callee: &Expr, args: &[Expr]) -> Ty {
        let ExprKind::Path(path) = &callee.kind else {
            self.expr(callee);
            self.args(args);
            return Ty::Unknown;
        };
        let names: Vec<&str> = path
            .segments
            .iter()
            .map(|s| s.ident.name.as_str())
            .collect();
        let idents: Vec<Ident> = path.segments.iter().map(|s| s.ident.clone()).collect();
        let local = matches!(names.as_slice(), [name] if self.local(name).is_some());
        let resolved = if local || names[0] == "Self" {
            Ok(None)
        } else {
            self.cx.names.resolve_in(self.module, &idents, Ns::Value)
        };
        let sig = match (names.as_slice(), resolved) {
            (["Self", name], _) => self.comp().and_then(|c| c.function(name)),
            (_, Ok(Some(Def::Algorithm(a)))) => Some(&self.cx.algorithms[a]),
            (_, Ok(Some(Def::Variant(data, variant)))) => {
                return self.variant_call(span, data, variant, args);
            }
            (_, Err(diagnostic)) => {
                self.error(diagnostic);
                self.args(args);
                return Ty::Unknown;
            }
            _ => None,
        };
        if let Some(sig) = sig {
            if names[0] == "Self" {
                let last = &path.segments[path.segments.len() - 1].ident;
                self.refs.push((last.span, sig.ast.name.span));
            }
            if sig.kind != FnKind::Algorithm {
                let kind = if sig.kind == FnKind::Route {
                    "a route handler"
                } else {
                    "a message"
                };
                self.error(
                    Diagnostic::new(
                        format!("`{}` is {kind}, so it is not called directly", sig.name()),
                        callee.span,
                        "only an algorithm is called by name",
                    )
                    .with_help("send a message through a handle, or move the shared code into an algorithm"),
                );
                self.args(args);
                return Ty::Unknown;
            }
            if sig.params.len() != args.len() {
                self.error(arity(span, sig.name(), sig.params.len(), args.len()));
                self.args(args);
            } else {
                for (arg, param) in args.iter().zip(&sig.params) {
                    self.expect(arg, param, &format!("this parameter of `{}`", sig.name()));
                }
            }
            return sig.ret.clone();
        }
        match names.as_slice() {
            ["Some"] if args.len() == 1 => Ty::Std("Option", vec![self.expr(&args[0])]),
            ["Ok"] if args.len() == 1 => Ty::Std("Result", vec![self.expr(&args[0]), Ty::Unknown]),
            ["Err"] if args.len() == 1 => Ty::Std("Result", vec![Ty::Unknown, self.expr(&args[0])]),
            ["Json"] if args.len() == 1 => Ty::Std("Json", vec![self.expr(&args[0])]),
            _ => {
                self.expr(callee);
                self.args(args);
                match names.as_slice() {
                    ["String", "new" | "from"] => Ty::String,
                    ["Vec", "new"] => Ty::Std("Vec", vec![Ty::Unknown]),
                    _ => Ty::Unknown,
                }
            }
        }
    }

    /// `Enum::Variant(values)`, which makes a value of the enum.
    fn variant_call(&mut self, span: Span, data: usize, variant: usize, args: &[Expr]) -> Ty {
        let fields = self.cx.datas[data].variants[variant].fields.clone();
        let name = format!(
            "{}::{}",
            self.cx.datas[data].name.name, self.cx.datas[data].variants[variant].name.name
        );
        if fields.len() != args.len() {
            self.error(arity(span, &name, fields.len(), args.len()));
            self.args(args);
        } else {
            for (arg, ty) in args.iter().zip(&fields) {
                self.expect(arg, ty, &format!("this value of `{name}`"));
            }
        }
        data_ty(&self.cx.names, data)
    }

    fn args(&mut self, args: &[Expr]) {
        for arg in args {
            self.expr(arg);
        }
    }

    fn method_call(
        &mut self,
        span: Span,
        receiver: &Expr,
        method: &Ident,
        args: &[Expr],
        usage: Use,
    ) -> (Ty, bool) {
        let receiver_ty = self.expr(receiver);
        match receiver_ty.deref().clone() {
            Ty::Handle(component, _) => (
                self.message_call(span, component, method, args, usage),
                true,
            ),
            Ty::Outlet(ty) => {
                self.emit(span, receiver, &ty, method, args, usage);
                (Ty::unit(), true)
            }
            Ty::HttpServer => {
                self.error(
                    Diagnostic::new(
                        "`HttpServer` has no methods",
                        method.span,
                        "requests arrive at route handlers",
                    )
                    .with_help("handle requests with a route attribute naming this field in `via`"),
                );
                self.args(args);
                (Ty::Unknown, true)
            }
            Ty::Table(key, value) => (
                self.table_call(span, method, args, &key, &value, usage),
                true,
            ),
            other => {
                self.args(args);
                if method.name == "clone"
                    && let Ty::Data {
                        name, clone: false, ..
                    } = other.deref()
                {
                    self.error(
                        Diagnostic::new(
                            format!("`{name}` cannot be cloned"),
                            method.span,
                            format!("`{name}` does not derive `Clone`"),
                        )
                        .with_help(format!("add `Clone` to the `#[derive]` of `{name}`")),
                    );
                }
                let ty = match (method.name.as_str(), other.deref()) {
                    ("to_string", _) => Ty::String,
                    ("to_owned", Ty::Str) => Ty::String,
                    ("clone", Ty::Str | Ty::Slice(_)) => other.clone(),
                    ("clone", inner) => inner.clone(),
                    ("len", _) => Ty::Int(Some("usize")),
                    ("is_empty", _) => Ty::Bool,
                    _ => Ty::Unknown,
                };
                (ty, false)
            }
        }
    }

    fn message_call(
        &mut self,
        span: Span,
        component: usize,
        method: &Ident,
        args: &[Expr],
        usage: Use,
    ) -> Ty {
        let comp = Some(&self.cx.components[component]);
        let component = self.cx.components[component].name();
        let sig = comp.and_then(|c| c.function(&method.name));
        let sig = match sig {
            Some(sig) if sig.kind == FnKind::Message => sig,
            other => {
                let messages: Vec<&str> = comp
                    .map(|c| {
                        c.functions
                            .iter()
                            .filter(|f| f.kind == FnKind::Message)
                            .map(|f| f.name())
                            .collect()
                    })
                    .unwrap_or_default();
                let label = match other {
                    Some(sig) if sig.kind == FnKind::Route => {
                        format!("`{}` is a route handler", method.name)
                    }
                    Some(_) => format!("`{}` is an algorithm", method.name),
                    None => format!("`{component}` has no function `{}`", method.name),
                };
                let help = if messages.is_empty() {
                    format!("`{component}` has no `#[message]` functions")
                } else {
                    format!("the messages of `{component}` are {}", names(messages))
                };
                self.error(
                    Diagnostic::new(
                        format!("`{component}` has no message `{}`", method.name),
                        method.span,
                        label,
                    )
                    .with_help(help),
                );
                self.args(args);
                return Ty::Unknown;
            }
        };
        self.refs.push((method.span, sig.ast.name.span));
        let full = format!("{component}::{}", method.name);
        if sig.params.len() != args.len() {
            self.error(arity(span, &full, sig.params.len(), args.len()));
            self.args(args);
        } else {
            let into = format!("into `{component}`");
            for (arg, param) in args.iter().zip(&sig.params) {
                self.boundary_arg(arg, param, usage == Use::Sent, &into);
            }
        }
        match usage {
            Use::Value => {
                self.error(
                    Diagnostic::new(
                        "a message is awaited or sent with `send`",
                        span,
                        "this message is neither awaited nor sent",
                    )
                    .with_help("add `.await` to wait for the reply, or write `send` before it if it replies with nothing"),
                );
                Ty::Unknown
            }
            Use::Sent if !sig.ret.is_unit() && sig.ret.is_known() => {
                self.error(
                    Diagnostic::new(
                        format!("`send` does not wait, so it cannot take the reply of `{full}`"),
                        span,
                        format!("this message replies with a `{}`", sig.ret),
                    )
                    .with_help("await the message instead, with `.await`"),
                );
                Ty::unit()
            }
            Use::Sent => Ty::unit(),
            Use::Awaited => sig.ret.clone(),
        }
    }

    fn emit(
        &mut self,
        span: Span,
        receiver: &Expr,
        ty: &Ty,
        method: &Ident,
        args: &[Expr],
        usage: Use,
    ) {
        let outlet = match &receiver.kind {
            ExprKind::Field { name, .. } => format!("`{}`", name.name),
            _ => "the outlet".to_string(),
        };
        if method.name != "emit" {
            self.error(Diagnostic::new(
                format!("an outlet has one method, `emit`, and no `{}`", method.name),
                method.span,
                "not a method of an outlet",
            ));
            self.args(args);
            return;
        }
        match args {
            [arg] => self.boundary_arg(arg, ty, true, &format!("out through {outlet}")),
            _ => {
                self.error(arity(span, "emit", 1, args.len()));
                self.args(args);
            }
        }
        if usage != Use::Sent {
            self.error(
                Diagnostic::new(
                    "`emit` is used with `send`",
                    span,
                    "an outlet has nothing to await",
                )
                .with_help(format!(
                    "write `send` before it: `send self.{}.emit(...)`",
                    outlet.trim_matches('`')
                )),
            );
        }
    }

    fn table_call(
        &mut self,
        span: Span,
        method: &Ident,
        args: &[Expr],
        key: &Ty,
        value: &Ty,
        usage: Use,
    ) -> Ty {
        let (params, ret, changes): (Vec<Option<&Ty>>, Ty, bool) = match method.name.as_str() {
            // The key may be any borrowed form of `K`, as with `HashMap::get`.
            "get" => (vec![None], Ty::Std("Option", vec![value.clone()]), false),
            "insert" => (vec![Some(key), Some(value)], Ty::unit(), true),
            "remove" => (vec![None], Ty::Std("Option", vec![value.clone()]), true),
            "entries" => (
                vec![],
                Ty::Std("Vec", vec![Ty::Tuple(vec![key.clone(), value.clone()])]),
                false,
            ),
            _ => {
                self.error(
                    Diagnostic::new(
                        format!("`Table` has no method `{}`", method.name),
                        method.span,
                        "not a method of a table",
                    )
                    .with_help("a table's methods are `get`, `insert`, `remove` and `entries`"),
                );
                self.args(args);
                return Ty::Unknown;
            }
        };
        if params.len() != args.len() {
            self.error(arity(span, &method.name, params.len(), args.len()));
            self.args(args);
        } else {
            for (arg, param) in args.iter().zip(params) {
                match param {
                    Some(param) => {
                        self.expect(arg, param, &format!("this argument of `{}`", method.name));
                    }
                    None => {
                        self.expr(arg);
                    }
                }
            }
        }
        if changes && self.receiver().is_some_and(|r| !r.mutable) {
            self.error(
                Diagnostic::new(
                    format!(
                        "`{}` changes the table, so `{}` takes `&mut self`",
                        method.name,
                        self.fn_name()
                    ),
                    method.span,
                    "this changes a capability the component holds",
                )
                .with_help("take `&mut self`"),
            );
        }
        match usage {
            Use::Awaited => ret,
            Use::Value => {
                self.error(
                    Diagnostic::new(
                        "a capability call is awaited",
                        span,
                        "this call is not awaited",
                    )
                    .with_help("add `.await`"),
                );
                Ty::Unknown
            }
            Use::Sent => {
                self.error(
                    Diagnostic::new(
                        "`send` sends a message, and this is a capability call",
                        span,
                        "a capability call is awaited",
                    )
                    .with_help("remove `send` and add `.await`"),
                );
                Ty::Unknown
            }
        }
    }

    /// Checks a value crossing into another component in a message or an
    /// outlet, against what syntax.md's ownership table allows.
    fn boundary_arg(&mut self, arg: &Expr, param: &Ty, sent: bool, into: &str) {
        let ty = match &arg.kind {
            ExprKind::Ref {
                mutable: true,
                operand,
            } => {
                self.expr(operand);
                self.error(mutable_borrow(arg.span, into));
                return;
            }
            ExprKind::Ref {
                mutable: false,
                operand,
            } => {
                let inner = self.expr(operand);
                if sent {
                    self.error(
                        Diagnostic::new(
                            "`send` does not wait, so it cannot lend a value",
                            arg.span,
                            "the sender may change or drop this before it arrives",
                        )
                        .with_help("move the value with `move`, or send a copy"),
                    );
                } else if inner.is_clone() == Some(false) {
                    self.error(
                        Diagnostic::new(
                            format!("a lent `{inner}` cannot be copied to the receiver"),
                            arg.span,
                            "a value lent in a message reaches the receiver as a copy",
                        )
                        .with_help("lend a type that is `Clone`, or move the value"),
                    );
                }
                Ty::Ref {
                    mutable: false,
                    inner: Box::new(inner),
                }
            }
            ExprKind::Move(place) => {
                let ty = self.place(place);
                if place.parts[0].name == "self" {
                    self.error(
                        Diagnostic::new(
                            "a field cannot be moved out of the component",
                            arg.span,
                            "the component keeps its fields",
                        )
                        .with_help("send a copy with `.clone()`, or move a local value"),
                    );
                }
                ty
            }
            _ => {
                let ty = self.expr(arg);
                if let Some(name) = place_text(arg)
                    && ty.is_copy() == Some(false)
                {
                    self.error(without_move(&name, arg.span, into, !sent));
                }
                ty
            }
        };
        if !fits(param, &ty) {
            self.error(mismatch(arg.span, param, &ty, "the receiver's parameter"));
        }
    }

    fn macro_call(&mut self, name: &Path, args: &[Expr]) -> Ty {
        let macro_name = name.segments.last().map_or("", |s| s.ident.name.as_str());
        let span = name.span;
        if name.segments.len() == 1
            && matches!(
                macro_name,
                "println" | "print" | "eprintln" | "eprint" | "dbg"
            )
        {
            self.error(
                Diagnostic::new(
                    format!("Inset has no `{macro_name}!`"),
                    span,
                    "printing is an effect",
                )
                .with_help("send output through a capability the app grants"),
            );
            self.args(args);
            return Ty::unit();
        }
        if name.segments.len() != 1 || !MACROS.contains(&macro_name) {
            self.error(
                Diagnostic::new(
                    format!("unknown macro `{macro_name}!`"),
                    span,
                    "Inset does not keep this macro",
                )
                .with_help(format!(
                    "the macros Inset keeps are {}",
                    names(
                        MACROS
                            .iter()
                            .map(|m| format!("{m}!"))
                            .collect::<Vec<_>>()
                            .iter()
                            .map(String::as_str)
                    )
                )),
            );
            self.args(args);
            return Ty::Unknown;
        }
        match macro_name {
            "format" | "write" | "panic" | "assert" => {
                let format_at = if macro_name == "format" || macro_name == "panic" {
                    0
                } else {
                    1
                };
                let mut named = Vec::new();
                for (i, arg) in args.iter().enumerate() {
                    if i == format_at {
                        continue;
                    }
                    match &arg.kind {
                        ExprKind::Assign { op: None, lhs, rhs } if i > format_at => {
                            if let ExprKind::Path(p) = &lhs.kind {
                                named.push(p.segments[0].ident.name.clone());
                            }
                            self.expr(rhs);
                        }
                        _ => {
                            self.expr(arg);
                        }
                    }
                }
                match args.get(format_at) {
                    Some(Expr {
                        kind:
                            ExprKind::Lit(Lit {
                                kind: LitKind::Str(text),
                                span,
                            }),
                        ..
                    }) => self.format_captures(*span, text, &named),
                    Some(other) => {
                        self.expr(other);
                    }
                    None => {}
                }
                match macro_name {
                    "format" => Ty::String,
                    "panic" => Ty::Never,
                    "assert" => Ty::unit(),
                    _ => Ty::Unknown,
                }
            }
            "vec" => {
                let elems: Vec<Ty> = args.iter().map(|a| self.expr(a)).collect();
                Ty::Std("Vec", vec![elems.into_iter().next().unwrap_or(Ty::Unknown)])
            }
            "matches" => {
                // The pattern is not an expression, so only the value is checked.
                if let Some(value) = args.first() {
                    self.expr(value);
                }
                Ty::Bool
            }
            _ => {
                self.decimal_literal(span, args);
                Ty::Decimal
            }
        }
    }

    /// Checks that `dec!` holds one number that a `Decimal` can hold
    /// exactly, as `rust_decimal`'s macro requires.
    fn decimal_literal(&mut self, span: Span, args: &[Expr]) {
        let text = match args {
            [arg] => match &arg.kind {
                ExprKind::Lit(lit) => Some((lit, arg.span)),
                ExprKind::Unary {
                    op: UnaryOp::Neg,
                    operand,
                } => match &operand.kind {
                    ExprKind::Lit(lit) => Some((lit, arg.span)),
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        };
        let number = text.and_then(|(lit, span)| match &lit.kind {
            LitKind::Int(text) | LitKind::Float(text) => Some((text, span)),
            _ => None,
        });
        let Some((text, at)) = number else {
            self.error(
                Diagnostic::new("`dec!` takes one number", span, "not one number")
                    .with_help("write the number in it, such as `dec!(0.10)`"),
            );
            return;
        };
        if let Err(diagnostic) = crate::values::decimal(text) {
            self.error(diagnostic(at));
        }
    }

    /// Checks the variables a format string names, such as `{name}`.
    fn format_captures(&mut self, span: Span, text: &str, named: &[String]) {
        // A literal written without escapes has each character of `text` in
        // the source, so a name in it can be pointed at.
        let plain = span.end - span.start == text.len() + 2;
        let mut at = 0;
        while let Some(open) = text[at..].find(['{', '}']).map(|i| at + i) {
            let brace = text.as_bytes()[open];
            at = open + 1;
            if text.as_bytes().get(at) == Some(&brace) {
                at += 1;
                continue;
            }
            if brace == b'}' {
                continue;
            }
            let close = text[at..].find('}').map_or(text.len(), |i| at + i);
            let inside = &text[at..close];
            let name = inside.split(':').next().unwrap_or("");
            let name_at = at + (name.len() - name.trim_start().len());
            let name = name.trim();
            at = close;
            let is_name = name.starts_with(|c: char| c.is_alphabetic() || c == '_')
                && name.chars().all(|c| c.is_alphanumeric() || c == '_');
            if !is_name || named.iter().any(|n| n == name) {
                continue;
            }
            if plain {
                let start = span.start + 1 + name_at;
                let ident = Ident::new(name, Span::new(start, start + name.len()));
                if self.use_local(&ident).is_some() {
                    continue;
                }
            } else if self.local(name).is_some() {
                continue;
            }
            self.error(Diagnostic::new(
                format!("cannot find `{name}` in this scope"),
                span,
                format!("`{{{name}}}` in this format string names no variable"),
            ));
        }
    }
}

fn lit_ty(lit: &Lit) -> Ty {
    match &lit.kind {
        LitKind::Bool(_) => Ty::Bool,
        LitKind::Char(_) => Ty::Char,
        LitKind::Str(_) => Ty::Ref {
            mutable: false,
            inner: Box::new(Ty::Str),
        },
        LitKind::Int(text) => Ty::Int(INTS.iter().find(|s| text.ends_with(**s)).copied()),
        LitKind::Float(text) => Ty::Float(FLOATS.iter().find(|s| text.ends_with(**s)).copied()),
    }
}

/// The text of a variable or field, such as `order` or `self.orders`, or
/// `None` if the expression is neither.
fn place_text(expr: &Expr) -> Option<String> {
    match &expr.kind {
        ExprKind::Path(path) if path.segments.len() == 1 => {
            Some(path.segments[0].ident.name.clone())
        }
        ExprKind::Field { base, name } => Some(format!("{}.{}", place_text(base)?, name.name)),
        _ => None,
    }
}

fn arity(span: Span, name: &str, expected: usize, found: usize) -> Diagnostic {
    let plural = |n: usize| if n == 1 { "" } else { "s" };
    Diagnostic::new(
        format!(
            "`{name}` takes {expected} argument{}, but {found} {} given",
            plural(expected),
            if found == 1 { "is" } else { "are" }
        ),
        span,
        format!("expected {expected} argument{}", plural(expected)),
    )
}

fn cannot_find_type(path: &Path) -> Diagnostic {
    let name = path
        .segments
        .iter()
        .map(|s| s.ident.name.as_str())
        .collect::<Vec<_>>()
        .join("::");
    Diagnostic::new(
        format!("cannot find `{name}`"),
        path.span,
        "not a type or function this program knows",
    )
    .with_help("declaring `struct` and `enum` types is not supported yet")
}

/// The type of each element a `for` loop takes from a value of type `ty`.
fn element(ty: &Ty) -> Ty {
    match ty {
        Ty::Std("Vec" | "VecDeque" | "HashSet" | "BTreeSet" | "Option" | "Range", args) => {
            args[0].clone()
        }
        Ty::Std("HashMap" | "BTreeMap", args) => Ty::Tuple(args.clone()),
        Ty::Ref { mutable, inner } => {
            let by_ref = |t: Ty| {
                if t.is_known() {
                    Ty::Ref {
                        mutable: *mutable,
                        inner: Box::new(t),
                    }
                } else {
                    Ty::Unknown
                }
            };
            match &**inner {
                Ty::Std("Vec" | "VecDeque" | "HashSet" | "BTreeSet" | "Option", args) => {
                    by_ref(args[0].clone())
                }
                Ty::Slice(elem) => by_ref((**elem).clone()),
                Ty::Std("HashMap" | "BTreeMap", args) => {
                    Ty::Tuple(vec![by_ref(args[0].clone()), by_ref(args[1].clone())])
                }
                _ => Ty::Unknown,
            }
        }
        _ => Ty::Unknown,
    }
}

/// Whether a pattern matches every value, so its arm leaves no case out.
fn irrefutable(pattern: &Pattern) -> bool {
    match &pattern.kind {
        PatternKind::Wild => true,
        PatternKind::Binding { name, .. } => name.name != "None",
        PatternKind::Tuple(elems) => elems.iter().all(irrefutable),
        PatternKind::Or(alternatives) => alternatives.iter().any(irrefutable),
        _ => false,
    }
}

/// The last name of each variant, literal or constructor a pattern names
/// at its top level: `Some`, `NotFound` or `true`.
fn mentioned_names(pattern: &Pattern, out: &mut Vec<String>) {
    match &pattern.kind {
        PatternKind::TupleStruct { path, .. }
        | PatternKind::Path(path)
        | PatternKind::Struct { path, .. } => {
            if let Some(last) = path.segments.last() {
                out.push(last.ident.name.clone());
            }
        }
        PatternKind::Binding { name, .. } if name.name == "None" => out.push("None".into()),
        PatternKind::Lit(Lit {
            kind: LitKind::Bool(b),
            ..
        }) => out.push(b.to_string()),
        PatternKind::Or(alternatives) => {
            for alternative in alternatives {
                mentioned_names(alternative, out);
            }
        }
        _ => {}
    }
}
