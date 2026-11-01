//! What each name in a program refers to, which an editor shows on hover and
//! follows to a definition.
//!
//! The declarations of the code are [`Symbol`]s, and so is every variable a
//! body binds. A [`Reference`] is a span that names a symbol. Names in bodies
//! and requirement steps are recorded as the checker resolves them, so they
//! refer to exactly what the check used. The `start` statements, grants,
//! route attributes and `failure` arms are walked here, since they resolve
//! by name alone.

use std::collections::HashMap;

use inset_syntax::ast::*;
use inset_syntax::{Ident, Span};

use crate::{Checker, FnKind, names};

/// The symbols of a program, and the references to them.
#[derive(Clone, Debug, Default)]
pub struct Index {
    pub symbols: Vec<Symbol>,
    pub references: Vec<Reference>,
}

/// Something declared in the code.
#[derive(Clone, Debug)]
pub struct Symbol {
    /// The file it is declared in, by its index in the program.
    pub file: usize,
    /// Its name, with the span in that file.
    pub name: Ident,
    pub kind: SymbolKind,
    /// The whole declaration, without its doc comment.
    pub span: Span,
    /// What hovering over it shows, as Markdown: its declaration, its doc
    /// comment, and what it holds or serves.
    pub hover: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolKind {
    App,
    Setting,
    Grant,
    Component,
    Field,
    /// A component started under a name.
    Child,
    Route,
    Message,
    Algorithm,
    Struct,
    Enum,
    Variant,
    Module,
    /// A parameter or a variable.
    Variable,
}

/// A name in some file that refers to a symbol.
#[derive(Clone, Copy, Debug)]
pub struct Reference {
    pub file: usize,
    pub span: Span,
    /// The symbol's position in [`Index::symbols`].
    pub symbol: usize,
}

impl Index {
    /// The symbol named at a byte offset of a file: the one the smallest
    /// reference there refers to, or the one declared there.
    pub fn at(&self, file: usize, offset: usize) -> Option<&Symbol> {
        let contains = |span: Span| span.start <= offset && offset <= span.end;
        let reference = self
            .references
            .iter()
            .filter(|r| r.file == file && contains(r.span))
            .min_by_key(|r| r.span.end - r.span.start);
        if let Some(reference) = reference {
            return Some(&self.symbols[reference.symbol]);
        }
        self.symbols
            .iter()
            .find(|s| s.file == file && contains(s.name.span))
    }
}

/// Builds the index from a checked program. `refs` holds the references
/// the requirements make, each with its file and the name span of its target.
pub(crate) fn build(cx: &Checker, refs: Vec<(usize, Span, Span)>) -> Index {
    let modules = cx.names.modules;
    let mut builder = Builder {
        cx,
        symbols: Vec::new(),
        refs: Vec::new(),
    };
    builder.items();
    for (name, ty) in &cx.locals {
        let code = if ty.is_known() {
            format!("{}: {ty}", name.name)
        } else {
            name.name.clone()
        };
        builder.symbol(
            name,
            SymbolKind::Variable,
            name.span,
            hover(&code, &[], &[]),
        );
    }
    let by_name: HashMap<Span, usize> = builder
        .symbols
        .iter()
        .enumerate()
        .map(|(i, s)| (s.name.span, i))
        .collect();
    let names_refs = cx.names.refs.borrow();
    let code_refs = cx
        .refs
        .iter()
        .chain(&builder.refs)
        .chain(names_refs.iter())
        .filter_map(|&(span, target)| {
            let (file, span) = modules.locate(span)?;
            Some((file, span, target))
        });
    let mut references: Vec<Reference> = code_refs
        .chain(refs)
        .filter_map(|(file, span, target)| {
            Some(Reference {
                file,
                span,
                symbol: *by_name.get(&target)?,
            })
        })
        .collect();
    references.sort_by_key(|r| (r.file, r.span.start, r.span.end));
    references.dedup_by_key(|r| (r.file, r.span, r.symbol));
    let symbols = builder
        .symbols
        .into_iter()
        .filter_map(|mut symbol| {
            let (file, name) = modules.locate(symbol.name.span)?;
            symbol.file = file;
            symbol.name.span = name;
            symbol.span = modules.locate(symbol.span)?.1;
            Some(symbol)
        })
        .collect();
    Index {
        symbols,
        references,
    }
}

struct Builder<'c, 'a> {
    cx: &'c Checker<'a>,
    symbols: Vec<Symbol>,
    /// References in the code, each with the name span of its target.
    refs: Vec<(Span, Span)>,
}

/// The names a `start` statement can hand off: in the app, its settings,
/// grants and children; in a component, its fields and children.
struct Scope<'a> {
    names: Vec<&'a Ident>,
    starts: &'a [Start],
    /// The component doing the starting.
    component: Option<&'a Component>,
}

