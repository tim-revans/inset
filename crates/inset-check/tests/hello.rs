//! Hello HTTP, the example the checker covers, checks clean.

use std::path::Path;

use inset_check::{Program, check};

#[test]
fn hello_http_checks_clean() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/hello-http");
    let program = Program::load(&dir).expect("load Hello HTTP");
    let rendered: Vec<String> = check(&program)
        .iter()
        .map(|r| program.render(r, false))
        .collect();
    assert!(rendered.is_empty(), "{}", rendered.concat());
}

/// The checker runs on whatever parsed, so it must not panic on broken
/// programs. Each example is checked with each of its lines removed in turn.
#[test]
fn never_panics_on_broken_examples() {
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    for example in ["hello-http", "stock-tracker", "csv-import"] {
        let program = Program::load(&examples.join(example)).expect("load example");
        check(&program);
        for (index, file) in program.files.iter().enumerate() {
            let lines: Vec<&str> = file.text.lines().collect();
            for removed in 0..lines.len() {
                let mut broken = program.clone();
                broken.files[index].text = lines
                    .iter()
                    .enumerate()
                    .filter(|&(i, _)| i != removed)
                    .map(|(_, line)| format!("{line}\n"))
                    .collect();
                check(&broken);
            }
        }
    }
}
