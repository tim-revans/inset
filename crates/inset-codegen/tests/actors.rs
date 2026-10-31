//! The Rust generated for components that send each other messages: a
//! named child, a child nested in another, an outlet connected to a
//! sibling and one connected to the parent, `send`, `.await`, a lent
//! argument, and `failure` blocks that restart a child and roll back.

use inset_check::{Program, SourceFile};
use inset_codegen::generate;

const COUNTING: &str = r#"app Counting {
    setting port: u16 = 8080;
    grant http: HttpServer { port };
    start Front { http: move http, seen: 0 };
}

component Front {
    http: HttpServer,
    seen: u32,

    start tally: Tally { total: 0 };
    start log: Log { entries: Vec::new(), counted: send tally.add, logged: send self.logged };

    failure {
        /// A tally that fails starts again from nothing.
        Child(tally) => restart,
        /// The request that made the tally fail fails too.
        Message(tally) => rollback,
    }

    #[get("/count/{n}", via = http)]
    async fn count(&self, n: u32) -> String {
        send self.log.record(n);
        let total = self.tally.total().await;
        format!("{total}")
    }

    #[get("/name/{name}", via = http)]
    async fn name(&self, name: String) -> String {
        self.log.note(&name).await
    }

    #[message]
    fn logged(&mut self, n: u32) {
        self.seen += n;
    }
}

component Tally {
    total: u32,

    #[message]
    fn add(&mut self, n: u32) {
        self.total += n;
    }

    #[message]
    fn total(&self) -> u32 {
        self.total
    }
}

component Log {
    entries: Vec<u32>,
    counted: Outlet<u32>,
    logged: Outlet<u32>,

    start clock: Clock { ticks: 0 };

    #[message]
    async fn record(&mut self, n: u32) {
        self.entries.push(n);
        self.clock.tick().await;
        send self.counted.emit(n);
        send self.logged.emit(n);
    }

    #[message]
    fn note(&self, name: &str) -> String {
        format!("noted {name}")
    }
}

component Clock {
    ticks: u64,

    #[message]
    fn tick(&mut self) {
        self.ticks += 1;
    }
}
"#;

#[test]
fn messages_outlets_children_and_restart() {
    let program = Program::from_files(vec![SourceFile {
        path: "src/main.inset".into(),
        text: COUNTING.into(),
    }]);
    let generated = generate(&program).unwrap_or_else(|reports| {
        let rendered: String = reports.iter().map(|r| program.render(r, false)).collect();
        panic!("the program compiles:\n{rendered}")
    });
    insta::assert_snapshot!(generated.source);
}
