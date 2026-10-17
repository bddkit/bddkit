use anyhow::{Context, Result};
use gherkin::{Feature, GherkinEnv};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ExpandedStep {
    /// `Given `, `When `, `And ` — as written, trailing space included, which
    /// is how the Cucumber JSON report expects it.
    pub keyword: String,
    pub text: String,
    pub line: usize,
    pub docstring: Option<String>,
    pub table: Option<Vec<Vec<String>>>,
}

#[derive(Debug, Clone)]
pub struct ExpandedScenario {
    pub name: String,
    pub line: usize,
    /// 0-based row ordinal across the outline's `Examples` tables; `None` for
    /// a plain scenario. Rows share `line`, so this is what tells them apart.
    pub example: Option<usize>,
    /// The line of the `Examples` block the row came from; `None` for a plain
    /// scenario. Rows of different blocks of one outline share `line`.
    pub examples_line: Option<usize>,
    pub steps: Vec<ExpandedStep>,
}

#[derive(Debug)]
pub struct LoadedFeature {
    pub path: PathBuf,
    pub feature: Feature,
    /// Descriptions as written, by the line of their element's keyword.
    descriptions: BTreeMap<usize, String>,
    /// Each element's own tags as `(name without "@", line)`, by keyword line.
    tags: BTreeMap<usize, Vec<(String, usize)>>,
}

/// Formats a path for user-facing output with `/` throughout, even on Windows,
/// where a path built by joining discovered directory entries mixes `/` (what
/// the tester typed) with `\` (what `Path::join` adds) — see issue #56. Output
/// must stay round-trippable: Windows itself accepts `/` in paths, so this
/// never changes what a tester can paste back into `bddkit run`.
pub fn display_path(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// Selects scenarios by tag. An empty filter lets everything through.
pub struct TagFilter {
    wanted: Vec<String>,
}

impl TagFilter {
    pub fn new(tags: &[String]) -> Self {
        Self {
            wanted: tags.iter().map(|t| strip_at(t).to_string()).collect(),
        }
    }

    /// A scenario is selected if it carries at least one of the requested tags.
    pub fn matches(&self, tags: &[String]) -> bool {
        self.wanted.is_empty()
            || tags
                .iter()
                .any(|t| self.wanted.iter().any(|w| w == strip_at(t)))
    }
}

/// A leading `@` is optional on both sides: `--tag smoke` and `--tag @smoke`
/// both select `@smoke` in the feature file.
fn strip_at(tag: &str) -> &str {
    tag.strip_prefix('@').unwrap_or(tag)
}

/// All tags of a file: above `Feature:`, above each `Rule:` and above each
/// scenario. `load` mixes the outer tags into the scenarios, but a feature or
/// rule with no scenarios must also be readable, so every source is listed
/// explicitly.
fn all_tags(lf: &LoadedFeature) -> impl Iterator<Item = &String> {
    lf.feature
        .tags
        .iter()
        .chain(lf.feature.rules.iter().flat_map(|rule| &rule.tags))
        .chain(lf.scenarios().flat_map(|sc| &sc.scenario.tags))
}

static SERIAL_TAG: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"^serial\((.*)\)$").expect("constant regex"));

/// Chain name from the `@serial(name)` tag. Files in the same chain run strictly one
/// after another; the chain sets ORDER, not shared state — variables live per
/// file (invariant 2) and never flow between the chain's files.
///
/// A file cannot belong to two chains: it runs exactly once.
pub fn serial_of(lf: &LoadedFeature) -> Result<Option<String>, String> {
    let mut found: Option<String> = None;
    for tag in all_tags(lf) {
        let Some(c) = SERIAL_TAG.captures(strip_at(tag)) else {
            continue;
        };
        let name = c.get(1).expect("group 1 is required").as_str().trim();
        if name.is_empty() {
            return Err(format!(
                "{}: @serial() tag has no chain name",
                display_path(&lf.path)
            ));
        }
        match &found {
            Some(first) if first != name => {
                return Err(format!(
                    "{}: file is tagged with two chains — @serial({first}) and @serial({name}); \
                     a file can belong to only one",
                    display_path(&lf.path)
                ));
            }
            Some(_) => {}
            None => found = Some(name.to_string()),
        }
    }
    Ok(found)
}

static PRIORITY_TAG: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"^priority\((.*)\)$").expect("constant regex"));

/// A file's priority from the `@priority(N)` tag: higher goes earlier in the queue,
/// no tag means 0, negatives are allowed. A file takes the MAXIMUM across all its
/// tags — the file is the scheduling unit, and one urgent scenario lifts the whole thing.
///
/// A non-numeric argument is a startup error, not a silent zero: a mistyped tag
/// that silently does nothing is more expensive to debug than a failed start.
pub fn priority_of(lf: &LoadedFeature) -> Result<i64, String> {
    let mut best: Option<i64> = None;
    for tag in all_tags(lf) {
        let Some(c) = PRIORITY_TAG.captures(strip_at(tag)) else {
            continue;
        };
        let raw = c.get(1).expect("group 1 is required").as_str();
        let value = raw.trim().parse::<i64>().map_err(|_| {
            format!(
                "{}: tag @priority({raw}) — the argument must be an integer \
                 (higher goes earlier in the queue, default 0)",
                display_path(&lf.path)
            )
        })?;
        best = Some(best.map_or(value, |b: i64| b.max(value)));
    }
    Ok(best.unwrap_or(0))
}

static EXPORTS_TAG: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"^exports\((.*)\)$").expect("constant regex"));

/// Names an `I include`-d scenario hands back to its caller, from
/// `@exports(a,b,*)`. A scenario may carry more than one such tag (its own
/// plus one inherited from `Feature:`, already merged into `sc.tags` by
/// `load`) — every one contributes, in the order found.
pub fn exports_of(lf: &LoadedFeature, sc: &gherkin::Scenario) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for tag in &sc.tags {
        let Some(c) = EXPORTS_TAG.captures(strip_at(tag)) else {
            continue;
        };
        let raw = c.get(1).expect("group 1 is required").as_str();
        for name in raw.split(',') {
            let name = name.trim();
            if name.is_empty() {
                return Err(format!(
                    "{}: @exports() tag has an empty name",
                    display_path(&lf.path)
                ));
            }
            out.push(name.to_string());
        }
    }
    Ok(out)
}

