//! The app, components, their fields and functions, routes and `failure`
//! blocks: everything checked from declarations alone.

use inset_syntax::ast::*;
use inset_syntax::{Diagnostic, Span};

use crate::data::Data;
use crate::types::{Resolver, Ty};
use crate::{Checker, Comp, Field, FnKind, Reply, RouteDecl, Segment, Sig, names};

impl<'a> Checker<'a> {
    pub(crate) fn collect_items(&mut self) {
        if let Some(app) = &self.names.app {
            let item = app.item;
            self.no_item_attrs(item, "an app");
        }
        let mut components = Vec::new();
        for decl in &self.names.components {
            let mut errors = Vec::new();
            if let Some(attr) = decl.item.attrs.first() {
                errors.push(Diagnostic::new(
                    "a component takes no attributes",
                    attr.span,
                    "remove this attribute",
                ));
            }
            let resolver = Resolver {
                names: &self.names,
                module: decl.module,
            };
            components.push(component_decl(
                decl.ast,
                decl.module,
                &resolver,
                &mut errors,
            ));
            self.diagnostics.extend(errors);
        }
        self.components = components;
        let mut algorithms = Vec::new();
        for decl in &self.names.algorithms {
            let resolver = Resolver {
                names: &self.names,
                module: decl.module,
            };
            let mut errors = Vec::new();
            algorithms.push(signature(
                decl.ast,
                decl.module,
                None,
                &resolver,
                &mut errors,
            ));
            self.diagnostics.extend(errors);
        }
        self.algorithms = algorithms;
        for index in 0..self.components.len() {
            self.check_routes(index);
        }
    }

    fn no_item_attrs(&mut self, item: &Item, what: &str) {
        if let Some(attr) = item.attrs.first() {
            self.error(Diagnostic::new(
                format!("{what} takes no attributes"),
                attr.span,
                "remove this attribute",
            ));
        }
    }
}

fn component_decl<'a>(
    component: &'a Component,
    module: usize,
    resolver: &Resolver,
    diagnostics: &mut Vec<Diagnostic>,
) -> Comp<'a> {
    let mut fields: Vec<Field<'a>> = Vec::new();
    for decl in &component.fields {
        let mut handed_down = false;
        for attr in &decl.attrs {
            match attr.kind {
                AttributeKind::HandedDown => handed_down = true,
                _ => diagnostics.push(Diagnostic::new(
                    "a field takes no attribute but `#[handed_down]`",
                    attr.span,
                    "this attribute goes on a function",
                )),
            }
        }
        if fields.iter().any(|f| f.decl.name.name == decl.name.name) {
            diagnostics.push(Diagnostic::new(
                format!("the field `{}` is declared twice", decl.name.name),
                decl.name.span,
                "this repeats an earlier field",
            ));
            continue;
        }
        let ty = resolver.resolve(&decl.ty, diagnostics);
        check_field_type(decl, &ty, diagnostics);
        fields.push(Field {
            decl,
            ty,
            handed_down,
        });
    }
    let mut functions: Vec<Sig<'a>> = Vec::new();
    for function in &component.functions {
        if functions.iter().any(|f| f.name() == function.name.name) {
            diagnostics.push(defined_twice(&function.name.name, function.name.span));
            continue;
        }
        functions.push(signature(
            function,
            module,
            Some(&component.name.name),
            resolver,
            diagnostics,
        ));
    }
    Comp {
        ast: component,
        module,
        fields,
        functions,
        routes: Vec::new(),
    }
}

/// What a component field may hold: anything a parent can hand it, and
/// nothing that shares state across the boundary.
fn check_field_type(decl: &FieldDecl, ty: &Ty, diagnostics: &mut Vec<Diagnostic>) {
    let span = decl.ty.span;
    match ty {
        Ty::Handle(_, c) => diagnostics.push(
            Diagnostic::new(
                "a component handle cannot be moved",
                span,
                "a field cannot own a handle",
            )
            .with_help(format!(
                "a sibling lent with `&` is declared as `&Handle<{c}>`; a component \
                     reaches its own children through their `start` names"
            )),
        ),
        Ty::Ref {
            mutable: false,
            inner,
        } if matches!(**inner, Ty::Handle(..)) => {}
        Ty::Ref { .. } => diagnostics.push(
            Diagnostic::new(
                "a field cannot hold a borrow",
                span,
                "only a lent sibling, `&Handle<C>`, is borrowed",
            )
            .with_help("make the field own its value; the parent moves or copies it in"),
        ),
        _ => {}
    }
    if let Some(inner) = ty.effect_inside()
        && !matches!(ty, Ty::Ref { .. })
    {
        diagnostics.push(held_in_data(inner, span));
    }
    if let Some(shared) = ty.shared_state() {
        diagnostics.push(shared_across(shared, span));
    }
}

