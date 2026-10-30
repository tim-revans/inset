//! The code dialect's lexer, which follows Rust's lexical rules.

use std::fmt;

use crate::{Diagnostic, Span};

#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    Ident(String),
    Keyword(Keyword),
    /// An integer, as written, with any suffix such as `u16`.
    Int(String),
    /// A floating-point number, as written.
    Float(String),
    /// A string literal's value, with escapes resolved.
    Str(String),
    /// A character literal's value.
    Char(char),
    /// The text of a `///` comment, without the slashes.
    Doc(String),
    Punct(Punct),
    Eof,
}

impl fmt::Display for TokenKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TokenKind::Ident(name) => write!(f, "`{name}`"),
            TokenKind::Keyword(kw) => write!(f, "keyword `{}`", kw.as_str()),
            TokenKind::Int(text) | TokenKind::Float(text) => write!(f, "`{text}`"),
            TokenKind::Str(_) => f.write_str("a string"),
            TokenKind::Char(_) => f.write_str("a character"),
            TokenKind::Doc(_) => f.write_str("a doc comment"),
            TokenKind::Punct(p) => write!(f, "`{}`", p.as_str()),
            TokenKind::Eof => f.write_str("the end of the file"),
        }
    }
}

macro_rules! table {
    ($(#[$meta:meta])* $name:ident { $($variant:ident = $text:literal,)* }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum $name {
            $($variant,)*
        }

        impl $name {
            pub fn as_str(self) -> &'static str {
                match self {
                    $($name::$variant => $text,)*
                }
            }

            const ALL: &[$name] = &[$($name::$variant,)*];
        }
    };
}

table! {
    /// Rust's strict keywords and the ones Inset adds. None can be a name.
    Keyword {
        As = "as",
        Async = "async",
        Await = "await",
        Break = "break",
        Const = "const",
        Continue = "continue",
        Crate = "crate",
        Dyn = "dyn",
        Else = "else",
        Enum = "enum",
        Extern = "extern",
        False = "false",
        Fn = "fn",
        For = "for",
        If = "if",
        Impl = "impl",
        In = "in",
        Let = "let",
        Loop = "loop",
        Match = "match",
        Mod = "mod",
        Move = "move",
        Mut = "mut",
        Pub = "pub",
        Ref = "ref",
        Return = "return",
        SelfValue = "self",
        SelfType = "Self",
        Static = "static",
        Struct = "struct",
        Super = "super",
        Trait = "trait",
        True = "true",
        Type = "type",
        Unsafe = "unsafe",
        Use = "use",
        Where = "where",
        While = "while",
        App = "app",
        Component = "component",
        Setting = "setting",
        Grant = "grant",
        Start = "start",
        Send = "send",
        Fork = "fork",
        Failure = "failure",
        Rollback = "rollback",
        Restart = "restart",
    }
}

table! {
    /// Punctuation, longest first so the lexer takes the longest match.
    Punct {
        ShlEq = "<<=",
        ShrEq = ">>=",
        DotDotEq = "..=",
        DotDot = "..",
        PathSep = "::",
        Arrow = "->",
        FatArrow = "=>",
        EqEq = "==",
        Ne = "!=",
        Le = "<=",
        Ge = ">=",
        AndAnd = "&&",
        OrOr = "||",
        Shl = "<<",
        Shr = ">>",
        PlusEq = "+=",
        MinusEq = "-=",
        StarEq = "*=",
        SlashEq = "/=",
        PercentEq = "%=",
        CaretEq = "^=",
        AndEq = "&=",
        OrEq = "|=",
        LBrace = "{",
        RBrace = "}",
        LParen = "(",
        RParen = ")",
        LBracket = "[",
        RBracket = "]",
        Comma = ",",
        Semi = ";",
        Colon = ":",
        Dot = ".",
        Eq = "=",
        Lt = "<",
        Gt = ">",
        Plus = "+",
        Minus = "-",
        Star = "*",
        Slash = "/",
        Percent = "%",
        Not = "!",
        And = "&",
        Or = "|",
        Caret = "^",
        Question = "?",
        Pound = "#",
        At = "@",
        Underscore = "_",
    }
}

impl Keyword {
    /// Whether Inset adds the keyword, so a name that is fine in Rust is not.
    pub fn is_inset_only(self) -> bool {
        use Keyword::*;
        matches!(
            self,
            App | Component | Setting | Grant | Start | Send | Fork | Failure | Rollback | Restart
        )
    }

