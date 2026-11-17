use super::{of_type, read_stream};
use crate::common;
use std::process::Command;

/// Gate 1: the core's scenarios must be green. No API, no database.
#[test]
fn all_feature_files_pass() {
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config", "tests/acceptance.yaml"])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "run must be green\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(stdout.contains("failed: 0"), "{stdout}");
}

/// The JSON assertions on a variable, over a document a plugin published
/// (a core step cannot set a variable to text holding quotes).
#[test]
fn json_assertions_work_over_a_variable_a_plugin_published() {
    let dir = std::env::temp_dir().join(format!("bddkit-var-json-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::create_dir_all(dir.join("macros")).expect("mkdir");
    std::fs::create_dir_all(dir.join(".bddkit")).expect("mkdir .bddkit");
    std::fs::write(
        dir.join(".bddkit/plugins.yaml"),
        format!(
            "plugin:\n  - name: echo\n    path: {}\n",
            common::build_fixture_plugin().display()
        ),
    )
    .expect("write lock");
    std::fs::copy(
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/macros/vars.yaml"),
        dir.join("macros/vars.yaml"),
    )
    .expect("copy macros");
    std::fs::write(
        dir.join("features/json.feature"),
        r#"Feature: JSON over a variable
  Scenario: JSON assertions over a variable
    Given I echo the plain doc string:
      """
      {"user": {"id": 7, "roles": ["a", "b"]}}
      """
    Then variable "echo_doc" should contain JSON:
      """
      {"user": {"id": "@variableType(int)", "roles": ["b"]}}
      """
    And variable "echo_doc" should equal JSON:
      """
      {"user": {"id": 7, "roles": ["a", "b"]}}
      """
    And variable "echo_doc" should not contain JSON:
      """
      {"user": {"id": 8}}
      """
    And extract "user.roles[1]" from variable "echo_doc" as JSON as "role"
    And variable "role" should be equal to "b"
    And I pull the user id out of variable "echo_doc"
    And variable "uid" should be equal to "7"
    And variable "digits" should be equal to "7"
"#,
    )
    .expect("write feature");
    let abs = |name: &str| dir.join(name).display().to_string().replace('\\', "/");
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "concurrency: 1\npaths: [{}]\nmacro_paths: [{}]\nresources:\n  api: {{}}\n",
            abs("features"),
            abs("macros")
        ),
    )
    .expect("write config");
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config", "cfg.yaml", "--bddkit-dir"])
        .arg(dir.join(".bddkit"))
        .current_dir(&dir)
        .output()
        .expect("failed to run bddkit");
    assert!(
        out.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// An unknown step must fail BEFORE the first request, with code 2.
#[test]
fn unknown_step_fails_before_running() {
    let dir = std::env::temp_dir().join("bddkit-validate-test");
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/bad.feature"),
        "Feature: f\n  Scenario: s\n    When I refund the order\n",
    )
    .expect("write feature");
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "paths: [{}]\nresources:\n  api: {{}}\n",
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("write config");

    let exe = env!("CARGO_BIN_EXE_bddkit");
    let out = Command::new(exe)
        .args([
            "run",
            "--config",
            dir.join("cfg.yaml").to_str().expect("path is UTF-8"),
        ])
        .output()
        .expect("failed to run bddkit");

    assert_eq!(out.status.code(), Some(2), "static-check exit code");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("run not started"), "{stderr}");
    assert!(stderr.contains("I refund the order"), "{stderr}");
}

/// A `<<variable>>` in a typed position is checked once its value exists:
/// after interpolation, before dispatch. The wrong type fails the STEP (exit 1),
/// never the validation — nothing about the text was wrong.
#[test]
fn a_variable_of_the_wrong_type_fails_the_step() {
    let dir = std::env::temp_dir().join(format!("bddkit-typed-var-test-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/typed.feature"),
        "Feature: f\n  Scenario: s\n    Given set variable \"code\" to \"abc\"\n    When I sleep <<code>> seconds\n",
    )
    .expect("write feature");
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "paths: [{}]\nresources:\n  api: {{}}\n",
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("write config");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "run",
            "--config",
            dir.join("cfg.yaml").to_str().expect("path is UTF-8"),
        ])
        .output()
        .expect("failed to run bddkit");

    assert_eq!(
        out.status.code(),
        Some(1),
        "a failed step, not a failed validation"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(r#"parameter <seconds> expects uint, got "abc""#),
        "{stdout}"
    );
}

