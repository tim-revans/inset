//! Requirement steps, resolved against the app: its name, its `HttpServer`
//! grants and the routes they serve, and its `Table` grants and what they
//! hold. A step that names something the program does not have is a compile
//! error, not a test failure. A step that resolves refers to what it names:
//! its app, the handler of its route, or its grant.
//!
//! Checking the steps also plans them: a [`Plan`] is every requirement as
//! the test runner runs it, with each request's server chosen, each status
//! a number, and each value in a fact or check typed and ready to store or
//! compare.

use std::collections::HashMap;

use inset_syntax::ast::{
    Body, ExprKind, FactKind, ForAll, Lit, LitKind, RequirementsFile, Step, StepKind, TypeKind,
};
use inset_syntax::{Diagnostic, Ident, Span};
use inset_value::{Check, Shape, Types, Value};

use crate::types::{Resolver, Ty};
use crate::values::{Values, shape};
use crate::{BodyDecl, Checker, Program, Reply, Report, RouteInfo, Segment, names, status};

/// A program's requirements, as the test runner runs them.
#[derive(Clone, Debug)]
pub struct Plan {
    pub app: String,
    /// The program's structs and enums, which the values' shapes refer to.
    pub types: Types,
    /// In order of file and then of position.
    pub requirements: Vec<PlannedRequirement>,
}

#[derive(Clone, Debug)]
pub struct PlannedRequirement {
    /// The file's index in [`Program::files`].
    pub file: usize,
    pub sentence: String,
    pub span: Span,
    /// The steps, when there is no examples table and no `for all`.
    pub steps: Vec<PlannedStep>,
    pub examples: Option<PlannedExamples>,
    /// What a property generates, when the requirement has `for all`. Its
    /// steps are planned for each case, by [`crate::Cases`].
    pub property: Option<PlannedProperty>,
}

/// A property's inputs and `where` condition, and where its steps are.
#[derive(Clone, Debug)]
pub struct PlannedProperty {
    pub inputs: Vec<PlannedInput>,
    /// The `where` condition, which reads the inputs as [`Check::Input`].
    pub condition: Option<Check>,
    /// The `for all` line, from `for` to the end of the condition.
    pub span: Span,
    /// The text of the steps in the requirement's file, which each case
    /// reads with its values in place of the placeholders.
    pub steps: Span,
}

#[derive(Clone, Debug)]
pub struct PlannedInput {
    pub name: String,
    /// The type of value to generate: an integer type, `bool` or `Decimal`.
    pub shape: Shape,
}

/// An examples table, whose rows each run as a case of their own.
#[derive(Clone, Debug)]
pub struct PlannedExamples {
    pub headings: Vec<String>,
    pub rows: Vec<PlannedRow>,
}

#[derive(Clone, Debug)]
pub struct PlannedRow {
    /// Each cell as written.
    pub cells: Vec<String>,
    pub span: Span,
    /// The steps, read with the row's cells.
    pub steps: Vec<PlannedStep>,
}

#[derive(Clone, Debug)]
pub struct PlannedStep {
    pub span: Span,
    pub kind: Planned,
}

#[derive(Clone, Debug)]
pub enum Planned {
    /// Delivers a request through the `HttpServer` grant `server`.
    Request {
        server: String,
        method: String,
        /// The path and any query, as written.
        path: String,
        /// Empty text when the step gives no body.
        body: PlannedBody,
    },
    /// The reply to the latest request has this status and body. Empty
    /// text is an empty body.
    Reply { status: u16, body: PlannedBody },
    /// Sets up what a fake holds.
    Given(PlannedFact),
    /// Checks what a fake holds.
    Then(PlannedFact),
    /// A condition on what the fakes hold.
    Check(Check),
}

/// A request's or reply's body.
#[derive(Clone, Debug)]
pub enum PlannedBody {
    /// Sent, or compared, exactly as written.
    Text(String),
    /// A value of a `Json<T>` route's `T`, sent as its JSON, or compared
    /// as a value with the reply's JSON read back as `T`.
    Json { value: Value, shape: Shape },
}

