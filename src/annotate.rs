//! Source annotations discarded by Gherkin, classified against the loaded registry.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use crate::feature::{
    ExpandedScenario, ExpandedStep, LoadedFeature, ScenarioRef, Walk, substitute,
};
use crate::steps::Registry;

static KEYWORDS: LazyLock<BTreeMap<String, BTreeMap<String, Vec<String>>>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("gherkin_keywords.json")).expect("vendored Gherkin keywords")
});

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Comment,
    Blank,
    Step,
    Keyword,
    Tag,
    Description,
    Docstring,
    Table,
}

#[derive(Debug, Clone)]
pub struct SourceLine {
    pub line: usize,
    pub kind: Kind,
    pub text: String,
    /// Candidate step text is separate so outline substitution never rewrites
    /// prose, including a keyword-prefixed comment that matches no step.
    candidate: Option<DisabledCandidate>,
}

#[derive(Debug, Clone)]
struct DisabledCandidate {
    keyword: String,
    text: String,
}

#[derive(Debug, Default)]
pub struct Source {
    regions: BTreeMap<usize, Vec<SourceLine>>,
    comments: BTreeMap<usize, String>,
}

impl Source {
    pub fn comment(&self, line: usize) -> Option<&str> {
        self.comments.get(&line).map(String::as_str)
    }

    pub(crate) fn scan(text: &str, feature: &gherkin::Feature) -> Self {
        let walk = Walk::new(feature);
        let lines: Vec<_> = text.lines().collect();
        let ends_with_newline = text.ends_with('\n');
        // The parser accepts a directive only before the feature. All later
        // `# language:` lines are ordinary comments, including in docstrings.
        let language_line = lines
            .iter()
            .take(feature.position.line - 1)
            .position(|line| language(line).is_some());
        let language = language_line
            .and_then(|i| language(lines[i]))
            .unwrap_or("en");
        let mut keywords: Vec<_> = KEYWORDS
            .get(language)
            .into_iter()
            .flat_map(|k| k.values().flatten())
            .map(String::as_str)
            .collect();
        keywords.sort_by_key(|k| std::cmp::Reverse(k.len()));
        keywords.dedup();
        let marks: BTreeSet<_> = walk.below.iter().copied().collect();
        // A keyword-prefixed comment in a block that starts right under a step
        // or table row is that scenario's disabled step, not the next element's
        // comment.
        let ends_a_step = |mut at: usize| {
            while at > 0 && comment_text(lines[at - 1]).is_some() {
                at -= 1;
            }
            at > 0 && (marks.contains(&at) || lines[at - 1].trim_start().starts_with('|'))
        };
        let mut source = Self::default();
        let mut starts = BTreeMap::new();
        for element in &walk.elements {
            let mut start = element.line - 1;
            let mut comment = Vec::new();
            while start > 0 {
                let previous = lines[start - 1].trim_start();
                if Some(start - 1) == language_line || previous.starts_with('@') {
                    start -= 1;
                } else if let Some(text) = comment_text(previous) {
                    if keywords.iter().any(|k| text.trim_start().starts_with(*k))
                        && ends_a_step(start - 1)
                    {
                        break;
                    }
                    comment.push(text);
                    start -= 1;
                } else {
                    break;
                }
            }
            comment.reverse();
            let comment = trim_block(&comment);
            if !comment.is_empty() {
                source.comments.insert(element.line, comment);
            }
            starts.insert(element.line, start);
        }
        let mut table_lines = BTreeSet::new();
        for table in &walk.tables {
            // The parser's span runs one byte past a file that ends on a row
            // with no newline.
            let body = &text[table.span.start..table.span.end.min(text.len())];
            if let Some(last) = body
                .lines()
                .enumerate()
                .filter(|(_, line)| line.trim_start().starts_with('|'))
                .map(|(index, _)| index)
                .last()
            {
                // The parser's span can include trailing comments. Only the
                // first through last row belong to the table; a note after
                // its last row may still explain the following step.
                table_lines.extend(table.position.line..=table.position.line + last);
            }
        }
        for (index, element) in walk.elements.iter().enumerate() {
            if !matches!(
                element.kind,
                "background" | "rule background" | "scenario" | "examples"
            ) {
                continue;
            }
            let end = walk
                .elements
                .get(index + 1)
                .map_or(lines.len(), |next| starts[&next.line]);
            let mut fence: Option<&str> = None;
            let mut seen_step = false;
            let mut region = Vec::new();
            for (index, text) in lines.iter().enumerate().take(end).skip(element.line - 1) {
                let trimmed = text.trim_start();
                let delimiter = ["\"\"\"", "```"]
                    .into_iter()
                    .find(|f| trimmed.starts_with(f));
                let kind = if index + 1 == element.line {
                    Kind::Keyword
                } else if marks.contains(&(index + 1)) && !trimmed.starts_with('|') {
                    // Parser-recognized steps are authoritative. An annotation
                    // scan must never suppress execution, even after a fence.
                    fence = None;
                    seen_step = true;
                    Kind::Step
                } else if table_lines.contains(&(index + 1)) {
                    Kind::Table
                } else if let Some(open) = fence {
                    if closes_docstring(trimmed, open, index + 1 < lines.len() || ends_with_newline)
                    {
                        fence = None;
                    }
                    Kind::Docstring
                } else if let Some(delimiter) = delimiter.filter(|_| seen_step) {
                    // gherkin accepts inline docstrings and closes at the first
                    // matching delimiter, including before a trailing comment.
                    fence = (!closes_docstring(
                        &trimmed[delimiter.len()..],
                        delimiter,
                        index + 1 < lines.len() || ends_with_newline,
                    ))
                    .then_some(delimiter);
                    Kind::Docstring
                } else if trimmed.is_empty() {
                    Kind::Blank
                } else if trimmed.starts_with('#') {
                    Kind::Comment
                } else if trimmed.starts_with('@') {
                    Kind::Tag
                } else if (seen_step || element.kind == "examples") && trimmed.starts_with('|') {
                    Kind::Table
                } else {
                    Kind::Description
                };
                let candidate = (kind == Kind::Comment)
                    .then(|| {
                        let text = comment_text(trimmed)?.trim_start();
                        keywords
                            .iter()
                            .find(|k| text.starts_with(**k))
                            .map(|k| DisabledCandidate {
                                keyword: (*k).to_string(),
                                text: text[k.len()..].trim().to_string(),
                            })
                    })
                    .flatten();
                region.push(SourceLine {
                    line: index + 1,
                    kind,
                    text: (*text).to_string(),
                    candidate,
                });
            }
            // Comments interspersed in a description are not step annotations.
            // An Examples block contributes only annotations after its table;
            // those closing notes repeat only for rows from that Examples block.
            let start = region
                .iter()
                .rposition(|line| {
                    line.kind
                        == if element.kind == "examples" {
                            Kind::Table
                        } else {
                            Kind::Description
                        }
                })
                .map_or(0, |i| i + 1);
            let region = region.split_off(start);
            source.regions.insert(element.line, region);
        }
        source
    }
}

