//! Regression tests for the defects this crate used to have.
//!
//! Every test here pinned the *wrong* behavior on purpose while the defect was live. Each
//! one is now inverted: it asserts the correct behavior, and its doc comment records what
//! used to happen instead, so a reappearance of the old behavior fails a named test rather
//! than passing silently.

use std::collections::HashSet;

use serde_ini_spanned::{Entry, Options, Parsed, ProblemKind, Severity, parse};

fn kinds(parsed: &Parsed) -> Vec<&ProblemKind> {
    parsed
        .problems()
        .iter()
        .map(|problem| &problem.kind)
        .collect()
}

fn inline(prefixes: &[&str]) -> Options {
    Options {
        inline_comment_prefixes: prefixes.iter().map(|p| (*p).to_string()).collect(),
        ..Options::default()
    }
}

/// A leading non-breaking space is just leading whitespace.
///
/// Was: leading whitespace was counted in `char`s and the count used as a *byte* offset,
/// so the index landed inside the two-byte `\u{a0}` and slicing panicked with
/// `start byte index 1 is not a char boundary`.
#[test]
fn a_leading_non_breaking_space_is_just_whitespace() {
    let source = "[s]\n\u{a0}k = 1\n";
    let parsed = parse(source, &Options::default()).unwrap();

    assert!(parsed.problems().is_empty());
    let entry = parsed.ini().get("s", "k").unwrap();
    assert_eq!(entry.as_str(), "1");
    assert_eq!(parsed.text_of(&entry.key), Some("k"));
    assert_eq!(parsed.text_of(&entry.value), Some("1"));

    // Without a section header the option is reported and discarded, but still not a panic.
    let bare = parse("\u{a0}k = 1\n", &Options::default()).unwrap();
    assert_eq!(kinds(&bare), vec![&ProblemKind::MissingSectionHeader]);
    assert_eq!(bare.text(bare.problems()[0].span), Some("k"));
}

/// A trailing non-breaking space is trailing whitespace and is simply trimmed.
///
/// Was: trailing whitespace was counted in `char`s and subtracted from a byte index, so the
/// span end landed inside the two-byte `\u{a0}` and slicing panicked with
/// `end byte index 6 is not a char boundary`.
#[test]
fn a_trailing_non_breaking_space_is_just_whitespace() {
    let source = "[s]\nk = 1\u{a0}\n";
    let parsed = parse(source, &Options::default()).unwrap();

    assert!(parsed.problems().is_empty());
    let entry = parsed.ini().get("s", "k").unwrap();
    assert_eq!(entry.as_str(), "1");
    assert_eq!(parsed.text_of(&entry.value), Some("1"));
}

/// With `#` configured as an inline comment prefix, `foo # bar = 1` has its comment
/// stripped first and is then a line with no assignment at all — a recoverable problem.
///
/// Was: the assignment delimiter was searched *before* the inline comment was stripped, so
/// the delimiter was found past the truncated end of the line and the value span was built
/// backwards, panicking on `assertion failed: start <= end` in debug and on a reversed
/// slice in release.
#[test]
fn an_inline_comment_before_an_assignment_delimiter_is_a_recoverable_problem() {
    let source = "[s]\nfoo # bar = 1\n";
    let parsed = parse(source, &inline(&["#"])).unwrap();

    assert!(matches!(
        kinds(&parsed).as_slice(),
        [ProblemKind::MissingAssignmentDelimiter { .. }]
    ));
    assert_eq!(parsed.text(parsed.problems()[0].span), Some("foo"));
    assert!(parsed.ini().section("s").unwrap().is_empty());
}

/// A key span slices the source back to exactly the stored key.
///
/// Was: spans were computed in byte offsets and then run through a char-index-to-byte
/// conversion a second time, so the two-byte `\u{301}` shifted the key span end by one and
/// the reported span swallowed the space after the key.
#[test]
fn a_key_span_survives_a_multi_byte_character() {
    let source = "[s]\nke\u{301} = 1\n";
    let parsed = parse(source, &Options::default()).unwrap();

    let entry = parsed.ini().get("s", "ke\u{301}").unwrap();
    assert_eq!(entry.key.as_str(), "ke\u{301}");
    assert_eq!(parsed.text_of(&entry.key), Some("ke\u{301}"));
    assert_eq!(parsed.text_of(&entry.value), Some("1"));
}

