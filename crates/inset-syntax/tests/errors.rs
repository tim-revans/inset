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
        r#"fn spin() -> u32 {
    let mut n = 0;
    loop {
        n += 1;
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

#[test]
fn examples_row_with_too_few_cells() {
    requirements(
        "examples_row_with_too_few_cells",
        r#"requirement "Visitors are greeted by the name in the web address"
    when HelloHttp receives GET /hello/<name>
    then HelloHttp replies OK with "Hello, <name>!"
    examples
    | name | greeting |
    | Ada  |
"#,
    );
}

#[test]
fn examples_heading_repeated() {
    requirements(
        "examples_heading_repeated",
        r#"requirement "Visitors are greeted by the name in the web address"
    when HelloHttp receives GET /hello/<name>
    then HelloHttp replies OK with "Hello, <name>!"
    examples
    | name | <name> | name |
    | Ada  | Ada    | Ada  |
"#,
    );
}

#[test]
fn examples_without_rows() {
    requirements(
        "examples_without_rows",
        r#"requirement "Visitors are greeted by the name in the web address"
    when HelloHttp receives GET /hello/<name>
    then HelloHttp replies OK with "Hello, <name>!"
    examples
    | name |
"#,
    );
}

/// A placeholder no heading names stays as written, so a step it breaks
/// says which placeholder has no column.
#[test]
fn placeholder_without_a_column() {
    requirements(
        "placeholder_without_a_column",
        r#"requirement "Visitors are greeted by the name in the web address"
    when HelloHttp receives GET /hello/Ada
    then HelloHttp replies <status> with "Hello, <name>!"
    examples
    | name |
    | Ada  |
"#,
    );
}

/// A cell that breaks the step is reported once for each row it is in,
/// at the placeholder, naming the row. A mistake outside the placeholders
/// is reported once.
#[test]
fn examples_row_that_breaks_a_step() {
    requirements(
        "examples_row_that_breaks_a_step",
        r#"requirement "Visitors are greeted by the name in the web address"
    when HelloHttp receives GET /hello/Ada
    then HelloHttp replies OK with <greeting>
    examples
    | greeting      |
    | "Hello, Ada!" |
    | "Hello        |
"#,
    );
}

#[test]
fn placeholder_without_examples() {
    requirements(
        "placeholder_without_examples",
        r#"requirement "Visitors are greeted by the name in the web address"
    when HelloHttp receives GET /hello/Ada
    then HelloHttp replies OK with <greeting>
"#,
    );
}

#[test]
fn defensive_without_a_reason() {
    code(
        "defensive_without_a_reason",
        "component C {\n    fn f(n: u32) -> u32 {\n        match n {\n            #[defensive]\n            0 => 1,\n            n => n,\n        }\n    }\n}\n",
    );
}

#[test]
fn another_attribute_on_a_statement() {
    code(
        "another_attribute_on_a_statement",
        "component C {\n    fn f(n: u32) -> u32 {\n        #[message]\n        n\n    }\n}\n",
    );
}

#[test]
fn external_function_with_a_body() {
    code(
        "external_function_with_a_body",
        r#"#[rust(path = "reader.rs")]
extern component Reader {
    #[message]
    fn read(&self, text: &str) -> u64 {
        0
    }
}
"#,
    );
}

#[test]
fn external_with_a_start() {
    code(
        "external_with_a_start",
        r#"#[rust(path = "reader.rs")]
extern component Reader {
    start helper: Helper {};
}
"#,
    );
}

#[test]
fn rust_attribute_without_a_path() {
    code(
        "rust_attribute_without_a_path",
        r#"#[rust(crates = ["csv@1.3"])]
extern component Reader {
    #[message]
    fn read(&self, text: &str) -> u64;
}
"#,
    );
}

#[test]
fn rust_attribute_with_an_unknown_argument() {
    code(
        "rust_attribute_with_an_unknown_argument",
        r#"#[rust(path = "reader.rs", features = ["std"])]
extern component Reader {
    #[message]
    fn read(&self, text: &str) -> u64;
}
"#,
    );
}

/// `fork { a, b }` is Draft, and `fork` needs `for` after it otherwise.
#[test]
fn fork_of_expressions() {
    code(
        "fork_of_expressions",
        r#"fn both(a: u32, b: u32) -> (u32, u32) {
    let pair = fork { a, b };
    let other = fork while a > b { a };
    pair
}
"#,
    );
}

/// A property's placeholders name its inputs, its inputs come before its
/// steps, and it has no examples table.
#[test]
fn property_mistakes() {
    requirements(
        "property_mistakes",
        r#"requirement "Any name is greeted"
    for all n: u32 where n > 0
    when HelloHttp receives GET /hello/<n>
    then HelloHttp replies OK with <greeting>

requirement "Greetings are polite"
    when HelloHttp receives GET /hello/Ada
    for all n: u32
    then HelloHttp replies OK with "Hello, Ada!"

requirement "Every number is greeted"
    for all n: u32
    when HelloHttp receives GET /hello/<n>
    then HelloHttp replies OK with "Hello, <n>!"
    examples
    | n |
    | 1 |

requirement "Inputs are typed"
    for all n u32
    when HelloHttp receives GET /hello/<n>
    then HelloHttp replies OK with "Hello, <n>!"
"#,
    );
}
