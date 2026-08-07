## serde-ini-spanned

[<img alt="lint status" src="https://img.shields.io/github/actions/workflow/status/romnn/serde-ini-spanned/lint.yaml?branch=main&label=lint">](https://github.com/romnn/serde-ini-spanned/actions/workflows/lint.yaml)
[<img alt="test status" src="https://img.shields.io/github/actions/workflow/status/romnn/serde-ini-spanned/test.yaml?branch=main&label=test">](https://github.com/romnn/serde-ini-spanned/actions/workflows/test.yaml)
[![dependency status](https://deps.rs/repo/github/romnn/serde-ini-spanned/status.svg)](https://deps.rs/repo/github/romnn/serde-ini-spanned)
[<img alt="crates.io" src="https://img.shields.io/crates/v/serde-ini-spanned">](https://crates.io/crates/serde-ini-spanned)
[<img alt="docs.rs" src="https://img.shields.io/docsrs/serde-ini-spanned/latest?label=docs.rs">](https://docs.rs/serde-ini-spanned)

INI config file deserialization similar to Python's [configparser](https://docs.python.org/3/library/configparser.html) written in Rust.
Tracks detailed span information for precise error messages.

```bash
cargo add serde-ini-spanned
```

#### Usage

```rust
use serde_ini_spanned::{Entry, Options, parse};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let source = "\
[DEFAULT]
retries = 3

[server]
host = example.com
banner = welcome
    to the server
";

    let parsed = parse(source, &Options::default())?;

    // Every problem the document has is reported, not just the first one.
    for problem in parsed.problems() {
        eprintln!("{} at {:?}", problem.kind, parsed.text(problem.span));
    }

    let ini = parsed.into_result().map_err(|parsed| parsed.to_string())?;
    let server = ini.section("server").expect("the section exists");

    assert_eq!(server.get("host").map(Entry::as_str), Some("example.com"));
    // A multi-line value is joined with newlines, its indentation stripped.
    assert_eq!(
        server.get("banner").map(Entry::as_str),
        Some("welcome\nto the server"),
    );
    // `[DEFAULT]` is inherited by every section.
    assert_eq!(server.get("retries").map(Entry::as_str), Some("3"));
    Ok(())
}
```

Every name and value carries the byte range it was read from, and `Parsed` owns the
source, so a span can always be resolved back to text:

```rust
use serde_ini_spanned::{Options, parse};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let source = "[server]\nhost = example.com\n";
    let parsed = parse(source, &Options::default())?;
    let entry = parsed.ini().get("server", "host").expect("the option exists");

    assert_eq!(parsed.text_of(&entry.key), Some("host"));
    assert_eq!(parsed.text_of(&entry.value), Some("example.com"));
    Ok(())
}
```

With the default `codespan` feature, `Problem::to_diagnostic` renders a problem as a
[`codespan-reporting`](https://docs.rs/codespan-reporting) diagnostic that underlines the
exact bytes responsible.
