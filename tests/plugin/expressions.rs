use super::{ECHO_GROUP, assert_green, macro_project, project, run, run_with};
use std::path::PathBuf;
use std::process::Command;

/// A file of `feature` run through the echo fixture, for the `expression` steps.
fn expression_project(name: &str, feature: &str) -> PathBuf {
    project(name, &format!("Feature: f\n{feature}"), ECHO_GROUP)
}

#[test]
fn an_expression_step_gets_one_arg_per_parameter_with_or_without_its_optional_quotes() {
    let dir = expression_project(
        "expression-args",
        "  Scenario: s\n    When I echo the number 5\n    Then variable \"echo_args\" should be equal to \"5\"\n    When I echo the number \"6\"\n    Then variable \"echo_args\" should be equal to \"6\"\n",
    );
    assert_green(&run(&dir));
}

#[test]
fn validate_step_gets_one_arg_per_parameter_with_or_without_its_optional_quotes() {
    for (name, step) in [
        ("expression-validate-bare", "I echo the number 13"),
        ("expression-validate-quoted", "I echo the number \"13\""),
    ] {
        let dir = expression_project(name, &format!("  Scenario: s\n    When {step}\n"));
        let out = run(&dir);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(2), "{stderr}");
        assert!(stderr.contains("the echo refuses 13"), "{stderr}");
    }
}

#[test]
fn a_typed_parameter_holding_a_wrong_literal_is_exit_2_naming_the_parameter() {
    let dir = expression_project(
        "expression-wrong-literal",
        "  Scenario: s\n    When I echo the number abc\n",
    );
    let out = run(&dir);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains(r#"parameter <n> expects uint, got "abc""#),
        "{stderr}"
    );
}

#[test]
fn a_slot_holding_a_wrong_value_fails_the_step_as_a_builtins_would() {
    let dir = expression_project(
        "expression-wrong-slot",
        "  Scenario: s\n    Given set variable \"n\" to \"abc\"\n    When I echo the number <<n>>\n",
    );
    let out = run(&dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(
        stdout.contains(r#"parameter <n> expects uint, got "abc""#),
        "{stdout}"
    );
    assert!(stdout.contains("failed: 1 (1 input)"), "{stdout}");
}

#[test]
fn a_literal_secret_parameter_is_hidden_everywhere_and_joins_the_secret_set() {
    let dir = expression_project(
        "expression-secret",
        "  Scenario: a\n    Then the echo should fail with key \"key-literal-1\"\n\n  Scenario: b\n    Then the echo should fail quoting \"key-literal-1\"\n",
    );
    let out = run_with(
        &dir,
        &["--events", "events.ndjson", "--junit", "junit.xml"],
        &[],
    );
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains(r#"with key "***""#), "{stdout}");
    assert!(stdout.contains("the echo saw ***"), "{stdout}");
    assert!(stdout.contains("the echo quoted ***"), "{stdout}");
    assert!(!stdout.contains("key-literal-1"), "{stdout}");
    for report in ["events.ndjson", "junit.xml"] {
        let text = std::fs::read_to_string(dir.join(report)).expect("report");
        assert!(!text.contains("key-literal-1"), "{report}: {text}");
    }
    let stream = std::fs::read_to_string(dir.join("events.ndjson")).expect("stream");
    assert!(stream.contains(r#"with key \"***\""#), "{stream}");
}

#[test]
fn steps_list_shows_an_expression_steps_template() {
    let dir = expression_project("expression-list", "");
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["steps", "list", "echo", "--config", "cfg.yaml"])
        .current_dir(&dir)
        .output()
        .expect("run bddkit");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(stdout.contains("I echo the number <n>"), "{stdout}");
    assert!(
        stdout.contains(r#"the echo should fail with key "<key>""#),
        "{stdout}"
    );
}

#[test]
fn a_macro_conflicting_with_an_expression_step_is_refused_at_startup() {
    let dir = macro_project(
        "expression-conflict",
        "Feature: f\n  Scenario: s\n    When I echo the number 5\n",
        "- step: I echo the number {x:uint}\n  do: [set variable \"a\" to \"b\"]\n",
    );
    let out = run(&dir);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("conflicts with plugin step \"I echo the number <n>\""),
        "{stderr}"
    );
}

#[test]
fn a_literal_secret_in_a_step_no_single_entry_matched_stays_hidden() {
    for (name, yaml, step) in [
        // A refused typed literal: the step matches nothing.
        (
            "secret-refused-literal",
            "- step: I ship {n:uint} with key {key:secret}\n  do: [set variable \"a\" to \"b\"]\n",
            "I ship abc with key \"real-secret-value\"",
        ),
        // A macro overlapping a plugin pattern: the step matches two.
        (
            "secret-ambiguous",
            "- step: I echo {user:text} with password {key:secret}\n  do: [set variable \"a\" to \"b\"]\n",
            "I echo \"ann\" with password \"real-secret-value\"",
        ),
    ] {
        let dir = macro_project(
            name,
            &format!("Feature: f\n  Scenario: s\n    When {step}\n"),
            yaml,
        );
        let out = run(&dir);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(2), "{name}: {stderr}");
        assert!(stderr.contains("***"), "{name}: {stderr}");
        assert!(!stderr.contains("real-secret-value"), "{name}: {stderr}");
    }
}