fn signature<'a>(
    function: &'a Function,
    module: usize,
    component: Option<&str>,
    resolver: &Resolver,
    diagnostics: &mut Vec<Diagnostic>,
) -> Sig<'a> {
    let mut routes = 0;
    let mut message = false;
    for attr in &function.attrs {
        match &attr.kind {
            AttributeKind::Route(_) => routes += 1,
            AttributeKind::Message => message = true,
            AttributeKind::HandedDown => diagnostics.push(Diagnostic::new(
                "`#[handed_down]` goes on a field",
                attr.span,
                "a function is not handed down",
            )),
            AttributeKind::Derive(_) | AttributeKind::Status(_) => {
                diagnostics.push(Diagnostic::new(
                    "this attribute does not go on a function",
                    attr.span,
                    "`#[derive]` goes on a struct or enum, and `#[status]` on an enum's variant",
                ));
            }
        }
        let handler = matches!(attr.kind, AttributeKind::Route(_) | AttributeKind::Message);
        if handler && component.is_none() {
            diagnostics.push(
                Diagnostic::new(
                    "a handler belongs to a component",
                    attr.span,
                    "this function is at the top level of the module",
                )
                .with_help("move the function into the component whose fields it uses"),
            );
        }
    }
    if routes > 1 || (routes == 1 && message) {
        diagnostics.push(
            Diagnostic::new(
                "a function is one kind of handler",
                function.name.span,
                "this function has more than one route or message attribute",
            )
            .with_help("give each route or message a function of its own"),
        );
    }
    let kind = if component.is_none() {
        FnKind::Algorithm
    } else if routes > 0 {
        FnKind::Route
    } else if message {
        FnKind::Message
    } else {
        FnKind::Algorithm
    };
    if kind == FnKind::Algorithm {
        if let Some(receiver) = &function.receiver {
            diagnostics.push(
                Diagnostic::new(
                    format!("the algorithm `{}` cannot take `self`", function.name.name),
                    receiver.span,
                    "only a route handler or a `#[message]` takes `self`",
                )
                .with_help("pass the algorithm what it needs as parameters, or make it a handler"),
            );
        }
        if function.is_async {
            diagnostics.push(
                Diagnostic::new(
                    format!("the algorithm `{}` cannot be `async`", function.name.name),
                    function.name.span,
                    "an algorithm is pure, so it has nothing to wait for",
                )
                .with_help("remove `async`, or make the function a handler"),
            );
        }
    }
    let params: Vec<Ty> = function
        .params
        .iter()
        .map(|p| resolver.resolve(&p.ty, diagnostics))
        .collect();
    let ret = match &function.ret {
        Some(ty) => resolver.resolve(ty, diagnostics),
        None => Ty::unit(),
    };
    for (param, ty) in function.params.iter().zip(&params) {
        check_passed_type(diagnostics, ty, param.ty.span, kind);
        if kind == FnKind::Message && matches!(ty, Ty::Ref { mutable: true, .. }) {
            diagnostics.push(
                Diagnostic::new(
                    "a message cannot take `&mut`",
                    param.ty.span,
                    "`&mut` cannot cross a component boundary",
                )
                .with_help("take the value, or `&` to read it, and reply with any change"),
            );
        }
    }
    if let Some(ty) = &function.ret {
        check_passed_type(diagnostics, &ret, ty.span, kind);
    }
    Sig {
        ast: function,
        module,
        kind,
        params,
        ret,
    }
}

/// Function parameters and replies cannot carry capabilities, handles or
/// outlets, and a message's cannot share state.
fn check_passed_type(diagnostics: &mut Vec<Diagnostic>, ty: &Ty, span: Span, kind: FnKind) {
    if ty.is_effect() || ty.effect_inside().is_some() {
        let effect = if ty.is_effect() {
            ty
        } else {
            ty.effect_inside().unwrap_or(ty)
        };
        diagnostics.push(
            Diagnostic::new(
                format!("`{effect}` cannot be passed to or from a function"),
                span,
                "only a component's fields hold capabilities, handles and outlets",
            )
            .with_help("use the field from a handler through `self`"),
        );
    }
    if kind == FnKind::Message
        && let Some(shared) = ty.shared_state()
    {
        diagnostics.push(shared_across(shared, span));
    }
}

