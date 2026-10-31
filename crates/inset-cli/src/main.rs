//! The `inset` command line.
//!
//! `inset check <dir>` checks the program in a directory: its code in
//! `src/` and its requirements in `requirements/`. It prints nothing when
//! the program is sound.
//!
//! `inset build <dir>` checks the program and compiles it to a WebAssembly
//! component. The generated Cargo project is written to `<dir>/target/inset/`
//! and the component's path is printed. Beside the component it writes the
//! program's API contract, `<package>.openapi.json` and `<package>.d.ts`, so
//! a front end can import the TypeScript types.
//!
//! `inset contract <dir> [--openapi | --typescript] [-o file]` checks the
//! program and prints its API contract, as OpenAPI 3.1 (the default) or as
//! TypeScript types, or writes it to `file`.
//!
//! `inset run <dir> [--set name=value]...` builds the program and runs it in
//! `inset-host` with real capabilities, applying each `--set` to a setting,
//! until it stops. Relative paths in grants resolve against the directory
//! `inset run` is run from.
//!
//! `inset test <dir> [--json]` builds the program and runs each of its
//! requirements against the component, in the host with a fake behind every
//! grant, as `requirements.rs` describes. It prints each requirement's
//! sentence with whether it passed, and the step that failed and why, or
//! with `--json` the same report as JSON for tools. It fails if any
//! requirement fails.
//!
//! `inset diagram <dir> [--json] [view options]` checks the program and
//! draws its diagram as SVG, with the view options of
//! `docs/language/diagrams.md`, or with `--json` prints the diagram data.
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
use inset_diagram::{VIEW_OPTIONS, View};
use inset_syntax::{Diagnostic, is_requirements_file, parse_code, parse_requirements};

mod requirements;

const USAGE: &str = "usage: inset check <dir>
       inset build <dir>
       inset contract <dir> [--openapi | --typescript] [-o file]
       inset run <dir> [--set name=value]...
       inset test <dir> [--json]
       inset diagram <dir> [--json] [view options]
       inset parse <file>
       inset lsp";

fn usage() -> String {
    format!("{USAGE}\n\nview options:\n{VIEW_OPTIONS}")
}

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
        ["contract", dir, options @ ..] => contract(dir, options),
        ["run", dir, sets @ ..] => run(dir, sets),
        ["test", dir] => test(dir, false),
        ["test", dir, "--json"] => test(dir, true),
        ["diagram", dir, options @ ..] => diagram(dir, options),
        ["parse", path] => parse(path),
        ["lsp"] => lsp(),
        _ => {
            eprintln!("{}", usage());
            ExitCode::FAILURE
        }
    }
}

fn load(dir: &str) -> Result<Program, ExitCode> {
    Program::load(Path::new(dir)).map_err(|e| {
        eprintln!("inset: {e}");
        ExitCode::FAILURE
    })
}

fn print_reports(program: &Program, reports: &[inset_check::Report]) {
    let color = std::io::stderr().is_terminal();
    for report in reports {
        eprint!("{}", program.render(report, color));
    }
}

