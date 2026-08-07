//! The core invariant of this crate: every span it reports must slice the text it stores.
//!
//! For a section name, an option key or a single-line value, `source[span]` must equal the
//! stored string exactly. For a value built from several lines the stored string cannot be
//! one contiguous slice — indentation and inline comments are gone — so the weaker rule
//! applies: the span starts at the first fragment that contributed text and ends at the
//! last. A span that points anywhere else is worse than no span at all, because it makes a
//! diagnostic underline the wrong bytes.

use serde_ini_spanned::{Entry, Ini, Options, Section, Span, Spanned, parse};

/// Records every way `spanned` can disagree with the source it claims to come from.
fn check(source: &str, what: &str, spanned: &Spanned<String>, out: &mut Vec<String>) {
    let held = spanned.as_str();
    let Some(span) = spanned.span else {
        out.push(format!("{what} has no span but holds {held:?}"));
        return;
    };
    let (start, end) = (span.start(), span.end());

    if end > source.len() {
        out.push(format!(
            "{what} span {start}..{end} runs past the end of the {} byte source",
            source.len()
        ));
        return;
    }
    let Some(sliced) = span.slice(source) else {
        out.push(format!(
            "{what} span {start}..{end} does not land on char boundaries but holds {held:?}"
        ));
        return;
    };

    // A value built from several lines holds the joined text of all of them with the
    // indentation and any inline comments stripped, so only its ends are pinned.
    if held.contains('\n') {
        let first = held.split('\n').next().unwrap_or_default();
        let last = held.rsplit('\n').next().unwrap_or_default();
        if !sliced.starts_with(first) {
            out.push(format!(
                "{what} span {start}..{end} slices {sliced:?} which does not start with {first:?}"
            ));
        }
        if !sliced.ends_with(last) {
            out.push(format!(
                "{what} span {start}..{end} slices {sliced:?} which does not end with {last:?}"
            ));
        }
        return;
    }

    if sliced != held {
        out.push(format!(
            "{what} span {start}..{end} slices {sliced:?} but holds {held:?}"
        ));
    }
}

fn check_entry(source: &str, section: &str, entry: &Entry, out: &mut Vec<String>) {
    let key = entry.key.as_str();
    check(source, &format!("[{section}] key '{key}'"), &entry.key, out);
    check(
        source,
        &format!("[{section}] value of '{key}'"),
        &entry.value,
        out,
    );
}

/// Collects every span in `ini` that disagrees with `source`.
///
/// All mismatches are collected instead of failing on the first one, so a regression shows
/// its whole blast radius at once.
fn span_mismatches(source: &str, ini: &Ini) -> Vec<String> {
    let mut out = Vec::new();

    for (_, entry) in ini.defaults().iter() {
        check_entry(source, "<defaults>", entry, &mut out);
    }

    for name in ini.section_names() {
        let Some(view) = ini.section(name) else {
            out.push(format!(
                "section '{name}' is listed but cannot be looked up"
            ));
            continue;
        };
        match view.header_span() {
            Some(span) => check(
                source,
                &format!("section '{name}'"),
                &Spanned::from_source(span, name.to_string()),
                &mut out,
            ),
            None => out.push(format!("section '{name}' has no header span")),
        }
        for (key, entry) in view.iter() {
            // An inherited default was already checked above, with the defaults.
            if ini.defaults().get(key) == Some(entry) {
                continue;
            }
            check_entry(source, name, entry, &mut out);
        }
    }

    out
}

#[track_caller]
fn assert_spans_slice_source(source: &str, ini: &Ini) {
    let mismatches = span_mismatches(source, ini);
    assert!(
        mismatches.is_empty(),
        "{} span(s) disagree with the source:\n{}",
        mismatches.len(),
        mismatches.join("\n"),
    );
}

