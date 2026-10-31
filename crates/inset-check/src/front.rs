//! The app's front end: `front end via server calls [...]`, its files in
//! `web/`, and the routes it calls.
//!
//! The compiler does not read the front end's JavaScript, so the calls are
//! declared, and checked against the routes of the server named by `via`.
//! Each call is drawn as an arrow on the diagram, and the generated API
//! contract types each route the front end uses.

use inset_syntax::ast::{FrontEnd, HttpMethod, StrLit, TypeKind};
use inset_syntax::{Diagnostic, Span};

use crate::{Checker, RouteInfo, names};

impl Checker<'_> {
    /// Checks the front end against the app's grants and routes, once the
    /// routes are known.
    pub(crate) fn check_front_end(&mut self) {
        let Some(app) = self.app else {
            return;
        };
        for extra in app.front_ends.iter().skip(1) {
            self.error(
                Diagnostic::new(
                    "an app has one front end",
                    extra.span,
                    "this is a second front end",
                )
                .with_help("a program's front end is the files in `web/`; list every route it calls in one `front end`"),
            );
        }
        let Some(front) = app.front_ends.first() else {
            return;
        };
        let servers: Vec<&str> = app
            .grants
            .iter()
            .filter(|g| matches!(&g.ty.kind, TypeKind::Path(p) if p.segments.len() == 1 && p.segments[0].ident.name == "HttpServer"))
            .map(|g| g.name.name.as_str())
            .collect();
        if !servers.contains(&front.via.name.as_str()) {
            let is_grant = app.grants.iter().any(|g| g.name.name == front.via.name);
            let help = if servers.is_empty() {
                format!("`{}` grants no `HttpServer`", app.name.name)
            } else {
                format!("the servers of `{}` are {}", app.name.name, names(servers))
            };
            self.error(
                Diagnostic::new(
                    if is_grant {
                        format!("`{}` is not an `HttpServer`", front.via.name)
                    } else {
                        format!("`{}` has no capability `{}`", app.name.name, front.via.name)
                    },
                    front.via.span,
                    "the front end is served through an `HttpServer`",
                )
                .with_help(help),
            );
            return;
        }
        if let Some(grant) = app.grants.iter().find(|g| g.name.name == front.via.name) {
            self.refs.push((front.via.span, grant.name.span));
        }
        let mut seen: Vec<(HttpMethod, &str)> = Vec::new();
        for call in &front.calls {
            let Some((method, template)) = self.front_end_call(front, call) else {
                continue;
            };
            if seen.contains(&(method, template)) {
                self.error(Diagnostic::new(
                    "the front end lists this route twice",
                    call.span,
                    "this repeats an earlier call",
                ));
                continue;
            }
            seen.push((method, template));
        }
    }

    /// Resolves one call to the route it names, recording the reference.
    fn front_end_call<'s>(
        &mut self,
        front: &FrontEnd,
        call: &'s StrLit,
    ) -> Option<(HttpMethod, &'s str)> {
        let Some((method, template)) = call.value.split_once(' ') else {
            self.error(
                Diagnostic::new(
                    "a call names a route by its method and path",
                    call.span,
                    "expected a method, a space and a path",
                )
                .with_help("write the route as \"GET /items/{sku}\", with the path as its attribute writes it"),
            );
            return None;
        };
        let Some(method) = HttpMethod::from_request(method) else {
            let help = match HttpMethod::from_request(&method.to_uppercase()) {
                Some(m) => format!("methods are written in capitals: `{}`", m.as_str()),
                None => "the methods are `GET`, `POST`, `PUT`, `PATCH` and `DELETE`".into(),
            };
            self.error(
                Diagnostic::new(
                    format!("unknown HTTP method `{method}`"),
                    call.span,
                    "the call starts with its route's method",
                )
                .with_help(help),
            );
            return None;
        };
        let routes: Vec<&RouteInfo> = self
            .routes
            .iter()
            .filter(|r| r.grant.as_deref() == Some(front.via.name.as_str()))
            .collect();
        if let Some(route) = routes
            .iter()
            .find(|r| r.decl.method == method && r.decl.template == template)
        {
            let target = route.decl.function;
            self.refs.push((call.span, target));
            return Some((method, template));
        }
        let listed: Vec<String> = routes
            .iter()
            .map(|r| format!("\"{} {}\"", r.decl.method.as_str(), r.decl.template))
            .collect();
        let near = routes.iter().find(|r| {
            r.decl.method == method
                && r.decl.template.split('?').next() == template.split('?').next()
        });
        let help = match near {
            Some(route) => format!(
                "write the path as its attribute does: \"{} {}\"",
                route.decl.method.as_str(),
                route.decl.template
            ),
            None if listed.is_empty() => format!("`{}` serves no routes", front.via.name),
            None => format!(
                "the routes of `{}` are {}",
                front.via.name,
                listed.join(", ")
            ),
        };
        self.error(
            Diagnostic::new(
                format!("no route of `{}` is `{}`", front.via.name, call.value),
                call.span,
                "the front end calls a route that does not exist",
            )
            .with_help(help),
        );
        None
    }

    /// Checks the front end's files: `web/` holds them, with an
    /// `index.html` for the page at `/`, and holds nothing unless the app
    /// declares a front end. `web` is the files under `web/`, or `None`
    /// when they are not known, as in an editor.
    pub(crate) fn check_web(&mut self, web: Option<&[String]>) {
        let Some(web) = web else {
            return;
        };
        let front = self.app.and_then(|a| a.front_ends.first());
        match front {
            Some(front) if !web.iter().any(|f| f == "index.html") => {
                self.error(
                    Diagnostic::new(
                        "the front end has no `web/index.html`",
                        front.span,
                        "the page at `/` is `web/index.html`",
                    )
                    .with_help("write the front end's page in `web/index.html`, beside `src/`"),
                );
            }
            None if !web.is_empty() => {
                let span = self.app.map_or(Span::new(0, 0), |a| a.name.span);
                self.error(
                    Diagnostic::new(
                        "`web/` holds files, but the app has no front end",
                        span,
                        "this app declares no `front end`",
                    )
                    .with_help("declare it in the app, as `front end via http calls [\"GET /items\"];`, or remove `web/`"),
                );
            }
            _ => {}
        }
    }
}