// In gherkin's docstring rule, `nl()` consumes a # comment and its newline
// before individual characters are considered. A delimiter after # on that
// line is therefore body content. Without a newline, that alternative fails.
fn closes_docstring(line: &str, fence: &str, has_newline: bool) -> bool {
    let content = if has_newline {
        line.split_once('#').map_or(line, |(before, _)| before)
    } else {
        line
    };
    content.contains(fence)
}

fn language(line: &str) -> Option<&str> {
    let line = line.trim().strip_prefix('#')?.trim_start();
    let line = line
        .strip_prefix("language")?
        .trim_start()
        .strip_prefix(':')?;
    Some(line.trim())
}

fn comment_text(line: &str) -> Option<&str> {
    let text = line.trim_start().strip_prefix('#')?;
    Some(text.strip_prefix(' ').unwrap_or(text))
}

fn trim_block(lines: &[&str]) -> String {
    let start = lines
        .iter()
        .position(|line| !line.trim().is_empty())
        .unwrap_or(lines.len());
    let end = lines
        .iter()
        .rposition(|line| !line.trim().is_empty())
        .map_or(start, |i| i + 1);
    lines[start..end].join("\n")
}

#[derive(Debug)]
pub enum Item {
    Step {
        line: usize,
        comment: String,
    },
    Disabled {
        line: usize,
        keyword: String,
        text: String,
        comment: String,
    },
    Section {
        line: usize,
        title: String,
        note: String,
    },
}

#[derive(Debug, Default)]
pub struct Resolved {
    pub items: Vec<Item>,
    pub unmatched: BTreeSet<usize>,
}

pub enum ExecutionItem<'a> {
    Step {
        step: &'a ExpandedStep,
        comment: Option<&'a str>,
    },
    Annotation(&'a Item),
}