#[derive(Clone, Debug)]
pub enum PlannedFact {
    /// `grant[key] is value`
    Entry {
        grant: String,
        /// The key's stored form.
        key: Vec<u8>,
        key_value: Value,
        value: Value,
        /// The shape of the table's values.
        shape: Shape,
    },
    /// `grant is empty`
    Empty {
        grant: String,
        key_shape: Shape,
        shape: Shape,
    },
    /// `grant fails`: every call to the table or client fails.
    Fails { grant: String },
    /// `grant["METHOD /path"] replies Status with body`: how the fake
    /// `HttpClient` answers the call.
    Reply {
        grant: String,
        method: String,
        path: String,
        status: u16,
        body: Vec<u8>,
    },
}

pub(crate) fn check(
    cx: &Checker,
    program: &Program,
    files: &[(usize, RequirementsFile)],
    refs: &mut Vec<(usize, Span, Span)>,
    plans: &mut Vec<PlannedRequirement>,
) -> Vec<Report> {
    let mut reports = Vec::new();
    let mut sentences: HashMap<&str, (usize, Span)> = HashMap::new();
    for (file, ast) in files {
        let mut report = |diagnostic| {
            reports.push(Report {
                file: *file,
                diagnostic,
            })
        };
        for requirement in &ast.requirements {
            let sentence = &requirement.sentence;
            if let Some(&(first_file, first_span)) = sentences.get(sentence.value.as_str()) {
                let first = &program.files[first_file];
                let line = first.text[..first_span.start].matches('\n').count() + 1;
                report(
                    Diagnostic::new(
                        "two requirements say the same thing",
                        sentence.span,
                        "this repeats an earlier requirement's sentence",
                    )
                    .with_help(format!(
                        "the first is on line {line} of {}; each requirement is one test, so give \
                         each a sentence of its own, or merge the two",
                        first.path
                    )),
                );
            } else {
                sentences.insert(&sentence.value, (*file, sentence.span));
            }
            let mut cases = Vec::new();
            let mut found: Vec<Diagnostic> = Vec::new();
            for (row, steps) in requirement.cases() {
                let mut diagnostics = Vec::new();
                let planned = plan_steps(cx, *file, steps, refs, |d| diagnostics.push(d));
                for mut diagnostic in diagnostics {
                    // A diagnostic about a placeholder is the row's own.
                    if let Some(row) = row
                        && row
                            .placeholders
                            .iter()
                            .any(|p| overlaps(*p, diagnostic.span))
                    {
                        let text = &program.files[*file].text;
                        let line = text[..row.span.start].matches('\n').count() + 1;
                        let note = format!("in the examples row on line {line}");
                        diagnostic.help = Some(match diagnostic.help {
                            Some(help) => format!("{help}; {note}"),
                            None => note,
                        });
                    }
                    if let Some(for_all) = &requirement.for_all
                        && let Some(input) = for_all
                            .placeholders
                            .iter()
                            .find(|p| overlaps(**p, diagnostic.span))
                            .and_then(|p| {
                                let text = &program.files[*file].text;
                                let name = text.get(p.start + 1..p.end - 1)?;
                                for_all.inputs.iter().find(|i| i.name.name == name)
                            })
                    {
                        let note = format!(
                            "with `<{}>` read as a generated value, such as `{}`",
                            input.name.name,
                            ForAll::sample(&input.ty)
                        );
                        diagnostic.help = Some(match diagnostic.help {
                            Some(help) => format!("{help}; {note}"),
                            None => note,
                        });
                    }
                    // Every row reads the steps, so one mistake outside the
                    // placeholders would be found in each.
                    if !found.contains(&diagnostic) {
                        found.push(diagnostic.clone());
                        report(diagnostic);
                    }
                }
                cases.push((row, planned));
            }
            let (steps, examples) = match &requirement.examples {
                None => (
                    cases.pop().map(|(_, steps)| steps).unwrap_or_default(),
                    None,
                ),
                Some(examples) => (
                    Vec::new(),
                    Some(PlannedExamples {
                        headings: examples.headings.iter().map(|h| h.value.clone()).collect(),
                        rows: cases
                            .into_iter()
                            .filter_map(|(row, steps)| {
                                let row = row?;
                                Some(PlannedRow {
                                    cells: row.cells.iter().map(|c| c.value.clone()).collect(),
                                    span: row.span,
                                    steps,
                                })
                            })
                            .collect(),
                    }),
                ),
            };
            let (steps, property) = match &requirement.for_all {
                Some(for_all) => (
                    Vec::new(),
                    plan_property(cx, for_all, &program.files[*file].text, &mut report),
                ),
                None => (steps, None),
            };
            plans.push(PlannedRequirement {
                file: *file,
                sentence: sentence.value.clone(),
                span: requirement.span,
                steps,
                examples,
                property,
            });
        }
    }
    reports
}