/// A typed parameter takes its quotes or leaves them, and a malformed slot in
/// a typed position is refused before the first request (exit 2), not mid-run.
#[test]
fn a_number_may_be_written_with_or_without_quotes_and_a_bad_slot_is_refused_up_front() {
    let dir = std::env::temp_dir().join(format!("bddkit-bare-typed-test-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "paths: [{}]\nresources:\n  api: {{}}\n",
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("write config");
    let run = |steps: &str| {
        std::fs::write(
            dir.join("features/f.feature"),
            format!("Feature: f\n  Scenario: s\n{steps}"),
        )
        .expect("write feature");
        Command::new(env!("CARGO_BIN_EXE_bddkit"))
            .args([
                "run",
                "--config",
                dir.join("cfg.yaml").to_str().expect("UTF-8"),
            ])
            .env_remove("CI")
            .output()
            .expect("failed to run bddkit")
    };
    let ok = run("    Given I sleep 0 seconds\n    And I sleep \"0\" seconds\n");
    assert_eq!(
        ok.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&ok.stdout)
    );
    let bad = run("    Given I sleep <<user-id>> seconds\n");
    assert_eq!(
        bad.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&bad.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The same check guards a macro's typed parameter: the value is refused
/// before the macro body runs, and it fails the calling step (exit 1).
#[test]
fn a_variable_of_the_wrong_type_fails_a_typed_macro_parameter() {
    let dir = std::env::temp_dir().join(format!("bddkit-typed-macro-test-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/typed.feature"),
        "Feature: f\n  Scenario: s\n    Given set variable \"code\" to \"abc\"\n    Then the \"/ping\" page answers <<code>>\n",
    )
    .expect("write feature");
    std::fs::write(
        dir.join("macros.yaml"),
        "- step: 'the \"{path:text}\" page answers {code:uint}'\n  do:\n    - I sleep <<code>> seconds\n",
    )
    .expect("write macros");
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "macro_paths: [{}]\npaths: [{}]\nresources:\n  api: {{}}\n",
            dir.join("macros.yaml")
                .display()
                .to_string()
                .replace('\\', "/"),
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("write config");

    let events = dir.join("events.ndjson");
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "run",
            "--config",
            dir.join("cfg.yaml").to_str().expect("path is UTF-8"),
            "--events",
            events.to_str().expect("path is UTF-8"),
        ])
        .output()
        .expect("failed to run bddkit");

    assert_eq!(
        out.status.code(),
        Some(1),
        "a failed step, not a failed validation"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(r#"parameter <code> expects uint, got "abc""#),
        "{stdout}"
    );
    // Refused before the body: the calling step started and failed, and no
    // step of the macro's body ever started.
    let stream = read_stream(&events);
    assert!(
        of_type(&stream, "step_started").iter().any(|e| e["text"]
            .as_str()
            .is_some_and(|t| t.contains("page answers"))),
        "{stream:?}"
    );
    assert_eq!(
        of_type(&stream, "step_finished")
            .iter()
            .filter(|e| e["status"] == "failed")
            .count(),
        1
    );
    assert!(
        of_type(&stream, "step_started")
            .iter()
            .all(|e| !e["text"].as_str().is_some_and(|t| t.starts_with("I sleep"))),
        "{stream:?}"
    );
}

/// `resources.api` may be absent entirely — legal for a scenario that makes
/// no HTTP requests (symmetric to `resources.db`).
#[test]
fn a_config_without_any_api_resource_runs_a_non_http_scenario() {
    let dir = std::env::temp_dir().join("bddkit-no-api-ok-test");
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/vars.feature"),
        "Feature: f\n  Scenario: s\n    \
         Given set variable \"x\" to \"1\"\n    Then variable \"x\" should be equal to \"1\"\n",
    )
    .expect("write feature");
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "paths: [{}]\nresources:\n  api: {{}}\n",
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("write config");

    let exe = env!("CARGO_BIN_EXE_bddkit");
    let out = Command::new(exe)
        .args([
            "run",
            "--config",
            dir.join("cfg.yaml").to_str().expect("path is UTF-8"),
        ])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "a config with no API resource must still run if the scenario does not touch HTTP\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
}

/// Shared helper: writes two tagged feature files and a config pointing at
/// the stub into a temp directory, returns the config path.
fn write_tagged_project(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("bddkit-{name}-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/smoke.feature"),
        "Feature: smoke\n  @smoke\n  Scenario: ping\n    When set variable \"ping\" to \"ok\"\n    Then variable \"ping\" should be equal to \"ok\"\n",
    )
    .expect("write smoke feature");
    std::fs::write(
        dir.join("features/slow.feature"),
        "Feature: slow\n  @slow\n  Scenario: ping slowly\n    When set variable \"ping\" to \"ok\"\n    Then variable \"ping\" should be equal to \"ok\"\n",
    )
    .expect("write slow feature");
    let cfg = dir.join("cfg.yaml");
    std::fs::write(
        &cfg,
        format!(
            "paths: [{}]\nresources:\n  api: {{}}\n",
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("write config");
    cfg
}

#[test]
fn tag_filter_runs_only_the_matching_scenarios() {
    let cfg = write_tagged_project("tagfilter");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "run",
            "--config",
            cfg.to_str().expect("path is UTF-8"),
            "--tag",
            "smoke",
        ])
        .output()
        .expect("failed to run bddkit");

    // Scenario names print only on failure, so the selection is visible via
    // file names and the final counters.
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("smoke.feature"),
        "the selected scenario must run:\n{stdout}"
    );
    assert!(
        !stdout.contains("slow.feature"),
        "a scenario without the tag must not run:\n{stdout}"
    );
    assert!(
        stdout.contains("files: 1, scenarios: 1, failed: 0"),
        "exactly one scenario must pass:\n{stdout}"
    );
}