/// A scenario with the `Rule:` it is written under, if any. The scenario's
/// `tags` are already the effective set (`load` merged the rule's and the
/// feature's into them).
#[derive(Debug, Clone, Copy)]
pub struct ScenarioRef<'a> {
    pub scenario: &'a gherkin::Scenario,
    pub rule: Option<&'a gherkin::Rule>,
}

/// An element of a feature that gherkin gives a keyword line to.
struct Element<'a> {
    /// `feature`, `background`, `rule`, `rule background`, `scenario` or `examples`.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "named in the agreement tests' messages")
    )]
    kind: &'static str,
    line: usize,
    /// gherkin's own description, with what it loses.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the reference the agreement test compares to")
    )]
    description: Option<&'a str>,
    /// gherkin's own tags; `None` where Gherkin has none (a Background).
    tags: Option<&'a [String]>,
}

/// Every element in file order, and the lines of what hangs below them (steps and
/// `Examples` tables) — the one walk of the tree both source scanners start from.
struct Walk<'a> {
    elements: Vec<Element<'a>>,
    below: Vec<usize>,
}

impl<'a> Walk<'a> {
    fn new(f: &'a Feature) -> Self {
        let mut walk = Walk {
            elements: Vec::new(),
            below: Vec::new(),
        };
        walk.push("feature", f.position.line, &f.description, Some(&f.tags));
        if let Some(bg) = &f.background {
            walk.background("background", bg);
        }
        for sc in &f.scenarios {
            walk.scenario(sc);
        }
        for rule in &f.rules {
            walk.push(
                "rule",
                rule.position.line,
                &rule.description,
                Some(&rule.tags),
            );
            if let Some(bg) = &rule.background {
                walk.background("rule background", bg);
            }
            for sc in &rule.scenarios {
                walk.scenario(sc);
            }
        }
        walk
    }

    fn push(
        &mut self,
        kind: &'static str,
        line: usize,
        description: &'a Option<String>,
        tags: Option<&'a [String]>,
    ) {
        let description = description.as_deref();
        self.elements.push(Element {
            kind,
            line,
            description,
            tags,
        });
    }

    fn background(&mut self, kind: &'static str, bg: &'a gherkin::Background) {
        self.push(kind, bg.position.line, &bg.description, None);
        self.below.extend(bg.steps.iter().map(|s| s.position.line));
    }

    fn scenario(&mut self, sc: &'a gherkin::Scenario) {
        self.push(
            "scenario",
            sc.position.line,
            &sc.description,
            Some(&sc.tags),
        );
        self.below.extend(sc.steps.iter().map(|s| s.position.line));
        for ex in &sc.examples {
            self.push(
                "examples",
                ex.position.line,
                &ex.description,
                Some(&ex.tags),
            );
            self.below.extend(ex.table.iter().map(|t| t.position.line));
        }
    }
}

/// Reads every element's own tags from the source. gherkin keeps them as bare
/// names with no position, and `from_text` then merges rule and feature tags into
/// each scenario, so neither the line nor the level survives in the tree. The tags
/// of an element are the `@` lines directly above its keyword, past any blank or
/// `#` lines; a word that begins with `#` starts a comment, as gherkin reads it
/// (`@b#x` is one tag, `b#x`).
fn scan_tags(text: &str, feature: &Feature) -> BTreeMap<usize, Vec<(String, usize)>> {
    let walk = Walk::new(feature);
    let keywords = walk
        .elements
        .iter()
        .filter(|e| e.tags.is_some())
        .map(|e| e.line);
    let lines: Vec<&str> = text.lines().collect();
    keywords
        .filter_map(|keyword| {
            let mut above: Vec<Vec<(String, usize)>> = Vec::new();
            for line in (1..keyword).rev() {
                let text = lines.get(line - 1)?.trim();
                if text.is_empty() || text.starts_with('#') {
                    continue;
                }
                if !text.starts_with('@') {
                    break;
                }
                above.push(
                    text.split_whitespace()
                        .take_while(|word| !word.starts_with('#'))
                        .filter_map(|word| word.strip_prefix('@'))
                        .map(|name| (name.to_string(), line))
                        .collect(),
                );
            }
            let tags: Vec<_> = above.into_iter().rev().flatten().collect();
            (!tags.is_empty()).then_some((keyword, tags))
        })
        .collect()
}

impl LoadedFeature {
    /// A feature built or changed by hand, which has no source text to describe.
    #[cfg(test)]
    pub(crate) fn without_source(path: PathBuf, feature: Feature) -> Self {
        LoadedFeature {
            path,
            feature,
            descriptions: BTreeMap::new(),
            tags: BTreeMap::new(),
        }
    }

    /// Parses `text` the way `load` does for the file at `path`.
    pub fn from_text(path: PathBuf, text: &str) -> Result<Self> {
        let mut feature = Feature::parse(text, GherkinEnv::default())
            .with_context(|| format!("failed to parse {}", display_path(&path)))?;
        // Tags above `Feature:` apply to all of its scenarios. gherkin stores them
        // separately, but a tester who writes @billing above the feature expects the
        // filter to select the whole file.
        let feature_tags = feature.tags.clone();
        for sc in &mut feature.scenarios {
            sc.tags.extend(feature_tags.iter().cloned());
        }
        // A rule's scenario carries its own tags, its rule's and its feature's.
        for rule in &mut feature.rules {
            for sc in &mut rule.scenarios {
                sc.tags.extend(rule.tags.iter().cloned());
                sc.tags.extend(feature_tags.iter().cloned());
            }
        }
        let descriptions = scan_descriptions(text, &feature);
        let tags = scan_tags(text, &feature);
        Ok(LoadedFeature {
            path,
            feature,
            descriptions,
            tags,
        })
    }

