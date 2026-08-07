//! What one source line says, decided without looking at any other line.

use crate::fragment::Fragment;
use crate::options::CompiledOptions;
use crate::problem::ProblemKind;
use crate::span::Span;
use aho_corasick::AhoCorasick;

/// What one source line says.
///
/// A line is classified on its own; the only thing the lexer knows about the rest of the
/// document is `indent_baseline` (see [`classify`]), which is why a malformed line is a
/// value rather than an error: nothing about it invalidates the lines around it.
#[derive(Debug, Clone)]
pub(crate) enum Line<'a> {
    /// Empty, whitespace-only, or a full-line comment.
    ///
    /// There is no separate comment variant: Python's `configparser` treats a comment line
    /// exactly like a blank line, including for continuation purposes, and no caller of this
    /// crate can observe the difference.
    Blank,
    /// A section header; the fragment is the name with the brackets excluded and **not**
    /// trimmed, because `configparser` does not trim it either (`[ s ]` names the section
    /// `" s "`).
    SectionHeader(Fragment<'a>),
    /// An assignment, both halves trimmed and with any inline comment already removed.
    Option {
        /// The option name as written, left of the delimiter.
        key: Fragment<'a>,
        /// The value as written, right of the delimiter; empty for `key =`.
        value: Fragment<'a>,
    },
    /// A line indented further than the option it continues, comment-stripped and trimmed.
    Continuation(Fragment<'a>),
    /// A line the parser records as a problem and then skips.
    Malformed {
        /// What is wrong with the line.
        kind: ProblemKind,
        /// The bytes to point the diagnostic at.
        span: Span,
    },
}

/// Classifies one source line.
///
/// `indent_baseline` is `Some` only while an option is open for continuation, and holds
/// that option's own line indent; a line indented further than it continues that option.
///
/// The phases run in this order, and the order is the contract:
///
/// 1. the indent is read from the raw line, before anything is stripped;
/// 2. a comment start is located;
/// 3. the content is everything before it, trimmed — so the delimiter search below
///    structurally cannot see into a comment;
/// 4. empty content is [`Line::Blank`];
/// 5. a line indented past `indent_baseline` is a [`Line::Continuation`], **before** the
///    section-header check, so an indented `[t]` continues the open option as it does in
///    `configparser` rather than opening a section;
/// 6. content starting with `[` is a section header, closed by its **last** `]`, because
///    `configparser`'s `SECTCRE` header group is greedy and trailing text is allowed;
/// 7. otherwise the leftmost assignment delimiter splits the line — with the caller's own
///    list order breaking a tie between two that start at the same offset — and the value
///    begins at the **end** of the delimiter match, so a multi-byte delimiter such as `:=`
///    does not leave its own tail in the value.
///
/// Never panics: every offset comes from `str::find`/`AhoCorasick` on the line's own text,
/// and a match of a UTF-8 pattern in UTF-8 text always starts and ends on a `char`
/// boundary, so none of the four `subrange` calls below can return `None`; if one ever did,
/// the line degrades to [`Line::Blank`] rather than panicking.
pub(crate) fn classify<'a>(
    line: Fragment<'a>,
    indent_baseline: Option<usize>,
    options: &CompiledOptions,
) -> Line<'a> {
    try_classify(line, indent_baseline, options).unwrap_or(Line::Blank)
}

/// [`classify`], with the coordinate arithmetic still fallible.
///
/// Every `subrange` here is built from a match offset inside the line's own text, so `None`
/// is unreachable; it exists so that no step has to assume that in a way that could panic.
fn try_classify<'a>(
    line: Fragment<'a>,
    indent_baseline: Option<usize>,
    options: &CompiledOptions,
) -> Option<Line<'a>> {
    let indent = line.indent();
    let text = line.text();
    let content_end = comment_start(text, options).unwrap_or(text.len());
    let content = line.subrange(0..content_end)?.trim();

    if content.text().is_empty() {
        return Some(Line::Blank);
    }
    if indent_baseline.is_some_and(|baseline| indent > baseline) {
        return Some(Line::Continuation(content));
    }
    if content.text().starts_with('[') {
        return section_header(content, options);
    }
    assignment(content, options)
}

