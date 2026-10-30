//! A document's outline: the app with its settings, grants and children;
//! each component with its fields, children and handlers; and each
//! requirement by its sentence. It comes from the file's syntax tree alone,
//! so it works on a file that does not check.

use inset_syntax::ast::*;
use inset_syntax::{Ident, Span, parse_code, parse_requirements};
use lsp_types::{DocumentSymbol, SymbolKind};

use crate::position::LineIndex;

pub(crate) fn outline(text: &str, requirements: bool, lines: &LineIndex) -> Vec<DocumentSymbol> {
    let outline = Outline { text, lines };
    if requirements {
        let file = parse_requirements(text).ast;
        return file
            .requirements
            .iter()
            .filter_map(|r| {
                outline.symbol(
                    &r.sentence.value,
                    SymbolKind::FUNCTION,
                    Some("requirement".into()),
                    r.span,
                    r.sentence.span,
                    Vec::new(),
                )
            })
            .collect();
    }
    let module = parse_code(text).ast;
    module
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ItemKind::App(app) => outline.app(app, item.span),
            ItemKind::Component(component) => outline.component(component, item.span),
            ItemKind::Function(function) => outline.function(function),
        })
        .collect()
}

struct Outline<'a> {
    text: &'a str,
    lines: &'a LineIndex<'a>,
}

impl Outline<'_> {
    fn symbol(
        &self,
        name: &str,
        kind: SymbolKind,
        detail: Option<String>,
        span: Span,
        selection: Span,
        children: Vec<DocumentSymbol>,
    ) -> Option<DocumentSymbol> {
        // A client rejects a symbol without a name, which a broken
        // declaration can have.
        if name.trim().is_empty() {
            return None;
        }
        #[allow(deprecated)]
        Some(DocumentSymbol {
            name: name.to_string(),
            detail,
            kind,
            tags: None,
            deprecated: None,
            range: self.lines.range(span),
            selection_range: self.lines.range(selection),
            children: (!children.is_empty()).then_some(children),
        })
    }

    fn named(
        &self,
        name: &Ident,
        kind: SymbolKind,
        detail: Option<String>,
        span: Span,
        children: Vec<DocumentSymbol>,
    ) -> Option<DocumentSymbol> {
        self.symbol(&name.name, kind, detail, span, name.span, children)
    }

    fn source(&self, span: Span) -> String {
        self.text.get(span.range()).unwrap_or("").trim().to_string()
    }

    fn app(&self, app: &App, span: Span) -> Option<DocumentSymbol> {
        let settings = app.settings.iter().filter_map(|s| {
            let detail = Some(self.source(s.ty.span));
            self.named(&s.name, SymbolKind::CONSTANT, detail, s.span, Vec::new())
        });
        let grants = app.grants.iter().filter_map(|g| {
            let detail = Some(self.source(g.ty.span));
            self.named(&g.name, SymbolKind::PROPERTY, detail, g.span, Vec::new())
        });
        let children = settings
            .chain(grants)
            .chain(self.starts(&app.starts))
            .collect();
        self.named(
            &app.name,
            SymbolKind::MODULE,
            Some("app".into()),
            span,
            children,
        )
    }

    fn component(&self, component: &Component, span: Span) -> Option<DocumentSymbol> {
        let fields = component.fields.iter().filter_map(|f| {
            let detail = Some(self.source(f.ty.span));
            self.named(&f.name, SymbolKind::FIELD, detail, f.span, Vec::new())
        });
        let functions = component.functions.iter().filter_map(|f| self.function(f));
        let children = fields
            .chain(self.starts(&component.starts))
            .chain(functions)
            .collect();
        let detail = Some("component".into());
        self.named(&component.name, SymbolKind::CLASS, detail, span, children)
    }

    fn starts<'s>(&'s self, starts: &'s [Start]) -> impl Iterator<Item = DocumentSymbol> + 's {
        starts.iter().filter_map(|s| {
            let detail = Some(s.component.name.clone());
            let name = s.binding.as_ref().unwrap_or(&s.component);
            self.named(name, SymbolKind::OBJECT, detail, s.span, Vec::new())
        })
    }

    /// A handler, shown with its route or as a message, or an algorithm.
    fn function(&self, function: &Function) -> Option<DocumentSymbol> {
        let route = function.attrs.iter().find_map(|a| match &a.kind {
            AttributeKind::Route(route) => Some(route),
            _ => None,
        });
        let message = function
            .attrs
            .iter()
            .any(|a| a.kind == AttributeKind::Message);
        let (kind, detail) = match route {
            Some(route) => (
                SymbolKind::METHOD,
                format!("{} {}", route.method.as_str(), route.path.value),
            ),
            None if message => (SymbolKind::METHOD, "message".into()),
            None => (SymbolKind::FUNCTION, "algorithm".into()),
        };
        self.named(
            &function.name,
            kind,
            Some(detail),
            function.span,
            Vec::new(),
        )
    }
}
