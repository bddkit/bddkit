//! Reading Gherkin text: what `feature::LoadedFeature::from_text` hands to
//! gherkin, and what it does with a failure. A leading byte-order mark is not
//! part of the text, a parse error says the same thing on every run and never
//! names a line past the end of the file, and a description line that is one
//! edit away from a keyword (`Scenaro:`) is an error, not free text.

use std::ops::Range;

use gherkin::{Feature, GherkinEnv};

use super::Walk;

/// Why a feature file's text is not accepted: where, and what.
#[derive(Debug)]
pub(crate) struct SyntaxError {
    pub(crate) line: usize,
    col: usize,
    what: String,
}

impl std::fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Error at {}:{}: {}", self.line, self.col, self.what)
    }
}

impl std::error::Error for SyntaxError {}

/// `text` without a leading UTF-8 byte-order mark, which Windows editors write.
pub(crate) fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

/// What gherkin says when its token set names one of these reasons, in words that
/// do not change (the first listed wins; its own message prints the set in hash
/// order). Anything else is "not valid Gherkin".
const REASONS: [(&str, &str); 4] = [
    (
        "\"inconsistent table row sizes\"",
        "inconsistent table row sizes",
    ),
    ("\"tag character\"", "invalid character in a tag"),
    ("\"unknown keyword\"", "unknown keyword"),
    ("\"fatal error\"", "unsupported `# language:`"),
];

/// Parses `text` (already without a BOM). gherkin's own error keeps its position
/// private and prints the tokens it expected in hash order, so the message is
/// built here: the reason when it is one of [`REASONS`], and the line is the last
/// line of the file at most (gherkin reports where it stopped, one past the end
/// when the last line is the bad one). A clamped line has no meaningful column.
/// When the bad line, or the one before it, is a misspelled keyword, that is the
/// error: gherkin stops after a typo'd keyword, not on it.
pub(crate) fn parse(text: &str) -> Result<Feature, SyntaxError> {
    let feature = Feature::parse(text, GherkinEnv::default()).map_err(|error| {
        // `Display` starts with "Error at LINE:COL: "; `a_broken_feature_file_is_listed_with_its_error`
        // pins that. Line 1 when the format ever changes.
        let shown = error.to_string();
        let at = shown
            .strip_prefix("Error at ")
            .and_then(|rest| rest.split_once(": "))
            .and_then(|(at, _)| at.split_once(':'))
            .and_then(|(line, col)| Some((line.parse().ok()?, col.parse().ok()?)))
            .unwrap_or((1, 1));
        let lines: Vec<&str> = text.lines().collect();
        let last = lines.len().max(1);
        let (line, col) = if at.0 > last { (last, 1) } else { at };
        let first = lines
            .iter()
            .find(|l| !matches!(l.trim_start().chars().next(), None | Some('#' | '@')));
        if first.is_some_and(|l| english_feature(l.split(':').next().unwrap_or(""))) {
            let before = line.checked_sub(2).and_then(|i| keyword_typo(&lines, i));
            if let Some(typo) = before.or_else(|| keyword_typo(&lines, line - 1)) {
                return typo;
            }
        }
        let what = REASONS
            .iter()
            .find(|(token, _)| shown.contains(token))
            .map_or("not valid Gherkin", |(_, words)| words);
        SyntaxError {
            line,
            col,
            what: what.into(),
        }
    })?;
    // `Feature:` as gherkin read it: a file in another language is not checked.
    if english_feature(&feature.keyword) {
        let lines: Vec<&str> = text.lines().collect();
        for (_, span) in description_spans(&lines, &feature) {
            if let Some(typo) = span
                .into_iter()
                .find_map(|index| keyword_typo(&lines, index))
            {
                return Err(typo);
            }
        }
    }
    Ok(feature)
}

/// Whether `word` is how English Gherkin spells a `Feature`. Only `en` and
/// `en-Scouse` use these (`languages.json`), and `en-Scouse` shares the block
/// keywords below.
fn english_feature(word: &str) -> bool {
    matches!(
        word.trim().trim_end_matches(':'),
        "Feature" | "Business Need" | "Ability"
    )
}

/// For every element, the lines gherkin read as its description: the keyword's
/// line, and the range of lines (0-based indexes into `lines`) below it up to
/// the next line gherkin gave a position to, or a tag line. The positions, not a
/// list of keywords, so a `# language: ru` file needs nothing extra.
pub(crate) fn description_spans(lines: &[&str], feature: &Feature) -> Vec<(usize, Range<usize>)> {
    let walk = Walk::new(feature);
    let marks: std::collections::BTreeSet<usize> = walk
        .elements
        .iter()
        .map(|e| e.line)
        .chain(walk.below.iter().copied())
        .collect();
    walk.elements
        .iter()
        .map(|e| e.line)
        .filter_map(|line| {
            // Lines are 1-based, so `lines[line..]` starts right after the keyword.
            let end = marks
                .range(line + 1..)
                .next()
                .map_or(lines.len(), |next| next - 1);
            let body = lines.get(line..end)?;
            let kept = body
                .iter()
                .take_while(|l| !l.trim_start().starts_with('@'))
                .count();
            Some((line, line..line + kept))
        })
        .collect()
}

/// The block keywords of English Gherkin, as Gherkin spells them.
const KEYWORDS: [&str; 11] = [
    "Feature",
    "Business Need",
    "Ability",
    "Background",
    "Rule",
    "Scenario",
    "Scenario Outline",
    "Scenario Template",
    "Scenarios",
    "Examples",
    "Example",
];