/// Checks a property's inputs and `where` condition. Its steps were
/// checked with a sample value for each input, as its case.
fn plan_property(
    cx: &Checker,
    for_all: &ForAll,
    text: &str,
    report: &mut impl FnMut(Diagnostic),
) -> Option<PlannedProperty> {
    let resolver = Resolver {
        names: &cx.names,
        module: 0,
    };
    let mut ok = true;
    let mut inputs: Vec<(Ident, Ty)> = Vec::new();
    for input in &for_all.inputs {
        let name = &input.name;
        if inputs.iter().any(|(n, _)| n.name == name.name) {
            report(Diagnostic::new(
                format!("`{}` is an input twice", name.name),
                name.span,
                "each input has a name of its own",
            ));
            ok = false;
            continue;
        }
        let mut diagnostics = Vec::new();
        let ty = resolver.resolve(&input.ty, &mut diagnostics);
        if !diagnostics.is_empty() {
            diagnostics.into_iter().for_each(&mut *report);
            ok = false;
        } else if !generated(&ty) {
            report(
                Diagnostic::new(
                    format!("a property cannot generate {} yet", crate::values::a(&ty)),
                    input.ty.span,
                    "not a type `for all` generates",
                )
                .with_help(
                    "an input is an integer type other than `u128`, `bool` or `Decimal`, whose \
                     text in a step is the value as Rust writes it",
                ),
            );
            ok = false;
        }
        let used = for_all
            .placeholders
            .iter()
            .any(|p| text.get(p.start + 1..p.end - 1) == Some(name.name.as_str()));
        if !used {
            report(
                Diagnostic::new(
                    format!("no step uses `<{}>`", name.name),
                    name.span,
                    "this input is generated and never used",
                )
                .with_help(format!(
                    "write `<{}>` in a step where the value goes, or remove the input",
                    name.name
                )),
            );
            ok = false;
        }
        inputs.push((name.clone(), ty));
    }
    let condition = match &for_all.condition {
        Some(condition) if ok => {
            let mut values = Values {
                cx,
                diagnostics: Vec::new(),
                refs: Vec::new(),
                inputs: inputs.clone(),
            };
            let check = values.check(condition);
            values.diagnostics.into_iter().for_each(&mut *report);
            Some(check?)
        }
        _ => None,
    };
    if !ok {
        return None;
    }
    Some(PlannedProperty {
        inputs: inputs
            .iter()
            .map(|(name, ty)| PlannedInput {
                name: name.name.clone(),
                shape: shape(ty),
            })
            .collect(),
        condition,
        span: for_all.span,
        steps: for_all.steps,
    })
}

/// Whether a property can generate values of a type.
fn generated(ty: &Ty) -> bool {
    match ty {
        Ty::Int(Some(name)) => *name != "u128",
        Ty::Bool | Ty::Decimal => true,
        _ => false,
    }
}

