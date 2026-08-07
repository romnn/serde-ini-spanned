//! Byte ranges into the source text, and values tagged with where they came from.

/// A byte range into the source text a value was parsed from.
///
/// A span is a start offset plus a **length**, never a start and an end: a length
/// cannot be negative, so `start <= end` holds by construction and a reversed range
/// is not expressible. Offsets are byte offsets, because every span this crate
/// produces is derived from `str` length arithmetic over the source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Span {
    start: usize,
    len: usize,
}

impl Span {
    /// A span of `len` bytes beginning at byte offset `start`.
    #[must_use]
    pub const fn new(start: usize, len: usize) -> Self {
        Self { start, len }
    }

    /// The byte offset the span begins at.
    #[must_use]
    pub const fn start(self) -> usize {
        self.start
    }

    /// The byte offset one past the last byte of the span.
    ///
    /// Saturating; a `Span` produced by this crate never comes close to overflowing,
    /// because `start + len` is bounded by the length of the source text.
    #[must_use]
    pub const fn end(self) -> usize {
        self.start.saturating_add(self.len)
    }

    /// The number of bytes the span covers.
    #[must_use]
    pub const fn len(self) -> usize {
        self.len
    }

    /// Whether the span covers no bytes at all.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.len == 0
    }

    /// The text this span points at, or `None` when it does not fit `source`
    /// or does not land on `char` boundaries.
    ///
    /// Prefer [`Parsed::text`](crate::Parsed::text) or
    /// [`Parsed::text_of`](crate::Parsed::text_of), which own the source they slice and so
    /// cannot be handed the wrong one.
    #[must_use]
    pub fn slice(self, source: &str) -> Option<&str> {
        source.get(self.start..self.end())
    }
}

impl From<Span> for std::ops::Range<usize> {
    fn from(span: Span) -> Self {
        span.start()..span.end()
    }
}

/// A value together with where in the source it came from, if it came from source at all.
///
/// A value that was not parsed has `span: None` rather than a fabricated `0..0`, so a
/// diagnostic builder is forced to decide what to do with it instead of pointing a caret
/// at the first character of the file.
///
/// `PartialEq` is derived, so the span participates in equality: a test that compares two
/// `Spanned` values cannot silently skip the provenance it was written to check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spanned<T> {
    /// The value itself.
    pub inner: T,
    /// Where `inner` was read from, or `None` when it was not read from source.
    pub span: Option<Span>,
}

impl<T> Spanned<T> {
    /// A value read from source text at `span`.
    #[must_use]
    pub const fn from_source(span: Span, value: T) -> Self {
        Self {
            inner: value,
            span: Some(span),
        }
    }

    /// A value the caller constructed rather than parsed; it has no place in the source.
    #[must_use]
    pub const fn synthetic(value: T) -> Self {
        Self {
            inner: value,
            span: None,
        }
    }

    /// Applies `f` to the value, keeping the span it came from.
    #[must_use]
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Spanned<U> {
        Spanned {
            inner: f(self.inner),
            span: self.span,
        }
    }

    /// Discards the span and returns the value.
    #[must_use]
    pub fn into_inner(self) -> T {
        self.inner
    }

    /// Borrows the value without its span.
    #[must_use]
    pub const fn as_ref(&self) -> &T {
        &self.inner
    }
}

impl Spanned<String> {
    /// Borrows the value as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.inner.as_str()
    }
}

impl<T> std::fmt::Display for Spanned<T>
where
    T: std::fmt::Display,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.inner.fmt(f)
    }
}

/// Convenience for callers building configuration programmatically; the produced
/// value is synthetic and carries no span.
impl From<&str> for Spanned<String> {
    fn from(value: &str) -> Self {
        Self::synthetic(value.to_string())
    }
}

/// Convenience for callers building configuration programmatically; the produced
/// value is synthetic and carries no span.
impl From<String> for Spanned<String> {
    fn from(value: String) -> Self {
        Self::synthetic(value)
    }
}

#[cfg(test)]
mod tests {
    use super::{Span, Spanned};

