//! The parser must never panic, whatever it is given. These tests feed both
//! dialects random input and render every diagnostic, which also checks that
//! every span lies within the source on a character boundary.

use inset_syntax::{parse_code, parse_requirements};
use proptest::prelude::*;

const HELLO: &str = include_str!("../../../examples/hello-http/src/main.inset");
const HELLO_REQ: &str = include_str!("../../../examples/hello-http/requirements/hello.req.inset");
const WAREHOUSE: &str = include_str!("../../../examples/stock-tracker/src/warehouse.inset");

/// Fragments that reach deep into both grammars when strung together.
const VOCABULARY: &[&str] = &[
    "app",
    "component",
    "setting",
    "grant",
    "start",
    "send",
    "fork",
    "fn",
    "async",
    "let",
    "if",
    "else",
    "return",
    "move",
    "mut",
    "self",
    "Self",
    "pub",
    "pub(crate)",
    "match",
    "for",
    "in",
    "while",
    "struct",
    "enum",
    "mod",
    "use",
    "=>",
    "..",
    "|",
    "::",
    "{",
    "}",
    "#[derive(Clone)]",
    "#[status(NotFound)]",
    "Greeter",
    "http",
    "x",
    "u16",
    "String",
    "Vec",
    "Option",
    "8080",
    "1.5",
    "\"text\"",
    "'c'",
    "'a",
    "{",
    "}",
    "(",
    ")",
    "[",
    "]",
    "<",
    ">",
    ">>",
    "=",
    "==",
    "<=",
    "&&",
    "&",
    "||",
    "!",
    "?",
    ".",
    "..",
    "::",
    ":",
    ";",
    ",",
    "->",
    "=>",
    "#",
    "#[get(\"/a\", via = http)]",
    "#[message]",
    "#[handed_down]",
    "///doc\n",
    "//c\n",
    "/*",
    "*/",
    "format!",
    "r#\"",
    "\"#",
    "\"",
    "\\",
    "\n",
    "    ",
    "\t",
    "requirement",
    "when",
    "then",
    "and",
    "given",
    "receives",
    "replies",
    "with",
    "GET",
    "/hello/Ada",
    "OK",
    "\"\"\"",
    "é",
    "\r\n",
];

fn check(source: &str) {
    for diagnostic in parse_code(source).diagnostics {
        diagnostic.render("main.inset", source, false);
    }
    for diagnostic in parse_requirements(source).diagnostics {
        diagnostic.render("hello.req.inset", source, false);
    }
}

fn soup() -> impl Strategy<Value = String> {
    prop::collection::vec(prop::sample::select(VOCABULARY), 0..200)
        .prop_map(|parts| parts.join(" "))
}

/// A program with one span of characters replaced, so most of the input
/// is valid and the error lands somewhere inside a real program.
fn mutated(source: &'static str) -> impl Strategy<Value = String> {
    let len = source.chars().count();
    (0..=len, 0..8usize, soup()).prop_map(move |(at, cut, insert)| {
        let chars: Vec<char> = source.chars().collect();
        let end = (at + cut).min(len);
        let mut out: String = chars[..at].iter().collect();
        out.extend(insert.chars().take(12));
        out.extend(&chars[end..]);
        out
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    #[test]
    fn random_bytes(bytes in prop::collection::vec(any::<u8>(), 0..400)) {
        check(&String::from_utf8_lossy(&bytes));
    }

    #[test]
    fn random_tokens(source in soup()) {
        check(&source);
    }

    #[test]
    fn mutated_hello(source in mutated(HELLO)) {
        check(&source);
    }

    #[test]
    fn mutated_modules_and_types(source in mutated(WAREHOUSE)) {
        check(&source);
    }

    #[test]
    fn mutated_requirements(source in mutated(HELLO_REQ)) {
        check(&source);
    }
}

/// Nesting far past the limit is reported, not recursed into.
#[test]
fn deep_nesting() {
    let depth = 10_000;
    let nest = |open: &str, inner: &str, close: &str| {
        format!("{}{inner}{}", open.repeat(depth), close.repeat(depth))
    };
    let sources = [
        format!("fn f() {{ {} }}", nest("(", "x", ")")),
        format!("fn f() {{ {} }}", nest("{", "x", "}")),
        format!("fn f() {{ {} }}", nest("[", "x", "]")),
        format!("fn f() {{ {} }}", nest("-", "x", "")),
        format!("fn f() {{ {} }}", nest("&", "x", "")),
        format!("fn f() {{ {} }}", nest("if x {", "x", "}")),
        format!("fn f() -> {} {{}}", nest("Vec<", "u8", ">")),
        format!("fn f() -> {} {{}}", nest("&", "u8", "")),
        format!("fn f() {{ let {} = x; }}", nest("(", "x", ")")),
        format!("fn f() {{ {} }}", nest("match x { _ => ", "x", "}")),
        format!("fn f() {{ {} }}", nest("for x in y {", "x", "}")),
        format!("fn f() {{ let {} = x; }}", nest("Some(", "x", ")")),
        format!("use {};", nest("a::{", "b", "}")),
    ];
    for source in sources {
        let parsed = parse_code(&source);
        assert!(
            !parsed.diagnostics.is_empty(),
            "{}: expected a diagnostic",
            &source[..20]
        );
        check(&source);
    }
}
