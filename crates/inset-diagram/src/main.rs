//! `inset-diagram`: checks and draws diagram data written by hand, such as
//! the expected diagrams of the examples. `inset diagram` draws a program's.

use std::process::ExitCode;

use inset_diagram::{VIEW_OPTIONS, View};

fn usage() -> String {
    format!(
        "usage:
  inset-diagram render <diagram.json> [options]   draw a view as SVG
  inset-diagram check <diagram.json>              check the data is valid
  inset-diagram schema                            print the JSON Schema

render options:
{VIEW_OPTIONS}"
    )
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("inset-diagram: {message}");
            ExitCode::FAILURE
        }
    }
}

fn read(path: &str) -> Result<inset_diagram::Diagram, String> {
    let json = std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    inset_diagram::from_json(&json).map_err(|e| format!("{path}: {e}"))
}

fn run(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("schema") => {
            print!("{}", inset_diagram::schema());
            Ok(())
        }
        Some("check") => {
            let path = args.get(1).ok_or_else(usage)?;
            read(path)?;
            Ok(())
        }
        Some("render") => {
            let path = args.get(1).ok_or_else(usage)?;
            let (view, out) =
                View::from_args(&args[2..]).map_err(|e| format!("{e}\n{}", usage()))?;
            let diagram = read(path)?;
            let svg = inset_diagram::render(&diagram, &view)?;
            match out {
                Some(out) => {
                    std::fs::write(&out, svg).map_err(|e| format!("cannot write {out}: {e}"))
                }
                None => {
                    print!("{svg}");
                    Ok(())
                }
            }
        }
        _ => Err(usage()),
    }
}
