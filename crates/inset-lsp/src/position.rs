//! Converts between byte offsets, which spans use, and LSP positions, which
//! count a line's characters in the encoding agreed with the client.

use inset_syntax::Span;
use lsp_types::{Position, PositionEncodingKind, Range};

/// How a position counts the characters of a line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Encoding {
    /// In UTF-16 code units, which every client supports.
    Utf16,
    /// In bytes.
    Utf8,
}

impl Encoding {
    /// UTF-8 if the client offers it, as it needs no conversion, and UTF-16
    /// otherwise.
    pub(crate) fn negotiate(offered: Option<&[PositionEncodingKind]>) -> Encoding {
        match offered {
            Some(kinds) if kinds.contains(&PositionEncodingKind::UTF8) => Encoding::Utf8,
            _ => Encoding::Utf16,
        }
    }

    pub(crate) fn kind(self) -> PositionEncodingKind {
        match self {
            Encoding::Utf16 => PositionEncodingKind::UTF16,
            Encoding::Utf8 => PositionEncodingKind::UTF8,
        }
    }

    fn width(self, c: char) -> usize {
        match self {
            Encoding::Utf16 => c.len_utf16(),
            Encoding::Utf8 => c.len_utf8(),
        }
    }
}

/// Where each line of a text starts.
pub(crate) struct LineIndex<'a> {
    text: &'a str,
    starts: Vec<usize>,
    encoding: Encoding,
}

impl<'a> LineIndex<'a> {
    pub(crate) fn new(text: &'a str, encoding: Encoding) -> LineIndex<'a> {
        let starts = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        LineIndex {
            text,
            starts,
            encoding,
        }
    }

    pub(crate) fn position(&self, offset: usize) -> Position {
        let offset = floor_char_boundary(self.text, offset);
        let line = self.starts.partition_point(|&s| s <= offset) - 1;
        let character = self.text[self.starts[line]..offset]
            .chars()
            .map(|c| self.encoding.width(c))
            .sum::<usize>();
        Position::new(line as u32, character as u32)
    }

    pub(crate) fn range(&self, span: Span) -> Range {
        Range::new(self.position(span.start), self.position(span.end))
    }

    /// The byte offset of a position. A position past the end of its line
    /// is the line's end, and one past the last line is the end of the text.
    pub(crate) fn offset(&self, position: Position) -> usize {
        let Some(&start) = self.starts.get(position.line as usize) else {
            return self.text.len();
        };
        let line = &self.text[start..];
        let line = &line[..line.find('\n').unwrap_or(line.len())];
        let mut units = 0;
        for (i, c) in line.char_indices() {
            if units >= position.character as usize {
                return start + i;
            }
            units += self.encoding.width(c);
        }
        start + line.len()
    }
}

fn floor_char_boundary(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_utf16_units_or_bytes() {
        let text = "a\n€𝄞x\n";
        let utf16 = LineIndex::new(text, Encoding::Utf16);
        let utf8 = LineIndex::new(text, Encoding::Utf8);
        let x = text.find('x').unwrap();
        assert_eq!(utf16.position(x), Position::new(1, 3));
        assert_eq!(utf8.position(x), Position::new(1, 7));
        assert_eq!(utf16.offset(Position::new(1, 3)), x);
        assert_eq!(utf8.offset(Position::new(1, 7)), x);
        assert_eq!(utf16.offset(Position::new(1, 99)), x + 1);
        assert_eq!(utf16.offset(Position::new(9, 0)), text.len());
        assert_eq!(utf16.position(text.len()), Position::new(2, 0));
    }
}
