//! Constructs that `inset check` accepts but the compiler cannot emit yet
//! are reported at the construct, so an author knows it is the compiler's
//! limit and not their mistake.

use inset_check::{Program, SourceFile};
use inset_codegen::generate;

/// A child lent to a later sibling, which no example needs yet.
const LENDING: &str = r#"app Lending {
    grant http: HttpServer { port: 8080 };
    start Front { http: move http };
}

component Front {
    http: HttpServer,

    start tally: Tally { total: 0 };
    start reader: Reader { tally: &tally };

    #[get("/total", via = http)]
    async fn total(&self) -> String {
        let total = self.reader.read().await;
        total.to_string()
    }
}

component Tally {
    total: u32,

    #[message]
    fn total(&self) -> u32 {
        self.total
    }
}

component Reader {
    tally: &Handle<Tally>,

    #[message]
    async fn read(&self) -> u32 {
        self.tally.total().await
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
fn a_lent_sibling() {
    insta::assert_snapshot!(errors(LENDING));
}