    /// Every scenario in file order: those outside any rule, then each rule's.
    /// The one way to walk a feature — nothing reads `feature.scenarios` directly.
    pub fn scenarios(&self) -> impl Iterator<Item = ScenarioRef<'_>> {
        let outside = self.feature.scenarios.iter().map(|scenario| ScenarioRef {
            scenario,
            rule: None,
        });
        let inside = self.feature.rules.iter().flat_map(|rule| {
            rule.scenarios.iter().map(move |scenario| ScenarioRef {
                scenario,
                rule: Some(rule),
            })
        });
        outside.chain(inside)
    }

    /// The feature's Background steps, then the rule's own (as Cucumber does).
    pub fn background_steps(&self, rule: Option<&gherkin::Rule>) -> Vec<ExpandedStep> {
        self.feature
            .background
            .iter()
            .chain(rule.and_then(|r| r.background.as_ref()))
            .flat_map(|bg| bg.steps.iter().map(to_step))
            .collect()
    }

    /// The description written under the element whose keyword is on `line`
    /// (`position.line` of a feature, background, rule, scenario or examples).
    pub fn description(&self, line: usize) -> Option<&str> {
        self.descriptions.get(&line).map(String::as_str)
    }

    /// The tags written directly above the element whose keyword is on `line`
    /// (feature, rule, scenario or examples), each with the line it is written on.
    pub fn own_tags(&self, line: usize) -> &[(String, usize)] {
        self.tags.get(&line).map_or(&[], Vec::as_slice)
    }

    /// What a scenario is tagged with as the filter sees it: its own tags, then
    /// its rule's, then the feature's — the order `from_text` merges them in. A name
    /// written at two levels is listed once, with the line of the first.
    pub fn effective_tags(&self, sc: ScenarioRef<'_>) -> Vec<(&str, usize)> {
        let lines = [
            Some(sc.scenario.position.line),
            sc.rule.map(|r| r.position.line),
            Some(self.feature.position.line),
        ];
        let mut seen = BTreeSet::new();
        lines
            .into_iter()
            .flatten()
            .flat_map(|line| self.own_tags(line))
            .filter(|(name, _)| seen.insert(name))
            .map(|(name, line)| (name.as_str(), *line))
            .collect()
    }

    /// Whether the file has at least one scenario passing the filter. Expanding
    /// a Scenario Outline does not change tags, so this can be checked before that.
    pub fn has_selected_scenario(&self, filter: &TagFilter) -> bool {
        self.scenarios().any(|sc| filter.matches(&sc.scenario.tags))
    }
}

pub fn to_step(s: &gherkin::Step) -> ExpandedStep {
    ExpandedStep {
        keyword: s.keyword.clone(),
        text: s.value.clone(),
        line: s.position.line,
        docstring: s.docstring.clone(),
        table: s.table.as_ref().map(|t| t.rows.clone()),
    }
}

static PLACEHOLDER: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"<<(\w+)>>|<(\w+)>").expect("constant regex"));

/// Every `<name>` (single-bracket, Outline-style) token a scenario's steps,
/// docstrings and table cells use. Used to validate an `I include ... with:`
/// table: its columns must be exactly this set (see `src/include.rs`).
pub fn scenario_placeholders(sc: &gherkin::Scenario) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    let mut collect = |text: &str| {
        for caps in PLACEHOLDER.captures_iter(text) {
            if let Some(m) = caps.get(2) {
                out.insert(m.as_str().to_string());
            }
        }
    };
    for step in &sc.steps {
        collect(&step.value);
        if let Some(d) = &step.docstring {
            collect(d);
        }
        if let Some(t) = &step.table {
            for row in &t.rows {
                for cell in row {
                    collect(cell);
                }
            }
        }
    }
    out
}

/// Substitutes `<key>` from an Examples row. Single pass, because a naive
/// `replace("<key>", v)` would eat the inner `<key>` inside the runtime token `<<key>>`
/// (in `<<userId>>` the substring `<userId>` starts at position 1) — and `<<…>>` must
/// survive untouched until execution. Double brackets match the first alternative
/// and are returned as-is; single brackets are replaced by the column value.
pub(crate) fn substitute(text: &str, keys: &[String], row: &[String]) -> String {
    PLACEHOLDER
        .replace_all(text, |caps: &regex::Captures| {
            if let Some(m) = caps.get(1) {
                return format!("<<{}>>", m.as_str()); // runtime token, leave untouched
            }
            let key = caps
                .get(2)
                .expect("second group when the first is absent")
                .as_str();
            match keys.iter().position(|k| k == key) {
                Some(i) => row[i].clone(),
                None => caps
                    .get(0)
                    .expect("group 0 always exists")
                    .as_str()
                    .to_string(),
            }
        })
        .into_owned()
}

/// Expands a `Scenario Outline` into individual scenarios. A scenario without
/// `Examples` is returned as-is.
pub fn expand_outlines(sc: &gherkin::Scenario) -> Vec<ExpandedScenario> {
    let base: Vec<ExpandedStep> = sc.steps.iter().map(to_step).collect();
    if sc.examples.is_empty() {
        return vec![ExpandedScenario {
            name: sc.name.clone(),
            line: sc.position.line,
            example: None,
            examples_line: None,
            steps: base,
        }];
    }
    let mut out = Vec::new();
    for ex in &sc.examples {
        let Some(table) = &ex.table else { continue };
        let Some(keys) = table.rows.first() else {
            continue;
        };
        for row in table.rows.iter().skip(1) {
            let steps = base
                .iter()
                .map(|s| ExpandedStep {
                    keyword: s.keyword.clone(),
                    text: substitute(&s.text, keys, row),
                    line: s.line,
                    docstring: s.docstring.as_ref().map(|d| substitute(d, keys, row)),
                    table: s.table.as_ref().map(|t| {
                        t.iter()
                            .map(|r| r.iter().map(|c| substitute(c, keys, row)).collect())
                            .collect()
                    }),
                })
                .collect();
            out.push(ExpandedScenario {
                name: format!("{} [{}]", sc.name, row.join(", ")),
                line: sc.position.line,
                example: Some(out.len()),
                examples_line: Some(ex.position.line),
                steps,
            });
        }
    }
    out
}

