//! Settings, grants and `start` statements: how values, capabilities,
//! handles and outlets reach each component, and so which grant serves
//! each route.

use std::collections::{HashMap, HashSet};

use inset_syntax::ast::*;
use inset_syntax::{Diagnostic, Ident, Span};

use crate::body::Body;
use crate::names::{Def, Ns};
use crate::types::{INTS, Resolver, Ty, fits};
use crate::{Checker, FnKind, RouteInfo, Segment, names};

/// Where a `start` statement is written.
#[derive(Clone, Copy)]
enum Scope {
    App,
    Component(usize),
}

/// What a name in a `start` statement refers to.
#[derive(Clone)]
enum Named {
    Setting(Ty),
    Grant(Ty),
    Field(Ty),
    Child(usize, String),
}

impl Named {
    fn ty(&self) -> Ty {
        match self {
            Named::Setting(ty) | Named::Grant(ty) | Named::Field(ty) => ty.clone(),
            Named::Child(c, name) => Ty::Handle(*c, name.clone()),
        }
    }
}

/// The capabilities the standard library declares, with their grant fields.
fn grant_fields(ty: &Ty) -> &'static [(&'static str, Ty)] {
    const SERVER: [(&str, Ty); 1] = [("port", Ty::Int(Some("u16")))];
    const TABLE: [(&str, Ty); 2] = [("file", Ty::String), ("name", Ty::String)];
    match ty {
        Ty::HttpServer => &SERVER,
        Ty::Table(..) => &TABLE,
        _ => &[],
    }
}

impl<'a> Checker<'a> {
    pub(crate) fn check_app(&mut self) {
        let Some(app) = self.app else {
            return;
        };
        let mut resolved = Vec::new();
        let (setting_tys, grant_tys): (Vec<Ty>, Vec<Ty>) = {
            let resolver = Resolver {
                names: &self.names,
                module: 0,
            };
            (
                app.settings
                    .iter()
                    .map(|s| resolver.resolve(&s.ty, &mut resolved))
                    .collect(),
                app.grants
                    .iter()
                    .map(|g| resolver.resolve(&g.ty, &mut resolved))
                    .collect(),
            )
        };
        self.diagnostics.extend(resolved);
        let mut used: HashSet<&str> = HashSet::new();
        let mut settings: HashMap<&str, Ty> = HashMap::new();
        for (setting, ty) in app.settings.iter().zip(setting_tys) {
            if !used.insert(&setting.name.name) {
                self.name_used_twice(&setting.name);
            }
            let allowed = matches!(ty, Ty::Bool | Ty::String | Ty::Int(_) | Ty::Unknown);
            if !allowed {
                self.error(
                    Diagnostic::new(
                        format!("a setting cannot be a `{ty}`"),
                        setting.ty.span,
                        "settings are given on the command line as text",
                    )
                    .with_help("make the setting a `bool`, an integer type or a `String`"),
                );
            } else if let Err(diagnostic) =
                literal_fits(&setting.default, &ty, "a setting's default")
            {
                self.error(diagnostic);
            }
            settings.insert(&setting.name.name, ty);
        }
        for (grant, ty) in app.grants.iter().zip(grant_tys) {
            if !used.insert(&grant.name.name) {
                self.name_used_twice(&grant.name);
            }
            self.check_grant(grant, &ty, &settings);
        }
        self.check_starts(Scope::App, &app.starts, &mut used);
    }

    fn name_used_twice(&mut self, name: &Ident) {
        self.error(
            Diagnostic::new(
                format!("the name `{}` is used twice", name.name),
                name.span,
                "an earlier setting, grant, field or child has this name",
            )
            .with_help("rename one of the two"),
        );
    }