/// The byte offset into `text` at which a comment begins, if one does.
///
/// A full-line prefix counts only where the line's text begins, and an inline prefix counts
/// only at the start of the line or after a whitespace `char` — which is what keeps the `#`
/// in `url = http://x.com#anchor` part of the value.
fn comment_start(text: &str, options: &CompiledOptions) -> Option<usize> {
    if starts_with_pattern(options.comment_prefixes(), text.trim_start()) {
        return Some(0);
    }
    options
        .inline_comment_prefixes()
        .find_iter(text)
        .find(|found| preceded_by_whitespace(text, found.start()))
        .map(|found| found.start())
}

/// Whether `text` begins with one of `matcher`'s patterns.
///
/// Exactly what `str::starts_with` over the whole pattern list would answer: the matcher is
/// built leftmost-first, so its leftmost match starts at 0 whenever any pattern does.
fn starts_with_pattern(matcher: &AhoCorasick, text: &str) -> bool {
    matcher.find(text).is_some_and(|found| found.start() == 0)
}

/// Whether the byte at `at` starts the line or follows a whitespace `char`.
fn preceded_by_whitespace(text: &str, at: usize) -> bool {
    at == 0
        || text
            .get(..at)
            .and_then(|before| before.chars().next_back())
            .is_some_and(char::is_whitespace)
}

/// Classifies content already known to start with `[`.
fn section_header<'a>(content: Fragment<'a>, options: &CompiledOptions) -> Option<Line<'a>> {
    let Some(close) = content.text().rfind(']') else {
        return Some(Line::Malformed {
            kind: ProblemKind::SectionNotClosed,
            span: content.span(),
        });
    };

    // `close` is at least 1, because byte 0 is the `[` that got us here.
    let name = content.subrange(1..close)?;
    if name.text().is_empty() {
        return Some(Line::Malformed {
            kind: ProblemKind::EmptySectionName,
            span: content.span(),
        });
    }
    if !options.allow_brackets_in_section_name() && name.text().contains(']') {
        return Some(Line::Malformed {
            kind: ProblemKind::InvalidSectionName,
            span: name.span(),
        });
    }
    Some(Line::SectionHeader(name))
}

/// Classifies content that is neither blank, nor a continuation, nor a section header.
fn assignment<'a>(content: Fragment<'a>, options: &CompiledOptions) -> Option<Line<'a>> {
    let text = content.text();
    let Some(delimiter) = options.assignment_delimiters().find(text) else {
        return Some(Line::Malformed {
            kind: ProblemKind::MissingAssignmentDelimiter {
                delimiters: options.delimiter_names().to_vec(),
            },
            span: content.span(),
        });
    };

    let key = content.subrange(0..delimiter.start())?.trim();
    let value = content.subrange(delimiter.end()..text.len())?.trim();
    if key.text().is_empty() {
        return Some(Line::Malformed {
            kind: ProblemKind::EmptyOptionName,
            span: key.span(),
        });
    }
    Some(Line::Option { key, value })
}

#[cfg(test)]
mod tests {
    use super::{Line, classify};
    use crate::fragment::Fragment;
    use crate::options::{CompiledOptions, Options};
    use crate::problem::ProblemKind;
    use std::ops::Range;

