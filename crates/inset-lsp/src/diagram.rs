//! `inset/diagram`, a request of Inset's own: the picture `inset diagram`
//! draws of the program a document belongs to, for an editor to show
//! beside the code, and where in the source each element of it is.
//!
//! The parameters are the document and the view options `inset diagram`
//! takes, such as `["--theme", "dark"]`, so a picture in the editor is the
//! picture on the command line. The result has the SVG, the functions whose
//! flowchart can be drawn, by ID, and a location for every element the
//! picture draws, by the ID in its `data-id` attribute. A program that does
//! not check, or has a part that cannot be drawn yet, gets its problems
//! instead, each with its location. A document outside any program gets
//! `null`.
//!
//! It is answered here rather than by running `inset diagram`, so the
//! picture comes from the text being edited, by the same compiler that
//! publishes its diagnostics, and its locations count characters as the
//! editor does.

use std::collections::BTreeMap;

use inset_diagram::model::{Component, Diagram, Function};
use inset_diagram::{View, emit_with_sources, render};
use lsp_types::{Location, TextDocumentIdentifier};
use serde::{Deserialize, Serialize};

use crate::position::LineIndex;
use crate::{Server, Unit, uri};

pub(crate) const METHOD: &str = "inset/diagram";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Params {
    pub text_document: TextDocumentIdentifier,
    /// View options, as `inset diagram` takes them.
    #[serde(default)]
    pub options: Vec<String>,
}

#[derive(Debug, Default, Serialize)]
pub(crate) struct Picture {
    /// The app's name.
    pub app: Option<String>,
    pub svg: Option<String>,
    /// The functions with a flowchart, by ID, in the order they are drawn.
    pub functions: Vec<String>,
    /// Where each element is, by ID.
    pub sources: BTreeMap<String, Location>,
    /// Why there is no picture.
    pub problems: Vec<Problem>,
}

#[derive(Debug, Serialize)]
pub(crate) struct Problem {
    /// As a diagnostic's: the message, then its label and help.
    pub message: String,
    pub location: Option<Location>,
}

impl Server<'_> {
    /// The picture, or `Err` with a message for view options it cannot read.
    pub(crate) fn diagram(&self, params: Params) -> Result<Option<Picture>, String> {
        let (view, _) = View::from_args(&params.options)?;
        let Some(path) = uri::to_path(&params.text_document.uri) else {
            return Ok(None);
        };
        let Unit::Program(root) = self.unit(&path) else {
            return Ok(None);
        };
        let program = self.program(&root);
        let location = |file: usize, span| {
            let file = &program.files[file];
            Location {
                uri: self.uri_of(&root.join(&file.path)),
                range: LineIndex::new(&file.text, self.encoding).range(span),
            }
        };
        let emitted = match emit_with_sources(&program) {
            Ok(emitted) => emitted,
            Err(reports) => {
                let problems = reports
                    .iter()
                    .map(|report| Problem {
                        message: crate::message(&report.diagnostic),
                        location: Some(location(report.file, report.diagnostic.span)),
                    })
                    .collect();
                return Ok(Some(Picture {
                    problems,
                    ..Picture::default()
                }));
            }
        };
        let diagram = &emitted.diagram;
        let mut picture = Picture {
            app: Some(diagram.app.name.clone()),
            functions: functions(diagram),
            sources: emitted
                .sources
                .iter()
                .map(|(id, span)| (id.clone(), location(emitted.file, *span)))
                .collect(),
            ..Picture::default()
        };
        match render(diagram, &view) {
            Ok(svg) => picture.svg = Some(svg),
            Err(message) => picture.problems.push(Problem {
                message,
                location: None,
            }),
        }
        Ok(Some(picture))
    }
}

/// The IDs of the functions with a flowchart: each component's in the
/// order they are drawn, then the algorithms at the top level.
fn functions(diagram: &Diagram) -> Vec<String> {
    fn walk(component: &Component, out: &mut Vec<String>) {
        let handlers = component.routes.iter().map(|r| &r.handler);
        let functions = handlers
            .chain(&component.messages)
            .chain(&component.algorithms);
        out.extend(functions.filter(|f| f.body.is_some()).map(id));
        for child in &component.components {
            walk(child, out);
        }
    }
    fn id(function: &Function) -> String {
        function.id.clone()
    }
    let mut out = Vec::new();
    for component in &diagram.app.components {
        walk(component, &mut out);
    }
    out.extend(diagram.algorithms.iter().map(id));
    out
}
