//! The `inset` command line.
//!
//! `inset check <dir>` checks the program in a directory: its code in
//! `src/` and its requirements in `requirements/`. It prints nothing when
//! the program is sound.
//!
//! `inset build <dir>` checks the program and compiles it to a WebAssembly
//! component. The generated Cargo project is written to `<dir>/target/inset/`
//! and the component's path is printed.
//!
//! `inset run <dir> [--set name=value]...` builds the program and runs it in
//! `inset-host` with real capabilities, applying each `--set` to a setting,
//! until it stops. Relative paths in grants resolve against the directory
//! `inset run` is run from.
//!
//! `inset parse <file>` prints a file's syntax tree, for debugging the
//! parser. The dialect follows from the file name: `*.req.inset` is
//! requirements and any other file is code.
//!
//! Diagnostics go to standard error, and every command fails if there are any.
//!
//! `inset lsp` runs the language server over standard input and output, for
//! an editor to start. It is part of this binary so the editor and the
//! command line always check with the same compiler.

use std::io::{IsTerminal, Write};
use std::process::ExitCode;

use std::path::{Path, PathBuf};

use inset_check::Program;
use inset_codegen::BuildError;
use inset_syntax::{Diagnostic, is_requirements_file, parse_code, parse_requirements};

const USAGE: &str = "usage: inset check <dir>
       inset build <dir>
       inset run <dir> [--set name=value]...
       inset parse <file>
       inset lsp";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["check", dir] => check(dir),
        ["build", dir] => match build(dir) {
            Ok(component) => {
                println!("{}", component.display());
                ExitCode::SUCCESS
            }
            Err(code) => code,
        },
        ["run", dir, sets @ ..] => run(dir, sets),
        ["parse", path] => parse(path),
        ["lsp"] => lsp(),
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

/// Builds the program in `dir`, printing any diagnostics, and returns its component.
fn build(dir: &str) -> Result<PathBuf, ExitCode> {
    let dir = Path::new(dir);
    let program = Program::load(dir).map_err(|e| {
        eprintln!("inset: {e}");
        ExitCode::FAILURE
    })?;
    let out = dir.join("target").join("inset");
    match inset_codegen::build(&program, &out, None) {
        Ok(built) => Ok(built.component),
        Err(BuildError::Rejected(reports)) => {
            let color = std::io::stderr().is_terminal();
            for report in &reports {
                eprint!("{}", program.render(report, color));
            }
            Err(ExitCode::FAILURE)
        }
        Err(e) => {
            eprintln!("inset: {e}");
            Err(ExitCode::FAILURE)
        }
    }
}

fn run(dir: &str, sets: &[&str]) -> ExitCode {
    let mut settings = Vec::new();
    let mut sets = sets.iter();
    while let Some(arg) = sets.next() {
        let setting = match (*arg, sets.next()) {
            ("--set", Some(setting)) => setting,
            _ => {
                eprintln!("{USAGE}");
                return ExitCode::FAILURE;
            }
        };
        let Some((name, value)) = setting.split_once('=') else {
            eprintln!("inset: `--set {setting}` is not `--set name=value`");
            return ExitCode::FAILURE;
        };
        settings.push((name.to_string(), value.to_string()));
    }
    let component = match build(dir) {
        Ok(component) => component,
        Err(code) => return code,
    };
    let served = std::env::current_dir()
        .map_err(|e| e.to_string())
        .and_then(|working_dir| {
            let options = inset_host::Options {
                settings,
                mode: inset_host::Mode::Real,
                working_dir,
            };
            let program = inset_host::Program::load(&component).map_err(|e| e.to_string())?;
            let running = program.start(&options).map_err(|e| e.to_string())?;
            inset_host::serve(running).map_err(|e| e.to_string())
        });
    match served {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("inset: {e}");
            ExitCode::FAILURE
        }
    }
}

fn lsp() -> ExitCode {
    match inset_lsp::serve_stdio() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("inset: language server: {e}");
            ExitCode::FAILURE
        }
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
