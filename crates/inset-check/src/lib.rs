//! Inset's semantic checks, which reject a program whose diagrams would
//! not be true to its source.
//!
//! [`check`] parses every file of a [`Program`] with `inset-syntax` and then
//! checks what parsed: names resolve, types fit, each component uses only the
//! capabilities it was handed, values cross component boundaries only as
//! syntax.md allows, and every requirement step resolves to a real route or
//! grant, with each value in it typed. [`plan`] hands the test runner the
//! requirements in that resolved form, and [`plan_with`] also hands it the
//! checked program as [`Cases`], which plans each generated case of a
//! property. Each problem is a [`Report`]: a diagnostic in one file. The
//! checks run on whatever parsed, so a parse error does not hide the semantic errors
//! elsewhere, which the language server relies on.
//!
//! The type check is partial on purpose. A type the checker cannot work out
//! is unknown and fits anywhere, so it reports only mismatches it is sure
//! of. Rust's own checks still run on the generated code, but an error
//! there is a compiler bug, so what rustc would surely refuse, such as a
//! derive a field cannot have or a `match` that leaves out a variant, is
//! reported here. The checker covers the code of the examples: the
//! structure of components, outlets, messages and `failure` blocks, which
//! decide what the diagram draws, the modules and data types around it,
//! and what a branch of a `fork` may not do.
//!
//! A program's files are its modules (`modules`), parsed with spans that
//! are unique across the program, so the checker compares spans without
//! asking which file each is in, and turns them back into a file and an
//! offset only to report. [`resolve`] hands the modules, and what each name
//! resolved to, to the stages that emit from a checked program.
//!
//! The work is split by what is checked:
//!
//! - `modules`: which files are modules, from the `mod` declarations;
//! - `names`: what each module declares and imports, how paths resolve, and
//!   where `pub` lets an item be named from;
//! - `data`: structs and enums, their derives, and error enums' statuses;
//! - `items`: the app, components, fields, function kinds, routes and
//!   `failure` blocks;
//! - `wiring`: settings, grants, `start` statements and their handoffs, and
//!   which grant reaches each route;
//! - `body`: names and types in function bodies, closures, forks, messages
//!   and capability calls;
//! - `defensive`: where `#[defensive]` may mark a branch;
//! - `requirements`: requirement steps against the app's routes and grants,
//!   a property's inputs, and the plan the test runner follows;
//! - `values`: values and checks in requirement facts, and a property's
//!   `where` condition, typed against the program.
//!
//! Diagnostic wording is pinned by `insta` snapshot tests in `tests/`.

mod api;
mod body;
mod data;
mod defensive;
mod external;
mod front;
mod index;
mod items;
mod modules;
mod names;
mod program;
mod requirements;
pub mod status;
mod types;
mod values;
mod wiring;

use std::collections::HashMap;

use inset_syntax::ast::{self, HttpMethod};
use inset_syntax::{Diagnostic, Ident, Span, is_requirements_file, parse_requirements};

pub use api::{Api, ApiBody, ApiError, ApiParam, ApiRoute, ApiSegment, TypeDocs, api};
pub use external::{
    External, ExternalMessage, ExternalParam, ExternalReply, ExternalType, ExternalTypeKind,
    WitType, externals, wit_field_name, wit_type_name,
};
pub use index::{Index, Reference, Symbol, SymbolKind};
pub use modules::{Module, Modules};
use names::Names;
pub use program::{Asset, MAIN, Program, SourceFile};
pub use requirements::{
    Plan, Planned, PlannedBody, PlannedExamples, PlannedFact, PlannedInput, PlannedProperty,
    PlannedRequirement, PlannedRow, PlannedStep,
};
use types::Ty;

/// A diagnostic in one file of a program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    /// The file's index in [`Program::files`].
    pub file: usize,
    pub diagnostic: Diagnostic,
}

/// What checking a program found: its problems, and what each name in it
/// refers to.
#[derive(Clone, Debug)]
pub struct Analysis {
    /// In order of file and then of position.
    pub reports: Vec<Report>,
    pub index: Index,
}

/// Parses and checks every file of the program. The reports are in order of
/// file and then of position.
pub fn check(program: &Program) -> Vec<Report> {
    analyze(program).reports
}

/// A program that checked clean, for the stages that emit from it: its
/// modules, and what each name in its code refers to.
#[derive(Clone, Debug)]
pub struct Resolved {
    pub modules: Modules,
    /// For each name in the code that refers to a declaration, the name of
    /// that declaration. Both spans are program-wide (see [`Modules`]).
    pub refs: HashMap<Span, Span>,
    /// The external components, as their WIT worlds and bindings describe them.
    pub externals: Vec<External>,
}

