//! Checks the hand-written diagram data of every example, and pins the
//! committed schema and SVGs. Run with `INSET_BLESS=1` to rewrite the pinned
//! files after a deliberate change.

use std::path::{Path, PathBuf};

use inset_diagram::{Hide, Theme, View};

const EXAMPLES: [&str; 3] = ["hello-http", "stock-tracker", "csv-import"];

/// Every committed SVG: the example, the file name, and the view it shows.
/// They use the default theme, so each follows its reader's light or dark setting.
fn views() -> Vec<(&'static str, &'static str, View)> {
    let auto = View::default();
    assert_eq!(auto.theme, Theme::Auto);
    let expand = |id: &str| View {
        expand: [id.to_string()].into(),
        ..auto.clone()
    };
    let flowchart = |id: &str| View {
        flowchart: Some(id.to_string()),
        ..auto.clone()
    };
    vec![
        ("hello-http", "diagram.svg", auto.clone()),
        (
            "hello-http",
            "hello.svg",
            flowchart("HelloHttp/Greeter/fn:hello"),
        ),
        ("stock-tracker", "diagram.svg", auto.clone()),
        (
            "stock-tracker",
            "warehouse.svg",
            expand("StockTracker/Warehouse"),
        ),
        (
            "stock-tracker",
            "ship.svg",
            flowchart("StockTracker/Warehouse/inventory/fn:ship"),
        ),
        (
            "stock-tracker",
            "reorder_quantity.svg",
            flowchart("StockTracker/Warehouse/inventory/fn:reorder_quantity"),
        ),
        ("csv-import", "diagram.svg", auto.clone()),
        ("csv-import", "importer.svg", expand("PriceImport/Importer")),
        (
            "csv-import",
            "import.svg",
            flowchart("PriceImport/Importer/fn:import"),
        ),
        (
            "csv-import",
            "parse_price_list.svg",
            flowchart("PriceImport/fn:parse_price_list"),
        ),
    ]
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn expected(example: &str, file: &str) -> PathBuf {
    repo()
        .join("examples")
        .join(example)
        .join("expected")
        .join(file)
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

fn diagram(example: &str) -> inset_diagram::Diagram {
    inset_diagram::from_json(&read(&expected(example, "diagram.json")))
        .unwrap_or_else(|e| panic!("{example}: {e}"))
}

/// Compares fresh output with a committed file, or rewrites the file when blessing.
fn pinned(path: &Path, actual: &str) {
    if std::env::var_os("INSET_BLESS").is_some() {
        std::fs::write(path, actual).unwrap();
        return;
    }
    let committed = std::fs::read_to_string(path).unwrap_or_default();
    assert!(
        committed == actual,
        "{} is out of date; run the tests with INSET_BLESS=1 and review the difference",
        path.display()
    );
}

#[test]
fn schema_is_up_to_date() {
    pinned(
        &repo().join("docs/language/diagram.schema.json"),
        &inset_diagram::schema(),
    );
}

#[test]
fn examples_validate_against_the_schema() {
    let schema: serde_json::Value = serde_json::from_str(&inset_diagram::schema()).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    for example in EXAMPLES {
        let json: serde_json::Value =
            serde_json::from_str(&read(&expected(example, "diagram.json"))).unwrap();
        let errors: Vec<String> = validator
            .iter_errors(&json)
            .map(|e| format!("{} at {}", e, e.instance_path()))
            .collect();
        assert!(errors.is_empty(), "{example}:\n{}", errors.join("\n"));
    }
}

#[test]
fn examples_are_consistent() {
    for example in EXAMPLES {
        let errors = inset_diagram::validate(&diagram(example));
        assert!(errors.is_empty(), "{example}:\n{}", errors.join("\n"));
    }
}

#[test]
fn examples_round_trip() {
    for example in EXAMPLES {
        let json = read(&expected(example, "diagram.json"));
        let original: serde_json::Value = serde_json::from_str(&json).unwrap();
        let parsed = diagram(example);
        let written = inset_diagram::to_json(&parsed);
        let reparsed: serde_json::Value = serde_json::from_str(&written).unwrap();
        assert_eq!(original, reparsed, "{example} changed in a round trip");
        assert_eq!(inset_diagram::from_json(&written).unwrap(), parsed);
    }
}

#[test]
fn committed_svgs_are_up_to_date() {
    for (example, file, view) in views() {
        let svg = inset_diagram::render(&diagram(example), &view).unwrap();
        assert_eq!(
            svg,
            inset_diagram::render(&diagram(example), &view).unwrap()
        );
        pinned(&expected(example, file), &svg);
    }
}

/// Hiding a label leaves it off and changes nothing else: the picture is the
/// full picture without the lines that carry that label's class.
#[test]
fn hiding_a_label_changes_nothing_else() {
    for (example, file, view) in views() {
        let full = inset_diagram::render(&diagram(example), &view).unwrap();
        for hide in Hide::ALL {
            let mut hidden_view = view.clone();
            hidden_view.hide.insert(hide);
            let hidden = inset_diagram::render(&diagram(example), &hidden_view).unwrap();
            let class = format!(" opt-{hide}");
            let carries = |line: &str| {
                line.match_indices(&class)
                    .any(|(i, _)| matches!(line[i + class.len()..].chars().next(), Some('"' | ' ')))
                    || line.contains(&format!("\"opt-{hide}\""))
            };
            let expected: String = full
                .lines()
                .filter(|line| !carries(line))
                .map(|line| format!("{line}\n"))
                .collect();
            assert_eq!(hidden, expected, "{example}/{file} with --hide {hide}");
        }
    }
}

#[test]
fn every_label_option_hides_something_somewhere() {
    for hide in Hide::ALL {
        let used = views().into_iter().any(|(example, _, view)| {
            let svg = inset_diagram::render(&diagram(example), &view).unwrap();
            svg.contains(&format!("opt-{hide}\"")) || svg.contains(&format!("opt-{hide} "))
        });
        assert!(
            used,
            "no committed view shows a label that --hide {hide} hides"
        );
    }
}

#[test]
fn validation_reports_bad_ids_and_references() {
    let json = read(&expected("hello-http", "diagram.json"))
        .replace(
            "\"id\": \"HelloHttp/Greeter/fn:hello\"",
            "\"id\": \"HelloHttp/Greeter/fn:greet\"",
        )
        .replace(
            "\"via\": \"HelloHttp/Greeter/field:http\"",
            "\"via\": \"HelloHttp/setting:port\"",
        );
    let errors = inset_diagram::from_json(&json).unwrap_err();
    assert!(
        errors.contains("`HelloHttp/Greeter/fn:greet` should be `HelloHttp/Greeter/fn:hello`"),
        "{errors}"
    );
    assert!(
        errors.contains("`HelloHttp/setting:port` is not a capability field"),
        "{errors}"
    );
}

/// The theme changes only the palette, and leaving the background off removes
/// only the background.
#[test]
fn themes_and_background_change_nothing_else() {
    let palette = |svg: &str| -> String {
        svg.lines()
            .filter(|line| {
                !line.starts_with(".inset-diagram ") && !line.starts_with("@media") && *line != "}"
            })
            .map(|line| format!("{line}\n"))
            .collect()
    };
    for (example, file, view) in views() {
        let render = |theme| {
            let view = View {
                theme,
                ..view.clone()
            };
            inset_diagram::render(&diagram(example), &view).unwrap()
        };
        let (auto, dark, light) = (
            render(Theme::Auto),
            render(Theme::Dark),
            render(Theme::Light),
        );
        assert_ne!(dark, light, "{example}/{file}");
        assert_eq!(palette(&dark), palette(&light), "{example}/{file}");
        assert_eq!(palette(&auto), palette(&dark), "{example}/{file}");
        let style = |svg: &str| {
            svg[svg.find("<style>").unwrap()..svg.find("</style>").unwrap()].to_string()
        };
        assert!(
            style(&auto).starts_with(&style(&dark)),
            "{example}/{file}: auto starts dark"
        );
        assert!(
            style(&auto).contains(&style(&light)["<style>\n".len()..]),
            "{example}/{file}: auto holds light"
        );

        let bare_view = View {
            transparent: true,
            ..view.clone()
        };
        let bare = inset_diagram::render(&diagram(example), &bare_view).unwrap();
        let full = inset_diagram::render(&diagram(example), &view).unwrap();
        let without_ground: String = full
            .lines()
            .filter(|line| !line.contains(r#"class="ground""#))
            .map(|line| format!("{line}\n"))
            .collect();
        assert_eq!(bare, without_ground, "{example}/{file}");
    }
}
