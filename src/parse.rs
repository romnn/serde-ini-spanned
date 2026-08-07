//! The fold from classified lines into a document, recording every problem it meets.

use crate::fragment::Fragment;
use crate::ini::{Ini, Section, SectionView};
use crate::lexer::{Line, classify};
use crate::options::{CompiledOptions, Options};
use crate::problem::{Problem, ProblemKind, Problems, Severity};
use crate::span::{Span, Spanned};

/// Where subsequent options land.
///
/// This is three states because "no section header has been seen yet" and "write to the
/// defaults" are different situations that an `Option<name>` would conflate.
enum Cursor {
    /// Before any section header was seen; an option here has nowhere to go.
    Preamble,
    /// Inside the section whose options every other section inherits.
    Defaults,
    /// Inside the named section.
    Named(String),
}

/// The option currently open for continuation lines. Absent when none is.
struct OpenOption {
    /// The name as written, with this occurrence's span.
    key: Spanned<String>,
    /// The indent of the option's own line; a continuation must exceed it.
    indent: usize,
    /// The value accumulated so far, with continuations joined by `\n`.
    value: String,
    /// Extended only to the end of a fragment that actually contributed text, so a blank
    /// line that turns out to be trailing never has to be subtracted back off.
    span: Span,
    /// Blank lines seen since the last contributing fragment.
    ///
    /// They are flushed as one `\n` each when a non-blank continuation arrives and are
    /// dropped when the option closes, which is why a value never ends in a blank line and
    /// its span never runs past the last text it holds.
    pending_blank_lines: usize,
}

/// Extends `span` so that it ends at `end`.
///
/// `end` is the end of a fragment on a later line than the one `span` began on, so it is
/// never before `span.start()`; the saturating subtraction states that rather than relying
/// on it.
fn extend_to(span: Span, end: usize) -> Span {
    Span::new(span.start(), end.saturating_sub(span.start()))
}

/// Reads `source` into a document, recording every problem it meets.
fn fold(source: &str, options: &CompiledOptions, strict: bool) -> (Ini, Vec<Problem>) {
    let mut ini = Ini::default();
    let mut problems = Problems::new(strict);
    let mut cursor = Cursor::Preamble;
    let mut open: Option<OpenOption> = None;

    for line in Fragment::lines(source) {
        match classify(line, open.as_ref().map(|open| open.indent), options) {
            Line::Blank => {
                if options.allow_empty_lines_in_values() {
                    // A comment line is blank too, but `configparser` grows an open value by
                    // an empty line only for a line that carried no comment at all.
                    if line.is_blank()
                        && let Some(open) = open.as_mut()
                    {
                        open.pending_blank_lines += 1;
                    }
                } else {
                    close(&mut open, &cursor, &mut ini, &mut problems);
                }
            }
            Line::Continuation(fragment) => {
                if let Some(open) = open.as_mut() {
                    for _ in 0..open.pending_blank_lines {
                        open.value.push('\n');
                    }
                    open.pending_blank_lines = 0;
                    open.value.push('\n');
                    open.value.push_str(fragment.text());
                    open.span = extend_to(open.span, fragment.span().end());
                }
            }
            Line::SectionHeader(name) => {
                close(&mut open, &cursor, &mut ini, &mut problems);
                cursor = open_section(name, &mut ini, &mut problems, options);
            }
            Line::Option { key, value } => {
                close(&mut open, &cursor, &mut ini, &mut problems);
                open = Some(OpenOption {
                    key: key.to_spanned(),
                    indent: line.indent(),
                    value: value.text().to_string(),
                    span: value.span(),
                    pending_blank_lines: 0,
                });
            }
            Line::Malformed { kind, span } => {
                close(&mut open, &cursor, &mut ini, &mut problems);
                problems.push(kind, span);
            }
        }
    }
    close(&mut open, &cursor, &mut ini, &mut problems);

    (ini, problems.into_items())
}