impl Item {
    fn line(&self) -> usize {
        match self {
            Self::Step { line, .. } | Self::Disabled { line, .. } | Self::Section { line, .. } => {
                *line
            }
        }
    }
}

impl Resolved {
    /// Parsed steps decide execution. Missing or mismatched source annotations
    /// may lose a comment, but must never remove a step or turn a failure green.
    pub fn execution<'a>(
        &'a self,
        steps: impl Iterator<Item = &'a ExpandedStep>,
    ) -> Vec<ExecutionItem<'a>> {
        let mut annotations = self.items.iter().peekable();
        let mut execution = Vec::new();
        for step in steps {
            let mut comment = None;
            while let Some(item) = annotations.next_if(|item| item.line() <= step.line) {
                match item {
                    Item::Step {
                        line,
                        comment: text,
                    } if *line == step.line => comment = Some(text.as_str()),
                    Item::Step { .. } => {}
                    _ => execution.push(ExecutionItem::Annotation(item)),
                }
            }
            execution.push(ExecutionItem::Step { step, comment });
        }
        execution.extend(
            annotations
                .filter(|item| !matches!(item, Item::Step { .. }))
                .map(ExecutionItem::Annotation),
        );
        execution
    }
}

/// The same classification for run, includes and doctor. Only a successful
/// registry match makes a disabled step; matching never needs variable values.
pub fn resolve(lines: &[SourceLine], registry: &Registry) -> Resolved {
    let mut resolved = Resolved::default();
    let mut pending: Vec<(usize, &str)> = Vec::new();
    for line in lines {
        match line.kind {
            Kind::Comment => {
                let text = comment_text(&line.text).unwrap_or("");
                if let Some(candidate) = &line.candidate {
                    if matches!(registry.find(&candidate.text), Ok(Some(_))) {
                        resolved.items.push(Item::Disabled {
                            line: line.line,
                            keyword: candidate.keyword.clone(),
                            text: candidate.text.clone(),
                            comment: take_comment(&mut pending),
                        });
                        continue;
                    }
                    resolved.unmatched.insert(line.line);
                }
                pending.push((line.line, text));
            }
            Kind::Step => resolved.items.push(Item::Step {
                line: line.line,
                comment: take_comment(&mut pending),
            }),
            Kind::Blank => section(&mut pending, &mut resolved.items),
            Kind::Keyword | Kind::Tag | Kind::Description | Kind::Docstring | Kind::Table => {
                pending.clear();
            }
        }
    }
    section(&mut pending, &mut resolved.items);
    resolved
}

fn take_comment(pending: &mut Vec<(usize, &str)>) -> String {
    let lines: Vec<_> = pending.drain(..).map(|(_, text)| text).collect();
    trim_block(&lines)
}

fn section(pending: &mut Vec<(usize, &str)>, items: &mut Vec<Item>) {
    let Some((line, _)) = pending.first().copied() else {
        return;
    };
    let text = take_comment(pending);
    if text.is_empty() {
        return;
    }
    let (title, note) = text.split_once('\n').unwrap_or((&text, ""));
    items.push(Item::Section {
        line,
        title: title.to_string(),
        note: note.to_string(),
    });
}