    /// The single line of a single-line fixture.
    fn line(source: &str) -> Fragment<'_> {
        let mut lines = Fragment::lines(source);
        let line = lines.next().unwrap();
        assert!(lines.next().is_none(), "fixture must be a single line");
        line
    }

    fn defaults() -> CompiledOptions {
        Options::default().compile().unwrap()
    }

    fn with_inline(prefixes: &[&str]) -> CompiledOptions {
        Options {
            inline_comment_prefixes: prefixes.iter().map(|p| (*p).to_string()).collect(),
            ..Options::default()
        }
        .compile()
        .unwrap()
    }

    #[track_caller]
    fn as_option(classified: &Line<'_>) -> ((String, Range<usize>), (String, Range<usize>)) {
        match classified {
            Line::Option { key, value } => (
                (key.text().to_string(), key.span().into()),
                (value.text().to_string(), value.span().into()),
            ),
            other => panic!("expected an option, got {other:?}"),
        }
    }

    #[track_caller]
    fn as_header(classified: &Line<'_>) -> (String, Range<usize>) {
        match classified {
            Line::SectionHeader(name) => (name.text().to_string(), name.span().into()),
            other => panic!("expected a section header, got {other:?}"),
        }
    }

    #[track_caller]
    fn as_continuation(classified: &Line<'_>) -> (String, Range<usize>) {
        match classified {
            Line::Continuation(text) => (text.text().to_string(), text.span().into()),
            other => panic!("expected a continuation, got {other:?}"),
        }
    }

    #[track_caller]
    fn as_malformed(classified: &Line<'_>) -> (ProblemKind, Range<usize>) {
        match classified {
            Line::Malformed { kind, span } => (kind.clone(), (*span).into()),
            other => panic!("expected a malformed line, got {other:?}"),
        }
    }

    #[track_caller]
    fn assert_blank(classified: &Line<'_>) {
        assert!(
            matches!(classified, Line::Blank),
            "expected a blank line, got {classified:?}"
        );
    }

    #[test]
    fn empty_and_whitespace_only_lines_are_blank() {
        for source in ["   ", "\t\t", "\u{a0}\u{3000}"] {
            assert_blank(&classify(line(source), None, &defaults()));
        }
        // An empty line has no text at all, only a position.
        let empty = Fragment::lines("a\n\nb").nth(1).unwrap();
        assert_blank(&classify(empty, None, &defaults()));
    }

    /// A prefix list in which one pattern is a prefix of another recognizes both.
    #[test]
    fn overlapping_comment_prefixes_are_blank() {
        let options = Options {
            comment_prefixes: vec!["#".to_string(), "##".to_string()],
            ..Options::default()
        }
        .compile()
        .unwrap();
        assert_blank(&classify(line("# x"), None, &options));
        assert_blank(&classify(line("## x"), None, &options));
    }

    #[test]
    fn comment_lines_are_blank() {
        for source in ["; note", "# note", "   ; indented", ";", "#"] {
            assert_blank(&classify(line(source), None, &defaults()));
        }
    }

    /// A multi-character comment prefix used to lose exactly one byte to an off-by-one;
    /// there is no longer any code path that keeps the text of a comment at all.
    #[test]
    fn multi_character_comment_prefixes_are_blank() {
        let options = Options {
            comment_prefixes: vec!["----".to_string(), "//".to_string()],
            ..Options::default()
        }
        .compile()
        .unwrap();
        assert_blank(&classify(line("---- a = 1"), None, &options));
        assert_blank(&classify(line("// a = 1"), None, &options));
        // `;` is no longer configured, so it is an ordinary line again.
        assert!(matches!(
            classify(line("; a = 1"), None, &options),
            Line::Option { .. }
        ));
    }

    /// An inline prefix at the very start of the line makes the whole line a comment, which
    /// is what stops `@ hello` from being a hard error when `@` is an inline prefix.
    #[test]
    fn an_inline_prefix_at_the_start_of_the_line_is_blank() {
        let options = with_inline(&["@"]);
        assert_blank(&classify(line("@ hello"), None, &options));
        assert_blank(&classify(line("   @ hello"), None, &options));
    }

    #[test]
    fn a_simple_assignment() {
        let classified = classify(line("key = value"), None, &defaults());
        assert_eq!(
            as_option(&classified),
            (("key".to_string(), 0..3), ("value".to_string(), 6..11))
        );
    }

    #[test]
    fn both_halves_are_trimmed_and_spans_are_absolute() {
        let source = "[s]\n   key\t=\tvalue   \n";
        let second = Fragment::lines(source).nth(1).unwrap();
        let classified = classify(second, None, &defaults());
        assert_eq!(
            as_option(&classified),
            (("key".to_string(), 7..10), ("value".to_string(), 13..18))
        );
    }

    #[test]
    fn an_empty_value_is_allowed() {
        let classified = classify(line("key ="), None, &defaults());
        let (key, value) = as_option(&classified);
        assert_eq!(key, ("key".to_string(), 0..3));
        assert_eq!(value, (String::new(), 5..5));
    }

    #[test]
    fn a_carriage_return_is_not_part_of_the_value() {
        let source = "key = value\r\n";
        let classified = classify(Fragment::lines(source).next().unwrap(), None, &defaults());
        assert_eq!(as_option(&classified).1, ("value".to_string(), 6..11));
    }

    /// With no inline prefixes configured — the default — a `#` is ordinary value text.
    #[test]
    fn a_hash_stays_in_the_value_by_default() {
        let classified = classify(line("a = 3 # test"), None, &defaults());
        assert_eq!(as_option(&classified).1, ("3 # test".to_string(), 4..12));
    }

    #[test]
    fn an_inline_comment_is_stripped_from_the_value() {
        let classified = classify(line("a = 3 # test"), None, &with_inline(&["#"]));
        assert_eq!(as_option(&classified).1, ("3".to_string(), 4..5));
    }

    /// An inline prefix counts only after whitespace, so a URL fragment survives.
    #[test]
    fn an_inline_prefix_inside_a_word_is_not_a_comment() {
        let classified = classify(
            line("url = http://x.com#anchor"),
            None,
            &with_inline(&["#"]),
        );
        assert_eq!(
            as_option(&classified).1,
            ("http://x.com#anchor".to_string(), 6..25)
        );
    }

    #[test]
    fn the_first_qualifying_inline_prefix_wins() {
        // The `#` inside the word does not qualify, so the second one starts the comment.
        let classified = classify(line("a = b#c # note"), None, &with_inline(&["#"]));
        assert_eq!(as_option(&classified).1, ("b#c".to_string(), 4..7));

        // With two qualifying occurrences, the value stops at the earlier one.
        let classified = classify(line("a = b # c # d"), None, &with_inline(&["#"]));
        assert_eq!(as_option(&classified).1, ("b".to_string(), 4..5));
    }

    /// The confirmed panic: the delimiter sat inside the comment, and the code sliced the
    /// value out of a range that no longer existed. Stripping the comment first removes the
    /// site entirely — this line simply has no delimiter.
    #[test]
    fn a_delimiter_inside_an_inline_comment_is_invisible() {
        let classified = classify(line("foo # bar = 1"), None, &with_inline(&["#"]));
        let (kind, span) = as_malformed(&classified);
        assert!(matches!(
            kind,
            ProblemKind::MissingAssignmentDelimiter { .. }
        ));
        assert_eq!(span, 0..3);
    }

    /// The value starts at the **end** of the delimiter match, not one byte past its start.
    #[test]
    fn a_multi_byte_delimiter_does_not_leak_into_the_value() {
        let options = Options {
            assignment_delimiters: vec![":=".to_string()],
            ..Options::default()
        }
        .compile()
        .unwrap();
        let classified = classify(line("a := 3"), None, &options);
        assert_eq!(
            as_option(&classified),
            (("a".to_string(), 0..1), ("3".to_string(), 5..6))
        );
    }

    /// When two delimiters start at the same offset the caller's list order decides, in both
    /// directions, exactly as `configparser`'s regex alternation does. Matching whichever
    /// candidate *ends* soonest instead would make `["==", "="]` unusable, because `=` would
    /// always win and leak the second `=` into every value.
    #[test]
    fn overlapping_delimiters_follow_the_order_the_caller_listed() {
        let with_delimiters = |delimiters: &[&str]| {
            Options {
                assignment_delimiters: delimiters.iter().map(|d| (*d).to_string()).collect(),
                ..Options::default()
            }
            .compile()
            .unwrap()
        };

        let classified = classify(line("a == b"), None, &with_delimiters(&["==", "="]));
        assert_eq!(as_option(&classified).1, ("b".to_string(), 5..6));

        let classified = classify(line("a == b"), None, &with_delimiters(&["=", "=="]));
        assert_eq!(as_option(&classified).1, ("= b".to_string(), 3..6));
    }

    /// A comment prefix is recognized at offset 0 even when a shorter configured prefix
    /// occurs later in the same word and would end sooner — `m` inside `rem `.
    #[test]
    fn a_comment_prefix_containing_another_prefix_still_starts_a_comment() {
        let options = Options {
            comment_prefixes: vec!["rem ".to_string(), "m".to_string()],
            ..Options::default()
        }
        .compile()
        .unwrap();
        assert_blank(&classify(line("rem this is a comment"), None, &options));
        // The shorter prefix still works where it does begin the line.
        assert_blank(&classify(line("multi"), None, &options));
    }

    #[test]
    fn a_line_without_a_delimiter_reports_the_configured_delimiters() {
        let classified = classify(line("  just words  "), None, &defaults());
        let (kind, span) = as_malformed(&classified);
        assert_eq!(
            kind,
            ProblemKind::MissingAssignmentDelimiter {
                delimiters: vec!["=".to_string(), ":".to_string()],
            }
        );
        assert_eq!(span, 2..12, "the span covers the trimmed content");
    }

    #[test]
    fn an_assignment_without_a_name_is_malformed() {
        let (kind, span) = as_malformed(&classify(line("  = 3"), None, &defaults()));
        assert_eq!(kind, ProblemKind::EmptyOptionName);
        assert_eq!(span, 2..2, "an empty key still points where it would be");
    }

    #[test]
    fn a_section_header() {
        assert_eq!(
            as_header(&classify(line("[s]"), None, &defaults())),
            ("s".to_string(), 1..2)
        );
        assert_eq!(
            as_header(&classify(line("  [ spaced ]  "), None, &defaults())).1,
            3..11
        );
    }

    /// `configparser` does not trim the header group, so `[ s ]` is a section named `" s "`.
    #[test]
    fn a_section_name_is_not_trimmed() {
        assert_eq!(
            as_header(&classify(line("[ s ]"), None, &defaults())),
            (" s ".to_string(), 1..4)
        );
        assert_eq!(
            as_header(&classify(
                line("[Section\\with$weird%characters[\t]"),
                None,
                &defaults()
            )),
            ("Section\\with$weird%characters[\t".to_string(), 1..32)
        );
    }

    /// The header group is greedy: the **last** `]` closes it, so trailing text is allowed.
    #[test]
    fn a_section_header_closes_at_the_last_bracket() {
        assert_eq!(
            as_header(&classify(line("[a]b]"), None, &defaults())),
            ("a]b".to_string(), 1..4)
        );
        assert_eq!(
            as_header(&classify(line("[s] junk"), None, &defaults())),
            ("s".to_string(), 1..2)
        );
    }

    /// A comment after a header is stripped before the brackets are looked at, so the
    /// header still closes even though the line does not end with `]`.
    #[test]
    fn a_section_header_may_be_followed_by_a_comment() {
        assert_eq!(
            as_header(&classify(line("[s] ; c"), None, &with_inline(&[";", "#"]))),
            ("s".to_string(), 1..2)
        );
        assert_eq!(
            as_header(&classify(line("[s] # c"), None, &with_inline(&[";", "#"]))),
            ("s".to_string(), 1..2)
        );
    }

    #[test]
    fn a_bracket_in_a_section_name_can_be_rejected() {
        let options = Options {
            allow_brackets_in_section_name: false,
            ..Options::default()
        }
        .compile()
        .unwrap();
        let (kind, span) = as_malformed(&classify(line("[a]b]"), None, &options));
        assert_eq!(kind, ProblemKind::InvalidSectionName);
        assert_eq!(span, 1..4, "the span covers the name, not the whole line");
    }

    #[test]
    fn an_unclosed_section_header_is_malformed() {
        let (kind, span) = as_malformed(&classify(line("  [section  "), None, &defaults()));
        assert_eq!(kind, ProblemKind::SectionNotClosed);
        assert_eq!(span, 2..10);
    }

    /// `[]` is a header with nothing in it, which used to be accepted as a section whose
    /// name is the empty string.
    #[test]
    fn an_empty_section_name_is_malformed() {
        let (kind, span) = as_malformed(&classify(line("[]"), None, &defaults()));
        assert_eq!(kind, ProblemKind::EmptySectionName);
        assert_eq!(span, 0..2);
    }

    /// A single `]` never opens a header, so it is an ordinary line.
    #[test]
    fn a_line_that_does_not_start_with_a_bracket_is_not_a_header() {
        let classified = classify(line("a = [b]"), None, &defaults());
        assert_eq!(as_option(&classified).1, ("[b]".to_string(), 4..7));
    }

    #[test]
    fn an_indented_line_continues_an_open_option() {
        let source = "a = 1\n    more text\n";
        let second = Fragment::lines(source).nth(1).unwrap();
        assert_eq!(
            as_continuation(&classify(second, Some(0), &defaults())),
            ("more text".to_string(), 10..19)
        );
    }

    /// A continuation is decided before the section-header check, so an indented `[t]`
    /// extends the open value instead of opening a section, exactly as in `configparser`.
    #[test]
    fn a_continuation_beats_a_section_header() {
        assert_eq!(
            as_continuation(&classify(line("  [t]"), Some(0), &defaults())),
            ("[t]".to_string(), 2..5)
        );
        assert_eq!(
            as_header(&classify(line("[t]"), Some(0), &defaults())),
            ("t".to_string(), 1..2)
        );
    }

    #[test]
    fn a_continuation_needs_more_indent_than_its_option() {
        // Equal indent closes the option instead of continuing it.
        let classified = classify(line("    b = 2"), Some(4), &defaults());
        assert_eq!(as_option(&classified).0, ("b".to_string(), 4..5));

        // And a deeper indent continues it even when it looks like an assignment.
        assert_eq!(
            as_continuation(&classify(line("     b = 2"), Some(4), &defaults())),
            ("b = 2".to_string(), 5..10)
        );
    }

    /// Without an open option there is no baseline, so indentation means nothing.
    #[test]
    fn indentation_without_an_open_option_is_not_a_continuation() {
        let (kind, _) = as_malformed(&classify(line("    more text"), None, &defaults()));
        assert!(matches!(
            kind,
            ProblemKind::MissingAssignmentDelimiter { .. }
        ));
    }

    /// The blank check runs first, so an indented empty or comment line never becomes a
    /// continuation carrying an empty fragment.
    #[test]
    fn blankness_is_decided_before_continuation() {
        assert_blank(&classify(line("      "), Some(0), &defaults()));
        assert_blank(&classify(line("      ; note"), Some(0), &defaults()));
    }

    #[test]
    fn a_continuation_is_trimmed_and_comment_stripped() {
        let classified = classify(line("    text  # note"), Some(0), &with_inline(&["#"]));
        assert_eq!(as_continuation(&classified), ("text".to_string(), 4..8));
    }

    /// Non-ASCII whitespace is a byte count in a span and a character count nowhere, which
    /// is the pair of confirmed panics this fixture stands for.
    #[test]
    fn non_ascii_whitespace_does_not_shift_spans() {
        let source = "\u{a0}\u{3000}café\u{3000}=\u{a0}valeur\u{a0}\u{3000}";
        let classified = classify(line(source), None, &defaults());
        let ((key, key_span), (value, value_span)) = as_option(&classified);
        assert_eq!(key, "café");
        assert_eq!(value, "valeur");
        assert_eq!(source.get(key_span), Some("café"));
        assert_eq!(source.get(value_span), Some("valeur"));
    }

    #[test]
    fn non_ascii_whitespace_counts_as_indent() {
        // U+3000 is three bytes, so this line is indented by three.
        assert_eq!(
            as_continuation(&classify(line("\u{3000}a = 1"), Some(2), &defaults())),
            ("a = 1".to_string(), 3..8)
        );
        assert!(matches!(
            classify(line("\u{3000}a = 1"), Some(3), &defaults()),
            Line::Option { .. }
        ));
    }

    #[test]
    fn a_non_ascii_section_name_keeps_its_bytes() {
        let source = "[größe]";
        let classified = classify(line(source), None, &defaults());
        let (name, span) = as_header(&classified);
        assert_eq!(name, "größe");
        assert_eq!(source.get(span), Some("größe"));
    }

    /// Every fragment a classified line carries must still slice out of the source it came
    /// from — the invariant that no test of the old parser ever asserted.
    #[test]
    fn every_fragment_agrees_with_the_source() {
        let source = concat!(
            "[s]\n",
            "a = 1\n",
            "    continued\n",
            "; comment\n",
            "\u{a0}b\u{3000}:\u{a0}2 # note\n",
            "[ t ]\n",
            "broken\n",
            "[unclosed\n",
            "[]\n",
            "= 3\n",
        );
        let options = with_inline(&["#"]);
        let mut checked = 0usize;
        for fragment in Fragment::lines(source) {
            let mut check = |part: Fragment<'_>| {
                assert_eq!(part.span().slice(source), Some(part.text()));
                checked += 1;
            };
            match classify(fragment, Some(0), &options) {
                Line::Blank | Line::Malformed { .. } => {}
                Line::SectionHeader(name) | Line::Continuation(name) => check(name),
                Line::Option { key, value } => {
                    check(key);
                    check(value);
                }
            }
        }
        assert!(checked > 0, "the fixture must classify something");
    }

    /// Malformed lines are values, not errors: one line's problem says nothing about the
    /// next, so the parser recovers by skipping.
    #[test]
    fn several_malformed_lines_are_all_reported() {
        let source = "[unclosed\n= 3\nbroken\n[]\n";
        let kinds: Vec<ProblemKind> = Fragment::lines(source)
            .filter_map(|fragment| match classify(fragment, None, &defaults()) {
                Line::Malformed { kind, .. } => Some(kind),
                _ => None,
            })
            .collect();
        assert_eq!(kinds.len(), 4);
        assert_eq!(kinds.first(), Some(&ProblemKind::SectionNotClosed));
        assert_eq!(kinds.get(1), Some(&ProblemKind::EmptyOptionName));
        assert!(matches!(
            kinds.get(2),
            Some(ProblemKind::MissingAssignmentDelimiter { .. })
        ));
        assert_eq!(kinds.get(3), Some(&ProblemKind::EmptySectionName));
    }

    /// A delimiter surrounded by multi-byte text splits on byte offsets that stay on `char`
    /// boundaries, which the old character-count arithmetic did not.
    #[test]
    fn a_delimiter_between_multi_byte_characters() {
        let classified = classify(line("é=é"), None, &defaults());
        assert_eq!(
            as_option(&classified),
            (("é".to_string(), 0..2), ("é".to_string(), 3..5))
        );
    }

    /// `classify` is total: every line of every shape yields a `Line`, never a panic. The
    /// three confirmed panics of the old lexer all lived on inputs of this kind.
    #[test]
    fn classification_is_total() {
        let sources = [
            " ",
            "\t",
            "[",
            "]",
            "[]",
            "[[]]",
            "[]]",
            "]a[",
            "=",
            "==",
            ":=",
            "=:",
            "#",
            ";",
            "##",
            "a",
            "a=",
            "=a",
            "a==b",
            "a = = b",
            "[a",
            "a]",
            "[a]b",
            "[ ]",
            "[\t]",
            "\u{a0}",
            "é",
            "é=é",
            "[é",
            "é]",
            "[é]",
            "\u{3000}=\u{a0}",
            "a#=b",
            "a # = b",
            "# a = b",
            "; [s]",
            "  [t]",
            "\u{a0}[\u{3000}]\u{a0}",
            "----",
            "a=b#c",
            "a=#",
        ];
        let option_sets = [
            defaults(),
            with_inline(&["#", ";"]),
            Options {
                assignment_delimiters: vec![":=".to_string()],
                comment_prefixes: vec!["----".to_string()],
                inline_comment_prefixes: vec!["#".to_string()],
                allow_brackets_in_section_name: false,
                ..Options::default()
            }
            .compile()
            .unwrap(),
        ];
        for source in sources {
            for baseline in [None, Some(0), Some(1), Some(usize::MAX)] {
                for options in &option_sets {
                    let classified = classify(line(source), baseline, options);
                    // Whatever it decided, the fragments it kept must still be real text.
                    match classified {
                        Line::Blank | Line::Malformed { .. } => {}
                        Line::SectionHeader(part) | Line::Continuation(part) => {
                            assert_eq!(part.span().slice(source), Some(part.text()));
                        }
                        Line::Option { key, value } => {
                            assert_eq!(key.span().slice(source), Some(key.text()));
                            assert_eq!(value.span().slice(source), Some(value.text()));
                        }
                    }
                }
            }
        }
    }
}
