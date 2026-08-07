//! Characterization tests for the crate's observable behavior.
//!
//! These pin what a caller sees through the public API: names, ordering, continuation
//! joining, comment handling, the option knobs, and the spans that come back with every
//! value. Where the behavior deliberately changed during the rewrite, the test says what it
//! used to be.

use serde_ini_spanned::{
    Entry, Ini, Options, Parsed, ProblemKind, Section, SectionView, Severity, Span, parse,
    parse_reader,
};

fn section_names(ini: &Ini) -> Vec<&str> {
    ini.section_names().collect()
}

fn keys(view: SectionView<'_>) -> Vec<&str> {
    view.keys().collect()
}

fn items(view: SectionView<'_>) -> Vec<(&str, &str)> {
    view.iter()
        .map(|(key, entry)| (key, entry.as_str()))
        .collect()
}

fn value_of<'a>(ini: &'a Ini, section: &str, option: &str) -> Option<&'a str> {
    ini.get(section, option).map(Entry::as_str)
}

fn messages(parsed: &Parsed) -> Vec<String> {
    parsed
        .problems()
        .iter()
        .map(|problem| problem.kind.to_string())
        .collect()
}

fn with_comment_prefixes(prefixes: &[&str]) -> Options {
    Options {
        comment_prefixes: prefixes.iter().map(|p| (*p).to_string()).collect(),
        ..Options::default()
    }
}

fn with_inline_comment_prefixes(prefixes: &[&str]) -> Options {
    Options {
        inline_comment_prefixes: prefixes.iter().map(|p| (*p).to_string()).collect(),
        ..Options::default()
    }
}

