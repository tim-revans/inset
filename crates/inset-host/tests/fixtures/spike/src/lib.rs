//! The runtime spike's program: rollback after each kind of failure, and the
//! routes that measure lending. Written in the shape the compiler will emit
//! for this source:
//!
//! ```inset
//! app Spike {
//!     grant http: HttpServer { port: 8080 };
//!     grant counts: Table<String, u32> { file: "counts.db", name: "counts" };
//!     start Front { http: move http, counts: move counts, hits: 0 };
//! }
//!
//! component Front {
//!     http: HttpServer,
//!     counts: Table<String, u32>,
//!     hits: u32,
//!     start log: Log { entries: Vec::new() };
//!     start audit: Audit { checked: Vec::new() };
//!
//!     failure {
//!         /// The next request may succeed.
//!         Capability(counts) => rollback,
//!         /// Only the request that hit the bug fails.
//!         Panic => rollback,
//!         /// Only the request that overflowed fails.
//!         Overflow => rollback,
//!         /// Only the request that failed the audit fails.
//!         Message(audit) => rollback,
//!     }
//!
//!     /// Counts a visit to `key`, changing every kind of state before it
//!     /// fails on the keys that ask it to.
//!     #[post("/count/{key}", via = http)]
//!     async fn count(&mut self, key: String) -> String {
//!         self.hits += 1;
//!         let n = self.counts.get(&key).await.unwrap_or(0) + 1;
//!         self.counts.insert(key.clone(), n).await;
//!         send self.log.push(format!("{key} {n}"));
//!         match key.as_str() {
//!             "panic" => panic!("asked to panic"),
//!             "overflow" => self.hits += u32::MAX,
//!             "audit" | "log" => {
//!                 self.audit.check(&key).await;
//!                 self.log.check(&key).await;
//!             }
//!             _ => {}
//!         }
//!         format!("{key} {n}")
//!     }
//!
//!     #[get("/count/{key}", via = http)]
//!     async fn stored(&self, key: String) -> Option<String> { ... }
//!     #[get("/hits", via = http)]
//!     fn hits(&self) -> String { ... }
//!     #[get("/log", via = http)]
//!     async fn log(&self) -> String { ... }
//!     #[get("/audit", via = http)]
//!     async fn audited(&self) -> String { ... }
//!
//!     /// Lends `bytes` bytes to `Log` `times` times. Each crossing copies.
//!     #[post("/lend/{bytes}/{times}", via = http)]
//!     async fn lend(&self, bytes: usize, times: u32) -> String { ... }
//!     /// Moves `bytes` bytes to `Log` and back `times` times. Nothing is copied.
//!     #[post("/move/{bytes}/{times}", via = http)]
//!     async fn moved(&self, bytes: usize, times: u32) -> String { ... }
//! }
//!
//! /// Records every count, and panics checking "log". It responds to nothing.
//! component Log { entries: Vec<String>, ... }
//!
//! /// Records every audited key, and panics checking "audit".
//! component Audit {
//!     checked: Vec<String>,
//!     failure {
//!         /// An audit that panics fails only its own request.
//!         Panic => rollback,
//!     }
//!     ...
//! }
//! ```

use inset_runtime::prelude::*;

struct Spike;

impl App for Spike {
    const NAME: &'static str = "Spike";
    const SETTINGS: &'static [Setting] = &[];
    const GRANTS: &'static [Grant] = &[
        Grant {
            name: "http",
            capability: "HttpServer",
            fields: &[("port", GrantValue::Literal("8080"))],
        },
        Grant {
            name: "counts",
            capability: "Table<String, u32>",
            fields: &[
                ("file", GrantValue::Literal("counts.db")),
                ("name", GrantValue::Literal("counts")),
            ],
        },
    ];

    fn start(scope: &Scope, grants: &mut Grants) -> Result<(), Stopped> {
        let http = grants.http_server("http");
        let counts = grants.table("counts")?;
        scope.start(|scope| Front::start(scope, http, counts, 0));
        Ok(())
    }
}

