//! The diagram palette: one colour per role, in a dark and a light mode.
//!
//! Every role colour was chosen by measurement against its mode's background,
//! not by eye. Each pair of role colours stays apart under simulated
//! protanopia and deuteranopia (OKLab ΔE×100 of at least 8) and with normal
//! vision (at least 15), each role colour has at least 3:1 contrast with the
//! background, and each text colour at least 4.5:1 on every box it sits on.
//! Colour only repeats what the shape already says, so a reader who cannot
//! tell two colours apart loses nothing. `docs/language/diagrams.md` lists the
//! roles.
//!
//! The coverage colours were measured the same way, against the four role
//! colours and each other, after searching OKLCH for a green, a yellow and a
//! grey that pass. The closest pairs are in the light palette: uncovered and
//! capability differ by 15.5 with normal vision and 8.6 under protanopia,
//! and uncovered and failure by 9.2 under deuteranopia. The lowest text
//! contrast on a box is light uncovered's, 4.5:1. The shapes carry the same
//! meaning: an uncovered branch is drawn thick, an excused
//! one dotted, and each row coverage counts gets a mark, `✓`, `✗`, `~` or `!`.

/// Which palette a picture uses. Following the reader's setting is the default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Theme {
    Dark,
    Light,
    /// Both palettes in one picture: light where the reader's system asks for
    /// light, and dark everywhere else, including in renderers that ignore
    /// the media query.
    #[default]
    Auto,
}

impl std::str::FromStr for Theme {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "dark" => Ok(Theme::Dark),
            "light" => Ok(Theme::Light),
            "auto" => Ok(Theme::Auto),
            _ => Err(format!("unknown theme `{s}`; expected dark, light or auto")),
        }
    }
}

/// Each token's name, and its value in the dark and the light palette.
const TOKENS: [(&str, &str, &str); 20] = [
    // The background, the app box and component boxes.
    ("ground", "#0c1015", "#fdfdfd"),
    ("app-fill", "#13181d", "#f2f6fa"),
    ("app-stroke", "#6a7683", "#6a7683"),
    ("box-fill", "#161c23", "#ffffff"),
    // Text, and the message arrows drawn in the same ink.
    ("ink", "#e7ecf0", "#182029"),
    ("dim", "#aab2bb", "#565f69"),
    ("outside", "#727b86", "#78818c"),
    // The roles: components, capabilities, external components and failures.
    ("component", "#5797f8", "#196df7"),
    ("keyword", "#5797f8", "#1066f0"),
    ("capability", "#eba566", "#e2760f"),
    ("external", "#e057c5", "#a41faa"),
    ("external-fill", "#271923", "#fef3fe"),
    ("failure", "#f64234", "#9c200b"),
    // Flowchart shapes.
    ("decide-fill", "#36210d", "#fff0e7"),
    ("term-fill", "#1a273a", "#e9f1fe"),
    ("node-fill", "#161c23", "#ffffff"),
    ("bar", "#e7ecf0", "#182029"),
    // Coverage: reached, not reached, and excused by `#[defensive]`.
    ("covered", "#2fd4b4", "#0b7d74"),
    ("uncovered", "#e8e337", "#8f6b09"),
    ("excused", "#7f8a7a", "#4a4858"),
];

/// The rules, written against the tokens and scoped to the diagram, so an
/// SVG placed inline in a page neither takes nor gives styles.
const RULES: &str = r#".inset-diagram text { font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, "DejaVu Sans Mono", monospace; font-size: 12px; fill: var(--ink); white-space: pre; }
.inset-diagram .title { font-weight: bold; }
.inset-diagram .kw { fill: var(--keyword); }
.inset-diagram .dim { fill: var(--dim); }
.inset-diagram .doc { fill: var(--dim); font-style: italic; }
.inset-diagram .sec { fill: var(--dim); font-size: 10px; letter-spacing: 0.05em; }
.inset-diagram .warn { fill: var(--failure); font-weight: bold; }
.inset-diagram .ground { fill: var(--ground); }
.inset-diagram .app { fill: var(--app-fill); stroke: var(--app-stroke); stroke-width: 1.5; }
.inset-diagram .comp { fill: var(--box-fill); stroke: var(--component); stroke-width: 1.4; }
.inset-diagram .ext { fill: var(--external-fill); stroke: var(--external); stroke-width: 1.3; }
.inset-diagram .outside { fill: var(--box-fill); stroke: var(--outside); stroke-dasharray: 3 2; }
.inset-diagram .port { fill: var(--box-fill); stroke: var(--component); stroke-width: 1.6; }
.inset-diagram .edge { fill: none; stroke: var(--ink); stroke-width: 1.2; }
.inset-diagram .send { stroke-dasharray: 5 3; }
.inset-diagram .cap { fill: none; stroke: var(--capability); stroke-width: 1.8; }
.inset-diagram .node { fill: var(--node-fill); stroke: var(--ink); }
.inset-diagram .decide { fill: var(--decide-fill); stroke: var(--ink); }
.inset-diagram .term { fill: var(--term-fill); stroke: var(--ink); }
.inset-diagram .bar { fill: var(--bar); }
.inset-diagram .arrow { fill: var(--ink); }
.inset-diagram .arrow-cap { fill: var(--capability); }
"#;

/// The rules for coverage, which only a picture that shows it carries.
const COVERAGE_RULES: &str = r#".inset-diagram .cov-covered { stroke: var(--covered); }
.inset-diagram .cov-uncovered { stroke: var(--uncovered); stroke-width: 3; }
.inset-diagram .cov-excused { stroke: var(--excused); stroke-dasharray: 2 3; }
.inset-diagram .cov-contradicted { stroke: var(--uncovered); stroke-width: 3; stroke-dasharray: 2 3; }
.inset-diagram text.cov-covered { fill: var(--covered); stroke: none; }
.inset-diagram text.cov-uncovered { fill: var(--uncovered); stroke: none; font-weight: bold; }
.inset-diagram text.cov-excused { fill: var(--excused); stroke: none; }
.inset-diagram text.cov-contradicted { fill: var(--uncovered); stroke: none; font-weight: bold; }
.inset-diagram .band { fill: var(--uncovered); fill-opacity: 0.18; stroke: none; }
"#;

/// The `<style>` element for a theme, with the coverage rules if the
/// picture shows coverage. The rules name tokens as `var(--name)`, and each
/// is replaced by its value rather than left as a CSS custom property,
/// because some SVG renderers, such as image converters, do not support
/// custom properties.
pub(crate) fn style(theme: Theme, coverage: bool) -> String {
    let rules = |palette| rules(palette, coverage);
    match theme {
        Theme::Dark => format!("<style>\n{}</style>\n", rules(Theme::Dark)),
        Theme::Light => format!("<style>\n{}</style>\n", rules(Theme::Light)),
        Theme::Auto => format!(
            "<style>\n{}@media (prefers-color-scheme: light) {{\n{}}}\n</style>\n",
            rules(Theme::Dark),
            rules(Theme::Light)
        ),
    }
}

/// The rules with one palette's values in place of the tokens.
fn rules(palette: Theme, coverage: bool) -> String {
    let mut rules = RULES.to_string();
    if coverage {
        rules.push_str(COVERAGE_RULES);
    }
    for (name, dark, light) in TOKENS {
        let value = if palette == Theme::Light { light } else { dark };
        rules = rules.replace(&format!("var(--{name})"), value);
    }
    debug_assert!(!rules.contains("var(--"), "every token is defined");
    rules
}
