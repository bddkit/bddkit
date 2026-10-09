use crate::feature::{LoadedFeature, TagFilter, display_path, expand_outlines};
use crate::steps::{Registry, StepTarget};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Problem {
    pub file: PathBuf,
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "  {}:{}\n    {}",
            display_path(&self.file),
            self.line,
            self.message
        )
    }
}

struct SelectedStep {
    text: String,
    line: usize,
    has_docstring: bool,
    table: Option<Vec<Vec<String>>>,
}

/// Matches every step of every file before the first request. Returns ALL
/// problems at once — a run that fails midway due to a typo costs more
/// than a full check that takes milliseconds.
pub fn check(features: &[&LoadedFeature], reg: &Registry, filter: &TagFilter) -> Vec<Problem> {
    let mut problems = Vec::new();
    for lf in features {
        let base_dir = lf
            .path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        for step in selected_steps(lf, filter) {
            match reg.find(&step.text) {
                Ok(Some((StepTarget::Macro(_), _))) if step.has_docstring => {
                    problems.push(Problem {
                        file: lf.path.clone(),
                        line: step.line,
                        message: "macro calls do not support a docstring".into(),
                    });
                }
                Ok(Some((StepTarget::Macro(_), _))) if step.table.is_some() => {
                    problems.push(Problem {
                        file: lf.path.clone(),
                        line: step.line,
                        message: "macro calls do not support a table".into(),
                    })
                }
                Ok(Some((StepTarget::Builtin { id, .. }, caps)))
                    if matches!(
                        id,
                        crate::steps::StepId::Include | crate::steps::StepId::IncludeScenario
                    ) =>
                {
                    if step.has_docstring {
                        problems.push(Problem {
                            file: lf.path.clone(),
                            line: step.line,
                            message: "I include does not support a docstring".into(),
                        });
                        continue;
                    }
                    let scenario_name = if id == crate::steps::StepId::IncludeScenario {
                        Some(caps[1].as_str())
                    } else {
                        None
                    };
                    if let Err(message) = check_one_include(
                        &caps[0],
                        scenario_name,
                        &base_dir,
                        step.table.as_deref(),
                        reg,
                    ) {
                        problems.push(Problem {
                            file: lf.path.clone(),
                            line: step.line,
                            message,
                        });
                    }
                }
                Ok(Some((
                    StepTarget::Builtin {
                        id: crate::steps::StepId::AttachFile,
                        ..
                    },
                    caps,
                ))) => {
                    if let Err(message) = check_attached_file(&caps[0], &base_dir) {
                        problems.push(Problem {
                            file: lf.path.clone(),
                            line: step.line,
                            message,
                        });
                    }
                }
                Ok(Some(_)) => {}
                Ok(None) => problems.push(Problem {
                    file: lf.path.clone(),
                    line: step.line,
                    message: format!("unknown step: {:?}", step.text),
                }),
                Err(e) => problems.push(Problem {
                    file: lf.path.clone(),
                    line: step.line,
                    message: e,
                }),
            }
        }
    }
    problems
}

/// Every step a run would execute from this file. Background is always
/// included: it runs before every selected scenario. Filtered-out scenarios
/// are not — a typo in something that never runs must not fail the run.
fn selected_steps(lf: &LoadedFeature, filter: &TagFilter) -> Vec<SelectedStep> {
    let mut all_steps: Vec<SelectedStep> = Vec::new();
    if let Some(bg) = &lf.feature.background {
        for s in &bg.steps {
            all_steps.push(SelectedStep {
                text: s.value.clone(),
                line: s.position.line,
                has_docstring: s.docstring.is_some(),
                table: s.table.as_ref().map(|t| t.rows.clone()),
            });
        }
    }
    for sc in &lf.feature.scenarios {
        if !filter.matches(&sc.tags) {
            continue;
        }
        for ex in expand_outlines(sc) {
            for st in ex.steps {
                all_steps.push(SelectedStep {
                    text: st.text,
                    line: st.line,
                    has_docstring: st.docstring.is_some(),
                    table: st.table,
                });
            }
        }
    }
    all_steps
}

