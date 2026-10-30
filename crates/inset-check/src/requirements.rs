//! Requirement steps, resolved against the app: its name, its `HttpServer`
//! grants and the routes they serve. A step that names something the program
//! does not have is a compile error, not a test failure. A step that
//! resolves refers to what it names: its app, and the handler of its route.

use std::collections::HashMap;

use inset_syntax::ast::{RequirementsFile, StepKind, TypeKind};
use inset_syntax::{Diagnostic, Ident, Span};

use crate::types::Ty;
use crate::{Checker, Program, Reply, Report, RouteInfo, Segment, names, status};

pub(crate) fn check(
    cx: &Checker,
    program: &Program,
    files: &[(usize, RequirementsFile)],
    refs: &mut Vec<(usize, Span, Span)>,
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
            // The route the most recent `when` step reached, if it resolved.
            let mut last: Option<Option<&RouteInfo>> = None;
            for step in &requirement.steps {
                match &step.kind {
                    StepKind::Request {
                        app,
                        method,
                        path,
                        via,
                        ..
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
                        if let Some(route) = route {
                            refs.push((*file, step.span, route.decl.function));
                        }
                        last = Some(route);
                    }
                    StepKind::Reply { app, status, .. } => {
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
                    }
                }
            }
        }
    }
    reports
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
    if path.contains('?') {
        report(Diagnostic::new(
            "a query in a request is not supported yet",
            span,
            "the checker does not handle this yet",
        ));
        return None;
    }
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
    let (mut possible, why) = match route.decl.reply {
        Reply::Fallible => return,
        Reply::Text => (vec!["OK"], "it returns a `String`"),
        Reply::Optional => (vec!["OK", "NotFound"], "it returns an `Option<String>`"),
    };
    if route.decl.body {
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
