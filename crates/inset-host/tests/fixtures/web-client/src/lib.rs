//! A program that makes an outgoing web request, which no Inset capability
//! grants. Inset source could not write this; it is written by hand.

use inset_runtime::prelude::*;
use wasip2::http::{outgoing_handler, types};

struct WebClient;

impl App for WebClient {
    const NAME: &'static str = "WebClient";
    const SETTINGS: &'static [Setting] = &[];
    const GRANTS: &'static [Grant] = &[Grant {
        name: "http",
        capability: "HttpServer",
        fields: &[("port", GrantValue::Literal("8080"))],
    }];

    fn start(scope: &Scope, grants: &mut Grants) -> Result<(), Stopped> {
        let http = grants.http_server("http");
        scope.start(|_| Fetcher { http });
        Ok(())
    }
}

struct Fetcher {
    http: HttpServer,
}

impl Component for Fetcher {
    const NAME: &'static str = "Fetcher";
    type Saved = ();

    fn save(&self) {}

    fn restore(&mut self, (): ()) {}

    fn servers(&self) -> Vec<(&'static str, &HttpServer)> {
        vec![("http", &self.http)]
    }

    fn route(_: &str, _: &Request) -> Route<Self> {
        Route::to(|_: &mut Self| {
            Box::pin(async move {
                let request = types::OutgoingRequest::new(types::Fields::new());
                match outgoing_handler::handle(request, None) {
                    Ok(_) => "sent".to_string(),
                    Err(e) => format!("refused: {e:?}"),
                }
            })
        })
    }
}

inset_runtime::export!(WebClient);