/// The steps `check` matched, asked a second question: does the resource each
/// one reaches for exist. `unserved` answers `Some(message)` for a step whose
/// group has nothing declared to run on. One finding per file and message —
/// the first such step names the line, every later one would only repeat it.
///
/// Deliberately NOT part of `check`, and so never a reason for `run` to exit
/// 2: a suite may declare an empty group and keep the scenarios using it out
/// with `--tag`, and `run` does not know what will be selected until it is.
/// `doctor` applies no filter, so it is the one caller with an answer.
pub fn unserved_resources(
    features: &[&LoadedFeature],
    reg: &Registry,
    filter: &TagFilter,
    unserved: impl Fn(&StepTarget) -> Option<String>,
) -> Vec<Problem> {
    let mut problems: Vec<Problem> = Vec::new();
    for lf in features {
        for step in selected_steps(lf, filter) {
            let Ok(Some((target, _))) = reg.find(&step.text) else {
                continue;
            };
            let Some(message) = unserved(&target) else {
                continue;
            };
            if problems
                .iter()
                .any(|p| p.file == lf.path && p.message == message)
            {
                continue;
            }
            problems.push(Problem {
                file: lf.path.clone(),
                line: step.line,
                message,
            });
        }
    }
    problems
}

/// One include call, checked in full: literal path, resolution, scenario
/// selection, its `with:`/Examples shape, and — recursively — every step of
/// the selected scenario itself (Background + body), including any further
/// `I include` it contains. Detects a cycle (the same resolved absolute file
/// and scenario name reappearing on the current call stack) and caps nesting
/// at depth 16, mirroring the macro cycle-DFS's reasoning without sharing its
/// code (see CLAUDE.md).
fn check_one_include(
    path_literal: &str,
    scenario_name: Option<&str>,
    base_dir: &Path,
    with_table: Option<&[Vec<String>]>,
    reg: &Registry,
) -> Result<(), String> {
    let mut stack = Vec::new();
    check_include_recursive(
        path_literal,
        scenario_name,
        base_dir,
        with_table,
        reg,
        &mut stack,
    )
}

fn check_include_recursive(
    path_literal: &str,
    scenario_name: Option<&str>,
    base_dir: &Path,
    with_table: Option<&[Vec<String>]>,
    reg: &Registry,
    stack: &mut Vec<(PathBuf, String)>,
) -> Result<(), String> {
    if stack.len() >= 16 {
        return Err(format!(
            "I include {path_literal:?}: nesting exceeds 16 (chain: {})",
            render_chain(stack)
        ));
    }
    crate::include::check_literal(path_literal)?;
    let resolved = crate::include::resolve(path_literal, base_dir)?;
    let canonical = resolved.canonicalize().unwrap_or_else(|_| resolved.clone());
    let included = crate::feature::load(&resolved).map_err(|e| e.to_string())?;
    let scenario = crate::include::select_scenario(&included, scenario_name)?;
    crate::feature::exports_of(&included, scenario)?;
    let node = (canonical, scenario.name.clone());
    if let Some(pos) = stack.iter().position(|n| n == &node) {
        let mut chain: Vec<String> = stack[pos..]
            .iter()
            .map(|(p, s)| format!("{} › {s:?}", display_path(p)))
            .collect();
        chain.push(format!("{} › {:?}", display_path(&node.0), node.1));
        return Err(format!("cycle in includes: {}", chain.join(" → ")));
    }
    let expanded = crate::include::build_scenario(scenario, with_table)?;
    let included_base = resolved
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let background: Vec<crate::feature::ExpandedStep> = included
        .feature
        .background
        .as_ref()
        .map(|bg| bg.steps.iter().map(crate::feature::to_step).collect())
        .unwrap_or_default();

    stack.push(node);
    for step in background.iter().chain(expanded.steps.iter()) {
        let result = check_include_body_step(step, &included_base, reg, stack);
        if let Err(e) = result {
            stack.pop();
            return Err(format!(
                "{}:{}: {e}",
                crate::feature::display_path(&resolved),
                step.line
            ));
        }
    }
    stack.pop();
    Ok(())
}

