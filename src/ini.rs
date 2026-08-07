//! The document model: what an INI file says, once it has been parsed.
//!
//! An [`Ini`] is a set of named sections plus a set of defaults every section inherits.
//! A [`Section`] maps an option name to the [`Entry`] that last defined it, in insertion
//! order. Option names are compared case-insensitively via [`str::to_lowercase`]; that is
//! not configurable, and the `optionxform` hook of `CPython` is out of scope. Section names
//! are compared exactly, as `CPython` does.

use crate::span::{Span, Spanned};
use indexmap::IndexMap;
use std::borrow::Cow;

/// Normalizes an option name for the case-insensitive comparison INI uses.
///
/// Borrows when `raw` is already lowercase, so a lookup with an already-normalized key
/// allocates nothing. The check is conservative: any name it borrows is unchanged by
/// [`str::to_lowercase`], so a borrowed name and an owned one never disagree.
fn normalize(raw: &str) -> Cow<'_, str> {
    let already_normalized = raw.chars().all(|c| {
        let mut lowered = c.to_lowercase();
        lowered.next() == Some(c) && lowered.next().is_none()
    });
    if already_normalized {
        Cow::Borrowed(raw)
    } else {
        Cow::Owned(raw.to_lowercase())
    }
}

/// Formats a value with its [`Display`](std::fmt::Display) impl where [`Debug`](std::fmt::Debug)
/// is what the formatter will call, as `debug_map` does for its values.
struct DisplayRepr<T>(T);

impl<T> std::fmt::Debug for DisplayRepr<T>
where
    T: std::fmt::Display,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

/// The error returned when a value cannot be read as a boolean.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
#[error("invalid boolean: {0:?}")]
pub struct InvalidBooleanError(pub String);

/// Returns a boolean value, translating from the spellings INI files use.
///
/// Adopted from <https://github.com/python/cpython/blob/main/Lib/configparser.py#L634>
///
/// # Errors
/// When the given value is not a valid boolean.
pub fn convert_to_boolean(value: &str) -> Result<bool, InvalidBooleanError> {
    let value = value.to_ascii_lowercase();
    match value.as_str() {
        "1" | "yes" | "true" | "on" => Ok(true),
        "0" | "no" | "false" | "off" => Ok(false),
        _ => Err(InvalidBooleanError(value)),
    }
}

/// One option occurrence: the name as it was written, and its value.
///
/// Both spans describe the *same* occurrence, because an entry is replaced whole when an
/// option is defined twice — a key span and a value span can never be taken from different
/// lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The option name exactly as written, with the span of this occurrence.
    pub key: Spanned<String>,
    /// The option value, with the span of this occurrence.
    pub value: Spanned<String>,
}

impl Entry {
    /// The value as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.value.as_str()
    }

    /// The value parsed as an `i32`, keeping the span it was read from.
    ///
    /// # Errors
    /// When the value is not a decimal integer, or does not fit in an `i32`; parse
    /// [`Entry::as_str`] yourself for other widths.
    pub fn as_int(&self) -> Result<Spanned<i32>, std::num::ParseIntError> {
        let parsed = self.value.as_str().parse()?;
        Ok(Spanned {
            inner: parsed,
            span: self.value.span,
        })
    }

    /// The value parsed as an `f64`, keeping the span it was read from.
    ///
    /// # Errors
    /// When the value is not a decimal float; a magnitude too large for an `f64` parses as
    /// an infinity rather than failing. Parse [`Entry::as_str`] yourself for other widths.
    pub fn as_float(&self) -> Result<Spanned<f64>, std::num::ParseFloatError> {
        let parsed = self.value.as_str().parse()?;
        Ok(Spanned {
            inner: parsed,
            span: self.value.span,
        })
    }

    /// The value parsed as a boolean, keeping the span it was read from.
    ///
    /// # Errors
    /// When the value is not a valid boolean.
    pub fn as_bool(&self) -> Result<Spanned<bool>, InvalidBooleanError> {
        let parsed = convert_to_boolean(self.value.as_str())?;
        Ok(Spanned {
            inner: parsed,
            span: self.value.span,
        })
    }
}

