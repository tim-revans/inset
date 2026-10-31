//! Constructs that `inset check` accepts but the compiler cannot emit yet
//! are reported at the construct, so an author knows it is the compiler's
//! limit and not their mistake.

use inset_check::{Program, SourceFile};
use inset_codegen::generate;

/// Messages, an outlet, named and nested children, component state,
/// `send` and `.await`.
const COUNTING: &str = r#"app Counting {
    setting port: u16 = 8080;
    grant http: HttpServer { port };
    start Front { http: move http };
}

component Front {
    http: HttpServer,

    start tally: Tally { total: 0 };
    start log: Log { entries: Vec::new(), counted: send tally.add };

    #[get("/count/{n}", via = http)]
    async fn count(&self, n: u32) -> String {
        send self.log.record(n);
        let total = self.tally.total().await;
        format!("{total}")
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

    #[message]
    fn record(&mut self, n: u32) {
        self.entries.push(n);
        send self.counted.emit(n);
    }
}
"#;

/// A `failure` block that restarts.
const RESTARTING: &str = r#"app Restarting {
    setting port: u16 = 8080;
    grant http: HttpServer { port };
    grant names: Table<String, String> { file: "names.db", name: "names" };
    start Names { http: move http, names: move names };
}

component Names {
    http: HttpServer,
    names: Table<String, String>,

    failure {
        /// A lost write is retried by the client.
        Capability(names) => rollback,
        /// A bug leaves nothing behind that a fresh start would not clear.
        Panic => restart,
    }

    #[get("/names/{key}", via = http)]
    async fn get(&self, key: String) -> Option<String> {
        self.names.get(&key).await
    }
}
"#;

fn errors(source: &str) -> String {
    let program = Program::from_files(vec![SourceFile {
        path: "src/main.inset".into(),
        text: source.into(),
    }]);
    assert!(
        inset_check::check(&program).is_empty(),
        "the program checks clean"
    );
    let reports = generate(&program).expect_err("the program cannot be compiled yet");
    reports.iter().map(|r| program.render(r, false)).collect()
}

#[test]
fn messages_outlets_and_children() {
    insta::assert_snapshot!(errors(COUNTING));
}

#[test]
fn restart() {
    insta::assert_snapshot!(errors(RESTARTING));
}
