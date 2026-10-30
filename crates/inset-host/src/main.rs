//! `inset-host run <program.wasm> [--set name=value]...`
//!
//! Runs a built Inset program with real capabilities until it stops. The
//! `inset` command line will run programs through this crate; until it
//! exists, this is how a built program is run by hand.

use std::path::PathBuf;
use std::process::ExitCode;

use inset_host::{Mode, Options, Program};

const USAGE: &str = "usage: inset-host run <program.wasm> [--set name=value]...";

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("inset-host: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    let mut args = args.into_iter();
    if args.next().as_deref() != Some("run") {
        return Err(USAGE.into());
    }
    let program = PathBuf::from(args.next().ok_or(USAGE)?);
    let mut settings = Vec::new();
    while let Some(arg) = args.next() {
        let setting = match arg.as_str() {
            "--set" => args.next().ok_or(USAGE)?,
            _ => return Err(USAGE.into()),
        };
        let (name, value) = setting.split_once('=').ok_or(USAGE)?;
        settings.push((name.to_string(), value.to_string()));
    }
    let options = Options {
        settings,
        mode: Mode::Real,
        working_dir: std::env::current_dir().map_err(|e| e.to_string())?,
    };
    let program = Program::load(&program).map_err(|e| e.to_string())?;
    let running = program.start(&options).map_err(|e| e.to_string())?;
    inset_host::serve(running).map_err(|e| e.to_string())
}