/// The options `cfgparser.2.ini` is written for.
fn cfgparser_2_options() -> Options {
    Options {
        comment_prefixes: [";", "#", "----", "//"].map(String::from).to_vec(),
        inline_comment_prefixes: vec!["//".to_string()],
        allow_empty_lines_in_values: false,
        ..Options::default()
    }
}

/// The options `cfgparser.3.ini` documents in its own leading comment block.
fn cfgparser_3_options() -> Options {
    Options {
        comment_prefixes: [";", "#"].map(String::from).to_vec(),
        inline_comment_prefixes: vec!["#".to_string()],
        allow_empty_lines_in_values: true,
        ..Options::default()
    }
}

/// `cfgparser.0.ini` used to report exactly one wrong span: `workgroup` is followed by a
/// blank line and then a block of comment lines, and every one of them pushed the value's
/// span end forward. Blank lines are now buffered and only flushed by a line that actually
/// contributes text, so a trailing run of them never reaches the span at all.
#[test]
fn spans_slice_source_for_cfgparser_0() {
    let source = include_str!("../test-data/cfgparser.0.ini");
    let ini = parse(source, &Options::default()).unwrap().into_ini();
    assert_eq!(span_mismatches(source, &ini), Vec::<String>::new());
}

#[test]
fn spans_slice_source_for_cfgparser_1() {
    let source = include_str!("../test-data/cfgparser.1.ini");
    assert_spans_slice_source(
        source,
        &parse(source, &Options::default()).unwrap().into_ini(),
    );
}

#[test]
fn spans_slice_source_for_cfgparser_2() {
    let source = include_str!("../test-data/cfgparser.2.ini");
    assert_spans_slice_source(
        source,
        &parse(source, &cfgparser_2_options()).unwrap().into_ini(),
    );
}

/// The same defect as `cfgparser.0`: `another value` is emptied by an inline comment and is
/// followed by a comment line, so its span end used to drift over the comments after it.
#[test]
fn spans_slice_source_for_cfgparser_3() {
    let source = include_str!("../test-data/cfgparser.3.ini");
    let ini = parse(source, &cfgparser_3_options()).unwrap().into_ini();
    assert_eq!(span_mismatches(source, &ini), Vec::<String>::new());
}

/// Spans after a non-ASCII character on the same line used to be shifted by the number of
/// extra UTF-8 bytes before them, because byte offsets were re-read as char indices.
///
/// This is the only fixture carrying CRLF terminators and non-ASCII whitespace, so the two
/// are asserted present: dropping them from the file would leave the oracle green while
/// silently covering less than it claims.
#[test]
fn spans_slice_source_for_unicode() {
    let source = include_str!("../test-data/unicode.ini");
    assert!(
        source.contains("\r\n"),
        "the fixture must keep its CRLF lines"
    );
    assert!(
        source.contains('\u{a0}') && source.contains('\u{3000}'),
        "the fixture must keep its non-ASCII whitespace"
    );

    let parsed = parse(source, &Options::default()).unwrap();
    assert!(parsed.problems().is_empty(), "{:?}", parsed.problems());

    // Non-ASCII whitespace around a key, its delimiter and its value is whitespace, and a
    // CRLF terminator is no part of the value it ends.
    let entry = parsed.ini().get("crlf", "schlüssel").unwrap();
    assert_eq!(entry.as_str(), "wert");
    assert_eq!(parsed.text_of(&entry.key), Some("schlüssel"));
    assert_eq!(parsed.text_of(&entry.value), Some("wert"));

    assert_spans_slice_source(source, parsed.ini());
}

/// The minimal form of the drift the two fixtures above carried.
#[test]
fn a_value_span_stops_at_the_text_it_holds() {
    let source = "[s]\nkey = value\n\n# a comment\n\nother = 2\n";
    let parsed = parse(source, &Options::default()).unwrap();
    let entry = parsed.ini().get("s", "key").unwrap();

    assert_eq!(entry.as_str(), "value");
    assert_eq!(parsed.text_of(&entry.value), Some("value"));
    assert_eq!(entry.value.span, Some(Span::new(10, 5)));
    assert_spans_slice_source(source, parsed.ini());
}