/// The description a tester wrote, from the source `lines` between an element's
/// keyword line and its first child. gherkin joins paragraphs with a single `\n`,
/// trims every line and drops `#` lines, so what it returns cannot be shown as
/// written. Here `#` lines go, a run of blank lines becomes one paragraph break
/// (none at the edges), and the longest indentation every line shares is removed
/// — a prefix, not a count, because Gherkin allows tabs.
pub(crate) fn description_of<'a>(lines: impl Iterator<Item = &'a str>) -> Option<String> {
    let mut kept: Vec<&str> = Vec::new();
    for line in lines.filter(|l| !l.trim_start().starts_with('#')) {
        let line = line.trim_end();
        if line.is_empty() && kept.last().is_none_or(|l| l.is_empty()) {
            continue;
        }
        kept.push(line);
    }
    if kept.last().is_some_and(|l| l.is_empty()) {
        kept.pop();
    }
    let indent = kept
        .iter()
        .filter(|l| !l.is_empty())
        .map(|l| &l[..l.len() - l.trim_start().len()])
        .reduce(|a, b| {
            let end = a.chars().zip(b.chars()).take_while(|(x, y)| x == y);
            &a[..end.map(|(c, _)| c.len_utf8()).sum()]
        })?;
    let text = kept
        .iter()
        .map(|l| l.strip_prefix(indent).unwrap_or(l))
        .collect::<Vec<_>>()
        .join("\n");
    Some(text)
}

/// Reads every description from the source. An element's description is the text
/// between its keyword line and the next line gherkin gave a position to — the
/// positions, not a list of keywords, so a `# language: ru` file needs nothing
/// extra. Tags have no position, so a tag line ends the text too. A table row needs
/// no rule of its own: gherkin positions every table, and a `|` line elsewhere is
/// description text.
fn scan_descriptions(text: &str, feature: &Feature) -> BTreeMap<usize, String> {
    let walk = Walk::new(feature);
    let lines: Vec<&str> = text.lines().collect();
    let marks: BTreeSet<usize> = walk
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
            let body = lines.get(line..end)?.iter().copied();
            let body = body.take_while(|l| !l.trim_start().starts_with('@'));
            Some((line, description_of(body)?))
        })
        .collect()
}

pub fn load(path: &Path) -> Result<LoadedFeature> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", display_path(path)))?;
    LoadedFeature::from_text(path.to_path_buf(), &text)
}

/// Recursively collects `.feature` files from directories; a file path is taken as-is.
pub fn discover(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for p in paths {
        collect(p, &mut out)?;
    }
    out.sort();
    Ok(out)
}

fn collect(p: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    if p.is_file() {
        out.push(p.to_path_buf());
        return Ok(());
    }
    if !p.is_dir() {
        anyhow::bail!("path {} does not exist", p.display());
    }
    for entry in std::fs::read_dir(p).with_context(|| format!("failed to read {}", p.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "feature") {
            out.push(path);
        }
    }
    Ok(())
}

