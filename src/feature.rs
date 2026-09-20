use anyhow::{Context, Result};
use gherkin::{Feature, GherkinEnv};
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
    pub steps: Vec<ExpandedStep>,
}

#[derive(Debug)]
pub struct LoadedFeature {
    pub path: PathBuf,
    pub feature: Feature,
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

/// All tags of a file: above `Feature:` and above each scenario. `load` mixes
/// the feature's tags into the scenarios, but a feature with no scenarios must
/// also be readable, so both sources are listed explicitly.
fn all_tags(lf: &LoadedFeature) -> impl Iterator<Item = &String> {
    lf.feature
        .tags
        .iter()
        .chain(lf.feature.scenarios.iter().flat_map(|sc| &sc.tags))
}

static SERIAL_TAG: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"^serial\((.*)\)$").expect("constant regex")
});

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
                lf.path.display()
            ));
        }
        match &found {
            Some(first) if first != name => {
                return Err(format!(
                    "{}: file is tagged with two chains — @serial({first}) and @serial({name}); \
                     a file can belong to only one",
                    lf.path.display()
                ));
            }
            Some(_) => {}
            None => found = Some(name.to_string()),
        }
    }
    Ok(found)
}

static PRIORITY_TAG: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"^priority\((.*)\)$").expect("constant regex")
});

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
                lf.path.display()
            )
        })?;
        best = Some(best.map_or(value, |b: i64| b.max(value)));
    }
    Ok(best.unwrap_or(0))
}

impl LoadedFeature {
    /// Whether the file has at least one scenario passing the filter. Expanding
    /// a Scenario Outline does not change tags, so this can be checked before that.
    pub fn has_selected_scenario(&self, filter: &TagFilter) -> bool {
        self.feature
            .scenarios
            .iter()
            .any(|sc| filter.matches(&sc.tags))
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

static PLACEHOLDER: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"<<(\w+)>>|<(\w+)>").expect("constant regex")
});

/// Substitutes `<key>` from an Examples row. Single pass, because a naive
/// `replace("<key>", v)` would eat the inner `<key>` inside the runtime token `<<key>>`
/// (in `<<userId>>` the substring `<userId>` starts at position 1) — and `<<…>>` must
/// survive untouched until execution. Double brackets match the first alternative
/// and are returned as-is; single brackets are replaced by the column value.
fn substitute(text: &str, keys: &[String], row: &[String]) -> String {
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
                steps,
            });
        }
    }
    out
}

pub fn load(path: &Path) -> Result<LoadedFeature> {
    let mut feature = Feature::parse_path(path, GherkinEnv::default())
        .with_context(|| format!("failed to parse {}", path.display()))?;
    // Tags above `Feature:` apply to all of its scenarios. gherkin stores them
    // separately, but a tester who writes @billing above the feature expects the
    // filter to select the whole file.
    let feature_tags = feature.tags.clone();
    for sc in &mut feature.scenarios {
        sc.tags.extend(feature_tags.iter().cloned());
    }
    Ok(LoadedFeature {
        path: path.to_path_buf(),
        feature,
    })
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
    for entry in
        std::fs::read_dir(p).with_context(|| format!("failed to read {}", p.display()))?
    {
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
            LoadedFeature {
                path: PathBuf::from("t.feature"),
                feature: parse_str(src).expect("gherkin parses"),
            }
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
            assert_eq!(
                serial_of(&lf).expect("names match"),
                Some("x".to_string())
            );
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
            assert!(serial_of(&lf).is_err(), "@serial() without a name must fail");
        }
    }

    mod priority_tag {
        use super::super::*;

        fn loaded(src: &str) -> LoadedFeature {
            LoadedFeature {
                path: PathBuf::from("t.feature"),
                feature: parse_str(src).expect("gherkin parses"),
            }
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
}
