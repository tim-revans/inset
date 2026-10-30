//! The app, components, their fields and functions, routes and `failure`
//! blocks: everything checked from declarations alone.

use std::collections::HashSet;

use inset_syntax::ast::*;
use inset_syntax::{Diagnostic, Span};

use crate::types::{Resolver, Ty};
use crate::{Checker, Comp, Field, FnKind, Reply, RouteDecl, Segment, Sig, names};

impl<'a> Checker<'a> {
    pub(crate) fn collect_items(&mut self) {
        let component_names = self.component_names();
        let resolver = Resolver {
            components: &component_names,
        };
        let mut types: HashSet<&str> = HashSet::new();
        let mut functions: HashSet<&str> = HashSet::new();
        for item in &self.module.items {
            match &item.kind {
                ItemKind::App(app) => {
                    self.no_item_attrs(item, "an app");
                    if let Some(first) = self.app {
                        self.error(
                            Diagnostic::new(
                                "a program has one app",
                                app.name.span,
                                "this is a second app",
                            )
                            .with_help(format!(
                                "put its settings, grants and starts in `app {}`",
                                first.name.name
                            )),
                        );
                        continue;
                    }
                    if !types.insert(&app.name.name) {
                        self.defined_twice(&app.name.name, app.name.span);
                    }
                    self.app = Some(app);
                }
                ItemKind::Component(component) => {
                    self.no_item_attrs(item, "a component");
                    if !types.insert(&component.name.name) {
                        self.defined_twice(&component.name.name, component.name.span);
                        continue;
                    }
                    let comp = self.component_decl(component, &resolver);
                    self.components.push(comp);
                }
                ItemKind::Function(function) => {
                    if !functions.insert(&function.name.name) {
                        self.defined_twice(&function.name.name, function.name.span);
                        continue;
                    }
                    let sig = self.signature(function, None, &resolver);
                    self.algorithms.push(sig);
                }
            }
        }
        if self.app.is_none() {
            self.error(
                Diagnostic::new(
                    "the program has no app",
                    Span::new(0, 0),
                    "no `app` in this file",
                )
                .with_help("add `app Name { ... }` with the program's settings, grants and starts"),
            );
        }
        for index in 0..self.components.len() {
            self.check_routes(index);
        }
    }

