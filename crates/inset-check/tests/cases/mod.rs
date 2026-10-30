//! The negative cases of the semantic checks, shared by the checker's tests
//! and the language server's, which must report the same diagnostics. Each
//! case is a small change to Hello HTTP, or to `COUNTING`, a program with
//! messages and an outlet between three components.

pub const HELLO: &str = r#"app HelloHttp {
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

pub const HELLO_REQ: &str = r#"requirement "Visitors are greeted by the name in the web address"
    when HelloHttp receives GET /hello/Ada
    then HelloHttp replies OK with "Hello, Ada!"
"#;

pub const COUNTING: &str = r#"app Counting {
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

pub const COUNTING_REQ: &str = r#"requirement "Each count adds to the total"
    when Counting receives GET /count/2
    then Counting replies OK with "2"
"#;

/// A broken program: its code and its requirements.
pub struct Case {
    pub name: &'static str,
    pub main: String,
    pub requirements: String,
}

/// Replaces one piece of `source`, which must be there.
pub fn edit(source: &str, from: &str, to: &str) -> String {
    assert!(source.contains(from), "no `{from}` to replace");
    source.replacen(from, to, 1)
}

/// Every case, each of which has diagnostics.
pub fn all() -> Vec<Case> {
    let mut cases = Vec::new();
    let mut add = |name, main: String, requirements: String| {
        cases.push(Case {
            name,
            main,
            requirements,
        })
    };
    {
        let main = edit(
            HELLO,
            "    fn hello(name: String) -> String {\n        format!(\"Hello, {name}!\")",
            "    async fn hello(&self, name: String) -> Option<String> {\n        self.greetings.get(&name).await",
        );
        add("undeclared_capability", main, HELLO_REQ.to_string());
    }
    {
        let main = edit(
            COUNTING,
            "        send self.log.record(n);",
            "        let mut n = n;\n        send self.tally.add(&mut n);",
        );
        add(
            "mutable_borrow_across_boundary",
            main,
            COUNTING_REQ.to_string(),
        );
    }
    {
        let req = edit(HELLO_REQ, "GET /hello/Ada", "GET /helo/Ada");
        add("unknown_route_in_step", HELLO.to_string(), req);
    }
    {
        let req = edit(HELLO_REQ, "GET /hello/Ada", "GET /hello/Ada via https");
        add("unknown_capability_in_step", HELLO.to_string(), req);
    }
    {
        let req = format!(
            "{HELLO_REQ}\n{}",
            edit(HELLO_REQ, "/hello/Ada", "/hello/Bo")
        );
        add("duplicate_requirement", HELLO.to_string(), req);
    }
    {
        let main = edit(
            COUNTING,
            "    start log: Log { entries: Vec::new(), counted: send tally.add };\n",
            "    start log: Log { entries: Vec::new(), counted: send tally.add };\n\n    \
             failure {\n        Message(tally) => rollback,\n    }\n",
        );
        add("failure_arm_without_reason", main, COUNTING_REQ.to_string());
    }
    {
        let main = edit(
            COUNTING,
            "        let total = self.tally.total().await;",
            "        let total: u32 = vec![1, 2].iter().map(|_| self.tally.total().await).sum();",
        );
        add("closure_that_awaits", main, COUNTING_REQ.to_string());
    }
    {
        let main = edit(COUNTING, ", counted: send tally.add", "");
        add("unconnected_outlet", main, COUNTING_REQ.to_string());
    }
    {
        let main = edit(
            COUNTING,
            "counted: send tally.add",
            "counted: send tally.note",
        );
        add(
            "outlet_connected_to_wrong_type",
            main,
            COUNTING_REQ.to_string(),
        );
    }
    {
        let main = edit(
            COUNTING,
            "        send self.log.record(n);",
            "        let text = n.to_string();\n        send self.tally.note(text);",
        );
        add(
            "value_across_boundary_without_move",
            main,
            COUNTING_REQ.to_string(),
        );
    }
    {
        let main = edit(COUNTING, "self.tally.total().await", "self.tally.total()");
        add(
            "message_neither_awaited_nor_sent",
            main,
            COUNTING_REQ.to_string(),
        );
    }
    {
        let main = edit(
            COUNTING,
            "send self.log.record(n);",
            "send self.tally.total();",
        );
        add("send_with_reply", main, COUNTING_REQ.to_string());
    }
    {
        let main = edit(HELLO, "http: move http", "http: &http");
        add("capability_lent", main, HELLO_REQ.to_string());
    }
    {
        let main = edit(
            HELLO,
            "    http: HttpServer,",
            "    #[handed_down]\n    http: HttpServer,",
        );
        add("handed_down_field_kept", main, HELLO_REQ.to_string());
    }
    {
        let main = edit(
            COUNTING,
            "    total: u32,\n",
            "    total: u32,\n\n    start inner: Tally { total: 0 };\n",
        );
        add("component_starts_itself", main, COUNTING_REQ.to_string());
    }
    {
        let main = edit(HELLO, "\"/hello/{name}\"", "\"/hello\"");
        let req = edit(HELLO_REQ, "/hello/Ada", "/hello");
        add("route_parameter_unsupplied", main, req);
    }
    {
        let main = edit(HELLO, "{name}!", "{nme}!");
        add("format_names_no_variable", main, HELLO_REQ.to_string());
    }
    {
        let main = edit(HELLO, "format!(\"Hello, {name}!\")", "\"Hello!\"");
        add("reply_type_mismatch", main, HELLO_REQ.to_string());
    }
    {
        let main = edit(
            HELLO,
            "        format!(",
            "        println!(\"{name}\");\n        let _ = std::fs::read_to_string(&name);\n        format!(",
        );
        add("printing_and_std_io", main, HELLO_REQ.to_string());
    }
    {
        let main = edit(
            HELLO,
            "setting port: u16 = 8080;",
            "setting port: u16 = 80800;\n    setting host: String = \"localhost\";",
        );
        let main = edit(&main, "HttpServer { port }", "HttpServer { port: host }");
        add("setting_and_grant_types", main, HELLO_REQ.to_string());
    }
    {
        let req = edit(COUNTING_REQ, "/count/2", "/count/two");
        add("step_path_of_wrong_type", COUNTING.to_string(), req);
    }
    {
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
        add("statuses", HELLO.to_string(), req);
    }
    {
        let main = edit(
            HELLO,
            "format!(\"Hello, {name}!\")",
            "format!(\"{}\", self.http)",
        );
        add("self_without_receiver", main, HELLO_REQ.to_string());
    }
    cases
}
