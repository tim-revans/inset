//! Function bodies: names, types, `self`, closures, messages, outlets,
//! capability calls and values crossing component boundaries.

use inset_syntax::ast::*;
use inset_syntax::{Diagnostic, Ident, Span};

use crate::items::handed_down_use;
use crate::types::{FLOATS, INTS, Resolver, Ty, fits};
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
    component_names: Vec<&'a str>,
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
        Body {
            cx,
            component_names: cx.component_names(),
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
            Some(Stmt::Expr { expr, semi: false }) => Some(expr),
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
            components: &self.component_names,
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
        match &pattern.kind {
            PatternKind::Wild | PatternKind::Lit(_) | PatternKind::Path(_) => {}
            PatternKind::Binding { name, .. } => {
                if name.name != "None" {
                    self.locals.push((name.clone(), ty.clone()));
                    self.scopes.last_mut().expect("a scope").push(Local {
                        name: name.name.clone(),
                        ty,
                        span: name.span,
                    });
                }
            }
            PatternKind::Tuple(elems) => {
                let tys = match ty {
                    Ty::Tuple(tys) if tys.len() == elems.len() => tys,
                    _ => vec![Ty::Unknown; elems.len()],
                };
                for (elem, ty) in elems.iter().zip(tys) {
                    self.bind(elem, ty);
                }
            }
            PatternKind::TupleStruct { path, elems } => {
                let name = path.segments.last().map_or("", |s| s.ident.name.as_str());
                let inner = match (name, &ty) {
                    ("Some", Ty::Std("Option", args)) | ("Ok", Ty::Std("Result", args)) => {
                        args[0].clone()
                    }
                    ("Err", Ty::Std("Result", args)) => args[1].clone(),
                    ("Some" | "Ok" | "Err", _) => Ty::Unknown,
                    _ => {
                        self.error(cannot_find_type(path));
                        Ty::Unknown
                    }
                };
                for elem in elems {
                    self.bind(
                        elem,
                        if elems.len() == 1 {
                            inner.clone()
                        } else {
                            Ty::Unknown
                        },
                    );
                }
            }
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
                Stmt::Expr { expr, semi } => {
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
                self.struct_lit(path);
                for field in fields {
                    match &field.value {
                        Some(value) => {
                            self.expr(value);
                        }
                        None => {
                            self.path_value(&Path {
                                segments: vec![PathSegment {
                                    ident: field.name.clone(),
                                    generics: Vec::new(),
                                }],
                                span: field.name.span,
                            });
                        }
                    }
                }
                if let Some(rest) = rest {
                    self.expr(rest);
                }
                Ty::Unknown
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
                self.expect(cond, &Ty::Bool, "an `if` condition");
                let then_ty = self.block(then_block);
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
            if let Some(algorithm) = self.cx.algorithms.iter().find(|a| a.name() == name) {
                self.refs.push((first.span, algorithm.ast.name.span));
                return Ty::Unknown;
            }
            return match name {
                "None" => Ty::Std("Option", vec![Ty::Unknown]),
                "Some" | "Ok" | "Err" => Ty::Unknown,
                _ => {
                    let diagnostic = self.cannot_find(first);
                    self.error(diagnostic);
                    Ty::Unknown
                }
            };
        }
        self.path_prefix(path);
        Ty::Unknown
    }

    /// Checks what a path of several segments starts with.
    fn path_prefix(&mut self, path: &Path) {
        let first = &path.segments[0].ident;
        match first.name.as_str() {
            "Self" => {
                let name = &path.segments[1].ident;
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
            "std" | "core" => {
                let module = path.segments[1].ident.name.as_str();
                let effect = match module {
                    "fs" => Some(("file access is an effect", "use a capability the app grants")),
                    "io" => Some(("input and output are effects", "use a capability the app grants")),
                    "net" => Some(("networking is an effect", "use a capability the app grants, such as `HttpServer`")),
                    "process" => Some(("running a process is an effect", "use a capability the app grants")),
                    "env" => Some(("environment variables are an effect", "declare a `setting` in the app")),
                    "time" => Some(("the clock is an effect", "use a capability the app grants, so tests can fake the clock")),
                    "thread" => Some(("threads are concurrency the flowchart cannot draw", "use `fork`")),
                    _ => None,
                };
                if let Some((why, help)) = effect {
                    let span = first.span.to(path.segments[1].ident.span);
                    self.error(
                        Diagnostic::new(format!("Inset has no `{}::{module}`", first.name), span, why)
                            .with_help(format!("{help}, so the diagram shows it and a test can fake it")),
                    );
                }
            }
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
        if let Some(child) = comp.child(&name.name) {
            let ty = Ty::Handle(child.name.clone());
            if let Some(binding) = comp.child_binding(&name.name) {
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
                self.expr(lhs);
                self.expr(rhs);
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

    fn struct_lit(&mut self, path: &Path) {
        let name = path.segments.last().map_or("", |s| s.ident.name.as_str());
        let diagnostic = match name {
            "HttpServer" | "Table" => Diagnostic::new(
                "only an app's `grant` creates a capability",
                path.span,
                "this would make a capability no grant declares",
            )
            .with_help(
                "declare a field for the capability, and have the component's parent move one in",
            ),
            "Handle" | "Outlet" => Diagnostic::new(
                format!("a `{name}` cannot be made in code"),
                path.span,
                "`start` makes handles, and connects outlets",
            ),
            _ => cannot_find_type(path),
        };
        self.error(diagnostic);
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
        let sig = match names.as_slice() {
            [name] if self.local(name).is_none() => {
                self.cx.algorithms.iter().find(|a| a.name() == *name)
            }
            ["Self", name] => self.comp().and_then(|c| c.function(name)),
            _ => None,
        };
        if let Some(sig) = sig {
            let last = &path.segments[path.segments.len() - 1].ident;
            self.refs.push((last.span, sig.ast.name.span));
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
            Ty::Handle(component) => (
                self.message_call(span, &component, method, args, usage),
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
        component: &str,
        method: &Ident,
        args: &[Expr],
        usage: Use,
    ) -> Ty {
        let comp = self.cx.component(component).map(|i| &self.cx.components[i]);
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
            _ => Ty::Decimal,
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
