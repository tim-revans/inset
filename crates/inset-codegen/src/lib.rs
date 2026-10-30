//! Turns a checked Inset program into Rust, and builds that Rust to one
//! WebAssembly component.
//!
//! [`generate`] checks the program with `inset-check` and emits the Rust for
//! it, which links `inset-runtime`. [`build`] writes that Rust into a Cargo
//! project of its own, builds it for `wasm32-wasip2` and returns the
//! component, which `inset-host` runs. `docs/language/runtime.md` describes
//! the shape of the emitted Rust and why.
//!
//! **Emitting.** Each Inset item becomes Rust items in source order, built
//! as tokens with `quote`, parsed with `syn` and printed with `prettyplease`,
//! so the output is formatted the same way every time. Doc comments are
//! kept. Inside a function Inset is Rust, so bodies are emitted as written.
//! The rest refers to the runtime by its full path, `::inset_runtime::...`,
//! so no name a program chooses, such as a component called `Route`, can
//! clash with it. Rust's own errors in the emitted code are compiler bugs:
//! the checks in `inset-check` must reject the program first.
//!
//! **What is covered.** The constructs Hello HTTP uses: settings,
//! `HttpServer` grants, `start` statements in the app that move grants into
//! components, components whose fields are all `HttpServer`s, route
//! handlers, algorithms, and the Rust inside function bodies. Anything else
//! that `inset check` accepts gets a diagnostic saying it cannot be compiled
//! yet, at the construct, rather than Rust that would not build. Messages,
//! children, state, tables and `failure` blocks follow with the Stock
//! tracker.
//!
//! **Building.** The generated project depends on `inset-runtime` by path,
//! at the copy this crate was built next to, until the runtime is published.
//! It starts from the workspace's `Cargo.lock` and `rust-toolchain.toml`, so
//! a program builds with the dependency versions and toolchain that Inset's
//! own tests use. Its release profile keeps overflow checks, because Inset
//! checks integer arithmetic in every build.

mod body;
mod items;
mod project;

use inset_check::{MAIN, Program, Report};
use inset_syntax::ast::ItemKind;
use inset_syntax::{Diagnostic, Span, parse_code};
use quote::quote;

pub use project::{BuildError, Built, build, cargo_toml};

/// The Rust for a program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Generated {
    /// The Cargo package's name, from the app's: `HelloHttp` is `hello-http`.
    pub package: String,
    /// `src/lib.rs`.
    pub source: String,
}

/// Checks the program and emits its Rust. The reports are the checker's,
/// or else one for each construct that cannot be compiled yet.
pub fn generate(program: &Program) -> Result<Generated, Vec<Report>> {
    let reports = inset_check::check(program);
    if !reports.is_empty() {
        return Err(reports);
    }
    let main = program
        .files
        .iter()
        .position(|f| f.path == MAIN)
        .expect("a checked program has its main file");
    let module = parse_code(&program.files[main].text).ast;
    let app = module
        .items
        .iter()
        .find_map(|item| match &item.kind {
            ItemKind::App(app) => Some(app.name.name.clone()),
            _ => None,
        })
        .expect("a checked program has an app");

    let mut emitter = Emitter::default();
    let items = emitter.module(&module);
    if !emitter.diagnostics.is_empty() {
        return Err(emitter
            .diagnostics
            .into_iter()
            .map(|diagnostic| Report {
                file: main,
                diagnostic,
            })
            .collect());
    }
    let header = format!(" The Rust that `inset build` generated for the app `{app}`.");
    let tokens = quote! {
        #![doc = #header]
        #![doc = " It is written again on every build, so a change to it would be lost."]

        #items
    };
    let file: syn::File = syn::parse2(tokens).expect("the emitted tokens are a Rust file");
    Ok(Generated {
        package: package_name(&app),
        source: spaced(&prettyplease::unparse(&file)),
    })
}

/// Puts a blank line between items, and between the members of an `impl`
/// block, which `prettyplease` leaves out.
fn spaced(source: &str) -> String {
    let indent = |line: &str| line.len() - line.trim_start().len();
    let mut out = String::new();
    let mut in_impl = false;
    let mut prev: Option<&str> = None;
    for line in source.lines() {
        if indent(line) == 0 && line.starts_with("impl") {
            in_impl = true;
        }
        if let Some(prev) = prev {
            let ends_item = prev.ends_with('}') || prev.ends_with(';');
            let starts_item = !line.trim_start().starts_with('}');
            let same = |depth| indent(prev) == depth && indent(line) == depth;
            let crate_docs_end = prev.starts_with("//!") && !line.starts_with("//!");
            if crate_docs_end || (ends_item && starts_item && (same(0) || (in_impl && same(4)))) {
                out.push('\n');
            }
        }
        if line == "}" {
            in_impl = false;
        }
        out.push_str(line);
        out.push('\n');
        prev = Some(line);
    }
    out
}

/// Collects the constructs that cannot be compiled yet while emitting.
#[derive(Default)]
struct Emitter {
    diagnostics: Vec<Diagnostic>,
}

impl Emitter {
    fn unsupported(&mut self, what: &str, span: Span, label: &str) {
        self.diagnostics.push(
            Diagnostic::new(format!("{what} cannot be compiled yet"), span, label).with_help(
                "`inset check` accepts this, but the compiler emits Rust only for what \
                 Hello HTTP uses so far",
            ),
        );
    }
}

/// `HelloHttp` as `hello-http`: a word starts at each capital that follows
/// a lowercase letter or digit, or that starts a word after an acronym.
fn package_name(app: &str) -> String {
    let chars: Vec<char> = app.chars().collect();
    let mut name = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if c.is_ascii_uppercase() && i > 0 {
            let prev = chars[i - 1];
            let next_lower = chars.get(i + 1).is_some_and(char::is_ascii_lowercase);
            if prev.is_ascii_lowercase()
                || prev.is_ascii_digit()
                || (prev.is_ascii_uppercase() && next_lower)
            {
                name.push('-');
            }
        }
        name.push(match c {
            '_' => '-',
            c => c.to_ascii_lowercase(),
        });
    }
    name
}

#[cfg(test)]
mod tests {
    use super::package_name;

    #[test]
    fn package_names_split_words() {
        assert_eq!(package_name("HelloHttp"), "hello-http");
        assert_eq!(package_name("StockTracker"), "stock-tracker");
        assert_eq!(package_name("CSVImport"), "csv-import");
        assert_eq!(package_name("Shop2Go"), "shop2-go");
        assert_eq!(package_name("my_app"), "my-app");
    }
}
