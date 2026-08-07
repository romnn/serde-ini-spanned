//! Everything wrong with a document that the caller should be shown.

use crate::span::Span;

#[cfg(feature = "codespan")]
use codespan_reporting::diagnostic::{Diagnostic, Label};

/// How seriously a [`Problem`] should be taken.
///
/// A document containing an [`Severity::Error`] was still parsed as far as possible, but the
/// parser does not vouch for the result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Severity {
    /// The document is usable; something in it is suspect.
    Warning,
    /// The document is malformed.
    Error,
}

/// Something wrong with the document, independent of where it occurred.
///
/// Every kind describes a condition the parser recovered from: a malformed line is skipped, a
/// duplicate is applied. None of them abort parsing, which is why all of them are reported
/// instead of only the first.
#[non_exhaustive]
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum ProblemKind {
    /// A line opened a section header with `[` but never closed it.
    #[error(r"section was not closed: missing ']'")]
    SectionNotClosed,
    /// A section name contained a `]`, which the caller did not allow.
    #[error(r"invalid section name: contains ']'")]
    InvalidSectionName,
    /// A section header was written as `[]`.
    #[error("empty section name")]
    EmptySectionName,
    /// An assignment had nothing to the left of its delimiter.
    #[error("empty option name")]
    EmptyOptionName,
    /// A non-blank, non-header line contained none of the configured delimiters.
    #[error("variable assignment missing one of: {}", fmt_delimiters(delimiters))]
    MissingAssignmentDelimiter {
        /// The delimiters that were searched for, in the order the caller configured them.
        delimiters: Vec<String>,
    },
    /// An option appeared before any section header.
    #[error("option outside of any section")]
    MissingSectionHeader,
    /// An option name occurred twice in one section.
    #[error("duplicate option `{key}`")]
    DuplicateOption {
        /// The option name as written at the later occurrence.
        key: String,
        /// Where the earlier occurrence was read from, or `None` when it was not parsed
        /// from source but inserted by the caller.
        previous: Option<Span>,
    },
    /// A section header named a section that already exists; the two are merged.
    #[error("duplicate section `{name}`")]
    DuplicateSection {
        /// The section name as written at the later occurrence.
        name: String,
        /// Where the earlier header was read from, or `None` when the section was not parsed
        /// from source but inserted by the caller.
        previous: Option<Span>,
    },
}

impl ProblemKind {
    /// How seriously this kind should be taken, given the caller's strictness.
    ///
    /// The six syntax kinds are [`Severity::Error`] in both modes: they are the conditions a
    /// stricter parser refuses outright, and reporting them as warnings would quietly weaken
    /// the guarantee callers already have. Only the two duplicate kinds are advisory by
    /// default and become errors under `strict`.
    ///
    /// The match is exhaustive with no wildcard arm, so a new variant cannot inherit a
    /// severity by accident.
    #[must_use]
    pub fn severity(&self, strict: bool) -> Severity {
        match self {
            Self::SectionNotClosed
            | Self::InvalidSectionName
            | Self::EmptySectionName
            | Self::EmptyOptionName
            | Self::MissingAssignmentDelimiter { .. }
            | Self::MissingSectionHeader => Severity::Error,
            Self::DuplicateOption { .. } | Self::DuplicateSection { .. } => {
                if strict {
                    Severity::Error
                } else {
                    Severity::Warning
                }
            }
        }
    }
}

