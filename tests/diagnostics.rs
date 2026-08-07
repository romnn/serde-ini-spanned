//! Rendering problems as `codespan-reporting` diagnostics.
//!
//! This is what the crate exists for beyond a plain INI parser: a span that points at real
//! bytes can be underlined. The whole file is gated on the feature that provides
//! [`Problem::to_diagnostic`], so `--no-default-features` still builds it.
#![cfg(feature = "codespan")]

use serde_ini_spanned::codespan_reporting::diagnostic::{
    Diagnostic, LabelStyle, Severity as CodespanSeverity,
};
use serde_ini_spanned::codespan_reporting::{files, term};
use serde_ini_spanned::{Options, Parsed, parse};

/// Renders diagnostics to an in-memory buffer, so a test can assert on the rendered text
/// instead of on a structure the reader never sees.
struct Printer {
    writer: term::termcolor::Buffer,
    config: term::Config,
    files: files::SimpleFiles<String, String>,
}

impl Printer {
    fn new() -> Self {
        Self {
            writer: term::termcolor::Buffer::no_color(),
            config: term::Config::default(),
            files: files::SimpleFiles::new(),
        }
    }

    fn add(&mut self, name: &str, source: &str) -> usize {
        self.files.add(name.to_string(), source.to_string())
    }

    /// # Errors
    /// When the diagnostic names a file this printer was not given.
    fn emit(&mut self, diagnostic: &Diagnostic<usize>) -> Result<(), files::Error> {
        term::emit(&mut self.writer, &self.config, &self.files, diagnostic)
    }

    fn rendered(&self) -> String {
        String::from_utf8_lossy(self.writer.as_slice()).into_owned()
    }
}

/// Renders every problem in `parsed` and returns what a reader would see.
///
/// # Errors
/// When `codespan-reporting` cannot resolve a span against the source, which would mean
/// the parser produced a span that does not belong to the document it parsed.
fn render(parsed: &Parsed) -> Result<String, files::Error> {
    let mut printer = Printer::new();
    let file_id = printer.add("config.ini", parsed.source());
    for problem in parsed.problems() {
        printer.emit(&problem.to_diagnostic(file_id))?;
    }
    Ok(printer.rendered())
}

#[test]
fn a_syntax_problem_underlines_the_bytes_that_provoked_it() {
    let source = "[s]\nbroken line\n";
    let parsed = parse(source, &Options::default()).unwrap();
    let rendered = render(&parsed).unwrap();

    assert!(
        rendered.contains("error: variable assignment missing one of: `=`, `:`"),
        "{rendered}"
    );
    assert!(rendered.contains("config.ini:2:1"), "{rendered}");
    assert!(rendered.contains("broken line"), "{rendered}");
    assert!(rendered.contains("^^^^^^^^^^^"), "{rendered}");
}

/// A duplicate points at both occurrences, which is only possible because the entry it
/// replaced kept its own span.
#[test]
fn a_duplicate_option_points_at_both_occurrences() {
    let source = "[s]\nkey = first\nkey = second\n";
    let parsed = parse(source, &Options::default()).unwrap();

    let problem = parsed.problems().first().expect("one problem");
    let diagnostic = problem.to_diagnostic(0usize);
    assert_eq!(diagnostic.severity, CodespanSeverity::Warning);
    assert_eq!(diagnostic.labels.len(), 2);
    assert_eq!(diagnostic.labels[0].style, LabelStyle::Primary);
    assert_eq!(diagnostic.labels[0].range, 16..19);
    assert_eq!(diagnostic.labels[1].style, LabelStyle::Secondary);
    assert_eq!(diagnostic.labels[1].range, 4..7);

    let rendered = render(&parsed).unwrap();
    assert!(
        rendered.contains("warning: duplicate option `key`"),
        "{rendered}"
    );
    assert!(
        rendered.contains("second use of option `key`"),
        "{rendered}"
    );
    assert!(rendered.contains("first use of option `key`"), "{rendered}");
}

#[test]
fn strict_mode_renders_a_duplicate_as_an_error() {
    let options = Options {
        strict: true,
        ..Options::default()
    };
    let parsed = parse("[s]\nkey = first\nkey = second\n", &options).unwrap();
    let rendered = render(&parsed).unwrap();

    assert!(
        rendered.contains("error: duplicate option `key`"),
        "{rendered}"
    );
    assert!(parsed.into_result().is_err());
}

/// Every problem a document has renders, with the span it was recorded at.
#[test]
fn every_problem_of_a_thoroughly_broken_document_renders() {
    let source = "[unclosed\nno delimiter here\n[]\n= 1\nk = 1\n";
    let parsed = parse(source, &Options::default()).unwrap();
    assert_eq!(parsed.problems().len(), 5);

    for problem in parsed.problems() {
        let diagnostic = problem.to_diagnostic(0usize);
        assert_eq!(diagnostic.severity, CodespanSeverity::Error);
        assert!(!diagnostic.message.is_empty());
        let range: std::ops::Range<usize> = problem.span.into();
        assert_eq!(diagnostic.labels[0].range, range);
        assert!(parsed.text(problem.span).is_some());
    }

    let rendered = render(&parsed).unwrap();
    for message in [
        "section was not closed: missing ']'",
        "variable assignment missing one of: `=`, `:`",
        "empty section name",
        "empty option name",
        "option outside of any section",
    ] {
        assert!(
            rendered.contains(message),
            "{message} missing from {rendered}"
        );
    }
}
