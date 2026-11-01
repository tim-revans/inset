//! Diagram data emitted from a checked program, which `inset diagram` draws.
//!
//! [`emit`] checks the program with `inset-check`, takes its modules and
//! resolved names from the checker, and walks the syntax trees in source
//! order, so the same source always gives the same data. Every label is the source text of what it labels, because
//! that is what the compiler knows to be true; text that spans lines is
//! joined onto one.
//!
//! **What is covered.** The app's settings and grants, the components it
//! starts and the children they start, with every field, `failure` arm,
//! route, message handler and algorithm, capability edges with the
//! operations called on each capability, and a flowchart for every function
//! whose body uses statements, `let ... else`, `if`, `if let`, `match`,
//! `for`, `while`, `return`, `?` in a function that returns an `Option`
//! or a `Result`, awaited capability calls, each a step linked to the
//! capability's edge, and messages, awaited or sent, and values emitted
//! through an outlet, each a step linked to its message or emit edge. Algorithms in every module are drawn, each owned by
//! the app and its module's path. Anything else that `inset check` accepts
//! gets a diagnostic saying it cannot be drawn yet, at the construct,
//! rather than a diagram that leaves it out. Edges are grouped by kind,
//! capabilities first, then messages and emits, each kind in the order its
//! elements are drawn.

mod flow;

use std::collections::{BTreeMap, BTreeSet, HashMap};

use inset_check::{Modules, Program, Report, Resolved};
use inset_syntax::ast::{
    self, AttributeKind, ExprKind, FailureConditionKind, FailureResponse, HandoffValue, ItemKind,
    LitKind, TypeKind,
};
use inset_syntax::{Diagnostic, Span};

use crate::model::{
    App, Component, Condition, Config, Diagram, Edge, External, FailureArm, Field, FieldKind,
    FrontEnd, Function, Grant, Handoff, Method, Outside, OutsideKind, Param, Receiver, Response,
    Route, Setting,
};

/// Checks the program and emits its diagram data. The reports are the
/// checker's, or else one for each construct that cannot be drawn yet.
pub fn emit(program: &Program) -> Result<Diagram, Vec<Report>> {
    emit_with_sources(program).map(|emitted| emitted.diagram)
}

/// Diagram data, with where each element is in the source.
#[derive(Debug, Clone)]
pub struct Emitted {
    pub diagram: Diagram,
    /// For every element a picture can draw, by ID, where in the source a
    /// reader should be taken: a declaration's name, a step's statement, a
    /// branch's block. An instance of a component goes to the component's
    /// declaration, a capability edge to the field that holds the
    /// capability, and what a grant reaches outside the app to the grant.
    pub sources: BTreeMap<String, Source>,
}

/// A place in a program's source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Source {
    /// The file, by its index in the program.
    pub file: usize,
    pub span: Span,
}

/// As [`emit`], and records where each element is in the source, so an
/// editor can take a reader from a picture to the code.
pub fn emit_with_sources(program: &Program) -> Result<Emitted, Vec<Report>> {
    let resolved = inset_check::resolve(program)?;
    let mut emitter = Emitter::new(&resolved);
    let diagram = emitter.diagram();
    let modules = &resolved.modules;
    if emitter.diagnostics.is_empty() {
        let sources = emitter
            .sources
            .into_iter()
            .filter_map(|(id, span)| {
                let (file, span) = modules.locate(span)?;
                Some((id, Source { file, span }))
            })
            .collect();
        Ok(Emitted { diagram, sources })
    } else {
        Err(emitter
            .diagnostics
            .into_iter()
            .filter_map(|diagnostic| modules.report(diagnostic))
            .collect())
    }
}