/// Resolve each execution level separately, preserving Background closing notes
/// before the next level and repeating them for every expanded scenario.
pub fn scenario(
    lf: &LoadedFeature,
    sc: ScenarioRef<'_>,
    expanded: &ExpandedScenario,
    registry: &Registry,
) -> Resolved {
    let mut resolved = Resolved::default();
    let backgrounds = lf
        .feature
        .background
        .iter()
        .chain(sc.rule.and_then(|r| r.background.as_ref()));
    for (line, substitutions) in backgrounds
        .map(|bg| (bg.position.line, &[][..]))
        .chain(std::iter::once((
            expanded.line,
            expanded.substitutions.as_slice(),
        )))
        .chain(
            sc.scenario
                .examples
                .iter()
                .filter(|examples| Some(examples.position.line) == expanded.examples_line)
                .map(|examples| (examples.position.line, expanded.substitutions.as_slice())),
        )
    {
        let Some(lines) = lf.annotations.regions.get(&line) else {
            continue;
        };
        let lines = if substitutions.is_empty() {
            std::borrow::Cow::Borrowed(lines.as_slice())
        } else {
            let (keys, row): (Vec<_>, Vec<_>) = substitutions.iter().cloned().unzip();
            std::borrow::Cow::Owned(
                lines
                    .iter()
                    .map(|line| SourceLine {
                        candidate: line.candidate.as_ref().map(|candidate| DisabledCandidate {
                            keyword: candidate.keyword.clone(),
                            text: substitute(&candidate.text, &keys, &row),
                        }),
                        ..line.clone()
                    })
                    .collect::<Vec<_>>(),
            )
        };
        let region = resolve(&lines, registry);
        resolved.items.extend(region.items);
        resolved.unmatched.extend(region.unmatched);
    }
    resolved
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Annotations of the first expansion of the scenario at `index`, as
    /// `kind:line` strings in order.
    fn annotations(text: &str, index: usize) -> Vec<String> {
        let registry = Registry::new().unwrap();
        let lf = LoadedFeature::from_text("t.feature".into(), text).unwrap();
        let sc = lf.scenarios().nth(index).unwrap();
        let expanded = crate::feature::expand_outlines(sc.scenario).remove(0);
        super::scenario(&lf, sc, &expanded, &registry)
            .items
            .iter()
            .map(|item| match item {
                Item::Step { line, .. } => format!("step:{line}"),
                Item::Disabled { line, .. } => format!("disabled:{line}"),
                Item::Section { line, .. } => format!("section:{line}"),
            })
            .collect()
    }

    #[test]
    fn a_block_after_the_last_step_of_the_file_is_a_closing_section() {
        let text =
            "Feature: f\n  Scenario: a\n    Given set variable \"x\" to \"1\"\n\n    # The end\n";
        assert_eq!(annotations(text, 0), ["step:3", "section:5"]);
    }

    #[test]
    fn a_disabled_step_right_above_the_next_scenario_stays_in_its_own() {
        let text = "Feature: f\n  Scenario: a\n    Given set variable \"x\" to \"1\"\n    # When set variable \"y\" to \"2\"\n  Scenario: b\n    Given set variable \"y\" to \"1\"\n";
        assert_eq!(annotations(text, 0), ["step:3", "disabled:4"]);
        let lf = LoadedFeature::from_text("t.feature".into(), text).unwrap();
        assert_eq!(lf.annotations.comment(5), None);
    }

    #[test]
    fn a_file_ending_on_a_table_row_without_a_newline_loads() {
        for text in [
            "Feature: f\n  Scenario: s\n    Given set variable \"x\" to \"1\"\n      | a | b |",
            "Feature: f\n  Scenario Outline: s\n    Given set variable \"x\" to \"<a>\"\n    Examples:\n      | a |\n      | 1 |",
        ] {
            assert!(LoadedFeature::from_text("t.feature".into(), text).is_ok());
        }
    }

    #[test]
    fn vendored_step_keywords_agree_with_the_parser_in_several_languages() {
        let registry = Registry::new().unwrap();
        for (language, feature, scenario, keywords) in [
            (
                "en",
                "Feature",
                "Scenario",
                ["Given ", "When ", "Then ", "And ", "But ", "* "],
            ),
            (
                "ru",
                "Функционал",
                "Сценарий",
                ["Допустим ", "Когда ", "Тогда ", "И ", "Но ", "* "],
            ),
            (
                "fr",
                "Fonctionnalité",
                "Scénario",
                ["Soit ", "Quand ", "Alors ", "Et ", "Mais ", "* "],
            ),
            (
                "ja",
                "フィーチャ",
                "シナリオ",
                ["前提", "もし", "ならば", "かつ", "しかし", "* "],
            ),
        ] {
            for keyword in keywords {
                let text = format!(
                    "# language: {language}\n{feature}: f\n  {scenario}: s\n    # {keyword}set variable \"disabled\" to \"1\"\n    {keyword}set variable \"enabled\" to \"1\"\n"
                );
                let lf = LoadedFeature::from_text("keywords.feature".into(), &text).unwrap();
                let sc = lf.scenarios().next().unwrap();
                let expanded = crate::feature::expand_outlines(sc.scenario).remove(0);
                let resolved = super::scenario(&lf, sc, &expanded, &registry);
                assert_eq!(expanded.steps[0].keyword, keyword, "{language}: {keyword}");
                let Item::Disabled {
                    keyword: disabled_keyword,
                    text,
                    ..
                } = &resolved.items[0]
                else {
                    panic!("{language}: {keyword}: {:?}", resolved.items);
                };
                assert_eq!(disabled_keyword, keyword);
                assert_eq!(text, "set variable \"disabled\" to \"1\"");
            }
        }
    }
}
