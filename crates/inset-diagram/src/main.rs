//! `inset-diagram`: checks and draws diagram data until the `inset` CLI exists.

use std::process::ExitCode;

use inset_diagram::{Hide, View};

const USAGE: &str = "usage:
  inset-diagram render <diagram.json> [options]   draw a view as SVG
  inset-diagram check <diagram.json>              check the data is valid
  inset-diagram schema                            print the JSON Schema

render options:
  --expand <component id>   draw the component with its children inside (repeatable)
  --expand-all              draw every component with its children inside
  --flowchart <function id> draw the function's flowchart instead
  --hide <label>            leave a kind of label off (repeatable)
  --theme <dark|light>      the palette (default dark)
  --transparent             leave the background off
  -o <file>                 write the SVG to a file instead of standard output";

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
            let path = args.get(1).ok_or(USAGE)?;
            read(path)?;
            Ok(())
        }
        Some("render") => {
            let path = args.get(1).ok_or(USAGE)?;
            let mut view = View::default();
            let mut out = None;
            let mut rest = args[2..].iter();
            while let Some(arg) = rest.next() {
                let mut value = || rest.next().cloned().ok_or(format!("{arg} needs a value"));
                match arg.as_str() {
                    "--expand" => {
                        view.expand.insert(value()?);
                    }
                    "--expand-all" => view.expand_all = true,
                    "--flowchart" => view.flowchart = Some(value()?),
                    "--hide" => {
                        view.hide.insert(value()?.parse::<Hide>()?);
                    }
                    "--theme" => view.theme = value()?.parse()?,
                    "--transparent" => view.transparent = true,
                    "-o" => out = Some(value()?),
                    _ => return Err(format!("unknown option `{arg}`\n{USAGE}")),
                }
            }
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
        _ => Err(USAGE.into()),
    }
}
