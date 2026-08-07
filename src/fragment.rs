//! A slice of the source text that knows its own absolute byte span.

use crate::span::{Span, Spanned};

/// A slice of the source text that knows its absolute byte span.
///
/// The `start..start + len` range is always a valid `char`-boundary range of `source`;
/// this is guaranteed by construction, because the only entry point is [`Fragment::lines`]
/// and every other method derives its range from `str` operations on the current text.
/// A method that takes a raw index returns [`Option`], so a mid-codepoint offset is not
/// merely avoided but inexpressible.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Fragment<'a> {
    /// The whole source text, not just this fragment's slice of it, so that the
    /// fragment can always check its own span against the text it claims to cover.
    source: &'a str,
    start: usize,
    len: usize,
}

impl<'a> Fragment<'a> {
    /// Splits `source` into lines on `\n`, `\r\n` and a lone `\r`, excluding the terminator.
    ///
    /// An empty source yields no fragments; a source with no trailing newline yields a final
    /// fragment for the unterminated text. Concatenating every fragment's text with the
    /// terminator that followed it reconstructs `source` exactly.
    pub(crate) fn lines(source: &'a str) -> impl Iterator<Item = Fragment<'a>> {
        let mut offset = 0usize;
        std::iter::from_fn(move || {
            let rest = source.get(offset..)?;
            if rest.is_empty() {
                return None;
            }
            let (len, terminator) = match rest.find(['\n', '\r']) {
                Some(index) => {
                    let bytes = rest.as_bytes();
                    let is_crlf =
                        bytes.get(index) == Some(&b'\r') && bytes.get(index + 1) == Some(&b'\n');
                    (index, if is_crlf { 2 } else { 1 })
                }
                None => (rest.len(), 0),
            };
            let line = Fragment {
                source,
                start: offset,
                len,
            };
            offset += len + terminator;
            Some(line)
        })
    }

    /// The text this fragment covers.
    pub(crate) fn text(self) -> &'a str {
        self.span().slice(self.source).unwrap_or_default()
    }

    /// Where this fragment sits in the source.
    pub(crate) fn span(self) -> Span {
        Span::new(self.start, self.len)
    }

    /// Whether the fragment is empty or contains only whitespace.
    pub(crate) fn is_blank(self) -> bool {
        self.text().trim().is_empty()
    }

    /// The number of leading whitespace **bytes**, which is what a span is measured in.
    pub(crate) fn indent(self) -> usize {
        let text = self.text();
        // `trim_start` returns a suffix of `text`, so this difference cannot underflow.
        text.len() - text.trim_start().len()
    }

    /// The fragment with leading and trailing whitespace removed, span included.
    ///
    /// Offsets come from the lengths of the trimmed strings, so non-ASCII whitespace
    /// shortens the span by its byte length rather than by a character count.
    pub(crate) fn trim(self) -> Self {
        let text = self.text();
        let without_leading = text.trim_start();
        let leading = text.len() - without_leading.len();
        let trimmed = without_leading.trim_end();
        Self {
            source: self.source,
            start: self.start + leading,
            len: trimmed.len(),
        }
    }

    /// The sub-fragment covering `range`.
    ///
    /// `range` is relative to [`Fragment::text`]. Returns `None` when it is not a valid
    /// sub-slice: reversed, out of range, or not on `char` boundaries.
    pub(crate) fn subrange(self, range: std::ops::Range<usize>) -> Option<Self> {
        let std::ops::Range { start, end } = range;
        let text = self.text().get(start..end)?;
        Some(Self {
            source: self.source,
            start: self.start + start,
            len: text.len(),
        })
    }