/// Checks a case's steps, and plans them.
pub(crate) fn plan_steps(
    cx: &Checker,
    file: usize,
    steps: &[Step],
    refs: &mut Vec<(usize, Span, Span)>,
    mut report: impl FnMut(Diagnostic),
) -> Vec<PlannedStep> {
    let file = &file;
    // The route the most recent `when` step reached, if it resolved.
    let mut last: Option<Option<&RouteInfo>> = None;
    let mut planned = Vec::new();
    for step in steps {
        let mut plan = |kind| {
            planned.push(PlannedStep {
                span: step.span,
                kind,
            })
        };
        match &step.kind {
            StepKind::Request {
                app,
                method,
                path,
                via,
                body,
            } => {
                app_name(cx, app, &mut report, &mut |span, target| {
                    refs.push((*file, span, target))
                });
                let route = resolve_request(
                    cx,
                    *method,
                    &path.value,
                    path.span,
                    via.as_ref(),
                    &mut report,
                );
                let planned_body = route.and_then(|route| {
                    request_body(
                        cx,
                        route,
                        body.as_ref(),
                        &mut report,
                        &mut |span, target| refs.push((*file, span, target)),
                    )
                });
                if let (Some(route), Some(body)) = (route, planned_body) {
                    refs.push((*file, step.span, route.decl.function));
                    plan(Planned::Request {
                        server: via.as_ref().map_or_else(
                            || route.grant.clone().unwrap_or_default(),
                            |v| v.name.clone(),
                        ),
                        method: method.as_str().to_string(),
                        path: path.value.clone(),
                        body,
                    });
                }
                last = Some(route);
            }
            StepKind::Reply { app, status, body } => {
                app_name(cx, app, &mut report, &mut |span, target| {
                    refs.push((*file, span, target))
                });
                let Some(route) = last else {
                    report(
                        Diagnostic::new(
                            "a reply is checked after a request",
                            step.span,
                            "no `when` step comes before this",
                        )
                        .with_help("add a `when` step above, which this reply answers"),
                    );
                    continue;
                };
                if let Some(route) = route {
                    refs.push((*file, step.span, route.decl.function));
                }
                check_status(status, route, &mut report);
                let body = reply_body(
                    cx,
                    route,
                    body.as_ref(),
                    &mut report,
                    &mut |span, target| refs.push((*file, span, target)),
                );
                if let (Some(code), Some(body)) = (status::code(&status.name), body) {
                    plan(Planned::Reply { status: code, body });
                }
            }
            StepKind::Fact { given, fact } => {
                let mut values = Values {
                    cx,
                    diagnostics: Vec::new(),
                    refs: Vec::new(),
                    inputs: Vec::new(),
                };
                if let Some(grant) = cx
                    .app
                    .and_then(|a| a.grants.iter().find(|g| g.name.name == fact.grant.name))
                {
                    refs.push((*file, fact.grant.span, grant.name.span));
                }
                if let Some(fact) = plan_fact(&mut values, *given, fact) {
                    plan(if *given {
                        Planned::Given(fact)
                    } else {
                        Planned::Then(fact)
                    });
                }
                refs.extend(values.refs.into_iter().map(|(s, t)| (*file, s, t)));
                values.diagnostics.into_iter().for_each(&mut report);
            }
            StepKind::Check(expr) => {
                let mut values = Values {
                    cx,
                    diagnostics: Vec::new(),
                    refs: Vec::new(),
                    inputs: Vec::new(),
                };
                if let Some(check) = values.check(expr) {
                    plan(Planned::Check(check));
                }
                refs.extend(values.refs.into_iter().map(|(s, t)| (*file, s, t)));
                values.diagnostics.into_iter().for_each(&mut report);
            }
        }
    }
    planned
}

/// Whether two spans share a byte, or an empty one touches the other.
fn overlaps(a: Span, b: Span) -> bool {
    a.start < b.end.max(b.start + 1) && b.start < a.end
}

