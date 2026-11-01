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
//! **What is covered.** Settings, `HttpServer` and `Table` grants, `start`
//! statements in the app and in components, with `#[handed_down]` fields,
//! named children and outlets connected to a message, components holding
//! capabilities and state, route and message handlers, `send` and awaited
//! messages, `async` handlers that await capability calls, `failure` blocks
//! that roll back and restart, algorithms, and the Rust inside function
//! bodies. Also modules,
//! each emitted as a Rust module of the same name, so paths and privacy
//! mean the same in both, and structs and enums, with an error enum's
//! `#[status]` attributes becoming the runtime's `Status`, and every data
//! type implementing the runtime's `Stored`, so a table can hold it and a
//! component's state can be saved. `dec!` is emitted as the `Decimal` it
//! stands for. Anything else that `inset check` accepts gets a diagnostic
//! saying it cannot be compiled yet, at the construct, rather than Rust that
//! would not build, such as a child lent to a sibling, which no example
//! needs yet.
//!
//! **External components.** An `extern component` is emitted as a
//! component whose handlers pass each message to the host, and is built
//! as a WebAssembly component of its own from a WIT world written from its
//! declaration (`external.rs`), with Rust as the first language adapter
//! (`external/rust.rs`).
//!
//! **Coverage.** The emitted code marks each function, branch, `?` error
//! path and `failure` arm it reaches, by its diagram ID, so `inset test` can
//! tell which requirement reached what. `coverage.rs` walks each body as the
//! diagram's flowchart does, so the IDs agree, and a test checks that they
//! do for every example and test program.
//!
//! **Building.** The generated project depends on `inset-runtime` by path,
//! at the copy this crate was built next to, until the runtime is published.
//! It starts from the workspace's `Cargo.lock` and `rust-toolchain.toml`, so
//! a program builds with the dependency versions and toolchain that Inset's
//! own tests use. Its release profile keeps overflow checks, because Inset
//! checks integer arithmetic in every build.
//!
//! Every generated project is a workspace root of its own, so Cargo gives
//! two programs with the same app name the same fingerprints. In a target
//! directory other projects share, Cargo could then take one program's
//! build as the other's, so the package is cleaned before each build there.

mod body;
mod coverage;
mod external;
mod items;
mod project;

use std::collections::HashMap;

use inset_check::{Program, Report, Resolved};
use inset_syntax::ast::{Component, ItemKind};
use inset_syntax::{Diagnostic, Span};
use quote::quote;

pub use coverage::Elements;
pub use external::rust::{LOCK_FILE, interface_lib};
pub use external::world;
pub use project::{BuildError, Built, build, cargo_toml};

/// The Rust for a program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Generated {
    /// The Cargo package's name, from the app's: `HelloHttp` is `hello-http`.
    pub package: String,
    /// `src/lib.rs`.
    pub source: String,
    /// What coverage counts, as the emitted code marks it.
    pub elements: Elements,
    /// The external components, each built as a component of its own.
    pub externals: Vec<inset_check::External>,
}