fn defined_twice(name: &str, span: Span) -> Diagnostic {
    Diagnostic::new(
        format!("the name `{name}` is defined twice"),
        span,
        "this repeats an earlier name",
    )
    .with_help("rename one of the two")
}

impl<'a> Checker<'a> {
    /// Checks each route of a component and records it.
    fn check_routes(&mut self, index: usize) {
        let mut routes = Vec::new();
        let comp = &self.components[index];
        let mut errors = Vec::new();
        for sig in &comp.functions {
            for attr in &sig.ast.attrs {
                if let AttributeKind::Route(route) = &attr.kind
                    && let Some(decl) =
                        route_decl(&self.datas, comp, sig, route, attr.span, &mut errors)
                {
                    routes.push(decl);
                }
            }
        }
        self.diagnostics.extend(errors);
        self.components[index].routes = routes;
    }

    /// Checks a component's `failure` block against its fields and children.
    pub(crate) fn check_failure(&mut self, index: usize) {
        let comp = &self.components[index];
        let Some(failure) = &comp.ast.failure else {
            return;
        };
        let mut errors = Vec::new();
        let mut seen: Vec<&FailureConditionKind> = Vec::new();
        let mut after_any = false;
        for arm in &failure.arms {
            let condition = &arm.condition;
            if arm.docs.is_empty() {
                errors.push(
                    Diagnostic::new(
                        format!(
                            "the failure arm `{}` gives no reason",
                            condition_text(&condition.kind)
                        ),
                        condition.span,
                        "this response has no doc comment",
                    )
                    .with_help(
                        "write why the program can go on after this failure as a `///` comment \
                         above the arm; the diagram shows it to the reviewer",
                    ),
                );
            }
            if after_any {
                errors.push(
                    Diagnostic::new(
                        "a failure arm after `_` is never used",
                        condition.span,
                        "`_` above already responds to every failure",
                    )
                    .with_help("move `_` to the end of the block"),
                );
            } else if seen.contains(&&condition.kind) {
                errors.push(Diagnostic::new(
                    format!(
                        "the failure arm `{}` is repeated",
                        condition_text(&condition.kind)
                    ),
                    condition.span,
                    "an earlier arm responds to this failure",
                ));
            }
            seen.push(&condition.kind);
            match &condition.kind {
                FailureConditionKind::Other => after_any = true,
                FailureConditionKind::Capability(field) => match comp.field(&field.name) {
                    Some(f) if f.ty.is_capability() && !f.handed_down => {}
                    Some(f) if f.handed_down => {
                        errors.push(handed_down_use(comp.name(), &field.name, field.span))
                    }
                    _ => errors.push(
                        Diagnostic::new(
                            format!("`{}` is not a capability of `{}`", field.name, comp.name()),
                            field.span,
                            "`Capability(...)` names a field holding a capability",
                        )
                        .with_help(capability_fields_help(comp)),
                    ),
                },
                FailureConditionKind::Message(name) => {
                    let lent = comp.field(&name.name).is_some_and(|f| {
                        matches!(&f.ty, Ty::Ref { inner, .. } if matches!(**inner, Ty::Handle(..)))
                    });
                    if comp.child(&name.name).is_none() && !lent {
                        errors.push(
                            Diagnostic::new(
                                format!("`{}` is not a child or lent sibling of `{}`", name.name, comp.name()),
                                name.span,
                                "`Message(...)` names a component this one sends messages to",
                            )
                            .with_help("name a child started with `start name: Component`, or a sibling lent to this component"),
                        );
                    }
                }
                FailureConditionKind::Child(name) => {
                    if comp.child(&name.name).is_none() {
                        errors.push(
                            Diagnostic::new(
                                format!("`{}` is not a child of `{}`", name.name, comp.name()),
                                name.span,
                                "`Child(...)` names a child this component starts",
                            )
                            .with_help("start the child with a name, `start name: Component { ... }`, and use that name"),
                        );
                    }
                }
                FailureConditionKind::Panic | FailureConditionKind::Overflow => {}
            }
        }
        self.diagnostics.extend(errors);
    }
}

