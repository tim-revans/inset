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
//! `docs/language/syntax.md` that the three examples need, built so the
//! rest slots in: apps, components with fields, `start` statements,
//! `failure` blocks and functions, `extern component`, route, message,
//! `derive`, `status` and `rust` attributes, `struct`, `enum`, `mod` and
//! `use`, and Rust's operators, calls, closures, ranges, `let`, `if`,
//! `if let`, `match`, `for`, `while` and `fork for`. It also reads `move` in
//! a message's arguments and `via` in a `when` step, which the checks in
//! `inset-check` need. In requirements it reads `given`, `when` and `then`
//! steps, with the facts and checks about capabilities, whose values and
//! conditions are Rust expressions read by the code dialect's parser,
//! `examples` tables and `for all`. A requirement with a table has its
//! steps read once for each row, from the steps' text with the row's cells
//! in place of the placeholders, and every span mapped back to the file: a
//! span from a cell becomes its placeholder's. A property's steps are read
//! the same way, once with a sample value for each input so they can be
//! checked, and again by the test runner with each generated case
//! ([`read_property_case`]). Other constructs in the spec, such as
//! `fork { ... }`, are reported as "not supported yet" where they are
//! written, so an author learns it is the compiler's limit and not their
//! mistake. They are added as the examples that use them are compiled.
//!
//! Spans are byte offsets into one file's source, or, from
//! [`parse_code_at`], into a whole program's, where each file starts at an
//! offset of its own. Diagnostics are rendered
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
    parse_code_at(source, 0)
}

/// Parses one file of the code dialect, with every span, in the tree and in
/// the diagnostics, moved on by `base`. A program of several files gives
/// each file a range of offsets of its own, as rustc's source map does, so a
/// span alone says which file it is in.
pub fn parse_code_at(source: &str, base: usize) -> Parsed<ast::Module> {
    let mut lexed = lexer::lex(source);
    for token in &mut lexed.tokens {
        token.span = token.span.shifted(base);
    }
    let mut diagnostics: Vec<Diagnostic> = lexed
        .diagnostics
        .into_iter()
        .map(|d| d.shifted(base))
        .collect();
    let mut parser = parser::Parser::new(source, base, lexed.tokens);
    let ast = parser.parse_module();
    let cutoff = lexed.cutoff.map_or(usize::MAX, |c| c + base);
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

/// Parses the fact or check in a requirement step, which spans `span` of
/// a requirements file's `source`, with the code dialect's parser.
///
/// In an examples row, `source` is the row's text and `row` maps its spans
/// back to the file, so the tokens, and so the syntax tree and the
/// diagnostics, have the file's spans.
fn parse_step_fact(
    source: &str,
    span: Span,
    allow_check: bool,
    row: Option<&requirements::RowMap>,
) -> (Option<parser::StepFact>, Vec<Diagnostic>) {
    parse_step_part(source, span, row, |parser| {
        parser.parse_step_fact(allow_check)
    })
}

/// Parses a step's body written as a value, as [`parse_step_fact`] parses a fact.
fn parse_step_value(
    source: &str,
    span: Span,
    row: Option<&requirements::RowMap>,
) -> (Option<ast::Expr>, Vec<Diagnostic>) {
    parse_step_part(source, span, row, |parser| parser.parse_step_value())
}

/// Parses a property's inputs and condition, after `for all`, as
/// [`parse_step_fact`] parses a fact.
fn parse_for_all(source: &str, span: Span) -> (Option<parser::ForAllParts>, Vec<Diagnostic>) {
    parse_step_part(source, span, None, |parser| parser.parse_for_all())
}

/// A property's steps read with one case's values in place of its inputs'
/// placeholders, as the test runner reads them for each generated case.
/// `source` is the requirements file the property is in, `steps` is
/// [`ast::ForAll::steps`], and `values` are the text of each input's value,
/// in the order of `names`. The spans are the file's, with a span from a
/// value mapped to its placeholder.
pub fn read_property_case(
    source: &str,
    steps: Span,
    names: &[String],
    values: &[String],
) -> (Vec<ast::Step>, Vec<Diagnostic>) {
    requirements::read_property_case(source, steps, names, values)
}

fn parse_step_part<T>(
    source: &str,
    span: Span,
    row: Option<&requirements::RowMap>,
    parse: impl FnOnce(&mut parser::Parser<'_>) -> parser::PResult<T>,
) -> (Option<T>, Vec<Diagnostic>) {
    let text = &source[span.start..span.end];
    let map = |s: Span| row.map_or(s, |row| row.span(s));
    let mut lexed = lexer::lex(text);
    for token in &mut lexed.tokens {
        token.span = map(token.span.shifted(span.start));
    }
    let mut diagnostics: Vec<Diagnostic> = lexed
        .diagnostics
        .into_iter()
        .map(|d| {
            let mut d = d.shifted(span.start);
            d.span = map(d.span);
            d
        })
        .collect();
    let base = map(span).start;
    let mut parser = parser::Parser::new(text, base, lexed.tokens);
    let parsed = parse(&mut parser).ok();
    diagnostics.extend(parser.diagnostics.into_iter().map(|(_, d)| d));
    (parsed, diagnostics)
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
