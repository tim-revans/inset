//! The emitted code marks exactly the elements the diagram gives coverage
//! to: every function, branch, `?` error path and `failure` arm, by the
//! same ID. The two are worked out separately, by the code generator and by
//! the diagram's flowcharts, so this test is what keeps them in step.

use std::collections::BTreeSet;
use std::path::Path;

use inset_check::Program;
use inset_diagram::model::{Component, Diagram, Function, Step};

/// Every element of the diagram that coverage counts.
fn counted(diagram: &Diagram) -> BTreeSet<String> {
    fn steps(list: &[Step], out: &mut BTreeSet<String>) {
        for step in list {
            match step {
                Step::Action { exit, .. } => out.extend(exit.iter().map(|e| e.id.clone())),
                Step::Decision { branches, .. } | Step::Fork { branches, .. } => {
                    for branch in branches {
                        out.insert(branch.id.clone());
                        steps(&branch.steps, out);
                    }
                }
                Step::Loop { body, skip, .. } => {
                    for branch in [body, skip] {
                        out.insert(branch.id.clone());
                        steps(&branch.steps, out);
                    }
                }
                Step::Return { .. } => {}
            }
        }
    }
    fn function(f: &Function, out: &mut BTreeSet<String>) {
        if let Some(body) = &f.body {
            out.insert(f.id.clone());
            steps(body, out);
        }
    }
    fn component(c: &Component, out: &mut BTreeSet<String>) {
        out.extend(c.failure.iter().map(|arm| arm.id.clone()));
        let handlers = c.routes.iter().map(|r| &r.handler);
        for f in handlers.chain(&c.messages).chain(&c.algorithms) {
            function(f, out);
        }
        for child in &c.components {
            component(child, out);
        }
    }
    let mut out = BTreeSet::new();
    for c in &diagram.app.components {
        component(c, &mut out);
    }
    for f in &diagram.algorithms {
        function(f, &mut out);
    }
    out
}

/// Every element the emitted code marks, with each component's under the
/// ID of each running component of its type.
fn marked(diagram: &Diagram, elements: &inset_codegen::Elements) -> BTreeSet<String> {
    fn walk(c: &Component, elements: &inset_codegen::Elements, out: &mut BTreeSet<String>) {
        let listed = elements
            .components
            .iter()
            .find(|(path, _)| *path == c.definition)
            .unwrap_or_else(|| panic!("no elements for {}", c.definition));
        out.extend(listed.1.iter().map(|id| format!("{}/{id}", c.id)));
        for child in &c.components {
            walk(child, elements, out);
        }
    }
    let mut out: BTreeSet<String> = elements.algorithms.iter().cloned().collect();
    for c in &diagram.app.components {
        walk(c, elements, &mut out);
    }
    out
}

#[test]
fn the_emitted_code_marks_what_the_diagram_counts() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut dirs = Vec::new();
    for parent in [
        "examples",
        "crates/inset-host/tests/programs",
        "crates/inset-cli/tests/programs",
        "crates/inset-diagram/tests/programs",
    ] {
        for entry in std::fs::read_dir(repo.join(parent)).expect("read programs") {
            dirs.push(entry.expect("read programs").path());
        }
    }
    dirs.sort();
    let mut compared = 0;
    for dir in dirs {
        let program = Program::load(&dir).expect("load program");
        let (Ok(generated), Ok(diagram)) = (
            inset_codegen::generate(&program),
            inset_diagram::emit(&program),
        ) else {
            continue;
        };
        let counted = counted(&diagram);
        let marked = marked(&diagram, &generated.elements);
        assert_eq!(
            marked.difference(&counted).collect::<Vec<_>>(),
            Vec::<&String>::new(),
            "{}: marked, but not in the diagram",
            dir.display()
        );
        assert_eq!(
            counted.difference(&marked).collect::<Vec<_>>(),
            Vec::<&String>::new(),
            "{}: in the diagram, but not marked",
            dir.display()
        );
        compared += 1;
    }
    assert!(compared >= 8, "only {compared} programs compared");
}
