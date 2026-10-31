//! Diagram data emitted from source: every example the compiler covers is
//! drawn as its hand-written expected diagram, the output is pinned for a
//! program with every shape drawn so far, and what cannot be drawn yet is
//! reported at the construct.

use std::path::{Path, PathBuf};

use inset_check::{Program, SourceFile, check};
use inset_diagram::{Diagram, View, emit, emit_with_sources, render, to_json, validate};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn drawn(program: &Program) -> Diagram {
    emit(program).unwrap_or_else(|reports| {
        let rendered: String = reports.iter().map(|r| program.render(r, false)).collect();
        panic!("not drawn:\n{rendered}")
    })
}

#[test]
fn every_checked_example_is_drawn_as_its_expected_diagram() {
    let mut examples: Vec<_> = std::fs::read_dir(repo().join("examples"))
        .expect("read examples")
        .map(|e| e.expect("read examples").path())
        .collect();
    examples.sort();
    let mut drawn_examples = Vec::new();
    for dir in examples {
        let program = Program::load(&dir).expect("load example");
        if !check(&program).is_empty() {
            continue;
        }
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        // Compared as JSON values, because the expected files are laid
        // out by hand.
        let emitted: serde_json::Value = serde_json::from_str(&to_json(&drawn(&program))).unwrap();
        let expected = std::fs::read_to_string(dir.join("expected/diagram.json")).unwrap();
        let expected: serde_json::Value = serde_json::from_str(&expected).unwrap();
        assert_eq!(emitted, expected, "{name}");
        drawn_examples.push(name);
    }
    // The examples the compiler covers so far. Another example joins this
    // list when it checks clean.
    assert_eq!(drawn_examples, ["hello-http"]);
}

/// Every view of a diagram draws, which also checks each flowchart's ID.
fn draw_every_view(diagram: &Diagram) {
    every_view(diagram);
}

/// Every view of a diagram, drawn: components collapsed and expanded, and
/// each function's flowchart.
fn every_view(diagram: &Diagram) -> Vec<String> {
    let mut functions = Vec::new();
    let mut components = vec![&diagram.app.components];
    while let Some(list) = components.pop() {
        for c in list {
            functions.extend(c.routes.iter().map(|r| &r.handler));
            functions.extend(&c.messages);
            functions.extend(&c.algorithms);
            components.push(&c.components);
        }
    }
    functions.extend(&diagram.algorithms);
    let expanded = View {
        expand_all: true,
        ..View::default()
    };
    let mut drawn = vec![
        render(diagram, &View::default()).unwrap(),
        render(diagram, &expanded).unwrap(),
    ];
    for function in functions {
        let view = View {
            flowchart: Some(function.id.clone()),
            ..View::default()
        };
        drawn.push(render(diagram, &view).unwrap_or_else(|e| panic!("{}: {e}", function.id)));
    }
    drawn
}

/// The IDs a picture gives its elements, which an editor reads to know
/// what was clicked.
fn drawn_ids(svg: &str) -> Vec<String> {
    svg.split(r#"data-id=""#)
        .skip(1)
        .map(|rest| {
            let id = &rest[..rest.find('"').expect("a closing quote")];
            id.replace("&gt;", ">")
                .replace("&lt;", "<")
                .replace("&quot;", "\"")
                .replace("&amp;", "&")
        })
        .collect()
}

/// Every element any view draws has a place in the source, and the places
/// are pinned for the program with every shape, one line each: the ID, the
/// file, line and column, and the start of the source there.
#[test]
fn every_drawn_element_has_a_source() {
    let mut pinned = String::new();
    for dir in [
        "examples/hello-http",
        "crates/inset-diagram/tests/programs/shapes",
        "crates/inset-host/tests/programs/stock-room",
    ] {
        let program = Program::load(&repo().join(dir)).expect("load program");
        let emitted = emit_with_sources(&program).expect("drawn");
        for svg in every_view(&emitted.diagram) {
            for id in drawn_ids(&svg) {
                assert!(
                    emitted.sources.contains_key(&id),
                    "{dir}: `{id}` has no source"
                );
            }
        }
        if dir.ends_with("shapes") {
            for (id, source) in &emitted.sources {
                let text = &program.files[source.file].text;
                let span = source.span;
                let before = &text[..span.start];
                let line = before.matches('\n').count() + 1;
                let column = before.len() - before.rfind('\n').map_or(0, |i| i + 1) + 1;
                let start = text[span.start..span.end]
                    .lines()
                    .next()
                    .unwrap_or_default();
                let path = &program.files[source.file].path;
                pinned.push_str(&format!("{id}  {path}:{line}:{column}  {start}\n"));
            }
        }
    }
    insta::assert_snapshot!(pinned);
}

#[test]
fn shapes_are_drawn() {
    let program = Program::load(&repo().join("crates/inset-diagram/tests/programs/shapes"))
        .expect("load shapes");
    let diagram = drawn(&program);
    assert_eq!(validate(&diagram), Vec::<String>::new());
    draw_every_view(&diagram);
    let json = to_json(&diagram);
    assert_eq!(to_json(&drawn(&program)), json, "emitting is deterministic");
    insta::assert_snapshot!(json);
}

/// The flowcharts of a `match`, and of `for` and `while` loops, are pinned
/// as SVGs beside the program, where a reviewer can open them. Run with
/// `INSET_BLESS=1` to rewrite them after a deliberate change.
#[test]
fn match_and_loop_flowcharts_are_pinned() {
    let dir = repo().join("crates/inset-diagram/tests/programs/shapes");
    let program = Program::load(&dir).expect("load shapes");
    let diagram = drawn(&program);
    for (file, function) in [
        ("classify.svg", "Shapes/mod:sizes/fn:classify"),
        ("total.svg", "Shapes/mod:sizes/fn:total"),
        ("measure.svg", "Shapes/front/fn:measure"),
    ] {
        let view = View {
            flowchart: Some(function.into()),
            ..View::default()
        };
        let svg = render(&diagram, &view).unwrap();
        let path = dir.join("expected").join(file);
        if std::env::var_os("INSET_BLESS").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &svg).unwrap();
            continue;
        }
        let committed = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            committed == svg,
            "{} is out of date; run the tests with INSET_BLESS=1 and review the difference",
            path.display()
        );
    }
}