/// Checks the program, and returns its modules and resolved names if it
/// checks clean, or its reports if not.
pub fn resolve(program: &Program) -> Result<Resolved, Vec<Report>> {
    let (modules, mut reports) = Modules::load(program);
    let (refs, externals) = {
        let mut checker = Checker::new(&modules);
        checker.check();
        finish(
            program,
            &modules,
            &mut checker,
            &mut reports,
            &mut Vec::new(),
        );
        let mut refs: HashMap<Span, Span> = checker.refs.iter().copied().collect();
        refs.extend(checker.names.refs.borrow().iter().copied());
        (refs, external::describe(&checker))
    };
    if !reports.is_empty() {
        return Err(reports);
    }
    Ok(Resolved {
        modules,
        refs,
        externals,
    })
}

/// Parses and checks every file of the program, and indexes its names.
pub fn analyze(program: &Program) -> Analysis {
    let (modules, mut reports) = Modules::load(program);
    let index = {
        let mut checker = Checker::new(&modules);
        checker.check();
        finish(
            program,
            &modules,
            &mut checker,
            &mut reports,
            &mut Vec::new(),
        )
    };
    Analysis { reports, index }
}

/// Checks the program, and returns its requirements as the test runner
/// runs them, with every step resolved and every value typed, or its
/// reports if it does not check clean.
pub fn plan(program: &Program) -> Result<Plan, Vec<Report>> {
    plan_with(program, |plan, _| plan)
}

/// Plans the program's requirements as [`plan`] does, and hands `run` the
/// plan with the checked program, which plans each case of a property.
pub fn plan_with<R>(
    program: &Program,
    run: impl FnOnce(Result<Plan, Vec<Report>>, &Cases<'_, '_>) -> R,
) -> R {
    let (modules, mut reports) = Modules::load(program);
    let mut checker = Checker::new(&modules);
    checker.check();
    let mut requirements = Vec::new();
    finish(
        program,
        &modules,
        &mut checker,
        &mut reports,
        &mut requirements,
    );
    let plan = if reports.is_empty() {
        Ok(Plan {
            app: checker
                .app
                .map_or_else(String::new, |a| a.name.name.clone()),
            types: values::types(&checker),
            requirements,
        })
    } else {
        Err(reports)
    };
    run(
        plan,
        &Cases {
            checker: &checker,
            program,
        },
    )
}

/// A checked program, which plans the steps of a property's cases as the
/// test runner generates them. The steps were checked with a sample value
/// for each input, so a case's values, written as Rust writes them, give
/// steps that check too, unless a value is out of its type's range.
pub struct Cases<'c, 'a> {
    checker: &'c Checker<'a>,
    program: &'c Program,
}

impl Cases<'_, '_> {
    /// The program the cases are of.
    pub fn program(&self) -> &Program {
        self.program
    }

    /// The steps of the property `requirement`, read with `values`, the
    /// text of each input's value in the order of its inputs, and planned.
    /// The first problem found, if they do not check.
    pub fn property(
        &self,
        requirement: &PlannedRequirement,
        values: &[String],
    ) -> Result<Vec<PlannedStep>, Diagnostic> {
        let property = requirement
            .property
            .as_ref()
            .expect("only a property has cases to plan");
        let names: Vec<String> = property.inputs.iter().map(|i| i.name.clone()).collect();
        let text = &self.program.files[requirement.file].text;
        let (steps, mut diagnostics) =
            inset_syntax::read_property_case(text, property.steps, &names, values);
        let planned = requirements::plan_steps(
            self.checker,
            requirement.file,
            &steps,
            &mut Vec::new(),
            |d| diagnostics.push(d),
        );
        match diagnostics.into_iter().min_by_key(|d| d.span.start) {
            Some(diagnostic) => Err(diagnostic),
            None => Ok(planned),
        }
    }
}

