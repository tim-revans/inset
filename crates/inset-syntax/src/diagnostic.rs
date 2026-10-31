use ariadne::{Config, IndexType, Label, Report, ReportKind, Source};

use crate::Span;

/// An error in the source: what is wrong, where, and how to fix it if that
/// is known.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    /// What is wrong, as one sentence without a full stop.
    pub message: String,
    pub span: Span,
    /// Said at the span, such as "expected `;` after this".
    pub label: String,
    /// What to change, when the parser can tell.
    pub help: Option<String>,
}

impl Diagnostic {
    pub fn new(message: impl Into<String>, span: Span, label: impl Into<String>) -> Diagnostic {
        Diagnostic {
            message: message.into(),
            span,
            label: label.into(),
            help: None,
        }
    }

    /// The diagnostic with its span moved on by `offset` bytes.
    pub fn shifted(mut self, offset: usize) -> Diagnostic {
        self.span = self.span.shifted(offset);
        self
    }

    /// The diagnostic with its span moved back by `offset` bytes.
    pub fn unshifted(mut self, offset: usize) -> Diagnostic {
        self.span = self.span.unshifted(offset);
        self
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Diagnostic {
        self.help = Some(help.into());
        self
    }

    /// Renders the diagnostic with the source line it points at, as the
    /// command line shows it. `path` names the file in the output.
    pub fn render(&self, path: &str, source: &str, color: bool) -> String {
        let span = visible(self.span, source);
        let mut report = Report::build(ReportKind::Error, (path, span.range()))
            .with_config(
                Config::default()
                    .with_color(color)
                    .with_index_type(IndexType::Byte),
            )
            .with_message(&self.message)
            .with_label(Label::new((path, span.range())).with_message(&self.label));
        if let Some(help) = &self.help {
            report = report.with_help(help);
        }
        let mut out = Vec::new();
        report
            .finish()
            .write((path, Source::from(source)), &mut out)
            .expect("writing to a Vec cannot fail");
        String::from_utf8_lossy(&out).into_owned()
    }
}

/// Widens an empty span, such as the end of the file, to a character next to
/// it, so the rendered report has something to underline.
fn visible(span: Span, source: &str) -> Span {
    let end = span.end.min(source.len());
    let start = span.start.min(end);
    if start < end {
        return Span::new(start, end);
    }
    if let Some(c) = source[start..].chars().next() {
        Span::new(start, start + c.len_utf8())
    } else if let Some(c) = source[..start].chars().next_back() {
        Span::new(start - c.len_utf8(), start)
    } else {
        Span::new(0, 0)
    }
}
