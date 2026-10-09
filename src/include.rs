use crate::feature::{
    ExpandedScenario, ExpandedStep, LoadedFeature, display_path, scenario_placeholders, substitute,
    to_step,
};
use std::path::{Path, PathBuf};

/// Rejects a runtime token or a macro parameter in an include's file
/// literal: the path decides which steps run, so it must be known before
/// interpolation, same reasoning as invariant 1.
pub fn check_literal(path_literal: &str) -> Result<(), String> {
    if path_literal.contains("<<") || path_literal.contains(">>") {
        return Err(format!(
            "I include {path_literal:?}: the path must be a literal — a `<<...>>` \
             runtime token is not allowed, because the path decides which steps \
             run and must be known before the first request"
        ));
    }
    if path_literal.contains('{') || path_literal.contains('}') {
        return Err(format!(
            "I include {path_literal:?}: the path must be a literal — a macro \
             `{{param}}` is not allowed, for the same reason a `<<...>>` token isn't"
        ));
    }
    Ok(())
}

/// A step's file path against the directory of the file the step is written
/// in; an absolute path is used as-is. Shared by `I include` and
/// `I attach the file`, so the two cannot disagree about "relative to".
pub fn join(path: &str, base_dir: &Path) -> PathBuf {
    let candidate = Path::new(path);
    if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        base_dir.join(candidate)
    }
}

/// Resolves an include's file literal relative to `base_dir` (the directory
/// of the `.feature` file, or the macro YAML file, the step is written in).
/// Absolute literals are used as-is.
pub fn resolve(path_literal: &str, base_dir: &Path) -> Result<PathBuf, String> {
    let resolved = join(path_literal, base_dir);
    if !resolved.is_file() {
        return Err(format!(
            "I include {path_literal:?}: no such file ({})",
            display_path(&resolved)
        ));
    }
    if resolved.extension().and_then(|e| e.to_str()) != Some("feature") {
        return Err(format!(
            "I include {path_literal:?}: not a .feature file ({})",
            display_path(&resolved)
        ));
    }
    Ok(resolved)
}

/// Picks the one scenario an include runs. With no name: the file must hold
/// exactly one scenario (an Outline counts as one). With a name: an exact
/// match, and exactly one.
pub fn select_scenario<'a>(
    included: &'a LoadedFeature,
    scenario_name: Option<&str>,
) -> Result<&'a gherkin::Scenario, String> {
    let path = display_path(&included.path);
    match scenario_name {
        None => match included.feature.scenarios.as_slice() {
            [only] => Ok(only),
            scenarios => {
                let names: Vec<&str> = scenarios.iter().map(|s| s.name.as_str()).collect();
                Err(format!(
                    "I include {path:?}: the file must contain exactly one scenario \
                     to be included without naming one, but it has {}: {}",
                    scenarios.len(),
                    names.join(", ")
                ))
            }
        },
        Some(name) => {
            let matches: Vec<&gherkin::Scenario> = included
                .feature
                .scenarios
                .iter()
                .filter(|s| s.name == name)
                .collect();
            match matches.as_slice() {
                [only] => Ok(only),
                [] => Err(format!(
                    "I include {path:?} scenario {name:?}: no scenario with that name"
                )),
                _ => Err(format!(
                    "I include {path:?} scenario {name:?}: more than one scenario has that name"
                )),
            }
        }
    }
}