/// Checks a fact against the grant it names, and types its values.
fn plan_fact(
    values: &mut Values,
    given: bool,
    fact: &inset_syntax::ast::Fact,
) -> Option<PlannedFact> {
    let grant = &fact.grant;
    match &fact.kind {
        FactKind::Fails => {
            if !given {
                values.diagnostics.push(
                    Diagnostic::new(
                        "`fails` follows `given`",
                        fact.span,
                        "a `then` step checks what happened, and this sets up a failure",
                    )
                    .with_help(format!(
                        "move it to a `given` step before the request it fails: `and {} fails`",
                        grant.name
                    )),
                );
                return None;
            }
            if values.table(&grant.name).is_none() && !values.is_client(&grant.name) {
                let diagnostic = values.not_a_table(grant, "can be made to fail")?;
                values.diagnostics.push(diagnostic);
                return None;
            }
            Some(PlannedFact::Fails {
                grant: grant.name.clone(),
            })
        }
        FactKind::Replies { key, status, body } => {
            if !values.is_client(&grant.name) {
                let diagnostic = values.not_a_client(grant)?;
                values.diagnostics.push(diagnostic);
                return None;
            }
            if !given {
                values.diagnostics.push(
                    Diagnostic::new(
                        "`replies` follows `given`",
                        fact.span,
                        "this sets up how a call is answered, which a `then` step cannot check",
                    )
                    .with_help("move it to a `given` step before the request that makes the call"),
                );
                return None;
            }
            let call = match &key.kind {
                ExprKind::Lit(Lit {
                    kind: LitKind::Str(text),
                    ..
                }) => text
                    .split_once(' ')
                    .filter(|(m, p)| {
                        matches!(*m, "GET" | "POST" | "PUT" | "DELETE") && p.starts_with('/')
                    })
                    .map(|(m, p)| (m.to_string(), p.to_string())),
                _ => None,
            };
            let Some((method, path)) = call else {
                values.diagnostics.push(
                    Diagnostic::new(
                        "expected a call, such as `\"GET /forecast/London\"`",
                        key.span,
                        "a call is its method and path, in one string",
                    )
                    .with_help(
                        "write the method, `GET`, `POST`, `PUT` or `DELETE`, a space, and the \
                         path from the client's base address, starting with `/`",
                    ),
                );
                return None;
            };
            let mut ok = true;
            check_status(status, None, &mut |d| {
                ok = false;
                values.diagnostics.push(d);
            });
            let body = match body.as_deref().map(|b| &b.kind) {
                None => Vec::new(),
                Some(ExprKind::Lit(Lit {
                    kind: LitKind::Str(text),
                    ..
                })) => text.clone().into_bytes(),
                Some(_) => {
                    values.diagnostics.push(Diagnostic::new(
                        "a reply's body is text, written as a string",
                        body.as_ref().map_or(fact.span, |b| b.span),
                        "expected a string",
                    ));
                    return None;
                }
            };
            Some(PlannedFact::Reply {
                grant: grant.name.clone(),
                method,
                path,
                status: status::code(&status.name).filter(|_| ok)?,
                body,
            })
        }
        FactKind::Empty => {
            let (key, value) = values.entry_types(grant)?;
            Some(PlannedFact::Empty {
                grant: grant.name.clone(),
                key_shape: shape(key),
                shape: shape(value),
            })
        }
        FactKind::Entry { key, value } => {
            let (key_ty, value_ty) = values.entry_types(grant)?;
            let (key_ty, value_ty) = (key_ty.clone(), value_ty.clone());
            let key_value = values.value(key, &key_ty, &format!("the key of `{}`", grant.name));
            let value = values.value(value, &value_ty, &format!("a value of `{}`", grant.name));
            if !given && let Some(diagnostic) = values.not_comparable(&value_ty, fact.span) {
                values.diagnostics.push(diagnostic);
                return None;
            }
            let key_value = key_value?;
            Some(PlannedFact::Entry {
                grant: grant.name.clone(),
                key: inset_value::key::encode(&key_value, &shape(&key_ty)).ok()?,
                key_value,
                value: value?,
                shape: shape(&value_ty),
            })
        }
    }
}

fn app_name(
    cx: &Checker,
    name: &Ident,
    report: &mut impl FnMut(Diagnostic),
    refer: &mut impl FnMut(Span, Span),
) {
    let Some(app) = cx.app else {
        return;
    };
    if app.name.name == name.name {
        refer(name.span, app.name.span);
    } else {
        report(
            Diagnostic::new(
                format!("no app `{}`", name.name),
                name.span,
                "a step names the program's app",
            )
            .with_help(format!("the app is `{}`", app.name.name)),
        );
    }
}