/// An option before the first header is reported and discarded; the spans of everything
/// after it, including the report's own span, still slice the source.
#[test]
fn spans_slice_source_across_a_reported_preamble() {
    let source = "before = a value\n\n[section]\nafter = another value\n  continued here\n";
    let parsed = parse(source, &Options::default()).unwrap();

    assert_eq!(parsed.problems().len(), 1);
    for problem in parsed.problems() {
        assert_eq!(parsed.text(problem.span), Some("before"));
    }
    assert_spans_slice_source(source, parsed.ini());
    assert_eq!(
        parsed.ini().get("section", "after").map(Entry::as_str),
        Some("another value\ncontinued here")
    );
}

/// Guards the oracle itself: one mismatch is reported for each kind of wrong span, and a
/// bad span does not hide the ones after it.
#[test]
fn oracle_reports_every_wrong_entry_span() {
    let source = "[owner]\nname = ada\nrole = a1b2c3\n";
    let mut ini = parse(source, &Options::default()).unwrap().into_ini();
    assert_spans_slice_source(source, &ini);

    let section = ini.section_mut("owner").unwrap();
    // Off by the newline before the key, and one byte short of the value.
    section.get_mut("role").unwrap().key.span = Some(Span::new(18, 5));
    section.get_mut("role").unwrap().value.span = Some(Span::new(26, 5));
    section.insert(
        Spanned::from_source(Span::new(900, 99), "oob".to_string()),
        Spanned::from_source(Span::new(26, 6), "a1b2c3".to_string()),
    );
    section.insert("synthetic".into(), "no span".into());

    assert_eq!(
        span_mismatches(source, &ini),
        vec![
            "[owner] key 'role' span 18..23 slices \"\\nrole\" but holds \"role\"".to_string(),
            r#"[owner] value of 'role' span 26..31 slices "a1b2c" but holds "a1b2c3""#.to_string(),
            "[owner] key 'oob' span 900..999 runs past the end of the 33 byte source".to_string(),
            "[owner] key 'synthetic' has no span but holds \"synthetic\"".to_string(),
            "[owner] value of 'synthetic' has no span but holds \"no span\"".to_string(),
        ],
    );
}

/// A span that does not land on a `char` boundary is reported, not sliced.
#[test]
fn oracle_reports_a_span_that_splits_a_character() {
    let source = "[s]\ncafé = 1\n";
    let mut ini = parse(source, &Options::default()).unwrap().into_ini();
    assert_spans_slice_source(source, &ini);

    // `é` occupies bytes 6..8, so a four byte key span ends inside it.
    ini.section_mut("s")
        .unwrap()
        .get_mut("café")
        .unwrap()
        .key
        .span = Some(Span::new(4, 4));

    assert_eq!(
        span_mismatches(source, &ini),
        vec![
            "[s] key 'café' span 4..8 does not land on char boundaries but holds \"café\""
                .to_string(),
        ],
    );
}

/// A section that was never parsed from source has no header to point at, and the oracle
/// says so rather than inventing `0..0`.
#[test]
fn oracle_reports_a_section_without_a_header_span() {
    let mut ini = Ini::default();
    ini.insert_section("owner", Section::default());
    assert_eq!(
        span_mismatches("[owner]\n", &ini),
        vec!["section 'owner' has no header span".to_string()],
    );
}

/// A header span belongs to the source it was parsed from; checked against another one it
/// is reported rather than silently sliced.
#[test]
fn oracle_reports_a_header_span_from_another_source() {
    let ini = parse("[beta]\n", &Options::default()).unwrap().into_ini();
    // The same document, indented by two: every offset in it has moved.
    assert_eq!(
        span_mismatches("  [beta]\n", &ini),
        vec![r#"section 'beta' span 1..5 slices " [be" but holds "beta""#.to_string()],
    );
}
