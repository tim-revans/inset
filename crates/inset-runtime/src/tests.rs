//! The runtime's semantics, checked natively with a small app written the
//! way the compiler emits one. Tables need the host, so a failing capability
//! is simulated with [`fail`].
//!
//! ```inset
//! app Shop {
//!     grant http: HttpServer { port: 8080 };
//!     start Front { http: move http, total: 0 };
//! }
//!
//! component Front {
//!     http: HttpServer,
//!     total: u32,
//!     start log: Log { entries: Vec::new() };
//!     failure {
//!         Capability(http) => rollback,
//!         Message(log) => rollback,
//!         Panic => rollback,
//!     }
//!
//!     #[get("/total", via = http)]
//!     fn total(&self) -> String { self.total.to_string() }
//!
//!     /// Adds to the total. Adding 13 fails after changing everything.
//!     #[post("/add/{n}", via = http)]
//!     async fn add(&mut self, n: u32) -> String {
//!         self.total += n;
//!         send self.log.push(format!("add {n}"));
//!         let seen = self.log.entries().await;
//!         if n == 13 { /* a capability call that fails */ }
//!         format!("{} after {}", self.total, seen.len())
//!     }
//!
//!     #[post("/note/{text}", via = http)]
//!     async fn note(&self, text: String) -> String {
//!         self.log.note(&text).await;
//!         "noted".to_string()
//!     }
//!
//!     #[get("/log", via = http)]
//!     async fn log(&self) -> String { self.log.entries().await.join(",") }
//!
//!     #[post("/crash", via = http)]
//!     async fn crash(&self) -> String { /* a failure nothing responds to */ }
//!
//!     #[post("/panic", via = http)]
//!     async fn panic(&mut self) -> String {
//!         self.total += 100;
//!         send self.log.push("panic".to_string());
//!         panic!("boom")
//!     }
//! }
//!
//! component Log {
//!     entries: Vec<String>,
//!     failure { Capability(disk) => rollback }
//!
//!     #[message]
//!     fn push(&mut self, entry: String) { self.entries.push(entry); }
//!     #[message]
//!     fn entries(&self) -> Vec<String> { self.entries.clone() }
//!     /// Notes `text`. Fails after noting "bad", and panics after noting "panic".
//!     #[message]
//!     async fn note(&mut self, text: &str) { ... }
//! }
//! ```

use crate::actor::fail;
use crate::prelude::*;
use crate::program::{self, report::REPORTS};
use crate::{Reply, Stopped};

struct Shop;

impl App for Shop {
    const NAME: &'static str = "Shop";
    const SETTINGS: &'static [Setting] = &[];
    const GRANTS: &'static [Grant] = &[Grant {
        name: "http",
        capability: "HttpServer",
        fields: &[("port", GrantValue::Literal("8080"))],
    }];

    fn start(scope: &Scope, grants: &mut Grants) -> Result<(), Stopped> {
        let http = grants.http_server("http");
        scope.start(|scope| Front::new(scope, http, 0));
        Ok(())
    }
}

struct Front {
    http: HttpServer,
    total: u32,
    log: Handle<Log>,
}

impl Front {
    fn new(scope: &Scope, http: HttpServer, total: u32) -> Front {
        let log = scope.start_as("log", |_| Log {
            entries: Vec::new(),
        });
        Front { http, total, log }
    }

    fn total(&self) -> String {
        self.total.to_string()
    }

    async fn add(&mut self, n: u32) -> String {
        self.total += n;
        send(self.log.push(format!("add {n}")));
        let seen = self.log.entries().await;
        if n == 13 {
            fail::<()>(Failure::Capability("http"), "unlucky".into()).await;
        }
        format!("{} after {}", self.total, seen.len())
    }

    async fn note(&self, text: String) -> String {
        self.log.note(&text).await;
        "noted".to_string()
    }

    async fn log(&self) -> String {
        self.log.entries().await.join(",")
    }

    async fn crash(&self) -> String {
        fail(Failure::Capability("elsewhere"), "nobody responds".into()).await
    }