    fn defined_twice(&mut self, name: &str, span: Span) {
        self.error(
            Diagnostic::new(
                format!("the name `{name}` is defined twice"),
                span,
                "this repeats an earlier name",
            )
            .with_help("rename one of the two"),
        );
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

    fn component_decl(&mut self, component: &'a Component, resolver: &Resolver) -> Comp<'a> {
        let mut fields: Vec<Field<'a>> = Vec::new();
        for decl in &component.fields {
            let mut handed_down = false;
            for attr in &decl.attrs {
                match attr.kind {
                    AttributeKind::HandedDown => handed_down = true,
                    _ => self.error(Diagnostic::new(
                        "a field takes no attribute but `#[handed_down]`",
                        attr.span,
                        "this attribute goes on a function",
                    )),
                }
            }
            if fields.iter().any(|f| f.decl.name.name == decl.name.name) {
                self.error(Diagnostic::new(
                    format!("the field `{}` is declared twice", decl.name.name),
                    decl.name.span,
                    "this repeats an earlier field",
                ));
                continue;
            }
            let ty = resolver.resolve(&decl.ty, &mut self.diagnostics);
            self.check_field_type(decl, &ty);
            fields.push(Field {
                decl,
                ty,
                handed_down,
            });
        }
        let mut functions: Vec<Sig<'a>> = Vec::new();
        for function in &component.functions {
            if functions.iter().any(|f| f.name() == function.name.name) {
                self.defined_twice(&function.name.name, function.name.span);
                continue;
            }
            functions.push(self.signature(function, Some(&component.name.name), resolver));
        }
        Comp {
            ast: component,
            fields,
            functions,
            routes: Vec::new(),
        }
    }

    /// What a component field may hold: anything a parent can hand it, and
    /// nothing that shares state across the boundary.
    fn check_field_type(&mut self, decl: &FieldDecl, ty: &Ty) {
        let span = decl.ty.span;
        match ty {
            Ty::Handle(c) => self.error(
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
            } if matches!(**inner, Ty::Handle(_)) => {}
            Ty::Ref { .. } => self.error(
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
            self.error(held_in_data(inner, span));
        }
        if let Some(shared) = ty.shared_state() {
            self.error(shared_across(shared, span));
        }
    }

    fn signature(
        &mut self,
        function: &'a Function,
        component: Option<&str>,
        resolver: &Resolver,
    ) -> Sig<'a> {
        let mut routes = 0;
        let mut message = false;
        for attr in &function.attrs {
            match &attr.kind {
                AttributeKind::Route(_) => routes += 1,
                AttributeKind::Message => message = true,
                AttributeKind::HandedDown => self.error(Diagnostic::new(
                    "`#[handed_down]` goes on a field",
                    attr.span,
                    "a function is not handed down",
                )),
            }
            let handler = matches!(attr.kind, AttributeKind::Route(_) | AttributeKind::Message);
            if handler && component.is_none() {
                self.error(
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
            self.error(
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
                self.error(
                    Diagnostic::new(
                        format!("the algorithm `{}` cannot take `self`", function.name.name),
                        receiver.span,
                        "only a route handler or a `#[message]` takes `self`",
                    )
                    .with_help(
                        "pass the algorithm what it needs as parameters, or make it a handler",
                    ),
                );
            }
            if function.is_async {
                self.error(
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
            .map(|p| resolver.resolve(&p.ty, &mut self.diagnostics))
            .collect();
        let ret = match &function.ret {
            Some(ty) => resolver.resolve(ty, &mut self.diagnostics),
            None => Ty::unit(),
        };
        for (param, ty) in function.params.iter().zip(&params) {
            self.check_passed_type(ty, param.ty.span, kind);
            if kind == FnKind::Message && matches!(ty, Ty::Ref { mutable: true, .. }) {
                self.error(
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
            self.check_passed_type(&ret, ty.span, kind);
        }
        Sig {
            ast: function,
            kind,
            params,
            ret,
        }
    }

    /// Function parameters and replies cannot carry capabilities, handles or
    /// outlets, and a message's cannot share state.
    fn check_passed_type(&mut self, ty: &Ty, span: Span, kind: FnKind) {
        if ty.is_effect() || ty.effect_inside().is_some() {
            let effect = if ty.is_effect() {
                ty
            } else {
                ty.effect_inside().unwrap_or(ty)
            };
            self.error(
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
            self.error(shared_across(shared, span));
        }
    }

    /// Checks each route of a component and records it.
    fn check_routes(&mut self, index: usize) {
        let mut routes = Vec::new();
        let comp = &self.components[index];
        let mut errors = Vec::new();
        for sig in &comp.functions {
            for attr in &sig.ast.attrs {
                if let AttributeKind::Route(route) = &attr.kind
                    && let Some(decl) = route_decl(comp, sig, route, attr.span, &mut errors)
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
                        matches!(&f.ty, Ty::Ref { inner, .. } if matches!(**inner, Ty::Handle(_)))
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
        Ty::Std("Result", args) if args[0] == Ty::String => {
            if args[1].is_known() {
                errors.push(
                    Diagnostic::new(
                        "a route's error type is an enum with a `#[status]` on each variant",
                        function.ret.as_ref().map_or(function.name.span, |t| t.span),
                        format!("`{}` names no statuses", args[1]),
                    )
                    .with_help("declaring enums is not supported yet"),
                );
            }
            Reply::Fallible
        }
        Ty::Unknown => Reply::Fallible,
        other => {
            let (span, label) = match &function.ret {
                Some(ty) => (ty.span, format!("a route cannot reply with `{other}`")),
                None => (function.name.span, "this route returns nothing".to_string()),
            };
            errors.push(
                Diagnostic::new("a route returns its reply", span, label)
                    .with_help("return a `String`, an `Option<String>` or a `Result<String, E>`"),
            );
            Reply::Fallible
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