/// Projects an `IndexMap` entry onto the pair the public iterators yield.
fn as_pair<'a>((key, entry): (&'a String, &'a Entry)) -> (&'a str, &'a Entry) {
    (key.as_str(), entry)
}

/// The concrete iterator behind `IntoIterator for &Section`.
type SectionIter<'a> = std::iter::Map<
    indexmap::map::Iter<'a, String, Entry>,
    fn((&'a String, &'a Entry)) -> (&'a str, &'a Entry),
>;

/// One section: an insertion-ordered map from case-insensitive option name to entry.
///
/// The map keys are always normalized, because `entries` is private and [`Section::insert`]
/// is the only way to put anything in it. The name as the author wrote it is kept on
/// [`Entry::key`], so nothing is lost by normalizing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Section {
    entries: IndexMap<String, Entry>,
    header: Option<Span>,
}

impl Section {
    /// The span of the `[name]` header that introduced this section, or `None` when the
    /// section did not come from source text.
    ///
    /// The section's *name* is not stored here: it is owned by the [`Ini`] map key, so a
    /// name and its header span cannot drift apart.
    #[must_use]
    pub fn header_span(&self) -> Option<Span> {
        self.header
    }

    /// The entry for `key`, compared case-insensitively.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Entry> {
        self.entries.get(normalize(key).as_ref())
    }

    /// The entry for `key`, compared case-insensitively, for in-place modification.
    ///
    /// Writing a different name into [`Entry::key`] does not re-key the section; the entry
    /// stays reachable under the name it was inserted with.
    pub fn get_mut(&mut self, key: &str) -> Option<&mut Entry> {
        self.entries.get_mut(normalize(key).as_ref())
    }

    /// Whether an entry for `key` exists. Defined as [`Section::get`] being `Some`.
    #[must_use]
    pub fn contains(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// Defines `key` as `value`, replacing any previous occurrence wholesale — key and
    /// value together — and returning it.
    ///
    /// A replaced entry keeps the position of the occurrence it replaces, as INI expects:
    /// redefining an option does not move it to the end of the section.
    pub fn insert(&mut self, key: Spanned<String>, value: Spanned<String>) -> Option<Entry> {
        let normalized = normalize(key.as_str()).into_owned();
        self.entries.insert(normalized, Entry { key, value })
    }

    /// Removes and returns the entry for `key`, shifting later entries down to preserve
    /// insertion order.
    pub fn remove(&mut self, key: &str) -> Option<Entry> {
        self.entries.shift_remove(normalize(key).as_ref())
    }

    /// Yields `(normalized_key, entry)` in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Entry)> {
        self.entries.iter().map(as_pair)
    }

    /// Yields the normalized option names in insertion order.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    /// The number of options defined in this section.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the section defines no options at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Removes every option, keeping the header span.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Records the `[name]` header this section was introduced by.
    ///
    /// Which occurrence wins when a header repeats is a parser policy decision, made at the
    /// single call site rather than here.
    pub(crate) fn set_header(&mut self, span: Span) {
        self.header = Some(span);
    }
}

impl FromIterator<(Spanned<String>, Spanned<String>)> for Section {
    fn from_iter<I: IntoIterator<Item = (Spanned<String>, Spanned<String>)>>(iter: I) -> Self {
        let mut section = Self::default();
        for (key, value) in iter {
            section.insert(key, value);
        }
        section
    }
}

impl<const N: usize> From<[(Spanned<String>, Spanned<String>); N]> for Section {
    fn from(entries: [(Spanned<String>, Spanned<String>); N]) -> Self {
        entries.into_iter().collect()
    }
}

impl From<Vec<(Spanned<String>, Spanned<String>)>> for Section {
    fn from(entries: Vec<(Spanned<String>, Spanned<String>)>) -> Self {
        entries.into_iter().collect()
    }
}

