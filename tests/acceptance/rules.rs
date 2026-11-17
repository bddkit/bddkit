use super::run_bddkit_in;
use std::process::Command;

/// Writes `files` (path relative to the project, content) plus the usual
/// stub-less `cfg.yaml`; `features/` is the only suite path, so a file under
/// `lib/` is reachable by `I include` and never run on its own.
fn write_rule_project(slug: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("bddkit-{slug}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (path, content) in files {
        let dest = dir.join(path);
        std::fs::create_dir_all(dest.parent().expect("has a parent")).expect("mkdir");
        std::fs::write(dest, content).expect("write feature");
    }
    std::fs::write(
        dir.join("cfg.yaml"),
        "paths: [features]\nresources:\n  api: {}\n",
    )
    .expect("write config");
    dir
}

const RULE_REPRO: &str = "Feature: rules
  Scenario: outside
    Given set variable \"x\" to \"1\"

  Rule: a rule
    Scenario: inside fails on purpose
      Given set variable \"y\" to \"1\"
      Then variable \"y\" should be equal to \"2\"
";

#[test]
fn a_scenario_inside_a_rule_runs() {
    let dir = write_rule_project("rule-runs", &[("features/r.feature", RULE_REPRO)]);
    let (code, stdout, _) = run_bddkit_in("run", &dir);
    assert_eq!(code, Some(1), "{stdout}");
    assert!(stdout.contains("scenarios: 2"), "{stdout}");
    assert!(stdout.contains("failed: 1"), "{stdout}");
}

