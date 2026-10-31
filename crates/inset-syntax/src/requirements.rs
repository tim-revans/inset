//! The requirements dialect's parser.
//!
//! Requirements are laid out by lines, so this parser works a line at a time:
//! a requirement starts at the left margin, each step is indented by four
//! spaces, and a step continues onto lines indented further. Within a step,
//! words are separated by spaces, and strings follow Rust's rules. An error
//! in one step is reported and the parser goes on with the next.

use crate::ast::*;
use crate::lexer::{self, TokenKind};
use crate::parser::StepFact;
use crate::{Diagnostic, Ident, Span};

const STEP_INDENT: usize = 4;

pub(crate) fn parse(source: &str) -> (RequirementsFile, Vec<Diagnostic>) {
    let mut parser = ReqParser {
        source,
        lines: lines(source),
        next: 0,
        diagnostics: Vec::new(),
        row: None,
        mapped: Vec::new(),
        for_all: false,
    };
    let mut requirements = Vec::new();
    while let Some(line) = parser.next_line() {
        let first_word = line.text.split_whitespace().next();
        if line.indent == 0 && first_word == Some("requirement") {
            if let Some(requirement) = parser.parse_requirement(line) {
                requirements.push(requirement);
            }
        } else if line.indent == 0 {
            parser.error(Diagnostic::new(
                "expected `requirement`",
                line.text_span(),
                "a line at the left margin starts a requirement",
            ));
        } else {
            parser.error(
                Diagnostic::new(
                    "step outside a requirement",
                    line.text_span(),
                    "this step has no `requirement` above it",
                )
                .with_help("start the requirement with `requirement \"...\"` at the left margin"),
            );
            // Its continuation lines belong to it, so skip them too.
            parser.continuation(line.indent);
        }
    }
    (RequirementsFile { requirements }, parser.diagnostics)
}

#[derive(Clone, Copy)]
struct Line<'a> {
    /// The byte offset of the line's first character.
    start: usize,
    /// Spaces before the text.
    indent: usize,
    /// The text after the indentation, without the line break.
    text: &'a str,
}

impl Line<'_> {
    fn text_start(&self) -> usize {
        self.start + self.indent
    }

    fn end(&self) -> usize {
        self.text_start() + self.text.len()
    }

    fn text_span(&self) -> Span {
        Span::new(self.text_start(), self.end())
    }

    /// Blank, or only a comment.
    fn is_blank(&self) -> bool {
        let text = self.text.trim_start();
        text.is_empty() || text.starts_with("//")
    }
}

fn lines(source: &str) -> Vec<Line<'_>> {
    let mut lines = Vec::new();
    let mut start = 0;
    for raw in source.split_inclusive('\n') {
        let text = raw.strip_suffix('\n').unwrap_or(raw);
        let text = text.strip_suffix('\r').unwrap_or(text);
        let indent = text.len() - text.trim_start_matches(' ').len();
        lines.push(Line {
            start,
            indent,
            text: &text[indent..],
        });
        start += raw.len();
    }
    lines
}

struct ReqParser<'a> {
    source: &'a str,
    lines: Vec<Line<'a>>,
    next: usize,
    diagnostics: Vec<Diagnostic>,
    /// While reading an examples row, how the row's text maps to the file.
    row: Option<&'a RowMap>,
    /// Diagnostics that already have the file's spans, from facts read
    /// in an examples row.
    mapped: Vec<Diagnostic>,
    /// Whether the requirement being read starts with `for all`, whose
    /// placeholders are not supported yet.
    for_all: bool,
}

impl<'a> ReqParser<'a> {
    fn error(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    /// The next line that is not blank, reporting any tab in its indentation.
    fn next_line(&mut self) -> Option<Line<'a>> {
        loop {
            let line = *self.lines.get(self.next)?;
            self.next += 1;
            if line.text.starts_with('\t') {
                let tab = line.text_start();
                self.error(
                    Diagnostic::new(
                        "tab in a requirement's indentation",
                        Span::new(tab, tab + 1),
                        "this is a tab",
                    )
                    .with_help("indent with spaces: four for a step"),
                );
                continue;
            }
            if !line.is_blank() {
                return Some(line);
            }
        }
    }