/// Checks one route attribute, returning it as a declaration if it can be
/// resolved far enough to serve requests.
fn route_decl(
    datas: &[Data],
    comp: &Comp,
    sig: &Sig,
    route: &Route,
    span: Span,
    errors: &mut Vec<Diagnostic>,
) -> Option<RouteDecl> {
    let function = sig.ast;
    let via = &route.via;
    match comp.field(&via.name) {
        Some(field) if field.handed_down => {
            errors.push(handed_down_use(comp.name(), &via.name, via.span));
        }
        Some(field) if field.ty == Ty::HttpServer => {}
        Some(field) => errors.push(
            Diagnostic::new(
                format!(
                    "`via = {}` names a `{}`, not an `HttpServer`",
                    via.name, field.ty
                ),
                via.span,
                "requests arrive through an `HttpServer`",
            )
            .with_help(server_fields_help(comp)),
        ),
        None => errors.push(
            Diagnostic::new(
                format!("`via = {}` names no field of `{}`", via.name, comp.name()),
                via.span,
                format!("`{}` has no field `{}`", comp.name(), via.name),
            )
            .with_help(server_fields_help(comp)),
        ),
    }
    let path = &route.path;
    let template = path.value.as_str();
    let mut segments = Vec::new();
    let mut ok = true;
    if !template.starts_with('/') {
        errors.push(
            Diagnostic::new(
                "a route's path starts with `/`",
                path.span,
                "this path does not",
            )
            .with_help(format!("write \"/{template}\"")),
        );
        return None;
    }
    if template.contains('?') {
        errors.push(Diagnostic::new(
            "query parameters are not supported yet",
            path.span,
            "the checker does not handle this yet",
        ));
        return None;
    }
    let param = |name: &str| {
        function
            .params
            .iter()
            .zip(&sig.params)
            .find_map(|(p, ty)| match &p.pattern.kind {
                PatternKind::Binding { name: n, .. } if n.name == name => Some((p, ty)),
                _ => None,
            })
    };
    let mut bound: Vec<&str> = Vec::new();
    for text in template[1..].split('/') {
        if let Some(name) = text.strip_prefix('{').and_then(|t| t.strip_suffix('}')) {
            if bound.contains(&name) {
                errors.push(Diagnostic::new(
                    format!("the path parameter `{{{name}}}` appears twice"),
                    path.span,
                    "each path parameter binds a different function parameter",
                ));
                ok = false;
                continue;
            }
            bound.push(name);
            match param(name) {
                Some((p, ty)) => {
                    check_from_text(ty, p, "path", errors);
                    segments.push(Segment::Param(ty.clone()));
                }
                None => {
                    errors.push(
                        Diagnostic::new(
                            format!("the path parameter `{{{name}}}` has no function parameter"),
                            path.span,
                            format!("`{}` takes no parameter `{name}`", function.name.name),
                        )
                        .with_help(format!("add `{name}: String` to the function's parameters")),
                    );
                    ok = false;
                }
            }
        } else if text.contains('{') || text.contains('}') {
            errors.push(
                Diagnostic::new(
                    "a path parameter is a whole segment",
                    path.span,
                    format!("`{text}` mixes text and a parameter"),
                )
                .with_help("put the parameter between `/`s of its own, such as \"/items/{sku}\""),
            );
            ok = false;
        } else {
            segments.push(Segment::Literal(text.to_string()));
        }
    }
    if let Some(body) = &route.body {
        if bound.contains(&body.name.as_str()) {
            errors.push(Diagnostic::new(
                format!("`{}` is both a path parameter and the body", body.name),
                body.span,
                "a parameter is supplied once",
            ));
        }
        match param(&body.name) {
            Some((p, ty)) => check_from_text(ty, p, "body", errors),
            None => errors.push(
                Diagnostic::new(
                    format!(
                        "`body = {}` names no parameter of `{}`",
                        body.name, function.name.name
                    ),
                    body.span,
                    "the body binds to a function parameter",
                )
                .with_help(format!(
                    "add `{}: String` to the function's parameters",
                    body.name
                )),
            ),
        }
        bound.push(&body.name);
    }
    for p in &function.params {
        let name = match &p.pattern.kind {
            PatternKind::Binding { name, .. } => name,
            _ => {
                errors.push(Diagnostic::new(
                    "a route handler's parameters are names",
                    p.pattern.span,
                    "a path parameter or the body binds to a parameter by its name",
                ));
                continue;
            }
        };
        if !bound.contains(&name.name.as_str()) {
            errors.push(
                Diagnostic::new(
                    format!("nothing supplies the parameter `{}`", name.name),
                    name.span,
                    "neither the path nor the body binds this",
                )
                .with_help(format!(
                    "name it in the path as `{{{0}}}`, or read the body into it with `body = {0}`",
                    name.name
                )),
            );
        }
    }
    let reply = match &sig.ret {
        Ty::String => Reply::Text,
        Ty::Std("Option", args) if args[0] == Ty::String => Reply::Optional,
        Ty::Std("Result", args) if args[0] == Ty::String => match &args[1] {
            Ty::Data { id, .. } if datas[*id].statuses().is_some() => {
                Reply::Fallible(datas[*id].statuses().unwrap_or_default())
            }
            Ty::Unknown => Reply::Unknown,
            other => {
                let help = match other {
                    Ty::Data { id, .. } if datas[*id].is_enum => format!(
                        "write `#[status(Name)]` above each variant of `{other}`, naming the status it replies with"
                    ),
                    _ => "declare an enum with a variant for each way the route can fail, \
                          and a `#[status(Name)]` on each variant"
                        .to_string(),
                };
                errors.push(
                    Diagnostic::new(
                        "a route's error type is an enum with a `#[status]` on each variant",
                        function.ret.as_ref().map_or(function.name.span, |t| t.span),
                        format!("`{other}` names no statuses"),
                    )
                    .with_help(help),
                );
                Reply::Unknown
            }
        },
        Ty::Unknown => Reply::Unknown,
        other => {
            let (span, label) = match &function.ret {
                Some(ty) => (ty.span, format!("a route cannot reply with `{other}`")),
                None => (function.name.span, "this route returns nothing".to_string()),
            };
            errors.push(
                Diagnostic::new("a route returns its reply", span, label)
                    .with_help("return a `String`, an `Option<String>` or a `Result<String, E>`"),
            );
            Reply::Unknown
        }
    };
    ok.then(|| RouteDecl {
        method: route.method,
        template: template.to_string(),
        segments,
        via: via.name.clone(),
        body: route.body.is_some(),
        reply,
        span,
        function: function.name.span,
    })
}