#[test]
fn a_file_with_only_rules_is_selected() {
    let dir = write_rule_project(
        "rule-only",
        &[(
            "features/r.feature",
            "Feature: f\n  Rule: r\n    Scenario: s\n      Given set variable \"x\" to \"1\"\n",
        )],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(code, Some(0), "{stdout}\n{stderr}");
    assert!(stdout.contains("scenarios: 1"), "{stdout}");
}

#[test]
fn a_rule_background_runs_after_the_feature_background() {
    // The rule Background reads what the feature Background set, so it
    // passes only if the feature's ran first; the outside scenario must not
    // run the rule's.
    let dir = write_rule_project(
        "rule-background",
        &[(
            "features/r.feature",
            "Feature: f
  Background:
    Given set variable \"a\" to \"1\"

  Scenario: outside
    Then variable \"a\" should be equal to \"1\"

  Rule: r
    Background:
      Given set variable \"b\" to \"<<a>>2\"

    Scenario: inside
      Then variable \"b\" should be equal to \"12\"
",
        )],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(code, Some(0), "{stdout}\n{stderr}");
    assert!(stdout.contains("scenarios: 2"), "{stdout}");
}

#[test]
fn a_rule_tag_selects_its_scenarios_with_tag() {
    let dir = write_rule_project(
        "rule-tag",
        &[(
            "features/r.feature",
            "Feature: f
  Scenario: outside
    Then variable \"nope\" should be equal to \"1\"

  @picked
  Rule: r
    Scenario: inside
      Given set variable \"x\" to \"1\"
",
        )],
    );
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config", "cfg.yaml", "--tag", "picked"])
        .current_dir(&dir)
        .output()
        .expect("failed to run bddkit");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(stdout.contains("scenarios: 1"), "{stdout}");
}

#[test]
fn an_unknown_step_inside_a_rule_is_refused_before_the_run() {
    let dir = write_rule_project(
        "rule-unknown-step",
        &[(
            "features/r.feature",
            "Feature: f\n  Rule: r\n    Scenario: s\n      When I frobnicate\n",
        )],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(code, Some(2), "{stdout}\n{stderr}");
    assert!(stderr.contains("I frobnicate"), "{stderr}");

    let (code, stdout, _) = run_bddkit_in("doctor", &dir);
    assert_eq!(code, Some(1), "{stdout}");
    assert!(stdout.contains("r.feature:4"), "{stdout}");
}

#[test]
fn a_scenario_inside_a_rule_can_be_included_by_name() {
    let dir = write_rule_project(
        "rule-include-named",
        &[
            (
                "lib/lib.feature",
                "Feature: lib
  Background:
    Given set variable \"a\" to \"1\"

  Scenario: outside
    Given set variable \"wrong\" to \"1\"

  Rule: r
    Background:
      Given set variable \"b\" to \"<<a>>2\"

    @exports(b)
    Scenario: inside
      Then variable \"b\" should be equal to \"12\"
",
            ),
            (
                "features/c.feature",
                "Feature: c
  Scenario: s
    When I include \"../lib/lib.feature\" scenario \"inside\"
    Then variable \"b\" should be equal to \"12\"
",
            ),
        ],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(code, Some(0), "{stdout}\n{stderr}");
}

#[test]
fn rule_scenarios_count_toward_an_unnamed_include() {
    let dir = write_rule_project(
        "rule-include-unnamed",
        &[
            (
                "lib/lib.feature",
                "Feature: lib
  Scenario: outside
    Given set variable \"a\" to \"1\"

  Rule: r
    Scenario: inside
      Given set variable \"b\" to \"1\"
",
            ),
            (
                "features/c.feature",
                "Feature: c\n  Scenario: s\n    When I include \"../lib/lib.feature\"\n",
            ),
        ],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(code, Some(2), "{stdout}\n{stderr}");
    assert!(stderr.contains("exactly one scenario"), "{stderr}");
    assert!(stderr.contains("outside, inside"), "{stderr}");
}

#[test]
fn a_serial_tag_on_a_rule_schedules_the_file() {
    let dir = write_rule_project(
        "rule-serial",
        &[
            (
                "features/a.feature",
                "Feature: a\n  @serial(x)\n  Rule: r\n    Scenario: s\n      Given set variable \"v\" to \"1\"\n",
            ),
            (
                "features/b.feature",
                "Feature: b\n  @serial(y)\n  Rule: r\n    Scenario: s\n      Given set variable \"v\" to \"1\"\n",
            ),
            (
                "features/c.feature",
                "@serial(x)\nFeature: c\n  @serial(y)\n  Rule: r\n    Scenario: s\n      Given set variable \"v\" to \"1\"\n",
            ),
        ],
    );
    // c.feature carries @serial(x) on the feature and @serial(y) on its Rule:
    // line, so it is in two chains, which is a startup error.
    let (code, _, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains("two chains"), "{stderr}");
}

#[test]
fn an_included_rule_scenario_runs_its_rule_background() {
    // The rule Background sets `b`, which the scenario exports; the caller
    // sees it only if the include ran that Background.
    let dir = write_rule_project(
        "rule-include-background",
        &[
            (
                "lib/lib.feature",
                "Feature: lib
  Rule: r
    Background:
      Given set variable \"b\" to \"set-by-rule\"

    @exports(b)
    Scenario: inside
      Given set variable \"x\" to \"1\"
",
            ),
            (
                "features/c.feature",
                "Feature: c
  Scenario: s
    When I include \"../lib/lib.feature\"
    Then variable \"b\" should be equal to \"set-by-rule\"
",
            ),
        ],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(code, Some(0), "{stdout}\n{stderr}");
}

#[test]
fn a_rule_background_is_not_checked_when_no_scenario_of_the_rule_is_selected() {
    let dir = write_rule_project(
        "rule-background-filtered",
        &[(
            "features/r.feature",
            "Feature: f
  @picked
  Scenario: outside
    Given set variable \"x\" to \"1\"

  Rule: unselected
    Background:
      Given I frobnicate

    Scenario: inside
      Given set variable \"y\" to \"1\"
",
        )],
    );
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config", "cfg.yaml", "--tag", "picked"])
        .current_dir(&dir)
        .output()
        .expect("failed to run bddkit");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(stdout.contains("scenarios: 1"), "{stdout}");
}

#[test]
fn a_malformed_priority_on_a_rule_without_scenarios_fails_the_startup() {
    let dir = write_rule_project(
        "rule-empty-priority",
        &[(
            "features/r.feature",
            "Feature: f
  Scenario: outside
    Given set variable \"x\" to \"1\"

  @priority(high)
  Rule: empty
",
        )],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(code, Some(2), "{stdout}\n{stderr}");
    assert!(stderr.contains("@priority(high)"), "{stderr}");
}

#[test]
fn a_scenario_outline_inside_a_rule_runs_every_example_row() {
    let dir = write_rule_project(
        "rule-outline",
        &[(
            "features/r.feature",
            "Feature: f
  Rule: r
    Scenario Outline: sets <v>
      Given set variable \"v\" to \"<v>\"
      Then variable \"v\" should be equal to \"<v>\"

      Examples:
        | v |
        | a |
        | b |
",
        )],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(code, Some(0), "{stdout}\n{stderr}");
    assert!(stdout.contains("scenarios: 2"), "{stdout}");
}