/// A literal `I attach the file` path must exist before the first request. A
/// path holding a `<<variable>>` can only be read once it is interpolated, so
/// the step itself reports it then. A macro body is not walked here, so a
/// literal path inside one is also caught only at the step.
fn check_attached_file(file: &str, base_dir: &Path) -> Result<(), String> {
    if file.contains("<<") {
        return Ok(());
    }
    let path = crate::include::join(file, base_dir);
    if path.is_file() {
        Ok(())
    } else {
        Err(format!(
            "I attach the file {file:?}: no such file ({})",
            display_path(&path)
        ))
    }
}

fn check_include_body_step(
    step: &crate::feature::ExpandedStep,
    base_dir: &Path,
    reg: &Registry,
    stack: &mut Vec<(PathBuf, String)>,
) -> Result<(), String> {
    match reg.find(&step.text) {
        Ok(Some((StepTarget::Macro(_), _))) if step.docstring.is_some() => {
            Err("macro calls do not support a docstring".into())
        }
        Ok(Some((StepTarget::Macro(_), _))) if step.table.is_some() => {
            Err("macro calls do not support a table".into())
        }
        Ok(Some((StepTarget::Builtin { id, .. }, caps)))
            if matches!(
                id,
                crate::steps::StepId::Include | crate::steps::StepId::IncludeScenario
            ) =>
        {
            if step.docstring.is_some() {
                return Err("I include does not support a docstring".into());
            }
            let scenario_name = if id == crate::steps::StepId::IncludeScenario {
                Some(caps[1].as_str())
            } else {
                None
            };
            check_include_recursive(
                &caps[0],
                scenario_name,
                base_dir,
                step.table.as_deref(),
                reg,
                stack,
            )
        }
        Ok(Some((
            StepTarget::Builtin {
                id: crate::steps::StepId::AttachFile,
                ..
            },
            caps,
        ))) => check_attached_file(&caps[0], base_dir),
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(format!("unknown step: {:?}", step.text)),
        Err(e) => Err(e),
    }
}