    /// The fragment's text as an owned value that remembers where it came from.
    pub(crate) fn to_spanned(self) -> Spanned<String> {
        let span = self.span();
        debug_assert_eq!(
            span.slice(self.source),
            Some(self.text()),
            "fragment span and text disagree"
        );
        Spanned::from_source(span, self.text().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::Fragment;
    use crate::span::{Span, Spanned};

    fn texts(source: &str) -> Vec<&str> {
        Fragment::lines(source).map(Fragment::text).collect()
    }

    fn spans(source: &str) -> Vec<std::ops::Range<usize>> {
        Fragment::lines(source)
            .map(|line| line.span().into())
            .collect()
    }

    /// The one fragment covering all of `source`, as the lexer sees a single line.
    fn whole(source: &str) -> Fragment<'_> {
        let mut lines = Fragment::lines(source);
        let line = lines.next().unwrap();
        assert!(lines.next().is_none(), "fixture must be a single line");
        line
    }

    #[test]
    fn lines_of_empty_input() {
        assert!(texts("").is_empty());
    }

    #[test]
    fn lines_without_a_trailing_newline() {
        assert_eq!(texts("a\nb"), vec!["a", "b"]);
        assert_eq!(spans("a\nb"), vec![0..1, 2..3]);
    }

    #[test]
    fn lines_with_a_trailing_newline() {
        assert_eq!(texts("a\nb\n"), vec!["a", "b"]);
        assert_eq!(spans("a\nb\n"), vec![0..1, 2..3]);
    }

    #[test]
    fn a_lone_newline_is_one_empty_line() {
        assert_eq!(texts("\n"), vec![""]);
        assert_eq!(spans("\n"), vec![0..0]);
    }

    #[test]
    fn consecutive_newlines_yield_empty_lines() {
        assert_eq!(texts("a\n\n\nb\n"), vec!["a", "", "", "b"]);
        assert_eq!(spans("a\n\n\nb\n"), vec![0..1, 2..2, 3..3, 4..5]);
    }

    #[test]
    fn crlf_terminators_are_excluded() {
        assert_eq!(texts("a\r\nb\r\n"), vec!["a", "b"]);
        assert_eq!(spans("a\r\nb\r\n"), vec![0..1, 3..4]);
    }

    #[test]
    fn a_lone_carriage_return_terminates_a_line() {
        assert_eq!(texts("a\rb\r"), vec!["a", "b"]);
        assert_eq!(spans("a\rb\r"), vec![0..1, 2..3]);
    }

    #[test]
    fn a_carriage_return_before_a_blank_line_is_not_crlf() {
        // `\r\r\n` is a lone `\r` terminating an empty line, then a CRLF.
        assert_eq!(texts("a\r\r\nb"), vec!["a", "", "b"]);
        assert_eq!(spans("a\r\r\nb"), vec![0..1, 2..2, 4..5]);
    }

    #[test]
    fn multi_byte_characters_do_not_shift_spans() {
        let source = "café\ngröße\n";
        assert_eq!(texts(source), vec!["café", "größe"]);
        assert_eq!(spans(source), vec![0..5, 6..13]);
        for line in Fragment::lines(source) {
            assert_eq!(line.span().slice(source), Some(line.text()));
        }
    }

    #[test]
    fn the_iterator_is_fused() {
        let mut lines = Fragment::lines("a\n");
        assert!(lines.next().is_some());
        assert!(lines.next().is_none());
        assert!(lines.next().is_none());
    }

    /// Every fragment agrees with its span, and the fragments plus their terminators
    /// are exactly the source — over a fixture mixing all three terminators with
    /// multi-byte text, non-ASCII whitespace and blank lines.
    #[test]
    fn lines_reconstruct_the_source() {
        let source = "café = valeur\r\n[größe]\r\n\u{3000}clé\u{a0}: ✓\rx\n\ny = 日本語\r\ntail";
        let lines: Vec<_> = Fragment::lines(source).collect();

        let mut rebuilt = String::new();
        let mut cursor = 0usize;
        for (index, line) in lines.iter().enumerate() {
            assert_eq!(line.span().start(), cursor, "fragment must start at cursor");
            assert_eq!(
                line.span().slice(source),
                Some(line.text()),
                "span and text must agree"
            );
            rebuilt.push_str(line.text());
            let next_start = lines
                .get(index + 1)
                .map_or(source.len(), |next| next.span().start());
            let terminator = &source[line.span().end()..next_start];
            assert!(
                matches!(terminator, "" | "\n" | "\r" | "\r\n"),
                "gap between fragments must be a line terminator, got {terminator:?}"
            );
            rebuilt.push_str(terminator);
            cursor = next_start;
        }

        assert_eq!(rebuilt, source);
        assert_eq!(cursor, source.len());
    }

    #[test]
    fn is_blank_covers_whitespace_only_lines() {
        assert!(whole("\n").is_blank(), "an empty line is blank");
        assert!(whole("   \t ").is_blank());
        assert!(whole("\u{a0}\u{3000}").is_blank());
        assert!(!whole("  x  ").is_blank());
    }

    #[test]
    fn indent_counts_bytes_not_characters() {
        assert_eq!(whole("  x").indent(), 2);
        assert_eq!(whole("x").indent(), 0);
        assert_eq!(whole("   ").indent(), 3);
        // U+00A0 is two bytes, U+3000 is three.
        assert_eq!(whole("\u{a0}\u{3000}x").indent(), 5);
    }

    #[test]
    fn trim_removes_ascii_whitespace() {
        let line = whole("  key = value  ").trim();
        assert_eq!(line.text(), "key = value");
        assert_eq!(line.span(), Span::new(2, 11));
    }

    /// The exact input that panics today: non-ASCII whitespace on both sides, where a
    /// character count and a byte count disagree.
    #[test]
    fn trim_removes_non_ascii_whitespace() {
        let source = "\u{a0}\u{3000}k = 1\u{3000}\u{a0}";
        let line = whole(source).trim();
        assert_eq!(line.text(), "k = 1");
        assert_eq!(line.span(), Span::new(5, 5));
        assert_eq!(line.span().slice(source), Some("k = 1"));
    }

    #[test]
    fn trim_of_a_blank_line_is_an_empty_span_inside_it() {
        let line = whole("   ").trim();
        assert_eq!(line.text(), "");
        assert!(line.span().is_empty());
        assert!(line.span().start() <= line.span().end());
    }

    #[test]
    fn trim_of_an_empty_line_keeps_its_position() {
        let source = "a\n\nb";
        let blank = Fragment::lines(source).nth(1).unwrap().trim();
        assert_eq!(blank.text(), "");
        assert_eq!(blank.span(), Span::new(2, 0));
    }

    #[test]
    fn subrange_is_relative_to_the_fragment() {
        let source = "x\n  name = 1\n";
        let line = Fragment::lines(source).nth(1).unwrap();
        let value = line.subrange(9..10).unwrap();
        assert_eq!(value.text(), "1");
        assert_eq!(value.span(), Span::new(11, 1));
        assert_eq!(value.span().slice(source), Some("1"));
    }

    #[test]
    fn subrange_rejects_invalid_ranges() {
        let line = whole("café au lait");
        assert!(line.subrange(0..4).is_none(), "mid-codepoint end");
        assert!(line.subrange(4..6).is_none(), "mid-codepoint start");
        assert!(line.subrange(0..99).is_none(), "out of range");
        // Built from bindings so the literal reversed range does not trip a lint.
        let (start, end) = (5usize, 2usize);
        assert!(line.subrange(start..end).is_none(), "reversed");
        assert_eq!(line.subrange(0..5).unwrap().text(), "café");
    }

    #[test]
    fn subrange_of_a_subrange_stays_absolute() {
        let source = "0123456789";
        let outer = whole(source).subrange(2..8).unwrap();
        let inner = outer.subrange(1..3).unwrap();
        assert_eq!(inner.text(), "34");
        assert_eq!(inner.span(), Span::new(3, 2));
    }

    #[test]
    fn to_spanned_pairs_the_text_with_its_span() {
        let source = "a\ncafé = x\n";
        let line = Fragment::lines(source).nth(1).unwrap();
        let key = line.subrange(0..5).unwrap();
        assert_eq!(
            key.to_spanned(),
            Spanned::from_source(Span::new(2, 5), "café".to_string())
        );
    }
}
