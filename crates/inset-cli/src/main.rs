//! The `inset` command line.
//!
//! `inset check <dir>` checks the program in a directory: its code in
//! `src/` and its requirements in `requirements/`. It prints nothing when
//! the program is sound.
//!
//! `inset parse <file>` prints a file's syntax tree, for debugging the
//! parser. The dialect follows from the file name: `*.req.inset` is
//! requirements and any other file is code.
//!
//! Diagnostics go to standard error, and either command fails if there are any.

use std::io::{IsTerminal, Write};
use std::process::ExitCode;

use inset_check::Program;
use inset_syntax::{Diagnostic, is_requirements_file, parse_code, parse_requirements};

const USAGE: &str = "usage: inset check <dir>\n       inset parse <file>";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["check", dir] => check(dir),
        ["parse", path] => parse(path),
        _ => {
            eprintln!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}

fn check(dir: &str) -> ExitCode {
    let program = match Program::load(std::path::Path::new(dir)) {
        Ok(program) => program,
        Err(e) => {
            eprintln!("inset: {e}");
            return ExitCode::FAILURE;
        }
    };
    let reports = inset_check::check(&program);
    let color = std::io::stderr().is_terminal();
    for report in &reports {
        eprint!("{}", program.render(report, color));
    }
    if reports.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn parse(path: &str) -> ExitCode {
    let source = match std::fs::read_to_string(path) {
        Ok(source) => source,
        Err(e) => {
            eprintln!("inset: cannot read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let diagnostics = if is_requirements_file(path) {
        let parsed = parse_requirements(&source);
        print_tree(&parsed.ast);
        parsed.diagnostics
    } else {
        let parsed = parse_code(&source);
        print_tree(&parsed.ast);
        parsed.diagnostics
    };
    report(path, &source, &diagnostics)
}

/// Prints the tree, ignoring a closed pipe, as when piped into `head`.
fn print_tree(tree: &impl std::fmt::Debug) {
    let _ = writeln!(std::io::stdout().lock(), "{tree:#?}");
}

fn report(path: &str, source: &str, diagnostics: &[Diagnostic]) -> ExitCode {
    let color = std::io::stderr().is_terminal();
    for diagnostic in diagnostics {
        eprint!("{}", diagnostic.render(path, source, color));
    }
    if diagnostics.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
