//! Caller-supplied parsing preferences, compiled once into the matchers the lexer needs.

use crate::parse::Error;
use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind};

/// Caller-supplied parsing preferences.
///
/// Every field is public and every field has a default, so a caller overrides only what it
/// cares about:
///
/// ```
/// use serde_ini_spanned::Options;
///
/// let options = Options {
///     comment_prefixes: vec!["#".to_string()],
///     ..Options::default()
/// };
/// assert_eq!(options.assignment_delimiters, ["=", ":"]);
/// ```
///
/// The preferences are plain data and are never validated on assignment; they are validated
/// exactly once, by [`parse`](crate::parse), before any line of the source is read, so an
/// unusable combination is an [`Error::InvalidOptions`](crate::Error::InvalidOptions) rather
/// than a surprise part way through a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// Treat recoverable problems as errors.
    ///
    /// Strict mode changes only how a problem is *reported*; it never changes which value
    /// the parser keeps.
    pub strict: bool,

    /// Name of the section whose options every other section inherits.
    pub default_section: String,

    /// The patterns that separate an option name from its value, so that with `"="`
    /// configured, `a = 3` assigns `3` to `a`.
    ///
    /// The list must be non-empty and no pattern may be the empty string; [`parse`](crate::parse)
    /// rejects both.
    pub assignment_delimiters: Vec<String>,

    /// The patterns that introduce a full-line comment, matched against the line's first
    /// non-whitespace text.
    ///
    /// A comment line is treated exactly like a blank line. No pattern may be the empty
    /// string.
    pub comment_prefixes: Vec<String>,

    /// The patterns that introduce a comment in the middle of a line.
    ///
    /// Empty by default, so `a = 3 # test` yields the value `3 # test` unless `#` is listed
    /// here as well. No pattern may be the empty string.
    pub inline_comment_prefixes: Vec<String>,

    /// Whether a blank line may appear inside a multi-line value rather than ending it.
    pub allow_empty_lines_in_values: bool,

    /// Whether a section name may itself contain `]`, as in `[a]b]`.
    pub allow_brackets_in_section_name: bool,
}

impl Options {
    /// The name of the inherited-defaults section unless the caller picks another.
    pub const DEFAULT_SECTION: &'static str = "DEFAULT";

