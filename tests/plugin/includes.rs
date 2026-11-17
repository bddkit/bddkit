use super::{ECHO_GROUP, INCLUDE, project};
use std::process::Command;

/// `doctor` over a caller and the `helper.feature` it includes.
fn doctor_on_include(name: &str, caller: &str, helper: &str) -> String {
    let dir = project(
        name,
        &format!("Feature: only\n  Scenario: one\n{caller}"),
        ECHO_GROUP,
    );
    std::fs::write(
        dir.join("features/helper.feature"),
        format!("Feature: helper\n{helper}"),
    )
    .expect("write helper");
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config", "cfg.yaml"])
        .current_dir(&dir)
        .output()
        .expect("run bddkit");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert_eq!(out.status.code(), Some(0), "both are hints: {stdout}");
    stdout
}

const SENDS: &str = "  Scenario: make\n    Given I send the echo\n";

#[test]
fn an_extract_shaped_step_after_an_include_gets_the_hint_naming_the_state() {
    let out = doctor_on_include(
        "effects-hint",
        &format!("{INCLUDE}    And I keep the echo as \"team_id\"\n"),
        SENDS,
    );
    assert!(
        out.contains(
            "plugin.feature:4: extract \"team_id\" reads the exchange the include left behind; export the value from the helper instead"
        ),
        "{out}"
    );
    // A pattern step fills a placeholder from a named capture group.
    let out = doctor_on_include(
        "effects-hint-pattern",
        &format!("{INCLUDE}    And I keep the echo slot \"a\" as \"team_id\"\n"),
        SENDS,
    );
    assert!(
        out.contains("extract \"team_id\" reads the slot:a the include left behind"),
        "{out}"
    );
}

#[test]
fn the_hint_stays_quiet_about_an_export_a_reader_and_an_unknown_name() {
    for (name, next) in [
        (
            "effects-quiet-read",
            "Then the echo exchange should be kept",
        ),
        ("effects-quiet-slot", "And I keep the echo as \"<<team>>\""),
        (
            "effects-quiet-state",
            "And I keep the echo slot \"<<s>>\" as \"team_id\"",
        ),
    ] {
        let out = doctor_on_include(name, &format!("{INCLUDE}    {next}\n"), SENDS);
        assert!(!out.contains("left behind"), "{name}: {out}");
    }
    let out = doctor_on_include(
        "effects-quiet-export",
        "    Given I include \"helper.feature\" with prefix \"a\"\n    And I keep the echo as \"a_team_id\"\n",
        "  @exports(team_id)\n  Scenario: make\n    Given I keep the echo as \"team_id\"\n",
    );
    assert!(!out.contains("left behind"), "{out}");
    assert!(!out.contains("sets it"), "{out}");
}

#[test]
fn a_family_gets_no_hint_of_its_own_and_its_fixed_name_does() {
    let out = doctor_on_include(
        "effects-family-hint",
        &format!("{INCLUDE}    And I spread the echo over \"fam\"\n"),
        SENDS,
    );
    assert!(out.contains("extract \"echo_spread\" reads"), "{out}");
    assert!(!out.contains("fam_"), "{out}");
}

#[test]
fn the_writer_list_takes_sets_vars_and_reports_what_no_step_writes() {
    let out = doctor_on_include(
        "effects-writers",
        INCLUDE,
        "  @exports(team_id,nobody)\n  Scenario: make\n    Given I send the echo\n    And I keep the echo as \"team_id\"\n",
    );
    assert!(out.contains("@exports names \"nobody\""), "{out}");
    assert!(!out.contains("\"team_id\", but"), "{out}");
}

#[test]
fn a_family_matches_an_export_by_prefix() {
    let out = doctor_on_include(
        "effects-family",
        INCLUDE,
        "  @exports(fam_one,echo_spread,other)\n  Scenario: make\n    Given I spread the echo over \"fam\"\n",
    );
    assert!(out.contains("@exports names \"other\""), "{out}");
    assert!(!out.contains("\"fam_one\", but"), "{out}");
    assert!(!out.contains("\"echo_spread\", but"), "{out}");
}

#[test]
fn a_step_with_no_effects_or_an_unknown_name_makes_the_writer_list_give_up() {
    for (name, helper) in [
        (
            "effects-giveup-none",
            "  @exports(nobody)\n  Scenario: make\n    Given I echo \"x\" as \"y\"\n",
        ),
        (
            "effects-giveup-slot",
            "  @exports(nobody)\n  Scenario: make\n    Given I keep the echo as \"<<n>>\"\n",
        ),
        (
            "effects-giveup-column",
            "  @exports(nobody)\n  Scenario Outline: make\n    Given I keep the echo as \"<k>\"\n    Examples:\n      | k |\n      | a |\n",
        ),
    ] {
        let out = doctor_on_include(name, INCLUDE, helper);
        assert!(!out.contains("sets it"), "{name}: {out}");
    }
}