impl Scope<'_> {
    fn find(&self, name: &str) -> Option<Span> {
        self.names.iter().find(|n| n.name == name).map(|n| n.span)
    }
}

impl<'a> Builder<'_, 'a> {
    fn symbol(&mut self, name: &Ident, kind: SymbolKind, span: Span, hover: String) {
        self.symbols.push(Symbol {
            file: 0,
            name: name.clone(),
            kind,
            span,
            hover,
        });
    }

    fn refer(&mut self, span: Span, target: Span) {
        self.refs.push((span, target));
    }

    /// The source text of a span.
    fn source(&self, span: Span) -> &'a str {
        self.cx.names.modules.text(span).trim()
    }

    fn items(&mut self) {
        let modules = self.cx.names.modules;
        for (index, module) in modules.list.iter().enumerate() {
            for item in &module.ast.items {
                let span = Span::new(
                    item.docs.last().map_or(item.span.start, |d| d.span.end),
                    item.span.end,
                );
                match &item.kind {
                    ItemKind::App(app) => self.app(app, &item.docs, span),
                    ItemKind::Component(component) => self.component(component, &item.docs, span),
                    ItemKind::Function(function) => self.function(function, None),
                    ItemKind::Struct(_) | ItemKind::Enum(_) => self.data(item, span),
                    ItemKind::Mod(name) => {
                        let child = modules.list.iter().find(|m| {
                            m.parent
                                .as_ref()
                                .is_some_and(|(p, decl)| *p == index && decl.span == name.span)
                        });
                        let note = match child {
                            Some(child) => format!(
                                "The module `{}`, in `{}`.",
                                child.rust_path(),
                                child.file_path
                            ),
                            None => "A module with no file.".to_string(),
                        };
                        let code = format!("mod {}", name.name);
                        let hover = hover(&code, &item.docs, &[note]);
                        self.symbol(name, SymbolKind::Module, span, hover);
                    }
                    ItemKind::Use(_) => {}
                }
            }
        }
    }

    /// A struct or enum, its fields and its variants.
    fn data(&mut self, item: &'a Item, span: Span) {
        let Some(index) = self
            .cx
            .names
            .datas
            .iter()
            .position(|d| std::ptr::eq(d.item, item))
        else {
            return;
        };
        let data = &self.cx.datas[index];
        let kind = if data.is_enum { "enum" } else { "struct" };
        let mut notes = Vec::new();
        if !data.derives.is_empty() {
            notes.push(format!(
                "Derives {}.",
                names(data.derives.iter().map(String::as_str))
            ));
        }
        if let Some(statuses) = data.statuses() {
            let mut unique: Vec<&str> = Vec::new();
            for status in &statuses {
                if !unique.contains(&status.as_str()) {
                    unique.push(status);
                }
            }
            notes.push(format!("An error that replies {}.", names(unique)));
        }
        let json = data.derives("Serialize") || data.derives("Deserialize");
        let code = format!("{kind} {}", data.name.name);
        let symbol_kind = if data.is_enum {
            SymbolKind::Enum
        } else {
            SymbolKind::Struct
        };
        self.symbol(
            &data.name,
            symbol_kind,
            span,
            hover(&code, &item.docs, &notes),
        );
        match &item.kind {
            ItemKind::Struct(s) => {
                for (i, (field, info)) in s.fields.iter().zip(&data.fields).enumerate() {
                    let code = format!("{}: {}", field.name.name, self.source(field.ty.span));
                    let ty = info.ty.to_string();
                    let mut note = format!("A field of `{}`", data.name.name);
                    if ty != self.source(field.ty.span) && info.ty.is_known() {
                        note.push_str(&format!(", a `{ty}`"));
                    }
                    note.push('.');
                    let mut notes = vec![note];
                    if json && let Some(name) = data.json.get(i) {
                        notes.push(format!("Written `{name}` in JSON."));
                    }
                    let hover = hover(&code, &field.docs, &notes);
                    self.symbol(&field.name, SymbolKind::Field, field.span, hover);
                }
            }
            ItemKind::Enum(e) => {
                for (i, (variant, info)) in e.variants.iter().zip(&data.variants).enumerate() {
                    let code = format!(
                        "{}::{}",
                        data.name.name,
                        self.source(Span::new(variant.name.span.start, variant.span.end))
                    );
                    let mut notes = match &info.status {
                        Some(status) => vec![format!("Replies `{}`.", status.name)],
                        None => Vec::new(),
                    };
                    if json && let Some(name) = data.json.get(i) {
                        notes.push(format!("Written `{name}` in JSON."));
                    }
                    let hover = hover(&code, &variant.docs, &notes);
                    self.symbol(&variant.name, SymbolKind::Variant, variant.span, hover);
                }
            }
            _ => {}
        }
    }

    fn app(&mut self, app: &'a App, docs: &[Doc], span: Span) {
        let grants: Vec<String> = app
            .grants
            .iter()
            .map(|g| format!("{}: {}", g.name.name, self.source(g.ty.span)))
            .collect();
        let notes = if grants.is_empty() {
            vec!["Grants no capabilities.".to_string()]
        } else {
            vec![format!(
                "Grants {}.",
                names(grants.iter().map(String::as_str))
            )]
        };
        let code = format!("app {}", app.name.name);
        self.symbol(&app.name, SymbolKind::App, span, hover(&code, docs, &notes));
        for setting in &app.settings {
            let code = self.source(setting.span).to_string();
            let hover = hover(&code, &setting.docs, &["A setting of the app.".into()]);
            self.symbol(&setting.name, SymbolKind::Setting, setting.span, hover);
        }
        for grant in &app.grants {
            let code = self.source(grant.span).to_string();
            let notes = ["A capability the app grants.".to_string()];
            let hover = hover(&code, &grant.docs, &notes);
            self.symbol(&grant.name, SymbolKind::Grant, grant.span, hover);
            for field in &grant.fields {
                let setting = match &field.value {
                    None => Some(&field.name),
                    Some(Expr {
                        kind: ExprKind::Path(path),
                        ..
                    }) if path.segments.len() == 1 => Some(&path.segments[0].ident),
                    Some(_) => None,
                };
                if let Some(name) = setting
                    && let Some(target) = app.settings.iter().find(|s| s.name.name == name.name)
                {
                    self.refer(name.span, target.name.span);
                }
            }
        }
        let names = app
            .settings
            .iter()
            .map(|s| &s.name)
            .chain(app.grants.iter().map(|g| &g.name))
            .chain(app.starts.iter().filter_map(|s| s.binding.as_ref()))
            .collect();
        let scope = Scope {
            names,
            starts: &app.starts,
            component: None,
        };
        self.starts(&scope);
    }

    fn component(&mut self, component: &'a Component, docs: &[Doc], span: Span) {
        let comp = self
            .cx
            .components
            .iter()
            .find(|c| std::ptr::eq(c.ast, component));
        let mut notes = Vec::new();
        if let Some(comp) = comp {
            let held: Vec<String> = comp
                .fields
                .iter()
                .filter(|f| f.ty.is_capability())
                .map(|f| format!("{}: {}", f.decl.name.name, f.ty))
                .collect();
            notes.push(if held.is_empty() {
                "Holds no capabilities.".to_string()
            } else {
                format!("Holds {}.", names(held.iter().map(String::as_str)))
            });
            let routes: Vec<String> = comp
                .routes
                .iter()
                .map(|r| format!("{} {}", r.method.as_str(), r.template))
                .collect();
            if !routes.is_empty() {
                notes.push(format!(
                    "Serves {}.",
                    names(routes.iter().map(String::as_str))
                ));
            }
            let messages: Vec<&str> = comp
                .functions
                .iter()
                .filter(|f| f.kind == FnKind::Message)
                .map(|f| f.name())
                .collect();
            if !messages.is_empty() {
                notes.push(format!("Takes the messages {}.", names(messages)));
            }
        }
        let rust = self
            .cx
            .names
            .components
            .iter()
            .find(|d| std::ptr::eq(d.ast, component))
            .and_then(|d| {
                d.item.attrs.iter().find_map(|a| match &a.kind {
                    AttributeKind::Rust(rust) => Some(rust),
                    _ => None,
                })
            });
        if let Some(rust) = rust {
            let crates: Vec<&str> = rust.crates.iter().map(|c| c.value.as_str()).collect();
            let with = if crates.is_empty() {
                String::new()
            } else {
                format!(", with the crates {}", names(crates))
            };
            notes.insert(
                0,
                format!("Written in Rust, in `{}`{with}.", rust.path.value),
            );
        }
        let code = if component.external.is_some() {
            format!("extern component {}", component.name.name)
        } else {
            format!("component {}", component.name.name)
        };
        self.symbol(
            &component.name,
            SymbolKind::Component,
            span,
            hover(&code, docs, &notes),
        );
        for decl in &component.fields {
            let field = comp.and_then(|c| c.field(&decl.name.name));
            let owner = &component.name.name;
            let notes = [match field {
                Some(f) if f.handed_down => {
                    format!("A field `{owner}` hands down to a child, and does not use.")
                }
                Some(f) if f.ty.is_capability() => format!("A capability `{owner}` holds."),
                _ => format!("A field of `{owner}`."),
            }];
            let code = format!("{}: {}", decl.name.name, self.source(decl.ty.span));
            let hover = hover(&code, &decl.docs, &notes);
            self.symbol(&decl.name, SymbolKind::Field, decl.span, hover);
        }
        if let Some(failure) = &component.failure {
            for arm in &failure.arms {
                let name = match &arm.condition.kind {
                    FailureConditionKind::Capability(name)
                    | FailureConditionKind::Message(name)
                    | FailureConditionKind::Child(name) => name,
                    _ => continue,
                };
                let target = component
                    .fields
                    .iter()
                    .map(|f| &f.name)
                    .chain(component.starts.iter().filter_map(|s| s.binding.as_ref()))
                    .find(|n| n.name == name.name);
                if let Some(target) = target {
                    self.refer(name.span, target.span);
                }
            }
        }
        for function in &component.functions {
            self.function(function, Some(component));
        }
        let names = component
            .fields
            .iter()
            .map(|f| &f.name)
            .chain(component.starts.iter().filter_map(|s| s.binding.as_ref()))
            .collect();
        let scope = Scope {
            names,
            starts: &component.starts,
            component: Some(component),
        };
        self.starts(&scope);
    }

    fn function(&mut self, function: &'a Function, component: Option<&'a Component>) {
        let header_start = function
            .docs
            .last()
            .map_or(function.span.start, |d| d.span.end);
        let code = self.source(Span::new(header_start, function.body.span.start));
        // Later lines lose the indentation of the first, as `fn` sits in a
        // component's body.
        let modules = self.cx.names.modules;
        let module = &modules.list[modules.at(header_start).unwrap_or(0)];
        let text = &module.text[..header_start - module.base];
        let line_start = text.rfind('\n').map_or(0, |i| i + 1);
        let indent = text[line_start..].chars().take_while(|c| *c == ' ').count();
        let code = code
            .lines()
            .map(|line| {
                let spaces = line.len() - line.trim_start_matches(' ').len();
                &line[spaces.min(indent)..]
            })
            .collect::<Vec<_>>()
            .join("\n");
        let sig = match component {
            Some(c) => self
                .cx
                .components
                .iter()
                .find(|comp| std::ptr::eq(comp.ast, c))
                .and_then(|comp| comp.function(&function.name.name)),
            None => self
                .cx
                .algorithms
                .iter()
                .find(|a| std::ptr::eq(a.ast, function)),
        };
        let kind = match sig.map(|s| s.kind) {
            Some(FnKind::Route) => SymbolKind::Route,
            Some(FnKind::Message) => SymbolKind::Message,
            _ => SymbolKind::Algorithm,
        };
        let owner = component.map(|c| c.name.name.as_str());
        let note = match (kind, owner) {
            (SymbolKind::Route, Some(owner)) => {
                let grant = self
                    .cx
                    .routes
                    .iter()
                    .find(|r| r.decl.function == function.name.span)
                    .and_then(|r| r.grant.as_deref());
                match grant {
                    Some(grant) => format!(
                        "A route of `{owner}`, whose requests arrive through the grant `{grant}`."
                    ),
                    None => format!("A route of `{owner}`."),
                }
            }
            (SymbolKind::Message, Some(owner)) => format!("A message of `{owner}`."),
            (_, Some(owner)) => format!("An algorithm of `{owner}`."),
            (_, None) => "An algorithm.".to_string(),
        };
        let span = Span::new(header_start, function.span.end);
        let hover = hover(&code, &function.docs, &[note]);
        self.symbol(&function.name, kind, span, hover);
        for attr in &function.attrs {
            if let AttributeKind::Route(route) = &attr.kind {
                self.route(route, function, component);
            }
        }
    }

    /// The names in a route attribute: its `via` field, and the parameters
    /// its path and body bind.
    fn route(&mut self, route: &Route, function: &Function, component: Option<&Component>) {
        if let Some(field) =
            component.and_then(|c| c.fields.iter().find(|f| f.name.name == route.via.name))
        {
            self.refer(route.via.span, field.name.span);
        }
        let param = |name: &str| {
            function.params.iter().find_map(|p| match &p.pattern.kind {
                PatternKind::Binding { name: n, .. } if n.name == name => Some(n.span),
                _ => None,
            })
        };
        if let Some(body) = &route.body
            && let Some(target) = param(&body.name)
        {
            self.refer(body.span, target);
        }
        // A path written without escapes has each character of its value in
        // the source, so a `{name}` in it can be pointed at.
        let path = &route.path;
        if path.span.end - path.span.start != path.value.len() + 2 {
            return;
        }
        let mut at = 0;
        while let Some(open) = path.value[at..].find('{').map(|i| at + i) {
            let Some(close) = path.value[open..].find('}').map(|i| open + i) else {
                break;
            };
            let name = &path.value[open + 1..close];
            if let Some(target) = param(name) {
                let start = path.span.start + 1 + open + 1;
                self.refer(Span::new(start, start + name.len()), target);
            }
            at = close;
        }
    }

    fn starts(&mut self, scope: &Scope<'a>) {
        for start in scope.starts {
            let started = self
                .cx
                .started(start)
                .map(|c| self.cx.names.components[c].ast);
            if let Some(binding) = &start.binding {
                let code = self.source(start.span).to_string();
                let notes = [match scope.component {
                    Some(c) => format!("A child of `{}`.", c.name.name),
                    None => "A component the app starts.".to_string(),
                }];
                let hover = hover(&code, &start.docs, &notes);
                self.symbol(binding, SymbolKind::Child, start.span, hover);
            }
            let Some(started) = started else {
                continue;
            };
            self.refer(start.component.span, started.name.span);
            for handoff in &start.handoffs {
                if let Some(field) = started
                    .fields
                    .iter()
                    .find(|f| f.name.name == handoff.field.name)
                {
                    self.refer(handoff.field.span, field.name.span);
                }
                match &handoff.value {
                    HandoffValue::Move(place) => self.scope_name(scope, &place.parts[0]),
                    HandoffValue::Lend(name) => self.scope_name(scope, name),
                    HandoffValue::Connect { target, message } => {
                        let receiver = if target.name == "self" {
                            scope.component
                        } else {
                            self.scope_name(scope, target);
                            scope
                                .starts
                                .iter()
                                .find(|s| s.binding.as_ref().is_some_and(|b| b.name == target.name))
                                .and_then(|s| self.cx.started(s))
                                .map(|c| self.cx.names.components[c].ast)
                        };
                        if let Some(function) = receiver
                            .and_then(|c| c.functions.iter().find(|f| f.name.name == message.name))
                        {
                            self.refer(message.span, function.name.span);
                        }
                    }
                    HandoffValue::Expr(Expr {
                        kind: ExprKind::Path(path),
                        ..
                    }) if path.segments.len() == 1 => {
                        self.scope_name(scope, &path.segments[0].ident);
                    }
                    HandoffValue::Expr(_) => {}
                }
            }
        }
    }

    fn scope_name(&mut self, scope: &Scope, name: &Ident) {
        if let Some(target) = scope.find(&name.name) {
            self.refer(name.span, target);
        }
    }
}

/// Hover text: the declaration as code, its doc comment, then notes.
fn hover(code: &str, docs: &[Doc], notes: &[String]) -> String {
    let mut out = format!("```inset\n{code}\n```");
    if !docs.is_empty() {
        let docs: Vec<&str> = docs
            .iter()
            .map(|d| d.text.strip_prefix(' ').unwrap_or(&d.text))
            .collect();
        out.push_str("\n\n");
        out.push_str(&docs.join("\n"));
    }
    if !notes.is_empty() {
        out.push_str("\n\n");
        out.push_str(&notes.join(" "));
    }
    out
}
