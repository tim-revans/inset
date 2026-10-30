//! The requirements dialect's parser.
//!
//! Requirements are laid out by lines, so this parser works a line at a time:
//! a requirement starts at the left margin, each step is indented by four
//! spaces, and a step continues onto lines indented further. Within a step,
//! words are separated by spaces, and strings follow Rust's rules. An error
//! in one step is reported and the parser goes on with the next.

use crate::ast::*;
use crate::lexer::{self, TokenKind};
use crate::{Diagnostic, Ident, Span};

const STEP_INDENT: usize = 4;

pub(crate) fn parse(source: &str) -> (RequirementsFile, Vec<Diagnostic>) {
    let mut parser = ReqParser {
        source,
        lines: lines(source),
        next: 0,
        diagnostics: Vec::new(),
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
            if let Some(step) = self.parse_step(line, previous) {
                steps.push(step);
            }
        }
        let sentence = sentence?;
        if !any_step_lines {
            self.error(Diagnostic::new(
                "requirement has no steps",
                sentence.span,
                "this requirement says nothing to test",
            ));
        }
        let end = steps.last().map_or(sentence.span, |s| s.span);
        Some(Requirement {
            span: Span::new(header.text_start(), end.end),
            sentence,
            steps,
        })
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
        let (keyword, request) = match word.as_str() {
            "when" => (StepKeyword::When, true),
            "then" => (StepKeyword::Then, false),
            "and" => match previous {
                Some(StepKind::Request { .. }) => (StepKeyword::And, true),
                Some(StepKind::Reply { .. }) => (StepKeyword::And, false),
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
            "given" | "examples" => {
                self.unsupported(&format!("`{word}`"), word_span);
                return None;
            }
            "for" => {
                self.unsupported("`for all`", word_span);
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
        let app = self.app_name(cursor)?;
        let kind = if request {
            self.expect_word(cursor, "receives", "after the app's name in a `when` step")?;
            let method = self.method(cursor)?;
            let path = self.path(cursor)?;
            let body = self.body(cursor, text_block, block_failed)?;
            StepKind::Request {
                app,
                method,
                path,
                body,
            }
        } else {
            match cursor.peek_word() {
                Some(word) if word == "replies" => {}
                _ => {
                    self.unsupported(
                        "a fact or check in a `then` step",
                        word_span.to(cursor.span_of_rest()),
                    );
                    return None;
                }
            }
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
            Some(word) if word == "via" => {
                let span = cursor.here();
                self.unsupported("`via`", span);
                return None;
            }
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

fn is_upper_camel(word: &str) -> bool {
    word.starts_with(|c: char| c.is_ascii_uppercase())
        && word.chars().all(|c| c.is_ascii_alphanumeric())
}