/// Finishes the open option, if any, and files it where the cursor points.
///
/// A duplicate is reported and then applied: the last occurrence wins in both strictness
/// modes, so strictness changes what is said about the document and never what it holds.
fn close(open: &mut Option<OpenOption>, cursor: &Cursor, ini: &mut Ini, problems: &mut Problems) {
    let Some(OpenOption {
        key, value, span, ..
    }) = open.take()
    else {
        return;
    };
    // Every key here came from a source fragment, so it has a span; falling back to the
    // value's span keeps the report pointing at real text rather than at byte zero.
    let key_span = key.span.unwrap_or(span);
    let value = Spanned::from_source(span, value);

    let section = match cursor {
        Cursor::Preamble => {
            problems.push(ProblemKind::MissingSectionHeader, key_span);
            return;
        }
        Cursor::Defaults => ini.defaults_mut(),
        // The cursor only names a section that `open_section` inserted.
        Cursor::Named(name) => match ini.section_mut(name) {
            Some(section) => section,
            None => return,
        },
    };

    let duplicate = section
        .get(key.as_str())
        .map(|existing| ProblemKind::DuplicateOption {
            key: key.inner.clone(),
            previous: existing.key.span,
        });
    if let Some(kind) = duplicate {
        problems.push(kind, key_span);
    }
    section.insert(key, value);
}

/// Points the cursor at the section `name` introduces, creating it when it is new.
///
/// A repeated header merges into the section that already exists and keeps that section's
/// first header span, because a name has exactly one definition site.
fn open_section(
    name: Fragment<'_>,
    ini: &mut Ini,
    problems: &mut Problems,
    options: &CompiledOptions,
) -> Cursor {
    let text = name.text();

    if text == options.default_section() {
        let defaults = ini.defaults_mut();
        if let Some(previous) = defaults.header_span() {
            problems.push(
                ProblemKind::DuplicateSection {
                    name: text.to_string(),
                    previous: Some(previous),
                },
                name.span(),
            );
        } else {
            defaults.set_header(name.span());
        }
        return Cursor::Defaults;
    }

    if let Some(previous) = ini.section(text).map(SectionView::header_span) {
        problems.push(
            ProblemKind::DuplicateSection {
                name: text.to_string(),
                previous,
            },
            name.span(),
        );
    } else {
        let mut section = Section::default();
        section.set_header(name.span());
        ini.insert_section(text, section);
    }
    Cursor::Named(text.to_string())
}

/// A parsed document together with the source it was parsed from and everything wrong
/// with it.
///
/// The source is owned so that a span and the text it points at cannot be separated:
/// [`Parsed::text`] cannot be handed a different document. [`Parsed::into_ini`] and
/// [`Parsed::into_result`] are the visible way out of that pairing.
#[derive(Debug, Clone)]
#[must_use]
pub struct Parsed {
    source: String,
    ini: Ini,
    problems: Vec<Problem>,
}

impl Parsed {
    /// The document that was read, however malformed the source was.
    #[must_use]
    pub fn ini(&self) -> &Ini {
        &self.ini
    }

    /// Everything wrong with the document, in the order it was met.
    #[must_use]
    pub fn problems(&self) -> &[Problem] {
        &self.problems
    }

    /// The source text the document was parsed from.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The text `span` points at, or `None` when the span is not a `char`-boundary range
    /// of this document's source.
    ///
    /// Every span this crate produces satisfies that, so `None` means the span came from
    /// somewhere else.
    #[must_use]
    pub fn text(&self, span: Span) -> Option<&str> {
        span.slice(&self.source)
    }

    /// The text `spanned` was read from, or `None` when it was not read from this source.
    #[must_use]
    pub fn text_of<T>(&self, spanned: &Spanned<T>) -> Option<&str> {
        spanned.span.and_then(|span| self.text(span))
    }