fn resolve_request<'r>(
    cx: &'r Checker,
    method: inset_syntax::ast::HttpMethod,
    path: &str,
    span: Span,
    via: Option<&Ident>,
    report: &mut impl FnMut(Diagnostic),
) -> Option<&'r RouteInfo> {
    let app = cx.app?;
    let app_name = &app.name.name;
    let servers: Vec<&str> = app
        .grants
        .iter()
        .filter(|g| matches!(&g.ty.kind, TypeKind::Path(p) if p.segments.len() == 1 && p.segments[0].ident.name == "HttpServer"))
        .map(|g| g.name.name.as_str())
        .collect();
    let servers_help = || {
        if servers.is_empty() {
            format!("`{app_name}` grants no `HttpServer`")
        } else {
            format!(
                "the servers of `{app_name}` are {}",
                names(servers.iter().copied())
            )
        }
    };
    let grant = match via {
        Some(via) => {
            if !servers.contains(&via.name.as_str()) {
                let is_grant = app.grants.iter().any(|g| g.name.name == via.name);
                let (message, label) = if is_grant {
                    (
                        format!("`{}` is not an `HttpServer`", via.name),
                        "requests arrive through an `HttpServer`",
                    )
                } else {
                    (
                        format!("`{app_name}` has no capability `{}`", via.name),
                        "not a grant of the app",
                    )
                };
                report(Diagnostic::new(message, via.span, label).with_help(servers_help()));
                return None;
            }
            Some(via.name.as_str())
        }
        None if servers.len() > 1 => {
            report(
                Diagnostic::new(
                    format!("`{app_name}` serves on more than one `HttpServer`"),
                    span,
                    "this request does not say which server it arrives through",
                )
                .with_help(format!(
                    "name the server after the path, such as `via {}`",
                    servers[0]
                )),
            );
            return None;
        }
        None => None,
    };
    let (path, query) = match path.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (path, None),
    };
    let segments: Vec<&str> = path[1..].split('/').collect();
    let candidates: Vec<&RouteInfo> = cx
        .routes
        .iter()
        .filter(|r| grant.is_none() || r.grant.as_deref() == grant)
        .collect();
    let shaped = |route: &RouteInfo| {
        route.decl.method == method
            && route.decl.segments.len() == segments.len()
            && route
                .decl
                .segments
                .iter()
                .zip(&segments)
                .all(|(s, text)| match s {
                    Segment::Literal(lit) => lit == text,
                    Segment::Param(_) => true,
                })
    };
    if let Some(route) = candidates.iter().copied().filter(|r| shaped(r)).find(|r| {
        r.decl
            .segments
            .iter()
            .zip(&segments)
            .all(|(s, text)| match s {
                Segment::Literal(_) => true,
                Segment::Param(ty) => converts(text, ty),
            })
    }) {
        if let Some(query) = query {
            check_query(route, query, span, report);
        }
        return Some(route);
    }
    let request = format!("{} {path}", method.as_str());
    let help = if let Some(route) = candidates.iter().copied().find(|r| shaped(r)) {
        let (text, ty) = route
            .decl
            .segments
            .iter()
            .zip(&segments)
            .find_map(|(s, text)| match s {
                Segment::Param(ty) if !converts(text, ty) => Some((*text, ty)),
                _ => None,
            })
            .unwrap_or(("", &Ty::Unknown));
        format!(
            "the route `{} {}` takes this path, but `{text}` is not a `{ty}`",
            route.decl.method.as_str(),
            route.decl.template
        )
    } else if candidates.is_empty() {
        format!("`{app_name}` has no routes")
    } else {
        let routes: Vec<String> = candidates
            .iter()
            .map(|r| format!("{} {}", r.decl.method.as_str(), r.decl.template))
            .collect();
        format!(
            "the routes of `{app_name}` are {}",
            names(routes.iter().map(String::as_str))
        )
    };
    report(
        Diagnostic::new(
            format!("no route of `{app_name}` takes `{request}`"),
            span,
            "no route matches this request",
        )
        .with_help(help),
    );
    None
}