    /// The next non-blank line, if it is indented more than `indent`, without
    /// consuming it otherwise.
    fn next_deeper(&mut self, indent: usize) -> Option<Line<'a>> {
        let saved = self.next;
        match self.next_line() {
            Some(line) if line.indent > indent => Some(line),
            _ => {
                self.next = saved;
                None
            }
        }
    }

    /// Skips the lines indented more than `indent`.
    fn continuation(&mut self, indent: usize) {
        while self.next_deeper(indent).is_some() {}
    }

    fn parse_requirement(&mut self, header: Line<'a>) -> Option<Requirement> {
        let mut cursor = Cursor::new(self.source, header.text_span());
        cursor.word();
        let sentence = match cursor.string() {
            Ok(Some(sentence)) => Some(sentence),
            Ok(None) => {
                self.error(
                    Diagnostic::new(
                        "expected the requirement as a string",
                        cursor.here(),
                        "the requirement's sentence goes here",
                    )
                    .with_help("write `requirement \"Visitors are greeted by name\"`"),
                );
                None
            }
            Err(diagnostic) => {
                self.error(diagnostic);
                None
            }
        };
        if sentence.is_some() {
            self.expect_end(&mut cursor);
        }
        self.diagnostics.append(&mut cursor.diagnostics);
        self.for_all = self.lines[self.next..]
            .iter()
            .find(|l| !l.is_blank())
            .is_some_and(|l| l.indent > 0 && l.text.starts_with("for "));
        let examples_at = self.find_examples();
        let first_step = self.next;
        let (steps, any_step_lines) = if examples_at.is_some() {
            // The steps have placeholders, so they are read once for each row.
            self.next = examples_at.unwrap_or(self.next);
            (Vec::new(), true)
        } else {
            self.parse_steps()
        };
        let examples = examples_at.and_then(|at| {
            let steps_end = self.lines[at].start;
            let steps_start = self.lines[first_step..at]
                .iter()
                .find(|l| !l.is_blank())
                .map_or(steps_end, |l| l.start);
            self.next = at + 1;
            self.examples(self.lines[at], Span::new(steps_start, steps_end))
        });
        let sentence = sentence?;
        if !any_step_lines {
            self.error(Diagnostic::new(
                "requirement has no steps",
                sentence.span,
                "this requirement says nothing to test",
            ));
        }
        let end = match &examples {
            Some(examples) => examples.rows.last().map_or(examples.span, |r| r.span),
            None => steps.last().map_or(sentence.span, |s| s.span),
        };
        Some(Requirement {
            span: Span::new(header.text_start(), end.end),
            sentence,
            steps,
            examples,
        })
    }

    /// The index of the requirement's `examples` line, if it has one.
    fn find_examples(&self) -> Option<usize> {
        for (i, line) in self.lines.iter().enumerate().skip(self.next) {
            if line.is_blank() {
                continue;
            }
            if line.indent == 0 {
                return None;
            }
            if line.indent == STEP_INDENT && line.text.split_whitespace().next() == Some("examples")
            {
                return Some(i);
            }
        }
        None
    }

    /// The steps up to the end of the requirement, and whether it had any
    /// lines.
    fn parse_steps(&mut self) -> (Vec<Step>, bool) {
        let mut steps: Vec<Step> = Vec::new();
        let mut any_step_lines = false;
        while let Some(line) = self.next_deeper(0) {
            any_step_lines = true;
            if line.indent != STEP_INDENT {
                self.error(
                    Diagnostic::new(
                        format!("step indented by {} spaces", line.indent),
                        Span::new(line.start, line.text_start()),
                        "steps are indented by four spaces",
                    )
                    .with_help("a line indented further than a step continues the step above it"),
                );
                self.continuation(line.indent);
                continue;
            }
            let previous = steps.last().map(|s| &s.kind);
            let reported = self.diagnostics.len();
            match self.parse_step(line, previous) {
                Some(step) => steps.push(step),
                None => self.placeholders(line, reported),
            }
        }
        (steps, any_step_lines)
    }

    /// The `examples` table on the line `keyword` and the lines after it,
    /// with each row's reading of the steps in `steps`.
    fn examples(&mut self, keyword: Line<'a>, steps: Span) -> Option<Examples> {
        let span = Span::new(
            keyword.text_start(),
            keyword.text_start() + "examples".len(),
        );
        let rest = Span::new(span.end, keyword.end());
        if !self.source[rest.start..rest.end].trim().is_empty() {
            self.error(
                Diagnostic::new(
                    "unexpected text after `examples`",
                    rest,
                    "the table starts on the next line",
                )
                .with_help("write the headings on the line below, as `| on hand | left |`"),
            );
        }
        let Some(heading_line) = self.next_deeper(0) else {
            self.error(
                Diagnostic::new("`examples` without a table", span, "no table follows this")
                    .with_help(
                        "write the headings and then one row per case, as `| on hand | left |`",
                    ),
            );
            return None;
        };
        let headings = self.cells(heading_line)?;
        let mut ok = true;
        for (i, heading) in headings.iter().enumerate() {
            let reason = if heading.value.is_empty() {
                Some("a heading names a placeholder, so it cannot be empty")
            } else if heading.value.contains(['<', '>']) {
                Some("a heading is written without `<` and `>`, which only its placeholders have")
            } else if headings[..i].iter().any(|h| h.value == heading.value) {
                Some("each heading names one placeholder, so it appears once")
            } else {
                None
            };
            if let Some(reason) = reason {
                self.error(Diagnostic::new(
                    format!(
                        "this heading cannot name a placeholder: `{}`",
                        heading.value
                    ),
                    heading.span,
                    reason,
                ));
                ok = false;
            }
        }
        let mut rows = Vec::new();
        while let Some(line) = self.next_deeper(0) {
            let Some(cells) = self.cells(line) else {
                self.continuation(line.indent);
                continue;
            };
            let span = line.text_span();
            if cells.len() != headings.len() {
                self.error(Diagnostic::new(
                    format!(
                        "this row has {}, but the table has {}",
                        count(cells.len(), "cell"),
                        count(headings.len(), "heading")
                    ),
                    span,
                    "each row has one cell under each heading",
                ));
                ok = false;
                continue;
            }
            rows.push((cells, span));
        }
        if rows.is_empty() && ok {
            self.error(
                Diagnostic::new(
                    "the examples table has no rows",
                    heading_line.text_span(),
                    "only the headings are here",
                )
                .with_help("add a row for each case, below the headings"),
            );
            return None;
        }
        if !ok {
            return None;
        }
        let mut seen: Vec<Diagnostic> = Vec::new();
        let rows = rows
            .into_iter()
            .map(|(cells, span)| {
                let (steps, placeholders, diagnostics) = self.row(steps, &headings, &cells);
                let line = self.source[..span.start].matches('\n').count() + 1;
                for mut diagnostic in diagnostics {
                    // A diagnostic about a placeholder is the row's own.
                    if placeholders.iter().any(|p| {
                        p.start < diagnostic.span.end.max(diagnostic.span.start + 1)
                            && diagnostic.span.start < p.end
                    }) {
                        let note = format!("in the examples row on line {line}");
                        diagnostic.help = Some(match diagnostic.help {
                            Some(help) => format!("{help}; {note}"),
                            None => note,
                        });
                    }
                    if !seen.contains(&diagnostic) {
                        seen.push(diagnostic.clone());
                        self.error(diagnostic);
                    }
                }
                ExampleRow {
                    cells,
                    span,
                    steps,
                    placeholders,
                }
            })
            .collect();
        Some(Examples {
            span,
            headings,
            rows,
        })
    }

    /// The cells of a table line, `| a | b |`, each trimmed.
    fn cells(&mut self, line: Line<'a>) -> Option<Vec<Text>> {
        let text = line.text.trim_end();
        let inner = text.strip_prefix('|').and_then(|t| t.strip_suffix('|'));
        let Some(inner) = inner.filter(|_| text.len() > 1) else {
            self.error(
                Diagnostic::new(
                    "expected a row of the examples table",
                    line.text_span(),
                    "a row starts and ends with `|`",
                )
                .with_help("write the cells between `|`, as `| 12 | 50 |`"),
            );
            return None;
        };
        let mut cells = Vec::new();
        let mut start = line.text_start() + 1;
        for raw in inner.split('|') {
            let leading = raw.len() - raw.trim_start().len();
            let value = raw.trim();
            let at = start + leading;
            cells.push(Text {
                value: value.to_string(),
                span: Span::new(at, at + value.len()),
            });
            start += raw.len() + 1;
        }
        Some(cells)
    }

    /// The steps in `steps`, read with `cells` in place of the placeholders
    /// that `headings` name, with the file's spans. Also where the
    /// placeholders are, and what was wrong.
    fn row(
        &self,
        steps: Span,
        headings: &[Text],
        cells: &[Text],
    ) -> (Vec<Step>, Vec<Span>, Vec<Diagnostic>) {
        let template = &self.source[steps.start..steps.end];
        let mut text = self.source[..steps.start].to_string();
        let mut map = RowMap {
            from: steps.start,
            segments: Vec::new(),
        };
        let mut placeholders = Vec::new();
        let mut copied = 0;
        let mut from = 0;
        while let Some((start, end)) =
            placeholder(&template[from..]).map(|(s, e)| (from + s, from + e))
        {
            let name = &template[start + 1..end - 1];
            from = end;
            let Some(column) = headings.iter().position(|h| h.value == name) else {
                continue;
            };
            let at = text.len();
            text.push_str(&template[copied..start]);
            map.segments.push(Segment {
                text: (at, text.len()),
                file: steps.start + copied,
                placeholder: None,
            });
            let placeholder = Span::new(steps.start + start, steps.start + end);
            let at = text.len();
            text.push_str(&cells[column].value);
            map.segments.push(Segment {
                text: (at, text.len()),
                file: placeholder.start,
                placeholder: Some(placeholder),
            });
            placeholders.push(placeholder);
            copied = end;
        }
        let at = text.len();
        text.push_str(&template[copied..]);
        map.segments.push(Segment {
            text: (at, text.len()),
            file: steps.start + copied,
            placeholder: None,
        });
        let mut parser = ReqParser {
            source: &text,
            lines: lines(&text),
            next: 0,
            diagnostics: Vec::new(),
            row: Some(&map),
            mapped: Vec::new(),
            for_all: false,
        };
        parser.next = parser
            .lines
            .iter()
            .position(|l| l.start >= steps.start)
            .unwrap_or(parser.lines.len());
        let (mut steps, _) = parser.parse_steps();
        for step in &mut steps {
            map.step(step);
        }
        let mut diagnostics: Vec<Diagnostic> = parser
            .diagnostics
            .into_iter()
            .map(|mut d| {
                d.span = map.span(d.span);
                d
            })
            .collect();
        diagnostics.append(&mut parser.mapped);
        diagnostics.sort_by_key(|d| d.span.start);
        (steps, placeholders, diagnostics)
    }

    /// Reports a step that failed to parse because it has a placeholder,
    /// such as `<on hand>`, as a placeholder, instead of the errors found
    /// since `reported`.
    fn placeholders(&mut self, line: Line<'a>, reported: usize) {
        let Some((start, end)) = placeholder(line.text) else {
            return;
        };
        self.diagnostics.truncate(reported);
        self.mapped.retain(|_| false);
        let span = Span::new(line.text_start() + start, line.text_start() + end);
        let diagnostic = if self.for_all {
            Diagnostic::new(
                "a placeholder from `for all` is not supported yet",
                span,
                "the parser does not handle this yet",
            )
        } else if self.row.is_some() {
            Diagnostic::new(
                format!(
                    "`{}` is not a heading of the examples table",
                    &line.text[start..end]
                ),
                span,
                "no column gives this placeholder a value",
            )
            .with_help(
                "name the placeholder after one of the table's headings, or add a column for it",
            )
        } else {
            Diagnostic::new(
                "a placeholder without an `examples` table",
                span,
                "nothing gives this placeholder a value",
            )
            .with_help("list the cases in an `examples` table after the steps, with a heading for each placeholder")
        };
        self.error(diagnostic);
    }

    /// Parses the step starting on `line`, with its continuation lines and
    /// any text block. `previous` is the kind of the step before, which an
    /// `and` step repeats.
    fn parse_step(&mut self, line: Line<'a>, previous: Option<&StepKind>) -> Option<Step> {
        let mut end = line.end();
        let mut text_block = None;
        let mut block_failed = false;
        while let Some(next) = self.next_deeper(STEP_INDENT) {
            if next.text.trim_end() == "\"\"\"" {
                text_block = self.parse_text_block(next);
                block_failed = text_block.is_none();
                break;
            }
            end = next.end();
        }
        let span = Span::new(line.text_start(), end);
        let mut cursor = Cursor::new(self.source, span);
        let step = self.parse_step_words(&mut cursor, previous, &mut text_block, block_failed);
        if step.is_none() {
            // A text block after a step that failed is part of what failed.
            text_block = None;
        }
        self.diagnostics.append(&mut cursor.diagnostics);
        if let Some(block) = text_block {
            self.error(
                Diagnostic::new(
                    "text block without `with`",
                    block.span,
                    "nothing in the step takes this text",
                )
                .with_help("a text block follows `with` at the end of a step"),
            );
        }
        let (keyword, kind) = step?;
        let span = match &kind {
            StepKind::Request { body: Some(b), .. } | StepKind::Reply { body: Some(b), .. } => {
                span.to(b.span)
            }
            _ => span,
        };
        Some(Step {
            keyword,
            kind,
            span,
        })
    }

    fn parse_step_words(
        &mut self,
        cursor: &mut Cursor<'a>,
        previous: Option<&StepKind>,
        text_block: &mut Option<Text>,
        block_failed: bool,
    ) -> Option<(StepKeyword, StepKind)> {
        let (word, word_span) = cursor.word()?;
        let (keyword, kind) = match word.as_str() {
            "given" => (StepKeyword::Given, Kind::Given),
            "when" => (StepKeyword::When, Kind::When),
            "then" => (StepKeyword::Then, Kind::Then),
            "and" => match previous {
                Some(StepKind::Request { .. }) => (StepKeyword::And, Kind::When),
                Some(StepKind::Fact { given: true, .. }) => (StepKeyword::And, Kind::Given),
                Some(StepKind::Reply { .. } | StepKind::Fact { .. } | StepKind::Check(_)) => {
                    (StepKeyword::And, Kind::Then)
                }
                None => {
                    self.error(
                        Diagnostic::new(
                            "`and` with no step before it",
                            word_span,
                            "`and` repeats the kind of the step before it",
                        )
                        .with_help("start with `given`, `when` or `then`"),
                    );
                    return None;
                }
            },
            "for" => {
                self.unsupported("`for all`", word_span);
                return None;
            }
            "examples" => {
                self.error(Diagnostic::new(
                    "`examples` after the steps' table",
                    word_span,
                    "a requirement has one examples table",
                ));
                return None;
            }
            _ => {
                self.error(Diagnostic::new(
                    format!("expected a step, found `{word}`"),
                    word_span,
                    "a step starts with `given`, `when`, `then` or `and`",
                ));
                return None;
            }
        };
        // A `then` step about a reply names the app and says `replies`.
        // Any other is a fact or a check.
        let replies = cursor.peek_second_word().is_some_and(|w| w == "replies");
        let request = match kind {
            Kind::Given => return self.fact(cursor, false).map(|kind| (keyword, kind)),
            Kind::Then if !replies => return self.fact(cursor, true).map(|kind| (keyword, kind)),
            Kind::When => true,
            Kind::Then => false,
        };
        let app = self.app_name(cursor)?;
        let kind = if request {
            self.expect_word(cursor, "receives", "after the app's name in a `when` step")?;
            let method = self.method(cursor)?;
            let path = self.path(cursor)?;
            let via = self.via(cursor)?;
            let body = self.body(cursor, text_block, block_failed)?;
            StepKind::Request {
                app,
                method,
                path,
                via,
                body,
            }
        } else {
            cursor.word();
            let status = match cursor.word() {
                Some((word, span)) if is_upper_camel(&word) => Ident::new(word, span),
                Some((word, span)) => {
                    self.error(
                        Diagnostic::new(
                            format!("expected a status, found `{word}`"),
                            span,
                            "statuses are written by name",
                        )
                        .with_help(
                            "write the status's name in UpperCamelCase, such as `OK` or `NotFound`",
                        ),
                    );
                    return None;
                }
                None => {
                    self.error(
                        Diagnostic::new(
                            "expected a status after `replies`",
                            cursor.here(),
                            "the reply's status goes here",
                        )
                        .with_help("such as `OK` or `NotFound`"),
                    );
                    return None;
                }
            };
            let body = self.body(cursor, text_block, block_failed)?;
            StepKind::Reply { app, status, body }
        };
        if !self.expect_end(cursor) {
            return None;
        }
        Some((keyword, kind))
    }

    /// The rest of a `given` or `then` step: a fact, or after `then`, a check.
    fn fact(&mut self, cursor: &mut Cursor<'a>, allow_check: bool) -> Option<StepKind> {
        let span = cursor.span_of_rest();
        if span.is_empty() {
            let what = if allow_check {
                "a fact or a condition"
            } else {
                "a fact"
            };
            self.error(Diagnostic::new(
                format!("expected {what}"),
                span,
                "the step says nothing here",
            ));
            return None;
        }
        cursor.pos = span.end;
        let (parsed, mut diagnostics) =
            crate::parse_step_fact(self.source, span, allow_check, self.row);
        if self.row.is_some() {
            self.mapped.append(&mut diagnostics);
        } else {
            self.diagnostics.append(&mut diagnostics);
        }
        match parsed? {
            StepFact::Fact(fact) => Some(StepKind::Fact {
                given: !allow_check,
                fact: *fact,
            }),
            StepFact::Check(expr) => Some(StepKind::Check(expr)),
        }
    }

    fn unsupported(&mut self, what: &str, span: Span) {
        self.error(Diagnostic::new(
            format!("{what} is not supported yet"),
            span,
            "the parser does not handle this yet",
        ));
    }

    fn app_name(&mut self, cursor: &mut Cursor<'a>) -> Option<Ident> {
        match cursor.word() {
            Some((word, span)) if is_upper_camel(&word) => Some(Ident::new(word, span)),
            Some((word, span)) => {
                self.error(Diagnostic::new(
                    format!("expected the app's name, found `{word}`"),
                    span,
                    "the step names the app, such as `HelloHttp`",
                ));
                None
            }
            None => {
                self.error(Diagnostic::new(
                    "expected the app's name",
                    cursor.here(),
                    "the step names the app here",
                ));
                None
            }
        }
    }

    fn expect_word(
        &mut self,
        cursor: &mut Cursor<'a>,
        expected: &str,
        context: &str,
    ) -> Option<()> {
        match cursor.word() {
            Some((word, _)) if word == expected => Some(()),
            Some((word, span)) => {
                self.error(Diagnostic::new(
                    format!("expected `{expected}` {context}, found `{word}`"),
                    span,
                    format!("expected `{expected}`"),
                ));
                None
            }
            None => {
                self.error(Diagnostic::new(
                    format!("expected `{expected}` {context}"),
                    cursor.here(),
                    format!("expected `{expected}`"),
                ));
                None
            }
        }
    }

    fn method(&mut self, cursor: &mut Cursor<'a>) -> Option<HttpMethod> {
        let Some((word, span)) = cursor.word() else {
            self.error(Diagnostic::new(
                "expected an HTTP method after `receives`",
                cursor.here(),
                "such as `GET` or `POST`",
            ));
            return None;
        };
        if let Some(method) = HttpMethod::from_request(&word) {
            return Some(method);
        }
        let mut diagnostic = Diagnostic::new(
            format!("unknown HTTP method `{word}`"),
            span,
            "expected `GET`, `POST`, `PUT`, `PATCH` or `DELETE`",
        );
        if let Some(method) = HttpMethod::from_request(&word.to_uppercase()) {
            diagnostic = diagnostic.with_help(format!(
                "methods are written in capitals: `{}`",
                method.as_str()
            ));
        }
        self.error(diagnostic);
        None
    }

    fn path(&mut self, cursor: &mut Cursor<'a>) -> Option<Text> {
        match cursor.word() {
            Some((word, span)) if word.starts_with('/') => Some(Text { value: word, span }),
            Some((word, span)) => {
                self.error(
                    Diagnostic::new(
                        format!("expected a path, found `{word}`"),
                        span,
                        "a path starts with `/`",
                    )
                    .with_help("write the path as in a browser's address bar, without quotes, such as `/hello/Ada`"),
                );
                None
            }
            None => {
                self.error(Diagnostic::new(
                    "expected a path after the method",
                    cursor.here(),
                    "such as `/hello/Ada`",
                ));
                None
            }
        }
    }

    /// `via server`, naming the `HttpServer` grant a request arrives
    /// through. `Some(None)` when there is no `via`.
    fn via(&mut self, cursor: &mut Cursor<'a>) -> Option<Option<Ident>> {
        match cursor.peek_word() {
            Some(word) if word == "via" => {}
            _ => return Some(None),
        }
        let (_, via) = cursor.word()?;
        match cursor.word() {
            Some((word, span)) if is_snake(&word) => Some(Some(Ident::new(word, span))),
            Some((word, span)) => {
                self.error(Diagnostic::new(
                    format!("expected the name of a server after `via`, found `{word}`"),
                    span,
                    "the name of an `HttpServer` grant, such as `http`, goes here",
                ));
                None
            }
            None => {
                self.error(Diagnostic::new(
                    "expected the name of a server after `via`",
                    via,
                    "nothing follows this",
                ));
                None
            }
        }
    }

    /// `with "text"`, or `with` at the end of the step and a text block
    /// below it. `Some(None)` when there is no `with`. `block_failed` says a
    /// text block was there but has already been reported.
    fn body(
        &mut self,
        cursor: &mut Cursor<'a>,
        text_block: &mut Option<Text>,
        block_failed: bool,
    ) -> Option<Option<Text>> {
        match cursor.peek_word() {
            Some(word) if word == "with" => {}
            _ => return Some(None),
        }
        let (_, with) = cursor.word()?;
        if cursor.at_end() {
            if let Some(block) = text_block.take() {
                return Some(Some(block));
            }
            if block_failed {
                return None;
            }
            self.error(
                Diagnostic::new(
                    "expected the body after `with`",
                    with,
                    "nothing follows this",
                )
                .with_help("write the body as a string, or as a text block between `\"\"\"` lines below the step"),
            );
            return None;
        }
        match cursor.string() {
            Ok(Some(text)) => Some(Some(Text {
                value: text.value,
                span: text.span,
            })),
            Ok(None) => {
                let span = cursor.here();
                self.error(
                    Diagnostic::new("expected the body as a string", span, "expected `\"`")
                        .with_help(
                            "an HTTP body is text, written as a string such as \"Hello, Ada!\"",
                        ),
                );
                None
            }
            Err(diagnostic) => {
                self.error(diagnostic);
                None
            }
        }
    }

    /// Reports anything left in the step. Returns whether it was empty.
    fn expect_end(&mut self, cursor: &mut Cursor<'a>) -> bool {
        if cursor.at_end() {
            return true;
        }
        let rest = cursor.span_of_rest();
        self.error(Diagnostic::new(
            "unexpected text at the end of the step",
            rest,
            "the step is complete before this",
        ));
        false
    }

    /// A text block whose opening `"""` is on `open`. Its lines are the ones
    /// up to a closing `"""` at the same indentation, with that indentation
    /// removed and each ending in a newline.
    fn parse_text_block(&mut self, open: Line<'a>) -> Option<Text> {
        let mut value = String::new();
        let mut short_line = None;
        while let Some(line) = self.lines.get(self.next).copied() {
            self.next += 1;
            let blank = line.text.trim().is_empty();
            if !blank && line.indent < open.indent {
                if line.indent == 0 {
                    // A new requirement: the block was never closed.
                    self.next -= 1;
                    break;
                }
                short_line.get_or_insert(Span::new(line.start, line.text_start()));
            }
            if line.indent == open.indent && line.text.trim_end() == "\"\"\"" {
                if let Some(span) = short_line {
                    self.error(
                        Diagnostic::new(
                            "text block line indented less than its `\"\"\"`",
                            span,
                            "this line starts left of the text block",
                        )
                        .with_help("indent every line of a text block at least as far as its opening `\"\"\"`"),
                    );
                    return None;
                }
                return Some(Text {
                    value,
                    span: Span::new(open.text_start(), line.end()),
                });
            }
            // The line's text after the block's indentation, kept as written.
            let raw_end = line.end();
            let from = (line.start + open.indent).min(raw_end);
            value.push_str(self.source.get(from..raw_end).unwrap_or(line.text));
            value.push('\n');
        }
        self.error(Diagnostic::new(
            "unclosed text block",
            Span::new(open.text_start(), open.text_start() + 3),
            "this `\"\"\"` has no closing `\"\"\"` at the same indentation",
        ));
        None
    }
}