    #[test]
    fn span_start_end_len() {
        let span = Span::new(3, 4);
        assert_eq!(span.start(), 3);
        assert_eq!(span.end(), 7);
        assert_eq!(span.len(), 4);
        assert!(!span.is_empty());
    }

    #[test]
    fn empty_span_is_empty() {
        let span = Span::new(9, 0);
        assert!(span.is_empty());
        assert_eq!(span.start(), span.end());
    }

    #[test]
    fn default_span_is_empty_at_zero() {
        let span = Span::default();
        assert_eq!(span.start(), 0);
        assert_eq!(span.end(), 0);
        assert!(span.is_empty());
    }

    /// `end()` saturates rather than overflowing, which no span from this crate can reach.
    #[test]
    fn end_saturates() {
        let span = Span::new(usize::MAX, 5);
        assert_eq!(span.end(), usize::MAX);
    }

    #[test]
    fn slice_returns_the_pointed_at_text() {
        let source = "hello world";
        assert_eq!(Span::new(6, 5).slice(source), Some("world"));
        assert_eq!(Span::new(0, 0).slice(source), Some(""));
        assert_eq!(Span::new(11, 0).slice(source), Some(""));
    }

    #[test]
    fn slice_out_of_range_is_none() {
        let source = "abc";
        assert_eq!(Span::new(2, 5).slice(source), None);
        assert_eq!(Span::new(4, 0).slice(source), None);
    }

    #[test]
    fn slice_off_a_char_boundary_is_none() {
        let source = "café";
        // The final `é` occupies bytes 3..5.
        assert_eq!(Span::new(0, 4).slice(source), None);
        assert_eq!(Span::new(4, 1).slice(source), None);
        assert_eq!(Span::new(3, 2).slice(source), Some("é"));
    }

    #[test]
    fn span_converts_to_a_range() {
        let range: std::ops::Range<usize> = Span::new(2, 3).into();
        assert_eq!(range, 2..5);
    }

    #[test]
    fn from_source_keeps_the_span() {
        let spanned = Spanned::from_source(Span::new(4, 5), "value".to_string());
        assert_eq!(spanned.span, Some(Span::new(4, 5)));
        assert_eq!(spanned.as_str(), "value");
    }

    #[test]
    fn synthetic_values_have_no_span() {
        let spanned = Spanned::synthetic(7);
        assert_eq!(spanned.span, None);
        assert_eq!(spanned.into_inner(), 7);
    }

    #[test]
    fn map_preserves_the_span() {
        let spanned = Spanned::from_source(Span::new(1, 2), "12".to_string());
        let mapped = spanned.map(|value| value.len());
        assert_eq!(mapped, Spanned::from_source(Span::new(1, 2), 2));

        let synthetic = Spanned::synthetic("12".to_string()).map(|value| value.len());
        assert_eq!(synthetic.span, None);
    }

    #[test]
    fn as_ref_borrows_the_value() {
        let spanned = Spanned::from_source(Span::new(0, 1), vec![1, 2, 3]);
        assert_eq!(spanned.as_ref().len(), 3);
    }

    #[test]
    fn display_forwards_to_inner() {
        assert_eq!(
            Spanned::from_source(Span::new(10, 3), "abc").to_string(),
            "abc"
        );
        assert_eq!(Spanned::synthetic(42).to_string(), "42");
    }

    /// The whole point of deriving `PartialEq`: a wrong span fails an equality assertion.
    #[test]
    fn equality_compares_the_span() {
        let value = "x".to_string();
        let here = Spanned::from_source(Span::new(0, 1), value.clone());
        let there = Spanned::from_source(Span::new(4, 1), value.clone());
        let nowhere = Spanned::synthetic(value);

        assert_eq!(here, here.clone());
        assert_ne!(here, there);
        assert_ne!(here, nowhere);
    }

    #[test]
    fn conversions_from_strings_are_synthetic() {
        assert_eq!(
            Spanned::from("borrowed"),
            Spanned::synthetic("borrowed".to_string())
        );
        assert_eq!(
            Spanned::from("owned".to_string()),
            Spanned::synthetic("owned".to_string())
        );
    }
}