/// Whether a path segment converts to the parameter's type, after its
/// `%XX` escapes are decoded, as the runtime converts it.
fn converts(text: &str, ty: &Ty) -> bool {
    let Some(text) = percent_decode(text) else {
        return false;
    };
    let text = text.as_str();
    match ty {
        Ty::Bool => text.parse::<bool>().is_ok(),
        Ty::Char => text.chars().count() == 1,
        Ty::Float(_) => text.parse::<f64>().is_ok(),
        Ty::Decimal => {
            let digits = text.strip_prefix('-').unwrap_or(text);
            let mut parts = digits.splitn(2, '.');
            let whole = parts.next().unwrap_or("");
            let fraction = parts.next().unwrap_or("0");
            !whole.is_empty()
                && whole.chars().all(|c| c.is_ascii_digit())
                && fraction.chars().all(|c| c.is_ascii_digit())
        }
        Ty::Int(Some(name)) => match *name {
            "u8" => text.parse::<u8>().is_ok(),
            "u16" => text.parse::<u16>().is_ok(),
            "u32" => text.parse::<u32>().is_ok(),
            "u64" | "usize" => text.parse::<u64>().is_ok(),
            "u128" => text.parse::<u128>().is_ok(),
            "i8" => text.parse::<i8>().is_ok(),
            "i16" => text.parse::<i16>().is_ok(),
            "i32" => text.parse::<i32>().is_ok(),
            "i64" | "isize" => text.parse::<i64>().is_ok(),
            _ => text.parse::<i128>().is_ok(),
        },
        _ => true,
    }
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let escaped = (bytes[i] == b'%')
            .then(|| text.get(i + 1..i + 3))
            .flatten()
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match escaped {
            Some(byte) => {
                out.push(byte);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// Each parameter a request's query gives must be one of the route's. Its
/// value is not checked, so a requirement can show that a value that does
/// not convert gets `BadRequest`.
fn check_query(route: &RouteInfo, query: &str, span: Span, report: &mut impl FnMut(Diagnostic)) {
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let name = pair.split_once('=').map_or(pair, |(name, _)| name);
        let name = percent_decode(name).unwrap_or_else(|| name.to_string());
        if route.decl.query.iter().any(|q| q.name == name) {
            continue;
        }
        let help = if route.decl.query.is_empty() {
            "it takes none".to_string()
        } else {
            format!(
                "it takes {}",
                names(route.decl.query.iter().map(|q| q.name.as_str()))
            )
        };
        report(
            Diagnostic::new(
                format!(
                    "the route `{} {}` takes no query parameter `{name}`",
                    route.decl.method.as_str(),
                    route.decl.template
                ),
                span,
                "the program would ignore this parameter",
            )
            .with_help(help),
        );
    }
}

/// A request's body, typed for its route: text as written, or a value of
/// the route's `Json<T>` body.
fn request_body(
    cx: &Checker,
    route: &RouteInfo,
    body: Option<&Body>,
    report: &mut impl FnMut(Diagnostic),
    refer: &mut impl FnMut(Span, Span),
) -> Option<PlannedBody> {
    match (body, &route.decl.body) {
        (None, _) => Some(PlannedBody::Text(String::new())),
        (Some(Body::Text(text)), _) => Some(PlannedBody::Text(text.value.clone())),
        (Some(Body::Value(value)), Some(BodyDecl::Json(ty))) => {
            json_value(cx, value, ty, "the body", false, report, refer)
        }
        (Some(Body::Value(value)), decl) => {
            let label = match decl {
                Some(_) => "this route reads its body as text",
                None => "this route reads no body",
            };
            report(
                Diagnostic::new(
                    "a body written as a value is sent as JSON",
                    value.span,
                    label,
                )
                .with_help(
                    "write a text body as a string, such as \"20\"; a value is for a route whose body is a `Json<T>`",
                ),
            );
            None
        }
    }
}

/// A reply's body, typed for its route: text as written, or a value of
/// the route's `Json<T>` reply.
fn reply_body(
    cx: &Checker,
    route: Option<&RouteInfo>,
    body: Option<&Body>,
    report: &mut impl FnMut(Diagnostic),
    refer: &mut impl FnMut(Span, Span),
) -> Option<PlannedBody> {
    match (body, route.and_then(|r| r.decl.reply_json.as_ref())) {
        (None, _) => Some(PlannedBody::Text(String::new())),
        (Some(Body::Text(text)), _) => Some(PlannedBody::Text(text.value.clone())),
        (Some(Body::Value(value)), Some(ty)) => {
            json_value(cx, value, ty, "the reply", true, report, refer)
        }
        (Some(Body::Value(_)), None) if route.is_none_or(|r| r.decl.reply == Reply::Unknown) => {
            None
        }
        (Some(Body::Value(value)), None) => {
            report(
                Diagnostic::new(
                    "a body written as a value is compared as JSON",
                    value.span,
                    "this route replies with text",
                )
                .with_help(
                    "write a text body as a string, such as \"40\"; a value is for a route that replies with a `Json<T>`",
                ),
            );
            None
        }
    }
}

fn json_value(
    cx: &Checker,
    value: &inset_syntax::ast::Expr,
    ty: &Ty,
    role: &str,
    compared: bool,
    report: &mut impl FnMut(Diagnostic),
    refer: &mut impl FnMut(Span, Span),
) -> Option<PlannedBody> {
    let mut values = Values {
        cx,
        diagnostics: Vec::new(),
        refs: Vec::new(),
        inputs: Vec::new(),
    };
    let typed = values.value(value, ty, role);
    if compared && let Some(diagnostic) = values.not_comparable(ty, value.span) {
        values.diagnostics.push(diagnostic);
    }
    for (span, target) in values.refs {
        refer(span, target);
    }
    let failed = !values.diagnostics.is_empty();
    values.diagnostics.into_iter().for_each(report);
    if failed {
        return None;
    }
    Some(PlannedBody::Json {
        value: typed?,
        shape: shape(ty),
    })
}

fn check_status(name: &Ident, route: Option<&RouteInfo>, report: &mut impl FnMut(Diagnostic)) {
    if status::code(&name.name).is_none() {
        let help = match status::by_other_case(&name.name) {
            Some(known) => format!("the status is written `{known}`"),
            None => "write a status by its standard name, such as `OK`, `NotFound` or `Conflict`"
                .to_string(),
        };
        report(
            Diagnostic::new(
                format!("unknown status `{}`", name.name),
                name.span,
                "not an HTTP status",
            )
            .with_help(help),
        );
        return;
    }
    let Some(route) = route else {
        return;
    };
    let success = match &route.decl.reply_json {
        Some(ty) => format!("Json<{ty}>"),
        None => "String".to_string(),
    };
    let (mut possible, why): (Vec<&str>, String) = match &route.decl.reply {
        Reply::Unknown => return,
        Reply::Text => (vec!["OK"], format!("it returns a `{success}`")),
        Reply::Optional => (
            vec!["OK", "NotFound"],
            format!("it returns an `Option<{success}>`"),
        ),
        Reply::Fallible(statuses) => {
            let mut possible = vec!["OK"];
            for status in statuses {
                if !possible.contains(&status.as_str()) {
                    possible.push(status);
                }
            }
            (
                possible,
                "it returns a `Result` whose error variants name these statuses".into(),
            )
        }
    };
    if route.decl.body.is_some() || !route.decl.query.is_empty() {
        possible.push("BadRequest");
    }
    // A failure the component rolls back replies `InternalServerError`, so
    // that status is always allowed.
    if name.name == "InternalServerError" || possible.contains(&name.name.as_str()) {
        return;
    }
    report(
        Diagnostic::new(
            format!(
                "the route `{} {}` never replies `{}`",
                route.decl.method.as_str(),
                route.decl.template,
                name.name
            ),
            name.span,
            "this reply cannot happen",
        )
        .with_help(format!(
            "the route replies {}, because {why}",
            names(possible)
        )),
    );
}