/// The error for line `index` (0-based) when its word before `:` is meant to be a
/// keyword and is not one: in the wrong case, or one edit from one (`Scenaro:`,
/// a missing or changed letter). gherkin reads such a line in a description as
/// free text, so the element's scenarios vanish without a word. A keyword with
/// one letter more (`Features: login`, `Featured: in news`) is prose, and a
/// word under five letters (`Rule`) is too short to tell from prose. A swapped
/// pair (`Scenairo`) is two edits and is not caught.
fn keyword_typo(lines: &[&str], index: usize) -> Option<SyntaxError> {
    let line = lines.get(index)?;
    let (word, _) = line.trim_start().split_once(':')?;
    let keyword = near_keyword(word)?;
    Some(SyntaxError {
        line: index + 1,
        col: line.chars().take_while(|c| c.is_whitespace()).count() + 1,
        what: format!("`{word}:` is not a keyword; did you mean `{keyword}:`?"),
    })
}

/// The keyword `word` is a typo of, written as the keyword is.
fn near_keyword(word: &str) -> Option<&'static str> {
    let word = word.trim();
    if word.chars().count() < 5 || !word.chars().all(|c| c.is_alphabetic() || c == ' ') {
        return None;
    }
    let lower = word.to_lowercase();
    let chars: Vec<char> = lower.chars().collect();
    KEYWORDS.iter().copied().find(|keyword| {
        let keyword_lower = keyword.to_lowercase();
        if lower == keyword_lower {
            // The right word in the wrong case; in the right case it parsed.
            return word != *keyword;
        }
        let one_more = lower
            .strip_prefix(&keyword_lower)
            .is_some_and(|rest| rest.chars().count() == 1);
        !one_more && one_edit_apart(&chars, &keyword_lower.chars().collect::<Vec<_>>())
    })
}

/// Whether `a` and `b` differ by exactly one inserted, dropped or changed letter.
fn one_edit_apart(a: &[char], b: &[char]) -> bool {
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    if long.len() - short.len() > 1 {
        return false;
    }
    let same = short.iter().zip(long).take_while(|(x, y)| x == y).count();
    if short.len() == long.len() {
        same < short.len() && short[same + 1..] == long[same + 1..]
    } else {
        short[same..] == long[same + 1..]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error_of(text: &str) -> String {
        parse(text).expect_err("does not parse").to_string()
    }

    #[test]
    fn a_misspelled_keyword_is_an_error_naming_its_line() {
        let text = "Feature: typo\n  Scenaro: s\n    When I sleep 0 seconds\n";
        assert_eq!(
            error_of(text),
            "Error at 2:3: `Scenaro:` is not a keyword; did you mean `Scenario:`?"
        );
        // The right word in the wrong case is not a keyword either.
        let text = "Feature: f\n  scenario: s\n";
        assert_eq!(
            error_of(text),
            "Error at 2:3: `scenario:` is not a keyword; did you mean `Scenario:`?"
        );
    }

    #[test]
    fn prose_with_a_colon_is_not_a_misspelled_keyword() {
        let text = "\
Feature: prose
  Note: this is a description
  Features: login, logout
  Role: max 3
  As a user: I want things
  Reason: nothing

  Scenario: s
    When I sleep 0 seconds
";
        assert!(parse(text).is_ok());
        // A step, a table cell and a doc string are never looked at.
        let text = "Feature: f\n  Scenario: s\n    When I send\n      \"\"\"\n      Scenaro: x\n      \"\"\"\n";
        assert!(parse(text).is_ok());
    }

    #[test]
    fn a_language_other_than_english_is_not_checked() {
        // gherkin takes a space before the colon too; the check follows what it parsed.
        for header in ["# language: fr", "# language : fr"] {
            let text = format!("{header}\nFonctionnalité: f\n  Exemple: s\n  Scénario: t\n");
            assert!(parse(&text).is_ok(), "{header}");
        }
        let text = "# language : fr\nFonctionnalité: Featured: x\n";
        assert!(parse(text).is_ok());
    }

    #[test]
    fn a_typo_after_a_scenarios_steps_is_named_on_its_own_line() {
        // gherkin stops on the line after it, which is a valid step.
        let text = "Feature: f\n  Scenario: s\n    When I sleep 0 seconds\n  Scenaro: typo\n    When I sleep 0 seconds\n";
        assert_eq!(
            error_of(text),
            "Error at 4:3: `Scenaro:` is not a keyword; did you mean `Scenario:`?"
        );
        // And as the last line, where gherkin stops one past the end.
        let text = "Feature: f\n  Scenario: s\n    When I sleep 0 seconds\n  Scenaro: typo\n";
        assert_eq!(
            error_of(text),
            "Error at 4:3: `Scenaro:` is not a keyword; did you mean `Scenario:`?"
        );
    }

    #[test]
    fn a_single_reason_from_gherkin_is_kept_in_fixed_words() {
        let text = "Feature: f\n  Scenario: s\n    Given a table\n      | a | b |\n      | 1 |\n";
        assert_eq!(error_of(text), "Error at 5:1: inconsistent table row sizes");
        let text = "# language: xx\nFeature: f\n";
        assert_eq!(error_of(text), "Error at 2:1: unsupported `# language:`");
    }

    #[test]
    fn a_bom_is_not_part_of_the_text() {
        let text = "\u{feff}Feature: f\n  Scenario: s\n    When I sleep 0 seconds\n";
        assert!(parse(strip_bom(text)).is_ok());
    }

    #[test]
    fn the_line_is_never_past_the_end_and_the_message_is_fixed() {
        let text = "Feature: x\n  Scenario: a\n    Given foo\n  Bogus line\n";
        assert_eq!(error_of(text), "Error at 4:1: not valid Gherkin");
        for _ in 0..20 {
            assert_eq!(error_of(""), "Error at 1:1: not valid Gherkin");
        }
    }
}