    fn check_grant(&mut self, grant: &Grant, ty: &Ty, settings: &HashMap<&str, Ty>) {
        if !ty.is_capability() {
            if ty.is_known() {
                self.error(
                    Diagnostic::new(
                        format!("`{ty}` is not a capability"),
                        grant.ty.span,
                        "only a capability is granted",
                    )
                    .with_help("the capabilities are `HttpServer` and `Table<K, V>`"),
                );
            }
            return;
        }
        if let Some(inner) = ty.effect_inside() {
            self.error(
                Diagnostic::new(
                    format!("a table cannot store a `{inner}`"),
                    grant.ty.span,
                    "a table's keys and values are data",
                )
                .with_help("store data types, which hold no capabilities, handles or outlets"),
            );
        }
        let expected = grant_fields(ty);
        let mut given: Vec<&str> = Vec::new();
        for field in &grant.fields {
            let Some((_, field_ty)) = expected.iter().find(|(n, _)| *n == field.name.name) else {
                self.error(
                    Diagnostic::new(
                        format!(
                            "`{}` has no field `{}`",
                            grant_type_name(ty),
                            field.name.name
                        ),
                        field.name.span,
                        "not a field of this capability",
                    )
                    .with_help(format!(
                        "the fields of `{}` are {}",
                        grant_type_name(ty),
                        names(expected.iter().map(|(n, _)| *n))
                    )),
                );
                continue;
            };
            if given.contains(&field.name.name.as_str()) {
                self.error(Diagnostic::new(
                    format!("the field `{}` is given twice", field.name.name),
                    field.name.span,
                    "this repeats an earlier field",
                ));
                continue;
            }
            given.push(&field.name.name);
            let setting = match &field.value {
                None => Some(&field.name),
                Some(Expr {
                    kind: ExprKind::Path(path),
                    ..
                }) if path.segments.len() == 1 => Some(&path.segments[0].ident),
                Some(_) => None,
            };
            let result = match (setting, &field.value) {
                (Some(name), _) => match settings.get(name.name.as_str()) {
                    Some(setting_ty) if fits(field_ty, setting_ty) => Ok(()),
                    Some(setting_ty) => Err(Diagnostic::new(
                        format!(
                            "the setting `{}` is a `{setting_ty}`, but `{}` takes a `{field_ty}`",
                            name.name, field.name.name
                        ),
                        name.span,
                        format!("expected `{field_ty}`"),
                    )),
                    None => Err(Diagnostic::new(
                        format!("no setting `{}`", name.name),
                        name.span,
                        "a grant's field names a setting of the app, or is a literal",
                    )
                    .with_help(format!(
                        "declare `setting {}: {field_ty} = ...;`, or write the value, such as `{}: ...`",
                        name.name, field.name.name
                    ))),
                },
                (None, Some(value)) => literal_fits(value, field_ty, "a grant's field"),
                (None, None) => Ok(()),
            };
            if let Err(diagnostic) = result {
                self.error(diagnostic);
            }
        }
        let missing: Vec<&str> = expected
            .iter()
            .map(|(n, _)| *n)
            .filter(|n| !given.contains(n))
            .collect();
        if !missing.is_empty() {
            self.error(Diagnostic::new(
                format!(
                    "the grant `{}` does not give {}",
                    grant.name.name,
                    names(missing)
                ),
                grant.name.span,
                format!("`{}` needs every field", grant_type_name(ty)),
            ));
        }
    }

    pub(crate) fn check_component_starts(&mut self, index: usize) {
        let comp = &self.components[index];
        let mut used: HashSet<&str> = comp
            .fields
            .iter()
            .map(|f| f.decl.name.name.as_str())
            .collect();
        let starts = &comp.ast.starts;
        let moved = self.check_starts(Scope::Component(index), starts, &mut used);
        // `#[handed_down]` marks exactly the fields moved into a child.
        let comp = &self.components[index];
        let mut errors = Vec::new();
        for field in &comp.fields {
            let name = &field.decl.name;
            match (field.handed_down, moved.get(name.name.as_str())) {
                (true, None) => errors.push(
                    Diagnostic::new(
                        format!("`{}` is marked `#[handed_down]` but no child receives it", name.name),
                        name.span,
                        "nothing moves this field into a child",
                    )
                    .with_help(format!(
                        "move it into a child with `{0}: move {0}`, or remove `#[handed_down]`",
                        name.name
                    )),
                ),
                (false, Some(span)) => errors.push(
                    Diagnostic::new(
                        format!("`{}` is moved into a child, so it is marked `#[handed_down]`", name.name),
                        *span,
                        format!("this moves `{}` out of `{}`", name.name, comp.name()),
                    )
                    .with_help(format!(
                        "write `#[handed_down]` above the field `{}`, so its declaration says it is only passed through",
                        name.name
                    )),
                ),
                _ => {}
            }
        }
        self.diagnostics.extend(errors);
    }