struct Front {
    http: HttpServer,
    counts: Table<String, u32>,
    hits: u32,
    log: Handle<Log>,
    audit: Handle<Audit>,
}

impl Front {
    fn start(scope: &Scope, http: HttpServer, counts: Table<String, u32>, hits: u32) -> Front {
        let log = scope.start_as("log", |_| Log {
            entries: Vec::new(),
        });
        let audit = scope.start_as("audit", |_| Audit {
            checked: Vec::new(),
        });
        Front {
            http,
            counts,
            hits,
            log,
            audit,
        }
    }

    async fn count(&mut self, key: String) -> String {
        self.hits += 1;
        let n = self.counts.get(&key).await.unwrap_or(0) + 1;
        self.counts.insert(key.clone(), n).await;
        send(self.log.push(format!("{key} {n}")));
        match key.as_str() {
            "panic" => panic!("asked to panic"),
            "overflow" => self.hits += u32::MAX,
            "audit" | "log" => {
                self.audit.check(&key).await;
                self.log.check(&key).await;
            }
            _ => {}
        }
        format!("{key} {n}")
    }

    async fn stored(&self, key: String) -> Option<String> {
        Some(self.counts.get(&key).await?.to_string())
    }

    fn hits(&self) -> String {
        self.hits.to_string()
    }

    async fn log(&self) -> String {
        self.log.entries().await.join(",")
    }

    async fn audited(&self) -> String {
        self.audit.checked().await.join(",")
    }

    async fn lend(&self, bytes: usize, times: u32) -> String {
        let text = "x".repeat(bytes);
        let mut total = 0;
        for _ in 0..times {
            total += self.log.measure(&text).await;
        }
        total.to_string()
    }

    async fn moved(&self, bytes: usize, times: u32) -> String {
        let mut text = "x".repeat(bytes);
        let mut total = 0;
        for _ in 0..times {
            text = self.log.echo(text).await;
            total += text.len();
        }
        total.to_string()
    }
}

impl Component for Front {
    const NAME: &'static str = "Front";
    const ROLLS_BACK: bool = true;
    type Saved = u32;

    fn save(&self) -> u32 {
        self.hits
    }

    fn restore(&mut self, hits: u32) {
        self.hits = hits;
    }

    fn respond(failure: &Failure) -> Option<Response> {
        match failure {
            // The next request may succeed.
            Failure::Capability("counts") => Some(Response::Rollback),
            // Only the request that hit the bug fails.
            Failure::Panic => Some(Response::Rollback),
            // Only the request that overflowed fails.
            Failure::Overflow => Some(Response::Rollback),
            // Only the request that failed the audit fails.
            Failure::Message("audit") => Some(Response::Rollback),
            _ => None,
        }
    }

    fn label(&mut self) {
        self.counts.label("counts");
    }

    fn servers(&self) -> Vec<(&'static str, &HttpServer)> {
        vec![("http", &self.http)]
    }

    fn route(field: &str, request: &Request) -> Route<Self> {
        if field != "http" {
            return Route::NotFound;
        }
        if let Some(path) = request.route(Method::Post, "/count/{key}") {
            let Some(key) = path.get("key") else {
                return Route::NotFound;
            };
            return Route::to(move |c: &mut Self| Box::pin(c.count(key)));
        }
        if let Some(path) = request.route(Method::Get, "/count/{key}") {
            let Some(key) = path.get("key") else {
                return Route::NotFound;
            };
            return Route::to(move |c: &mut Self| Box::pin(c.stored(key)));
        }
        if request.route(Method::Get, "/hits").is_some() {
            return Route::to(|c: &mut Self| Box::pin(async move { c.hits() }));
        }
        if request.route(Method::Get, "/log").is_some() {
            return Route::to(|c: &mut Self| Box::pin(c.log()));
        }
        if request.route(Method::Get, "/audit").is_some() {
            return Route::to(|c: &mut Self| Box::pin(c.audited()));
        }
        if let Some(path) = request.route(Method::Post, "/lend/{bytes}/{times}") {
            let (Some(bytes), Some(times)) = (path.get("bytes"), path.get("times")) else {
                return Route::NotFound;
            };
            return Route::to(move |c: &mut Self| Box::pin(c.lend(bytes, times)));
        }
        if let Some(path) = request.route(Method::Post, "/move/{bytes}/{times}") {
            let (Some(bytes), Some(times)) = (path.get("bytes"), path.get("times")) else {
                return Route::NotFound;
            };
            return Route::to(move |c: &mut Self| Box::pin(c.moved(bytes, times)));
        }
        Route::NotFound
    }
}

