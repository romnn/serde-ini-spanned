//! Compatibility with Python's `configparser`, ported from the crate's original in-source
//! test module so that it now runs against the public API only.
//!
//! Adapted from <https://github.com/python/cpython/blob/3.13/Lib/test/test_configparser.py>.

use color_eyre::eyre;
use serde_ini_spanned::{
    Entry, Ini, Options, Parsed, Section, SectionView, Severity, Span, Spanned, parse,
};
use similar_asserts::assert_eq as sim_assert_eq;
use std::fmt::Write as _;
use unindent::unindent;

/// The delimiters the ported tests are written against; `CPython` parameterizes its own
/// suite over exactly these two.
const EQUALS: &str = "=";
const COLON: &str = ":";

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

fn section_items<'a>(ini: &'a Ini, name: &str) -> Vec<(&'a str, &'a str)> {
    ini.section(name).map(items).unwrap_or_default()
}

fn value_of<'a>(ini: &'a Ini, section: &str, option: &str) -> Option<&'a str> {
    ini.get(section, option).map(Entry::as_str)
}

fn int_of(ini: &Ini, section: &str, option: &str) -> eyre::Result<Option<i32>> {
    Ok(ini
        .get(section, option)
        .map(Entry::as_int)
        .transpose()?
        .map(Spanned::into_inner))
}

fn float_of(ini: &Ini, section: &str, option: &str) -> eyre::Result<Option<f64>> {
    Ok(ini
        .get(section, option)
        .map(Entry::as_float)
        .transpose()?
        .map(Spanned::into_inner))
}

fn bool_of(ini: &Ini, section: &str, option: &str) -> eyre::Result<Option<bool>> {
    Ok(ini
        .get(section, option)
        .map(Entry::as_bool)
        .transpose()?
        .map(Spanned::into_inner))
}

fn problem_messages(parsed: &Parsed) -> Vec<String> {
    parsed
        .problems()
        .iter()
        .map(|problem| problem.kind.to_string())
        .collect()
}

/// The delimiter and prefix defaults the ported inputs are built from.
#[test]
fn the_ported_tests_use_the_documented_defaults() {
    sim_assert_eq!(Options::DEFAULT_ASSIGNMENT_DELIMITERS, [EQUALS, COLON]);
    sim_assert_eq!(Options::DEFAULT_COMMENT_PREFIXES, [";", "#"]);
    assert!(Options::DEFAULT_INLINE_COMMENT_PREFIXES.is_empty());
    sim_assert_eq!(Options::DEFAULT_SECTION, "DEFAULT");
}