fn check(dir: &str) -> ExitCode {
    let program = match load(dir) {
        Ok(program) => program,
        Err(code) => return code,
    };
    let reports = inset_check::check(&program);
    print_reports(&program, &reports);
    if reports.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Builds the program in `dir`, printing any diagnostics, and returns its component.
fn build(dir: &str) -> Result<PathBuf, ExitCode> {
    build_program(dir, &load(dir)?)
}

fn build_program(dir: &str, program: &Program) -> Result<PathBuf, ExitCode> {
    let out = Path::new(dir).join("target").join("inset");
    match inset_codegen::build(program, &out, None) {
        Ok(built) => {
            write_contract(program, &built.component)?;
            Ok(built.component)
        }
        Err(BuildError::Rejected(reports)) => {
            print_reports(program, &reports);
            Err(ExitCode::FAILURE)
        }
        Err(e) => {
            eprintln!("inset: {e}");
            Err(ExitCode::FAILURE)
        }
    }
}

/// Writes the API contract beside the component, named as it is.
fn write_contract(program: &Program, component: &Path) -> Result<(), ExitCode> {
    let fail = |e: String| {
        eprintln!("inset: {e}");
        ExitCode::FAILURE
    };
    let api = inset_check::api(program).map_err(|reports| {
        print_reports(program, &reports);
        ExitCode::FAILURE
    })?;
    for (extension, text) in [
        ("openapi.json", inset_contract::openapi(&api)),
        ("d.ts", inset_contract::typescript(&api)),
    ] {
        let text = text.map_err(fail)?;
        let path = component.with_extension(extension);
        if std::fs::read_to_string(&path).is_ok_and(|old| old == text) {
            continue;
        }
        std::fs::write(&path, text)
            .map_err(|e| fail(format!("cannot write {}: {e}", path.display())))?;
    }
    Ok(())
}

fn contract(dir: &str, options: &[&str]) -> ExitCode {
    let mut typescript = false;
    let mut out = None;
    let mut options = options.iter();
    while let Some(option) = options.next() {
        match (*option, options.clone().next()) {
            ("--openapi", _) => typescript = false,
            ("--typescript", _) => typescript = true,
            ("-o", Some(file)) => {
                out = Some(*file);
                options.next();
            }
            _ => {
                eprintln!("{}", usage());
                return ExitCode::FAILURE;
            }
        }
    }
    let program = match load(dir) {
        Ok(program) => program,
        Err(code) => return code,
    };
    let api = match inset_check::api(&program) {
        Ok(api) => api,
        Err(reports) => {
            print_reports(&program, &reports);
            return ExitCode::FAILURE;
        }
    };
    let text = if typescript {
        inset_contract::typescript(&api)
    } else {
        inset_contract::openapi(&api)
    };
    let written = text.and_then(|text| match out {
        Some(path) => {
            if let Some(dir) = Path::new(path)
                .parent()
                .filter(|d| !d.as_os_str().is_empty())
            {
                std::fs::create_dir_all(dir).map_err(|e| format!("cannot write {path}: {e}"))?;
            }
            std::fs::write(path, text).map_err(|e| format!("cannot write {path}: {e}"))
        }
        None => {
            print!("{text}");
            Ok(())
        }
    });
    match written {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("inset: {e}");
            ExitCode::FAILURE
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
                eprintln!("{}", usage());
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

fn test(dir: &str, json: bool) -> ExitCode {
    let tested = load(dir).and_then(|program| {
        let component = build_program(dir, &program)?;
        requirements::run(&program, &component).map_err(|e| {
            eprintln!("inset: {e}");
            ExitCode::FAILURE
        })
    });
    let report = match tested {
        Ok(report) => report,
        Err(code) => return code,
    };
    if json {
        let mut out = serde_json::to_string_pretty(&report).expect("a report always serialises");
        out.push('\n');
        print!("{out}");
    } else {
        print!("{}", requirements::render(&report));
    }
    if report.failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn diagram(dir: &str, options: &[&str]) -> ExitCode {
    let json = options.contains(&"--json");
    let options: Vec<String> = options
        .iter()
        .filter(|o| **o != "--json")
        .map(|o| o.to_string())
        .collect();
    let (view, out) = match View::from_args(&options) {
        Ok(view) => view,
        Err(e) => {
            eprintln!("inset: {e}\n{}", usage());
            return ExitCode::FAILURE;
        }
    };
    let program = match load(dir) {
        Ok(program) => program,
        Err(code) => return code,
    };
    let diagram = match inset_diagram::emit(&program) {
        Ok(diagram) => diagram,
        Err(reports) => {
            print_reports(&program, &reports);
            return ExitCode::FAILURE;
        }
    };
    let drawn = match json {
        true => Ok(inset_diagram::to_json(&diagram)),
        false => inset_diagram::render(&diagram, &view),
    };
    let written = drawn.and_then(|text| match &out {
        Some(path) => std::fs::write(path, text).map_err(|e| format!("cannot write {path}: {e}")),
        None => {
            print!("{text}");
            Ok(())
        }
    });
    match written {
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
