//! Hello HTTP: the smallest Inset program that serves the web.
//!
//! Emitted from `src/main.inset`. Every item keeps its doc comment, and each
//! Inset statement that is not plain Rust is written above what it became.

use inset_runtime::prelude::*;

/// Hello HTTP: the smallest Inset program that serves the web.
struct HelloHttp;

impl App for HelloHttp {
    const NAME: &'static str = "HelloHttp";

    const SETTINGS: &'static [Setting] = &[
        // setting port: u16 = 8080;
        Setting {
            name: "port",
            ty: SettingType::U16,
            default: "8080",
        },
    ];

    const GRANTS: &'static [Grant] = &[
        // grant http: HttpServer { port };
        Grant {
            name: "http",
            capability: "HttpServer",
            fields: &[("port", GrantValue::Setting("port"))],
        },
    ];

    fn start(scope: &Scope, grants: &mut Grants) -> Result<(), Stopped> {
        let http = grants.http_server("http");
        // start Greeter { http: move http };
        scope.start(|_| Greeter { http });
        Ok(())
    }
}

/// Greets visitors by name.
struct Greeter {
    http: HttpServer,
}

impl Greeter {
    /// Replies with a greeting for the name in the path.
    fn hello(name: String) -> String {
        format!("Hello, {name}!")
    }
}

impl Component for Greeter {
    const NAME: &'static str = "Greeter";

    type Saved = ();

    fn save(&self) {}

    fn restore(&mut self, (): ()) {}

    fn servers(&self) -> Vec<(&'static str, &HttpServer)> {
        vec![("http", &self.http)]
    }

    fn route(field: &str, request: &Request) -> Route<Self> {
        if field == "http" {
            // #[get("/hello/{name}", via = http)]
            if let Some(path) = request.route(Method::Get, "/hello/{name}") {
                let Some(name) = path.get("name") else {
                    return Route::NotFound;
                };
                return Route::to(move |_: &mut Self| Box::pin(async move { Self::hello(name) }));
            }
        }
        Route::NotFound
    }
}

inset_runtime::export!(HelloHttp);