    fn named(&self, scope: Scope, name: &str) -> Option<Named> {
        match scope {
            Scope::App => {
                let app = self.app?;
                let resolver = Resolver {
                    names: &self.names,
                    module: 0,
                };
                let mut sink = Vec::new();
                if let Some(s) = app.settings.iter().find(|s| s.name.name == name) {
                    return Some(Named::Setting(resolver.resolve(&s.ty, &mut sink)));
                }
                if let Some(g) = app.grants.iter().find(|g| g.name.name == name) {
                    return Some(Named::Grant(resolver.resolve(&g.ty, &mut sink)));
                }
                let start = app
                    .starts
                    .iter()
                    .find(|s| s.binding.as_ref().is_some_and(|b| b.name == name))?;
                let child = self.started(start)?;
                Some(Named::Child(
                    child,
                    self.components[child].name().to_string(),
                ))
            }
            Scope::Component(index) => {
                let comp = &self.components[index];
                if let Some(f) = comp.field(name) {
                    return Some(Named::Field(f.ty.clone()));
                }
                let start = comp.start_of(name)?;
                let child = self.started(start)?;
                Some(Named::Child(
                    child,
                    self.components[child].name().to_string(),
                ))
            }
        }
    }

    /// Checks the `start` statements of one body. Returns the names moved
    /// into children, with where each was moved.
    fn check_starts(
        &mut self,
        scope: Scope,
        starts: &'a [Start],
        used: &mut HashSet<&'a str>,
    ) -> HashMap<&'a str, Span> {
        let mut moved: HashMap<&'a str, Span> = HashMap::new();
        let mut errors = Vec::new();
        for (position, start) in starts.iter().enumerate() {
            if let Some(binding) = &start.binding
                && !used.insert(&binding.name)
            {
                errors.push(
                    Diagnostic::new(
                        format!("the name `{}` is used twice", binding.name),
                        binding.span,
                        "an earlier setting, grant, field or child has this name",
                    )
                    .with_help("rename one of the two"),
                );
            }
            let module = match scope {
                Scope::App => 0,
                Scope::Component(index) => self.components[index].module,
            };
            let path = [start.component.clone()];
            let child = match self.names.resolve_in(module, &path, Ns::Type) {
                Ok(Some(Def::Component(child))) => child,
                Err(diagnostic) => {
                    errors.push(diagnostic);
                    continue;
                }
                Ok(other) => {
                    let all: Vec<&str> = self
                        .names
                        .components
                        .iter()
                        .map(|c| c.ast.name.name.as_str())
                        .collect();
                    let help = if all.is_empty() {
                        "the program declares no components".to_string()
                    } else if other.is_none() && all.contains(&start.component.name.as_str()) {
                        format!(
                            "`{}` is declared in another module; import it with `use`",
                            start.component.name
                        )
                    } else {
                        format!("the components are {}", names(all))
                    };
                    errors.push(
                        Diagnostic::new(
                            format!("no component `{}`", start.component.name),
                            start.component.span,
                            "not a component of this program",
                        )
                        .with_help(help),
                    );
                    continue;
                }
            };
            self.started.insert(start.component.span, child);
            let child_name = self.components[child].name().to_string();
            let mut supplied: Vec<&str> = Vec::new();
            for handoff in &start.handoffs {
                let field_name = handoff.field.name.as_str();
                let Some(field) = self.components[child].field(field_name) else {
                    let fields: Vec<&str> = self.components[child]
                        .fields
                        .iter()
                        .map(|f| f.decl.name.name.as_str())
                        .collect();
                    let help = if fields.is_empty() {
                        format!("`{child_name}` has no fields, so its `start` hands it nothing")
                    } else {
                        format!("the fields of `{child_name}` are {}", names(fields))
                    };
                    errors.push(
                        Diagnostic::new(
                            format!("`{child_name}` has no field `{field_name}`"),
                            handoff.field.span,
                            "not a field of the component started",
                        )
                        .with_help(help),
                    );
                    continue;
                };
                if supplied.contains(&field_name) {
                    errors.push(Diagnostic::new(
                        format!("the field `{field_name}` is supplied twice"),
                        handoff.field.span,
                        "this repeats an earlier handoff",
                    ));
                    continue;
                }
                supplied.push(field_name);
                let field_ty = field.ty.clone();
                self.check_handoff(
                    scope,
                    starts,
                    position,
                    &child_name,
                    handoff,
                    &field_ty,
                    &mut moved,
                    &mut errors,
                );
            }
            for field in &self.components[child].fields {
                let name = field.decl.name.name.as_str();
                if supplied.contains(&name) {
                    continue;
                }
                let diagnostic = if let Ty::Outlet(ty) = &field.ty {
                    Diagnostic::new(
                        format!("the outlet `{name}` of `{child_name}` is not connected"),
                        start.component.span,
                        format!("this `start` leaves `{name}` unconnected"),
                    )
                    .with_help(format!(
                        "connect it to a message that takes one `{ty}`, such as `{name}: send child.message`"
                    ))
                } else {
                    Diagnostic::new(
                        format!("`start {child_name}` does not supply the field `{name}`"),
                        start.component.span,
                        format!("`{child_name}` needs `{name}`"),
                    )
                    .with_help(format!(
                        "add `{name}: ...` to the handoffs, with `move` for a capability"
                    ))
                };
                errors.push(diagnostic);
            }
        }
        self.diagnostics.extend(errors);
        moved
    }

    #[allow(clippy::too_many_arguments)]
    fn check_handoff(
        &self,
        scope: Scope,
        starts: &'a [Start],
        position: usize,
        child: &str,
        handoff: &'a Handoff,
        field_ty: &Ty,
        moved: &mut HashMap<&'a str, Span>,
        errors: &mut Vec<Diagnostic>,
    ) {
        let field = &handoff.field.name;
        let lent_handle = match field_ty {
            Ty::Ref { inner, .. } => match &**inner {
                Ty::Handle(_, c) => Some(c.as_str()),
                _ => None,
            },
            _ => None,
        };
        match (&handoff.value, field_ty, lent_handle) {
            (HandoffValue::Connect { target, message }, Ty::Outlet(ty), _) => {
                self.check_connect(scope, starts, field, ty, target, message, errors);
            }
            (_, Ty::Outlet(_), _) => errors.push(
                Diagnostic::new(
                    format!("the outlet `{field}` is connected with `send`"),
                    handoff.span,
                    "an outlet is connected to a message, not given a value",
                )
                .with_help(format!("write `{field}: send child.message`, naming a child or sibling and one of its messages")),
            ),
            (HandoffValue::Connect { .. }, _, _) => errors.push(
                Diagnostic::new(
                    format!("`{field}` is not an outlet"),
                    handoff.span,
                    "`send name.message` connects an outlet",
                )
                .with_help(format!("`{field}` of `{child}` is a `{field_ty}`")),
            ),
            (HandoffValue::Lend(name), _, Some(component)) => {
                let earlier = starts[..position]
                    .iter()
                    .find(|s| s.binding.as_ref().is_some_and(|b| b.name == name.name));
                match earlier {
                    Some(sibling) if sibling.component.name == component => {}
                    Some(sibling) => errors.push(Diagnostic::new(
                        format!(
                            "`{}` runs `{}`, but `{field}` is a `&Handle<{component}>`",
                            name.name, sibling.component.name
                        ),
                        name.span,
                        format!("expected a `{component}`"),
                    )),
                    None => errors.push(
                        Diagnostic::new(
                            format!("`&{}` lends no child started before this one", name.name),
                            name.span,
                            "a child is lent only to siblings started after it",
                        )
                        .with_help(format!(
                            "start a `{component}` with `start {}: {component} {{ ... }};` above this statement",
                            name.name
                        )),
                    ),
                }
            }
            (_, _, Some(component)) => errors.push(
                Diagnostic::new(
                    format!("`{field}` is a lent sibling, supplied with `&`"),
                    handoff.span,
                    format!("expected `&` and the name of an earlier `{component}`"),
                )
                .with_help(format!("write `{field}: &name`, naming a `{component}` started above")),
            ),
            (HandoffValue::Lend(name), _, None) => {
                let help = match self.named(scope, &name.name) {
                    Some(named) if named.ty().is_capability() => format!(
                        "a capability is never lent, so it has one holder; write `{field}: move {}`",
                        name.name
                    ),
                    _ => format!("`{field}` of `{child}` is a `{field_ty}`, which is moved or copied in"),
                };
                errors.push(
                    Diagnostic::new(
                        format!("`&{}` lends a child, and `{field}` takes no handle", name.name),
                        handoff.span,
                        "`&` in a `start` statement lends a child to a later sibling",
                    )
                    .with_help(help),
                );
            }
            (HandoffValue::Move(place), _, None) => {
                if place.parts.len() > 1 {
                    let help = if place.parts[0].name == "self" {
                        format!("in a `start` statement, name the field without `self.`: `move {}`", place.parts[1].name)
                    } else {
                        "hand off a whole setting, grant or field".to_string()
                    };
                    errors.push(
                        Diagnostic::new(
                            "only a whole name can be moved into a child",
                            place.span,
                            "this names a part of a value",
                        )
                        .with_help(help),
                    );
                    return;
                }
                let name = &place.parts[0];
                let Some(named) = self.named(scope, &name.name) else {
                    errors.push(self.unknown_name(scope, name));
                    return;
                };
                if let Named::Child(..) = named {
                    errors.push(
                        Diagnostic::new(
                            "a component handle cannot be moved",
                            name.span,
                            format!("`{}` is a child", name.name),
                        )
                        .with_help(format!("lend it to a later sibling with `&{}`", name.name)),
                    );
                    return;
                }
                if moved.contains_key(name.name.as_str()) && named.ty().is_copy() != Some(true) {
                    errors.push(
                        Diagnostic::new(
                            format!("`{}` is moved twice", name.name),
                            name.span,
                            "an earlier `start` already moved this",
                        )
                        .with_help(if named.ty().is_capability() {
                            "a capability has one holder; any other component that needs it sends that holder a message"
                        } else {
                            "move it once, and give the other child a copy or a new value"
                        }),
                    );
                    return;
                }
                if matches!(named, Named::Field(_) | Named::Grant(_)) {
                    moved.insert(&name.name, name.span);
                }
                let ty = named.ty();
                if !fits(field_ty, &ty) {
                    errors.push(mismatch(place.span, field_ty, &ty, &format!("`{field}` of `{child}`")));
                }
            }
            (HandoffValue::Expr(expr), _, None) => {
                self.check_handoff_expr(scope, child, field, field_ty, expr, errors);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn check_connect(
        &self,
        scope: Scope,
        starts: &[Start],
        outlet: &str,
        outlet_ty: &Ty,
        target: &Ident,
        message: &Ident,
        errors: &mut Vec<Diagnostic>,
    ) {
        let component = if target.name == "self" {
            match scope {
                Scope::App => {
                    errors.push(
                        Diagnostic::new(
                            "an app has no messages",
                            target.span,
                            "`send self.message` connects to the component doing the starting",
                        )
                        .with_help("connect the outlet to a message of a component the app starts"),
                    );
                    return;
                }
                Scope::Component(index) => index,
            }
        } else {
            let sibling = starts
                .iter()
                .find(|s| s.binding.as_ref().is_some_and(|b| b.name == target.name));
            match sibling.and_then(|s| self.started(s)) {
                Some(index) => index,
                None => {
                    if sibling.is_none() {
                        errors.push(
                            Diagnostic::new(
                                format!("no child `{}` to connect to", target.name),
                                target.span,
                                "not the name of a component started here",
                            )
                            .with_help("name a child started with `start name: Component { ... }` in this body, or `self`"),
                        );
                    }
                    return;
                }
            }
        };
        let comp = &self.components[component];
        let Some(sig) = comp
            .function(&message.name)
            .filter(|s| s.kind == FnKind::Message)
        else {
            let is_outlet = comp
                .field(&message.name)
                .is_some_and(|f| matches!(f.ty, Ty::Outlet(_)));
            if target.name == "self" && is_outlet {
                errors.push(Diagnostic::new(
                    "connecting an outlet to an outlet is not supported yet",
                    message.span,
                    "the checker does not handle this yet",
                ));
                return;
            }
            let messages: Vec<&str> = comp
                .functions
                .iter()
                .filter(|f| f.kind == FnKind::Message)
                .map(|f| f.name())
                .collect();
            let help = if messages.is_empty() {
                format!("`{}` has no `#[message]` functions", comp.name())
            } else {
                format!("the messages of `{}` are {}", comp.name(), names(messages))
            };
            errors.push(
                Diagnostic::new(
                    format!("`{}` has no message `{}`", comp.name(), message.name),
                    message.span,
                    "an outlet connects to a `#[message]` function",
                )
                .with_help(help),
            );
            return;
        };
        let full = format!("{}::{}", comp.name(), message.name);
        match sig.params.as_slice() {
            [param] if fits(param, outlet_ty) => {}
            [param] => errors.push(
                Diagnostic::new(
                    format!("the outlet `{outlet}` sends a `{outlet_ty}`, but `{full}` takes a `{param}`"),
                    message.span,
                    format!("this message takes a `{param}`"),
                )
                .with_help(format!(
                    "connect `{outlet}` to a message that takes one `{outlet_ty}`, or change the type of the outlet or the message"
                )),
            ),
            params => errors.push(
                Diagnostic::new(
                    format!(
                        "`{full}` takes {} values, but the outlet `{outlet}` sends one",
                        params.len()
                    ),
                    message.span,
                    format!("expected a message that takes one `{outlet_ty}`"),
                ),
            ),
        }
        if !sig.ret.is_unit() && sig.ret.is_known() {
            errors.push(
                Diagnostic::new(
                    format!("`{full}` replies, but an outlet does not wait for a reply"),
                    message.span,
                    format!("this message returns `{}`", sig.ret),
                )
                .with_help("connect the outlet to a message that returns nothing"),
            );
        }
    }

    fn check_handoff_expr(
        &self,
        scope: Scope,
        child: &str,
        field: &str,
        field_ty: &Ty,
        expr: &Expr,
        errors: &mut Vec<Diagnostic>,
    ) {
        let into = format!("into `{child}`");
        match &expr.kind {
            ExprKind::Ref { mutable: true, .. } => {
                errors.push(mutable_borrow(expr.span, &into));
                return;
            }
            ExprKind::Ref { mutable: false, .. } => {
                errors.push(
                    Diagnostic::new(
                        "`&` in a `start` statement lends a child, and nothing else",
                        expr.span,
                        "a child keeps what it is handed for as long as it runs",
                    )
                    .with_help(format!(
                        "move the value with `{field}: move ...`, or give it a copy"
                    )),
                );
                return;
            }
            ExprKind::Path(path) if path.segments.len() == 1 => {
                let name = &path.segments[0].ident;
                if let Some(named) = self.named(scope, &name.name) {
                    let ty = named.ty();
                    if let Named::Child(..) = named {
                        errors.push(
                            Diagnostic::new(
                                "a component handle cannot be moved or copied",
                                name.span,
                                format!("`{}` is a child", name.name),
                            )
                            .with_help(format!("lend it to a later sibling with `&{}`", name.name)),
                        );
                    } else if ty.is_copy() == Some(false) {
                        errors.push(without_move(&name.name, expr.span, &into, false));
                    } else if !fits(field_ty, &ty) {
                        errors.push(mismatch(
                            expr.span,
                            field_ty,
                            &ty,
                            &format!("`{field}` of `{child}`"),
                        ));
                    }
                    return;
                }
            }
            _ => {}
        }
        let (component, locals) = match scope {
            Scope::App => {
                let locals = self
                    .app
                    .map(|app| {
                        app.settings
                            .iter()
                            .filter_map(|s| {
                                self.named(scope, &s.name.name).map(|n| (&s.name, n.ty()))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                (None, locals)
            }
            Scope::Component(index) => {
                let locals = self.components[index]
                    .fields
                    .iter()
                    .map(|f| (&f.decl.name, f.ty.clone()))
                    .collect();
                (Some(index), locals)
            }
        };
        let (ty, diagnostics) = Body::start_expr(self, component, locals, expr);
        errors.extend(diagnostics);
        if !fits(field_ty, &ty) {
            errors.push(mismatch(
                expr.span,
                field_ty,
                &ty,
                &format!("`{field}` of `{child}`"),
            ));
        }
    }

    fn unknown_name(&self, scope: Scope, name: &Ident) -> Diagnostic {
        let (label, help) = match scope {
            Scope::App => (
                "not a setting, grant or child of the app",
                format!(
                    "declare it with `grant {}: ...` or `setting {}: ...`",
                    name.name, name.name
                ),
            ),
            Scope::Component(index) => (
                "not a field or child of this component",
                format!(
                    "declare `{}` as a field of `{}`, and have its parent hand it down",
                    name.name,
                    self.components[index].name()
                ),
            ),
        };
        Diagnostic::new(
            format!("cannot find `{}` to hand off", name.name),
            name.span,
            label,
        )
        .with_help(help)
    }

    /// Reports components that start themselves, directly or through others,
    /// which would start without end.
    pub(crate) fn check_cycles(&mut self) {
        let mut state = vec![0u8; self.components.len()];
        let mut stack = Vec::new();
        let mut errors = Vec::new();
        for index in 0..self.components.len() {
            self.visit(index, &mut state, &mut stack, &mut errors);
        }
        self.diagnostics.extend(errors);
    }

    fn visit(
        &self,
        index: usize,
        state: &mut [u8],
        stack: &mut Vec<usize>,
        errors: &mut Vec<Diagnostic>,
    ) {
        if state[index] != 0 {
            return;
        }
        state[index] = 1;
        stack.push(index);
        for start in &self.components[index].ast.starts {
            let Some(child) = self.started(start) else {
                continue;
            };
            if state[child] == 1 {
                let from = stack.iter().position(|&c| c == child).unwrap_or(0);
                let mut chain: Vec<&str> = stack[from..]
                    .iter()
                    .map(|&c| self.components[c].name())
                    .collect();
                chain.push(self.components[child].name());
                let chain = chain
                    .iter()
                    .map(|c| format!("`{c}`"))
                    .collect::<Vec<_>>()
                    .join(" → ");
                errors.push(
                    Diagnostic::new(
                        format!("`{}` starts itself", self.components[child].name()),
                        start.component.span,
                        "this `start` leads back to a component already starting",
                    )
                    .with_help(format!(
                        "the loop is {chain}; a component cannot be among its own descendants"
                    )),
                );
            } else {
                self.visit(child, state, stack, errors);
            }
        }
        stack.pop();
        state[index] = 2;
    }

    /// Follows grants down through the `start` statements to find which grant
    /// serves each route.
    pub(crate) fn collect_routes(&mut self) {
        let Some(app) = self.app else {
            return;
        };
        let mut routes = Vec::new();
        for start in &app.starts {
            let origins: HashMap<&str, Option<String>> = start
                .handoffs
                .iter()
                .map(|h| {
                    let grant = match &h.value {
                        HandoffValue::Move(place) if place.parts.len() == 1 => app
                            .grants
                            .iter()
                            .find(|g| g.name.name == place.parts[0].name)
                            .map(|g| g.name.name.clone()),
                        _ => None,
                    };
                    (h.field.name.as_str(), grant)
                })
                .collect();
            if let Some(child) = self.started(start) {
                self.instance(child, &origins, &mut Vec::new(), &mut routes);
            }
        }
        // Two routes of one server that match the same requests.
        let mut errors = Vec::new();
        for (i, later) in routes.iter().enumerate() {
            let clash = routes[..i].iter().find(|earlier: &&RouteInfo| {
                earlier.grant.is_some()
                    && earlier.grant == later.grant
                    && earlier.decl.span != later.decl.span
                    && earlier.decl.method == later.decl.method
                    && same_shape(&earlier.decl.segments, &later.decl.segments)
            });
            if let Some(earlier) = clash
                && !errors
                    .iter()
                    .any(|d: &Diagnostic| d.span == later.decl.span)
            {
                errors.push(
                    Diagnostic::new(
                        format!(
                            "two routes match `{} {}`",
                            later.decl.method.as_str(),
                            later.decl.template
                        ),
                        later.decl.span,
                        format!(
                            "this matches the same requests as `{} {}` in `{}`",
                            earlier.decl.method.as_str(),
                            earlier.decl.template,
                            earlier.component
                        ),
                    )
                    .with_help("give each route a path of its own"),
                );
            }
        }
        self.diagnostics.extend(errors);
        self.routes = routes;
    }

    fn instance(
        &self,
        index: usize,
        origins: &HashMap<&str, Option<String>>,
        stack: &mut Vec<usize>,
        routes: &mut Vec<RouteInfo>,
    ) {
        if stack.contains(&index) {
            return;
        }
        stack.push(index);
        let comp = &self.components[index];
        for decl in &comp.routes {
            routes.push(RouteInfo {
                decl: decl.clone(),
                component: comp.name().to_string(),
                grant: origins.get(decl.via.as_str()).cloned().flatten(),
            });
        }
        for start in &comp.ast.starts {
            let child_origins: HashMap<&str, Option<String>> = start
                .handoffs
                .iter()
                .map(|h| {
                    let grant = match &h.value {
                        HandoffValue::Move(place) if place.parts.len() == 1 => {
                            origins.get(place.parts[0].name.as_str()).cloned().flatten()
                        }
                        _ => None,
                    };
                    (h.field.name.as_str(), grant)
                })
                .collect();
            if let Some(child) = self.started(start) {
                self.instance(child, &child_origins, stack, routes);
            }
        }
        stack.pop();
    }
}

fn same_shape(a: &[Segment], b: &[Segment]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|pair| match pair {
            (Segment::Literal(x), Segment::Literal(y)) => x == y,
            (Segment::Param(_), Segment::Param(_)) => true,
            _ => false,
        })
}

fn grant_type_name(ty: &Ty) -> &'static str {
    match ty {
        Ty::Table(..) => "Table",
        _ => "HttpServer",
    }
}

/// Checks a literal against a setting's or grant field's type.
fn literal_fits(expr: &Expr, ty: &Ty, what: &str) -> Result<(), Diagnostic> {
    let (lit, negative) = match &expr.kind {
        ExprKind::Lit(lit) => (lit, false),
        ExprKind::Unary {
            op: UnaryOp::Neg,
            operand,
        } => match &operand.kind {
            ExprKind::Lit(lit) => (lit, true),
            _ => return Err(not_literal(expr, what)),
        },
        _ => return Err(not_literal(expr, what)),
    };
    let found = match (&lit.kind, ty) {
        (LitKind::Int(text), Ty::Int(Some(name))) => {
            return int_fits(text, negative, name, expr.span);
        }
        (_, Ty::Unknown) => return Ok(()),
        (LitKind::Bool(_), Ty::Bool) | (LitKind::Str(_), Ty::String) if !negative => return Ok(()),
        (LitKind::Int(_), _) => "an integer",
        (LitKind::Float(_), _) => "a float",
        (LitKind::Bool(_), _) => "a `bool`",
        (LitKind::Str(_), _) => "a string",
        (LitKind::Char(_), _) => "a `char`",
    };
    Err(Diagnostic::new(
        format!("expected a `{ty}`, found {found}"),
        expr.span,
        format!("expected `{ty}`"),
    ))
}

fn not_literal(expr: &Expr, what: &str) -> Diagnostic {
    Diagnostic::new(
        format!("{what} is a literal"),
        expr.span,
        "this is not a literal",
    )
    .with_help("write the value itself, such as `8080` or \"items.db\"; a grant's field may also name a setting")
}

fn int_fits(text: &str, negative: bool, name: &str, span: Span) -> Result<(), Diagnostic> {
    let digits: String = text.chars().filter(|c| *c != '_').collect();
    let suffix = INTS.iter().find(|s| digits.ends_with(**s));
    if let Some(suffix) = suffix
        && *suffix != name
    {
        return Err(Diagnostic::new(
            format!("expected a `{name}`, found a `{suffix}`"),
            span,
            format!("expected `{name}`"),
        ));
    }
    let digits = suffix.map_or(digits.as_str(), |s| &digits[..digits.len() - s.len()]);
    let (radix, digits) = match digits.get(..2) {
        Some("0x") => (16, &digits[2..]),
        Some("0o") => (8, &digits[2..]),
        Some("0b") => (2, &digits[2..]),
        _ => (10, digits),
    };
    let value = u128::from_str_radix(digits, radix).ok().map(|v| {
        if negative {
            -(v.min(i128::MAX as u128) as i128)
        } else {
            v.min(i128::MAX as u128) as i128
        }
    });
    let (min, max) = int_range(name);
    match value {
        Some(v) if v >= min && v <= max => Ok(()),
        _ => Err(Diagnostic::new(
            format!(
                "`{}{text}` does not fit in a `{name}`",
                if negative { "-" } else { "" }
            ),
            span,
            format!("a `{name}` holds {min} to {max}"),
        )),
    }
}

fn int_range(name: &str) -> (i128, i128) {
    let bits: u32 = match name.trim_start_matches(['i', 'u']) {
        "size" => 64,
        n => n.parse().unwrap_or(64),
    };
    if name.starts_with('u') {
        (
            0,
            if bits >= 127 {
                i128::MAX
            } else {
                (1i128 << bits) - 1
            },
        )
    } else if bits >= 128 {
        (i128::MIN, i128::MAX)
    } else {
        (-(1i128 << (bits - 1)), (1i128 << (bits - 1)) - 1)
    }
}

pub(crate) fn mismatch(span: Span, expected: &Ty, found: &Ty, what: &str) -> Diagnostic {
    let diagnostic = Diagnostic::new(
        format!("expected a `{expected}`, found a `{found}`"),
        span,
        format!("{what} is a `{expected}`"),
    );
    match (expected, found) {
        (Ty::String, Ty::Ref { inner, .. }) if **inner == Ty::Str => {
            diagnostic.with_help("make a `String` from it with `.to_string()`")
        }
        _ => diagnostic,
    }
}

pub(crate) fn mutable_borrow(span: Span, into: &str) -> Diagnostic {
    Diagnostic::new(
        "`&mut` cannot cross a component boundary",
        span,
        format!("this lends a value {into} to change"),
    )
    .with_help(
        "a component changes only its own fields: move the value across with `move` and use what \
         comes back, or keep it in the other component and send that component messages",
    )
}

pub(crate) fn without_move(name: &str, span: Span, into: &str, can_lend: bool) -> Diagnostic {
    let (notation, help) = if can_lend {
        (
            "`move` or `&`",
            format!("write `move {name}` to hand it over, or `&{name}` to lend it until the reply"),
        )
    } else {
        ("`move`", format!("write `move {name}` to hand it over"))
    };
    Diagnostic::new(
        format!("`{name}` crosses {into} without {notation}"),
        span,
        "only a `Copy` value crosses a boundary bare",
    )
    .with_help(help)
}