/// Walks the syntax tree, collecting what cannot be drawn yet.
struct Emitter<'a> {
    modules: &'a Modules,
    /// What each name in the code refers to, as the checker resolved it.
    refs: &'a HashMap<Span, Span>,
    app: String,
    /// Every component declared, by the span of its name, with its module.
    components: HashMap<Span, (&'a ast::Item, &'a ast::Component, usize)>,
    /// The ID of every algorithm at the top level of a module, by the span
    /// of its name.
    algorithms: HashMap<Span, String>,
    /// The names of the app's settings.
    app_settings: BTreeSet<&'a str>,
    /// For each capability field a component received, where it came from:
    /// a grant, or a field of its parent.
    moved_from: BTreeMap<String, String>,
    /// The capability fields, by ID, with the methods called on each.
    operations: BTreeMap<String, BTreeSet<String>>,
    /// Capability fields that are used, in the order the components are drawn.
    used_capabilities: Vec<String>,
    /// Message and emit edges, in the order their calls are drawn.
    message_edges: Vec<Edge>,
    emit_edges: Vec<Edge>,
    /// Where each element is, by program-wide span.
    sources: BTreeMap<String, Span>,
    /// The reason of the `#[defensive]` on the statement being drawn,
    /// which excuses its error path.
    excused: Option<String>,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> Emitter<'a> {
    fn new(resolved: &'a Resolved) -> Emitter<'a> {
        let mut components = HashMap::new();
        for (index, module) in resolved.modules.list.iter().enumerate() {
            for item in &module.ast.items {
                if let ItemKind::Component(c) = &item.kind {
                    components.insert(c.name.span, (item, c, index));
                }
            }
        }
        Emitter {
            modules: &resolved.modules,
            refs: &resolved.refs,
            app: String::new(),
            components,
            algorithms: HashMap::new(),
            app_settings: BTreeSet::new(),
            moved_from: BTreeMap::new(),
            operations: BTreeMap::new(),
            used_capabilities: Vec::new(),
            message_edges: Vec::new(),
            emit_edges: Vec::new(),
            sources: BTreeMap::new(),
            excused: None,
            diagnostics: Vec::new(),
        }
    }

    fn unsupported(&mut self, what: &str, span: Span, label: &str) {
        self.diagnostics.push(
            Diagnostic::new(format!("{what} cannot be drawn yet"), span, label).with_help(
                "`inset check` accepts this, but the diagram generator does not draw it yet",
            ),
        );
    }

    /// Records where an element is, replacing any earlier record.
    fn locate(&mut self, id: &str, span: Span) {
        self.sources.insert(id.to_string(), span);
    }

    /// The source text of a span, on one line.
    fn text(&self, span: Span) -> String {
        one_line(self.modules.text(span))
    }

    fn diagram(&mut self) -> Diagram {
        let modules = self.modules;
        let (item, app) = modules.list[0]
            .ast
            .items
            .iter()
            .find_map(|item| match &item.kind {
                ItemKind::App(app) => Some((item, app)),
                _ => None,
            })
            .expect("a checked program has an app");
        self.app = app.name.name.clone();
        self.locate(&app.name.name, app.name.span);
        self.app_settings = app.settings.iter().map(|s| s.name.name.as_str()).collect();
        for s in &app.settings {
            self.locate(
                &format!("{}/setting:{}", self.app, s.name.name),
                s.name.span,
            );
        }
        let settings = app
            .settings
            .iter()
            .map(|s| Setting {
                id: format!("{}/setting:{}", self.app, s.name.name),
                name: s.name.name.clone(),
                ty: self.text(s.ty.span),
                default: self.text(s.default.span),
                doc: doc(&s.docs),
            })
            .collect();
        let grants = app.grants.iter().map(|g| self.grant(app, g)).collect();
        let app_id = self.app.clone();
        // Every algorithm's ID is known before any flowchart links to it.
        let mut algorithms = Vec::new();
        for module in &modules.list {
            let owner = if module.path.is_empty() {
                app_id.clone()
            } else {
                format!("{app_id}/mod:{}", module.path.join("::"))
            };
            for item in &module.ast.items {
                if let ItemKind::Function(f) = &item.kind {
                    self.algorithms
                        .insert(f.name.span, format!("{owner}/fn:{}", f.name.name));
                    algorithms.push((owner.clone(), item, f));
                }
            }
        }
        let components = self.starts(&app_id, &app.starts, None);
        let front_end = app
            .front_ends
            .first()
            .map(|f| self.front_end(f, &components));
        let algorithms = algorithms
            .into_iter()
            .map(|(owner, item, f)| self.function(&owner, f, &item.docs, None))
            .collect();
        // Grouped by kind: capabilities, then messages, then emits.
        let mut edges = self.capability_edges();
        edges.append(&mut self.message_edges);
        edges.append(&mut self.emit_edges);
        Diagram {
            version: 1,
            app: App {
                id: app_id.clone(),
                name: app_id,
                doc: doc(&item.docs),
                settings,
                grants,
                front_end,
                components,
            },
            algorithms,
            edges,
            coverage: None,
        }
    }

    /// The front end, with each call as the ID of the handler of the route
    /// it names, on a component that the `via` grant reaches.
    fn front_end(&mut self, front: &ast::FrontEnd, components: &[Component]) -> FrontEnd {
        let id = format!("{}/front_end", self.app);
        self.locate(&id, front.span);
        let via = format!("{}/grant:{}", self.app, front.via.name);
        let mut routes = Vec::new();
        collect_routes(components, &mut routes);
        let calls = front
            .calls
            .iter()
            .filter_map(|call| {
                let (method, path) = call.value.split_once(' ')?;
                routes
                    .iter()
                    .find(|route| {
                        let mut origin = &route.via;
                        while let Some(next) = self.moved_from.get(origin) {
                            origin = next;
                        }
                        *origin == via && route.path == path && route.method.as_str() == method
                    })
                    .map(|route| route.handler.id.clone())
            })
            .collect();
        FrontEnd {
            id,
            via,
            doc: doc(&front.docs),
            calls,
        }
    }

    fn grant(&mut self, app: &ast::App, grant: &ast::Grant) -> Grant {
        let setting = |name: &str| app.settings.iter().find(|s| s.name.name == name);
        let config: Vec<Config> = grant
            .fields
            .iter()
            .map(|field| {
                let name = field.name.name.clone();
                let from_setting = match &field.value {
                    None => Some(name.as_str()),
                    Some(value) => match &value.kind {
                        ExprKind::Path(p) if p.segments.len() == 1 => {
                            Some(p.segments[0].ident.name.as_str())
                        }
                        _ => None,
                    },
                };
                match from_setting.and_then(setting) {
                    Some(s) => Config::Setting {
                        name: name.clone(),
                        setting: format!("{}/setting:{}", self.app, s.name.name),
                    },
                    None => Config::Fixed {
                        value: field
                            .value
                            .as_ref()
                            .map_or_else(String::new, |v| self.text(v.span)),
                        name,
                    },
                }
            })
            .collect();
        let capability = match &grant.ty.kind {
            TypeKind::Path(p) => p.segments.last().map(|s| s.ident.name.as_str()),
            _ => None,
        };
        let outside = match capability {
            Some("HttpServer") => Outside {
                kind: OutsideKind::Clients,
                label: "HTTP clients".into(),
                setting: None,
            },
            Some(kind @ ("Table" | "HttpClient")) => {
                // The file the table is stored in, or the service the client
                // calls, or the default of the setting that chooses it.
                let key = if kind == "Table" { "file" } else { "base" };
                let file = grant.fields.iter().find(|f| f.name.name == key);
                let literal = |expr: &ast::Expr| match &expr.kind {
                    ExprKind::Lit(ast::Lit {
                        kind: LitKind::Str(s),
                        ..
                    }) => Some(s.clone()),
                    _ => None,
                };
                let chosen = config.iter().find_map(|c| match c {
                    Config::Setting { name, setting } if name == key => Some(setting.clone()),
                    _ => None,
                });
                let label = match &chosen {
                    Some(id) => {
                        let name = id.rsplit(':').next().unwrap_or_default();
                        setting(name).and_then(|s| literal(&s.default))
                    }
                    None => file.and_then(|f| f.value.as_ref()).and_then(literal),
                };
                Outside {
                    kind: OutsideKind::Resource,
                    label: label.unwrap_or_else(|| grant.name.name.clone()),
                    setting: chosen,
                }
            }
            _ => {
                self.unsupported("a grant of this capability", grant.ty.span, "granted here");
                Outside {
                    kind: OutsideKind::Resource,
                    label: grant.name.name.clone(),
                    setting: None,
                }
            }
        };
        let id = format!("{}/grant:{}", self.app, grant.name.name);
        self.locate(&id, grant.name.span);
        self.locate(&format!("{id}/outside"), grant.name.span);
        Grant {
            id,
            name: grant.name.name.clone(),
            ty: self.text(grant.ty.span),
            config,
            outside,
            doc: doc(&grant.docs),
        }
    }

    /// The components started by `starts`, inside `parent`. `owner` is the
    /// component whose `start` statements these are, or `None` for the app.
    fn starts(
        &mut self,
        parent: &str,
        starts: &'a [ast::Start],
        owner: Option<&'a ast::Component>,
    ) -> Vec<Component> {
        let mut components = Vec::new();
        for start in starts {
            let Some((item, component, module)) = self
                .refs
                .get(&start.component.span)
                .and_then(|target| self.components.get(target))
                .copied()
            else {
                continue;
            };
            let segment = start
                .binding
                .as_ref()
                .map_or(&start.component.name, |b| &b.name);
            let id = format!("{parent}/{segment}");
            let handoffs = start
                .handoffs
                .iter()
                .filter_map(|h| self.handoff(parent, &id, h, owner))
                .collect();
            let definition = format!(
                "{}::{}",
                self.modules.list[module].rust_path(),
                component.name.name
            );
            components.push(self.component(id, start, item, component, handoffs, definition));
        }
        components
    }

    fn handoff(
        &mut self,
        parent: &str,
        id: &str,
        handoff: &ast::Handoff,
        owner: Option<&ast::Component>,
    ) -> Option<Handoff> {
        let field = handoff.field.name.clone();
        let target = format!("{id}/field:{field}");
        match &handoff.value {
            HandoffValue::Move(place) => {
                let parts: Vec<&str> = place.parts.iter().map(|p| p.name.as_str()).collect();
                let from = match (owner, parts.as_slice()) {
                    (None, [grant]) => format!("{}/grant:{grant}", self.app),
                    (Some(_), [name]) => format!("{parent}/field:{name}"),
                    _ => {
                        self.unsupported("this handoff", handoff.span, "handed off here");
                        return None;
                    }
                };
                self.moved_from.insert(target, from.clone());
                Some(Handoff::Move { field, from })
            }
            // A server the parent lends, from one of its fields.
            HandoffValue::Lend(name)
                if owner.is_some_and(|c| c.fields.iter().any(|f| f.name.name == name.name)) =>
            {
                let from = format!("{parent}/field:{}", name.name);
                self.moved_from.insert(target, from.clone());
                Some(Handoff::Lend { field, from })
            }
            HandoffValue::Lend(sibling) => Some(Handoff::Lend {
                field,
                from: format!("{parent}/{}", sibling.name),
            }),
            HandoffValue::Connect {
                target: to,
                message,
            } => {
                let to = if to.name == "self" {
                    format!("{parent}/fn:{}", message.name)
                } else {
                    format!("{parent}/{}/fn:{}", to.name, message.name)
                };
                // The picture draws the connection from the outlet.
                self.locate(&format!("{target}->{to}"), handoff.span);
                Some(Handoff::Connect { field, to })
            }
            HandoffValue::Expr(expr) => {
                // A name alone is a field of the parent, or a setting of the
                // app, whose value is copied. Anything else is a new value.
                let copied = match &expr.kind {
                    ExprKind::Path(p) if p.segments.len() == 1 => {
                        let name = &p.segments[0].ident.name;
                        match owner {
                            Some(c) => c
                                .fields
                                .iter()
                                .any(|f| &f.name.name == name)
                                .then(|| format!("{parent}/field:{name}")),
                            None => self
                                .app_settings
                                .contains(name.as_str())
                                .then(|| format!("{}/setting:{name}", self.app)),
                        }
                    }
                    _ => None,
                };
                Some(match copied {
                    Some(from) => Handoff::Copy { field, from },
                    None => Handoff::New {
                        field,
                        text: self.text(expr.span),
                    },
                })
            }
        }
    }

    fn component(
        &mut self,
        id: String,
        start: &'a ast::Start,
        item: &'a ast::Item,
        component: &'a ast::Component,
        start_handoffs: Vec<Handoff>,
        definition: String,
    ) -> Component {
        self.locate(&id, component.name.span);
        let fields: Vec<Field> = component
            .fields
            .iter()
            .map(|f| {
                let kind = field_kind(&f.ty);
                let handed_down = f
                    .attrs
                    .iter()
                    .any(|a| matches!(a.kind, AttributeKind::HandedDown));
                let field_id = format!("{id}/field:{}", f.name.name);
                self.locate(&field_id, f.name.span);
                if kind == FieldKind::Capability && !handed_down {
                    self.used_capabilities.push(field_id.clone());
                    self.operations.entry(field_id.clone()).or_default();
                }
                Field {
                    id: field_id,
                    name: f.name.name.clone(),
                    ty: self.text(f.ty.span),
                    kind,
                    handed_down,
                    doc: doc(&f.docs),
                }
            })
            .collect();
        let failure = component
            .failure
            .iter()
            .flat_map(|f| &f.arms)
            .map(|arm| self.failure_arm(&id, component, &start_handoffs, arm))
            .collect();
        let children = self.starts(&id, &component.starts, Some(component));
        let mut routes = Vec::new();
        let mut messages = Vec::new();
        let mut algorithms = Vec::new();
        // Whom the component's functions can send messages to: its named
        // children, and the siblings lent to it.
        let mut handles: Vec<(String, String, &ast::Component)> = Vec::new();
        for start in &component.starts {
            let (Some(binding), Some(child)) = (
                &start.binding,
                self.refs
                    .get(&start.component.span)
                    .and_then(|target| self.components.get(target)),
            ) else {
                continue;
            };
            handles.push((
                binding.name.clone(),
                format!("{id}/{}", binding.name),
                child.1,
            ));
        }
        let scope = Scope {
            component: &id,
            definition: component,
            handles,
        };
        for function in &component.functions {
            let route = function.attrs.iter().find_map(|a| match &a.kind {
                AttributeKind::Route(r) => Some(r),
                _ => None,
            });
            let is_message = function
                .attrs
                .iter()
                .any(|a| matches!(a.kind, AttributeKind::Message));
            let handler = self.function(&id, function, &function.docs, Some(&scope));
            if let Some(route) = route {
                routes.push(Route {
                    method: method(route.method),
                    path: route.path.value.clone(),
                    via: format!("{id}/field:{}", route.via.name),
                    body: route.body.as_ref().map(|b| b.name.clone()),
                    handler,
                });
            } else if is_message {
                messages.push(handler);
            } else {
                algorithms.push(handler);
            }
        }
        Component {
            name: start.binding.as_ref().map(|b| b.name.clone()),
            ty: component.name.name.clone(),
            definition,
            doc: doc(&item.docs),
            external: external(item),
            start: start_handoffs,
            fields,
            failure,
            routes,
            messages,
            algorithms,
            components: children,
            id,
        }
    }

    fn failure_arm(
        &mut self,
        id: &str,
        component: &ast::Component,
        handoffs: &[Handoff],
        arm: &ast::FailureArm,
    ) -> FailureArm {
        // A message failure names a child, or a sibling lent to a field.
        let message_target = |name: &str| {
            handoffs
                .iter()
                .find_map(|h| match h {
                    Handoff::Lend { field, from } if field == name => Some(from.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| format!("{id}/{name}"))
        };
        let (condition, segment) = match &arm.condition.kind {
            FailureConditionKind::Panic => (Condition::Panic, "panic".to_string()),
            FailureConditionKind::Overflow => (Condition::Overflow, "overflow".to_string()),
            FailureConditionKind::Capability(field) => (
                Condition::Capability {
                    field: format!("{id}/field:{}", field.name),
                },
                format!("capability:{}", field.name),
            ),
            FailureConditionKind::Message(name) => {
                let is_child = component
                    .starts
                    .iter()
                    .any(|s| s.binding.as_ref().is_some_and(|b| b.name == name.name));
                let target = if is_child {
                    format!("{id}/{}", name.name)
                } else {
                    message_target(&name.name)
                };
                (
                    Condition::Message { component: target },
                    format!("message:{}", name.name),
                )
            }
            FailureConditionKind::Child(name) => (
                Condition::Child {
                    component: format!("{id}/{}", name.name),
                },
                format!("child:{}", name.name),
            ),
            FailureConditionKind::Other => (Condition::Any, "any".to_string()),
        };
        let arm_id = format!("{id}/failure:{segment}");
        self.locate(&arm_id, arm.condition.span);
        FailureArm {
            id: arm_id,
            condition,
            response: match arm.response {
                FailureResponse::Rollback => Response::Rollback,
                FailureResponse::Restart => Response::Restart,
            },
            reason: doc(&arm.docs).unwrap_or_default(),
            defensive: arm.defensive.as_ref().map(|d| d.reason.value.clone()),
        }
    }

    fn function(
        &mut self,
        owner: &str,
        function: &'a ast::Function,
        docs: &[ast::Doc],
        scope: Option<&Scope<'_>>,
    ) -> Function {
        let id = format!("{owner}/fn:{}", function.name.name);
        self.locate(&id, function.name.span);
        let returns = function.ret.as_ref().map(|t| self.text(t.span));
        // An external component's messages are signatures, with no flowchart.
        let external = scope.is_some_and(|s| s.definition.external.is_some());
        let body = (!external).then(|| self.flowchart(&id, function, scope));
        Function {
            name: function.name.name.clone(),
            doc: doc(docs),
            receiver: function.receiver.as_ref().map(|r| match r.mutable {
                true => Receiver::Mut,
                false => Receiver::Ref,
            }),
            is_async: function.is_async,
            params: function
                .params
                .iter()
                .map(|p| Param {
                    name: self.text(p.pattern.span),
                    ty: self.text(p.ty.span),
                })
                .collect(),
            returns,
            body,
            id,
        }
    }

    /// An edge from each capability field a component uses to its grant,
    /// in the order the components are drawn.
    fn capability_edges(&mut self) -> Vec<Edge> {
        let mut edges = Vec::new();
        for field in &self.used_capabilities {
            let mut from = field;
            while let Some(next) = self.moved_from.get(from) {
                from = next;
            }
            if !from.contains("/grant:") {
                continue;
            }
            let id = format!("{field}->{from}");
            self.sources.insert(id.clone(), self.sources[field]);
            // The edge's steps link to it by this ID.
            edges.push(Edge::Capability {
                id,
                from: field.clone(),
                to: from.clone(),
                operations: self.operations[field].iter().cloned().collect(),
            });
        }
        edges
    }
}

/// Where an external component's code comes from, from its `#[rust]`.
fn external(item: &ast::Item) -> Option<External> {
    item.attrs.iter().find_map(|a| match &a.kind {
        AttributeKind::Rust(rust) => Some(External {
            language: "rust".into(),
            path: rust.path.value.clone(),
            crates: rust.crates.iter().map(|c| c.value.clone()).collect(),
        }),
        _ => None,
    })
}

/// The component a function belongs to, for resolving its calls.
struct Scope<'a> {
    component: &'a str,
    definition: &'a ast::Component,
    /// For each field that holds a handle, by its name, the ID of the
    /// component behind it, and that component's declaration.
    handles: Vec<(String, String, &'a ast::Component)>,
}

fn collect_routes<'c>(components: &'c [Component], out: &mut Vec<&'c Route>) {
    for component in components {
        out.extend(&component.routes);
        collect_routes(&component.components, out);
    }
}

fn doc(docs: &[ast::Doc]) -> Option<String> {
    (!docs.is_empty()).then(|| {
        docs.iter()
            .map(|d| d.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    })
}

fn field_kind(ty: &ast::Type) -> FieldKind {
    let last = |ty: &ast::Type| match &ty.kind {
        TypeKind::Path(p) => p.segments.last().map(|s| s.ident.name.clone()),
        _ => None,
    };
    match &ty.kind {
        TypeKind::Ref { inner, .. } if last(inner).as_deref() == Some("Handle") => {
            FieldKind::Handle
        }
        // A server the parent lends.
        TypeKind::Ref { inner, .. } if last(inner).as_deref() == Some("HttpServer") => {
            FieldKind::Capability
        }
        _ => match last(ty).as_deref() {
            Some("HttpServer" | "HttpClient" | "Table") => FieldKind::Capability,
            Some("Outlet") => FieldKind::Outlet,
            _ => FieldKind::State,
        },
    }
}

fn method(method: ast::HttpMethod) -> Method {
    match method {
        ast::HttpMethod::Get => Method::Get,
        ast::HttpMethod::Post => Method::Post,
        ast::HttpMethod::Put => Method::Put,
        ast::HttpMethod::Patch => Method::Patch,
        ast::HttpMethod::Delete => Method::Delete,
    }
}

/// Source text on one line: each line trimmed, and joined by a space, or by
/// nothing where a method chain continues or a bracket opens or closes.
fn one_line(text: &str) -> String {
    let mut out = String::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let glued =
            out.is_empty() || out.ends_with(['(', '[']) || line.starts_with(['.', ')', ']', '?']);
        if line.starts_with([')', ']']) && out.ends_with(',') {
            out.pop();
        }
        if !glued {
            out.push(' ');
        }
        out.push_str(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::one_line;

    #[test]
    fn source_text_is_joined_onto_one_line() {
        assert_eq!(
            one_line("a\n    .map(|w| {\n        w.len()\n    })\n    .collect()"),
            "a.map(|w| { w.len() }).collect()"
        );
        assert_eq!(one_line("f(\n    a,\n    b,\n)"), "f(a, b)");
    }
}
