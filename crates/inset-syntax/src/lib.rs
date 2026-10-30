//! Inset's lexer, parser and syntax tree, for both dialects: code in
//! `*.inset` files and requirements in `*.req.inset` files.
//!
//! [`parse_code`] and [`parse_requirements`] always return a syntax tree,
//! with a [`Diagnostic`] for each error found. After an error the parser
//! skips to the next item, member or statement (or, in requirements, the next
//! step) and carries on, so one mistake does not hide the rest, and the tree
//! holds everything that did parse. The language server relies on both.
//!
//! The parser is written by hand, by recursive descent, rather than
//! generated: an agent extending the grammar reads and edits ordinary Rust,
//! and every error message is written where the error is found. The code
//! dialect is Rust-shaped, so its lexer follows Rust's lexical rules, and
//! expressions are read by precedence climbing over Rust's operators.
//! Deeply nested input is reported rather than recursed into, so no input
//! can overflow the stack, and a property test feeds the parser random input
//! to check that it never panics.
//!
//! The grammar covers the part of "Grammar of the used subset" in
//! `docs/language/syntax.md` that Hello HTTP needs, built so the rest slots
//! in: apps, components with fields, `start` statements, `failure` blocks
//! and functions, route and message attributes, and Rust's operators,
//! calls, closures, `let` and `if`. It also reads `move` in a message's
//! arguments and `via` in a `when` step, which the checks in `inset-check`
//! need. Other constructs in the spec, such as `match`, `struct` and
//! requirement facts, are reported as "not supported yet" at the keyword,
//! so an author learns it is the compiler's limit and not their mistake.
//! They are added as the examples that use them are compiled.
//!
//! Spans are byte offsets into one file's source. Diagnostics are rendered
//! with `ariadne`, and their wording is pinned by `insta` snapshot tests, as
//! is Hello HTTP's syntax tree. After a deliberate change, run the tests with
//! `INSTA_UPDATE=always` and review the difference in `tests/snapshots/`
//! before committing it.

pub mod ast;
mod diagnostic;
mod lexer;
mod parser;
mod requirements;
mod span;

pub use diagnostic::Diagnostic;
pub use span::{Ident, Span};

/// A syntax tree, and the errors found while building it. The tree holds
/// whatever parsed, so it is incomplete when there are diagnostics.
#[derive(Clone, Debug)]
pub struct Parsed<T> {
    pub ast: T,
    pub diagnostics: Vec<Diagnostic>,
}

/// Parses one file of the code dialect.
pub fn parse_code(source: &str) -> Parsed<ast::Module> {
    let lexed = lexer::lex(source);
    let mut diagnostics = lexed.diagnostics;
    let mut parser = parser::Parser::new(source, lexed.tokens);
    let ast = parser.parse_module();
    let cutoff = lexed.cutoff.unwrap_or(usize::MAX);
    diagnostics.extend(
        parser
            .diagnostics
            .into_iter()
            .filter(|&(at, _)| at < cutoff)
            .map(|(_, d)| d),
    );
    diagnostics.sort_by_key(|d| d.span.start);
    Parsed { ast, diagnostics }
}

/// Parses one file of the requirements dialect.
pub fn parse_requirements(source: &str) -> Parsed<ast::RequirementsFile> {
    let (ast, mut diagnostics) = requirements::parse(source);
    diagnostics.sort_by_key(|d| d.span.start);
    Parsed { ast, diagnostics }
}

/// Whether a file name is in the requirements dialect, by its `.req.inset`
/// ending. Any other `.inset` file is code.
pub fn is_requirements_file(name: &str) -> bool {
    name.ends_with(".req.inset")
}