    /// The assignment delimiters used unless the caller picks others.
    pub const DEFAULT_ASSIGNMENT_DELIMITERS: [&'static str; 2] = ["=", ":"];

    /// The full-line comment prefixes used unless the caller picks others.
    pub const DEFAULT_COMMENT_PREFIXES: [&'static str; 2] = [";", "#"];

    /// The inline comment prefixes used unless the caller picks others: none, so a `#` inside
    /// a value stays part of the value.
    pub const DEFAULT_INLINE_COMMENT_PREFIXES: [&'static str; 0] = [];

    /// Validates the preferences and builds the matchers the lexer searches lines with.
    ///
    /// This is the single edge at which parsing preferences are checked, so every later
    /// stage may assume a delimiter exists and that no pattern matches the empty string.
    /// Because [`CompiledOptions`] has private fields and no other constructor, "the
    /// matchers agree with the options they were built from" is enforced by privacy rather
    /// than by convention.
    ///
    /// # Errors
    /// Returns [`Error::InvalidOptions`] when [`Options::assignment_delimiters`] is empty
    /// (nothing could ever be recognized as an assignment), when any pattern in any of the
    /// three lists is the empty string (it would match at every byte offset), or when the
    /// pattern matcher itself rejects the patterns.
    pub(crate) fn compile(&self) -> Result<CompiledOptions, Error> {
        if self.assignment_delimiters.is_empty() {
            return Err(Error::InvalidOptions(
                "at least one assignment delimiter is required".to_string(),
            ));
        }

        let pattern_lists = [
            ("assignment delimiter", &self.assignment_delimiters),
            ("comment prefix", &self.comment_prefixes),
            ("inline comment prefix", &self.inline_comment_prefixes),
        ];
        for (label, patterns) in pattern_lists {
            if patterns.iter().any(String::is_empty) {
                return Err(Error::InvalidOptions(format!(
                    "{label} must not be the empty string"
                )));
            }
        }

        Ok(CompiledOptions {
            assignment_delimiters: build(&self.assignment_delimiters)?,
            comment_prefixes: build(&self.comment_prefixes)?,
            inline_comment_prefixes: build(&self.inline_comment_prefixes)?,
            delimiter_names: self.assignment_delimiters.clone(),
            default_section: self.default_section.clone(),
            allow_empty_lines_in_values: self.allow_empty_lines_in_values,
            allow_brackets_in_section_name: self.allow_brackets_in_section_name,
        })
    }
}

/// Compiles one pattern list into the matcher the lexer searches lines with.
///
/// [`MatchKind::LeftmostFirst`] is what makes the caller's list order mean what
/// `configparser`'s regex alternation means: the earliest position in the line wins, and
/// among the patterns that start there the one the caller listed first wins. The default
/// [`MatchKind::Standard`] instead reports whichever match *ends* soonest, which would pick
/// `=` out of `["==", "="]` on `a == b` and leak the second `=` into the value, and would
/// miss `rem ` out of `["rem ", "m"]` because the `m` at offset 2 ends sooner.
///
/// The build error is rendered rather than wrapped, so the matcher crate stays out of this
/// crate's public API and can be replaced without a breaking change.
fn build(patterns: &[String]) -> Result<AhoCorasick, Error> {
    AhoCorasickBuilder::new()
        .match_kind(MatchKind::LeftmostFirst)
        .build(patterns)
        .map_err(|err| Error::InvalidOptions(err.to_string()))
}

fn owned(patterns: &[&str]) -> Vec<String> {
    patterns.iter().copied().map(String::from).collect()
}

impl Default for Options {
    fn default() -> Self {
        Self {
            strict: false,
            default_section: Self::DEFAULT_SECTION.to_string(),
            assignment_delimiters: owned(&Self::DEFAULT_ASSIGNMENT_DELIMITERS),
            comment_prefixes: owned(&Self::DEFAULT_COMMENT_PREFIXES),
            inline_comment_prefixes: owned(&Self::DEFAULT_INLINE_COMMENT_PREFIXES),
            allow_empty_lines_in_values: true,
            allow_brackets_in_section_name: true,
        }
    }
}

/// Validated preferences together with the matchers built from them.
///
/// Constructible only by [`Options::compile`], which is what makes the matchers and the
/// preferences they were built from impossible to disagree.
#[derive(Debug)]
pub(crate) struct CompiledOptions {
    assignment_delimiters: AhoCorasick,
    comment_prefixes: AhoCorasick,
    inline_comment_prefixes: AhoCorasick,
    delimiter_names: Vec<String>,
    default_section: String,
    allow_empty_lines_in_values: bool,
    allow_brackets_in_section_name: bool,
}

impl CompiledOptions {
    /// Searches for the patterns that separate an option name from its value.
    ///
    /// Guaranteed to hold at least one pattern, none of them empty.
    pub(crate) const fn assignment_delimiters(&self) -> &AhoCorasick {
        &self.assignment_delimiters
    }

    /// Searches for the patterns that introduce a full-line comment.
    pub(crate) const fn comment_prefixes(&self) -> &AhoCorasick {
        &self.comment_prefixes
    }

    /// Searches for the patterns that introduce a comment in the middle of a line.
    pub(crate) const fn inline_comment_prefixes(&self) -> &AhoCorasick {
        &self.inline_comment_prefixes
    }

    /// The assignment delimiters as written by the caller, for the diagnostic that lists
    /// what a line without an assignment could have used.
    pub(crate) fn delimiter_names(&self) -> &[String] {
        &self.delimiter_names
    }

    /// The name of the section whose options every other section inherits.
    pub(crate) fn default_section(&self) -> &str {
        &self.default_section
    }

    /// Whether a blank line may appear inside a multi-line value rather than ending it.
    pub(crate) const fn allow_empty_lines_in_values(&self) -> bool {
        self.allow_empty_lines_in_values
    }

    /// Whether a section name may itself contain `]`.
    pub(crate) const fn allow_brackets_in_section_name(&self) -> bool {
        self.allow_brackets_in_section_name
    }
}

#[cfg(test)]
mod tests {
    use super::{Error, Options};

    /// The defaults are the crate's documented behaviour; a silent change to any of them
    /// changes how every unconfigured caller parses.
    #[test]
    fn defaults_match_the_documented_constants() {
        let options = Options::default();
        assert!(!options.strict);
        assert_eq!(options.default_section, "DEFAULT");
        assert_eq!(Options::DEFAULT_SECTION, "DEFAULT");
        assert_eq!(options.assignment_delimiters, vec!["=", ":"]);
        assert_eq!(Options::DEFAULT_ASSIGNMENT_DELIMITERS, ["=", ":"]);
        assert_eq!(options.comment_prefixes, vec![";", "#"]);
        assert_eq!(Options::DEFAULT_COMMENT_PREFIXES, [";", "#"]);
        assert!(options.inline_comment_prefixes.is_empty());
        assert!(Options::DEFAULT_INLINE_COMMENT_PREFIXES.is_empty());
        assert!(options.allow_empty_lines_in_values);
        assert!(options.allow_brackets_in_section_name);
    }

    #[test]
    fn default_options_compile() {
        assert!(Options::default().compile().is_ok());
    }

    /// A delimiter longer than one byte must survive compilation; the `'static` lists it
    /// replaced could not express one chosen at runtime at all.
    #[test]
    fn multi_character_delimiters_compile() {
        let options = Options {
            assignment_delimiters: vec![":=".to_string(), "=".to_string()],
            ..Options::default()
        };
        let compiled = options.compile().unwrap();
        assert_eq!(compiled.delimiter_names().to_vec(), vec![":=", "="]);
        assert!(compiled.assignment_delimiters().is_match("a := 3"));
    }

    /// Without a delimiter no line could ever be an assignment, and the resulting diagnostic
    /// would read `variable assignment missing one of: ` with nothing after the colon.
    #[test]
    fn empty_delimiter_list_is_rejected() {
        let options = Options {
            assignment_delimiters: vec![],
            ..Options::default()
        };
        let message = match options.compile() {
            Err(Error::InvalidOptions(message)) => message,
            other => panic!("expected InvalidOptions, got {other:?}"),
        };
        assert!(message.contains("assignment delimiter"), "{message}");
    }

    /// An empty pattern matches at every byte offset, which would turn every line into an
    /// assignment, a comment, or both.
    #[test]
    fn an_empty_pattern_is_rejected_in_every_list() {
        let cases: [(&str, Options); 3] = [
            (
                "assignment delimiter",
                Options {
                    assignment_delimiters: vec!["=".to_string(), String::new()],
                    ..Options::default()
                },
            ),
            (
                "comment prefix",
                Options {
                    comment_prefixes: vec![String::new()],
                    ..Options::default()
                },
            ),
            (
                "inline comment prefix",
                Options {
                    inline_comment_prefixes: vec![String::new()],
                    ..Options::default()
                },
            ),
        ];
        for (label, options) in cases {
            match options.compile() {
                Err(Error::InvalidOptions(message)) => {
                    assert!(message.contains(label), "{message} should mention {label}");
                }
                other => panic!("expected InvalidOptions for {label}, got {other:?}"),
            }
        }
    }

    /// The matchers cannot be swapped for others after the fact, so the only thing left to
    /// check is that `compile` carries every preference across unchanged.
    #[test]
    fn compile_carries_every_preference_across() {
        let options = Options {
            strict: true,
            default_section: "COMMON".to_string(),
            assignment_delimiters: vec!["->".to_string()],
            comment_prefixes: vec!["//".to_string()],
            inline_comment_prefixes: vec!["%".to_string()],
            allow_empty_lines_in_values: false,
            allow_brackets_in_section_name: false,
        };
        let compiled = options.compile().unwrap();

        assert_eq!(compiled.default_section(), "COMMON");
        assert!(!compiled.allow_empty_lines_in_values());
        assert!(!compiled.allow_brackets_in_section_name());
        assert!(compiled.assignment_delimiters().is_match("a -> 3"));
        assert!(!compiled.assignment_delimiters().is_match("a = 3"));
        assert!(compiled.comment_prefixes().is_match("// comment"));
        assert!(!compiled.comment_prefixes().is_match("; comment"));
        assert!(compiled.inline_comment_prefixes().is_match("a -> 3 % note"));
    }

    /// An empty inline comment prefix list is the default and must build a matcher that
    /// never matches, rather than failing or matching everywhere.
    #[test]
    fn an_empty_pattern_list_matches_nothing() {
        let compiled = Options::default().compile().unwrap();
        assert!(!compiled.inline_comment_prefixes().is_match("a = 3 # note"));
    }
}
