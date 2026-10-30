//! Inset's semantic checks, which reject a program whose diagrams would
//! not be true to its source.
//!
//! [`check`] parses every file of a [`Program`] with `inset-syntax` and then
//! checks what parsed: names resolve, types fit, each component uses only the
//! capabilities it was handed, values cross component boundaries only as
//! syntax.md allows, and every requirement step resolves to a real route.
//! Each problem is a [`Report`]: a diagnostic in one file. The checks run on
//! whatever parsed, so a parse error does not hide the semantic errors
//! elsewhere, which the language server relies on.
//!
//! The type check is partial on purpose. A type the checker cannot work out
//! is unknown and fits anywhere, so it reports only mismatches it is sure
//! of. Rust's own checks still run on the generated code. The checker covers
//! what Hello HTTP uses, and the structure of components, outlets, messages
//! and `failure` blocks around it, because those decide what the diagram
//! draws.
//!
//! The work is split by what is checked:
//!
//! - `items`: the app, components, fields, function kinds, routes and
//!   `failure` blocks;
//! - `wiring`: settings, grants, `start` statements and their handoffs, and
//!   which grant reaches each route;
//! - `body`: names and types in function bodies, closures, messages and
//!   capability calls;
//! - `requirements`: requirement steps against the app's routes.
//!
//! Diagnostic wording is pinned by `insta` snapshot tests in `tests/`.

mod body;
mod items;
mod program;
mod requirements;
pub mod status;
mod types;
mod wiring;

use inset_syntax::ast::{self, HttpMethod};
use inset_syntax::{Diagnostic, Ident, Span, is_requirements_file, parse_code, parse_requirements};

pub use program::{MAIN, Program, SourceFile};
use types::Ty;

/// A diagnostic in one file of a program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    /// The file's index in [`Program::files`].
    pub file: usize,
    pub diagnostic: Diagnostic,
}

/// Parses and checks every file of the program. The reports are in order of
/// file and then of position.
pub fn check(program: &Program) -> Vec<Report> {
    let mut reports = Vec::new();
    let mut code = None;
    let mut requirement_files = Vec::new();
    for (index, file) in program.files.iter().enumerate() {
        let mut report = |diagnostic| {
            reports.push(Report {
                file: index,
                diagnostic,
            })
        };
        let is_req = is_requirements_file(&file.path);
        if file.path.starts_with("src/") {
            if is_req {
                report(misplaced(
                    &file.path,
                    "requirements",
                    "src/",
                    "requirements/",
                ));
            } else if file.path == MAIN {
                let parsed = parse_code(&file.text);
                parsed.diagnostics.into_iter().for_each(&mut report);
                code = Some((index, parsed.ast));
            } else {
                report(
                    Diagnostic::new(
                        "modules are not supported yet",
                        Span::new(0, 0),
                        "this file is not part of the program",
                    )
                    .with_help(format!("move its items into {MAIN}")),
                );
            }
        } else if file.path.starts_with("requirements/") {
            if is_req {
                let parsed = parse_requirements(&file.text);
                parsed.diagnostics.into_iter().for_each(&mut report);
                requirement_files.push((index, parsed.ast));
            } else {
                report(misplaced(&file.path, "code", "requirements/", "src/"));
            }
        }
    }
    let empty = ast::Module { items: Vec::new() };
    let (main, module) = match &code {
        Some((index, module)) => (Some(*index), module),
        None => (None, &empty),
    };
    let mut checker = Checker::new(module);
    checker.check();
    if let Some(main) = main {
        reports.extend(checker.diagnostics.drain(..).map(|diagnostic| Report {
            file: main,
            diagnostic,
        }));
    }
    reports.extend(requirements::check(&checker, program, &requirement_files));
    reports.sort_by_key(|r| (r.file, r.diagnostic.span.start));
    reports
}

fn misplaced(path: &str, what: &str, here: &str, there: &str) -> Diagnostic {
    let name = path.rsplit('/').next().unwrap_or(path);
    Diagnostic::new(
        format!("a {what} file in `{here}`"),
        Span::new(0, 0),
        format!("{what} belong in `{there}`"),
    )
    .with_help(format!("move `{name}` to `{there}`"))
}

