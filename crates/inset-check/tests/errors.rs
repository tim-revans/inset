//! Pins the semantic diagnostics: their wording, and the span each one
//! points at. Each case is a small change to Hello HTTP, or to `COUNTING`,
//! a program with messages and an outlet between three components.

use inset_check::{Program, SourceFile, check};

const HELLO: &str = r#"app HelloHttp {
    setting port: u16 = 8080;
    grant http: HttpServer { port };
    start Greeter { http: move http };
}

component Greeter {
    http: HttpServer,

    #[get("/hello/{name}", via = http)]
    fn hello(name: String) -> String {
        format!("Hello, {name}!")
    }
}
"#;

const HELLO_REQ: &str = r#"requirement "Visitors are greeted by the name in the web address"
    when HelloHttp receives GET /hello/Ada
    then HelloHttp replies OK with "Hello, Ada!"
"#;

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
    fn note(&mut self, text: String) {}

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

const COUNTING_REQ: &str = r#"requirement "Each count adds to the total"
    when Counting receives GET /count/2
    then Counting replies OK with "2"
"#;

/// Checks the program and renders its diagnostics.
fn rendered(main: &str, requirements: &str) -> String {
    let program = Program::from_files(vec![
        SourceFile {
            path: "src/main.inset".into(),
            text: main.into(),
        },
        SourceFile {
            path: "requirements/main.req.inset".into(),
            text: requirements.into(),
        },
    ]);
    check(&program)
        .iter()
        .map(|r| program.render(r, false))
        .collect()
}

/// Snapshots the diagnostics, which must not be empty.
fn errors(name: &str, main: &str, requirements: &str) {
    let rendered = rendered(main, requirements);
    assert!(!rendered.is_empty(), "{name}: expected diagnostics");
    insta::with_settings!({ omit_expression => true }, {
        insta::assert_snapshot!(name, rendered);
    });
}

/// Replaces one piece of `source`, which must be there.
fn edit(source: &str, from: &str, to: &str) -> String {
    assert!(source.contains(from), "no `{from}` to replace");
    source.replacen(from, to, 1)
}

#[test]
fn bases_check_clean() {
    assert_eq!(rendered(HELLO, HELLO_REQ), "");
    assert_eq!(rendered(COUNTING, COUNTING_REQ), "");
    // A value moved into a message, and a closure that only computes.
    let main = edit(
        COUNTING,
        "        send self.log.record(n);",
        "        let text = n.to_string();\n        send self.tally.note(move text);\n        \
         let doubled: Vec<u32> = vec![n].iter().map(move |x| x * 2).collect();\n        \
         send self.log.record(doubled[0]);",
    );
    assert_eq!(rendered(&main, COUNTING_REQ), "");
}

// ---- The cases Stage 6 of the plan names ------------------------------------

#[test]
fn undeclared_capability() {
    let main = edit(
        HELLO,
        "    fn hello(name: String) -> String {\n        format!(\"Hello, {name}!\")",
        "    async fn hello(&self, name: String) -> Option<String> {\n        self.greetings.get(&name).await",
    );
    errors("undeclared_capability", &main, HELLO_REQ);
}

#[test]
fn mutable_borrow_across_boundary() {
    let main = edit(
        COUNTING,
        "        send self.log.record(n);",
        "        let mut n = n;\n        send self.tally.add(&mut n);",
    );
    errors("mutable_borrow_across_boundary", &main, COUNTING_REQ);
}

#[test]
fn unknown_route_in_step() {
    let req = edit(HELLO_REQ, "GET /hello/Ada", "GET /helo/Ada");
    errors("unknown_route_in_step", HELLO, &req);
}

#[test]
fn unknown_capability_in_step() {
    let req = edit(HELLO_REQ, "GET /hello/Ada", "GET /hello/Ada via https");
    errors("unknown_capability_in_step", HELLO, &req);
}

#[test]
fn duplicate_requirement() {
    let req = format!(
        "{HELLO_REQ}\n{}",
        edit(HELLO_REQ, "/hello/Ada", "/hello/Bo")
    );
    errors("duplicate_requirement", HELLO, &req);
}

#[test]
fn failure_arm_without_reason() {
    let main = edit(
        COUNTING,
        "    start log: Log { entries: Vec::new(), counted: send tally.add };\n",
        "    start log: Log { entries: Vec::new(), counted: send tally.add };\n\n    \
         failure {\n        Message(tally) => rollback,\n    }\n",
    );
    errors("failure_arm_without_reason", &main, COUNTING_REQ);
}

