//! A program that reaches past its grants, as Rust in an external component
//! might. Inset source could not write these calls; this is written by hand.
//!
//! Each route makes one call and replies with what happened to it.

use inset_runtime::bindings::inset::host::capabilities;
use inset_runtime::prelude::*;

struct Sandbox;

impl App for Sandbox {
    const NAME: &'static str = "Sandbox";
    const SETTINGS: &'static [Setting] = &[];
    const GRANTS: &'static [Grant] = &[Grant {
        name: "http",
        capability: "HttpServer",
        fields: &[("port", GrantValue::Literal("8080"))],
    }];

    fn start(scope: &Scope, grants: &mut Grants) -> Result<(), Stopped> {
        let http = grants.http_server("http");
        scope.start(|_| Prober { http });
        Ok(())
    }
}

struct Prober {
    http: HttpServer,
}

fn outcome<T, E: std::fmt::Display>(result: Result<T, E>) -> String {
    match result {
        Ok(_) => "allowed".into(),
        Err(e) => format!("refused: {e}"),
    }
}

impl Component for Prober {
    const NAME: &'static str = "Prober";
    type Saved = ();

    fn save(&self) {}

    fn restore(&mut self, (): ()) {}

    fn servers(&self) -> Vec<(&'static str, &HttpServer)> {
        vec![("http", &self.http)]
    }

    fn route(_: &str, request: &Request) -> Route<Self> {
        let probe: fn() -> String = if request.route(Method::Get, "/file").is_some() {
            || outcome(std::fs::read_to_string("/etc/hostname"))
        } else if request.route(Method::Get, "/directory").is_some() {
            || outcome(std::fs::read_dir("."))
        } else if request.route(Method::Get, "/connect").is_some() {
            || outcome(std::net::TcpStream::connect("127.0.0.1:80"))
        } else if request.route(Method::Get, "/listen").is_some() {
            || outcome(std::net::TcpListener::bind("127.0.0.1:0"))
        } else if request.route(Method::Get, "/environment").is_some() {
            || format!("{} variables", std::env::vars().count())
        } else if request.route(Method::Get, "/table").is_some() {
            || outcome(capabilities::open_table("secrets"))
        } else {
            return Route::NotFound;
        };
        Route::to(move |_: &mut Self| Box::pin(async move { probe() }))
    }
}

inset_runtime::export!(Sandbox);