/// Renders a delimiter list the way the diagnostic messages want it.
fn fmt_delimiters(delimiters: &[String]) -> String {
    delimiters
        .iter()
        .map(|delimiter| format!("`{delimiter}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// One problem found in one document, with the source range that provoked it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// What is wrong.
    pub kind: ProblemKind,
    /// How seriously to take it, already resolved against the caller's strictness.
    pub severity: Severity,
    /// The bytes of the source that provoked it.
    pub span: Span,
}

#[cfg(feature = "codespan")]
impl Problem {
    /// Renders this problem as a `codespan-reporting` diagnostic against `file_id`.
    ///
    /// The primary label covers [`Problem::span`]. The two duplicate kinds also carry a
    /// secondary label on the earlier occurrence, but only when that occurrence has a span:
    /// an entry the caller inserted programmatically is nowhere in the source text.
    #[must_use]
    pub fn to_diagnostic<F: Copy>(&self, file_id: F) -> Diagnostic<F> {
        let (primary, secondary) = match &self.kind {
            ProblemKind::SectionNotClosed => ("missing `]`".to_string(), None),
            ProblemKind::InvalidSectionName => ("section must not contain `]`".to_string(), None),
            ProblemKind::EmptySectionName => ("section name must not be empty".to_string(), None),
            ProblemKind::EmptyOptionName => ("option name must not be empty".to_string(), None),
            ProblemKind::MissingAssignmentDelimiter { delimiters } => (
                format!("missing one of: {}", fmt_delimiters(delimiters)),
                None,
            ),
            ProblemKind::MissingSectionHeader => (
                "every option must follow a section header".to_string(),
                None,
            ),
            ProblemKind::DuplicateOption { key, previous } => (
                format!("second use of option `{key}`"),
                previous.map(|span| (span, format!("first use of option `{key}`"))),
            ),
            ProblemKind::DuplicateSection { name, previous } => (
                format!("second use of section `{name}`"),
                previous.map(|span| (span, format!("first use of section `{name}`"))),
            ),
        };

        let mut labels = vec![Label::primary(file_id, self.span).with_message(primary)];
        if let Some((span, message)) = secondary {
            labels.push(Label::secondary(file_id, span).with_message(message));
        }

        let diagnostic = match self.severity {
            Severity::Warning => Diagnostic::warning(),
            Severity::Error => Diagnostic::error(),
        };
        diagnostic
            .with_message(self.kind.to_string())
            .with_labels(labels)
    }
}

/// The sink the parser reports to.
///
/// It owns the strictness decision: severity is resolved once, in [`Problems::push`], so no
/// call site can report a problem without one and no second code path can reinterpret
/// strictness its own way.
#[derive(Debug)]
pub(crate) struct Problems {
    strict: bool,
    items: Vec<Problem>,
}

impl Problems {
    /// An empty sink that resolves severities for a caller who asked for `strict` or not.
    #[must_use]
    pub(crate) fn new(strict: bool) -> Self {
        Self {
            strict,
            items: Vec::new(),
        }
    }

    /// Records `kind` at `span`, assigning its severity.
    pub(crate) fn push(&mut self, kind: ProblemKind, span: Span) {
        let severity = kind.severity(self.strict);
        self.items.push(Problem {
            kind,
            severity,
            span,
        });
    }

    /// The problems recorded so far, in the order they were reported.
    #[must_use]
    pub(crate) fn into_items(self) -> Vec<Problem> {
        self.items
    }
}

#[cfg(test)]
mod tests {
    use super::{Problem, ProblemKind, Problems, Severity};
    use crate::span::Span;

    /// One value of every variant.
    ///
    /// The match in `covers_every_variant` has no wildcard, so a variant added to
    /// `ProblemKind` fails to compile until it is listed here and thus exercised by every
    /// test below.
    fn all_kinds() -> Vec<ProblemKind> {
        vec![
            ProblemKind::SectionNotClosed,
            ProblemKind::InvalidSectionName,
            ProblemKind::EmptySectionName,
            ProblemKind::EmptyOptionName,
            ProblemKind::MissingAssignmentDelimiter {
                delimiters: vec!["=".to_string(), ":".to_string()],
            },
            ProblemKind::MissingSectionHeader,
            ProblemKind::DuplicateOption {
                key: "key".to_string(),
                previous: Some(Span::new(4, 3)),
            },
            ProblemKind::DuplicateSection {
                name: "section".to_string(),
                previous: Some(Span::new(0, 9)),
            },
        ]
    }

    #[test]
    fn covers_every_variant() {
        for kind in all_kinds() {
            match kind {
                ProblemKind::SectionNotClosed
                | ProblemKind::InvalidSectionName
                | ProblemKind::EmptySectionName
                | ProblemKind::EmptyOptionName
                | ProblemKind::MissingAssignmentDelimiter { .. }
                | ProblemKind::MissingSectionHeader
                | ProblemKind::DuplicateOption { .. }
                | ProblemKind::DuplicateSection { .. } => {}
            }
        }
        assert_eq!(all_kinds().len(), 8);
    }

    /// Syntax kinds are hard errors regardless of strictness; only duplicates are advisory.
    #[test]
    fn severity_table() {
        for kind in all_kinds() {
            let expected_lenient = match kind {
                ProblemKind::DuplicateOption { .. } | ProblemKind::DuplicateSection { .. } => {
                    Severity::Warning
                }
                _ => Severity::Error,
            };
            assert_eq!(kind.severity(false), expected_lenient, "lenient: {kind}");
            assert_eq!(kind.severity(true), Severity::Error, "strict: {kind}");
        }
    }

    #[test]
    fn syntax_kinds_are_errors_in_both_modes() {
        let syntax = [
            ProblemKind::SectionNotClosed,
            ProblemKind::InvalidSectionName,
            ProblemKind::EmptySectionName,
            ProblemKind::EmptyOptionName,
            ProblemKind::MissingAssignmentDelimiter { delimiters: vec![] },
            ProblemKind::MissingSectionHeader,
        ];
        for kind in syntax {
            assert_eq!(kind.severity(false), Severity::Error);
            assert_eq!(kind.severity(true), Severity::Error);
        }
    }

    #[test]
    fn duplicate_kinds_are_warnings_unless_strict() {
        let duplicates = [
            ProblemKind::DuplicateOption {
                key: "a".to_string(),
                previous: None,
            },
            ProblemKind::DuplicateSection {
                name: "s".to_string(),
                previous: None,
            },
        ];
        for kind in duplicates {
            assert_eq!(kind.severity(false), Severity::Warning);
            assert_eq!(kind.severity(true), Severity::Error);
        }
    }

    #[test]
    fn display_text_of_every_kind() {
        let messages: Vec<String> = all_kinds().iter().map(ToString::to_string).collect();
        assert_eq!(
            messages,
            vec![
                "section was not closed: missing ']'",
                "invalid section name: contains ']'",
                "empty section name",
                "empty option name",
                "variable assignment missing one of: `=`, `:`",
                "option outside of any section",
                "duplicate option `key`",
                "duplicate section `section`",
            ]
        );
    }

    #[test]
    fn missing_delimiter_message_handles_one_and_none() {
        assert_eq!(
            ProblemKind::MissingAssignmentDelimiter {
                delimiters: vec!["=".to_string()],
            }
            .to_string(),
            "variable assignment missing one of: `=`"
        );
        assert_eq!(
            ProblemKind::MissingAssignmentDelimiter { delimiters: vec![] }.to_string(),
            "variable assignment missing one of: "
        );
    }

    #[test]
    fn push_resolves_severity_from_strictness() {
        let mut lenient = Problems::new(false);
        lenient.push(
            ProblemKind::DuplicateOption {
                key: "a".to_string(),
                previous: None,
            },
            Span::new(1, 2),
        );
        lenient.push(ProblemKind::EmptyOptionName, Span::new(3, 4));

        assert_eq!(
            lenient.into_items(),
            vec![
                Problem {
                    kind: ProblemKind::DuplicateOption {
                        key: "a".to_string(),
                        previous: None,
                    },
                    severity: Severity::Warning,
                    span: Span::new(1, 2),
                },
                Problem {
                    kind: ProblemKind::EmptyOptionName,
                    severity: Severity::Error,
                    span: Span::new(3, 4),
                },
            ]
        );

        let mut strict = Problems::new(true);
        strict.push(
            ProblemKind::DuplicateSection {
                name: "s".to_string(),
                previous: None,
            },
            Span::new(0, 3),
        );
        let items = strict.into_items();
        assert_eq!(items.len(), 1);
        assert_eq!(
            items.first().map(|problem| problem.severity),
            Some(Severity::Error)
        );
    }

    #[test]
    fn an_empty_sink_reports_nothing() {
        assert!(Problems::new(true).into_items().is_empty());
    }

    #[cfg(feature = "codespan")]
    mod diagnostic {
        use super::super::{Problem, ProblemKind, Severity};
        use crate::span::Span;
        use codespan_reporting::diagnostic::{LabelStyle, Severity as CodespanSeverity};

        #[test]
        fn severity_and_primary_label_carry_over() {
            let problem = Problem {
                kind: ProblemKind::SectionNotClosed,
                severity: Severity::Error,
                span: Span::new(2, 5),
            };
            let diagnostic = problem.to_diagnostic(7usize);

            assert_eq!(diagnostic.severity, CodespanSeverity::Error);
            assert_eq!(diagnostic.message, "section was not closed: missing ']'");
            assert_eq!(diagnostic.labels.len(), 1);
            let label = diagnostic.labels.first().unwrap();
            assert_eq!(label.style, LabelStyle::Primary);
            assert_eq!(label.file_id, 7);
            assert_eq!(label.range, 2..7);
            assert_eq!(label.message, "missing `]`");
        }

        #[test]
        fn warnings_render_as_warnings() {
            let problem = Problem {
                kind: ProblemKind::DuplicateOption {
                    key: "a".to_string(),
                    previous: None,
                },
                severity: Severity::Warning,
                span: Span::new(0, 1),
            };
            assert_eq!(
                problem.to_diagnostic(()).severity,
                CodespanSeverity::Warning
            );
        }

        #[test]
        fn a_previous_occurrence_becomes_a_secondary_label() {
            let problem = Problem {
                kind: ProblemKind::DuplicateOption {
                    key: "a".to_string(),
                    previous: Some(Span::new(10, 1)),
                },
                severity: Severity::Warning,
                span: Span::new(20, 1),
            };
            let diagnostic = problem.to_diagnostic(0usize);
            assert_eq!(diagnostic.labels.len(), 2);
            let secondary = diagnostic.labels.get(1).unwrap();
            assert_eq!(secondary.style, LabelStyle::Secondary);
            assert_eq!(secondary.range, 10..11);
            assert_eq!(secondary.message, "first use of option `a`");
        }

        /// A caller-inserted entry has no source location, so there is nothing to point at.
        #[test]
        fn a_synthetic_previous_occurrence_has_no_secondary_label() {
            let problem = Problem {
                kind: ProblemKind::DuplicateSection {
                    name: "s".to_string(),
                    previous: None,
                },
                severity: Severity::Warning,
                span: Span::new(0, 3),
            };
            assert_eq!(problem.to_diagnostic(0usize).labels.len(), 1);
        }

        /// The label under the caret is the only part of a diagnostic that says what to do
        /// about the span, so each one is pinned verbatim rather than merely non-empty. The
        /// table is positional against `all_kinds`, so a new variant fails the length
        /// assertion until its own message is written down here.
        #[test]
        fn every_kind_renders_its_own_primary_label() {
            let expected = [
                "missing `]`",
                "section must not contain `]`",
                "section name must not be empty",
                "option name must not be empty",
                "missing one of: `=`, `:`",
                "every option must follow a section header",
                "second use of option `key`",
                "second use of section `section`",
            ];
            let kinds = super::all_kinds();
            assert_eq!(kinds.len(), expected.len());

            for (kind, expected) in kinds.into_iter().zip(expected) {
                let severity = kind.severity(false);
                let message = kind.to_string();
                let problem = Problem {
                    kind,
                    severity,
                    span: Span::new(0, 1),
                };
                let diagnostic = problem.to_diagnostic(0usize);
                let primary = diagnostic.labels.first().unwrap();
                assert_eq!(primary.style, LabelStyle::Primary);
                assert_eq!(primary.message, expected);
                assert_eq!(diagnostic.message, message);
            }
        }
    }
}