    /// Whether any problem is severe enough that the parser does not vouch for the result.
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.problems
            .iter()
            .any(|problem| problem.severity == Severity::Error)
    }

    /// The document, discarding the source and the problems.
    #[must_use]
    pub fn into_ini(self) -> Ini {
        self.ini
    }

    /// The document, or — if any problem is an error — this whole `Parsed`, which still
    /// carries the best-effort document, the source and the problems.
    ///
    /// # Errors
    /// When [`Parsed::has_errors`] is true.
    #[expect(
        clippy::result_large_err,
        reason = "the error variant is the parse result itself, so the caller gets the \
                  best-effort document, its source and its problems back by value; boxing \
                  would silence the lint only by allocating on the path whose whole purpose \
                  is handing the same data back"
    )]
    pub fn into_result(self) -> Result<Ini, Self> {
        if self.has_errors() {
            Err(self)
        } else {
            Ok(self.ini)
        }
    }
}

impl std::fmt::Display for Parsed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} problem(s) in the configuration", self.problems.len())
    }
}

impl std::error::Error for Parsed {}

/// Reasons parsing could not run at all — never a statement about the document's contents.
///
/// Everything wrong with the *document* is a [`Problem`] carrying the span that provoked
/// it, so that all of them are reported instead of only the first.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// The source could not be read.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// The parsing preferences are contradictory or unusable.
    #[error("invalid options: {0}")]
    InvalidOptions(String),
}

/// Reads an INI document from `source`.
///
/// The source is copied into the returned [`Parsed`], which is what lets it answer
/// [`Parsed::text`] for any span it produced. Configuration files are small; a caller that
/// does not want the copy can drop it with [`Parsed::into_ini`].
///
/// # Errors
/// When the options cannot be compiled; see [`Options`].
pub fn parse(source: &str, options: &Options) -> Result<Parsed, Error> {
    parse_owned(source.to_string(), options)
}

/// Reads an INI document from `reader`.
///
/// The whole input is read into memory before parsing, so a span is an offset into the
/// complete document rather than into one line of it. Lines end at `\n`, `\r\n` or a lone
/// `\r`, matching `CPython`'s `configparser.read_file`.
///
/// # Errors
/// When the options cannot be compiled, when reading fails, or when the input is not UTF-8.
pub fn parse_reader(mut reader: impl std::io::Read, options: &Options) -> Result<Parsed, Error> {
    let mut source = String::new();
    reader.read_to_string(&mut source)?;
    parse_owned(source, options)
}

/// The single parsing path, once the source is owned.
fn parse_owned(source: String, options: &Options) -> Result<Parsed, Error> {
    let compiled = options.compile()?;
    let (ini, problems) = fold(&source, &compiled, options.strict);
    Ok(Parsed {
        source,
        ini,
        problems,
    })
}

#[cfg(test)]
mod tests {
    use super::{Error, Parsed, parse, parse_reader};
    use crate::ini::{Entry, SectionView};
    use crate::options::Options;
    use crate::problem::{ProblemKind, Severity};
    use crate::span::{Span, Spanned};

    #[track_caller]
    fn read(source: &str) -> Parsed {
        parse(source, &Options::default()).unwrap()
    }

    #[track_caller]
    fn read_with(source: &str, options: &Options) -> Parsed {
        parse(source, options).unwrap()
    }

