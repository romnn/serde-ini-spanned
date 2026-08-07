//! Reads INI / Python-`configparser`-style configuration text and remembers where every
//! piece of it came from.
//!
//! Parsing produces a [`Parsed`]: the [`Ini`] document, the source it was read from, and
//! every [`Problem`] found along the way. Section names, option names and values each carry
//! the byte [`Span`] of the text they were read from, so a caller can point a reader at the
//! exact characters responsible for a value.
//!
//! ```
//! use serde_ini_spanned::{Entry, Options, parse};
//!
//! let source = "\
//! [DEFAULT]
//! retries = 3
//!
//! [server]
//! host = example.com
//! banner = welcome
//!     to the server
//! ";
//!
//! let parsed = parse(source, &Options::default())?;
//! let ini = parsed.into_result().map_err(|parsed| parsed.to_string())?;
//!
//! let server = ini.section("server").expect("the section exists");
//! assert_eq!(server.get("host").map(Entry::as_str), Some("example.com"));
//! // A multi-line value is joined with newlines, its indentation stripped.
//! assert_eq!(
//!     server.get("banner").map(Entry::as_str),
//!     Some("welcome\nto the server"),
//! );
//! // `[DEFAULT]` is inherited by every section.
//! assert_eq!(server.get("retries").map(Entry::as_str), Some("3"));
//! # Ok::<_, Box<dyn std::error::Error>>(())
//! ```
//!
//! Spans point back at the source, which [`Parsed`] owns so that the two cannot be paired
//! wrongly:
//!
//! ```
//! use serde_ini_spanned::{Options, parse};
//!
//! let source = "[server]\nhost = example.com\n";
//! let parsed = parse(source, &Options::default())?;
//! let entry = parsed.ini().get("server", "host").expect("the option exists");
//!
//! assert_eq!(parsed.text_of(&entry.key), Some("host"));
//! assert_eq!(parsed.text_of(&entry.value), Some("example.com"));
//! # Ok::<_, Box<dyn std::error::Error>>(())
//! ```
//!
//! # Reporting problems
//!
//! A malformed line is recorded and skipped rather than aborting the parse, so one run
//! reports everything wrong with a file. [`Parsed::has_errors`] and
//! [`Parsed::into_result`] say whether the parser vouches for the result;
//! [`Options::strict`] promotes the two duplicate-definition problems from warnings to
//! errors without changing which value is kept.
//!
// Both links name items that only exist with the feature on, so the paragraph itself is
// feature-gated: under `--no-default-features` there is nothing left to resolve.
#![cfg_attr(
    feature = "codespan",
    doc = "With the default `codespan` feature, [`Problem::to_diagnostic`] renders a problem as a `codespan-reporting` diagnostic; the crate is re-exported as [`codespan_reporting`] so a caller can always name the matching types."
)]
//!
//! # Stated behaviour
//!
//! - **Option names are compared case-insensitively** via [`str::to_lowercase`]. That is
//!   not configurable, and `configparser`'s `optionxform` hook is out of scope. Section
//!   names are compared exactly.
//! - **[`parse`] copies its input** into the returned [`Parsed`]; that copy is what lets a
//!   span always be resolvable. [`Parsed::into_ini`] drops it.
//! - **Lines end at `\n`, `\r\n` or a lone `\r`**, matching `configparser`'s `read_file`.
//!   `read_string` does not split on a lone `\r`; neither behaviour is a clean reference,
//!   and this crate follows the one its reader entry point corresponds to.
//! - **`allow_empty_lines_in_values = false` closes the open option** on a blank *or
//!   comment* line, since a comment line is classified as blank. On a truly blank line
//!   `configparser` instead makes any later continuation impossible, which produces the
//!   same value by a different route; on a comment line it raises, where this crate
//!   recovers and reads the indented line after it as an option of its own.
//! - **Indentation is measured in bytes, not characters.** Multi-byte whitespace (NBSP,
//!   U+3000) counts for more than one column, so a line indented with it may continue an
//!   option `configparser` would reject — and, when the *option's own* line is indented
//!   with it, a line `configparser` would take as a continuation may not be one here.
//! - **Values are never interpolated.** `%(name)s` and `${a:b}` are ordinary text.

#![forbid(unsafe_code)]

mod fragment;
mod ini;
mod lexer;
mod options;
mod parse;
mod problem;
mod span;

pub use ini::{
    Entry, Ini, InvalidBooleanError, NoSectionError, Section, SectionView, convert_to_boolean,
};
pub use options::Options;
pub use parse::{Error, Parsed, parse, parse_reader};
pub use problem::{Problem, ProblemKind, Severity};
pub use span::{Span, Spanned};

/// The diagnostic renderer [`Problem::to_diagnostic`] targets, re-exported so that a caller
/// can always name the exact `Diagnostic` type this crate produces.
#[cfg(feature = "codespan")]
pub use codespan_reporting;
