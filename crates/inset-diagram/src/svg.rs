//! SVG output shared by both views.
//!
//! Text is monospaced, so a label's width is its length in characters, and
//! layout needs no font metrics. Every element is written on one line, and an
//! element that a view option can hide carries the class `opt-<name>`. Layout
//! always measures every label, shown or not, and an option only decides
//! whether a line is written. So hiding a label changes nothing else.

use crate::theme;
use crate::view::{Hide, View};

/// The width of one character at 12px, in a typical monospaced font.
pub(crate) const CW: f64 = 7.2;
/// The height of one line of text.
pub(crate) const LH: f64 = 16.0;

/// A run of text with one style, which may be hidden by view options.
#[derive(Debug, Clone)]
pub(crate) struct Seg {
    pub text: String,
    pub class: &'static str,
    pub opts: Vec<Hide>,
}

impl Seg {
    pub fn new(text: impl Into<String>, class: &'static str) -> Self {
        Seg {
            text: text.into(),
            class,
            opts: Vec::new(),
        }
    }

    pub fn opt(mut self, hide: Hide) -> Self {
        self.opts.push(hide);
        self
    }

    pub fn opts(mut self, hides: &[Hide]) -> Self {
        self.opts.extend_from_slice(hides);
        self
    }
}

pub(crate) fn chars(text: &str) -> f64 {
    text.chars().count() as f64 * CW
}

pub(crate) fn width(segs: &[Seg]) -> f64 {
    segs.iter().map(|seg| chars(&seg.text)).sum()
}

/// Formats a coordinate with at most one decimal place.
pub(crate) fn n(value: f64) -> String {
    // Adding zero turns -0.0 into 0.0.
    let text = format!("{:.1}", (value * 10.0).round() / 10.0 + 0.0);
    match text.strip_suffix(".0") {
        Some(whole) => whole.to_string(),
        None => text,
    }
}

pub(crate) fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// Breaks text into lines of at most `max` characters, at spaces where it can.
/// Single line breaks are joined, as in a doc comment, and blank lines start a
/// new paragraph.
pub(crate) fn wrap(text: &str, max: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.split("\n\n") {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            let len = line.chars().count();
            if len > 0 && len + 1 + word.chars().count() > max {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        if !line.is_empty() {
            lines.push(line);
        }
    }
    lines
}

pub(crate) struct Canvas<'v> {
    view: &'v View,
    lines: Vec<String>,
}

impl<'v> Canvas<'v> {
    pub fn new(view: &'v View) -> Self {
        Canvas {
            view,
            lines: Vec::new(),
        }
    }

    fn class(class: &str, opts: &[Hide]) -> String {
        let mut out = class.to_string();
        for opt in opts {
            out.push_str(" opt-");
            out.push_str(opt.name());
        }
        out
    }

    /// Writes one element, unless the view hides it.
    pub fn element(&mut self, opts: &[Hide], line: String) {
        if self.view.shows(opts) {
            self.lines.push(line);
        }
    }

    /// Writes text starting at `x`, one element per run, so each run can be hidden alone.
    pub fn text(&mut self, x: f64, y: f64, segs: &[Seg]) {
        let mut x = x;
        for seg in segs {
            if !seg.text.is_empty() {
                let class = Self::class(seg.class, &seg.opts);
                self.element(
                    &seg.opts,
                    format!(
                        r#"<text x="{}" y="{}" class="{class}">{}</text>"#,
                        n(x),
                        n(y),
                        escape(&seg.text)
                    ),
                );
            }
            x += chars(&seg.text);
        }
    }

    /// A rectangle given as `(x, y, width, height)`, with corners of radius `rx`.
    pub fn rect(
        &mut self,
        (x, y, w, h): (f64, f64, f64, f64),
        rx: f64,
        class: &str,
        id: Option<&str>,
    ) {
        let id = id.map_or(String::new(), |id| format!(r#" data-id="{}""#, escape(id)));
        self.lines.push(format!(
            r#"<rect x="{}" y="{}" width="{}" height="{}" rx="{}" class="{class}"{id}/>"#,
            n(x),
            n(y),
            n(w),
            n(h),
            n(rx)
        ));
    }

    /// A polyline through `points`, with an arrowhead at the end when `arrow` names a marker.
    pub fn path(
        &mut self,
        points: &[(f64, f64)],
        class: &str,
        arrow: Option<&str>,
        id: Option<&str>,
    ) {
        let mut d = String::new();
        for (i, (x, y)) in points.iter().enumerate() {
            d.push_str(if i == 0 { "M" } else { " L" });
            d.push_str(&format!("{},{}", n(*x), n(*y)));
        }
        let marker = arrow.map_or(String::new(), |m| format!(r#" marker-end="url(#{m})""#));
        let id = id.map_or(String::new(), |id| format!(r#" data-id="{}""#, escape(id)));
        self.lines
            .push(format!(r#"<path d="{d}" class="{class}"{marker}{id}/>"#));
    }

    pub fn polygon(&mut self, points: &[(f64, f64)], class: &str, id: Option<&str>) {
        let points: Vec<_> = points
            .iter()
            .map(|(x, y)| format!("{},{}", n(*x), n(*y)))
            .collect();
        let id = id.map_or(String::new(), |id| format!(r#" data-id="{}""#, escape(id)));
        self.lines.push(format!(
            r#"<polygon points="{}" class="{class}"{id}/>"#,
            points.join(" ")
        ));
    }

    pub fn circle(&mut self, x: f64, y: f64, r: f64, class: &str, id: Option<&str>) {
        let id = id.map_or(String::new(), |id| format!(r#" data-id="{}""#, escape(id)));
        self.lines.push(format!(
            r#"<circle cx="{}" cy="{}" r="{}" class="{class}"{id}/>"#,
            n(x),
            n(y),
            n(r)
        ));
    }

    /// Opens a group for one element, with its description as a tooltip.
    pub fn open(&mut self, id: &str, doc: Option<&str>) {
        self.lines.push(format!(r#"<g data-id="{}">"#, escape(id)));
        if let Some(doc) = doc {
            self.element(
                &[Hide::Docs],
                format!(
                    r#"<title class="opt-docs">{}</title>"#,
                    // A character reference keeps the element on one line.
                    escape(doc).replace('\n', "&#10;")
                ),
            );
        }
    }

    pub fn close(&mut self) {
        self.lines.push("</g>".into());
    }

    pub fn finish(self, width: f64, height: f64) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xml:space="preserve" class="inset-diagram" width="{w}" height="{h}" viewBox="0 0 {w} {h}">"#,
            w = n(width),
            h = n(height)
        ));
        out.push('\n');
        out.push_str(&theme::style(self.view.theme, self.view.coverage));
        out.push_str(DEFS);
        if !self.view.transparent {
            out.push_str(&format!(
                r#"<rect x="0" y="0" width="{}" height="{}" class="ground"/>"#,
                n(width),
                n(height)
            ));
            out.push('\n');
        }
        for line in self.lines {
            out.push_str(&line);
            out.push('\n');
        }
        out.push_str("</svg>\n");
        out
    }
}

const DEFS: &str = r##"<defs>
<marker id="arrow" viewBox="0 0 8 8" refX="8" refY="4" markerWidth="8" markerHeight="8" orient="auto"><path d="M0,0 L8,4 L0,8 z" class="arrow"/></marker>
<marker id="arrow-cap" viewBox="0 0 8 8" refX="8" refY="4" markerWidth="8" markerHeight="8" orient="auto"><path d="M0,0 L8,4 L0,8 z" class="arrow-cap"/></marker>
</defs>
"##;