/// Reads the words and strings of one step or header.
struct Cursor<'a> {
    source: &'a str,
    pos: usize,
    end: usize,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> Cursor<'a> {
    fn new(source: &'a str, span: Span) -> Cursor<'a> {
        Cursor {
            source,
            pos: span.start,
            end: span.end,
            diagnostics: Vec::new(),
        }
    }

    fn rest(&self) -> &'a str {
        &self.source[self.pos..self.end]
    }

    /// Skips spaces, line breaks and `//` comments.
    fn skip_space(&mut self) {
        loop {
            let rest = self.rest();
            let trimmed = rest.trim_start();
            self.pos += rest.len() - trimmed.len();
            if trimmed.starts_with("//") {
                let line_len = trimmed.find('\n').unwrap_or(trimmed.len());
                self.pos += line_len;
            } else {
                return;
            }
        }
    }

    fn at_end(&mut self) -> bool {
        self.skip_space();
        self.pos >= self.end
    }

    /// An empty span where the cursor is, for "expected ... here".
    fn here(&mut self) -> Span {
        self.skip_space();
        Span::new(self.pos, self.pos)
    }

    fn span_of_rest(&mut self) -> Span {
        self.skip_space();
        let text = self.rest().trim_end();
        Span::new(self.pos, self.pos + text.len())
    }

    /// The word after the next one.
    fn peek_second_word(&mut self) -> Option<String> {
        let saved = self.pos;
        self.word();
        let word = self.word().map(|(w, _)| w);
        self.pos = saved;
        word
    }

    fn peek_word(&mut self) -> Option<String> {
        let saved = self.pos;
        let word = self.word().map(|(w, _)| w);
        self.pos = saved;
        word
    }

    /// The next run of characters up to whitespace.
    fn word(&mut self) -> Option<(String, Span)> {
        self.skip_space();
        let rest = self.rest();
        let len = rest.find(char::is_whitespace).unwrap_or(rest.len());
        if len == 0 {
            return None;
        }
        let start = self.pos;
        self.pos += len;
        Some((rest[..len].to_string(), Span::new(start, self.pos)))
    }

    /// A string literal, read by the code dialect's lexer so escapes are
    /// Rust's. `Ok(None)` when the next character is not `"`.
    fn string(&mut self) -> Result<Option<StrLit>, Diagnostic> {
        self.skip_space();
        let rest = self.rest();
        if !rest.starts_with('"') {
            return Ok(None);
        }
        let mut escaped = false;
        let close = rest.char_indices().skip(1).find(|&(_, c)| {
            let found = c == '"' && !escaped;
            escaped = c == '\\' && !escaped;
            found
        });
        let start = self.pos;
        let Some((close, _)) = close else {
            return Err(Diagnostic::new(
                "unterminated string",
                Span::new(start, start + 1),
                "this string has no closing `\"`",
            ));
        };
        let literal = &rest[..=close];
        self.pos += literal.len();
        let lexed = lexer::lex(literal);
        self.diagnostics
            .extend(lexed.diagnostics.into_iter().map(|mut d| {
                d.span = Span::new(d.span.start + start, d.span.end + start);
                d
            }));
        let value = match lexed.tokens.into_iter().next().map(|t| t.kind) {
            Some(TokenKind::Str(value)) => value,
            _ => String::new(),
        };
        Ok(Some(StrLit {
            value,
            span: Span::new(start, self.pos),
        }))
    }
}

