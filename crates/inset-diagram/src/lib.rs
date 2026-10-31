//! Inset's diagram data: what the compiler emits about a program's structure,
//! its JSON Schema, and a renderer that draws it as SVG.
//!
//! [`emit`] produces the data from a checked program, as `inset diagram`
//! does. `emit/mod.rs` says what it covers so far. [`emit_with_sources`]
//! also says where in the source each element is, so an editor can take a
//! reader from a picture to the code: each element a picture draws carries
//! its ID in a `data-id` attribute.
//!
//! The data holds everything the source says, and a [`View`] chooses what a
//! picture shows. `docs/language/diagrams.md` describes the format, the ID
//! rules and the view options. The schema committed at
//! `docs/language/diagram.schema.json` is generated from [`model`], so the doc
//! comments there are the format's field reference.
//!
//! Pictures follow the reader's light or dark setting by default, falling
//! back to dark, with a fixed palette and a transparent background as options. `theme.rs` records how the palette was chosen.
//!
//! The renderer is deliberately crude: monospaced text, boxes stacked in
//! source order, and arrows in lanes. It exists so the data can be looked at
//! and reviewed. Layout measures every label whether or not it is shown, so
//! hiding a label never moves anything else, and a test checks this.
//!
//! The tests compare the committed schema and every example's rendered SVGs
//! with fresh output. After a deliberate change, run them with `INSET_BLESS=1`
//! to rewrite those files, and review the difference before committing it.

pub mod model;
pub mod view;

mod components;
mod emit;
mod flowchart;
mod svg;
mod theme;
mod validate;

pub use emit::{Emitted, emit, emit_with_sources};
pub use model::Diagram;
pub use theme::Theme;
pub use validate::validate;
pub use view::{Hide, VIEW_OPTIONS, View};

/// The JSON Schema of the diagram format, as pretty-printed JSON.
pub fn schema() -> String {
    let schema = schemars::schema_for!(Diagram);
    let mut out = serde_json::to_string_pretty(&schema).expect("a schema always serialises");
    out.push('\n');
    out
}

/// Reads diagram data from JSON, and checks that it is consistent.
pub fn from_json(json: &str) -> Result<Diagram, String> {
    let diagram: Diagram = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let errors = validate(&diagram);
    if errors.is_empty() {
        Ok(diagram)
    } else {
        Err(errors.join("\n"))
    }
}

/// Writes diagram data as pretty-printed JSON.
pub fn to_json(diagram: &Diagram) -> String {
    let mut out = serde_json::to_string_pretty(diagram).expect("diagram data always serialises");
    out.push('\n');
    out
}

/// Draws one view of the diagram as SVG.
pub fn render(diagram: &Diagram, view: &View) -> Result<String, String> {
    match &view.flowchart {
        None => Ok(components::render(diagram, view)),
        Some(id) => {
            let index = validate::Index::new(diagram);
            let function = index
                .functions
                .get(id.as_str())
                .ok_or_else(|| format!("no function has the ID `{id}`"))?;
            if function.body.is_none() {
                return Err(format!(
                    "`{id}` belongs to an external component, so it has no flowchart"
                ));
            }
            let owner = index
                .owners
                .get(id.as_str())
                .and_then(|owner| index.components.get(owner))
                .map(|component| component.ty.as_str());
            Ok(flowchart::render(function, owner, view))
        }
    }
}