/// What the checker knows about the program's code, built up as it checks.
pub(crate) struct Checker<'a> {
    pub app: Option<&'a ast::App>,
    pub components: Vec<Comp<'a>>,
    /// The algorithms at the top level of the module.
    pub algorithms: Vec<Sig<'a>>,
    /// Every route of every running component, with the grant it serves.
    pub routes: Vec<RouteInfo>,
    pub diagnostics: Vec<Diagnostic>,
    module: &'a ast::Module,
}

/// A component and what it declares.
pub(crate) struct Comp<'a> {
    pub ast: &'a ast::Component,
    pub fields: Vec<Field<'a>>,
    pub functions: Vec<Sig<'a>>,
    /// Its routes, in the order of its functions.
    pub routes: Vec<RouteDecl>,
}

pub(crate) struct Field<'a> {
    pub decl: &'a ast::FieldDecl,
    pub ty: Ty,
    pub handed_down: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum FnKind {
    Route,
    Message,
    Algorithm,
}

/// A function's kind and resolved signature.
pub(crate) struct Sig<'a> {
    pub ast: &'a ast::Function,
    pub kind: FnKind,
    pub params: Vec<Ty>,
    pub ret: Ty,
}

impl Sig<'_> {
    pub fn name(&self) -> &str {
        &self.ast.name.name
    }
}

/// A segment of a route's path.
#[derive(Clone, Debug)]
pub(crate) enum Segment {
    Literal(String),
    /// `{name}`, with the type of the parameter it binds.
    Param(Ty),
}

/// How a route's return type replies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reply {
    Text,
    Optional,
    /// `Result<String, E>`, whose statuses are not known yet.
    Fallible,
}

/// A route as its component declares it.
#[derive(Clone, Debug)]
pub(crate) struct RouteDecl {
    pub method: HttpMethod,
    pub template: String,
    pub segments: Vec<Segment>,
    /// The `HttpServer` field named by `via`.
    pub via: String,
    /// Whether a body is read and converted, which can fail.
    pub body: bool,
    pub reply: Reply,
    pub span: Span,
}

/// A route of a running component, and the grant its requests arrive through.
#[derive(Clone, Debug)]
pub(crate) struct RouteInfo {
    pub decl: RouteDecl,
    pub component: String,
    pub grant: Option<String>,
}

impl<'a> Checker<'a> {
    fn new(module: &'a ast::Module) -> Checker<'a> {
        Checker {
            app: None,
            components: Vec::new(),
            algorithms: Vec::new(),
            routes: Vec::new(),
            diagnostics: Vec::new(),
            module,
        }
    }

    fn check(&mut self) {
        self.collect_items();
        self.check_app();
        for index in 0..self.components.len() {
            self.check_component_starts(index);
            self.check_failure(index);
        }
        self.check_cycles();
        self.check_bodies();
        self.collect_routes();
    }

    pub(crate) fn error(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    pub(crate) fn component(&self, name: &str) -> Option<usize> {
        self.components.iter().position(|c| c.ast.name.name == name)
    }

    pub(crate) fn component_names(&self) -> Vec<&'a str> {
        self.module
            .items
            .iter()
            .filter_map(|item| match &item.kind {
                ast::ItemKind::Component(c) => Some(c.name.name.as_str()),
                _ => None,
            })
            .collect()
    }
}

impl Comp<'_> {
    pub(crate) fn name(&self) -> &str {
        &self.ast.name.name
    }

    pub(crate) fn field(&self, name: &str) -> Option<&Field<'_>> {
        self.fields.iter().find(|f| f.decl.name.name == name)
    }

    /// The component a child started with `start name: C` runs.
    pub(crate) fn child(&self, name: &str) -> Option<&Ident> {
        self.ast
            .starts
            .iter()
            .find(|s| s.binding.as_ref().is_some_and(|b| b.name == name))
            .map(|s| &s.component)
    }

    pub(crate) fn function(&self, name: &str) -> Option<&Sig<'_>> {
        self.functions.iter().find(|f| f.name() == name)
    }
}

/// `a`, `a and b`, or `a, b and c`, each in backticks.
pub(crate) fn names<'s>(names: impl IntoIterator<Item = &'s str>) -> String {
    let names: Vec<String> = names.into_iter().map(|n| format!("`{n}`")).collect();
    match names.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}