/// How the spans of an examples row's text, which has the row's cells in
/// place of its placeholders, map back to the file.
pub(crate) struct RowMap {
    /// Where the row's text first differs from the file. Before it, the two
    /// are the same.
    from: usize,
    /// The row's text from `from` on, in order: text copied from the file,
    /// and cells.
    segments: Vec<Segment>,
}

struct Segment {
    /// Where it is in the row's text.
    text: (usize, usize),
    /// Where it starts in the file.
    file: usize,
    /// For a cell, the placeholder it replaced.
    placeholder: Option<Span>,
}

impl RowMap {
    /// A span of the row's text as a span of the file. A part of a span in
    /// a cell maps to the cell's placeholder.
    pub(crate) fn span(&self, span: Span) -> Span {
        if span.end <= self.from {
            return span;
        }
        let start = self.point(span.start, false);
        let end = if span.end == span.start {
            start
        } else {
            self.point(span.end, true)
        };
        Span::new(start, end.max(start))
    }

    fn point(&self, pos: usize, end: bool) -> usize {
        if pos < self.from || (pos == self.from && !end) {
            return pos;
        }
        let segment = self.segments.iter().find(|s| {
            if end {
                s.text.0 < pos && pos <= s.text.1
            } else {
                s.text.0 <= pos && pos < s.text.1
            }
        });
        let Some(segment) = segment.or(self.segments.last()) else {
            return pos;
        };
        match segment.placeholder {
            Some(p) if end => p.end,
            Some(p) => p.start,
            None => segment.file + pos.min(segment.text.1).saturating_sub(segment.text.0),
        }
    }