/// Checks the program and emits its Rust. The reports are the checker's,
/// or else one for each construct that cannot be compiled yet.
pub fn generate(program: &Program) -> Result<Generated, Vec<Report>> {
    let resolved = inset_check::resolve(program)?;
    let modules = &resolved.modules;
    let app = modules.list[0]
        .ast
        .items
        .iter()
        .find_map(|item| match &item.kind {
            ItemKind::App(app) => Some(app.name.name.clone()),
            _ => None,
        })
        .expect("a checked program has an app");

    let mut emitter = Emitter::new(&resolved, &app);
    emitter.web = program
        .web
        .iter()
        .flatten()
        .map(|asset| asset.path.clone())
        .collect();
    let items = emitter.module(modules, 0);
    if !emitter.diagnostics.is_empty() {
        return Err(emitter
            .diagnostics
            .into_iter()
            .filter_map(|diagnostic| modules.report(diagnostic))
            .collect());
    }
    let header = format!(" The Rust that `inset build` generated for the app `{app}`.");
    let algorithms = emitter.has_algorithms.then(|| {
        let ids = &emitter.elements.algorithms;
        quote! {
            #[doc = " The elements coverage counts in the top-level algorithms, by their diagram IDs."]
            const ALGORITHM_BRANCHES: &[&str] = &[#(#ids),*];
        }
    });
    let tokens = quote! {
        #![doc = #header]
        #![doc = " It is written again on every build, so a change to it would be lost."]

        #items

        #algorithms
    };
    let file: syn::File = syn::parse2(tokens).expect("the emitted tokens are a Rust file");
    Ok(Generated {
        package: package_name(&app),
        source: spaced(&prettyplease::unparse(&file)),
        elements: emitter.elements,
        externals: resolved.externals.clone(),
    })
}

/// Puts a blank line between items, in the file and in each module, and
/// between the members of an `impl` block, which `prettyplease` leaves out.
fn spaced(source: &str) -> String {
    let indent = |line: &str| line.len() - line.trim_start().len();
    let mut out = String::new();
    // The indentation of each block whose entries are items or members.
    let mut containers = vec![0];
    let mut prev: Option<&str> = None;
    for line in source.lines() {
        let depth = indent(line);
        let trimmed = line.trim_start();
        if trimmed.starts_with('}') {
            containers.retain(|&c| c < depth + 4);
        }
        if let Some(prev) = prev {
            let ends_item = prev.ends_with('}') || prev.ends_with(';');
            let starts_item = !trimmed.starts_with('}');
            let same = indent(prev) == depth && containers.last() == Some(&depth);
            let crate_docs_end = prev.starts_with("//!") && !line.starts_with("//!");
            if crate_docs_end || (ends_item && starts_item && same) {
                out.push('\n');
            }
        }
        let opens_container = line.ends_with('{')
            && containers.last() == Some(&depth)
            && (trimmed.starts_with("impl")
                || trimmed.starts_with("mod ")
                || trimmed
                    .split_once(" mod ")
                    .is_some_and(|(vis, _)| vis.starts_with("pub")));
        if opens_container {
            containers.push(depth + 4);
        }
        out.push_str(line);
        out.push('\n');
        prev = Some(line);
    }
    out
}

/// Collects the constructs that cannot be compiled yet while emitting.
struct Emitter<'a> {
    /// What each name in the code refers to, as the checker resolved it.
    refs: &'a HashMap<Span, Span>,
    /// Every component, by the span of its name.
    components: HashMap<Span, &'a Component>,
    /// The paths of the front end's files, from `web/`.
    web: Vec<String>,
    diagnostics: Vec<Diagnostic>,
    /// The app's name, which starts every diagram ID.
    app: String,
    /// The path of the module being emitted, from the root.
    module_path: Vec<String>,
    /// Whether any module has a top-level algorithm.
    has_algorithms: bool,
    /// The names of the external components.
    externals: Vec<String>,
    /// Whose elements the function being emitted marks.
    owner: coverage::Owner,
    /// The elements of the component being emitted.
    branches: Vec<String>,
    /// Every element listed so far.
    elements: Elements,
    /// The index of the error path of the `?` in the step being emitted.
    exit: Option<usize>,
}

impl<'a> Emitter<'a> {
    fn new(resolved: &'a Resolved, app: &str) -> Emitter<'a> {
        let components = resolved
            .modules
            .list
            .iter()
            .flat_map(|module| &module.ast.items)
            .filter_map(|item| match &item.kind {
                ItemKind::Component(c) => Some((c.name.span, c)),
                _ => None,
            })
            .collect();
        let has_algorithms = resolved
            .modules
            .list
            .iter()
            .flat_map(|module| &module.ast.items)
            .any(|item| matches!(item.kind, ItemKind::Function(_)));
        Emitter {
            refs: &resolved.refs,
            components,
            web: Vec::new(),
            diagnostics: Vec::new(),
            app: app.to_string(),
            module_path: Vec::new(),
            has_algorithms,
            externals: resolved.externals.iter().map(|e| e.name.clone()).collect(),
            owner: coverage::Owner::Algorithms,
            branches: Vec::new(),
            elements: Elements::default(),
            exit: None,
        }
    }

    /// The component a `start` statement starts.
    fn started(&self, start: &inset_syntax::ast::Start) -> Option<&'a Component> {
        let target = self.refs.get(&start.component.span)?;
        self.components.get(target).copied()
    }

    fn unsupported(&mut self, what: &str, span: Span, label: &str) {
        self.diagnostics.push(
            Diagnostic::new(format!("{what} cannot be compiled yet"), span, label).with_help(
                "`inset check` accepts this, but the compiler does not emit Rust for it yet",
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