    async fn panic(&mut self) -> String {
        self.total += 100;
        send(self.log.push("panic".to_string()));
        panic!("boom")
    }
}

impl Component for Front {
    const NAME: &'static str = "Front";
    const ROLLS_BACK: bool = true;
    type Saved = u32;

    fn save(&self) -> u32 {
        self.total
    }

    fn restore(&mut self, total: u32) {
        self.total = total;
    }

    fn respond(failure: &Failure) -> Option<Response> {
        match failure {
            Failure::Capability("http") => Some(Response::Rollback),
            Failure::Message("log") => Some(Response::Rollback),
            Failure::Panic => Some(Response::Rollback),
            _ => None,
        }
    }

    fn servers(&self) -> Vec<(&'static str, &HttpServer)> {
        vec![("http", &self.http)]
    }

    fn route(field: &str, request: &Request) -> Route<Self> {
        if field != "http" {
            return Route::NotFound;
        }
        if request.route(Method::Get, "/total").is_some() {
            return Route::to(|c: &mut Self| Box::pin(async move { c.total() }));
        }
        if let Some(path) = request.route(Method::Post, "/add/{n}") {
            let Some(n) = path.get("n") else {
                return Route::NotFound;
            };
            return Route::to(move |c: &mut Self| Box::pin(c.add(n)));
        }
        if let Some(path) = request.route(Method::Post, "/note/{text}") {
            let Some(text) = path.get("text") else {
                return Route::NotFound;
            };
            return Route::to(move |c: &mut Self| Box::pin(c.note(text)));
        }
        if request.route(Method::Get, "/log").is_some() {
            return Route::to(|c: &mut Self| Box::pin(c.log()));
        }
        if request.route(Method::Post, "/crash").is_some() {
            return Route::to(|c: &mut Self| Box::pin(c.crash()));
        }
        if request.route(Method::Post, "/panic").is_some() {
            return Route::to(|c: &mut Self| Box::pin(c.panic()));
        }
        Route::NotFound
    }
}

struct Log {
    entries: Vec<String>,
}

impl Log {
    fn push(&mut self, entry: String) {
        self.entries.push(entry);
    }

    fn entries(&self) -> Vec<String> {
        self.entries.clone()
    }

    async fn note(&mut self, text: &str) {
        self.entries.push(text.to_string());
        if text == "bad" {
            fail::<()>(Failure::Capability("disk"), "disk full".into()).await;
        }
        assert!(text != "panic", "the log panics");
    }
}

impl Component for Log {
    const NAME: &'static str = "Log";
    const ROLLS_BACK: bool = true;
    type Saved = Vec<String>;

    fn save(&self) -> Vec<String> {
        self.entries.clone()
    }

    fn restore(&mut self, entries: Vec<String>) {
        self.entries = entries;
    }

    fn respond(failure: &Failure) -> Option<Response> {
        match failure {
            Failure::Capability("disk") => Some(Response::Rollback),
            _ => None,
        }
    }
}

/// The messages of `Log`, as the compiler emits them for its handle.
trait LogMessages {
    fn push(&self, entry: String) -> Message<Log, ()>;
    fn entries(&self) -> Message<Log, Vec<String>>;
    fn note(&self, text: &str) -> Message<Log, ()>;
}

impl LogMessages for Handle<Log> {
    fn push(&self, entry: String) -> Message<Log, ()> {
        self.message(move |c| Box::pin(async move { c.push(entry) }))
    }

    fn entries(&self) -> Message<Log, Vec<String>> {
        self.message(|c| Box::pin(async move { c.entries() }))
    }

    /// `text` is lent, so the receiver gets a copy.
    fn note(&self, text: &str) -> Message<Log, ()> {
        let text = text.to_string();
        self.message(move |c| Box::pin(async move { c.note(&text).await }))
    }
}

fn request(method: &str, path: &str) -> Result<Reply, Stopped> {
    program::handle(
        "http",
        Request {
            method: method.into(),
            path: path.into(),
            body: Vec::new(),
        },
    )
}