fn strict() -> Options {
    Options {
        strict: true,
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

#[test]
fn section_names_keep_source_order_and_case() {
    let parsed = parse(
        "[Beta]\na = 1\n[alpha]\nb = 2\n[Gamma Delta]\nc = 3\n",
        &Options::default(),
    )
    .unwrap();
    let ini = parsed.ini();

    assert_eq!(section_names(ini), ["Beta", "alpha", "Gamma Delta"]);
    assert!(ini.has_section("Beta"));
    assert!(!ini.has_section("beta"), "section lookup is case-sensitive");
    assert!(ini.section("beta").is_none());
}

#[test]
fn option_keys_are_case_insensitive_and_keep_their_spelling() {
    let parsed = parse(
        "[MixedCase]\nUPPER_Key = Value\nMiXeD = other\n",
        &Options::default(),
    )
    .unwrap();
    let ini = parsed.ini();
    let section = ini.section("MixedCase").unwrap();
    assert_eq!(keys(section), ["upper_key", "mixed"]);

    for lookup in ["UPPER_Key", "upper_key", "UPPER_KEY", "uPPer_kEY"] {
        assert_eq!(
            value_of(ini, "MixedCase", lookup),
            Some("Value"),
            "{lookup}"
        );
        assert!(section.contains(lookup), "{lookup}");
    }

    // Values keep their case; only the lookup is folded.
    assert_eq!(value_of(ini, "MixedCase", "mixed"), Some("other"));

    // The name as the author wrote it survives on the entry.
    let entry = section.get("UPPER_KEY").unwrap();
    assert_eq!(entry.key.as_str(), "UPPER_Key");
    assert_eq!(entry.as_str(), "Value");
}

#[test]
fn option_order_within_a_section_follows_source_order() {
    let parsed = parse(
        "[s]\nzebra = 1\nalpha = 2\nmiddle = 3\n",
        &Options::default(),
    )
    .unwrap();
    let section = parsed.ini().section("s").unwrap();

    assert_eq!(keys(section), ["zebra", "alpha", "middle"]);
    assert_eq!(
        items(section),
        [("zebra", "1"), ("alpha", "2"), ("middle", "3")]
    );
    assert_eq!(section.len(), 3);
}

/// The merge itself is unchanged; only the silence is. A repeated header is now reported.
#[test]
fn a_repeated_section_header_merges_and_is_reported() {
    let parsed = parse(
        "[Foo]\nfirst = 1\n[Bar]\nother = 2\n[Foo]\nsecond = 3\n",
        &Options::default(),
    )
    .unwrap();
    let ini = parsed.ini();

    assert_eq!(section_names(ini), ["Foo", "Bar"]);
    assert_eq!(
        items(ini.section("Foo").unwrap()),
        [("first", "1"), ("second", "3")]
    );
    assert_eq!(items(ini.section("Bar").unwrap()), [("other", "2")]);
    assert_eq!(messages(&parsed), ["duplicate section `Foo`"]);
    assert_eq!(parsed.problems()[0].severity, Severity::Warning);
}

#[test]
fn continuation_lines_are_joined_with_a_single_newline() {
    let parsed = parse(
        "[MySection]\nOption = first line   \n\tsecond line   \n",
        &Options::default(),
    )
    .unwrap();
    assert_eq!(
        value_of(parsed.ini(), "MySection", "Option"),
        Some("first line\nsecond line")
    );
    assert_eq!(keys(parsed.ini().section("MySection").unwrap()), ["option"]);
}

#[test]
fn a_value_that_starts_on_the_continuation_lines_keeps_a_leading_newline() {
    let parsed = parse(
        "[options.packages.find]\nexclude =\n    example*\n    tests*\n",
        &Options::default(),
    )
    .unwrap();
    // The empty text right after `=` is the first line of the value, so joining it with the
    // first continuation line produces a leading newline.
    assert_eq!(
        value_of(parsed.ini(), "options.packages.find", "exclude"),
        Some("\nexample*\ntests*")
    );
}

#[test]
fn trailing_blank_lines_are_not_part_of_a_continuation_value() {
    let parsed = parse(
        "[s]\nvalue = one\n\ttwo\n\n\n[next]\nother = x\n",
        &Options::default(),
    )
    .unwrap();
    assert_eq!(value_of(parsed.ini(), "s", "value"), Some("one\ntwo"));
    assert_eq!(value_of(parsed.ini(), "next", "other"), Some("x"));

    let entry = parsed.ini().get("s", "value").unwrap();
    assert_eq!(
        parsed.text_of(&entry.value),
        Some("one\n\ttwo"),
        "the span stops at the last line that contributed text"
    );
}

#[test]
fn trailing_blank_lines_are_not_part_of_the_last_value_either() {
    let parsed = parse("[s]\nvalue = one\n\ttwo   \n\n\n", &Options::default()).unwrap();
    assert_eq!(value_of(parsed.ini(), "s", "value"), Some("one\ntwo"));
    let entry = parsed.ini().get("s", "value").unwrap();
    assert_eq!(parsed.text_of(&entry.value), Some("one\n\ttwo"));
}

#[test]
fn blank_lines_are_kept_inside_values_when_empty_lines_in_values_is_enabled() {
    let options = Options {
        allow_empty_lines_in_values: true,
        ..Options::default()
    };
    let parsed = parse("[s]\nvalue = one\n\n\tother = two\n", &options).unwrap();

    assert_eq!(keys(parsed.ini().section("s").unwrap()), ["value"]);
    assert_eq!(
        value_of(parsed.ini(), "s", "value"),
        Some("one\n\nother = two"),
        "the blank line contributes a newline and the indented line continues the value"
    );
}

#[test]
fn blank_lines_terminate_values_when_empty_lines_in_values_is_disabled() {
    let options = Options {
        allow_empty_lines_in_values: false,
        ..Options::default()
    };
    let parsed = parse("[s]\nvalue = one\n\n\tother = two\n", &options).unwrap();

    assert_eq!(keys(parsed.ini().section("s").unwrap()), ["value", "other"]);
    assert_eq!(value_of(parsed.ini(), "s", "value"), Some("one"));
    assert_eq!(value_of(parsed.ini(), "s", "other"), Some("two"));

    // A comment line is classified as blank, so it closes the open option too. `CPython`
    // raises `ParsingError` on the line after it; this crate keeps parsing and the
    // continuation simply becomes an option of its own.
    let parsed = parse("[s]\nvalue = one\n# c\n\tother = two\n", &options).unwrap();

    assert_eq!(keys(parsed.ini().section("s").unwrap()), ["value", "other"]);
    assert_eq!(value_of(parsed.ini(), "s", "value"), Some("one"));
    assert!(parsed.problems().is_empty());
}

#[test]
fn full_line_comments_use_both_default_prefixes_and_may_be_indented() {
    let parsed = parse(
        "[s]\n; semicolon comment\n# hash comment\nkey = value\n   ; indented comment\nother = 2\n",
        &Options::default(),
    )
    .unwrap();

    assert_eq!(keys(parsed.ini().section("s").unwrap()), ["key", "other"]);
    assert_eq!(
        value_of(parsed.ini(), "s", "key"),
        Some("value"),
        "an indented comment line does not continue the previous value"
    );
    assert_eq!(value_of(parsed.ini(), "s", "other"), Some("2"));
}

/// A blank line inside a value contributes one newline; a comment line contributes nothing,
/// which is how `configparser` distinguishes the two.
#[test]
fn a_comment_line_inside_a_value_contributes_nothing() {
    let parsed = parse("[s]\nk = one\n\n# note\n\n  two\n", &Options::default()).unwrap();
    assert_eq!(value_of(parsed.ini(), "s", "k"), Some("one\n\n\ntwo"));
}

#[test]
fn comment_prefixes_only_apply_at_the_start_of_a_line_by_default() {
    let parsed = parse(
        "[s]\nkey = value # not a comment\nother = 2 ; neither is this\n",
        &Options::default(),
    )
    .unwrap();
    assert_eq!(
        value_of(parsed.ini(), "s", "key"),
        Some("value # not a comment"),
        "inline comments are off unless inline_comment_prefixes is configured"
    );
    assert_eq!(
        value_of(parsed.ini(), "s", "other"),
        Some("2 ; neither is this")
    );
}

#[test]
fn custom_comment_prefixes_replace_the_default_ones() {
    let parsed = parse(
        "[s]\n// a comment\nkey = value\n# no longer a comment = yes\n",
        &with_comment_prefixes(&["//"]),
    )
    .unwrap();

    assert_eq!(
        keys(parsed.ini().section("s").unwrap()),
        ["key", "# no longer a comment"]
    );
    assert_eq!(value_of(parsed.ini(), "s", "key"), Some("value"));
    assert_eq!(
        value_of(parsed.ini(), "s", "# no longer a comment"),
        Some("yes")
    );
}

#[test]
fn inline_comments_are_stripped_only_for_configured_prefixes() {
    let parsed = parse(
        "[s]\nkey = value # comment\nother = 2 ; kept\n",
        &with_inline_comment_prefixes(&["#"]),
    )
    .unwrap();
    assert_eq!(value_of(parsed.ini(), "s", "key"), Some("value"));
    assert_eq!(value_of(parsed.ini(), "s", "other"), Some("2 ; kept"));
}

#[test]
fn inline_comments_are_stripped_from_continuation_lines_too() {
    let parsed = parse(
        "[s]\nother = that do continue\n  in     # and still have\n  lines  # comments mixed\n",
        &with_inline_comment_prefixes(&["#"]),
    )
    .unwrap();
    assert_eq!(
        value_of(parsed.ini(), "s", "other"),
        Some("that do continue\nin\nlines")
    );
}

#[test]
fn custom_assignment_delimiters_replace_the_default_ones() {
    let options = Options {
        assignment_delimiters: vec!["|".to_string()],
        ..Options::default()
    };
    let parsed = parse("[s]\nkey | value\nother|2\n", &options).unwrap();

    assert_eq!(keys(parsed.ini().section("s").unwrap()), ["key", "other"]);
    assert_eq!(value_of(parsed.ini(), "s", "key"), Some("value"));
    assert_eq!(value_of(parsed.ini(), "s", "other"), Some("2"));
}

/// Every one of these used to abort the parse with a hard `Err`; they are now recorded and
/// skipped, so the rest of the document survives.
#[test]
fn syntax_problems_are_reported_and_recovered_from() {
    let options = Options {
        assignment_delimiters: vec!["|".to_string()],
        ..Options::default()
    };
    let parsed = parse("[s]\nkey = value\n", &options).unwrap();
    assert_eq!(
        messages(&parsed),
        ["variable assignment missing one of: `|`"]
    );

    let parsed = parse("No Section!\n", &Options::default()).unwrap();
    assert_eq!(
        messages(&parsed),
        ["variable assignment missing one of: `=`, `:`"]
    );

    for source in [
        "[Foo]\n=val-without-opt-name\n",
        "[Foo]\n:val-without-opt-name\n",
    ] {
        let parsed = parse(source, &Options::default()).unwrap();
        assert_eq!(messages(&parsed), ["empty option name"], "{source:?}");
    }

    // A rejected header opens no section, so the option after it has nowhere to land and
    // is reported in its own right.
    let parsed = parse("[Foo\nkey = value\n", &Options::default()).unwrap();
    assert_eq!(
        messages(&parsed),
        [
            "section was not closed: missing ']'",
            "option outside of any section",
        ]
    );
    assert!(parsed.ini().is_empty());
}

#[test]
fn brackets_in_section_names_are_allowed_by_default_and_reported_when_disabled() {
    let source = "[This One Has A ] In It]\nforks = spoons\n";
    let parsed = parse(source, &Options::default()).unwrap();
    assert_eq!(section_names(parsed.ini()), ["This One Has A ] In It"]);
    assert!(parsed.problems().is_empty());

    let options = Options {
        allow_brackets_in_section_name: false,
        ..Options::default()
    };
    let parsed = parse(source, &options).unwrap();
    assert_eq!(
        messages(&parsed),
        [
            "invalid section name: contains ']'",
            "option outside of any section",
        ],
        "a rejected header opens no section, so its options have nowhere to land"
    );
    assert_eq!(parsed.problems()[0].severity, Severity::Error);

    // The knob rejects only names that actually contain a `]`; an ordinary header is
    // unaffected.
    let parsed = parse("[s]\nk = 1\n", &options).unwrap();
    assert!(parsed.problems().is_empty());
    assert_eq!(section_names(parsed.ini()), ["s"]);
}

/// Was: an option before the first header was silently promoted to a global default that
/// `has_option` reported in every section and `get` could not retrieve.
#[test]
fn options_before_the_first_section_header_are_reported_and_discarded() {
    let source = "nameD = valueD\n[section1]\nname1 = value1\n[section2]\nname2 = value2\n";
    let parsed = parse(source, &Options::default()).unwrap();
    let ini = parsed.ini();

    assert_eq!(section_names(ini), ["section1", "section2"]);
    assert_eq!(messages(&parsed), ["option outside of any section"]);
    assert_eq!(parsed.problems()[0].severity, Severity::Error);
    assert_eq!(parsed.text(parsed.problems()[0].span), Some("nameD"));

    assert!(ini.defaults().is_empty());
    assert_eq!(value_of(ini, "section1", "named"), None);
    assert_eq!(keys(ini.section("section1").unwrap()), ["name1"]);
}

/// Was: `keys()` and `has_option` saw the defaults while `get` did not, `iter()` yielded
/// them in the opposite order, and a shadowed key appeared twice with two values.
#[test]
fn defaults_are_visible_through_every_accessor_in_one_order() {
    let source = "[DEFAULT]\nnameD = valueD\nshared = from defaults\n[section1]\nname1 = value1\nshared = mine\n";
    let parsed = parse(source, &Options::default()).unwrap();
    let section = parsed.ini().section("section1").unwrap();

    assert_eq!(
        items(section),
        [("name1", "value1"), ("shared", "mine"), ("named", "valueD"),],
        "the section's own options first, then the defaults it does not shadow"
    );
    assert!(section.keys().eq(section.iter().map(|(key, _)| key)));
    assert_eq!(section.len(), 3);

    for key in ["name1", "named", "shared", "missing"] {
        assert_eq!(section.contains(key), section.get(key).is_some(), "{key}");
    }
    assert_eq!(value_of(parsed.ini(), "section1", "named"), Some("valueD"));
    assert_eq!(value_of(parsed.ini(), "section1", "shared"), Some("mine"));
}

/// Was: `[DEFAULT]` was an ordinary section, listed by `section_names()` and inherited by
/// nobody.
#[test]
fn the_default_section_populates_the_inherited_defaults() {
    let parsed = parse(
        "[DEFAULT]\nkey1 = value1\n[other]\nkey2 = value2\n",
        &Options::default(),
    )
    .unwrap();
    let ini = parsed.ini();

    assert_eq!(section_names(ini), ["other"]);
    assert!(!ini.has_section("DEFAULT"));
    assert_eq!(
        ini.defaults().get("key1").map(Entry::as_str),
        Some("value1")
    );
    assert_eq!(value_of(ini, "other", "key1"), Some("value1"));
    assert_eq!(keys(ini.section("other").unwrap()), ["key2", "key1"]);
}

#[test]
fn typed_accessors_parse_decimals_floats_and_the_configparser_booleans() {
    let parsed = parse(
        "[Types]\nint = 42\nnegative = -7\nfloat = 0.44\nword = strange\nboolean = NO\n",
        &Options::default(),
    )
    .unwrap();
    let ini = parsed.ini();

    assert_eq!(ini.get("Types", "int").unwrap().as_int().unwrap().inner, 42);
    assert_eq!(
        ini.get("Types", "negative")
            .unwrap()
            .as_int()
            .unwrap()
            .inner,
        -7
    );
    for (option, expected) in [("float", 0.44), ("int", 42.0)] {
        let parsed_float = ini.get("Types", option).unwrap().as_float().unwrap();
        assert!(
            (parsed_float.inner - expected).abs() < f64::EPSILON,
            "{option} parsed as {}",
            parsed_float.inner
        );
    }
    assert!(
        !ini.get("Types", "boolean")
            .unwrap()
            .as_bool()
            .unwrap()
            .inner
    );

    // A missing option or section is `None` before any conversion happens.
    assert!(ini.get("Types", "no-such-int").is_none());
    assert!(ini.get("No Such Section", "int").is_none());

    assert_eq!(
        ini.get("Types", "word")
            .unwrap()
            .as_int()
            .unwrap_err()
            .to_string(),
        "invalid digit found in string"
    );
    assert_eq!(
        ini.get("Types", "float")
            .unwrap()
            .as_int()
            .unwrap_err()
            .to_string(),
        "invalid digit found in string"
    );
    assert_eq!(
        ini.get("Types", "word")
            .unwrap()
            .as_float()
            .unwrap_err()
            .to_string(),
        "invalid float literal"
    );

    // A typed value keeps the span it was read from.
    let port = ini.get("Types", "int").unwrap().as_int().unwrap();
    assert_eq!(port.span, ini.get("Types", "int").unwrap().value.span);
}

#[test]
fn as_bool_accepts_the_configparser_spellings_case_insensitively() {
    let parsed = parse(
        "[B]\nt1 = 1\nt2 = TRUE\nt3 = True\nt4 = oN\nt5 = yes\nf1 = 0\nf2 = FALSE\nf3 = False\nf4 = oFF\nf5 = nO\n",
        &Options::default(),
    ).unwrap();
    for (key, expected) in [
        ("t1", true),
        ("t2", true),
        ("t3", true),
        ("t4", true),
        ("t5", true),
        ("f1", false),
        ("f2", false),
        ("f3", false),
        ("f4", false),
        ("f5", false),
    ] {
        let entry = parsed.ini().get("B", key).unwrap();
        assert_eq!(entry.as_bool().unwrap().inner, expected, "{key}");
    }
}

#[test]
fn as_bool_rejects_other_values_and_lowercases_them_in_the_error() {
    let parsed = parse(
        "[B]\ne1 = 2\ne2 = Foo\ne3 = -1\ne4 = 0.1\ne5 = FALSE AND MORE\n",
        &Options::default(),
    )
    .unwrap();
    for (key, message) in [
        ("e1", r#"invalid boolean: "2""#),
        ("e2", r#"invalid boolean: "foo""#),
        ("e3", r#"invalid boolean: "-1""#),
        ("e4", r#"invalid boolean: "0.1""#),
        ("e5", r#"invalid boolean: "false and more""#),
    ] {
        let entry = parsed.ini().get("B", key).unwrap();
        assert_eq!(entry.as_bool().unwrap_err().to_string(), message, "{key}");
    }
}

#[test]
fn set_normalizes_the_key_and_returns_the_replaced_entry() {
    let mut ini = parse("[sect]\noption1 = foo\n", &Options::default())
        .unwrap()
        .into_ini();

    assert!(
        ini.set("sect", "OPTION2".into(), "bar".into())
            .unwrap()
            .is_none()
    );
    assert_eq!(keys(ini.section("sect").unwrap()), ["option1", "option2"]);
    assert_eq!(value_of(&ini, "sect", "option2"), Some("bar"));

    let replaced = ini
        .set("sect", "Option1".into(), "splat".into())
        .unwrap()
        .unwrap();
    assert_eq!(replaced.as_str(), "foo");
    assert_eq!(value_of(&ini, "sect", "option1"), Some("splat"));
    assert_eq!(
        keys(ini.section("sect").unwrap()),
        ["option1", "option2"],
        "a redefinition keeps its original position"
    );
}

#[test]
fn set_on_an_unknown_section_reports_no_section_error() {
    let mut ini = parse("[sect]\noption1 = foo\n", &Options::default())
        .unwrap()
        .into_ini();

    assert_eq!(
        ini.set("nope", "a".into(), "b".into())
            .unwrap_err()
            .to_string(),
        r#"missing section: "nope""#
    );
    assert_eq!(
        ini.set("SECT", "a".into(), "b".into())
            .unwrap_err()
            .to_string(),
        r#"missing section: "SECT""#,
        "section lookup stays case-sensitive"
    );
}

#[test]
fn insert_section_normalizes_keys_and_replaces_a_section_in_place() {
    let mut ini = Ini::default();

    assert!(
        ini.insert_section("A", Section::from([("KeyOne".into(), "V1".into())]))
            .is_none()
    );
    assert!(ini.insert_section("B", Section::default()).is_none());
    assert_eq!(section_names(&ini), ["A", "B"]);
    assert_eq!(keys(ini.section("A").unwrap()), ["keyone"]);
    assert_eq!(value_of(&ini, "A", "keyone"), Some("V1"));

    let replaced = ini
        .insert_section("A", Section::from([("KeyTwo".into(), "V2".into())]))
        .unwrap();
    assert_eq!(replaced.keys().collect::<Vec<_>>(), ["keyone"]);
    assert_eq!(
        section_names(&ini),
        ["A", "B"],
        "re-adding an existing section keeps its position"
    );
    assert_eq!(keys(ini.section("A").unwrap()), ["keytwo"]);
}

#[test]
fn remove_section_returns_the_section_once_and_none_afterwards() {
    let mut ini = parse("[one]\na = 1\n[two]\nb = 2\n", &Options::default())
        .unwrap()
        .into_ini();

    let removed = ini.remove_section("one").unwrap();
    assert_eq!(removed.keys().collect::<Vec<_>>(), ["a"]);
    assert_eq!(section_names(&ini), ["two"]);
    assert!(ini.remove_section("one").is_none());
    assert!(ini.remove_section("nope").is_none());
    assert!(!ini.has_section("one"));
}

#[test]
fn remove_option_removes_only_the_sections_own_options() {
    let mut ini = parse(
        "[DEFAULT]\nshared = default value\n[one]\na = 1\nb = 2\n",
        &Options::default(),
    )
    .unwrap()
    .into_ini();

    assert_eq!(
        ini.remove_option("one", "A")
            .map(|e| e.as_str().to_string())
            .as_deref(),
        Some("1")
    );
    assert!(ini.remove_option("one", "a").is_none());
    assert_eq!(keys(ini.section("one").unwrap()), ["b", "shared"]);

    assert!(ini.section("one").unwrap().contains("shared"));
    assert!(
        ini.remove_option("one", "shared").is_none(),
        "a default is visible through a section but cannot be removed through it"
    );
    assert!(ini.remove_option("no such section", "b").is_none());
    assert_eq!(
        ini.defaults_mut()
            .remove("SHARED")
            .map(|e| e.as_str().to_string())
            .as_deref(),
        Some("default value")
    );
    assert!(!ini.section("one").unwrap().contains("shared"));
}

#[test]
fn clear_drops_all_sections_but_keeps_the_defaults() {
    let mut ini = parse(
        "[DEFAULT]\nshared = default value\n[zing]\noption1 = value1\n",
        &Options::default(),
    )
    .unwrap()
    .into_ini();

    ini.clear();

    assert_eq!(section_names(&ini), [] as [&str; 0]);
    assert_eq!(ini.defaults().keys().collect::<Vec<_>>(), ["shared"]);
    assert!(ini.is_empty(), "a document with no sections is empty");
}

#[test]
fn empty_input_produces_an_empty_document() {
    let parsed = parse("", &Options::default()).unwrap();

    assert!(parsed.ini().is_empty());
    assert_eq!(section_names(parsed.ini()), [] as [&str; 0]);
    assert!(parsed.problems().is_empty());
    assert_eq!(parsed.ini(), &Ini::default());
}

/// Was: strict mode silently kept the *first* value and still returned `Ok`.
#[test]
fn a_duplicate_option_keeps_the_last_value_in_both_modes() {
    let source = "[Foo]\nx = 1\ny = 2\ny = 3\n";

    let parsed = parse(source, &Options::default()).unwrap();
    assert_eq!(value_of(parsed.ini(), "Foo", "x"), Some("1"));
    assert_eq!(value_of(parsed.ini(), "Foo", "y"), Some("3"));
    assert_eq!(keys(parsed.ini().section("Foo").unwrap()), ["x", "y"]);
    assert_eq!(messages(&parsed), ["duplicate option `y`"]);
    assert_eq!(parsed.problems()[0].severity, Severity::Warning);
    assert!(parsed.into_result().is_ok());

    let parsed = parse(source, &strict()).unwrap();
    assert_eq!(value_of(parsed.ini(), "Foo", "y"), Some("3"));
    assert_eq!(messages(&parsed), ["duplicate option `y`"]);
    assert_eq!(parsed.problems()[0].severity, Severity::Error);
    assert!(
        parsed.into_result().is_err(),
        "strict mode refuses to vouch for the document"
    );
}

#[test]
fn duplicate_options_across_repeated_section_headers_are_reported_too() {
    let source = "[Foo]\na = 1\n[Foo]\na = 2\n";

    for options in [Options::default(), strict()] {
        let parsed = parse(source, &options).unwrap();
        assert_eq!(value_of(parsed.ini(), "Foo", "a"), Some("2"));
        assert_eq!(
            messages(&parsed),
            ["duplicate section `Foo`", "duplicate option `a`"]
        );
    }
}

#[test]
fn a_key_differing_only_in_case_counts_as_a_duplicate() {
    let parsed = parse("[Foo]\nKey = 1\nkey = 2\n", &Options::default()).unwrap();

    assert_eq!(keys(parsed.ini().section("Foo").unwrap()), ["key"]);
    assert_eq!(value_of(parsed.ini(), "Foo", "key"), Some("2"));
    assert_eq!(messages(&parsed), ["duplicate option `key`"]);
    assert_eq!(
        parsed.problems()[0].kind,
        ProblemKind::DuplicateOption {
            key: "key".to_string(),
            previous: Some(Span::new(6, 3)),
        },
        "the report points at the occurrence it replaced"
    );
}

#[test]
fn spans_point_back_at_the_source_text() {
    let source = "[MixedCase]\nKey = Value\n";
    let parsed = parse(source, &Options::default()).unwrap();
    let section = parsed.ini().section("MixedCase").unwrap();

    assert_eq!(
        parsed.text(section.header_span().unwrap()),
        Some("MixedCase")
    );
    let entry = section.get("KEY").unwrap();
    assert_eq!(parsed.text_of(&entry.key), Some("Key"));
    assert_eq!(parsed.text_of(&entry.value), Some("Value"));
    assert_eq!(entry.value.span, Some(Span::new(18, 5)));
}

#[test]
fn a_continuation_value_span_covers_every_line_it_was_built_from() {
    let source = "[s]\nk = a\n\tb\n";
    let parsed = parse(source, &Options::default()).unwrap();
    let entry = parsed.ini().get("s", "k").unwrap();

    assert_eq!(entry.value.span, Some(Span::new(8, 4)));
    assert_eq!(parsed.text_of(&entry.value), Some("a\n\tb"));
    assert_eq!(entry.as_str(), "a\nb");
}

#[test]
fn parse_reader_and_parse_agree() {
    let source = "[s]\nkey = value\n\tcontinued\n";
    let from_str = parse(source, &Options::default()).unwrap();
    let from_bytes =
        parse_reader(std::io::Cursor::new(source.as_bytes()), &Options::default()).unwrap();

    assert_eq!(section_names(from_bytes.ini()), ["s"]);
    assert_eq!(
        value_of(from_bytes.ini(), "s", "key"),
        Some("value\ncontinued")
    );
    assert_eq!(from_bytes.ini(), from_str.ini());
    assert_eq!(from_bytes.source(), from_str.source());
    assert!(from_bytes.problems().is_empty());
}

#[test]
fn crlf_and_lone_cr_line_endings_are_stripped_from_values() {
    let parsed = parse("[s]\r\nkey = value\r\nother = 2\r\n", &Options::default()).unwrap();
    assert_eq!(section_names(parsed.ini()), ["s"]);
    assert_eq!(value_of(parsed.ini(), "s", "key"), Some("value"));
    assert_eq!(value_of(parsed.ini(), "s", "other"), Some("2"));

    // A lone `\r` is a line terminator too, so this is three lines, not one.
    let parsed = parse("[s]\rkey = value\rother = 2\r", &Options::default()).unwrap();
    assert_eq!(section_names(parsed.ini()), ["s"]);
    assert_eq!(value_of(parsed.ini(), "s", "key"), Some("value"));
    assert_eq!(value_of(parsed.ini(), "s", "other"), Some("2"));
}

#[test]
fn values_are_never_interpolated() {
    let parsed = parse(
        "[numbers]\none = 1\ntwo = %(one)s * 2\nthree = ${common:one} * 3\n",
        &Options::default(),
    )
    .unwrap();
    assert_eq!(value_of(parsed.ini(), "numbers", "one"), Some("1"));
    assert_eq!(
        value_of(parsed.ini(), "numbers", "two"),
        Some("%(one)s * 2")
    );
    assert_eq!(
        value_of(parsed.ini(), "numbers", "three"),
        Some("${common:one} * 3")
    );
}

#[test]
fn fixture_cfgparser_0_keeps_trailing_slashes_without_inline_comments() {
    let source = include_str!("../test-data/cfgparser.0.ini");
    let parsed = parse(source, &Options::default()).unwrap();

    assert_eq!(section_names(parsed.ini()), ["global"]);
    assert_eq!(
        items(parsed.ini().section("global").unwrap()),
        [
            ("workgroup", "MDKGROUP"),
            (
                "hosts allow",
                "127.  //note this is only my private IP address"
            ),
        ]
    );
}

#[test]
fn fixture_cfgparser_0_strips_the_trailing_comment_when_slashes_are_inline_prefixes() {
    let source = include_str!("../test-data/cfgparser.0.ini");
    let parsed = parse(source, &with_inline_comment_prefixes(&["//"])).unwrap();

    assert_eq!(
        items(parsed.ini().section("global").unwrap()),
        [("workgroup", "MDKGROUP"), ("hosts allow", "127.")]
    );
}

#[test]
fn fixture_cfgparser_3_routes_its_default_section_into_the_defaults() {
    let source = include_str!("../test-data/cfgparser.3.ini");
    let parsed = parse(source, &cfgparser_3_options()).unwrap();
    let ini = parsed.ini();

    assert_eq!(
        section_names(ini),
        [
            "strange",
            "corruption",
            "yeah, sections can be indented as well",
            "another one!",
            "no values here",
            "tricky interpolation",
            "more interpolation",
        ]
    );
    assert_eq!(
        ini.defaults().get("go").map(Entry::as_str),
        Some("%(interpolate)s"),
        "the leading comment lines contribute nothing; `[DEFAULT]` contributes `go`"
    );
    assert!(parsed.problems().is_empty(), "{:?}", parsed.problems());
}

#[test]
fn fixture_cfgparser_3_keeps_blank_lines_inside_a_value_but_drops_comment_lines() {
    let source = include_str!("../test-data/cfgparser.3.ini");
    let parsed = parse(source, &cfgparser_3_options()).unwrap();

    assert_eq!(
        value_of(parsed.ini(), "corruption", "value"),
        Some(
            "that is\n\n\nactually still here\n\n\nand holds all these weird newlines\n\n\nnor the indentation"
        )
    );
    assert_eq!(
        value_of(parsed.ini(), "corruption", "another value"),
        Some("")
    );
}

#[test]
fn fixture_cfgparser_3_continues_a_value_even_across_assignment_delimiters() {
    let source = include_str!("../test-data/cfgparser.3.ini");
    let parsed = parse(source, &cfgparser_3_options()).unwrap();

    assert_eq!(
        value_of(parsed.ini(), "another one!", "this too"),
        Some(
            "are there people with configurations broken as this?\nbeware, this is going to be a continuation\nof the value for\nkey \"this too\"\neven if it has a = character\nthis is still the continuation\nyour editor probably highlights it wrong\nbut that's life"
        )
    );
}

/// An indented header inside an open value continues that value instead of opening a
/// section, matching `configparser`.
#[test]
fn an_indented_section_header_after_an_option_continues_the_value() {
    let parsed = parse("[s]\na = 1\n  [t]\nb = 2\n", &Options::default()).unwrap();

    assert_eq!(section_names(parsed.ini()), ["s"]);
    assert_eq!(value_of(parsed.ini(), "s", "a"), Some("1\n[t]"));
    assert_eq!(value_of(parsed.ini(), "s", "b"), Some("2"));
}