impl<'a> IntoIterator for &'a Section {
    type Item = (&'a str, &'a Entry);
    type IntoIter = SectionIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter().map(as_pair as fn(_) -> _)
    }
}

impl std::fmt::Display for Section {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map()
            .entries(self.iter().map(|(key, entry)| (key, entry.as_str())))
            .finish()
    }
}

/// The error returned when an operation names a section the document does not have.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
#[error("missing section: {0:?}")]
pub struct NoSectionError(pub String);

/// A parsed INI document: named sections, plus the defaults every section inherits.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ini {
    sections: IndexMap<String, Section>,
    defaults: Section,
}

impl Ini {
    /// The options every section inherits.
    ///
    /// The defaults always exist; an empty document has empty defaults, not absent ones.
    #[must_use]
    pub fn defaults(&self) -> &Section {
        &self.defaults
    }

    /// The options every section inherits, for modification.
    pub fn defaults_mut(&mut self) -> &mut Section {
        &mut self.defaults
    }

    /// A read view of the section called `name`, with the defaults behind it.
    ///
    /// Section names are compared exactly; only option names are case-insensitive.
    #[must_use]
    pub fn section(&self, name: &str) -> Option<SectionView<'_>> {
        let (name, section) = self.sections.get_key_value(name)?;
        Some(SectionView {
            name: name.as_str(),
            section,
            defaults: &self.defaults,
        })
    }

    /// The section called `name`, for modification.
    ///
    /// This is the section's own entries only: mutating through the defaults of another
    /// section is not something a caller can ask for, so the view is not offered here.
    pub fn section_mut(&mut self, name: &str) -> Option<&mut Section> {
        self.sections.get_mut(name)
    }

    /// Adds a section, returning the one it replaced.
    pub fn insert_section(&mut self, name: impl Into<String>, section: Section) -> Option<Section> {
        self.sections.insert(name.into(), section)
    }

    /// Removes and returns the section called `name`, preserving the order of the rest.
    pub fn remove_section(&mut self, name: &str) -> Option<Section> {
        self.sections.shift_remove(name)
    }

    /// Yields the section names in the order they were first seen. The defaults are not
    /// a section and are not yielded.
    pub fn section_names(&self) -> impl Iterator<Item = &str> {
        self.sections.keys().map(String::as_str)
    }

    /// Whether a section called `name` exists.
    #[must_use]
    pub fn has_section(&self, name: &str) -> bool {
        self.sections.contains_key(name)
    }

    /// The number of sections, not counting the defaults.
    #[must_use]
    pub fn len(&self) -> usize {
        self.sections.len()
    }

    /// Whether the document has no sections. The defaults may still be non-empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sections.is_empty()
    }

    /// Removes every section, keeping the defaults.
    ///
    /// The defaults are not a section, so they survive, as they do in `configparser`.
    /// Use [`Ini::defaults_mut`] and [`Section::clear`] to drop them as well.
    pub fn clear(&mut self) {
        self.sections.clear();
    }

    /// Looks `option` up in `section`, then in the defaults.
    ///
    /// Returns `None` when there is no such section, so that a typo in a section name is
    /// not answered with a default.
    #[must_use]
    pub fn get(&self, section: &str, option: &str) -> Option<&Entry> {
        self.section(section)?.get(option)
    }

    /// Looks `option` up in `section`, then in the defaults, for modification.
    ///
    /// Applies the same rule as [`Ini::get`]: the section's own entries first, then the
    /// defaults. `ini::tests::ini_get_mut_resolves_like_get` pins that the two agree.
    pub fn get_mut(&mut self, section: &str, option: &str) -> Option<&mut Entry> {
        let in_section = self.sections.get(section).map(|s| s.contains(option))?;
        if in_section {
            self.sections
                .get_mut(section)
                .and_then(|s| s.get_mut(option))
        } else {
            self.defaults.get_mut(option)
        }
    }

    /// Defines `key` as `value` in `section`, returning the entry it replaced.
    ///
    /// # Errors
    /// When no section with the given name exists. Use [`Ini::defaults_mut`] to write a
    /// default; the defaults are not reachable under a section name.
    pub fn set(
        &mut self,
        section: &str,
        key: Spanned<String>,
        value: Spanned<String>,
    ) -> Result<Option<Entry>, NoSectionError> {
        let target = self
            .sections
            .get_mut(section)
            .ok_or_else(|| NoSectionError(section.to_string()))?;
        Ok(target.insert(key, value))
    }

    /// Removes `option` from `section`, returning the entry that was there.
    ///
    /// Only the section's own entries are considered: an inherited default is not removable
    /// through the section that inherits it.
    pub fn remove_option(&mut self, section: &str, option: &str) -> Option<Entry> {
        self.sections
            .get_mut(section)
            .and_then(|section| section.remove(option))
    }
}