fn render_chain(stack: &[(PathBuf, String)]) -> String {
    stack
        .iter()
        .map(|(p, s)| format!("{} › {s:?}", display_path(p)))
        .collect::<Vec<_>>()
        .join(" → ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature::{LoadedFeature, parse_str};
    use crate::macros::MacroCatalog;

    fn loaded(src: &str) -> LoadedFeature {
        LoadedFeature {
            path: PathBuf::from("t.feature"),
            feature: parse_str(src).unwrap(),
        }
    }

    fn macro_registry(name: &str) -> Registry {
        let path = std::env::temp_dir().join(format!(
            "bddkit-validate-macro-{}-{name}.yaml",
            std::process::id()
        ));
        std::fs::write(
            &path,
            "- step: I do business\n  do: [the response code is 200]\n",
        )
        .unwrap();
        Registry::with_macros(MacroCatalog::load(&[path]).unwrap()).unwrap()
    }

    #[test]
    fn accepts_a_file_of_known_steps() {
        let lf = loaded(
            "\
Feature: f
  Background:
    Given the \"Accept\" request header is \"application/json\"
  Scenario: s
    When I request \"/ping\" using HTTP POST
    Then the response code is 200
",
        );
        let p = check(&[&lf], &Registry::new().unwrap(), &TagFilter::new(&[]));
        assert!(p.is_empty(), "{p:?}");
    }

    #[test]
    fn reports_unknown_step_with_file_and_line() {
        let lf = loaded("Feature: f\n  Scenario: s\n    When I refund the order\n");
        let p = check(&[&lf], &Registry::new().unwrap(), &TagFilter::new(&[]));
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].line, 3);
        assert!(
            p[0].message.contains("I refund the order"),
            "{}",
            p[0].message
        );
    }

    #[test]
    fn reports_every_problem_not_just_the_first() {
        let lf = loaded(
            "\
Feature: f
  Scenario: s
    When I refund the order
    Then I ship the order
",
        );
        let p = check(&[&lf], &Registry::new().unwrap(), &TagFilter::new(&[]));
        assert_eq!(p.len(), 2, "all problems must be reported at once");
    }

    #[test]
    fn checks_background_steps_too() {
        let lf = loaded(
            "Feature: f\n  Background:\n    Given I refund the order\n  Scenario: s\n    Then the response code is 200\n",
        );
        let p = check(&[&lf], &Registry::new().unwrap(), &TagFilter::new(&[]));
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].line, 3);
    }

    #[test]
    fn a_filtered_out_scenario_with_a_bad_step_does_not_fail_validation() {
        let lf = loaded(
            "\
Feature: f
  @smoke
  Scenario: selected
    Then the response code is 200
  @slow
  Scenario: filtered_out
    When I refund the order
",
        );

        let p = check(
            &[&lf],
            &Registry::new().unwrap(),
            &TagFilter::new(&["smoke".to_string()]),
        );

        assert!(
            p.is_empty(),
            "a typo in an unselected scenario must not fail the run: {p:?}"
        );
    }

    #[test]
    fn checks_expanded_outline_steps() {
        let lf = loaded(
            "\
Feature: f
  Scenario Outline: s
    When I request \"/ping\" using HTTP <method>
    Examples:
      | method |
      | POSTT  |
",
        );
        let p = check(&[&lf], &Registry::new().unwrap(), &TagFilter::new(&[]));
        assert_eq!(
            p.len(),
            1,
            "a typo in the method must surface before the run"
        );
    }

    #[test]
    fn steps_with_variables_validate_without_values() {
        // Substitution happens in arguments, so matching does not require values.
        let lf = loaded(
            "Feature: f\n  Scenario: s\n    When I request \"/users/<<userId>>\" using HTTP GET\n",
        );
        let p = check(&[&lf], &Registry::new().unwrap(), &TagFilter::new(&[]));
        assert!(p.is_empty(), "{p:?}");
    }

    #[test]
    fn an_unserved_group_is_reported_once_per_file_at_its_first_step() {
        let lf = loaded(
            "\
Feature: f
  Scenario: s
    When I request \"/ping\" using HTTP GET
    Then the response code is 200
  Scenario: t
    Then the response code is 200
",
        );
        let unserved = |target: &StepTarget| match target {
            StepTarget::Builtin { .. } => Some("api declares none".to_string()),
            _ => None,
        };

        let p = unserved_resources(
            &[&lf],
            &Registry::new().unwrap(),
            &TagFilter::new(&[]),
            unserved,
        );

        assert_eq!(p.len(), 1, "one finding per file and group: {p:?}");
        assert_eq!(p[0].line, 3);
        assert_eq!(p[0].message, "api declares none");
    }

    #[test]
    fn a_served_step_is_not_a_finding() {
        let lf = loaded(
            "Feature: f\n  Scenario: s\n    When I frobnicate\n    Then the response code is 200\n",
        );

        let p = unserved_resources(
            &[&lf],
            &Registry::new().unwrap(),
            &TagFilter::new(&[]),
            |_| None,
        );

        assert!(
            p.is_empty(),
            "an unknown step is `check`'s finding, not this one: {p:?}"
        );
    }

    #[test]
    fn macro_call_with_docstring_is_rejected_before_running() {
        let lf = loaded(
            "Feature: f\n  Scenario: s\n    When I do business\n      \"\"\"\n      x\n      \"\"\"\n",
        );

        let problems = check(&[&lf], &macro_registry("docstring"), &TagFilter::new(&[]));

        assert_eq!(problems.len(), 1);
        assert!(problems[0].message.contains("docstring"), "{problems:?}");
    }

    #[test]
    fn macro_call_with_table_is_rejected_before_running() {
        let lf = loaded(
            "Feature: f\n  Scenario: s\n    When I do business\n      | value |\n      | x     |\n",
        );

        let problems = check(&[&lf], &macro_registry("table"), &TagFilter::new(&[]));

        assert_eq!(problems.len(), 1);
        assert!(problems[0].message.contains("table"), "{problems:?}");
    }

    #[test]
    fn include_of_a_missing_file_is_a_problem() {
        let dir = std::env::temp_dir().join(format!(
            "bddkit-validate-missing-file-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("caller.feature");
        std::fs::write(
            &path,
            "Feature: f\n  Scenario: s\n    Given I include \"nope.feature\"\n",
        )
        .unwrap();
        let lf = crate::feature::load(&path).unwrap();
        let reg = crate::steps::Registry::new().unwrap();
        let filter = crate::feature::TagFilter::new(&[]);
        let problems = check(&[&lf], &reg, &filter);
        assert!(problems.iter().any(|p| p.message.contains("nope.feature")));
    }

    fn attach_problems(tag: &str, path_literal: &str, create: bool) -> Vec<Problem> {
        let dir = std::env::temp_dir().join(format!(
            "bddkit-validate-attach-{tag}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(dir.join("fixtures")).unwrap();
        if create {
            std::fs::write(dir.join("fixtures/id.png"), b"x").unwrap();
        }
        let path = dir.join("caller.feature");
        std::fs::write(
            &path,
            format!(
                "Feature: f\n  Scenario: s\n    Given I attach the file \"{path_literal}\" to the request as \"content\"\n"
            ),
        )
        .unwrap();
        let lf = crate::feature::load(&path).unwrap();
        let reg = crate::steps::Registry::new().unwrap();
        check(&[&lf], &reg, &crate::feature::TagFilter::new(&[]))
    }

    #[test]
    fn attaching_a_missing_literal_file_is_a_problem_naming_it() {
        let problems = attach_problems("missing", "fixtures/id.png", false);
        assert!(
            problems
                .iter()
                .any(|p| p.message.contains("fixtures/id.png")),
            "{problems:?}"
        );
    }

    #[test]
    fn attaching_an_existing_literal_file_is_no_problem() {
        assert!(attach_problems("present", "fixtures/id.png", true).is_empty());
    }

    #[test]
    fn attaching_a_file_whose_path_has_a_variable_is_left_to_the_step() {
        assert!(attach_problems("variable", "fixtures/<<name>>.png", false).is_empty());
    }

    #[test]
    fn a_missing_attached_file_inside_an_included_scenario_is_a_problem() {
        let dir = std::env::temp_dir().join(format!(
            "bddkit-validate-attach-include-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("target.feature"),
            "Feature: t\n  Scenario: only\n    Given I attach the file \"gone.png\" to the request as \"c\"\n",
        )
        .unwrap();
        let path = dir.join("caller.feature");
        std::fs::write(
            &path,
            "Feature: f\n  Scenario: s\n    Given I include \"target.feature\"\n",
        )
        .unwrap();
        let lf = crate::feature::load(&path).unwrap();
        let reg = crate::steps::Registry::new().unwrap();
        let problems = check(&[&lf], &reg, &crate::feature::TagFilter::new(&[]));
        assert!(
            problems.iter().any(|p| p.message.contains("gone.png")),
            "{problems:?}"
        );
    }

    #[test]
    fn include_with_a_runtime_token_path_is_a_problem() {
        let dir = std::env::temp_dir().join(format!(
            "bddkit-validate-runtime-token-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("caller.feature");
        std::fs::write(
            &path,
            "Feature: f\n  Scenario: s\n    Given I include \"<<name>>.feature\"\n",
        )
        .unwrap();
        let lf = crate::feature::load(&path).unwrap();
        let reg = crate::steps::Registry::new().unwrap();
        let filter = crate::feature::TagFilter::new(&[]);
        let problems = check(&[&lf], &reg, &filter);
        assert!(problems.iter().any(|p| p.message.contains("literal")));
    }

    #[test]
    fn a_two_file_include_cycle_is_a_problem() {
        let dir =
            std::env::temp_dir().join(format!("bddkit-validate-cycle-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("a.feature"),
            "Feature: a\n  Scenario: sa\n    Given I include \"b.feature\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("b.feature"),
            "Feature: b\n  Scenario: sb\n    Given I include \"a.feature\"\n",
        )
        .unwrap();
        let path = dir.join("a.feature");
        let lf = crate::feature::load(&path).unwrap();
        let reg = crate::steps::Registry::new().unwrap();
        let filter = crate::feature::TagFilter::new(&[]);
        let problems = check(&[&lf], &reg, &filter);
        assert!(
            problems.iter().any(|p| p.message.contains("cycle")),
            "{problems:?}"
        );
    }

    #[test]
    fn an_unknown_step_inside_an_included_scenario_is_a_problem_naming_the_included_file() {
        let dir = std::env::temp_dir().join(format!(
            "bddkit-validate-unknown-step-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("target.feature"),
            "Feature: t\n  Scenario: only\n    Given this step does not exist\n",
        )
        .unwrap();
        let path = dir.join("caller.feature");
        std::fs::write(
            &path,
            "Feature: f\n  Scenario: s\n    Given I include \"target.feature\"\n",
        )
        .unwrap();
        let lf = crate::feature::load(&path).unwrap();
        let reg = crate::steps::Registry::new().unwrap();
        let filter = crate::feature::TagFilter::new(&[]);
        let problems = check(&[&lf], &reg, &filter);
        assert!(
            problems.iter().any(|p| p.message.contains("unknown step")
                && p.message.contains("this step does not exist")
                && p.message.contains("target.feature")),
            "{problems:?}"
        );
    }

    #[test]
    fn include_of_a_valid_one_scenario_file_has_no_problem() {
        let dir = std::env::temp_dir().join(format!(
            "bddkit-validate-valid-include-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("target.feature"),
            "Feature: f\n  Scenario: only\n    Given I am in debug mode\n",
        )
        .unwrap();
        let path = dir.join("caller.feature");
        std::fs::write(
            &path,
            "Feature: f\n  Scenario: s\n    Given I include \"target.feature\"\n",
        )
        .unwrap();
        let lf = crate::feature::load(&path).unwrap();
        let reg = crate::steps::Registry::new().unwrap();
        let filter = crate::feature::TagFilter::new(&[]);
        let problems = check(&[&lf], &reg, &filter);
        assert!(problems.is_empty(), "{problems:?}");
    }

    #[test]
    fn a_nested_include_with_a_docstring_is_rejected_at_validation_time() {
        let dir = std::env::temp_dir().join(format!(
            "bddkit-validate-nested-docstring-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("inner.feature"),
            "Feature: f\n  Scenario: only\n    Given I am in debug mode\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("middle.feature"),
            "Feature: f\n  Scenario: only\n    Given I include \"inner.feature\"\n      \"\"\"\n      not allowed\n      \"\"\"\n",
        )
        .unwrap();
        let path = dir.join("caller.feature");
        std::fs::write(
            &path,
            "Feature: f\n  Scenario: s\n    Given I include \"middle.feature\"\n",
        )
        .unwrap();
        let lf = crate::feature::load(&path).unwrap();
        let reg = crate::steps::Registry::new().unwrap();
        let filter = crate::feature::TagFilter::new(&[]);
        let problems = check(&[&lf], &reg, &filter);
        assert!(
            problems
                .iter()
                .any(|p| p.message.contains("does not support a docstring")),
            "{problems:?}"
        );
    }

    #[test]
    fn a_malformed_exports_tag_on_an_included_scenario_is_caught_at_validation_time() {
        let dir = std::env::temp_dir().join(format!(
            "bddkit-validate-malformed-exports-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("target.feature"),
            "Feature: f\n  @exports(a,,b)\n  Scenario: only\n    Given I am in debug mode\n",
        )
        .unwrap();
        let path = dir.join("caller.feature");
        std::fs::write(
            &path,
            "Feature: f\n  Scenario: s\n    Given I include \"target.feature\"\n",
        )
        .unwrap();
        let lf = crate::feature::load(&path).unwrap();
        let reg = crate::steps::Registry::new().unwrap();
        let filter = crate::feature::TagFilter::new(&[]);
        let problems = check(&[&lf], &reg, &filter);
        assert!(
            !problems.is_empty(),
            "expected a validation problem, got none"
        );
    }
}