/// The element with this ID, anywhere in the diagram's data.
fn find<'v>(value: &'v serde_json::Value, id: &str) -> Option<&'v serde_json::Value> {
    if value["id"] == id {
        return Some(value);
    }
    match value {
        serde_json::Value::Object(map) => map.values().find_map(|v| find(v, id)),
        serde_json::Value::Array(items) => items.iter().find_map(|v| find(v, id)),
        _ => None,
    }
}

/// Each awaited table call is a step linked to the table's capability
/// edge, which lists every method the component calls.
#[test]
fn table_calls_are_steps_on_the_capability_edge() {
    let program = Program::load(&repo().join("crates/inset-host/tests/programs/stock-room"))
        .expect("load stock room");
    let diagram = drawn(&program);
    assert_eq!(validate(&diagram), Vec::<String>::new());
    draw_every_view(&diagram);
    let json: serde_json::Value = serde_json::from_str(&to_json(&diagram)).unwrap();
    let edge = find(&json, "StockRoom/Room/field:items->StockRoom/grant:items").expect("an edge");
    assert_eq!(
        edge["operations"],
        serde_json::json!(["entries", "get", "insert", "remove"])
    );
    let receive = find(&json, "StockRoom/Room/fn:receive").expect("a handler");
    insta::assert_snapshot!(serde_json::to_string_pretty(&receive["body"]).unwrap());
}

/// Messages, an outlet, `send`, `.await`, and the shapes of body that
/// would hide a branch inside a step.
const UNDRAWN: &str = r#"app Counting {
    setting port: u16 = 8080;
    grant http: HttpServer { port };
    start Front { http: move http };
}

component Front {
    http: HttpServer,

    start tally: Tally { total: 0 };

    #[get("/count/{n}", via = http)]
    async fn count(&self, n: u32) -> String {
        send self.tally.add(n);
        let total = self.tally.total().await;
        format!("{total}")
    }

    #[get("/shapes/{n}", via = http)]
    fn shapes(n: u32) -> String {
        let small = double(if n > 10 { 10 } else { n });
        {
            let unused = small;
        }
        format!("{small}")
    }
}

component Tally {
    total: u32,

    #[message]
    fn add(&mut self, n: u32) {
        self.total += n;
    }

    #[message]
    fn total(&self) -> u32 {
        self.total
    }
}

fn double(n: u32) -> u32 {
    n * 2
}

fn size(text: &str) -> u32 {
    double(match text.len() {
        0 => 0,
        _ => 1,
    })
}
"#;

#[test]
fn what_cannot_be_drawn_yet_is_reported_at_the_construct() {
    let program = Program::from_files(vec![SourceFile {
        path: "src/main.inset".into(),
        text: UNDRAWN.into(),
    }]);
    assert!(check(&program).is_empty(), "the program checks clean");
    let reports = emit(&program).expect_err("parts cannot be drawn");
    let rendered: String = reports.iter().map(|r| program.render(r, false)).collect();
    insta::assert_snapshot!(rendered);
}
