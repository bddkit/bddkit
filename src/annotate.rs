//! Source annotations discarded by Gherkin, classified against the loaded registry.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use crate::feature::{
    Element, ExpandedScenario, ExpandedStep, LoadedFeature, ScenarioRef, Walk, substitute,
};
use crate::steps::Registry;

#[expect(
    clippy::expect_used,
    reason = "vendored data: a malformed file is a source defect, met by the first run"
)]
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
        let lines = Lines::new(text, feature, &walk);
        let mut source = Self::default();
        let mut starts = BTreeMap::new();
        for element in &walk.elements {
            let (start, comment) = lines.block_above(element.line);
            if !comment.is_empty() {
                source.comments.insert(element.line, comment);
            }
            starts.insert(element.line, start);
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
                .map_or(lines.all.len(), |next| starts[&next.line]);
            source
                .regions
                .insert(element.line, lines.region(element, end));
        }
        source
    }
}

/// One file's lines with what classifying them needs.
struct Lines<'a> {
    all: Vec<&'a str>,
    ends_with_newline: bool,
    /// Lines of parser-recognized steps and `Examples` tables.
    marks: BTreeSet<usize>,
    language_line: Option<usize>,
    /// Longest first, for the file's language.
    keywords: Vec<&'static str>,
    rows: BTreeSet<usize>,
}

/// Where a scan stands inside one element's body.
#[derive(Default)]
struct Block {
    fence: Option<&'static str>,
    seen_step: bool,
}

impl<'a> Lines<'a> {
    fn new(text: &'a str, feature: &gherkin::Feature, walk: &Walk) -> Self {
        let all: Vec<_> = text.lines().collect();
        // The parser accepts a directive only before the feature. All later
        // `# language:` lines are ordinary comments, including in docstrings.
        let language_line = all
            .iter()
            .take(feature.position.line - 1)
            .position(|line| language(line).is_some());
        let language = language_line.and_then(|i| language(all[i])).unwrap_or("en");
        let mut keywords: Vec<_> = KEYWORDS
            .get(language)
            .into_iter()
            .flat_map(|k| k.values().flatten())
            .map(String::as_str)
            .collect();
        keywords.sort_by_key(|k| std::cmp::Reverse(k.len()));
        keywords.dedup();
        Self {
            all,
            ends_with_newline: text.ends_with('\n'),
            marks: walk.below.iter().copied().collect(),
            language_line,
            keywords,
            rows: table_rows(text, walk),
        }
    }

    fn has_newline_after(&self, index: usize) -> bool {
        index + 1 < self.all.len() || self.ends_with_newline
    }

    /// A keyword-prefixed comment in a block that starts right under a step
    /// or table row is that scenario's disabled step, not the next element's
    /// comment.
    fn ends_a_step(&self, mut at: usize) -> bool {
        while at > 0 && comment_text(self.all[at - 1]).is_some() {
            at -= 1;
        }
        at > 0 && (self.marks.contains(&at) || self.all[at - 1].trim_start().starts_with('|'))
    }

    /// Where the tags and comments above an element begin, and the comment.
    fn block_above(&self, element_line: usize) -> (usize, String) {
        let mut start = element_line - 1;
        let mut comment = Vec::new();
        while start > 0 {
            let previous = self.all[start - 1].trim_start();
            if Some(start - 1) == self.language_line || previous.starts_with('@') {
                start -= 1;
            } else if let Some(text) = comment_text(previous) {
                if self.starts_with_keyword(text) && self.ends_a_step(start - 1) {
                    break;
                }
                comment.push(text);
                start -= 1;
            } else {
                break;
            }
        }
        comment.reverse();
        (start, trim_block(&comment))
    }

    fn starts_with_keyword(&self, text: &str) -> bool {
        self.keywords
            .iter()
            .any(|k| text.trim_start().starts_with(*k))
    }

    /// The annotation region of one element: its lines up to `end`, cut to
    /// what follows the body.
    fn region(&self, element: &Element, end: usize) -> Vec<SourceLine> {
        let mut block = Block::default();
        let mut region = Vec::new();
        for (index, text) in self.all.iter().enumerate().take(end).skip(element.line - 1) {
            let trimmed = text.trim_start();
            let kind = self.classify(element, index, trimmed, &mut block);
            region.push(SourceLine {
                line: index + 1,
                kind,
                text: (*text).to_string(),
                candidate: (kind == Kind::Comment)
                    .then(|| self.disabled_candidate(trimmed))
                    .flatten(),
            });
        }
        // Comments interspersed in a description are not step annotations.
        // An Examples block contributes only annotations after its table;
        // those closing notes repeat only for rows from that Examples block.
        let body = if element.kind == "examples" {
            Kind::Table
        } else {
            Kind::Description
        };
        let start = region
            .iter()
            .rposition(|line| line.kind == body)
            .map_or(0, |i| i + 1);
        region.split_off(start)
    }

    fn classify(&self, element: &Element, index: usize, trimmed: &str, block: &mut Block) -> Kind {
        let delimiter = ["\"\"\"", "```"]
            .into_iter()
            .find(|f| trimmed.starts_with(f));
        if index + 1 == element.line {
            Kind::Keyword
        } else if self.marks.contains(&(index + 1)) && !trimmed.starts_with('|') {
            // Parser-recognized steps are authoritative. An annotation
            // scan must never suppress execution, even after a fence.
            block.fence = None;
            block.seen_step = true;
            Kind::Step
        } else if self.rows.contains(&(index + 1)) {
            Kind::Table
        } else if let Some(open) = block.fence {
            if closes_docstring(trimmed, open, self.has_newline_after(index)) {
                block.fence = None;
            }
            Kind::Docstring
        } else if let Some(delimiter) = delimiter.filter(|_| block.seen_step) {
            // gherkin accepts inline docstrings and closes at the first
            // matching delimiter, including before a trailing comment.
            block.fence = (!closes_docstring(
                &trimmed[delimiter.len()..],
                delimiter,
                self.has_newline_after(index),
            ))
            .then_some(delimiter);
            Kind::Docstring
        } else if trimmed.is_empty() {
            Kind::Blank
        } else if trimmed.starts_with('#') {
            Kind::Comment
        } else if trimmed.starts_with('@') {
            Kind::Tag
        } else if (block.seen_step || element.kind == "examples") && trimmed.starts_with('|') {
            Kind::Table
        } else {
            Kind::Description
        }
    }

    fn disabled_candidate(&self, trimmed: &str) -> Option<DisabledCandidate> {
        let text = comment_text(trimmed)?.trim_start();
        self.keywords
            .iter()
            .find(|k| text.starts_with(**k))
            .map(|k| DisabledCandidate {
                keyword: (*k).to_string(),
                text: text[k.len()..].trim().to_string(),
            })
    }
}

/// The lines of every table's rows, first through last.
fn table_rows(text: &str, walk: &Walk) -> BTreeSet<usize> {
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
    table_lines
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