/// Path parameters and bodies are read from text, as Rust's `FromStr` reads them.
fn check_from_text(ty: &Ty, param: &Param, from: &str, errors: &mut Vec<Diagnostic>) {
    let readable = matches!(
        ty,
        Ty::Unknown | Ty::String | Ty::Bool | Ty::Char | Ty::Int(_) | Ty::Float(_) | Ty::Decimal
    );
    if !readable {
        errors.push(
            Diagnostic::new(
                format!("a `{ty}` cannot be read from the {from}"),
                param.ty.span,
                format!("the {from} is text"),
            )
            .with_help("take a `String`, a number, a `bool`, a `char` or a `Decimal`"),
        );
    }
}

pub(crate) fn condition_text(kind: &FailureConditionKind) -> String {
    match kind {
        FailureConditionKind::Panic => "Panic".into(),
        FailureConditionKind::Overflow => "Overflow".into(),
        FailureConditionKind::Capability(n) => format!("Capability({})", n.name),
        FailureConditionKind::Message(n) => format!("Message({})", n.name),
        FailureConditionKind::Child(n) => format!("Child({})", n.name),
        FailureConditionKind::Other => "_".into(),
    }
}

pub(crate) fn handed_down_use(component: &str, field: &str, span: Span) -> Diagnostic {
    Diagnostic::new(
        format!("`{field}` is handed down, so `{component}` does not keep it"),
        span,
        "a `#[handed_down]` field is passed straight to a child",
    )
    .with_help("use the field in the child it is moved into, or send that child a message")
}

fn server_fields_help(comp: &Comp) -> String {
    let servers: Vec<&str> = comp
        .fields
        .iter()
        .filter(|f| f.ty == Ty::HttpServer && !f.handed_down)
        .map(|f| f.decl.name.name.as_str())
        .collect();
    if servers.is_empty() {
        format!(
            "add an `HttpServer` field to `{}`, such as `http: HttpServer`, and have its parent move one in",
            comp.name()
        )
    } else {
        format!(
            "`{}` receives requests through {}",
            comp.name(),
            names(servers)
        )
    }
}

fn capability_fields_help(comp: &Comp) -> String {
    let caps: Vec<&str> = comp
        .fields
        .iter()
        .filter(|f| f.ty.is_capability() && !f.handed_down)
        .map(|f| f.decl.name.name.as_str())
        .collect();
    if caps.is_empty() {
        format!("`{}` holds no capabilities", comp.name())
    } else {
        format!("the capabilities of `{}` are {}", comp.name(), names(caps))
    }
}

fn held_in_data(inner: &Ty, span: Span) -> Diagnostic {
    Diagnostic::new(
        format!("`{inner}` cannot be held inside another type"),
        span,
        "capabilities, handles and outlets are held only by a component's fields",
    )
    .with_help("declare it as a field of its own")
}

pub(crate) fn shared_across(shared: &str, span: Span) -> Diagnostic {
    Diagnostic::new(
        format!("`{shared}` cannot cross a component boundary"),
        span,
        "two components would share state the diagram cannot show",
    )
    .with_help(
        "keep the state in one component and send it messages; inside one component it is allowed",
    )
}
