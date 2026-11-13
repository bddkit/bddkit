//! Values built from literals the program ships with. A failure here is a
//! defect in the source, never an input: the first run that reaches the site
//! meets it, so it cannot ship unnoticed.

use regex::Regex;

/// Compiles a pattern written in the source.
#[expect(
    clippy::expect_used,
    reason = "a literal pattern that does not compile is a source defect, met by the first run"
)]
pub fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("a pattern written in the source is a valid regex")
}