/// Checks the requirements against a checked program, and gathers the reports.
fn finish(
    program: &Program,
    modules: &Modules,
    checker: &mut Checker,
    reports: &mut Vec<Report>,
    plans: &mut Vec<PlannedRequirement>,
) -> Index {
    let mut requirement_files = Vec::new();
    for (index, file) in program.files.iter().enumerate() {
        let is_req = is_requirements_file(&file.path);
        let mut report = |diagnostic| {
            reports.push(Report {
                file: index,
                diagnostic,
            })
        };
        if file.path.starts_with("src/") && is_req {
            report(misplaced(
                &file.path,
                "requirements",
                "src/",
                "requirements/",
            ));
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
    let web: Option<Vec<String>> = program
        .web
        .as_ref()
        .map(|web| web.iter().map(|a| a.path.clone()).collect());
    checker.check_web(web.as_deref());
    checker.check_rust_files(program.rust.as_deref());
    reports.extend(
        checker
            .diagnostics
            .drain(..)
            .filter_map(|diagnostic| modules.report(diagnostic)),
    );
    let mut refs = Vec::new();
    reports.extend(requirements::check(
        checker,
        program,
        &requirement_files,
        &mut refs,
        plans,
    ));
    reports.sort_by_key(|r| (r.file, r.diagnostic.span.start));
    reports.dedup();
    index::build(checker, refs)
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
    pub names: Names<'a>,
    pub app: Option<&'a ast::App>,
    /// The types of the app's grants, in the order they are declared.
    pub grant_tys: Vec<Ty>,
    /// In the order of [`Names::components`].
    pub components: Vec<Comp<'a>>,
    /// The algorithms at the top level of each module, in the order of
    /// [`Names::algorithms`].
    pub algorithms: Vec<Sig<'a>>,
    /// The structs and enums, in the order of [`Names::datas`].
    pub datas: Vec<data::Data>,
    /// The component each `start` statement starts, by the span of its name.
    pub started: HashMap<Span, usize>,
    /// Every route of every running component, with the grant it serves.
    pub routes: Vec<RouteInfo>,
    pub diagnostics: Vec<Diagnostic>,
    /// The parameters and variables of every body, with their types.
    pub locals: Vec<(Ident, Ty)>,
    /// Each name in a body resolved, with the name of what it refers to.
    pub refs: Vec<(Span, Span)>,
    /// The Rust files the external components name.
    pub rust_sources: Vec<external::RustSource>,
}

/// A component and what it declares.
pub(crate) struct Comp<'a> {
    pub ast: &'a ast::Component,
    /// The module it is declared in.
    pub module: usize,
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
    /// The module it is declared in.
    pub module: usize,
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Reply {
    Text,
    Optional,
    /// `Result<String, E>`, with the statuses named on `E`'s variants.
    Fallible(Vec<String>),
    /// A reply whose statuses are not known.
    Unknown,
}

/// A route as its component declares it.
#[derive(Clone, Debug)]
pub(crate) struct RouteDecl {
    pub method: HttpMethod,
    pub template: String,
    pub segments: Vec<Segment>,
    /// The query parameters, in the order the path writes them.
    pub query: Vec<QueryParam>,
    /// The `HttpServer` field named by `via`.
    pub via: String,
    /// The body read, if any, which can fail to convert.
    pub body: Option<BodyDecl>,
    pub reply: Reply,
    /// `T`, when the route replies with a `Json<T>`.
    pub reply_json: Option<Ty>,
    /// The error enum of a route that returns a `Result`, by its index in
    /// [`Checker::datas`].
    pub error: Option<usize>,
    /// The route attribute.
    pub span: Span,
    /// The name of the handler.
    pub function: Span,
}

/// A query parameter, `{name}` after `?` in a route's path.
#[derive(Clone, Debug)]
pub(crate) struct QueryParam {
    pub name: String,
    /// The type it is read as: the parameter's, or what its `Option` holds.
    pub ty: Ty,
    /// Whether a request must give it, which it must unless its type is an `Option`.
    pub required: bool,
}

/// What a route reads its body as.
#[derive(Clone, Debug)]
pub(crate) enum BodyDecl {
    /// Text, converted to the parameter's type as a path segment is.
    Text(Ty),
    /// JSON for `T`, from a `Json<T>` parameter.
    Json(Ty),
}

/// A route of a running component, and the grant its requests arrive through.
#[derive(Clone, Debug)]
pub(crate) struct RouteInfo {
    pub decl: RouteDecl,
    pub component: String,
    pub grant: Option<String>,
}

impl<'a> Checker<'a> {
    fn new(modules: &'a Modules) -> Checker<'a> {
        let mut diagnostics = Vec::new();
        let names = Names::build(modules, &mut diagnostics);
        Checker {
            app: names.app.as_ref().map(|a| a.ast),
            grant_tys: Vec::new(),
            names,
            components: Vec::new(),
            algorithms: Vec::new(),
            datas: Vec::new(),
            started: HashMap::new(),
            routes: Vec::new(),
            diagnostics,
            locals: Vec::new(),
            refs: Vec::new(),
            rust_sources: Vec::new(),
        }
    }

    fn check(&mut self) {
        self.collect_datas();
        self.collect_items();
        self.check_externals();
        self.check_app();
        for index in 0..self.components.len() {
            self.check_component_starts(index);
            self.check_failure(index);
            self.check_external_failures(index);
        }
        self.check_cycles();
        self.check_bodies();
        self.collect_routes();
        self.check_front_end();
    }

    pub(crate) fn error(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    /// The component a `start` statement starts, once it has been resolved.
    pub(crate) fn started(&self, start: &ast::Start) -> Option<usize> {
        self.started.get(&start.component.span).copied()
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
        self.start_of(name).map(|s| &s.component)
    }

    pub(crate) fn start_of(&self, name: &str) -> Option<&ast::Start> {
        self.ast
            .starts
            .iter()
            .find(|s| s.binding.as_ref().is_some_and(|b| b.name == name))
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