    /// The value of one option, resolved through the section's defaults.
    #[track_caller]
    fn value_of<'a>(parsed: &'a Parsed, section: &str, option: &str) -> Option<&'a str> {
        parsed.ini().get(section, option).map(Entry::as_str)
    }

    fn kinds(parsed: &Parsed) -> Vec<ProblemKind> {
        parsed
            .problems()
            .iter()
            .map(|problem| problem.kind.clone())
            .collect()
    }

    #[test]
    fn an_empty_document_has_nothing_in_it() {
        let parsed = read("");
        assert!(parsed.ini().is_empty());
        assert!(parsed.ini().defaults().is_empty());
        assert!(parsed.problems().is_empty());
        assert!(!parsed.has_errors());
        assert_eq!(parsed.source(), "");
    }

    #[test]
    fn sections_and_options_keep_source_order() {
        let parsed = read("[b]\nz = 1\na = 2\n[a]\nk = 3\n");
        assert_eq!(
            parsed.ini().section_names().collect::<Vec<_>>(),
            vec!["b", "a"]
        );
        let section = parsed.ini().section("b").unwrap();
        assert_eq!(section.keys().collect::<Vec<_>>(), vec!["z", "a"]);
    }

    /// The whole point of the crate: a span slices the text it stands for.
    #[test]
    fn spans_slice_the_source_they_came_from() {
        let source = "[owner]\nName = Ada\n";
        let parsed = read(source);
        let entry = parsed.ini().get("owner", "name").unwrap();

        assert_eq!(parsed.text_of(&entry.key), Some("Name"));
        assert_eq!(parsed.text_of(&entry.value), Some("Ada"));
        assert_eq!(
            parsed
                .ini()
                .section("owner")
                .and_then(SectionView::header_span)
                .and_then(|span| parsed.text(span)),
            Some("owner")
        );
    }

    /// A span from another document is rejected rather than silently sliced, and a value
    /// that carries no span at all resolves to `None` rather than to the empty string.
    #[test]
    fn text_of_a_span_that_does_not_fit_is_none() {
        let parsed = read("[s]\nk = 1\n");
        assert_eq!(parsed.text(Span::new(900, 3)), None);

        let synthetic: Spanned<String> = "hello".into();
        assert_eq!(parsed.text_of(&synthetic), None);
    }

    #[test]
    fn a_continuation_extends_the_value_and_its_span() {
        let source = "[s]\nk = a\n\tb\n";
        let parsed = read(source);
        let entry = parsed.ini().get("s", "k").unwrap();
        assert_eq!(entry.as_str(), "a\nb");
        assert_eq!(entry.value.span, Some(Span::new(8, 4)));
        assert_eq!(parsed.text_of(&entry.value), Some("a\n\tb"));
    }

    /// The defect the whole `pending_blank_lines` design exists for: a blank line that
    /// turns out to be trailing must never have been added to the span in the first place.
    #[test]
    fn trailing_blank_lines_never_reach_the_span() {
        let source = "[server]\nk = 8080\n   \n[client]\nk = 1\n";
        let parsed = read(source);
        let entry = parsed.ini().get("server", "k").unwrap();
        assert_eq!(entry.as_str(), "8080");
        assert_eq!(parsed.text_of(&entry.value), Some("8080"));
    }

    /// The reversed-span reproduction: trailing whitespace lines at end of input.
    #[test]
    fn trailing_whitespace_lines_at_end_of_input_are_dropped() {
        let source = "[s]\nk = a\n  \n  \n";
        let parsed = read(source);
        let entry = parsed.ini().get("s", "k").unwrap();
        assert_eq!(entry.as_str(), "a");
        assert_eq!(parsed.text_of(&entry.value), Some("a"));
    }

    /// A comment line inside a value contributes nothing, while a blank line contributes
    /// exactly one newline — `configparser` counts only lines that carried no comment.
    #[test]
    fn blank_lines_count_and_comment_lines_do_not() {
        let parsed = read("[s]\nk = one\n\n# note\n\n  two\n");
        assert_eq!(value_of(&parsed, "s", "k"), Some("one\n\n\ntwo"));
    }

    /// A whitespace-only line contributes one newline, not its own raw whitespace.
    #[test]
    fn a_whitespace_only_line_inside_a_value_contributes_one_newline() {
        let parsed = read("[s]\nkey = one\n  \n    two\nother = x\n");
        assert_eq!(value_of(&parsed, "s", "key"), Some("one\n\ntwo"));
        assert_eq!(value_of(&parsed, "s", "other"), Some("x"));
    }

    #[test]
    fn a_blank_line_closes_the_value_when_empty_lines_are_not_allowed() {
        let options = Options {
            allow_empty_lines_in_values: false,
            ..Options::default()
        };
        let parsed = read_with("[s]\nkey = one\n\n\tother = two\n", &options);
        let section = parsed.ini().section("s").unwrap();
        assert_eq!(section.keys().collect::<Vec<_>>(), vec!["key", "other"]);
        assert_eq!(value_of(&parsed, "s", "key"), Some("one"));
        assert_eq!(value_of(&parsed, "s", "other"), Some("two"));
    }

    #[test]
    fn the_default_section_populates_the_defaults() {
        let source = "[DEFAULT]\nd = 1\n[s]\nkey = value\n";
        let parsed = read(source);

        assert_eq!(
            parsed.ini().section_names().collect::<Vec<_>>(),
            vec!["s"],
            "the defaults are not a section"
        );
        assert_eq!(
            parsed.ini().defaults().get("d").map(Entry::as_str),
            Some("1")
        );
        assert_eq!(value_of(&parsed, "s", "d"), Some("1"));
        assert_eq!(
            parsed
                .ini()
                .defaults()
                .header_span()
                .and_then(|span| parsed.text(span)),
            Some("DEFAULT")
        );
    }

    #[test]
    fn the_default_section_name_is_configurable() {
        let options = Options {
            default_section: "COMMON".to_string(),
            ..Options::default()
        };
        let parsed = read_with("[COMMON]\nd = 1\n[DEFAULT]\ne = 2\n[s]\n", &options);
        assert_eq!(
            parsed.ini().section_names().collect::<Vec<_>>(),
            vec!["DEFAULT", "s"]
        );
        assert_eq!(
            parsed.ini().defaults().get("d").map(Entry::as_str),
            Some("1")
        );
    }

    /// Section names are compared exactly, the default section's included, so `[default]`
    /// is an ordinary section rather than a differently spelled `[DEFAULT]`.
    #[test]
    fn the_default_section_name_is_matched_exactly() {
        let parsed = read("[default]\nx = 1\n");
        assert_eq!(
            parsed.ini().section_names().collect::<Vec<_>>(),
            vec!["default"]
        );
        assert!(parsed.ini().defaults().is_empty());
        assert_eq!(value_of(&parsed, "default", "x"), Some("1"));
    }

    /// An option before any header has nowhere to go, so it is reported and discarded
    /// rather than quietly promoted to a global default.
    #[test]
    fn an_option_before_any_header_is_reported_and_discarded() {
        let source = "stray = 1\n[s]\nown = 2\n";
        let parsed = read(source);

        assert_eq!(kinds(&parsed), vec![ProblemKind::MissingSectionHeader]);
        assert!(parsed.has_errors());
        assert!(parsed.ini().defaults().is_empty());
        assert_eq!(value_of(&parsed, "s", "stray"), None);
        assert_eq!(value_of(&parsed, "s", "own"), Some("2"));
        assert_eq!(
            parsed.problems().first().map(|problem| problem.span),
            Some(Span::new(0, 5))
        );
    }

    #[test]
    fn a_repeated_header_merges_and_keeps_the_first_span() {
        let source = "[a]\nx = 1\n[b]\ny = 2\n[a]\nz = 3\n";
        let parsed = read(source);

        assert_eq!(
            parsed.ini().section_names().collect::<Vec<_>>(),
            vec!["a", "b"]
        );
        let section = parsed.ini().section("a").unwrap();
        assert_eq!(section.keys().collect::<Vec<_>>(), vec!["x", "z"]);
        assert_eq!(section.header_span(), Some(Span::new(1, 1)));

        assert_eq!(
            kinds(&parsed),
            vec![ProblemKind::DuplicateSection {
                name: "a".to_string(),
                previous: Some(Span::new(1, 1)),
            }]
        );
        assert_eq!(
            parsed.problems().first().map(|problem| problem.severity),
            Some(Severity::Warning)
        );
        // The problem is reported at the *later* header, not at the one that survived.
        assert_eq!(parsed.problems()[0].span, Span::new(21, 1));
        assert_eq!(parsed.text(parsed.problems()[0].span), Some("a"));
    }

    #[test]
    fn a_repeated_default_header_is_reported_once() {
        let parsed = read("[DEFAULT]\na = 1\n[DEFAULT]\nb = 2\n");
        assert_eq!(
            kinds(&parsed),
            vec![ProblemKind::DuplicateSection {
                name: "DEFAULT".to_string(),
                previous: Some(Span::new(1, 7)),
            }]
        );
        assert_eq!(parsed.problems()[0].span, Span::new(17, 7));
        assert_eq!(parsed.text(parsed.problems()[0].span), Some("DEFAULT"));
        assert_eq!(
            parsed.ini().defaults().keys().collect::<Vec<_>>(),
            vec!["a", "b"]
        );
    }

    /// The last occurrence wins in both modes; strictness changes only the severity.
    #[test]
    fn the_last_duplicate_option_wins_in_both_modes() {
        let source = "[a]\nfoo = 1\nfoo = 2\n";
        for strict in [false, true] {
            let options = Options {
                strict,
                ..Options::default()
            };
            let parsed = read_with(source, &options);
            assert_eq!(value_of(&parsed, "a", "foo"), Some("2"), "strict={strict}");
            assert_eq!(
                kinds(&parsed),
                vec![ProblemKind::DuplicateOption {
                    key: "foo".to_string(),
                    previous: Some(Span::new(4, 3)),
                }]
            );
            assert_eq!(parsed.has_errors(), strict);
        }
    }

    /// A continuation must never splice onto an occurrence the parser rejected — that
    /// produced a value which appeared nowhere in the source.
    #[test]
    fn a_continuation_follows_the_occurrence_it_was_written_under() {
        let source = "[s]\nkey = first\nkey = second\n    continued\n";
        for strict in [false, true] {
            let options = Options {
                strict,
                ..Options::default()
            };
            let parsed = read_with(source, &options);
            assert_eq!(
                value_of(&parsed, "s", "key"),
                Some("second\ncontinued"),
                "strict={strict}"
            );
        }
    }

    /// Recovery is `continue`: one bad line says nothing about the next.
    #[test]
    fn every_malformed_line_is_reported() {
        let parsed = read("[unclosed\nno delimiter here\n[also unclosed\n= 1\n");
        assert_eq!(parsed.problems().len(), 4);
        assert!(parsed.has_errors());
        assert!(parsed.ini().is_empty());
    }

    #[test]
    fn parsing_continues_after_a_malformed_line() {
        let parsed = read("[s]\nbroken\nk = 1\n");
        assert_eq!(parsed.problems().len(), 1);
        assert_eq!(value_of(&parsed, "s", "k"), Some("1"));
    }

    /// A malformed line closes the open option, so neither its own text nor anything
    /// indented under it joins the value: `   more` is a second problem, not a continuation.
    #[test]
    fn a_malformed_line_closes_the_open_option() {
        let parsed = read("[s]\nk = 1\nbroken\n   more\n");
        assert_eq!(value_of(&parsed, "s", "k"), Some("1"));
        assert_eq!(
            parsed
                .ini()
                .get("s", "k")
                .and_then(|entry| entry.value.span),
            Some(Span::new(8, 1))
        );
        assert_eq!(parsed.problems().len(), 2);
    }

    #[test]
    fn into_result_is_ok_without_errors_and_carries_everything_back_with_them() {
        let clean = read("[s]\nk = 1\n");
        assert!(clean.into_result().is_ok());

        let broken = read("[s]\nbroken\n");
        let returned = broken.into_result().unwrap_err();
        assert_eq!(returned.problems().len(), 1);
        assert_eq!(returned.source(), "[s]\nbroken\n");
        assert!(returned.ini().has_section("s"));
        assert_eq!(returned.to_string(), "1 problem(s) in the configuration");

        // The count is the real one, not "at least one".
        assert_eq!(
            read("[unclosed\nno delimiter here\n[also unclosed\n= 1\n").to_string(),
            "4 problem(s) in the configuration"
        );
    }

    /// A warning does not make the result unusable.
    #[test]
    fn a_warning_alone_does_not_fail_into_result() {
        let parsed = read("[s]\nk = 1\nk = 2\n");
        assert_eq!(parsed.problems().len(), 1);
        assert!(parsed.into_result().is_ok());
    }

    #[test]
    fn invalid_options_are_reported_before_any_line_is_read() {
        let options = Options {
            assignment_delimiters: vec![],
            ..Options::default()
        };
        let error = parse("[s]\nk = 1\n", &options).unwrap_err();
        assert!(matches!(error, Error::InvalidOptions(_)), "{error}");
    }

    #[test]
    fn parse_reader_agrees_with_parse() {
        let source = "[s]\nkey = value\n\tcontinued\n";
        let from_str = read(source);
        let from_reader =
            parse_reader(std::io::Cursor::new(source.as_bytes()), &Options::default()).unwrap();

        assert_eq!(from_reader.source(), from_str.source());
        assert_eq!(from_reader.ini(), from_str.ini());
        assert_eq!(from_reader.problems(), from_str.problems());
    }

    #[test]
    fn parse_reader_reports_invalid_utf8_as_io() {
        let error =
            parse_reader(std::io::Cursor::new([0xff_u8, 0xfe]), &Options::default()).unwrap_err();
        assert!(matches!(error, Error::Io(_)), "{error}");
    }

    /// `parse` never panics, whatever it is handed.
    #[test]
    fn parsing_is_total() {
        let sources = [
            "",
            "\n",
            "\r",
            "\r\n",
            " ",
            "[",
            "]",
            "[]",
            "[]\nk = 1\n",
            "=",
            "= 1",
            "k",
            "k =",
            "k = 1",
            "[s]",
            "[s]\n",
            "[s]\n  \n",
            "[s]\nk = 1",
            "[s]\nk = 1\n\n\n\n\n\n\n\n\n\n",
            "[s]\nk = 1\n    \n    \n",
            "\u{a0}k = 1\n",
            "k = 1\u{a0}\n",
            "[s]\nfoo # bar = 1\n",
            "[café]\ngröße = 1\n  日本語\n",
            "[s]\nk = a\n  \n  b\n  \n",
            "[DEFAULT]\n[DEFAULT]\n",
            "[s]\n  [t]\n",
        ];
        let option_sets = [
            Options::default(),
            Options {
                strict: true,
                inline_comment_prefixes: vec!["#".to_string()],
                allow_empty_lines_in_values: false,
                allow_brackets_in_section_name: false,
                ..Options::default()
            },
        ];
        for source in sources {
            for options in &option_sets {
                let parsed = parse(source, options).unwrap();
                assert_eq!(parsed.source(), source);

                // Whatever it decided, every span it kept must still slice this source.
                let sections = parsed.ini().section_names().collect::<Vec<_>>();
                let entries = parsed.ini().defaults().iter().chain(
                    sections
                        .iter()
                        .filter_map(|name| parsed.ini().section(name))
                        .flat_map(SectionView::iter),
                );
                for (_, entry) in entries {
                    assert!(parsed.text_of(&entry.key).is_some(), "{source:?}");
                    assert!(parsed.text_of(&entry.value).is_some(), "{source:?}");
                }
                for problem in parsed.problems() {
                    assert!(parsed.text(problem.span).is_some(), "{source:?}");
                }
                for name in sections {
                    let header = parsed
                        .ini()
                        .section(name)
                        .and_then(SectionView::header_span);
                    assert_eq!(header.and_then(|span| parsed.text(span)), Some(name));
                }
            }
        }
    }
}