#[test]
fn parse_simple_ini() {
    let source = indoc::indoc! {r"
        [DEFAULT]
        key1 = value1
        pizzatime = yes

        cost = 9

        [topsecrets]
        nuclear launch codes = topsecret

        [github.com]
        User = QEDK
    "};

    let parsed = parse(source, &Options::default()).unwrap();
    assert!(parsed.problems().is_empty());
    let ini = parsed.ini();

    // `[DEFAULT]` is the inherited-defaults section, not an ordinary one.
    sim_assert_eq!(section_names(ini), ["topsecrets", "github.com"]);
    sim_assert_eq!(
        ini.defaults()
            .iter()
            .map(|(key, entry)| (key, entry.as_str()))
            .collect::<Vec<_>>(),
        [("key1", "value1"), ("pizzatime", "yes"), ("cost", "9"),]
    );
    sim_assert_eq!(
        section_items(ini, "topsecrets"),
        [
            ("nuclear launch codes", "topsecret"),
            ("key1", "value1"),
            ("pizzatime", "yes"),
            ("cost", "9"),
        ]
    );
    sim_assert_eq!(value_of(ini, "github.com", "user"), Some("QEDK"));

    // Spans are checked explicitly, because equality now compares them too.
    sim_assert_eq!(
        parsed.text(ini.defaults().header_span().unwrap()),
        Some("DEFAULT")
    );
    for (option, key_text, value_text) in [
        ("key1", "key1", "value1"),
        ("pizzatime", "pizzatime", "yes"),
        ("cost", "cost", "9"),
    ] {
        let entry = ini.defaults().get(option).unwrap();
        sim_assert_eq!(parsed.text_of(&entry.key), Some(key_text));
        sim_assert_eq!(parsed.text_of(&entry.value), Some(value_text));
    }

    let entry = ini.get("topsecrets", "nuclear launch codes").unwrap();
    sim_assert_eq!(parsed.text_of(&entry.key), Some("nuclear launch codes"));
    sim_assert_eq!(parsed.text_of(&entry.value), Some("topsecret"));

    let entry = ini.get("github.com", "user").unwrap();
    sim_assert_eq!(parsed.text_of(&entry.key), Some("User"));
    sim_assert_eq!(parsed.text_of(&entry.value), Some("QEDK"));
}

#[test]
fn configparser_compat_case_sensitivity() -> eyre::Result<()> {
    let mut ini = Ini::default();
    ini.insert_section("A", Section::default());
    ini.insert_section("a", Section::default());
    ini.insert_section("B", Section::default());

    sim_assert_eq!(section_names(&ini), ["A", "a", "B"]);

    ini.set("a", "B".into(), "value".into())?;
    sim_assert_eq!(keys(ini.section("a").unwrap()), ["b"]);
    sim_assert_eq!(
        value_of(&ini, "a", "b"),
        Some("value"),
        "could not locate option, expecting case-insensitive option names"
    );

    // Section names are case-sensitive.
    sim_assert_eq!(
        ini.set("b", "A".into(), "value".into())
            .unwrap_err()
            .to_string(),
        r#"missing section: "b""#,
    );

    assert!(ini.get("a", "b").is_some());
    assert!(ini.get("b", "b").is_none());

    ini.set("A", "A-B".into(), "A-B value".into())?;
    for option in ["a-b", "A-b", "a-B", "A-B"] {
        assert!(
            ini.get("A", option).is_some(),
            "lookup failed for option which should exist: {option}"
        );
    }

    sim_assert_eq!(keys(ini.section("A").unwrap()), ["a-b"]);
    sim_assert_eq!(keys(ini.section("a").unwrap()), ["b"]);

    ini.remove_option("a", "B");
    sim_assert_eq!(keys(ini.section("a").unwrap()), [] as [&str; 0]);

    // SF bug #432369:
    let source = unindent(&format!(
        "
        [MySection]
        Option{EQUALS} first line
        \tsecond line
        ",
    ));
    let parsed = parse(&source, &Options::default()).unwrap();
    sim_assert_eq!(keys(parsed.ini().section("MySection").unwrap()), ["option"]);
    sim_assert_eq!(
        value_of(parsed.ini(), "MySection", "Option"),
        Some("first line\nsecond line")
    );

    // SF bug #561822:
    let source = unindent(&format!(
        r"
        [section]
        nekey{EQUALS}nevalue\n
        ",
    ));
    let parsed = parse(&source, &Options::default()).unwrap();
    // `CPython` passes `defaults={"key": "value"}` here; this crate has no such argument,
    // so the option is absent rather than inherited.
    assert!(parsed.ini().get("section", "Key").is_none());
    Ok(())
}

#[test]
fn configparser_compat_case_insensitivity_mapping_access() {
    let mut ini = Ini::default();
    ini.insert_section("A", Section::default());
    ini.insert_section("a", Section::from([("B".into(), "value".into())]));
    ini.insert_section("B", Section::default());

    sim_assert_eq!(section_names(&ini), ["A", "a", "B"]);
    sim_assert_eq!(keys(ini.section("a").unwrap()), ["b"]);
    sim_assert_eq!(
        ini.section("a")
            .and_then(|view| view.get("b"))
            .map(Entry::as_str),
        Some("value"),
        "could not locate option, expecting case-insensitive option names"
    );

    // Section names are case-sensitive; a miss is `None`, never a panic.
    assert!(ini.section("b").is_none());
    assert!(ini.section("a").unwrap().contains("b"));

    ini.section_mut("A")
        .unwrap()
        .insert("A-B".into(), "A-B value".into());
    for option in ["a-b", "A-b", "a-B", "A-B"] {
        assert!(
            ini.get("A", option).is_some(),
            "lookup failed for option which should exist: {option}"
        );
    }

    sim_assert_eq!(keys(ini.section("A").unwrap()), ["a-b"]);
    sim_assert_eq!(keys(ini.section("a").unwrap()), ["b"]);
    ini.remove_option("a", "B");
    sim_assert_eq!(keys(ini.section("a").unwrap()), [] as [&str; 0]);

    // SF bug #432369:
    let source = format!("[MySection]\nOption{EQUALS} first line   \n\tsecond line   \n");
    let parsed = parse(&source, &Options::default()).unwrap();
    sim_assert_eq!(keys(parsed.ini().section("MySection").unwrap()), ["option"]);
    sim_assert_eq!(
        parsed
            .ini()
            .section("MySection")
            .and_then(|view| view.get("Option"))
            .map(Entry::as_str),
        Some("first line\nsecond line"),
    );
}

#[test]
fn configparser_compat_default_case_sensitivity() {
    let mut ini = Ini::default();
    ini.defaults_mut().insert("foo".into(), "Bar".into());
    sim_assert_eq!(
        ini.defaults().get("Foo").map(Entry::as_str),
        Some("Bar"),
        "could not locate option, expecting case-insensitive option names",
    );

    let mut ini = Ini::default();
    ini.defaults_mut().insert("Foo".into(), "Bar".into());
    sim_assert_eq!(
        ini.defaults().get("Foo").map(Entry::as_str),
        Some("Bar"),
        "could not locate option, expecting case-insensitive defaults",
    );
}

/// Every one of these was a hard `Err` that aborted the parse; each is now a recorded
/// problem that the parser recovers from.
#[test]
fn configparser_compat_parse_errors() {
    for delimiter in [EQUALS, COLON] {
        let source = format!("[Foo]\n{delimiter}val-without-opt-name\n");
        let parsed = parse(&source, &Options::default()).unwrap();
        sim_assert_eq!(problem_messages(&parsed), ["empty option name"]);
        assert!(parsed.has_errors());
    }

    // `CPython` raises `MissingSectionHeaderError`; this crate sees a line that is not an
    // assignment first.
    let parsed = parse("No Section!\n", &Options::default()).unwrap();
    sim_assert_eq!(
        problem_messages(&parsed),
        ["variable assignment missing one of: `=`, `:`"]
    );

    let parsed = parse("[Foo]\n  wrong-indent\n", &Options::default()).unwrap();
    sim_assert_eq!(
        problem_messages(&parsed),
        ["variable assignment missing one of: `=`, `:`"]
    );
}

#[test]
fn configparser_compat_query_errors() {
    let mut ini = Ini::default();
    sim_assert_eq!(
        section_names(&ini),
        [] as [&str; 0],
        "a new document should have no defined sections"
    );
    assert!(
        !ini.has_section("Foo"),
        "a new document should have no acknowledged sections"
    );
    assert!(ini.section("Foo").is_none());

    sim_assert_eq!(
        ini.set("foo", "bar".into(), "value".into())
            .unwrap_err()
            .to_string(),
        r#"missing section: "foo""#
    );

    ini.insert_section("foo", Section::default());
    assert!(ini.get("foo", "bar").is_none());
    assert!(ini.set("foo", "bar".into(), "value".into()).is_ok());
    sim_assert_eq!(value_of(&ini, "foo", "bar"), Some("value"));
}

#[test]
fn configparser_compat_boolean() -> eyre::Result<()> {
    let source = unindent(&format!(
        "
        [BOOLTEST]\n
        T1{EQUALS}1\n
        T2{EQUALS}TRUE\n
        T3{EQUALS}True\n
        T4{EQUALS}oN\n
        T5{EQUALS}yes\n
        F1{EQUALS}0\n
        F2{EQUALS}FALSE\n
        F3{EQUALS}False\n
        F4{EQUALS}oFF\n
        F5{EQUALS}nO\n
        E1{EQUALS}2\n
        E2{EQUALS}foo\n
        E3{EQUALS}-1\n
        E4{EQUALS}0.1\n
        E5{EQUALS}FALSE AND MORE",
    ));
    let parsed = parse(&source, &Options::default()).unwrap();
    let ini = parsed.ini();

    for x in 1..=5 {
        sim_assert_eq!(bool_of(ini, "BOOLTEST", &format!("t{x}"))?, Some(true));
        sim_assert_eq!(bool_of(ini, "BOOLTEST", &format!("f{x}"))?, Some(false));
        let error = ini
            .get("BOOLTEST", &format!("e{x}"))
            .unwrap()
            .as_bool()
            .unwrap_err();
        assert!(error.to_string().starts_with("invalid boolean: "));
    }
    Ok(())
}

#[test]
fn configparser_compat_weird_errors() {
    let mut ini = Ini::default();
    assert!(ini.insert_section("Foo", Section::default()).is_none());
    // Unlike `configparser` this is not an error; the caller is handed what was replaced.
    sim_assert_eq!(
        ini.insert_section("Foo", Section::default()),
        Some(Section::default())
    );

    // Options from every occurrence of a repeated header are collected into one section,
    // and the repeat is now reported instead of merging silently.
    let source = unindent(&format!(
        "
        [Foo]
        will this be added{EQUALS}True
        [Bar]
        what about this{EQUALS}True
        [Foo]
        oops{EQUALS}this won't
        ",
    ));
    let parsed = parse(&source, &Options::default()).unwrap();
    sim_assert_eq!(section_names(parsed.ini()), ["Foo", "Bar"]);
    sim_assert_eq!(
        section_items(parsed.ini(), "Foo"),
        [("will this be added", "True"), ("oops", "this won't")]
    );
    sim_assert_eq!(
        section_items(parsed.ini(), "Bar"),
        [("what about this", "True")]
    );
    sim_assert_eq!(problem_messages(&parsed), ["duplicate section `Foo`"]);
    sim_assert_eq!(
        parsed.problems().first().map(|problem| problem.severity),
        Some(Severity::Warning)
    );
    // The section keeps the span of its first header.
    sim_assert_eq!(
        parsed
            .ini()
            .section("Foo")
            .and_then(SectionView::header_span)
            .map(Span::start),
        Some(1)
    );
}

/// Strict mode used to keep the *first* of two duplicate options and return `Ok`; it now
/// keeps the last, exactly as lenient mode does, and reports the duplicate as an error.
#[test]
fn configparser_compat_get_after_duplicate_option() {
    let source = unindent(&format!(
        "
        [Foo]
        x{EQUALS}1
        y{EQUALS}2
        y{EQUALS}3
        ",
    ));
    for strict in [false, true] {
        let options = Options {
            strict,
            ..Options::default()
        };
        let parsed = parse(&source, &options).unwrap();
        sim_assert_eq!(value_of(parsed.ini(), "Foo", "x"), Some("1"));
        sim_assert_eq!(value_of(parsed.ini(), "Foo", "y"), Some("3"));
        sim_assert_eq!(problem_messages(&parsed), ["duplicate option `y`"]);
        sim_assert_eq!(parsed.has_errors(), strict);
    }
}

#[test]
fn configparser_compat_set_string_types() -> eyre::Result<()> {
    let source = unindent(&format!(
        "
        [sect]
        option1{EQUALS}foo
        ",
    ));
    let mut ini = parse(&source, &Options::default()).unwrap().into_ini();

    // Setting values in an existing section from either string type must just work.
    ini.set("sect", "option1".into(), "splat".into())?;
    ini.set("sect", "option1".into(), "splat".to_string().into())?;
    ini.set("sect", "option2".into(), "splat".into())?;
    ini.set("sect", "option2".into(), "splat".to_string().into())?;
    sim_assert_eq!(value_of(&ini, "sect", "option1"), Some("splat"));
    sim_assert_eq!(value_of(&ini, "sect", "option2"), Some("splat"));
    Ok(())
}

#[test]
fn configparser_compat_check_items_config() {
    let source = unindent(&format!(
        r"
        [section]
        name {EQUALS} %(value)s
        key{COLON} |%(name)s|
        getdefault{COLON} |%(default)s|
        ",
    ));
    let mut ini = parse(&source, &Options::default()).unwrap().into_ini();
    // `CPython` writes this as a leading `default = <default>` line, which this crate now
    // reports rather than promoting to a global default.
    ini.defaults_mut()
        .insert("default".into(), "<default>".into());

    sim_assert_eq!(
        section_items(&ini, "section"),
        [
            ("name", "%(value)s"),
            ("key", "|%(name)s|"),
            ("getdefault", "|%(default)s|"),
            ("default", "<default>"),
        ]
    );
    assert!(ini.section("no such section").is_none());

    // The leading form is a reported error, and the option is discarded.
    let stray = format!("default {EQUALS} <default>\n{source}");
    let parsed = parse(&stray, &Options::default()).unwrap();
    sim_assert_eq!(problem_messages(&parsed), ["option outside of any section"]);
    assert!(parsed.ini().defaults().is_empty());
}

#[test]
fn configparser_compat_popitem() {
    let source = unindent(&format!(
        r"
        [section1]
        name1 {EQUALS} value1
        [section2]
        name2 {EQUALS} value2
        [section3]
        name3 {EQUALS} value3
        ",
    ));
    let mut ini = parse(&source, &Options::default()).unwrap().into_ini();

    // `popitem` removes the *first* section, so this walks the insertion order.
    for expected in ["section1", "section2", "section3"] {
        let first = ini.section_names().next().map(ToString::to_string);
        sim_assert_eq!(first.as_deref(), Some(expected));
        assert!(ini.remove_section(expected).is_some());
    }
    assert!(ini.section_names().next().is_none());
    assert!(ini.is_empty());
}

#[test]
fn configparser_compat_clear() {
    let mut ini = Ini::default();
    ini.defaults_mut().insert("foo".into(), "Bar".into());
    sim_assert_eq!(
        ini.defaults().get("Foo").map(Entry::as_str),
        Some("Bar"),
        "could not locate option, expecting case-insensitive option names"
    );

    ini.insert_section(
        "zing",
        Section::from([
            ("option1".into(), "value1".into()),
            ("option2".into(), "value2".into()),
        ]),
    );

    sim_assert_eq!(section_names(&ini), ["zing"]);
    sim_assert_eq!(
        keys(ini.section("zing").unwrap()),
        ["option1", "option2", "foo"]
    );

    ini.clear();
    sim_assert_eq!(section_names(&ini), [] as [&str; 0]);
    sim_assert_eq!(
        ini.defaults().keys().collect::<Vec<_>>(),
        ["foo"],
        "the defaults are not a section and survive `clear`"
    );
}

#[test]
fn configparser_compat_setitem() {
    let source = unindent(&format!(
        r"
        [section1]
        name1 {EQUALS} value1
        [section2]
        name2 {EQUALS} value2
        [section3]
        name3 {EQUALS} value3
        ",
    ));
    let mut ini = parse(&source, &Options::default()).unwrap().into_ini();
    // `CPython` writes this as a leading `nameD = valueD` line, which this crate now
    // reports rather than promoting to a global default.
    ini.defaults_mut().insert("nameD".into(), "valueD".into());

    for (section, own) in [
        ("section1", "name1"),
        ("section2", "name2"),
        ("section3", "name3"),
    ] {
        sim_assert_eq!(keys(ini.section(section).unwrap()), [own, "named"]);
    }
    sim_assert_eq!(value_of(&ini, "section1", "name1"), Some("value1"));
    sim_assert_eq!(value_of(&ini, "section2", "name2"), Some("value2"));
    sim_assert_eq!(value_of(&ini, "section3", "name3"), Some("value3"));
    sim_assert_eq!(section_names(&ini), ["section1", "section2", "section3"]);

    // Replacing a section wholesale keeps its position and drops its old options.
    ini.insert_section(
        "section2",
        Section::from([("name22".into(), "value22".into())]),
    );
    sim_assert_eq!(keys(ini.section("section2").unwrap()), ["name22", "named"]);
    sim_assert_eq!(value_of(&ini, "section2", "name22"), Some("value22"));
    assert!(!ini.section("section2").unwrap().contains("name2"));

    sim_assert_eq!(section_names(&ini), ["section1", "section2", "section3"]);
    ini.insert_section("section3", Section::default());
    sim_assert_eq!(keys(ini.section("section3").unwrap()), ["named"]);
    assert!(!ini.section("section3").unwrap().contains("name3"));
    sim_assert_eq!(section_names(&ini), ["section1", "section2", "section3"]);

    // For bpo-32108, assigning the default section to itself.
    *ini.defaults_mut() = ini.defaults().clone();
    assert!(!ini.defaults().is_empty());
    *ini.defaults_mut() = Section::default();

    sim_assert_eq!(ini.defaults().keys().collect::<Vec<_>>(), [] as [&str; 0]);
    sim_assert_eq!(keys(ini.section("section1").unwrap()), ["name1"]);
    sim_assert_eq!(keys(ini.section("section2").unwrap()), ["name22"]);
    sim_assert_eq!(keys(ini.section("section3").unwrap()), [] as [&str; 0]);
    sim_assert_eq!(section_names(&ini), ["section1", "section2", "section3"]);

    // For bpo-32108, assigning a section to itself.
    let section2 = ini.section_mut("section2").unwrap().clone();
    *ini.section_mut("section2").unwrap() = section2;
    sim_assert_eq!(keys(ini.section("section2").unwrap()), ["name22"]);
}

/// The trailing line has no delimiter and is not indented, so it cannot continue the value
/// above it. It used to abort the parse; the rest of the document is now kept.
#[test]
fn configparser_compat_invalid_multiline_value() {
    let source = unindent(&format!(
        "\
        [DEFAULT]
        test {EQUALS} test
        invalid\
        ",
    ));
    let parsed = parse(&source, &Options::default()).unwrap();
    sim_assert_eq!(
        problem_messages(&parsed),
        ["variable assignment missing one of: `=`, `:`"]
    );
    assert!(parsed.has_errors());
    sim_assert_eq!(
        parsed.ini().defaults().get("test").map(Entry::as_str),
        Some("test")
    );
}

#[test]
fn configparser_compat_defaults_keyword() -> eyre::Result<()> {
    // bpo-23835 fix for ConfigParser
    let mut ini = Ini::default();
    ini.defaults_mut().insert("1".into(), "2.4".into());
    sim_assert_eq!(ini.defaults().get("1").map(Entry::as_str), Some("2.4"));
    sim_assert_eq!(
        ini.defaults()
            .get("1")
            .map(Entry::as_float)
            .transpose()?
            .map(Spanned::into_inner),
        Some(2.4)
    );

    let mut ini = Ini::default();
    ini.defaults_mut().insert("A".into(), "5.2".into());
    sim_assert_eq!(ini.defaults().get("a").map(Entry::as_str), Some("5.2"));
    sim_assert_eq!(
        ini.defaults()
            .get("a")
            .map(Entry::as_float)
            .transpose()?
            .map(Spanned::into_inner),
        Some(5.2)
    );
    Ok(())
}

#[test]
fn configparser_compat_no_interpolation_matches_ini() {
    let source = unindent(
        r"
        [numbers]
        one = 1
        two = %(one)s * 2
        three = ${common:one} * 3

        [hexen]
        sixteen = ${numbers:two} * 8
        ",
    );
    let parsed = parse(&source, &Options::default()).unwrap();
    let ini = parsed.ini();

    sim_assert_eq!(value_of(ini, "numbers", "one"), Some("1"));
    sim_assert_eq!(value_of(ini, "numbers", "two"), Some("%(one)s * 2"));
    sim_assert_eq!(value_of(ini, "numbers", "three"), Some("${common:one} * 3"));
    sim_assert_eq!(
        value_of(ini, "hexen", "sixteen"),
        Some("${numbers:two} * 8")
    );
}

#[test]
fn configparser_compat_empty_case() {
    let parsed = parse("", &Options::default()).unwrap();
    sim_assert_eq!(parsed.ini(), &Ini::default());
    assert!(parsed.ini().is_empty());
    assert!(parsed.problems().is_empty());
}

#[test]
fn configparser_compat_dominating_multiline_values() -> eyre::Result<()> {
    let wonderful_spam =
        "I'm having spam spam spam spam spam spam spam beaked beans spam spam spam and spam!"
            .replace(' ', "\n\t");

    let mut ini = Ini::default();
    for i in 0..100 {
        let section: Section = (0..10)
            .map(|j| {
                (
                    format!("lovely_spam{j}").into(),
                    wonderful_spam.clone().into(),
                )
            })
            .collect();
        ini.insert_section(format!("section{i}"), section);
    }
    sim_assert_eq!(
        value_of(&ini, "section8", "lovely_spam4"),
        Some(wonderful_spam.as_str())
    );

    // Reading the same thing back from text: the tabs are continuation indentation.
    let mut source = String::new();
    for i in 0..2 {
        writeln!(source, "[section{i}]")?;
        for j in 0..5 {
            writeln!(source, "lovely_spam{j} = {wonderful_spam}")?;
        }
    }
    let parsed = parse(&source, &Options::default()).unwrap();
    let want = wonderful_spam.replace("\n\t", "\n");
    sim_assert_eq!(
        value_of(parsed.ini(), "section1", "lovely_spam4"),
        Some(want.as_str())
    );
    Ok(())
}

#[test]
fn configparser_compat_parse_cfgparser_1() {
    let source = include_str!("../test-data/cfgparser.1.ini");
    let parsed = parse(source, &Options::default()).unwrap();

    sim_assert_eq!(section_names(parsed.ini()), ["Foo Bar"]);
    sim_assert_eq!(section_items(parsed.ini(), "Foo Bar"), [("foo", "newbar")]);
    assert!(parsed.problems().is_empty());
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

#[test]
fn configparser_compat_parse_cfgparser_2() -> eyre::Result<()> {
    let source = include_str!("../test-data/cfgparser.2.ini");
    let parsed = parse(source, &cfgparser_2_options()).unwrap();
    let ini = parsed.ini();

    sim_assert_eq!(
        section_names(ini),
        [
            "global",
            "homes",
            "printers",
            "print$",
            "pdf-generator",
            "tmp",
            "Agustin",
        ]
    );
    sim_assert_eq!(value_of(ini, "global", "workgroup"), Some("MDKGROUP"));
    sim_assert_eq!(int_of(ini, "global", "max log size")?, Some(50));
    sim_assert_eq!(value_of(ini, "global", "hosts allow"), Some("127."));
    sim_assert_eq!(value_of(ini, "tmp", "echo command"), Some("cat %s; rm %s"));
    sim_assert_eq!(
        section_items(ini, "Agustin"),
        [
            ("comment", "Agustin Private Files"),
            ("path", "/home/agustin/Documents"),
            ("valid users", "agustin"),
            ("writable", "yes"),
        ]
    );
    Ok(())
}

#[test]
fn configparser_compat_parse_cfgparser_3() {
    let source = include_str!("../test-data/cfgparser.3.ini");
    let parsed = parse(source, &cfgparser_3_options()).unwrap();
    let ini = parsed.ini();

    // `DEFAULT` is the inherited-defaults section, so it is no longer listed, and its one
    // option shows up at the end of every other section.
    sim_assert_eq!(
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
    sim_assert_eq!(
        ini.defaults()
            .iter()
            .map(|(key, entry)| (key, entry.as_str()))
            .collect::<Vec<_>>(),
        [("go", "%(interpolate)s")]
    );

    sim_assert_eq!(
        section_items(ini, "strange"),
        [
            ("values", "that are indented"),
            ("other", "that do continue\nin\nother\nlines"),
            ("go", "%(interpolate)s"),
        ]
    );

    let multiline_expected = indoc::indoc! {"\
        that is


        actually still here


        and holds all these weird newlines


        nor the indentation"};
    sim_assert_eq!(
        section_items(ini, "corruption"),
        [
            ("value", multiline_expected),
            ("another value", ""),
            ("go", "%(interpolate)s"),
        ]
    );

    sim_assert_eq!(
        section_items(ini, "yeah, sections can be indented as well"),
        [
            ("and that does not mean", "anything"),
            ("are they subsections", "False"),
            ("if you want subsections", "use XML"),
            ("lets use some unicode", "片仮名"),
            ("go", "%(interpolate)s"),
        ]
    );

    sim_assert_eq!(
        section_items(ini, "another one!"),
        [
            ("even if values are indented like this", "seriously"),
            ("yes, this still applies to", r#"section "another one!""#),
            (
                "this too",
                indoc::indoc! {
                    r#"are there people with configurations broken as this?
                    beware, this is going to be a continuation
                    of the value for
                    key "this too"
                    even if it has a = character
                    this is still the continuation
                    your editor probably highlights it wrong
                    but that's life"#,
                }
            ),
            ("interpolate", "anything will do"),
            ("go", "%(interpolate)s"),
        ]
    );

    sim_assert_eq!(
        section_items(ini, "no values here"),
        [("go", "%(interpolate)s")]
    );
    sim_assert_eq!(
        section_items(ini, "tricky interpolation"),
        [
            ("interpolate", "do this"),
            ("lets", "%(go)s"),
            ("go", "%(interpolate)s"),
        ]
    );
    sim_assert_eq!(
        section_items(ini, "more interpolation"),
        [
            ("interpolate", "go shopping"),
            ("lets", "%(go)s"),
            ("go", "%(interpolate)s"),
        ]
    );
    assert!(parsed.problems().is_empty(), "{:?}", parsed.problems());
}

/// Basic `configparser` compatibility.
///
/// Adapted from <https://github.com/python/cpython/blob/3.13/Lib/test/test_configparser.py#L294>
#[test]
fn configparser_compat_basic() -> eyre::Result<()> {
    let source = unindent(&format!(
        r"
        [Foo Bar]
        foo{EQUALS}bar1
        [Spacey Bar]
        foo {EQUALS} bar2
        [Spacey Bar From The Beginning]
          foo {EQUALS} bar3
          baz {EQUALS} qwe
        [Commented Bar]
        foo{COLON} bar4 # comment
        baz{EQUALS}qwe ;another one
        [Long Line]
        foo{COLON} this line is much, much longer than my editor
           likes it.
        [Section\with$weird%characters[\t]
        [Internationalized Stuff]
        foo[bg]{COLON} Bulgarian
        foo{EQUALS}Default
        foo[en]{EQUALS}English
        foo[de]{EQUALS}Deutsch
        [Spaces]
        key with spaces {COLON} value
        another with spaces {EQUALS} splat!
        [Types]
        int {COLON} 42
        float {EQUALS} 0.44
        boolean {EQUALS} NO
        123 {COLON} strange but acceptable
        [This One Has A ] In It]
          forks {EQUALS} spoons
        ",
    ));

    let options = Options {
        inline_comment_prefixes: ["#", ";"].map(String::from).to_vec(),
        ..Options::default()
    };
    let parsed = parse(&source, &options).unwrap();
    assert!(parsed.problems().is_empty(), "{:?}", parsed.problems());
    check_configparser_compat_basic(parsed.into_ini())
}

fn check_configparser_compat_basic(mut ini: Ini) -> eyre::Result<()> {
    sim_assert_eq!(
        section_names(&ini),
        [
            "Foo Bar",
            "Spacey Bar",
            "Spacey Bar From The Beginning",
            "Commented Bar",
            "Long Line",
            r"Section\with$weird%characters[\t",
            "Internationalized Stuff",
            "Spaces",
            "Types",
            "This One Has A ] In It",
        ]
    );

    sim_assert_eq!(
        section_items(&ini, "Spacey Bar From The Beginning"),
        [("foo", "bar3"), ("baz", "qwe")]
    );

    sim_assert_eq!(value_of(&ini, "Foo Bar", "foo"), Some("bar1"));
    sim_assert_eq!(value_of(&ini, "Spacey Bar", "foo"), Some("bar2"));
    sim_assert_eq!(
        value_of(&ini, "Spacey Bar From The Beginning", "foo"),
        Some("bar3")
    );
    sim_assert_eq!(
        value_of(&ini, "Spacey Bar From The Beginning", "baz"),
        Some("qwe")
    );
    sim_assert_eq!(value_of(&ini, "Commented Bar", "foo"), Some("bar4"));
    sim_assert_eq!(value_of(&ini, "Commented Bar", "baz"), Some("qwe"));
    sim_assert_eq!(value_of(&ini, "Spaces", "key with spaces"), Some("value"));
    sim_assert_eq!(
        value_of(&ini, "Spaces", "another with spaces"),
        Some("splat!")
    );
    sim_assert_eq!(int_of(&ini, "Types", "int")?, Some(42));
    sim_assert_eq!(value_of(&ini, "Types", "int"), Some("42"));
    sim_assert_eq!(float_of(&ini, "Types", "float")?, Some(0.44));
    sim_assert_eq!(value_of(&ini, "Types", "float"), Some("0.44"));
    sim_assert_eq!(bool_of(&ini, "Types", "boolean")?, Some(false));
    sim_assert_eq!(
        value_of(&ini, "Types", "123"),
        Some("strange but acceptable")
    );
    sim_assert_eq!(
        value_of(&ini, "This One Has A ] In It", "forks"),
        Some("spoons")
    );
    sim_assert_eq!(
        value_of(&ini, "Long Line", "foo"),
        Some("this line is much, much longer than my editor\nlikes it.")
    );

    // A missing section or option is `None`; there is no indexer left to panic.
    assert!(ini.get("No Such Foo Bar", "foo").is_none());
    assert!(ini.get("Foo Bar", "no-such-foo").is_none());
    assert!(ini.section("No Such Foo Bar").is_none());
    sim_assert_eq!(int_of(&ini, "Types", "no-such-int")?, None);
    sim_assert_eq!(float_of(&ini, "Types", "no-such-float")?, None);
    sim_assert_eq!(bool_of(&ini, "Types", "no-such-boolean")?, None);

    // Now the removal half, added for SourceForge bug #123324.
    ini.defaults_mut().insert("this_value".into(), "1".into());
    ini.defaults_mut().insert("that_value".into(), "2".into());

    assert!(ini.remove_section("Spaces").is_some());
    assert!(ini.get("Spaces", "key with spaces").is_none());
    assert!(ini.remove_section("Spaces").is_none());

    assert!(
        ini.remove_option("Foo Bar", "foo").is_some(),
        "remove_option() failed to report existence of option"
    );
    assert!(
        ini.get("Foo Bar", "foo").is_none(),
        "remove_option() failed to remove option"
    );
    assert!(
        ini.remove_option("Foo Bar", "foo").is_none(),
        "remove_option() failed to report non-existence of option that was removed"
    );

    // An inherited default is visible through a section but cannot be removed through it.
    assert!(ini.get("Foo Bar", "this_value").is_some());
    assert!(ini.remove_option("Foo Bar", "this_value").is_none());
    assert!(ini.defaults_mut().remove("this_value").is_some());
    assert!(ini.get("Foo Bar", "this_value").is_none());
    assert!(ini.defaults_mut().remove("this_value").is_none());
    assert!(ini.remove_option("No Such Section", "foo").is_none());

    ini.remove_section("Types");
    assert!(!ini.has_section("Types"));
    assert!(ini.remove_section("Types").is_none());

    assert!(ini.remove_option("Spacey Bar", "foo").is_some());
    assert!(ini.get("Spacey Bar", "foo").is_none());
    assert!(ini.remove_option("Spacey Bar", "foo").is_none());
    // `that_value` is inherited, so it resolves through the section but is not the
    // section's own and cannot be removed through it.
    assert!(ini.get("Spacey Bar", "that_value").is_some());
    assert!(ini.remove_option("Spacey Bar", "that_value").is_none());
    Ok(())
}

#[test]
fn parse_ini_multi_line_continuation() {
    let source = indoc::indoc! {r"
        [options.packages.find]
        exclude =
            example*
            tests*
            docs*
            build

        [bumpversion:file:CHANGELOG.md]
        replace = **unreleased**
            **v{new_version}**

        [bumpversion:part:release]
        optional_value = gamma
        values =
            dev
            gamma
    "};

    let parsed = parse(source, &Options::default()).unwrap();
    assert!(parsed.problems().is_empty());
    let ini = parsed.ini();

    sim_assert_eq!(
        section_names(ini),
        [
            "options.packages.find",
            "bumpversion:file:CHANGELOG.md",
            "bumpversion:part:release",
        ]
    );
    sim_assert_eq!(
        section_items(ini, "options.packages.find"),
        [("exclude", "\nexample*\ntests*\ndocs*\nbuild")]
    );
    sim_assert_eq!(
        section_items(ini, "bumpversion:file:CHANGELOG.md"),
        [("replace", "**unreleased**\n**v{new_version}**")]
    );
    sim_assert_eq!(
        section_items(ini, "bumpversion:part:release"),
        [("optional_value", "gamma"), ("values", "\ndev\ngamma")]
    );

    // The span of a continuation value runs from the first contributing fragment to the
    // last, and never past it.
    let entry = ini.get("bumpversion:part:release", "values").unwrap();
    sim_assert_eq!(parsed.text_of(&entry.value), Some("\n    dev\n    gamma"));
}
