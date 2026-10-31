use std::fmt;
use std::ops::Range;

/// A range of bytes in one source file. Both ends lie on character boundaries.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Span {
        Span { start, end }
    }

    /// The span from the start of `self` to the end of `other`.
    pub fn to(self, other: Span) -> Span {
        Span::new(self.start, other.end.max(self.start))
    }

    pub fn range(self) -> Range<usize> {
        self.start..self.end
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }

    /// The span moved on by `offset` bytes.
    pub fn shifted(self, offset: usize) -> Span {
        Span::new(self.start + offset, self.end + offset)
    }

    /// The span moved back by `offset` bytes, which it must start after.
    pub fn unshifted(self, offset: usize) -> Span {
        Span::new(self.start - offset, self.end - offset)
    }
}

// Short, so syntax tree snapshots stay readable.
impl fmt::Debug for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..{}", self.start, self.end)
    }
}

/// A name in the source, with where it was written.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

impl Ident {
    pub fn new(name: impl Into<String>, span: Span) -> Ident {
        Ident {
            name: name.into(),
            span,
        }
    }
}

impl fmt::Debug for Ident {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?} @ {:?}", self.name, self.span)
    }
}