/// Picks the concrete steps an include runs, in one of three shapes:
/// - `with:` given: the scenario must be an Outline; the table's single data
///   row's columns must exactly match the scenario's own placeholders; every
///   `<name>` in the scenario's steps becomes the runtime token `<<name>>`
///   (reusing `substitute`, aimed at a runtime token instead of a literal),
///   so the value comes from whatever `run_include` seeds into the fresh
///   `VarStack` — not from anything known at validation time.
/// - No `with:`, one Examples row total: the ordinary Outline literal
///   substitution already answers this — `Vec<ExpandedScenario>`'s single
///   element from `expand_outlines` is used unchanged.
/// - A plain `Scenario`, no `with:`: its steps as written.
pub fn build_scenario(
    sc: &gherkin::Scenario,
    with_table: Option<&[Vec<String>]>,
) -> Result<ExpandedScenario, String> {
    match with_table {
        Some(table) => {
            if sc.examples.is_empty() {
                return Err(format!(
                    "I include: `with:` is only valid on a Scenario Outline, \
                     but {:?} is a plain Scenario",
                    sc.name
                ));
            }
            // The row's own values are irrelevant here: with `with:`, every
            // placeholder becomes a runtime token regardless of what this
            // (validation-time, possibly not-yet-interpolated) row contains
            // — only its PRESENCE matters, to enforce "exactly one data row".
            let [header, _row] = table else {
                return Err(format!(
                    "I include {:?}: `with:` must have exactly one data row, found {}",
                    sc.name,
                    table.len().saturating_sub(1)
                ));
            };
            let placeholders = scenario_placeholders(sc);
            for column in header {
                if !placeholders.contains(column) {
                    return Err(format!(
                        "I include {:?}: `with:` column {column:?} is not a \
                         placeholder this scenario uses",
                        sc.name
                    ));
                }
            }
            for name in &placeholders {
                if !header.contains(name) {
                    return Err(format!(
                        "I include {:?}: placeholder <{name}> has no value in `with:`",
                        sc.name
                    ));
                }
            }
            let tokens: Vec<String> = header.iter().map(|name| format!("<<{name}>>")).collect();
            let steps = sc
                .steps
                .iter()
                .map(to_step)
                .map(|s| ExpandedStep {
                    keyword: s.keyword,
                    text: substitute(&s.text, header, &tokens),
                    line: s.line,
                    docstring: s.docstring.map(|d| substitute(&d, header, &tokens)),
                    table: s.table.map(|t| {
                        t.iter()
                            .map(|r| r.iter().map(|c| substitute(c, header, &tokens)).collect())
                            .collect()
                    }),
                })
                .collect();
            Ok(ExpandedScenario {
                name: sc.name.clone(),
                line: sc.position.line,
                example: None,
                steps,
            })
        }
        None => {
            let mut expanded = crate::feature::expand_outlines(sc);
            match expanded.len() {
                1 => Ok(expanded.remove(0)),
                n => Err(format!(
                    "I include {:?}: has {n} Examples rows — pass `with:` to pick one, \
                     or reduce Examples to a single row",
                    sc.name
                )),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A fresh, uniquely named directory under the OS temp dir — this
    /// codebase's own convention (`tests/acceptance.rs`), not `tempfile`.
    /// Tests in this module run in parallel within one process, so the pid
    /// alone (the convention's usual suffix) is not enough when a helper
    /// like `parse_scenario` is called from several tests at once — hence
    /// the added call counter.
    fn temp_test_dir(slug: &str) -> PathBuf {
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        let n = CALLS.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("bddkit-include-{slug}-{}-{n}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn parse_scenario(body: &str) -> gherkin::Scenario {
        let dir = temp_test_dir("parse-scenario");
        let path = dir.join("s.feature");
        fs::write(&path, format!("Feature: f\n{body}")).unwrap();
        let lf = crate::feature::load(&path).unwrap();
        lf.feature.scenarios[0].clone()
    }

    #[test]
    fn check_literal_rejects_a_runtime_token() {
        let err = check_literal("<<name>>.feature").unwrap_err();
        assert!(err.contains("literal"), "{err}");
    }

    #[test]
    fn check_literal_rejects_a_macro_param() {
        let err = check_literal("{name}.feature").unwrap_err();
        assert!(err.contains("literal"), "{err}");
    }

    #[test]
    fn check_literal_accepts_a_plain_path() {
        assert!(check_literal("flows/register.feature").is_ok());
    }

    #[test]
    fn resolve_joins_a_relative_path_to_the_base_dir() {
        let dir = temp_test_dir("resolve-joins");
        fs::create_dir_all(dir.join("flows")).unwrap();
        fs::write(
            dir.join("flows/register.feature"),
            "Feature: f\n  Scenario: s\n    Given x\n",
        )
        .unwrap();
        let resolved = resolve("flows/register.feature", &dir).unwrap();
        assert_eq!(resolved, dir.join("flows/register.feature"));
    }

    #[test]
    fn resolve_rejects_a_missing_file() {
        let dir = temp_test_dir("resolve-missing");
        let err = resolve("nope.feature", &dir).unwrap_err();
        assert!(err.contains("nope.feature"), "{err}");
    }

    #[test]
    fn resolve_rejects_a_non_feature_extension() {
        let dir = temp_test_dir("resolve-non-feature");
        fs::write(dir.join("notes.txt"), "hi").unwrap();
        let err = resolve("notes.txt", &dir).unwrap_err();
        assert!(err.contains(".feature"), "{err}");
    }

    fn write_feature(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn select_scenario_requires_exactly_one_when_no_name_given() {
        let dir = temp_test_dir("select-scenario-multi");
        let path = write_feature(
            &dir,
            "two.feature",
            "Feature: f\n  Scenario: a\n    Given x\n  Scenario: b\n    Given y\n",
        );
        let lf = crate::feature::load(&path).unwrap();
        let err = select_scenario(&lf, None).unwrap_err();
        assert!(err.contains("a") && err.contains("b"), "{err}");
    }

    #[test]
    fn select_scenario_picks_the_one_scenario() {
        let dir = temp_test_dir("select-scenario-one");
        let path = write_feature(
            &dir,
            "one.feature",
            "Feature: f\n  Scenario: only\n    Given x\n",
        );
        let lf = crate::feature::load(&path).unwrap();
        let sc = select_scenario(&lf, None).unwrap();
        assert_eq!(sc.name, "only");
    }

    #[test]
    fn select_scenario_matches_by_exact_name() {
        let dir = temp_test_dir("select-scenario-named");
        let path = write_feature(
            &dir,
            "two.feature",
            "Feature: f\n  Scenario: a\n    Given x\n  Scenario: b\n    Given y\n",
        );
        let lf = crate::feature::load(&path).unwrap();
        let sc = select_scenario(&lf, Some("b")).unwrap();
        assert_eq!(sc.name, "b");
    }

    #[test]
    fn select_scenario_rejects_an_unknown_name() {
        let dir = temp_test_dir("select-scenario-unknown");
        let path = write_feature(
            &dir,
            "one.feature",
            "Feature: f\n  Scenario: only\n    Given x\n",
        );
        let lf = crate::feature::load(&path).unwrap();
        let err = select_scenario(&lf, Some("nope")).unwrap_err();
        assert!(err.contains("nope"), "{err}");
    }

    #[test]
    fn build_scenario_rejects_with_on_a_plain_scenario() {
        let sc = parse_scenario("  Scenario: s\n    Given x\n");
        let table = vec![vec!["a".to_string()], vec!["1".to_string()]];
        let err = build_scenario(&sc, Some(&table)).unwrap_err();
        assert!(err.contains("Scenario Outline"), "{err}");
    }

    #[test]
    fn build_scenario_rejects_more_than_one_data_row() {
        let sc = parse_scenario(
            "  Scenario Outline: s\n    Given I do <a>\n    Examples:\n      | a |\n      | 1 |\n",
        );
        let table = vec![
            vec!["a".to_string()],
            vec!["1".to_string()],
            vec!["2".to_string()],
        ];
        let err = build_scenario(&sc, Some(&table)).unwrap_err();
        assert!(err.contains("one data row"), "{err}");
    }

    #[test]
    fn build_scenario_rejects_a_column_that_is_not_a_placeholder() {
        let sc = parse_scenario(
            "  Scenario Outline: s\n    Given I do <a>\n    Examples:\n      | a |\n      | 1 |\n",
        );
        let table = vec![
            vec!["a".to_string(), "b".to_string()],
            vec!["1".to_string(), "2".to_string()],
        ];
        let err = build_scenario(&sc, Some(&table)).unwrap_err();
        assert!(err.contains('b'), "{err}");
    }

    #[test]
    fn build_scenario_rejects_a_missing_placeholder_value() {
        let sc = parse_scenario(
            "  Scenario Outline: s\n    Given I do <a>\n    And I do <b>\n    Examples:\n      | a | b |\n      | 1 | 2 |\n",
        );
        let table = vec![vec!["a".to_string()], vec!["1".to_string()]];
        let err = build_scenario(&sc, Some(&table)).unwrap_err();
        assert!(err.contains('b'), "{err}");
    }

    #[test]
    fn build_scenario_rewrites_a_with_placeholder_to_a_runtime_token() {
        let sc = parse_scenario(
            "  Scenario Outline: s\n    Given I sign up as \"<email>\"\n    Examples:\n      | email |\n      | x |\n",
        );
        let table = vec![vec!["email".to_string()], vec!["ignored".to_string()]];
        let ex = build_scenario(&sc, Some(&table)).unwrap();
        assert_eq!(ex.steps[0].text, r#"I sign up as "<<email>>""#);
    }

    #[test]
    fn build_scenario_uses_the_lone_examples_row_when_no_with_is_given() {
        let sc = parse_scenario(
            "  Scenario Outline: s\n    Given I sign up as \"<email>\"\n    Examples:\n      | email |\n      | x |\n",
        );
        let ex = build_scenario(&sc, None).unwrap();
        assert_eq!(ex.steps[0].text, r#"I sign up as "x""#);
    }

    #[test]
    fn build_scenario_rejects_multiple_examples_rows_with_no_with() {
        let sc = parse_scenario(
            "  Scenario Outline: s\n    Given I sign up as \"<email>\"\n    Examples:\n      | email |\n      | x |\n      | y |\n",
        );
        let err = build_scenario(&sc, None).unwrap_err();
        assert!(err.contains("with:"), "{err}");
    }
}
