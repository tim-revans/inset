//! Pins the diagnostics for malformed input: their wording, and the span
//! each one points at. Each case is a small change to Hello HTTP.

use inset_syntax::{parse_code, parse_requirements};

/// Parses `source` as code and snapshots its rendered diagnostics.
fn code(name: &str, source: &str) {
    let parsed = parse_code(source);
    snapshot(name, "main.inset", source, &parsed.diagnostics);
}

/// Parses `source` as requirements and snapshots its rendered diagnostics.
fn requirements(name: &str, source: &str) {
    let parsed = parse_requirements(source);
    snapshot(name, "hello.req.inset", source, &parsed.diagnostics);
}

fn snapshot(name: &str, path: &str, source: &str, diagnostics: &[inset_syntax::Diagnostic]) {
    assert!(!diagnostics.is_empty(), "{name}: expected diagnostics");
    let rendered: String = diagnostics
        .iter()
        .map(|d| d.render(path, source, false))
        .collect();
    insta::with_settings!({ description => source, omit_expression => true }, {
        insta::assert_snapshot!(name, rendered);
    });
}

#[test]
fn missing_semicolon() {
    code(
        "missing_semicolon",
        r#"app HelloHttp {
    setting port: u16 = 8080
    grant http: HttpServer { port };
    start Greeter { http: move http };
}
"#,
    );
}

#[test]
fn setting_without_default() {
    code(
        "setting_without_default",
        r#"app HelloHttp {
    setting port: u16;
    grant http: HttpServer { port };
}
"#,
    );
}

#[test]
fn handoff_without_notation() {
    code(
        "handoff_without_notation",
        r#"app HelloHttp {
    setting port: u16 = 8080;
    grant http: HttpServer { port };
    start Greeter { http };
}
"#,
    );
}

#[test]
fn unterminated_string() {
    code(
        "unterminated_string",
        r#"component Greeter {
    http: HttpServer,

    #[get("/hello/{name}, via = http)]
    fn hello(name: String) -> String {
        format!("Hello, {name}!")
    }
}
"#,
    );
}

#[test]
fn route_without_via() {
    code(
        "route_without_via",
        r#"component Greeter {
    http: HttpServer,

    #[get("/hello/{name}")]
    fn hello(name: String) -> String {
        format!("Hello, {name}!")
    }
}
"#,
    );
}

#[test]
fn unknown_attribute() {
    code(
        "unknown_attribute",
        r#"component Greeter {
    http: HttpServer,

    #[route("/hello/{name}", via = http)]
    fn hello(name: String) -> String {
        format!("Hello, {name}!")
    }
}
"#,
    );
}

#[test]
fn self_by_value() {
    code(
        "self_by_value",
        r#"component Greeter {
    http: HttpServer,

    #[get("/hello/{name}", via = http)]
    fn hello(self, name: String) -> String {
        format!("Hello, {name}!")
    }
}
"#,
    );
}

#[test]
fn field_after_function() {
    code(
        "field_after_function",
        r#"component Greeter {
    #[get("/hello/{name}", via = http)]
    fn hello(name: String) -> String {
        format!("Hello, {name}!")
    }

    http: HttpServer,
}
"#,
    );
}

#[test]
fn unclosed_component() {
    code(
        "unclosed_component",
        r#"component Greeter {
    http: HttpServer,

    #[get("/hello/{name}", via = http)]
    fn hello(name: String) -> String {
        format!("Hello, {name}!")
    }
"#,
    );
}

#[test]
fn keyword_as_name() {
    code(
        "keyword_as_name",
        r#"fn greeting(name: String) -> String {
    let start = format!("Hello, {name}");
    start + "!"
}
"#,
    );
}

#[test]
fn chained_comparison() {
    code(
        "chained_comparison",
        r#"fn in_range(low: u32, x: u32, high: u32) -> bool {
    low <= x < high
}
"#,
    );
}

#[test]
fn unsupported_construct() {
    code(
        "unsupported_construct",
        r#"fn greeting(name: Option<String>) -> String {
    match name {
        Some(name) => format!("Hello, {name}!"),
        None => "Hello!".to_string(),
    }
}
"#,
    );
}

#[test]
fn unexpected_character() {
    code(
        "unexpected_character",
        r#"fn greeting(name: String) -> String {
    format!("Hello, {}!", $name)
}
"#,
    );
}

/// After an error, the parser carries on from the next member, so a later
/// error is reported too.
#[test]
fn several_errors() {
    code(
        "several_errors",
        r#"app HelloHttp {
    setting port: u16 = 8080
    grant http: HttpServer { port };
    start Greeter { http: move http };
}

component Greeter {
    http: HttpServer,

    #[get("/hello/{name}", via http)]
    fn hello(name: String) -> String {
        format!("Hello, {name}!")
    }
}
"#,
    );
}

#[test]
fn tab_indentation() {
    requirements(
        "tab_indentation",
        "requirement \"Visitors are greeted by the name in the web address\"\n\
         \twhen HelloHttp receives GET /hello/Ada\n    \
         then HelloHttp replies OK with \"Hello, Ada!\"\n",
    );
}

#[test]
fn step_indented_by_two() {
    requirements(
        "step_indented_by_two",
        r#"requirement "Visitors are greeted by the name in the web address"
  when HelloHttp receives GET /hello/Ada
    then HelloHttp replies OK with "Hello, Ada!"
"#,
    );
}

#[test]
fn and_as_first_step() {
    requirements(
        "and_as_first_step",
        r#"requirement "Visitors are greeted by the name in the web address"
    and HelloHttp receives GET /hello/Ada
    then HelloHttp replies OK with "Hello, Ada!"
"#,
    );
}

#[test]
fn lowercase_method() {
    requirements(
        "lowercase_method",
        r#"requirement "Visitors are greeted by the name in the web address"
    when HelloHttp receives get /hello/Ada
    then HelloHttp replies OK with "Hello, Ada!"
"#,
    );
}

#[test]
fn quoted_path() {
    requirements(
        "quoted_path",
        r#"requirement "Visitors are greeted by the name in the web address"
    when HelloHttp receives GET "/hello/Ada"
    then HelloHttp replies OK with "Hello, Ada!"
"#,
    );
}

#[test]
fn unclosed_text_block() {
    requirements(
        "unclosed_text_block",
        r#"requirement "Visitors are greeted by the name in the web address"
    when HelloHttp receives GET /hello/Ada
    then HelloHttp replies OK with
        """
        Hello, Ada!

requirement "Visitors with no name are greeted anyway"
    when HelloHttp receives GET /hello/
    then HelloHttp replies NotFound
"#,
    );
}

#[test]
fn macro_rules_left_out() {
    code(
        "macro_rules_left_out",
        r#"macro_rules! greet {
    () => { "Hello!" };
}
"#,
    );
}

#[test]
fn unknown_failure_condition() {
    code(
        "unknown_failure_condition",
        r#"component Greeter {
    http: HttpServer,

    failure {
        /// A greeting that times out affects only its own request.
        Timeout => rollback,
    }
}
"#,
    );
}

#[test]
fn via_without_server() {
    requirements(
        "via_without_server",
        "requirement \"Visitors are greeted by the name in the web address\"\n\
         \x20   when HelloHttp receives GET /hello/Ada via\n\
         \x20   then HelloHttp replies OK with \"Hello, Ada!\"\n",
    );
}