#[test]
fn closure_that_awaits() {
    let main = edit(
        COUNTING,
        "        let total = self.tally.total().await;",
        "        let total: u32 = vec![1, 2].iter().map(|_| self.tally.total().await).sum();",
    );
    errors("closure_that_awaits", &main, COUNTING_REQ);
}

#[test]
fn unconnected_outlet() {
    let main = edit(COUNTING, ", counted: send tally.add", "");
    errors("unconnected_outlet", &main, COUNTING_REQ);
}

#[test]
fn outlet_connected_to_wrong_type() {
    let main = edit(
        COUNTING,
        "counted: send tally.add",
        "counted: send tally.note",
    );
    errors("outlet_connected_to_wrong_type", &main, COUNTING_REQ);
}

// ---- Other checks ------------------------------------------------------------

#[test]
fn value_across_boundary_without_move() {
    let main = edit(
        COUNTING,
        "        send self.log.record(n);",
        "        let text = n.to_string();\n        send self.tally.note(text);",
    );
    errors("value_across_boundary_without_move", &main, COUNTING_REQ);
}

#[test]
fn message_neither_awaited_nor_sent() {
    let main = edit(COUNTING, "self.tally.total().await", "self.tally.total()");
    errors("message_neither_awaited_nor_sent", &main, COUNTING_REQ);
}

#[test]
fn send_with_reply() {
    let main = edit(
        COUNTING,
        "send self.log.record(n);",
        "send self.tally.total();",
    );
    errors("send_with_reply", &main, COUNTING_REQ);
}

#[test]
fn capability_lent() {
    let main = edit(HELLO, "http: move http", "http: &http");
    errors("capability_lent", &main, HELLO_REQ);
}

#[test]
fn handed_down_field_kept() {
    let main = edit(
        HELLO,
        "    http: HttpServer,",
        "    #[handed_down]\n    http: HttpServer,",
    );
    errors("handed_down_field_kept", &main, HELLO_REQ);
}

#[test]
fn component_starts_itself() {
    let main = edit(
        COUNTING,
        "    total: u32,\n",
        "    total: u32,\n\n    start inner: Tally { total: 0 };\n",
    );
    errors("component_starts_itself", &main, COUNTING_REQ);
}

#[test]
fn route_parameter_unsupplied() {
    let main = edit(HELLO, "\"/hello/{name}\"", "\"/hello\"");
    let req = edit(HELLO_REQ, "/hello/Ada", "/hello");
    errors("route_parameter_unsupplied", &main, &req);
}

#[test]
fn format_names_no_variable() {
    let main = edit(HELLO, "{name}!", "{nme}!");
    errors("format_names_no_variable", &main, HELLO_REQ);
}

#[test]
fn reply_type_mismatch() {
    let main = edit(HELLO, "format!(\"Hello, {name}!\")", "\"Hello!\"");
    errors("reply_type_mismatch", &main, HELLO_REQ);
}

#[test]
fn printing_and_std_io() {
    let main = edit(
        HELLO,
        "        format!(",
        "        println!(\"{name}\");\n        let _ = std::fs::read_to_string(&name);\n        format!(",
    );
    errors("printing_and_std_io", &main, HELLO_REQ);
}

#[test]
fn setting_and_grant_types() {
    let main = edit(
        HELLO,
        "setting port: u16 = 8080;",
        "setting port: u16 = 80800;\n    setting host: String = \"localhost\";",
    );
    let main = edit(&main, "HttpServer { port }", "HttpServer { port: host }");
    errors("setting_and_grant_types", &main, HELLO_REQ);
}

#[test]
fn step_path_of_wrong_type() {
    let req = edit(COUNTING_REQ, "/count/2", "/count/two");
    errors("step_path_of_wrong_type", COUNTING, &req);
}

#[test]
fn statuses() {
    let req = format!(
        "{}\n{}",
        edit(HELLO_REQ, "replies OK", "replies Ok"),
        edit(
            &edit(
                HELLO_REQ,
                "replies OK with \"Hello, Ada!\"",
                "replies NotFound"
            ),
            "Visitors",
            "Strangers"
        ),
    );
    errors("statuses", HELLO, &req);
}

#[test]
fn self_without_receiver() {
    let main = edit(
        HELLO,
        "format!(\"Hello, {name}!\")",
        "format!(\"{}\", self.http)",
    );
    errors("self_without_receiver", &main, HELLO_REQ);
}