/// Records every count, and panics checking "log". It responds to nothing.
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

    fn check(&mut self, key: &str) {
        assert!(key != "log", "the log refuses to check itself");
    }

    fn measure(&self, text: &str) -> usize {
        text.len()
    }

    fn echo(&self, text: String) -> String {
        text
    }
}

impl Component for Log {
    const NAME: &'static str = "Log";
    type Saved = Vec<String>;

    fn save(&self) -> Vec<String> {
        self.entries.clone()
    }

    fn restore(&mut self, entries: Vec<String>) {
        self.entries = entries;
    }
}

trait LogMessages {
    fn push(&self, entry: String) -> Message<Log, ()>;
    fn entries(&self) -> Message<Log, Vec<String>>;
    fn check(&self, key: &str) -> Message<Log, ()>;
    fn measure(&self, text: &str) -> Message<Log, usize>;
    fn echo(&self, text: String) -> Message<Log, String>;
}

impl LogMessages for Handle<Log> {
    fn push(&self, entry: String) -> Message<Log, ()> {
        self.message(move |c| Box::pin(async move { c.push(entry) }))
    }

    fn entries(&self) -> Message<Log, Vec<String>> {
        self.message(|c| Box::pin(async move { c.entries() }))
    }

    fn check(&self, key: &str) -> Message<Log, ()> {
        let key = key.to_owned();
        self.message(move |c| Box::pin(async move { c.check(&key) }))
    }

    fn measure(&self, text: &str) -> Message<Log, usize> {
        let text = text.to_owned();
        self.message(move |c| Box::pin(async move { c.measure(&text) }))
    }

    fn echo(&self, text: String) -> Message<Log, String> {
        self.message(move |c| Box::pin(async move { c.echo(text) }))
    }
}

/// Records every audited key, and panics checking "audit".
struct Audit {
    checked: Vec<String>,
}

impl Audit {
    fn check(&mut self, key: &str) {
        self.checked.push(key.to_string());
        assert!(key != "audit", "the audit refuses to check itself");
    }

    fn checked(&self) -> Vec<String> {
        self.checked.clone()
    }
}

impl Component for Audit {
    const NAME: &'static str = "Audit";
    const ROLLS_BACK: bool = true;
    type Saved = Vec<String>;

    fn save(&self) -> Vec<String> {
        self.checked.clone()
    }

    fn restore(&mut self, checked: Vec<String>) {
        self.checked = checked;
    }

    fn respond(failure: &Failure) -> Option<Response> {
        match failure {
            // An audit that panics fails only its own request.
            Failure::Panic => Some(Response::Rollback),
            _ => None,
        }
    }
}

trait AuditMessages {
    fn check(&self, key: &str) -> Message<Audit, ()>;
    fn checked(&self) -> Message<Audit, Vec<String>>;
}

impl AuditMessages for Handle<Audit> {
    fn check(&self, key: &str) -> Message<Audit, ()> {
        let key = key.to_owned();
        self.message(move |c| Box::pin(async move { c.check(&key) }))
    }

    fn checked(&self) -> Message<Audit, Vec<String>> {
        self.message(|c| Box::pin(async move { c.checked() }))
    }
}

inset_runtime::export!(Spike);