/// Every syntax problem a document has is reported, not just the first.
///
/// Was: parsing bailed out on the first syntax error, returned it as `Err`, and never
/// pushed anything into the caller's diagnostics, so the remaining defects were invisible.
#[test]
fn every_syntax_problem_is_reported() {
    let options = Options {
        allow_brackets_in_section_name: false,
        ..Options::default()
    };

    // Each line on its own is a different syntax error.
    let lines = ["[Foo\n", "[a]b]\n", "= no option name\n", "novalue\n"];
    let mut messages = Vec::new();
    for line in lines {
        let parsed = parse(line, &options).unwrap();
        assert_eq!(parsed.problems().len(), 1, "{line:?}");
        messages.push(parsed.problems()[0].kind.to_string());
    }
    assert_eq!(messages.iter().collect::<HashSet<_>>().len(), 4);

    let parsed = parse(&lines.concat(), &options).unwrap();
    assert_eq!(
        parsed
            .problems()
            .iter()
            .map(|problem| problem.kind.to_string())
            .collect::<Vec<_>>(),
        messages,
        "all four problems survive, in source order"
    );
    assert!(parsed.has_errors());
    assert!(parsed.into_result().is_err());
}

/// `[s] ; c` is a section header followed by a comment, and parses as section `s`.
///
/// Was: the header check required the whole *line* to end with `]`, and comments were only
/// considered for non-header lines, so a trailing comment made the header look unterminated
/// and aborted the parse with `section was not closed: missing ']'`.
#[test]
fn a_section_header_may_carry_a_trailing_comment() {
    for options in [Options::default(), inline(&[";"])] {
        let parsed = parse("[s] ; c\nk = 1\n", &options).unwrap();
        assert!(parsed.problems().is_empty(), "{:?}", parsed.problems());
        assert_eq!(
            parsed.ini().section_names().collect::<Vec<_>>(),
            vec!["s"],
            "the header closes at its own `]`"
        );
        assert_eq!(parsed.ini().get("s", "k").map(Entry::as_str), Some("1"));
    }

    // Trailing text that is not a comment is allowed too, as `configparser`'s greedy
    // header group allows it.
    let parsed = parse("[s] junk\nx = 1\n", &Options::default()).unwrap();
    assert!(parsed.problems().is_empty());
    assert_eq!(parsed.ini().section_names().collect::<Vec<_>>(), vec!["s"]);
}

/// `[]` has no section name and is reported.
///
/// Was: silently accepted as a section literally named `""`, with options attached to it
/// and no diagnostic at all.
#[test]
fn an_empty_section_name_is_reported() {
    let parsed = parse("[]\nk = 1\n", &Options::default()).unwrap();

    // The option after it has no section to land in, so it is reported as well.
    assert_eq!(
        kinds(&parsed),
        vec![
            &ProblemKind::EmptySectionName,
            &ProblemKind::MissingSectionHeader,
        ]
    );
    assert_eq!(parsed.problems()[0].severity, Severity::Error);
    assert_eq!(parsed.text(parsed.problems()[0].span), Some("[]"));
    assert!(!parsed.ini().has_section(""));
    assert_eq!(parsed.ini().section_names().count(), 0);
}

/// With `:=` configured as the assignment delimiter, `a := 3` assigns `3` to `a`.
///
/// Was: the value was taken from one byte past the *start* of the delimiter match instead
/// of past its end, so all but the first byte of a multi-character delimiter leaked into
/// the value and `a` came out as `"= 3"`.
#[test]
fn a_multi_character_assignment_delimiter_does_not_leak_into_the_value() {
    let source = "[s]\na := 3\n";
    let options = Options {
        assignment_delimiters: vec![":=".to_string()],
        ..Options::default()
    };
    let parsed = parse(source, &options).unwrap();

    assert!(parsed.problems().is_empty());
    let entry = parsed.ini().get("s", "a").unwrap();
    assert_eq!(entry.as_str(), "3");
    assert_eq!(parsed.text_of(&entry.value), Some("3"));
}

/// An inline comment prefix only starts a comment when it begins the line or follows
/// whitespace, as in `configparser`, so a URL keeps its fragment.
///
/// Was: any occurrence of the prefix started a comment, so the value was truncated at the
/// `#` and the fragment was silently dropped.
#[test]
fn a_hash_inside_a_url_stays_in_the_value() {
    let source = "[s]\nurl = http://x.com#anchor\n";
    let parsed = parse(source, &inline(&["#"])).unwrap();

    assert!(parsed.problems().is_empty());
    let entry = parsed.ini().get("s", "url").unwrap();
    assert_eq!(entry.as_str(), "http://x.com#anchor");
    assert_eq!(parsed.text_of(&entry.value), Some("http://x.com#anchor"));

    // A prefix that does follow whitespace still starts a comment.
    let parsed = parse("[s]\nurl = http://x.com # note\n", &inline(&["#"])).unwrap();
    assert_eq!(
        parsed.ini().get("s", "url").map(Entry::as_str),
        Some("http://x.com")
    );
}