fn text(method: &str, path: &str) -> (u16, String) {
    let reply = request(method, path).expect("the program is running");
    (reply.status, String::from_utf8(reply.body).unwrap())
}

fn start() {
    program::start::<Shop>(None).expect("Shop starts");
}

#[test]
fn a_route_handler_replies() {
    start();
    assert_eq!(text("GET", "/total"), (200, "0".into()));
    assert_eq!(text("GET", "/nowhere"), (404, String::new()));
    assert_eq!(text("POST", "/add/many"), (404, String::new()));
}

#[test]
fn a_sent_message_is_delivered_when_its_handler_completes_and_before_its_reply() {
    start();
    // `add` sends `push`, then awaits `entries`, which it finds without the push.
    assert_eq!(text("POST", "/add/2"), (200, "2 after 0".into()));
    // The push was delivered before the reply, so the next request sees it.
    assert_eq!(text("GET", "/log"), (200, "add 2".into()));
}

#[test]
fn a_rollback_restores_fields_and_drops_sent_messages() {
    start();
    text("POST", "/add/2");
    assert_eq!(text("POST", "/add/13"), (500, String::new()));
    assert_eq!(text("GET", "/total"), (200, "2".into()));
    assert_eq!(text("GET", "/log"), (200, "add 2".into()));
    REPORTS.with_borrow(|r| assert_eq!(*r, ["Shop/Front rolled back Capability(http): unlucky"]));
}

#[test]
fn a_failed_message_fails_the_handler_awaiting_it() {
    start();
    assert_eq!(text("POST", "/note/good"), (200, "noted".into()));
    assert_eq!(text("POST", "/note/bad"), (500, String::new()));
    assert_eq!(text("GET", "/log"), (200, "good".into()));
    REPORTS.with_borrow(|r| {
        assert_eq!(
            *r,
            [
                "Shop/Front/log rolled back Capability(disk): disk full",
                "Shop/Front rolled back Message(log): Shop/Front/log failed",
            ]
        )
    });
}

#[test]
fn a_failure_nothing_responds_to_stops_the_program() {
    start();
    let stopped = Stopped {
        component: "Shop/Front".into(),
        failure: "Capability(elsewhere)".into(),
        message: "nobody responds".into(),
    };
    assert_eq!(request("POST", "/crash"), Err(stopped.clone()));
    assert_eq!(request("GET", "/total"), Err(stopped));
}

/// Natively a panic unwinds, where in WebAssembly it traps, but the panic
/// hook runs first either way.
fn panics(method: &'static str, path: &'static str) {
    let outcome = std::panic::catch_unwind(|| request(method, path));
    assert!(outcome.is_err(), "{method} {path} panics");
}

#[test]
fn a_panic_hands_over_the_state_from_before_the_handler() {
    start();
    text("POST", "/add/2");
    panics("POST", "/panic");
    let report = REPORTS.with_borrow(|r| r.last().cloned().unwrap());
    let recovery = r#"{"Shop/Front":2,"Shop/Front/log":["add 2"]}"#;
    assert!(
        report.starts_with("Shop/Front trapping Panic: boom at "),
        "{report}"
    );
    assert!(
        report.ends_with(&format!("; recovery Some({recovery:?})")),
        "{report}"
    );
}

#[test]
fn a_panic_hands_over_nothing_unless_every_interrupted_handler_rolls_back() {
    start();
    panics("POST", "/note/panic");
    let report = REPORTS.with_borrow(|r| r.last().cloned().unwrap());
    assert!(
        report.starts_with("Shop/Front/log trapping Panic: the log panics at "),
        "{report}"
    );
    assert!(report.ends_with("; recovery None"), "{report}");
}

#[test]
fn a_program_restores_the_state_handed_over() {
    let state = br#"{"Shop/Front":7,"Shop/Front/log":["add 7"]}"#;
    program::start::<Shop>(Some(state.to_vec())).unwrap();
    assert_eq!(text("GET", "/total"), (200, "7".into()));
    assert_eq!(text("GET", "/log"), (200, "add 7".into()));
}