impl std::fmt::Display for Ini {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map()
            .entries(
                self.sections
                    .iter()
                    .map(|(name, section)| (name.as_str(), DisplayRepr(section))),
            )
            .entries(
                self.defaults
                    .iter()
                    .map(|(key, entry)| (key, DisplayRepr(entry.as_str()))),
            )
            .finish()
    }
}

/// A read view of one section with the document defaults behind it.
///
/// `get` and `iter` both implement one rule — this section's own entries first, then the
/// defaults it does not shadow. `contains` is defined as `get(..).is_some()`, and `keys`,
/// `len` and `is_empty` are defined over `iter`, so no accessor can drift from the two that
/// state the rule.
#[derive(Debug, Clone, Copy)]
pub struct SectionView<'a> {
    name: &'a str,
    section: &'a Section,
    defaults: &'a Section,
}

impl<'a> SectionView<'a> {
    /// The section name, borrowed from the document.
    #[must_use]
    pub fn name(self) -> &'a str {
        self.name
    }

    /// The span of the `[name]` header that introduced this section, if it came from source.
    #[must_use]
    pub fn header_span(self) -> Option<Span> {
        self.section.header_span()
    }

    /// The one lookup rule: this section first, then the defaults.
    #[must_use]
    pub fn get(self, key: &str) -> Option<&'a Entry> {
        self.section.get(key).or_else(|| self.defaults.get(key))
    }

    /// Whether the name resolves at all. Defined as [`SectionView::get`] being `Some`.
    #[must_use]
    pub fn contains(self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// This section's entries in insertion order, then the defaults it does not shadow.
    pub fn iter(self) -> impl Iterator<Item = (&'a str, &'a Entry)> {
        self.merged()
    }

    /// The names [`SectionView::iter`] yields, in the same order.
    pub fn keys(self) -> impl Iterator<Item = &'a str> {
        self.iter().map(|(key, _)| key)
    }

    /// The number of names that resolve through this view.
    #[must_use]
    pub fn len(self) -> usize {
        self.iter().count()
    }

    /// Whether no name resolves through this view.
    #[must_use]
    pub fn is_empty(self) -> bool {
        self.iter().next().is_none()
    }

    /// The single definition of "what this view contains", shared by `iter`, `keys`, `len`
    /// and `is_empty`.
    fn merged(self) -> impl Iterator<Item = (&'a str, &'a Entry)> {
        let section = self.section;
        section
            .iter()
            .chain(self.defaults.iter().filter(move |(key, _)| {
                // A default the section redefines is already yielded, with the section's entry.
                !section.contains(key)
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Entry, Ini, InvalidBooleanError, NoSectionError, Section, SectionView, convert_to_boolean,
    };
    use crate::span::{Span, Spanned};
    use std::borrow::Cow;

    fn at(start: usize, text: &str) -> Spanned<String> {
        Spanned::from_source(Span::new(start, text.len()), text.to_string())
    }

    /// A section that shadows one default and leaves another alone.
    fn fixture() -> Ini {
        let mut ini = Ini::default();
        ini.defaults_mut().insert("shared".into(), "default".into());
        ini.defaults_mut()
            .insert("only_default".into(), "inherited".into());
        ini.insert_section(
            "app",
            Section::from([
                ("shared".into(), "overridden".into()),
                ("own".into(), "mine".into()),
            ]),
        );
        ini
    }

    #[test]
    fn normalize_borrows_an_already_lowercase_name() {
        assert!(matches!(super::normalize("port"), Cow::Borrowed("port")));
        assert!(matches!(super::normalize(""), Cow::Borrowed("")));
        assert!(matches!(super::normalize("a-1_b"), Cow::Borrowed("a-1_b")));
    }

    #[test]
    fn normalize_lowercases_when_it_must() {
        assert_eq!(super::normalize("Port"), Cow::Owned::<str>("port".into()));
        assert_eq!(super::normalize("CAFÉ"), Cow::Owned::<str>("café".into()));
        // Whatever it borrows must be a fixed point of `to_lowercase`.
        for raw in ["port", "café", "ǆ", "σ", "ß"] {
            assert_eq!(super::normalize(raw), Cow::Borrowed(raw), "{raw:?}");
        }
    }

    /// The bug this shape exists to kill: the array/vec conversions used to bypass
    /// normalization, so an entry inserted as `Foo` was reachable under no name at all.
    #[test]
    fn conversions_normalize_keys() {
        let from_array = Section::from([("Foo".into(), "1".into())]);
        let from_vec = Section::from(vec![("Foo".into(), "1".into())]);
        let collected: Section = [("Foo".into(), "1".into())].into_iter().collect();

        for section in [from_array, from_vec, collected] {
            assert_eq!(section.keys().collect::<Vec<_>>(), vec!["foo"]);
            assert_eq!(section.get("Foo").map(Entry::as_str), Some("1"));
            assert_eq!(section.get("foo").map(Entry::as_str), Some("1"));
            assert_eq!(section.get("FOO").map(Entry::as_str), Some("1"));
            assert!(section.contains("Foo"));
        }
    }

    /// The stored key is normalized; the spelling the author used survives on the entry.
    #[test]
    fn insert_keeps_the_original_spelling_on_the_entry() {
        let mut section = Section::default();
        section.insert("Foo".into(), "1".into());
        let entry = section.get("foo").unwrap();
        assert_eq!(entry.key.as_str(), "Foo");
        assert_eq!(section.keys().collect::<Vec<_>>(), vec!["foo"]);
    }

    /// A duplicate option must report both spans from the *same* occurrence. The old
    /// `IndexMap<Spanned<String>, _>` kept the first key on overwrite, so a key span from
    /// line 2 was paired with a value span from line 3.
    #[test]
    fn redefinition_replaces_key_and_value_together() {
        // "foo = 1\nFOO = 2\n"
        let mut section = Section::default();
        section.insert(at(0, "foo"), at(6, "1"));
        let replaced = section.insert(at(8, "FOO"), at(14, "2"));

        assert_eq!(
            replaced,
            Some(Entry {
                key: at(0, "foo"),
                value: at(6, "1"),
            })
        );

        let entry = section.get("foo").unwrap();
        assert_eq!(entry.key, at(8, "FOO"));
        assert_eq!(entry.value, at(14, "2"));
        assert_eq!(section.len(), 1);
    }

    #[test]
    fn redefinition_keeps_the_original_position() {
        let mut section = Section::default();
        section.insert("a".into(), "1".into());
        section.insert("b".into(), "2".into());
        section.insert("A".into(), "3".into());

        assert_eq!(section.keys().collect::<Vec<_>>(), vec!["a", "b"]);
        assert_eq!(section.get("a").map(Entry::as_str), Some("3"));
    }

    /// Four entries, removing the second: a swap-remove would move `d` into the hole and
    /// yield `["a", "d", "c"]`, which three entries could not have told apart.
    #[test]
    fn remove_preserves_the_order_of_the_rest() {
        let mut section = Section::from([
            ("a".into(), "1".into()),
            ("b".into(), "2".into()),
            ("c".into(), "3".into()),
            ("d".into(), "4".into()),
        ]);
        let removed = section.remove("B").unwrap();
        assert_eq!(removed.value.as_str(), "2");
        assert_eq!(section.keys().collect::<Vec<_>>(), vec!["a", "c", "d"]);
        assert!(section.remove("b").is_none());
    }

    #[test]
    fn clear_keeps_the_header_span() {
        let mut section = Section::from([("a".into(), "1".into())]);
        section.set_header(Span::new(0, 5));
        section.clear();
        assert!(section.is_empty());
        assert_eq!(section.header_span(), Some(Span::new(0, 5)));
    }

    #[test]
    fn a_section_from_thin_air_has_no_header_span() {
        assert_eq!(Section::default().header_span(), None);
    }

    #[test]
    fn get_mut_edits_in_place() {
        let mut section = Section::from([("a".into(), "1".into())]);
        section.get_mut("A").unwrap().value = "2".into();
        assert_eq!(section.get("a").map(Entry::as_str), Some("2"));
    }

    #[test]
    fn section_iterates_in_insertion_order() {
        let section = Section::from([("b".into(), "1".into()), ("a".into(), "2".into())]);
        let by_iter: Vec<_> = section.iter().map(|(key, _)| key).collect();
        let by_into_iter: Vec<_> = (&section).into_iter().map(|(key, _)| key).collect();
        assert_eq!(by_iter, vec!["b", "a"]);
        assert_eq!(by_into_iter, by_iter);
    }

    #[test]
    fn section_display_shows_normalized_keys() {
        let section = Section::from([("Foo".into(), "1".into())]);
        assert_eq!(section.to_string(), r#"{"foo": "1"}"#);
    }

    /// `keys` is *defined* as the names `iter` yields; this pins that they cannot drift.
    #[test]
    fn view_keys_agree_with_iter() {
        let ini = fixture();
        let view = ini.section("app").unwrap();
        assert!(view.keys().eq(view.iter().map(|(key, _)| key)));
        assert_eq!(
            view.keys().collect::<Vec<_>>(),
            vec!["shared", "own", "only_default"]
        );
        assert_eq!(view.len(), 3);
        assert!(!view.is_empty());
    }

    /// `contains` is *defined* as `get(..).is_some()`. The old `has_option` consulted the
    /// defaults while `get` did not, so it could answer yes where `get` returned `None`.
    #[test]
    fn view_contains_agrees_with_get() {
        let ini = fixture();
        let view = ini.section("app").unwrap();
        for key in [
            "shared",
            "SHARED",
            "own",
            "only_default",
            "ONLY_DEFAULT",
            "missing",
            "",
        ] {
            assert_eq!(view.contains(key), view.get(key).is_some(), "{key:?}");
        }
    }

    /// Every name the view iterates must also resolve through `get`, and vice versa.
    #[test]
    fn view_iteration_and_lookup_see_the_same_entries() {
        let ini = fixture();
        let view = ini.section("app").unwrap();
        for (key, entry) in view.iter() {
            assert_eq!(view.get(key), Some(entry), "{key:?}");
        }
        assert_eq!(view.iter().count(), view.keys().filter(|_| true).count());
    }

    #[test]
    fn view_shadows_defaults_with_section_entries() {
        let ini = fixture();
        let view = ini.section("app").unwrap();
        assert_eq!(view.get("shared").map(Entry::as_str), Some("overridden"));
        assert_eq!(
            view.get("only_default").map(Entry::as_str),
            Some("inherited")
        );
        assert_eq!(view.get("own").map(Entry::as_str), Some("mine"));
        assert_eq!(view.get("nope"), None);
        assert_eq!(view.name(), "app");
    }

    #[test]
    fn a_view_of_an_empty_section_still_sees_the_defaults() {
        let mut ini = Ini::default();
        ini.defaults_mut().insert("shared".into(), "yes".into());
        ini.insert_section("empty", Section::default());
        let view = ini.section("empty").unwrap();
        assert!(!view.is_empty());
        assert_eq!(view.keys().collect::<Vec<_>>(), vec!["shared"]);
    }

    #[test]
    fn view_header_span_comes_from_the_section() {
        let mut section = Section::default();
        section.set_header(Span::new(3, 5));
        let mut ini = Ini::default();
        ini.insert_section("app", section);
        assert_eq!(
            ini.section("app").and_then(SectionView::header_span),
            Some(Span::new(3, 5))
        );
    }

    #[test]
    fn ini_get_falls_back_to_the_defaults() {
        let ini = fixture();
        assert_eq!(
            ini.get("app", "only_default").map(Entry::as_str),
            Some("inherited")
        );
        assert_eq!(
            ini.get("app", "shared").map(Entry::as_str),
            Some("overridden")
        );
    }

    /// A typo in a section name must not be answered with a default.
    #[test]
    fn ini_get_on_a_missing_section_is_none() {
        let ini = fixture();
        assert_eq!(ini.get("nope", "only_default"), None);
        assert_eq!(ini.section("nope").map(SectionView::name), None);
    }

    #[test]
    fn ini_get_mut_resolves_like_get() {
        let mut ini = fixture();
        for (section, option) in [
            ("app", "shared"),
            ("app", "only_default"),
            ("app", "missing"),
            ("nope", "only_default"),
        ] {
            let expected = ini.get(section, option).cloned();
            let found = ini.get_mut(section, option).map(|entry| entry.clone());
            assert_eq!(found, expected, "{section:?} {option:?}");
        }
    }

    #[test]
    fn ini_get_mut_writes_through_to_the_defaults() {
        let mut ini = fixture();
        ini.get_mut("app", "only_default").unwrap().value = "edited".into();
        assert_eq!(
            ini.defaults().get("only_default").map(Entry::as_str),
            Some("edited")
        );
    }

    #[test]
    fn set_requires_an_existing_section() {
        let mut ini = fixture();
        assert_eq!(
            ini.set("nope", "a".into(), "1".into()),
            Err(NoSectionError("nope".to_string()))
        );
        assert_eq!(
            ini.set("app", "Own".into(), "changed".into())
                .unwrap()
                .map(|entry| entry.value),
            Some("mine".into())
        );
        assert_eq!(ini.get("app", "own").map(Entry::as_str), Some("changed"));
    }

    #[test]
    fn section_names_are_case_sensitive() {
        let ini = fixture();
        assert!(ini.has_section("app"));
        assert!(!ini.has_section("APP"));
        assert_eq!(ini.section_names().collect::<Vec<_>>(), vec!["app"]);
        assert_eq!(ini.len(), 1);
        assert!(!ini.is_empty());
    }

    #[test]
    fn remove_option_ignores_inherited_defaults() {
        let mut ini = fixture();
        assert_eq!(ini.remove_option("app", "only_default"), None);
        assert_eq!(
            ini.remove_option("app", "SHARED").map(|e| e.value),
            Some("overridden".into())
        );
        // Removing the shadowing entry uncovers the default again.
        assert_eq!(ini.get("app", "shared").map(Entry::as_str), Some("default"));
        assert_eq!(ini.remove_option("nope", "shared"), None);
    }

    #[test]
    fn remove_section_leaves_the_defaults_alone() {
        let mut ini = fixture();
        let removed = ini.remove_section("app").unwrap();
        assert_eq!(removed.keys().collect::<Vec<_>>(), vec!["shared", "own"]);
        assert!(ini.is_empty());
        assert_eq!(ini.defaults().len(), 2);
    }

    /// `configparser`'s `clear()` deliberately spares `DEFAULTSECT`; so does this one.
    #[test]
    fn clear_empties_the_sections_but_spares_the_defaults() {
        let mut ini = fixture();
        ini.clear();
        assert!(ini.is_empty());
        assert_eq!(ini.defaults().len(), 2);
        ini.defaults_mut().clear();
        assert!(ini.defaults().is_empty());
    }

    #[test]
    fn section_mut_edits_the_named_section() {
        let mut ini = fixture();
        ini.section_mut("app")
            .unwrap()
            .insert("Extra".into(), "x".into());
        assert_eq!(ini.get("app", "extra").map(Entry::as_str), Some("x"));
        assert!(ini.section_mut("nope").is_none());
    }

    #[test]
    fn ini_display_lists_sections_then_defaults() {
        let ini = fixture();
        assert_eq!(
            ini.to_string(),
            r#"{"app": {"shared": "overridden", "own": "mine"}, "shared": default, "only_default": inherited}"#
        );
    }

    #[test]
    fn typed_accessors_keep_the_value_span() {
        let entry = Entry {
            key: at(0, "port"),
            value: at(7, "8080"),
        };
        assert_eq!(entry.as_str(), "8080");
        assert_eq!(
            entry.as_int().unwrap(),
            Spanned::from_source(Span::new(7, 4), 8080)
        );
        assert_eq!(
            entry.as_float().unwrap(),
            Spanned::from_source(Span::new(7, 4), 8080.0)
        );
        assert!(entry.as_bool().is_err());

        let flag = Entry {
            key: at(0, "tls"),
            value: at(6, "true"),
        };
        assert_eq!(
            flag.as_bool().unwrap(),
            Spanned::from_source(Span::new(6, 4), true)
        );
    }

    #[test]
    fn typed_accessors_report_bad_values() {
        let entry = Entry {
            key: "k".into(),
            value: "nope".into(),
        };
        assert!(entry.as_int().is_err());
        assert!(entry.as_float().is_err());
        assert_eq!(
            entry.as_bool(),
            Err(InvalidBooleanError("nope".to_string()))
        );
    }

    /// `as_int` is documented as an `i32` accessor: a decimal integer too wide for one is
    /// rejected, not truncated. `as_float` has no equivalent failure — `f64` saturates to an
    /// infinity — which is why only the integer width is a documented error.
    #[test]
    fn an_integer_too_wide_for_i32_is_an_error() {
        let entry = Entry {
            key: "k".into(),
            value: "99999999999999".into(),
        };
        assert!(entry.as_int().is_err());

        let huge = Entry {
            key: "k".into(),
            value: "1e400".into(),
        };
        assert!(huge.as_float().unwrap().into_inner().is_infinite());
    }

    #[test]
    fn booleans_use_the_configparser_spellings() {
        for truthy in ["1", "yes", "true", "on", "YES", "True", "ON"] {
            assert_eq!(convert_to_boolean(truthy), Ok(true), "{truthy:?}");
        }
        for falsy in ["0", "no", "false", "off", "NO", "False", "OFF"] {
            assert_eq!(convert_to_boolean(falsy), Ok(false), "{falsy:?}");
        }
        // Only the eight spellings above are accepted; nothing is inferred from a prefix.
        for rejected in ["y", "n", "t", "f", "", "2", "yes please", " yes"] {
            assert!(
                convert_to_boolean(rejected).is_err(),
                "{rejected:?} must not be a boolean"
            );
        }
        assert_eq!(
            convert_to_boolean("Maybe"),
            Err(InvalidBooleanError("maybe".to_string()))
        );
    }

    /// Equality is span-aware, because `Spanned` derives it.
    #[test]
    fn equality_notices_a_different_span() {
        let mut here = Section::default();
        here.insert(at(0, "a"), at(4, "1"));
        let mut there = Section::default();
        there.insert(at(8, "a"), at(12, "1"));
        assert_ne!(here, there);
        assert_eq!(here, here.clone());
    }
}