    /// Maps the spans of a step read from the row's text, but not those of
    /// a fact or check, which were read with the file's spans already.
    fn step(&self, step: &mut Step) {
        step.span = self.span(step.span);
        let ident = |i: &mut Ident| i.span = self.span(i.span);
        let text = |t: &mut Text| t.span = self.span(t.span);
        match &mut step.kind {
            StepKind::Request {
                app,
                path,
                via,
                body,
                ..
            } => {
                ident(app);
                text(path);
                if let Some(via) = via {
                    ident(via);
                }
                if let Some(body) = body {
                    text(body);
                }
            }
            StepKind::Reply { app, status, body } => {
                ident(app);
                ident(status);
                if let Some(body) = body {
                    text(body);
                }
            }
            StepKind::Fact { .. } | StepKind::Check(_) => {}
        }
    }
}

/// What a step does, whether written with its own word or with `and`.
#[derive(Clone, Copy)]
enum Kind {
    Given,
    When,
    Then,
}

/// Where the first placeholder in a step is, such as `<on hand>`: a name
/// in `<` `>` that starts with a letter, made of letters, digits, spaces,
/// `_` and `-`, so an operator such as `a < b && c > d` is not one.
fn placeholder(text: &str) -> Option<(usize, usize)> {
    let mut from = 0;
    while let Some(open) = text[from..].find('<').map(|i| from + i) {
        let rest = &text[open + 1..];
        if let Some(close) = rest.find('>') {
            let inner = &rest[..close];
            let named = inner.starts_with(|c: char| c.is_alphabetic())
                && inner
                    .chars()
                    .all(|c| c.is_alphanumeric() || matches!(c, ' ' | '_' | '-'));
            if named {
                return Some((open, open + close + 2));
            }
        }
        from = open + 1;
    }
    None
}

/// `1 cell`, `2 cells`.
fn count(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

fn is_snake(word: &str) -> bool {
    word.starts_with(|c: char| c.is_ascii_lowercase() || c == '_')
        && word
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn is_upper_camel(word: &str) -> bool {
    word.starts_with(|c: char| c.is_ascii_uppercase())
        && word.chars().all(|c| c.is_ascii_alphanumeric())
}