#[test]
fn a_tag_matching_nothing_fails_with_exit_code_two() {
    let cfg = write_tagged_project("tagempty");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "run",
            "--config",
            cfg.to_str().expect("path is UTF-8"),
            "--tag",
            "absent",
        ])
        .output()
        .expect("failed to run bddkit");

    assert_eq!(
        out.status.code(),
        Some(2),
        "an empty selection is not a green run"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("no scenario selected"), "{stderr}");
}

#[test]
fn a_positional_path_overrides_the_config_paths() {
    let cfg = write_tagged_project("positional");
    let only = cfg
        .parent()
        .expect("parent directory")
        .join("features/smoke.feature");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "run",
            "--config",
            cfg.to_str().expect("path is UTF-8"),
            only.to_str().expect("path is UTF-8"),
        ])
        .output()
        .expect("failed to run bddkit");

    // The counter is required: without it the assertion would pass even if
    // the positional path were completely ignored (the config's `paths` gives two files).
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("slow.feature"),
        "a file outside the given path must not run:\n{stdout}"
    );
    assert!(
        stdout.contains("files: 1, scenarios: 1, failed: 0"),
        "exactly one file from the given path must pass:\n{stdout}"
    );
}

#[test]
fn macro_cycle_fails_validation_with_exit_code_two() {
    let dir = std::env::temp_dir().join(format!("bddkit-cycle-test-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/cycle.feature"),
        "Feature: f\n  Scenario: s\n    When I do first\n",
    )
    .expect("write feature");
    std::fs::write(
        dir.join("macros.yaml"),
        "- step: I do first\n  do: [I do second]\n- step: I do second\n  do: [I do first]\n",
    )
    .expect("write macros");
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "macro_paths: [{}]\npaths: [{}]\nresources:\n  api: {{}}\n",
            dir.join("macros.yaml")
                .display()
                .to_string()
                .replace('\\', "/"),
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("write config");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "run",
            "--config",
            dir.join("cfg.yaml").to_str().expect("path is UTF-8"),
        ])
        .output()
        .expect("failed to run bddkit");

    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("cycle in macros"), "{stderr}");
    assert!(stderr.contains("run not started"), "{stderr}");
}

/// `(`, `)` and `/` in a macro template became Cucumber Expression syntax
/// (#66). A template written for the old literal meaning must fail LOUDLY:
/// the caller's step text can no longer match, so validation stops the run.
#[test]
fn a_macro_template_with_literal_parentheses_no_longer_matches_its_old_text() {
    let dir = std::env::temp_dir().join(format!("bddkit-macro-syntax-test-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/old.feature"),
        "Feature: f\n  Scenario: s\n    When I do (setup) things\n",
    )
    .expect("write feature");
    std::fs::write(
        dir.join("macros.yaml"),
        "- step: I do (setup) things\n  do: [Show all variables]\n",
    )
    .expect("write macros");
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "macro_paths: [{}]\npaths: [{}]\nresources:\n  api: {{}}\n",
            dir.join("macros.yaml")
                .display()
                .to_string()
                .replace('\\', "/"),
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("write config");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "run",
            "--config",
            dir.join("cfg.yaml").to_str().expect("path is UTF-8"),
        ])
        .output()
        .expect("failed to run bddkit");

    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("unknown step"), "{stderr}");
    assert!(stderr.contains("run not started"), "{stderr}");
}

/// `Print table:` fills `<<variables>>` in every cell, the header row too,
/// and aligns the columns on the values after that substitution.
#[test]
fn print_table_aligns_interpolated_cells_on_stderr() {
    let dir = std::env::temp_dir().join(format!("bddkit-print-table-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/table.feature"),
        "Feature: f\n  Scenario: s\n    Given set variable \"id\" to \"ord-7f3a91\"\n    And set variable \"col\" to \"статус\"\n    Then Print table:\n      | order | <<col>> |\n      | <<id>> | оплачен |\n",
    )
    .expect("write feature");
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "paths: [{}]\nresources:\n  api: {{}}\n",
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("write config");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "run",
            "--config",
            dir.join("cfg.yaml").to_str().expect("path is UTF-8"),
        ])
        .output()
        .expect("failed to run bddkit");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(
        stderr.contains("| order      | статус  |\n| ord-7f3a91 | оплачен |\n"),
        "{stderr}"
    );
}