/// Parses Gherkin from a string. Used by tests; `parse_path` reads files.
#[cfg(test)]
pub fn parse_str(src: &str) -> Result<Feature> {
    Feature::parse(src, GherkinEnv::default()).context("failed to parse Gherkin")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_background_scenario_and_steps() {
        let src = "\
Feature: demo
  Background:
    Given the \"Accept\" request header is \"application/json\"
  Scenario: ping
    When I request \"/ping\" using HTTP POST
    Then the response code is 200
";
        let f = parse_str(src).unwrap();
        assert_eq!(f.name, "demo");
        assert_eq!(f.background.as_ref().unwrap().steps.len(), 1);
        assert_eq!(f.scenarios.len(), 1);

        let sc = &f.scenarios[0];
        assert_eq!(sc.name, "ping");
        assert_eq!(sc.steps.len(), 2);
        // value does not include the keyword
        assert_eq!(sc.steps[0].value, "I request \"/ping\" using HTTP POST");
        assert_eq!(sc.steps[0].position.line, 5);
    }

    #[test]
    fn scenario_without_examples_expands_to_one() {
        let f =
            parse_str("Feature: f\n  Scenario: s\n    Then the response code is 200\n").unwrap();
        let e = expand_outlines(&f.scenarios[0]);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].name, "s");
        assert_eq!(e[0].steps[0].text, "the response code is 200");
    }

    #[test]
    fn outline_expands_one_scenario_per_example_row() {
        let src = "\
Feature: f
  Scenario Outline: method not allowed
    When I request \"/ping\" using HTTP <method>
    Then the response code is <code>
    Examples:
      | method | code |
      | POST   | 405  |
      | DELETE | 405  |
";
        let f = parse_str(src).unwrap();
        let e = expand_outlines(&f.scenarios[0]);
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].steps[0].text, "I request \"/ping\" using HTTP POST");
        assert_eq!(e[0].steps[1].text, "the response code is 405");
        assert_eq!(e[1].steps[0].text, "I request \"/ping\" using HTTP DELETE");
        assert!(
            e[0].name.contains("POST"),
            "the name must distinguish rows: {}",
            e[0].name
        );
    }

    #[test]
    fn outline_substitutes_inside_docstring() {
        let src = "\
Feature: f
  Scenario Outline: body
    Given the request body is:
      \"\"\"
      {\"email\": \"<email>\"}
      \"\"\"
    Examples:
      | email   |
      | a@b.net |
";
        let f = parse_str(src).unwrap();
        let e = expand_outlines(&f.scenarios[0]);
        assert_eq!(e.len(), 1);
        assert!(
            e[0].steps[0]
                .docstring
                .as_ref()
                .unwrap()
                .contains("a@b.net")
        );
    }

    #[test]
    fn step_carries_docstring_and_table() {
        let src = "\
Feature: f
  Scenario: s
    Given the request form parameters are:
      | name  | value |
      | email | a@b   |
";
        let f = parse_str(src).unwrap();
        let e = expand_outlines(&f.scenarios[0]);
        let t = e[0].steps[0].table.as_ref().unwrap();
        assert_eq!(t[0], vec!["name".to_string(), "value".to_string()]);
        assert_eq!(t[1], vec!["email".to_string(), "a@b".to_string()]);
    }

    #[test]
    fn runtime_token_survives_when_column_name_collides() {
        // `<<userId>>` contains the substring `<userId>`; a naive replace would eat it.
        let src = "\
Feature: f
  Scenario Outline: name collision
    When I request \"/users/<<userId>>/<action>\" using HTTP GET
    Examples:
      | userId | action |
      | 42     | ban    |
";
        let f = parse_str(src).unwrap();
        let e = expand_outlines(&f.scenarios[0]);
        assert_eq!(e.len(), 1);
        assert_eq!(
            e[0].steps[0].text,
            "I request \"/users/<<userId>>/ban\" using HTTP GET"
        );
    }

    #[test]
    fn outline_substitutes_inside_table_cells() {
        let src = "\
Feature: f
  Scenario Outline: table
    Given the request form parameters are:
      | name  | value    |
      | email | <email>  |
    Examples:
      | email   |
      | a@b.net |
";
        let f = parse_str(src).unwrap();
        let e = expand_outlines(&f.scenarios[0]);
        assert_eq!(e.len(), 1);
        let t = e[0].steps[0].table.as_ref().unwrap();
        assert_eq!(t[1], vec!["email".to_string(), "a@b.net".to_string()]);
    }

    mod tag_filter {
        use super::super::TagFilter;

        #[test]
        fn an_empty_filter_matches_every_scenario() {
            let f = TagFilter::new(&[]);
            assert!(f.matches(&[]));
        }

        #[test]
        fn matches_when_one_of_the_wanted_tags_is_present() {
            let f = TagFilter::new(&["smoke".to_string(), "slow".to_string()]);
            assert!(f.matches(&["slow".to_string()]));
        }

        #[test]
        fn a_leading_at_sign_is_optional_on_the_argument() {
            let f = TagFilter::new(&["@smoke".to_string()]);
            assert!(f.matches(&["smoke".to_string()]));
        }

        #[test]
        fn a_leading_at_sign_is_optional_on_the_gherkin_tag() {
            let f = TagFilter::new(&["smoke".to_string()]);
            assert!(f.matches(&["@smoke".to_string()]));
        }

        #[test]
        fn matches_a_scenario_that_carries_several_tags() {
            let f = TagFilter::new(&["slow".to_string()]);
            assert!(f.matches(&[
                "@billing".to_string(),
                "@slow".to_string(),
                "@wip".to_string(),
            ]));
        }

        #[test]
        fn an_untagged_scenario_is_rejected_by_a_non_empty_filter() {
            let f = TagFilter::new(&["smoke".to_string()]);
            assert!(!f.matches(&[]));
        }

        #[test]
        fn an_unrelated_tag_does_not_match() {
            let f = TagFilter::new(&["smoke".to_string()]);
            assert!(!f.matches(&["wip".to_string()]));
        }
    }

    mod serial_tag {
        use super::super::*;

        fn loaded(src: &str) -> LoadedFeature {
            LoadedFeature::from_text(PathBuf::from("t.feature"), src).expect("gherkin parses")
        }

        #[test]
        fn a_file_without_the_tag_belongs_to_no_chain() {
            let lf = loaded("Feature: f\n  Scenario: s\n    Then the response code is 200\n");
            assert_eq!(serial_of(&lf).expect("no tag is not an error"), None);
        }

        #[test]
        fn a_feature_level_tag_names_the_chain() {
            let lf = loaded(
                "@serial(companies)\nFeature: f\n  Scenario: s\n    Then the response code is 200\n",
            );
            assert_eq!(
                serial_of(&lf).expect("tag is valid"),
                Some("companies".to_string())
            );
        }

        #[test]
        fn a_scenario_level_tag_names_the_chain_for_the_whole_file() {
            // The scheduling unit is the file: one tagged scenario pulls the
            // whole file along, there is nothing to split.
            let lf = loaded(
                "Feature: f\n  @serial(companies)\n  Scenario: s\n    Then the response code is 200\n",
            );
            assert_eq!(
                serial_of(&lf).expect("tag is valid"),
                Some("companies".to_string())
            );
        }

        #[test]
        fn the_same_chain_named_twice_is_not_a_conflict() {
            let lf = loaded(
                "@serial(x)\nFeature: f\n  @serial(x)\n  Scenario: s\n    Then the response code is 200\n",
            );
            assert_eq!(serial_of(&lf).expect("names match"), Some("x".to_string()));
        }

        #[test]
        fn two_different_chains_in_one_file_is_an_error_naming_both() {
            let lf = loaded(
                "@serial(a)\nFeature: f\n  @serial(b)\n  Scenario: s\n    Then the response code is 200\n",
            );
            let err = serial_of(&lf).expect_err("a file cannot be in two chains");
            assert!(err.contains('a') && err.contains('b'), "{err}");
        }

        #[test]
        fn an_empty_chain_name_is_an_error() {
            let lf =
                loaded("@serial()\nFeature: f\n  Scenario: s\n    Then the response code is 200\n");
            assert!(
                serial_of(&lf).is_err(),
                "@serial() without a name must fail"
            );
        }
    }

    mod priority_tag {
        use super::super::*;

        fn loaded(src: &str) -> LoadedFeature {
            LoadedFeature::from_text(PathBuf::from("t.feature"), src).expect("gherkin parses")
        }

        #[test]
        fn a_file_without_the_tag_has_priority_zero() {
            let lf = loaded("Feature: f\n  Scenario: s\n    Then the response code is 200\n");
            assert_eq!(priority_of(&lf).expect("no tag is not an error"), 0);
        }

        #[test]
        fn a_feature_level_tag_sets_the_priority() {
            let lf = loaded(
                "@priority(5)\nFeature: f\n  Scenario: s\n    Then the response code is 200\n",
            );
            assert_eq!(priority_of(&lf).expect("tag is valid"), 5);
        }

        #[test]
        fn a_negative_priority_is_accepted() {
            let lf = loaded(
                "@priority(-5)\nFeature: f\n  Scenario: s\n    Then the response code is 200\n",
            );
            assert_eq!(priority_of(&lf).expect("tag is valid"), -5);
        }

        #[test]
        fn a_file_takes_the_highest_priority_among_its_tags() {
            // The scheduling unit is the file: one urgent scenario lifts the whole file.
            let lf = loaded(
                "@priority(1)\nFeature: f\n  @priority(9)\n  Scenario: s\n    Then the response code is 200\n",
            );
            assert_eq!(priority_of(&lf).expect("tag is valid"), 9);
        }

        #[test]
        fn a_non_numeric_priority_is_an_error_naming_the_file() {
            let lf = loaded(
                "@priority(urgent)\nFeature: f\n  Scenario: s\n    Then the response code is 200\n",
            );
            let err = priority_of(&lf).expect_err("argument is not a number");
            assert!(err.contains("t.feature"), "{err}");
        }

        #[test]
        fn an_empty_priority_is_an_error() {
            let lf = loaded(
                "@priority()\nFeature: f\n  Scenario: s\n    Then the response code is 200\n",
            );
            assert!(priority_of(&lf).is_err(), "@priority() must fail");
        }
    }

    #[test]
    fn display_path_normalizes_backslashes_to_forward_slashes() {
        let p = PathBuf::from("features\\api\\users\\create.feature");
        assert_eq!(display_path(&p), "features/api/users/create.feature");
    }

    #[test]
    fn display_path_leaves_a_unix_style_path_unchanged() {
        let p = PathBuf::from("features/api/users/create.feature");
        assert_eq!(display_path(&p), "features/api/users/create.feature");
    }

    fn describe(src: &str) -> Option<String> {
        description_of(src.lines())
    }

    #[test]
    fn a_description_keeps_its_paragraphs() {
        assert_eq!(
            describe("  First line\n  second line\n\n  Second paragraph\n").as_deref(),
            Some("First line\nsecond line\n\nSecond paragraph")
        );
    }

    #[test]
    fn a_description_keeps_relative_indentation() {
        assert_eq!(
            describe("    Steps:\n      - one\n      - two\n    Done\n").as_deref(),
            Some("Steps:\n  - one\n  - two\nDone")
        );
    }

    #[test]
    fn a_comment_line_is_not_part_of_a_description() {
        assert_eq!(describe("  a\n  # note\n  b\n").as_deref(), Some("a\nb"));
        assert_eq!(
            describe("  a\n\n  # note\n\n  b\n").as_deref(),
            Some("a\n\nb")
        );
        assert_eq!(describe("  # only a comment\n"), None);
    }

    #[test]
    fn blank_runs_collapse_and_the_edges_are_dropped() {
        assert_eq!(
            describe("\n\n  a\n\n\n\n  b\n\n").as_deref(),
            Some("a\n\nb")
        );
        assert_eq!(describe("\n  \n\t\n"), None);
        assert_eq!(describe(""), None);
    }

    #[test]
    fn crlf_does_not_leave_a_carriage_return_behind() {
        assert_eq!(describe("  a\r\n  \r\n  b\r\n").as_deref(), Some("a\n\nb"));
    }

    #[test]
    fn indentation_is_removed_by_the_exact_common_prefix() {
        assert_eq!(describe("\t\ta\n\t\t  b\n").as_deref(), Some("a\n  b"));
        // A tab against spaces shares no prefix: nothing is cut in the wrong place.
        assert_eq!(describe("\ta\n    b\n").as_deref(), Some("\ta\n    b"));
    }

    #[test]
    fn a_non_ascii_space_in_the_indent_does_not_split_a_character() {
        assert_eq!(
            describe("\u{a0}\u{a0}a\n\u{a0}b\n").as_deref(),
            Some("\u{a0}a\nb")
        );
    }

    const DESCRIBED: &str = "# language: en
@ft1
# c
@ft2
Feature: F
  feature desc

  feature para

  Background: B
    bg desc
    Given bg step

  @s1
  # c
  @s2 # see @x
  Scenario Outline: S
    sc desc
      indented
    # a comment
    Given a <x>
      | h |
      | v |

    @e1
    Examples: E
      ex desc
      | x |
      | 1 |

  Scenario: Bare
    Given b

  @r1
  Rule: R
    rule desc
    Background: RB
      rb desc
      Given rb step
    Scenario: RS
      rs desc
      Given z
";

    fn described() -> LoadedFeature {
        LoadedFeature::from_text(PathBuf::from("t.feature"), DESCRIBED).unwrap()
    }

    #[test]
    fn every_kind_of_element_gets_its_description_by_keyword_line() {
        let lf = described();
        let f = &lf.feature;
        let sc = &f.scenarios[0];
        let rule = &f.rules[0];
        let said = |line| lf.description(line);
        assert_eq!(said(f.position.line), Some("feature desc\n\nfeature para"));
        assert_eq!(
            said(f.background.as_ref().unwrap().position.line),
            Some("bg desc")
        );
        assert_eq!(said(sc.position.line), Some("sc desc\n  indented"));
        assert_eq!(said(sc.examples[0].position.line), Some("ex desc"));
        assert_eq!(said(rule.position.line), Some("rule desc"));
        assert_eq!(
            said(rule.background.as_ref().unwrap().position.line),
            Some("rb desc")
        );
        assert_eq!(said(rule.scenarios[0].position.line), Some("rs desc"));
    }

    /// Every `.feature` in the repository's own suites, and `DESCRIBED`, as
    /// `(name, source)` — what the agreement tests check the scanners against.
    fn fixture_sources() -> Vec<(String, String)> {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let dirs: Vec<PathBuf> = ["tests", "examples", "selftest"]
            .iter()
            .map(|d| root.join(d))
            .collect();
        let files = discover(&dirs).unwrap();
        assert!(files.len() > 20, "found only {files:?}");
        let mut sources: Vec<_> = files
            .iter()
            .map(|f| (display_path(f), std::fs::read_to_string(f).unwrap()))
            .collect();
        sources.push(("DESCRIBED".into(), DESCRIBED.into()));
        sources
    }

    /// What two descriptions say, ignoring what gherkin throws away or we keep:
    /// blank lines and indentation.
    fn said(text: Option<&str>) -> Vec<&str> {
        text.map_or_else(Vec::new, |t| {
            t.lines().map(str::trim).filter(|l| !l.is_empty()).collect()
        })
    }

    #[test]
    fn the_parser_and_the_scanner_agree_on_which_elements_have_a_description() {
        let mut with_text: BTreeMap<&str, usize> = BTreeMap::new();
        for (name, text) in &fixture_sources() {
            let lf = LoadedFeature::from_text(PathBuf::from(name), text)
                .unwrap_or_else(|e| panic!("{name} does not parse: {e:#}"));
            for Element {
                kind,
                line,
                description: parsed,
                ..
            } in Walk::new(&lf.feature).elements
            {
                let scanned = lf.description(line);
                assert_eq!(
                    said(parsed),
                    said(scanned),
                    "{name}:{line} ({kind}): gherkin says {parsed:?}, the scanner {scanned:?}"
                );
                assert_eq!(
                    parsed.is_some(),
                    scanned.is_some(),
                    "{name}:{line} ({kind}): gherkin says {parsed:?}, the scanner {scanned:?}"
                );
                if scanned.is_some() {
                    *with_text.entry(kind).or_default() += 1;
                }
            }
        }
        for kind in [
            "feature",
            "background",
            "rule",
            "rule background",
            "scenario",
            "examples",
        ] {
            assert!(
                with_text.get(kind).is_some_and(|n| *n > 0),
                "{kind}: no description seen"
            );
        }
    }

    /// `(name, line)` of an element's own tags, names without the `@`.
    fn own(lf: &LoadedFeature, keyword_line: usize) -> Vec<(&str, usize)> {
        lf.own_tags(keyword_line)
            .iter()
            .map(|(name, line)| (name.as_str(), *line))
            .collect()
    }

    #[test]
    fn tags_keep_their_own_line_and_order_per_element() {
        let src = "@a @b\n# c\n@c\nFeature: F\n\n  @s1 @s2 # trailing @x\n  # c\n\n  @s3\n  Scenario: S\n    Given x\n\n  @r1\n  Rule: R\n    @e\n    Scenario: T\n      Given y\n";
        let lf = LoadedFeature::from_text(PathBuf::from("t.feature"), src).unwrap();
        let f = &lf.feature;
        assert_eq!(own(&lf, f.position.line), [("a", 1), ("b", 1), ("c", 3)]);
        assert_eq!(
            own(&lf, f.scenarios[0].position.line),
            [("s1", 6), ("s2", 6), ("s3", 9)]
        );
        assert_eq!(own(&lf, f.rules[0].position.line), [("r1", 13)]);
        assert_eq!(own(&lf, f.rules[0].scenarios[0].position.line), [("e", 15)]);
    }

    #[test]
    fn a_scenario_lists_its_own_tags_then_its_rules_then_the_features() {
        let src = "@f\nFeature: F\n  @r @f\n  Rule: R\n    @s @r\n    Scenario: T\n      Given y\n";
        let lf = LoadedFeature::from_text(PathBuf::from("t.feature"), src).unwrap();
        let sc = lf.scenarios().next().unwrap();
        let tags: Vec<_> = lf
            .effective_tags(sc)
            .into_iter()
            .map(|(n, l)| (n.to_string(), l))
            .collect();
        // `r` is also the scenario's own tag and `f` the rule's: the first mention wins.
        assert_eq!(
            tags,
            [
                ("s".to_string(), 5),
                ("r".to_string(), 5),
                ("f".to_string(), 3)
            ]
        );
    }

    #[test]
    fn the_scanned_tags_are_the_ones_gherkin_parsed_for_every_element() {
        let mut tagged: BTreeMap<&str, usize> = BTreeMap::new();
        for (name, text) in &fixture_sources() {
            let lf = LoadedFeature::from_text(PathBuf::from(name), text).unwrap();
            // `from_text` merges feature and rule tags into each scenario; the
            // parser's own, unmerged view is the reference.
            let raw = Feature::parse(text, GherkinEnv::default()).unwrap();
            for Element {
                kind, line, tags, ..
            } in Walk::new(&raw).elements
            {
                let Some(tags) = tags else { continue };
                let scanned: Vec<&str> =
                    lf.own_tags(line).iter().map(|(n, _)| n.as_str()).collect();
                assert_eq!(tags, scanned.as_slice(), "{name}:{line} ({kind})");
                if !tags.is_empty() {
                    *tagged.entry(kind).or_default() += 1;
                }
            }
        }
        for kind in ["feature", "rule", "scenario", "examples"] {
            assert!(
                tagged.get(kind).is_some_and(|n| *n > 0),
                "{kind}: no tags seen"
            );
        }
    }

    #[test]
    fn a_description_line_that_starts_with_a_pipe_is_kept() {
        let src = "Feature: F\n  intro\n  | not a table\n  Scenario: s\n    why | because\n    | x |\n    Given set variable \"a\" to \"1\"\n";
        let lf = LoadedFeature::from_text(PathBuf::from("t.feature"), src).unwrap();
        assert_eq!(lf.description(1), Some("intro\n| not a table"));
        assert_eq!(lf.description(4), Some("why | because\n| x |"));
        // gherkin reads the same lines as description text.
        for Element {
            line, description, ..
        } in Walk::new(&lf.feature).elements
        {
            assert_eq!(said(description), said(lf.description(line)), "line {line}");
        }
    }

    #[test]
    fn an_element_without_a_description_has_none() {
        let lf = described();
        assert_eq!(lf.description(lf.feature.scenarios[1].position.line), None);
        assert_eq!(lf.description(999), None);
    }

    #[test]
    fn a_tag_line_ends_the_description_above_it() {
        let src = "Feature: F\n  desc\n  @tag\n  Scenario: S\n    Given a\n";
        let lf = LoadedFeature::from_text(PathBuf::from("t.feature"), src).unwrap();
        assert_eq!(lf.description(1), Some("desc"));
    }

    #[test]
    fn a_description_does_not_need_the_english_keywords() {
        let src = "# language: ru\nФункционал: F\n  описание\n  Сценарий: S\n    Допустим a\n";
        let lf = LoadedFeature::from_text(PathBuf::from("t.feature"), src).unwrap();
        assert_eq!(lf.description(2), Some("описание"));
    }

    #[test]
    fn load_names_the_path_when_the_file_cannot_be_read() {
        let path = std::env::temp_dir()
            .join("bddkit_no_such_dir")
            .join("a.feature");
        let err = format!("{:#}", load(&path).unwrap_err());
        assert!(err.contains("failed to read"), "{err}");
        assert!(err.contains("bddkit_no_such_dir/a.feature"), "{err}");
    }

    #[test]
    fn load_names_the_path_and_position_of_a_syntax_error() {
        let path = std::env::temp_dir().join("bddkit_syntax_error.feature");
        std::fs::write(
            &path,
            "Feature: x\n  Scenario: a\n    Given foo\n  Bogus line\n",
        )
        .unwrap();
        let err = format!("{:#}", load(&path).unwrap_err());
        std::fs::remove_file(&path).ok();
        assert!(err.contains("failed to parse"), "{err}");
        assert!(err.contains("bddkit_syntax_error.feature"), "{err}");
        assert!(err.contains("Error at 5:1"), "{err}");
    }

    #[test]
    fn feature_tags_are_inherited_by_every_scenario() {
        let path = std::env::temp_dir().join("bddkit_feature_tags_inherited.feature");
        std::fs::write(
            &path,
            "@billing\nFeature: f\n  @smoke\n  Scenario: s\n    When I request \"/x\"\n",
        )
        .expect("write feature file");
        let lf = load(&path).expect("file parses");
        assert!(
            lf.feature.scenarios[0]
                .tags
                .contains(&"billing".to_string()),
            "the feature tag must land on the scenario: {:?}",
            lf.feature.scenarios[0].tags
        );
        // The merge must APPEND, not replace: swapping `extend` for
        // assigning the feature's tag list would silently eat the scenario's own tag.
        assert!(
            lf.feature.scenarios[0].tags.contains(&"smoke".to_string()),
            "the scenario's own tag must survive: {:?}",
            lf.feature.scenarios[0].tags
        );
    }

    mod exports_tag {
        use super::super::*;

        fn loaded(src: &str) -> LoadedFeature {
            LoadedFeature::from_text(PathBuf::from("t.feature"), src).expect("gherkin parses")
        }

        #[test]
        fn exports_of_reads_a_single_tag() {
            let lf = loaded(
                "Feature: f\n  @exports(userId)\n  Scenario: s\n    Then the response code is 200\n",
            );
            let sc = &lf.feature.scenarios[0];
            assert_eq!(exports_of(&lf, sc).unwrap(), vec!["userId".to_string()]);
        }

        #[test]
        fn exports_of_splits_comma_separated_names_and_trims() {
            let lf = loaded(
                "Feature: f\n  @exports(userId,token)\n  Scenario: s\n    Then the response code is 200\n",
            );
            let sc = &lf.feature.scenarios[0];
            assert_eq!(
                exports_of(&lf, sc).unwrap(),
                vec!["userId".to_string(), "token".to_string()]
            );
        }

        #[test]
        fn exports_of_keeps_a_trailing_glob_star_as_one_token() {
            let lf = loaded(
                "Feature: f\n  @exports(last_insert_id_*)\n  Scenario: s\n    Then the response code is 200\n",
            );
            let sc = &lf.feature.scenarios[0];
            assert_eq!(
                exports_of(&lf, sc).unwrap(),
                vec!["last_insert_id_*".to_string()]
            );
        }

        #[test]
        fn exports_of_unions_two_exports_tags() {
            let lf = loaded(
                "Feature: f\n  @exports(a) @exports(b)\n  Scenario: s\n    Then the response code is 200\n",
            );
            let sc = &lf.feature.scenarios[0];
            assert_eq!(
                exports_of(&lf, sc).unwrap(),
                vec!["a".to_string(), "b".to_string()]
            );
        }

        #[test]
        fn exports_of_is_empty_with_no_tag() {
            let lf = loaded("Feature: f\n  Scenario: s\n    Then the response code is 200\n");
            let sc = &lf.feature.scenarios[0];
            assert_eq!(exports_of(&lf, sc).unwrap(), Vec::<String>::new());
        }
    }

    fn parse_inline_scenario(body: &str) -> gherkin::Scenario {
        let src = format!("Feature: f\n  {body}");
        let f = parse_str(&src).expect("gherkin parses");
        f.scenarios[0].clone()
    }

    #[test]
    fn scenario_placeholders_collects_every_name_used() {
        let sc = parse_inline_scenario(
            "Scenario Outline: s\n  Given I do <a>\n  When I check:\n    \"\"\"\n    <b>\n    \"\"\"\n",
        );
        let names = scenario_placeholders(&sc);
        assert!(names.contains("a"));
        assert!(names.contains("b"));
        assert_eq!(names.len(), 2);
    }

    #[test]
    fn scenario_placeholders_ignores_double_bracket_runtime_tokens() {
        let sc = parse_inline_scenario("Scenario Outline: s\n  Given I use \"<<userId>>\"\n");
        assert!(scenario_placeholders(&sc).is_empty());
    }
}