    fn from_str(text: &str) -> Option<Keyword> {
        Keyword::ALL.iter().copied().find(|k| k.as_str() == text)
    }
}

pub struct Lexed {
    pub tokens: Vec<Token>,
    pub diagnostics: Vec<Diagnostic>,
    /// Where an unterminated string or comment starts. Every token after it
    /// is out of step with what the author meant, so errors the parser finds
    /// from there on are not worth reporting.
    pub cutoff: Option<usize>,
}

/// Splits code into tokens, ending with [`TokenKind::Eof`]. Comments other
/// than `///` are dropped. A character that starts no token is reported and
/// skipped, so one bad character gives one diagnostic.
pub fn lex(source: &str) -> Lexed {
    let mut lexer = Lexer {
        src: source,
        pos: 0,
        tokens: Vec::new(),
        diagnostics: Vec::new(),
        cutoff: None,
    };
    lexer.run();
    Lexed {
        tokens: lexer.tokens,
        diagnostics: lexer.diagnostics,
        cutoff: lexer.cutoff,
    }
}

struct Lexer<'a> {
    src: &'a str,
    pos: usize,
    tokens: Vec<Token>,
    diagnostics: Vec<Diagnostic>,
    cutoff: Option<usize>,
}

impl Lexer<'_> {
    fn rest(&self) -> &str {
        &self.src[self.pos..]
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn peek_nth(&self, n: usize) -> Option<char> {
        self.rest().chars().nth(n)
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn eat_while(&mut self, f: impl Fn(char) -> bool) {
        while self.peek().is_some_and(&f) {
            self.bump();
        }
    }

    fn push(&mut self, kind: TokenKind, start: usize) {
        self.tokens.push(Token {
            kind,
            span: Span::new(start, self.pos),
        });
    }

    fn error(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    fn run(&mut self) {
        while let Some(c) = self.peek() {
            let start = self.pos;
            if c.is_whitespace() {
                self.bump();
            } else if self.rest().starts_with("//") {
                self.line_comment(start);
            } else if self.rest().starts_with("/*") {
                self.block_comment(start);
            } else if c == 'r' && matches!(self.peek_nth(1), Some('"' | '#')) {
                self.raw_string(start);
            } else if c == 'b' && matches!(self.peek_nth(1), Some('"' | '\'')) {
                self.bump();
                self.error(
                    Diagnostic::new(
                        "byte literals are not supported",
                        Span::new(start, self.pos),
                        "remove this `b`",
                    )
                    .with_help("Inset text is `String` and `&str`"),
                );
            } else if c == '_' || unicode_ident_start(c) {
                self.ident(start);
            } else if c.is_ascii_digit() {
                self.number(start);
            } else if c == '"' {
                self.string(start);
            } else if c == '\'' {
                self.char_or_lifetime(start);
            } else if let Some(p) = Punct::ALL
                .iter()
                .copied()
                .find(|p| self.rest().starts_with(p.as_str()))
            {
                self.pos += p.as_str().len();
                self.push(TokenKind::Punct(p), start);
            } else {
                self.bump();
                self.error(Diagnostic::new(
                    format!("unexpected character `{}`", c.escape_debug()),
                    Span::new(start, self.pos),
                    "Inset has no token that starts with this",
                ));
            }
        }
        let end = self.src.len();
        self.tokens.push(Token {
            kind: TokenKind::Eof,
            span: Span::new(end, end),
        });
    }

    fn line_comment(&mut self, start: usize) {
        self.eat_while(|c| c != '\n');
        let text = &self.src[start..self.pos];
        // As in Rust, exactly three slashes make a doc comment; four or more do not.
        if let Some(doc) = text.strip_prefix("///")
            && !doc.starts_with('/')
        {
            let doc = doc.strip_prefix(' ').unwrap_or(doc).trim_end();
            self.push(TokenKind::Doc(doc.to_string()), start);
        }
    }

    fn block_comment(&mut self, start: usize) {
        self.pos += 2;
        let mut depth = 1;
        while depth > 0 {
            if self.rest().starts_with("/*") {
                self.pos += 2;
                depth += 1;
            } else if self.rest().starts_with("*/") {
                self.pos += 2;
                depth -= 1;
            } else if self.bump().is_none() {
                self.cutoff.get_or_insert(start);
                self.error(Diagnostic::new(
                    "unterminated block comment",
                    Span::new(start, start + 2),
                    "this comment has no closing `*/`",
                ));
                return;
            }
        }
    }

    fn ident(&mut self, start: usize) {
        self.eat_while(unicode_ident_continue);
        let text = &self.src[start..self.pos];
        let kind = if text == "_" {
            TokenKind::Punct(Punct::Underscore)
        } else if let Some(kw) = Keyword::from_str(text) {
            TokenKind::Keyword(kw)
        } else {
            TokenKind::Ident(text.to_string())
        };
        self.push(kind, start);
    }

    fn number(&mut self, start: usize) {
        if ["0x", "0o", "0b"]
            .iter()
            .any(|p| self.rest().starts_with(p))
        {
            self.pos += 2;
            // Digits beyond the radix are left for rustc to reject.
            self.eat_while(|c| c.is_ascii_hexdigit() || c == '_');
            self.suffix();
            let text = self.src[start..self.pos].to_string();
            self.push(TokenKind::Int(text), start);
            return;
        }
        self.eat_while(|c| c.is_ascii_digit() || c == '_');
        let mut float = false;
        // `1.5` is a float, but `1..5` is a range and `x.0.len()` a field.
        if self.peek() == Some('.') && self.peek_nth(1).is_some_and(|c| c.is_ascii_digit()) {
            float = true;
            self.bump();
            self.eat_while(|c| c.is_ascii_digit() || c == '_');
        }
        if matches!(self.peek(), Some('e' | 'E')) {
            let sign = usize::from(matches!(self.peek_nth(1), Some('+' | '-')));
            if self.peek_nth(1 + sign).is_some_and(|c| c.is_ascii_digit()) {
                float = true;
                self.pos += 1 + sign;
                self.eat_while(|c| c.is_ascii_digit() || c == '_');
            }
        }
        self.suffix();
        let text = self.src[start..self.pos].to_string();
        let kind = if float || text.ends_with("f32") || text.ends_with("f64") {
            TokenKind::Float(text)
        } else {
            TokenKind::Int(text)
        };
        self.push(kind, start);
    }

    fn suffix(&mut self) {
        if self.peek().is_some_and(unicode_ident_start) {
            self.eat_while(unicode_ident_continue);
        }
    }

    fn string(&mut self, start: usize) {
        self.bump();
        let mut value = String::new();
        loop {
            let at = self.pos;
            match self.bump() {
                None => {
                    self.unterminated_string(start);
                    break;
                }
                Some('"') => break,
                Some('\\') => {
                    if let Some(c) = self.escape(at, true) {
                        value.push(c);
                    }
                }
                Some(c) => value.push(c),
            }
        }
        self.push(TokenKind::Str(value), start);
    }

    /// Reports a string that runs to the end of the file. Strings may span
    /// lines, so a string missing its closing `"` usually ends at the opening
    /// `"` of a later one instead, and this one is what is left over. The
    /// first string that spans lines is then the likely culprit.
    fn unterminated_string(&mut self, start: usize) {
        let culprit = self.tokens.iter().map(|t| t.span).find(|span| {
            let text = &self.src[span.range()];
            text.starts_with('"') && text.contains('\n')
        });
        let diagnostic = match culprit {
            Some(span) => {
                let line = self.src[..span.end].matches('\n').count() + 1;
                Diagnostic::new(
                    "unterminated string",
                    Span::new(span.start, span.start + 1),
                    format!("this string runs on to line {line}, which puts every string after it out of step"),
                )
                .with_help("if the string should end on this line, add its closing `\"`")
            }
            None => Diagnostic::new(
                "unterminated string",
                Span::new(start, start + 1),
                "this string has no closing `\"`",
            ),
        };
        self.cutoff
            .get_or_insert(culprit.map_or(start, |s| s.start));
        self.error(diagnostic);
    }

    /// Reads the escape after a `\` at `at`. Returns `None` for a line
    /// continuation, which stands for no character, and for a bad escape,
    /// which is reported.
    fn escape(&mut self, at: usize, in_string: bool) -> Option<char> {
        let c = self.bump();
        let resolved = match c {
            Some('n') => Some('\n'),
            Some('r') => Some('\r'),
            Some('t') => Some('\t'),
            Some('\\') => Some('\\'),
            Some('0') => Some('\0'),
            Some('\'') => Some('\''),
            Some('"') => Some('"'),
            Some('\n') if in_string => {
                self.eat_while(char::is_whitespace);
                return None;
            }
            Some('x') => {
                let digits = self.rest().get(..2).unwrap_or("");
                match u8::from_str_radix(digits, 16) {
                    Ok(b) if b < 0x80 && digits.chars().all(|c| c.is_ascii_hexdigit()) => {
                        self.pos += 2;
                        Some(char::from(b))
                    }
                    _ => None,
                }
            }
            Some('u') if self.peek() == Some('{') => {
                self.bump();
                let digits_start = self.pos;
                self.eat_while(|c| c.is_ascii_hexdigit() || c == '_');
                let digits = self.src[digits_start..self.pos].replace('_', "");
                let closed = self.peek() == Some('}');
                if closed {
                    self.bump();
                }
                u32::from_str_radix(&digits, 16)
                    .ok()
                    .and_then(char::from_u32)
                    .filter(|_| closed)
            }
            _ => None,
        };
        if resolved.is_none() {
            self.error(
                Diagnostic::new(
                    "unknown escape in a string",
                    Span::new(at, self.pos),
                    "this is not an escape Rust knows",
                )
                .with_help(
                    r#"the escapes are `\n`, `\r`, `\t`, `\\`, `\0`, `\'`, `\"`, `\x7F` and `\u{7FFF}`"#,
                ),
            );
        }
        resolved
    }

    fn raw_string(&mut self, start: usize) {
        self.bump();
        let hashes = self.rest().chars().take_while(|&c| c == '#').count();
        self.pos += hashes;
        if self.peek() != Some('"') {
            // `r#name` is a raw identifier in Rust. Inset has no use for it.
            self.error(Diagnostic::new(
                "expected `\"` to start a raw string",
                Span::new(start, self.pos),
                "a raw string is written `r\"...\"` or `r#\"...\"#`",
            ));
            return;
        }
        self.bump();
        let closing = format!("\"{}", "#".repeat(hashes));
        match self.rest().find(&closing) {
            Some(len) => {
                let value = self.rest()[..len].to_string();
                self.pos += len + closing.len();
                self.push(TokenKind::Str(value), start);
            }
            None => {
                self.pos = self.src.len();
                self.cutoff.get_or_insert(start);
                self.error(Diagnostic::new(
                    "unterminated raw string",
                    Span::new(start, start + 1),
                    format!("this raw string has no closing `{closing}`"),
                ));
            }
        }
    }

    fn char_or_lifetime(&mut self, start: usize) {
        self.bump();
        // `'a` with no closing quote is a lifetime.
        if self.peek().is_some_and(unicode_ident_start) && self.peek_nth(1) != Some('\'') {
            self.eat_while(unicode_ident_continue);
            self.error(
                Diagnostic::new(
                    "lifetimes are not supported yet",
                    Span::new(start, self.pos),
                    "the parser does not handle lifetimes yet",
                )
                .with_help("borrow without naming the lifetime, or use an owned type"),
            );
            return;
        }
        let at = self.pos;
        let value = match self.bump() {
            Some('\\') => self.escape(at, false),
            Some('\'') | Some('\n') | None => {
                self.error(Diagnostic::new(
                    "empty or unterminated character literal",
                    Span::new(start, self.pos),
                    "a character literal holds one character, such as `'a'`",
                ));
                return;
            }
            Some(c) => Some(c),
        };
        if self.peek() == Some('\'') {
            self.bump();
            if let Some(c) = value {
                self.push(TokenKind::Char(c), start);
            }
        } else {
            self.eat_while(|c| c != '\'' && c != '\n');
            if self.peek() == Some('\'') {
                self.bump();
            }
            self.error(
                Diagnostic::new(
                    "character literal holds more than one character",
                    Span::new(start, self.pos),
                    "a character literal holds one character",
                )
                .with_help("write text in double quotes, as a string"),
            );
        }
    }
}

// Rust identifiers are Unicode XID. Alphabetic and alphanumeric differ from it
// only in rare characters, which rustc rejects when it builds the program.
fn unicode_ident_start(c: char) -> bool {
    c == '_' || c.is_alphabetic()
}

fn unicode_ident_continue(c: char) -> bool {
    c == '_' || c.is_alphanumeric()
}
