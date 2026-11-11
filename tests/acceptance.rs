mod common;

use serde_json::{Value, json};
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

/// Prepares two feature files that write the same variable with different
/// values BEFORE the barrier and check it AFTER. The barrier guarantees that
/// both files have written before either reads: a run-wide `VarStack` would
/// fail this deterministically, not occasionally.
fn write_parallel_fixture(
    dir: &std::path::Path,
    concurrency: usize,
    tags: &str,
) -> std::path::PathBuf {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::create_dir_all(dir.join(".bddkit")).expect("mkdir .bddkit");
    std::fs::write(
        dir.join(".bddkit/plugins.yaml"),
        format!(
            "plugin:\n  - name: worker\n    path: {}\n",
            common::build_worker_plugin().display()
        ),
    )
    .expect("write lock");
    for who in ["a", "b"] {
        std::fs::write(
            dir.join(format!("features/{who}.feature")),
            format!(
                "{tags}Feature: parallel {who}\n  Scenario: both tasks meet at the barrier\n    \
                 Given set variable \"who\" to \"{who}\"\n    \
                 When I wait at the worker barrier for 2 parties\n    \
                 And variable \"who\" should be equal to \"{who}\"\n"
            ),
        )
        .expect("write feature");
    }
    let cfg = dir.join("cfg.yaml");
    std::fs::write(
        &cfg,
        format!(
            "concurrency: {concurrency}\npaths: [{}]\nresources:\n  api: {{}}\n",
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("write config");
    cfg
}

/// Runs the fixture from its own directory, where the plugin lock lives.
fn run_parallel_fixture(dir: &std::path::Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config", "cfg.yaml", "--bddkit-dir"])
        .arg(dir.join(".bddkit"))
        .current_dir(dir)
        .output()
        .expect("failed to run bddkit")
}

/// Gate M5: two files must run at the same time, or the barrier never opens.
#[test]
fn two_feature_files_run_at_the_same_time() {
    let dir = std::env::temp_dir().join(format!("bddkit-parallel-{}", std::process::id()));
    write_parallel_fixture(&dir, 2, "");
    let out = run_parallel_fixture(&dir);

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "both files must run in parallel\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(stdout.contains("failed: 0"), "{stdout}");
    assert!(stdout.contains("files: 2"), "{stdout}");
}

/// Negative control: with `concurrency: 1` the barrier never opens and the
/// run fails. Without this test, the first one could pass green for any
/// reason other than real parallelism.
#[test]
fn a_single_worker_cannot_release_the_barrier() {
    let dir = std::env::temp_dir().join(format!("bddkit-sequential-{}", std::process::id()));
    write_parallel_fixture(&dir, 1, "");
    let out = run_parallel_fixture(&dir);

    assert_eq!(
        out.status.code(),
        Some(1),
        "a single worker must hit the barrier timeout, not pass"
    );
}

/// Files in one chain must never be in flight at the same time — the
/// two-party barrier never opens, and the run must fail on timeout. This
/// mirrors the parallelism test: the same pair of files, just tagged.
#[test]
fn two_files_in_one_serial_chain_never_run_together() {
    let dir = std::env::temp_dir().join(format!("bddkit-serial-{}", std::process::id()));
    write_parallel_fixture(&dir, 2, "@serial(shared)\n");
    let out = run_parallel_fixture(&dir);

    assert_eq!(
        out.status.code(),
        Some(1),
        "the chain must serialize the files, the two-party barrier must not open\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// A broken scheduling tag — rejected before the first request, code 2.
#[test]
fn a_file_in_two_chains_fails_the_startup() {
    let dir = std::env::temp_dir().join(format!("bddkit-two-chains-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/bad.feature"),
        "@serial(a)\nFeature: f\n  @serial(b)\n  Scenario: s\n    Given set variable \"x\" to \"1\"\n",
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

    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("two chains"), "{stderr}");
}

/// The order files print in with one worker is the queue order.
/// Checking that `@priority` sets it: higher goes earlier.
#[test]
fn higher_priority_files_run_first() {
    let dir = std::env::temp_dir().join(format!("bddkit-priority-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    // Names are deliberately alphabetical in the reverse of the desired order:
    // without the tag the queue would be low → mid → high.
    for (name, tag) in [
        ("a_low", "@priority(-1)\n"),
        ("b_mid", ""),
        ("c_high", "@priority(5)\n"),
    ] {
        std::fs::write(
            dir.join(format!("features/{name}.feature")),
            format!("{tag}Feature: {name}\n  Scenario: s\n    Given set variable \"x\" to \"1\"\n"),
        )
        .expect("write feature");
    }
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "concurrency: 1\npaths: [{}]\nresources:\n  api: {{}}\n",
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

    let stdout = String::from_utf8_lossy(&out.stdout);
    let high = stdout.find("c_high").expect("file c_high in the output");
    let mid = stdout.find("b_mid").expect("file b_mid in the output");
    let low = stdout.find("a_low").expect("file a_low in the output");
    assert!(
        high < mid && mid < low,
        "queue order by priority:\n{stdout}"
    );
}

/// A non-numeric priority — rejected before the first request, code 2.
#[test]
fn a_non_numeric_priority_fails_the_startup() {
    let dir = std::env::temp_dir().join(format!("bddkit-bad-priority-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/bad.feature"),
        "@priority(urgent)\nFeature: f\n  Scenario: s\n    Given set variable \"x\" to \"1\"\n",
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

    assert_eq!(out.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("an integer"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `--fail-fast` stops dispatching NEW work after the first failure.
/// With one worker this is deterministic: the first file fails, the second
/// and third never start at all.
#[test]
fn fail_fast_stops_starting_new_files() {
    let dir = std::env::temp_dir().join(format!("bddkit-fail-fast-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/a_broken.feature"),
        "Feature: broken\n  Scenario: s\n    \
         Given set variable \"x\" to \"1\"\n    Then variable \"x\" should be equal to \"2\"\n",
    )
    .expect("write feature");
    for name in ["b_ok", "c_ok"] {
        std::fs::write(
            dir.join(format!("features/{name}.feature")),
            format!("Feature: {name}\n  Scenario: s\n    Given set variable \"x\" to \"1\"\n"),
        )
        .expect("write feature");
    }
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "concurrency: 1\npaths: [{}]\nresources:\n  api: {{}}\n",
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
            "--fail-fast",
        ])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(
        stdout.contains("a_broken"),
        "the failed file must appear in the report:\n{stdout}"
    );
    assert!(
        !stdout.contains("c_ok"),
        "no new files must start after the failure:\n{stdout}"
    );
    assert!(stdout.contains("files: 1"), "{stdout}");
}

/// `--fail-fast` also stops dispatching new work WITHIN a chain already
/// picked up: three files sharing one `@serial` name run strictly in order
/// on one worker, the first fails, the second and third never start. This is
/// a separate check from `fail_fast_stops_starting_new_files`, which only
/// exercises the "before the next chain" check — there each file was its own chain.
#[test]
fn fail_fast_stops_a_chain_partway_through() {
    let dir = std::env::temp_dir().join(format!("bddkit-fail-fast-chain-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/a_broken.feature"),
        "@serial(chain)\nFeature: broken\n  Scenario: s\n    \
         Given set variable \"x\" to \"1\"\n    Then variable \"x\" should be equal to \"2\"\n",
    )
    .expect("write feature");
    for name in ["b_ok", "c_ok"] {
        std::fs::write(
            dir.join(format!("features/{name}.feature")),
            format!(
                "@serial(chain)\nFeature: {name}\n  Scenario: s\n    Given set variable \"x\" to \"1\"\n"
            ),
        )
        .expect("write feature");
    }
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "concurrency: 1\npaths: [{}]\nresources:\n  api: {{}}\n",
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
            "--fail-fast",
        ])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(
        stdout.contains("a_broken"),
        "the failed file must appear in the report:\n{stdout}"
    );
    assert!(
        !stdout.contains("b_ok") && !stdout.contains("c_ok"),
        "the rest of the chain must not start after the failure:\n{stdout}"
    );
    assert!(stdout.contains("files: 1"), "{stdout}");
}

/// Without the flag, one file's failure does not block the rest: the run
/// must reach the end and show all failures at once.
#[test]
fn without_fail_fast_every_file_still_runs() {
    let dir = std::env::temp_dir().join(format!("bddkit-no-fail-fast-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/a_broken.feature"),
        "Feature: broken\n  Scenario: s\n    \
         Given set variable \"x\" to \"1\"\n    Then variable \"x\" should be equal to \"2\"\n",
    )
    .expect("write feature");
    std::fs::write(
        dir.join("features/c_ok.feature"),
        "Feature: whole\n  Scenario: s\n    Given set variable \"x\" to \"1\"\n",
    )
    .expect("write feature");
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "concurrency: 1\npaths: [{}]\nresources:\n  api: {{}}\n",
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

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("files: 2"), "{stdout}");
}

/// A project with a core-only feature and the given `resources:` body.
fn empty_group_project(name: &str, resources: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("bddkit-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/f.feature"),
        "Feature: f\n  Scenario: s\n    Given set variable \"a\" to \"1\"\n",
    )
    .expect("write feature");
    std::fs::write(
        dir.join("cfg.yaml"),
        format!("paths: [features]\nresources:\n{resources}"),
    )
    .expect("write config");
    // An empty bddkit dir: a developer's installed plugins never decide this.
    std::fs::create_dir_all(dir.join(".bddkit")).expect("mkdir");
    dir
}

fn run_empty_group_project(
    subcommand: &str,
    dir: &std::path::Path,
) -> (Option<i32>, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .current_dir(dir)
        .args([subcommand, "--config", "cfg.yaml", "--bddkit-dir"])
        .arg(dir.join(".bddkit"))
        .output()
        .expect("run bddkit");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// The databases are the `db` plugin's: a connection with no plugin installed
/// is a startup failure that says what to do, not a silent no-op.
#[test]
fn a_database_with_no_plugin_installed_exits_2_and_names_the_install() {
    let dir = empty_group_project(
        "db-no-plugin",
        "  db:\n    main:\n      dsn: postgres://u:p@127.0.0.1:1/x\n",
    );
    let (code, stdout, stderr) = run_empty_group_project("run", &dir);
    assert_eq!(code, Some(2), "{stdout}{stderr}");
    let all = format!("{stdout}{stderr}");
    assert!(
        all.contains("no installed plugin serves the group \"db\""),
        "{all}"
    );
    assert!(all.contains("bddkit plugin install"), "{all}");
    assert!(all.contains("run not started"), "{all}");
}

/// The HTTP API and SRP are the `api` plugin's, the same way.
#[test]
fn an_api_or_srp_with_no_plugin_installed_exits_2_and_names_the_install() {
    for (group, body) in [
        (
            "api",
            "  api:\n    main:\n      base_url: http://127.0.0.1:1\n",
        ),
        ("srp", "  srp:\n    main: {}\n"),
    ] {
        let dir = empty_group_project(&format!("{group}-no-plugin"), body);
        let (code, stdout, stderr) = run_empty_group_project("run", &dir);
        assert_eq!(code, Some(2), "{stdout}{stderr}");
        let all = format!("{stdout}{stderr}");
        assert!(
            all.contains(&format!("no installed plugin serves the group \"{group}\"")),
            "{all}"
        );
        assert!(all.contains("bddkit plugin install"), "{all}");
    }
}

/// A stray `default_db`, `default_api` or `default_srp` was an error while the
/// kinds were built in, and stays one with no plugin installed: `run` and
/// `doctor` both refuse it.
#[test]
fn a_stray_default_is_refused_with_no_plugin_installed() {
    for key in ["default_api", "default_db", "default_srp"] {
        let dir = empty_group_project("stray-default", &format!("  foo: {{}}\n{key}: main\n"));
        let (code, stdout, stderr) = run_empty_group_project("run", &dir);
        assert_eq!(code, Some(2), "{key}: {stdout}{stderr}");
        assert!(
            format!("{stdout}{stderr}").contains(key),
            "{stdout}{stderr}"
        );
        let (code, stdout, stderr) = run_empty_group_project("doctor", &dir);
        assert_eq!(code, Some(1), "{key}: {stdout}{stderr}");
        assert!(stdout.contains(key), "{stdout}{stderr}");
    }
}

#[test]
fn a_config_with_no_resources_api_runs() {
    let dir = empty_group_project("no-api-key", "  db: {}\n");
    let (code, stdout, stderr) = run_empty_group_project("run", &dir);
    assert_eq!(code, Some(0), "{stdout}{stderr}");
}

#[test]
fn an_empty_group_no_plugin_serves_runs_quietly_and_doctor_hints_at_it() {
    let dir = empty_group_project("empty-group", "  foo: {}\n");
    let (code, stdout, stderr) = run_empty_group_project("run", &dir);
    assert_eq!(code, Some(0), "{stdout}{stderr}");
    assert!(
        !stdout.contains("foo") && !stderr.contains("foo"),
        "{stdout}{stderr}"
    );

    let (code, stdout, stderr) = run_empty_group_project("doctor", &dir);
    assert_eq!(code, Some(0), "{stdout}{stderr}");
    assert!(
        stdout.contains("resources.foo is empty") && stdout.contains("can be removed"),
        "{stdout}"
    );
}

#[test]
fn a_group_with_an_instance_no_plugin_serves_still_exits_two() {
    let dir = empty_group_project("unserved-group", "  foo:\n    main:\n      x: 1\n");
    let (code, stdout, stderr) = run_empty_group_project("run", &dir);
    assert_eq!(code, Some(2), "{stdout}{stderr}");
    assert!(
        format!("{stdout}{stderr}").contains("no installed plugin serves the group"),
        "{stdout}{stderr}"
    );
}

/// `doctor` reaches every check a run makes before its first request, and a
/// bare invocation opens no socket at all.
#[test]
fn doctor_reports_a_healthy_suite_and_exits_zero() {
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config", "tests/acceptance.yaml"])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(0),
        "a sound suite is clean\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(stdout.contains("APP_ENV:"), "{stdout}");
    assert!(
        stdout.contains("--live"),
        "a static run must say what it did not do:\n{stdout}"
    );
}

/// Writes a project with an empty `api` section, returning the config path.
/// `feature` is the whole `.feature` file so a
/// caller can plant a typo or a tag; an empty one writes no feature file at
/// all. `extra` is appended after the API block — indented, it adds to
/// `resources:`; at column 0 it adds a top-level key.
fn write_doctor_project(name: &str, feature: &str, extra: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("bddkit-{name}-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    let only = dir.join("features/only.feature");
    if feature.is_empty() {
        // A previous run of this test binary may have left one behind.
        let _ = std::fs::remove_file(&only);
    } else {
        std::fs::write(&only, feature).expect("write feature");
    }
    let api = "  api: {}\n";
    let cfg = dir.join("cfg.yaml");
    std::fs::write(
        &cfg,
        format!(
            "paths: [{}]\nresources:\n{api}{extra}",
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
fn doctor_names_the_file_and_line_of_an_undefined_step() {
    let cfg = write_doctor_project(
        "doctor-step",
        "Feature: only\n  Scenario: one\n    When I frobnicate\n",
        "",
    );

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config", cfg.to_str().expect("path is UTF-8")])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("only.feature:3"), "{stdout}");
    assert!(stdout.contains("I frobnicate"), "{stdout}");
}

const THREE_BAD_STEPS: &str = "Feature: only\n  Scenario: one\n    When I frobnicate\n    Then I sleep a while\n    And I sleep abc seconds\n";

#[test]
fn doctor_reports_one_finding_per_bad_step() {
    let cfg = write_doctor_project("doctor-three", THREE_BAD_STEPS, "");
    let cfg = cfg.to_str().expect("path is UTF-8");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config", cfg])
        .output()
        .expect("failed to run bddkit");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    for line in [3, 4, 5] {
        assert!(stdout.contains(&format!("only.feature:{line}")), "{stdout}");
    }
    assert!(stdout.contains("3 problem(s)"), "{stdout}");
    assert!(
        stdout.contains("did you mean: I sleep <seconds> seconds"),
        "{stdout}"
    );
    assert!(
        stdout.contains(r#"parameter <seconds> expects uint, got "abc""#),
        "{stdout}"
    );

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--json", "--config", cfg])
        .output()
        .expect("failed to run bddkit");
    let report: Value = serde_json::from_slice(&out.stdout).expect("--json emits JSON only");
    let failed = report["checks"]
        .as_array()
        .expect("checks is an array")
        .iter()
        .filter(|c| c["stage"] == "steps" && c["status"] == "failed")
        .count();
    assert_eq!(failed, 3, "{report}");
}

#[test]
fn run_prints_the_suggestion_for_an_unknown_step() {
    let cfg = write_doctor_project("run-three", THREE_BAD_STEPS, "");
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config", cfg.to_str().expect("path is UTF-8")])
        .output()
        .expect("failed to run bddkit");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("did you mean: I sleep <seconds> seconds"),
        "{stderr}"
    );
}

/// A caller and a helper side by side; returns `doctor`'s exit code and stdout.
fn doctor_include_pair(name: &str, caller: &str, helper: &str) -> (Option<i32>, String) {
    let cfg = write_doctor_project(name, caller, "");
    let features = cfg.parent().expect("config has a parent").join("features");
    std::fs::write(features.join("helper.feature"), helper).expect("write helper");
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config", cfg.to_str().expect("path is UTF-8")])
        .output()
        .expect("failed to run bddkit");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

#[test]
fn doctor_hints_at_an_export_the_helper_never_sets() {
    let (code, stdout) = doctor_include_pair(
        "doctor-include-export",
        "Feature: only\n  Scenario: one\n    Given I include \"helper.feature\"\n",
        "Feature: helper\n  @exports(team_idd)\n  Scenario: make\n    Given set variable \"team_id\" to \"1\"\n",
    );
    assert_eq!(code, Some(0), "{stdout}");
    assert!(stdout.contains("helper.feature:3"), "{stdout}");
    assert!(stdout.contains("team_idd"), "{stdout}");
}

/// A scheduling tag `run` refuses to parse is a "nothing ran" failure like any
/// other, so `doctor` has to see it. It is the one pre-run check that lives
/// past `validate::check`, in `runner::build_chains`.
#[test]
fn doctor_reports_a_malformed_scheduling_tag() {
    let cfg = write_doctor_project(
        "doctor-tag",
        "Feature: only\n  @priority(soon)\n  Scenario: one\n    Given set variable \"ping\" to \"ok\"\n",
        "",
    );

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config", cfg.to_str().expect("path is UTF-8")])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("@priority(soon)"), "{stdout}");
}

/// `run` exits 2 on a selection with nothing in it. A green tick reading
/// "0 file(s), every step matched" is the most misleading line the command
/// could print, because it certifies a suite that cannot run.
#[test]
fn doctor_reports_a_suite_with_no_scenario_to_run() {
    let cfg = write_doctor_project("doctor-empty", "", "");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config", cfg.to_str().expect("path is UTF-8")])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("no scenario"), "{stdout}");
}

/// The decision a script depends on: `doctor` answers 0 or 1 and never 2, so
/// even a config it cannot parse comes back as an ordinary finding. `run`
/// exits 2 for the same file.
#[test]
fn doctor_reports_an_unparseable_config_as_an_ordinary_finding() {
    let dir = std::env::temp_dir().join(format!("bddkit-doctor-broken-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let cfg = dir.join("cfg.yaml");
    std::fs::write(
        &cfg,
        "paths: [features]\nresources:\n  api: {}\nmacro_paths: [${BDDKIT_ABSENT_VAR}]\n",
    )
    .expect("write config");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config", cfg.to_str().expect("path is UTF-8")])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(1),
        "a broken config is a finding, not a different exit currency\n{stdout}"
    );
    assert!(stdout.contains("BDDKIT_ABSENT_VAR"), "{stdout}");
}

/// The primary caller is a script or an agent, which is what `--json` is for.
#[test]
fn doctor_json_carries_the_env_the_status_and_every_check() {
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config", "tests/acceptance.yaml", "--json"])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let report: Value = serde_json::from_str(&stdout).expect("--json emits JSON only");
    assert_eq!(report["app_env"], "dev", "{stdout}");
    assert_eq!(report["live"], false, "{stdout}");
    assert_eq!(
        report["config_source"], "flag",
        "a structured field, not just the `(--config)` suffix on `config`: {stdout}"
    );
    let checks = report["checks"].as_array().expect("checks is an array");
    assert!(
        checks
            .iter()
            .any(|c| c["stage"] == "steps" && c["status"] == "ok"),
        "{stdout}"
    );
}

/// The flat `bddkit --config x.yaml` form is gone: `steps` has to be a real
/// subcommand, and clap cannot have both a positional path list at the top
/// level and subcommands to disambiguate it against.
#[test]
fn the_run_subcommand_is_how_a_suite_is_started() {
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["--config", "tests/acceptance.yaml"])
        .output()
        .expect("run bddkit");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "the flat form is a usage error");
    assert!(
        stderr.contains("unexpected argument"),
        "the flat form must be refused by the parser, not started:\n{stderr}"
    );

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--help"])
        .output()
        .expect("run bddkit");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains("Usage: bddkit run"), "{stdout}");
    assert!(
        stdout.contains("--fail-fast"),
        "run keeps every flag the flat form had:\n{stdout}"
    );
}

/// Writes a healthy doctor project (see `write_doctor_project`) under a fresh
/// directory whose name is the default config file (`bddkit.yaml` or
/// `bddkit.yml`), and returns that directory — the default-discovery tests
/// run the binary with `current_dir(dir)` and no `--config` at all.
fn write_default_config_project(name: &str, config_file_name: &str) -> std::path::PathBuf {
    let cfg = write_doctor_project(
        name,
        "Feature: only\n  Scenario: one\n    Given set variable \"ping\" to \"ok\"\n",
        "",
    );
    let dir = cfg.parent().expect("cfg has a parent").to_path_buf();
    std::fs::rename(&cfg, dir.join(config_file_name)).expect("rename to default config name");
    dir
}

#[test]
fn doctor_picks_up_bddkit_yaml_from_the_working_directory() {
    let dir = write_default_config_project("doctor-default-yaml", "bddkit.yaml");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .arg("doctor")
        .current_dir(&dir)
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(
        stdout.contains("bddkit.yaml (found in working directory)"),
        "{stdout}"
    );
}

#[test]
fn doctor_picks_up_the_yml_spelling_too() {
    let dir = write_default_config_project("doctor-default-yml", "bddkit.yml");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .arg("doctor")
        .current_dir(&dir)
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(
        stdout.contains("bddkit.yml (found in working directory)"),
        "{stdout}"
    );
}

#[test]
fn doctor_reports_both_default_spellings_present_as_a_failed_config_row() {
    let dir = write_default_config_project("doctor-default-both", "bddkit.yaml");
    std::fs::copy(dir.join("bddkit.yaml"), dir.join("bddkit.yml")).expect("copy config");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .arg("doctor")
        .current_dir(&dir)
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("bddkit.yaml"), "{stdout}");
    assert!(stdout.contains("bddkit.yml"), "{stdout}");
}

#[test]
fn doctor_reports_no_config_found_as_a_failed_config_row_not_exit_two() {
    let dir =
        std::env::temp_dir().join(format!("bddkit-doctor-default-none-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    // A previous run of this test binary may have left a config behind.
    let _ = std::fs::remove_file(dir.join("bddkit.yaml"));
    let _ = std::fs::remove_file(dir.join("bddkit.yml"));
    let _ = std::fs::remove_file(dir.join("junit.xml"));

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--junit", "junit.xml"])
        .current_dir(&dir)
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "doctor never exits 2: {stdout}");
    assert!(stdout.contains("no config found"), "{stdout}");
    // `run` prepares its report paths before it resolves the config, so
    // `doctor` must too — even when there is no config to resolve.
    assert!(stdout.contains("reports"), "{stdout}");
    assert!(dir.join("junit.xml").is_file(), "{stdout}");
}

#[test]
fn bddkit_config_env_var_is_honoured_when_no_flag_is_given() {
    let cfg = write_doctor_project(
        "doctor-default-env",
        "Feature: only\n  Scenario: one\n    Given set variable \"ping\" to \"ok\"\n",
        "",
    );

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .arg("doctor")
        .env("BDDKIT_CONFIG", &cfg)
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(stdout.contains("(BDDKIT_CONFIG)"), "{stdout}");
}

/// A `--config` naming a file that does not exist must never quietly fall
/// through to a `bddkit.yaml` that happens to sit in the working directory —
/// resolution order step 1 wins outright, existence or not.
#[test]
fn an_explicit_missing_config_flag_is_refused_not_a_fallback_to_the_default() {
    let dir = write_default_config_project("doctor-explicit-missing", "bddkit.yaml");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config", "does-not-exist.yaml"])
        .current_dir(&dir)
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(
        stdout.contains("does-not-exist.yaml"),
        "names the file it was told, not the default: {stdout}"
    );
    assert!(
        !stdout.contains("no problems found"),
        "must not silently succeed against bddkit.yaml instead: {stdout}"
    );
}

/// Same guarantee, for `$BDDKIT_CONFIG` — resolution order step 2.
#[test]
fn a_missing_bddkit_config_env_var_is_refused_not_a_fallback_to_the_default() {
    let dir = write_default_config_project("doctor-env-missing", "bddkit.yaml");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .arg("doctor")
        .env("BDDKIT_CONFIG", "does-not-exist.yaml")
        .current_dir(&dir)
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(
        stdout.contains("does-not-exist.yaml"),
        "names the file it was told, not the default: {stdout}"
    );
    assert!(
        !stdout.contains("no problems found"),
        "must not silently succeed against bddkit.yaml instead: {stdout}"
    );
}

/// Resolution order step 1 over step 2: an explicit `--config` beats
/// `$BDDKIT_CONFIG` even when both name a real, different, healthy project.
#[test]
fn explicit_config_flag_wins_over_the_env_var() {
    let winner = write_doctor_project(
        "doctor-precedence-flag",
        "Feature: only\n  Scenario: one\n    Given set variable \"ping\" to \"ok\"\n",
        "",
    );
    let loser = write_doctor_project(
        "doctor-precedence-env",
        "Feature: only\n  Scenario: one\n    Given set variable \"ping\" to \"ok\"\n",
        "",
    );

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "doctor",
            "--config",
            winner.to_str().expect("path is UTF-8"),
        ])
        .env("BDDKIT_CONFIG", &loser)
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(
        stdout.contains(&format!("{} (--config)", winner.display())),
        "the flag's path and source, not the env var's: {stdout}"
    );
}

#[test]
fn run_with_no_config_anywhere_exits_two_naming_the_files_it_looked_for() {
    let dir = std::env::temp_dir().join(format!("bddkit-run-default-none-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let _ = std::fs::remove_file(dir.join("bddkit.yaml"));
    let _ = std::fs::remove_file(dir.join("bddkit.yml"));
    let _ = std::fs::remove_file(dir.join("junit.xml"));

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--junit", "junit.xml"])
        .current_dir(&dir)
        .output()
        .expect("failed to run bddkit");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("bddkit.yaml"), "{stderr}");
    assert!(stderr.contains("bddkit.yml"), "{stderr}");
    // An empty report a CI parser rejects loudly, never yesterday's green one.
    assert!(dir.join("junit.xml").is_file(), "{stderr}");
}

#[test]
fn resource_add_with_no_config_anywhere_exits_one() {
    let dir = std::env::temp_dir().join(format!("bddkit-add-default-none-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let _ = std::fs::remove_file(dir.join("bddkit.yaml"));
    let _ = std::fs::remove_file(dir.join("bddkit.yml"));

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["resource", "add", "api", "staging"])
        .current_dir(&dir)
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("no config found"), "{stdout}");
}

/// `--no-config` must not even attempt to load `bddkit.yaml`, so a broken one
/// does not block the simplest possible vocabulary lookup — this is the
/// scenario the flag exists for (see issue #48).
#[test]
fn steps_list_no_config_ignores_a_present_bddkit_yaml() {
    let dir = std::env::temp_dir().join(format!("bddkit-steps-no-config-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join("bddkit.yaml"), "not: [valid, yaml: at all").expect("write");

    let without_flag = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["steps", "list"])
        .current_dir(&dir)
        .output()
        .expect("failed to run bddkit");
    assert_eq!(
        without_flag.status.code(),
        Some(2),
        "the broken default config is picked up and fails: {}",
        String::from_utf8_lossy(&without_flag.stdout)
    );

    let with_flag = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["steps", "list", "--no-config"])
        .current_dir(&dir)
        .output()
        .expect("failed to run bddkit");
    let stdout = String::from_utf8_lossy(&with_flag.stdout);
    assert_eq!(with_flag.status.code(), Some(0), "{stdout}");
    assert!(stdout.contains("I sleep"), "{stdout}");
}

/// clap's `conflicts_with` on the real binary, not just declared in
/// `main.rs` — for both commands that carry the flag.
#[test]
fn steps_list_refuses_config_and_no_config_together() {
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["steps", "list", "--config", "x.yaml", "--no-config"])
        .output()
        .expect("failed to run bddkit");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("cannot be used with"), "{stderr}");
}

#[test]
fn resource_fields_refuses_config_and_no_config_together() {
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["resource", "fields", "--config", "x.yaml", "--no-config"])
        .output()
        .expect("failed to run bddkit");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("cannot be used with"), "{stderr}");
}

/// The first question an agent asks a binary on `PATH`. `version` and
/// `--version` must be the same answer, and its first line must stay the one
/// line a script greps.
#[test]
fn version_answers_to_both_spellings_and_leads_with_the_semver() {
    let subcommand = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .arg("version")
        .output()
        .expect("failed to run bddkit");
    let flag = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .arg("--version")
        .output()
        .expect("failed to run bddkit");

    let short = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .arg("-V")
        .output()
        .expect("failed to run bddkit");

    assert_eq!(subcommand.status.code(), Some(0));
    assert_eq!(flag.status.code(), Some(0));
    assert_eq!(short.status.code(), Some(0));

    let text = String::from_utf8_lossy(&subcommand.stdout);
    assert_eq!(text, String::from_utf8_lossy(&flag.stdout), "same answer");

    let first = text.lines().next().expect("version prints something");
    assert_eq!(
        first,
        format!("bddkit {}", env!("CARGO_PKG_VERSION")),
        "the first line is the binary and its version, nothing else"
    );
    assert!(text.contains("bddkit steps list"), "signposts: {text}");
    assert!(text.contains("bddkit doctor"), "signposts: {text}");

    // The whole point of the `version` / `long_version` split: a one-token
    // edit that feeds the long form to `-V` too breaks every script grepping
    // it, and every assertion above would stay green.
    assert_eq!(
        String::from_utf8_lossy(&short.stdout),
        format!("bddkit {}\n", env!("CARGO_PKG_VERSION")),
        "-V stays the one line a script greps"
    );
}

/// A project with one passing and one failing scenario. The failure text
/// carries the two things an XML writer gets wrong first: a `]]>` in the
/// expected value and the NUL bytes of the `<<null>>` sentinel in the actual.
fn write_report_project(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("bddkit-{name}-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/report.feature"),
        r#"Feature: reported feature
  Scenario: passes
    When set variable "ping" to "ok"
    Then variable "ping" should be equal to "ok"

  Scenario: fails
    When set variable "ping" to "ok"
    And set variable "x" to "<<null>>"
    Then variable "x" should be equal to "]]>"
    And variable "ping" should be equal to "ok"
"#,
    )
    .expect("write feature");
    let config = dir.join("cfg.yaml");
    std::fs::write(
        &config,
        format!(
            "paths: [{}]\nresources:\n  api: {{}}\n",
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("write config");
    config
}

#[test]
fn run_junit_writes_a_well_formed_report_for_a_failing_run() {
    let config = write_report_project("junit");
    let report = config.with_file_name("out/junit.xml");
    let _ = std::fs::remove_dir_all(report.parent().expect("parent"));

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(&config)
        .arg("--junit")
        .arg(&report)
        .output()
        .expect("run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    let xml = std::fs::read_to_string(&report).expect("the report is written");
    let package = sxd_document::parser::parse(&xml)
        .unwrap_or_else(|e| panic!("the report must parse as XML: {e:?}\n{xml}"));
    let doc = package.as_document();
    let value = |xpath: &str| {
        sxd_xpath::evaluate_xpath(&doc, xpath)
            .expect("xpath evaluates")
            .string()
    };
    assert_eq!(value("count(//testsuite)"), "1", "{xml}");
    assert_eq!(value("count(//testcase)"), "2", "{xml}");
    assert_eq!(
        value("count(//testcase[@name='fails']/failure)"),
        "1",
        "{xml}"
    );
    assert_eq!(
        value("count(//testcase[@name='passes']/failure)"),
        "0",
        "{xml}"
    );
    let failure = value("//testcase[@name='fails']/failure");
    assert!(failure.contains("expected: ]]>"), "{failure}");
    assert!(
        value("//testcase[@name='passes']/@time")
            .parse::<f64>()
            .expect("time is a float")
            > 0.0,
        "{xml}"
    );
    let steps = value("//testcase[@name='fails']/system-out");
    assert!(
        steps.contains("passed") && steps.contains("failed") && steps.contains("skipped"),
        "{steps}"
    );
}

#[test]
fn run_cucumber_json_writes_the_feature_scenario_step_layout() {
    let config = write_report_project("cucumber-json");
    let report = config.with_file_name("cucumber.json");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(&config)
        .arg("--cucumber-json")
        .arg(&report)
        .output()
        .expect("run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    let json: Value =
        serde_json::from_slice(&std::fs::read(&report).expect("the report is written"))
            .expect("the report parses as JSON");
    let feature = &json[0];
    assert!(
        feature["uri"]
            .as_str()
            .expect("uri")
            .ends_with("report.feature"),
        "{json}"
    );
    assert_eq!(feature["name"], "reported feature", "{json}");
    let scenarios = feature["elements"].as_array().expect("elements");
    assert_eq!(scenarios.len(), 2, "{json}");
    let failing = &scenarios[1];
    assert_eq!(failing["name"], "fails", "{json}");
    assert_eq!(failing["type"], "scenario", "{json}");
    let steps = failing["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 4, "{json}");
    assert_eq!(steps[0]["keyword"], "When ", "{json}");
    assert_eq!(
        steps[0]["name"], "set variable \"ping\" to \"ok\"",
        "{json}"
    );
    assert_eq!(steps[0]["line"], 7, "{json}");
    assert_eq!(steps[0]["result"]["status"], "passed", "{json}");
    assert!(
        steps[0]["result"]["duration"]
            .as_u64()
            .expect("nanoseconds")
            > 0,
        "{json}"
    );
    assert_eq!(
        steps[1]["name"], "set variable \"x\" to \"<<null>>\"",
        "raw step text: {json}"
    );
    assert_eq!(steps[2]["result"]["status"], "failed", "{json}");
    assert!(
        steps[2]["result"]["error_message"]
            .as_str()
            .expect("error_message")
            .contains("expected: ]]>"),
        "{json}"
    );
    assert_eq!(steps[3]["result"]["status"], "skipped", "{json}");
}

#[test]
fn a_report_path_is_truncated_before_the_config_is_read() {
    let dir = std::env::temp_dir().join(format!("bddkit-report-broken-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let cfg = dir.join("cfg.yaml");
    std::fs::write(
        &cfg,
        "paths: [features]\nresources:\n  api: {}\nmacro_paths: [${BDDKIT_ABSENT_VAR}]\n",
    )
    .expect("write config");
    let report = dir.join("junit.xml");
    std::fs::write(&report, "<testsuites>yesterday's green report</testsuites>")
        .expect("stale report");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(&cfg)
        .arg("--junit")
        .arg(&report)
        .output()
        .expect("run bddkit");

    assert_eq!(
        out.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(&report).expect("the file still exists"),
        "",
        "a stale report must not survive a run that never started"
    );
}

#[test]
fn an_unwritable_report_path_stops_the_run_before_any_step_and_doctor_reports_it() {
    let config = write_report_project("unwritable-report-core");
    // A path under a regular file cannot be created.
    let report = config.join("cucumber.json");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(&config)
        .arg("--cucumber-json")
        .arg(&report)
        .output()
        .expect("run bddkit");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("cucumber.json"),
        "the path is named: {stderr}"
    );
    assert!(
        stderr.contains("run not started"),
        "no step may run before the report is prepared: {stderr}"
    );

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config"])
        .arg(&config)
        .arg("--cucumber-json")
        .arg(&report)
        .output()
        .expect("run bddkit");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("✗ reports"), "{stdout}");
}

/// Builds a temp project for an `I include` acceptance test: copies each
/// `(dest path under the temp dir, source fixture path)` pair, then writes
/// the one `cfg.yaml` every one of these tests shares (an empty `api`,
/// no resource at all — none of them touch a system under test). Returns the
/// project directory.
fn build_include_project(dir_slug: &str, fixtures: &[(&str, &str)]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("bddkit-{dir_slug}-{}", std::process::id()));
    for (dest, src) in fixtures {
        let dest_path = dir.join(dest);
        std::fs::create_dir_all(dest_path.parent().expect("dest has a parent")).expect("mkdir");
        std::fs::copy(src, &dest_path).unwrap_or_else(|e| panic!("copy {src} to {dest}: {e}"));
    }
    std::fs::write(
        dir.join("cfg.yaml"),
        "paths: [features]\nresources:\n  api: {}\n",
    )
    .expect("write config");
    dir
}

/// Runs `bddkit <subcommand> --config cfg.yaml` inside a project directory
/// built by `build_include_project`, and decodes the result for assertions.
fn run_bddkit_in(subcommand: &str, dir: &std::path::Path) -> (Option<i32>, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([subcommand, "--config", "cfg.yaml"])
        .current_dir(dir)
        .output()
        .expect("failed to run bddkit");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// `I include` runs another file's scenario inline and copies back only its
/// declared `@exports` variable — no HTTP steps involved, so no stub server
/// is needed.
#[test]
fn include_of_a_one_scenario_file_exports_its_declared_variable() {
    let dir = build_include_project(
        "include-test",
        &[
            (
                "features/target.feature",
                "tests/features/include/target.feature",
            ),
            (
                "features/caller.feature",
                "tests/features/include/caller.feature",
            ),
        ],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(0),
        "--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
}

/// A nested `I include` resolves its path against the file it is written in
/// (issue #76): `lib.feature` sits in `includes/` and includes a sibling, so
/// the runtime must agree with the validator on `includes/`, not its parent.
#[test]
fn a_nested_include_resolves_relative_to_the_intermediate_file() {
    let dir = build_include_project(
        "include-nested",
        &[
            (
                "includes/lib.feature",
                "tests/features/include/nested/includes/lib.feature",
            ),
            (
                "features/caller.feature",
                "tests/features/include/nested/features/caller.feature",
            ),
        ],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(0),
        "--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
}

/// `I include "<file>" with prefix "<p>"` (issue #52) renames every exported
/// variable to `<p>_<name>` as it crosses back into the caller — the same
/// target file is included twice here, once per prefix, and neither call's
/// `userId` export must clobber the other's.
#[test]
fn including_the_same_file_twice_with_different_prefixes_keeps_both_exports() {
    let dir = build_include_project(
        "include-prefix-test",
        &[
            (
                "features/target.feature",
                "tests/features/include/target.feature",
            ),
            (
                "features/caller.feature",
                "tests/features/include/caller_prefix.feature",
            ),
        ],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(0),
        "--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
}

/// A failed step inside an included scenario must not leave the caller's
/// file-level `VarStack` swapped for the included one — `run_include` must
/// restore it on the way out regardless of the include's own outcome. Two
/// scenarios in one file: the first sets a variable then includes a scenario
/// whose only step fails cleanly (an unset-variable assertion, never a
/// panic); the second — sharing the file's `VarStack` per invariant 2 —
/// asserts the first scenario's variable is still there. If a later refactor
/// ever short-circuits before the restore (a `?` between the swap and the
/// restore, or a fresh `push_frame` swapped in for the whole-stack swap),
/// this regresses: scenario 2 sees "before" undefined and fails too.
#[test]
fn a_failed_include_still_restores_the_callers_var_stack() {
    // These fixtures live under `tests/fixtures/include/`, NOT
    // `tests/features/`, because `restore_on_failure.feature` fails one
    // scenario on purpose — `tests/fixtures/` is this repo's existing home
    // for fixtures that are not part of the "every feature file passes"
    // acceptance gate (see `tests/fixtures/echo-plugin`,
    // `tests/fixtures/worker-plugin`). The include target is placed in a
    // sibling `targets/` directory, outside `paths: [features]` — its own
    // `.feature` file's `I include "../targets/..."` path matches — so it is
    // reached only through the include and never discovered and run a
    // second time on its own, which would add an unrelated failed scenario.
    let dir = build_include_project(
        "include-restore-test",
        &[
            (
                "targets/failing_target.feature",
                "tests/fixtures/include/failing_target.feature",
            ),
            (
                "features/restore_on_failure.feature",
                "tests/fixtures/include/restore_on_failure.feature",
            ),
        ],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(1),
        "one scenario fails (the include), the other passes — never a validation exit 2\n\
         --- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(
        stdout.contains("The included scenario fails"),
        "the failing scenario ran: --- stdout ---\n{stdout}"
    );
    // The report only prints FAILING scenarios, so scenario 2 passing leaves
    // no line of its own — the summary counts are the proof it ran and
    // passed: one file, two scenarios, exactly one failed.
    assert!(
        stdout.contains("scenarios: 2, failed: 1"),
        "scenario 2 (\"before\" == \"kept\") must pass silently, not add a \
         second failure — the swap must have been restored: \
         --- stdout ---\n{stdout}"
    );
    assert_eq!(
        stdout.matches("is not set").count(),
        1,
        "only the include's own failure should mention an unset variable; a \
         second occurrence would mean scenario 2 also saw \"before\" as \
         undefined — the swap was left in place: --- stdout ---\n{stdout}"
    );
}

/// A `with:` table cell is interpolated against the CALLER's scope before
/// `world.vars` is swapped for the include's fresh stack — never left as a
/// literal `<<...>>` token, and never resolved against the (empty) included
/// scope. `target_with.feature` stores the value it receives into its own
/// variable and compares it against a fixed literal, independent of the
/// substitution token itself, so a regression that skips the caller-scope
/// interpolation (or interpolates too late, after the swap) fails the run
/// instead of coincidentally still matching.
#[test]
fn with_table_cell_is_interpolated_against_the_callers_scope() {
    // Same reasoning and layout as the restore-on-failure test above:
    // `target_with.feature` fails standalone (its own Examples row is
    // `unused@example.com`, not `test@example.com`), so it goes in the
    // sibling `targets/` directory, reached only through the include.
    let dir = build_include_project(
        "include-with-test",
        &[
            (
                "targets/target_with.feature",
                "tests/fixtures/include/target_with.feature",
            ),
            (
                "features/caller_with.feature",
                "tests/fixtures/include/caller_with.feature",
            ),
        ],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(0),
        "--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
}

/// Invariant 2 (state scoping) says variables never flow into an included
/// scenario except through `with:` — this proves the negative directly: the
/// included scenario reads a variable the caller set and must find it
/// genuinely UNDEFINED, not merely empty. `variable "callerOnly" should be
/// empty` first calls `variable(w, name)` (`steps/vars.rs`), which fails with
/// `variable "callerOnly" is not set` for anything never `set` in the
/// included scenario's own fresh `VarStack` — a stale-but-visible value would
/// instead either match (if still `"secret"`) or fail on content, never on
/// "not set". If `run_include` ever stopped swapping in a fresh `VarStack`
/// (or swapped it in too late), this test would fail differently: the
/// assertion would report the value `"secret"` instead of "is not set", or
/// pass outright were the check changed to tolerate that.
#[test]
fn a_caller_variable_is_not_visible_inside_an_included_scenario() {
    // Same layout as `a_failed_include_still_restores_the_callers_var_stack`:
    // the included scenario fails on purpose, so both fixtures live under
    // `tests/fixtures/include/`, and the target sits in a sibling `targets/`
    // directory outside `paths: [features]` so it is reached only through
    // the include, never discovered and run a second time on its own.
    let dir = build_include_project(
        "include-isolation-test",
        &[
            (
                "targets/isolation_target_cannot_read_caller.feature",
                "tests/fixtures/include/isolation_target_cannot_read_caller.feature",
            ),
            (
                "features/isolation_caller_cannot_read.feature",
                "tests/fixtures/include/isolation_caller_cannot_read.feature",
            ),
        ],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(1),
        "the included scenario's own assertion must fail (the variable is \
         genuinely undefined inside the include), which fails the include \
         step and the caller scenario with it: \
         --- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(
        stdout.contains(r#"variable "callerOnly" is not set"#),
        "the failure must name the variable as UNSET, not merely empty or \
         mismatched — proving the caller's value never crossed into the \
         included scenario's scope: --- stdout ---\n{stdout}"
    );
}

/// Invariant 2's other half: a DECLARED `@exports` name reaches the caller,
/// but anything else the included scenario sets does not, even though both
/// live in the same fresh `VarStack` while the include runs. Two scenarios
/// sharing the caller file's `VarStack` (persisted across scenarios within
/// one file, per invariant 2): the first includes and checks the declared
/// export `kept` arrived; the second — without including anything itself —
/// asserts `notExported` is still undefined. If `run_include` ever copied
/// the whole included scope back instead of just the declared exports, the
/// second scenario would find `notExported == "no"` and pass instead of
/// failing — so this test is provative in the direction that matters: a
/// leak turns its expected failure into an unexpected pass, not the reverse.
#[test]
fn only_the_declared_export_reaches_the_caller() {
    let dir = build_include_project(
        "include-export-boundary-test",
        &[
            (
                "targets/isolation_target_export_boundary.feature",
                "tests/fixtures/include/isolation_target_export_boundary.feature",
            ),
            (
                "features/isolation_caller_export_boundary.feature",
                "tests/fixtures/include/isolation_caller_export_boundary.feature",
            ),
        ],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(1),
        "scenario 1 (declared export `kept`) must pass; scenario 2 must \
         fail on purpose, proving `notExported` never crossed the boundary: \
         --- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(
        stdout.contains("scenarios: 2, failed: 1"),
        "scenario 1 must pass silently (the declared export `kept` arrived \
         and equalled \"yes\") — a failure there would mean the export \
         itself is broken, not just the boundary: \
         --- stdout ---\n{stdout}"
    );
    assert!(
        stdout.contains(r#"variable "notExported" is not set"#),
        "scenario 2's failure must name `notExported` as UNSET — not \
         merely empty or some other mismatch — proving it never arrived, \
         rather than arriving as an unexpected value: \
         --- stdout ---\n{stdout}"
    );
}

/// A declared `@exports` name that the included scenario never `set`s must
/// fail the include step itself — not the caller scenario's own next
/// assertion — with an error naming the missing export, per `run_include`'s
/// `export_result` handling in `src/runner.rs`. If a regression silently
/// dropped an unresolved export instead of failing (e.g. by skipping it
/// rather than recording `missing`), this run would exit 0 instead of 1 and
/// the output would never mention "neverSet".
#[test]
fn a_declared_export_that_is_never_set_fails_the_include_step() {
    let dir = build_include_project(
        "include-missing-export-test",
        &[
            (
                "targets/isolation_target_missing_export.feature",
                "tests/fixtures/include/isolation_target_missing_export.feature",
            ),
            (
                "features/isolation_caller_missing_export.feature",
                "tests/fixtures/include/isolation_caller_missing_export.feature",
            ),
        ],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(1),
        "the missing export must fail the include step (and the caller \
         scenario with it), a scenario failure rather than a validation \
         (exit 2) or a silent pass: \
         --- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(
        stdout.contains("declared export \"neverSet\"")
            && stdout.contains("the variable is not set"),
        "the failure must clearly name the missing export \"neverSet\": \
         --- stdout ---\n{stdout}"
    );
}

#[test]
fn include_by_scenario_name_picks_the_named_one() {
    let dir = build_include_project(
        "include-multi-test",
        &[
            (
                "features/multi.feature",
                "tests/features/include/multi.feature",
            ),
            (
                "features/caller_multi.feature",
                "tests/features/include/caller_multi.feature",
            ),
        ],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(0),
        "--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
}

#[test]
fn include_of_an_outline_uses_the_callers_with_row() {
    let dir = build_include_project(
        "include-outline-with-test",
        &[
            (
                "features/outline.feature",
                "tests/features/include/outline.feature",
            ),
            (
                "features/caller_outline_with.feature",
                "tests/features/include/caller_outline_with.feature",
            ),
        ],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(0),
        "--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
}

#[test]
fn include_of_an_outline_with_one_examples_row_and_no_with_uses_that_row() {
    let dir = build_include_project(
        "include-outline-bare-test",
        &[
            (
                "features/outline.feature",
                "tests/features/include/outline.feature",
            ),
            (
                "features/caller_outline_bare.feature",
                "tests/features/include/caller_outline_bare.feature",
            ),
        ],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(0),
        "--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
}

#[test]
fn debug_mode_logs_the_includes_with_and_export_lines() {
    let dir = build_include_project(
        "debug-include-test",
        &[
            (
                "features/outline.feature",
                "tests/features/include/outline.feature",
            ),
            (
                "features/caller_debug_export.feature",
                "tests/features/include/caller_debug_export.feature",
            ),
        ],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(0),
        "--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );

    // In debug mode, the include logs its inputs and every exported variable
    assert!(
        stderr.contains("outline.feature ›"),
        "stderr should contain 'outline.feature ›' (entry line with filename and separator):\n{stderr}"
    );
    assert!(
        stderr.contains("  with value = "),
        "stderr should contain '  with value = ' (with two-space indentation):\n{stderr}"
    );
    assert!(
        stderr.contains("  export seen = "),
        "stderr should contain '  export seen = ' (with two-space indentation):\n{stderr}"
    );
}

/// The `with prefix "<p>"` name applies before the debug log line is
/// printed, so `--debug` shows the name that actually lands in the caller's
/// scope (`buyer_seen`), not the included scenario's own export name (`seen`).
#[test]
fn debug_mode_logs_the_prefixed_export_name() {
    let dir = build_include_project(
        "debug-include-prefix-test",
        &[
            (
                "features/outline.feature",
                "tests/features/include/outline.feature",
            ),
            (
                "features/caller_debug_export_prefix.feature",
                "tests/features/include/caller_debug_export_prefix.feature",
            ),
        ],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(0),
        "--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(
        stderr.contains("  export buyer_seen = "),
        "stderr should contain '  export buyer_seen = ' (prefixed export name):\n{stderr}"
    );
}

#[test]
fn debug_mode_logs_a_macro_s_exported_variables() {
    let dir = std::env::temp_dir().join(format!("bddkit-macro-debug-test-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("macros.yaml"),
        "- step: I login as user \"{email}\"\n  do:\n    - set variable \"token\" to \"<<email>>-token\"\n  exports: [token]\n",
    )
    .expect("write macros");
    std::fs::write(
        dir.join("features/caller.feature"),
        "Feature: f\n  Scenario: s\n    Given I am in debug mode\n    When I login as user \"alice@example.com\"\n",
    )
    .expect("write feature");
    std::fs::write(
        dir.join("cfg.yaml"),
        "macro_paths: [macros.yaml]\npaths: [features]\nresources:\n  api: {}\n",
    )
    .expect("write config");

    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(0),
        "--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(
        stderr.contains(r#"macro "I login as user \"alice@example.com\"""#),
        "stderr should name the macro call:\n{stderr}"
    );
    assert!(
        stderr.contains("  export token = alice@example.com-token"),
        "stderr should contain the exported variable:\n{stderr}"
    );
}

/// The header must print before the macro body runs, not after: otherwise a
/// macro whose body fails logs nothing at all, the case debug mode is for.
#[test]
fn debug_mode_names_the_macro_even_when_its_body_fails() {
    let dir = std::env::temp_dir().join(format!(
        "bddkit-macro-debug-fail-test-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("macros.yaml"),
        "- step: I login as user \"{email}\"\n  do:\n    - variable \"missing\" should be equal to \"x\"\n",
    )
    .expect("write macros");
    std::fs::write(
        dir.join("features/caller.feature"),
        "Feature: f\n  Scenario: s\n    Given I am in debug mode\n    When I login as user \"alice@example.com\"\n",
    )
    .expect("write feature");
    std::fs::write(
        dir.join("cfg.yaml"),
        "macro_paths: [macros.yaml]\npaths: [features]\nresources:\n  api: {}\n",
    )
    .expect("write config");

    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(1),
        "the macro body's assertion must fail the run\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(
        stderr.contains(r#"macro "I login as user \"alice@example.com\"""#),
        "the header must print even though the macro body failed:\n{stderr}"
    );
}

#[test]
fn doctor_reports_a_broken_include_the_same_way_run_does() {
    // Same missing-file fixture as include_of_a_missing_file_is_a_problem (Task 7),
    // but invoke `bddkit doctor --config cfg.yaml` instead of `run`.
    // Assert: exit code 1 (doctor's own convention — 0 or 1 only, never 2,
    // per CLAUDE.md) with the missing file's name appearing in stdout.
    let cfg = write_doctor_project(
        "doctor-include",
        "Feature: caller\n  Scenario: test\n    Given I include \"nope.feature\"\n",
        "",
    );

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config", cfg.to_str().expect("path is UTF-8")])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    // Doctor exits 1 (never 2) even for a config that would make run exit 2
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    // The missing file name is mentioned in the output
    assert!(stdout.contains("nope.feature"), "{stdout}");
}

#[test]
fn steps_list_includes_both_include_steps() {
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["steps", "list"])
        .output()
        .expect("failed to run bddkit");

    let text = String::from_utf8_lossy(&out.stdout);
    // Both Include and IncludeScenario steps start with "I include"
    assert!(
        text.contains("I include"),
        "steps list should contain 'I include': {text}"
    );
}

// ---------------------------------------------------------------------------
// `run --events`
// ---------------------------------------------------------------------------

/// A project under a fresh directory, returning the config path. `features`
/// are `(file name, content)` pairs; `macros` is one YAML file, if any. The
/// config declares an empty `api`.
fn write_events_project(
    name: &str,
    features: &[(&str, &str)],
    macros: Option<&str>,
) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("bddkit-events-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    for (file, content) in features {
        std::fs::write(dir.join("features").join(file), content).expect("write feature");
    }
    let mut config = format!("paths: [{}]\n", path_text(&dir.join("features")));
    if let Some(macros) = macros {
        std::fs::create_dir_all(dir.join("macros")).expect("mkdir");
        std::fs::write(dir.join("macros/m.yaml"), macros).expect("write macros");
        config.push_str(&format!(
            "macro_paths: [{}]\n",
            path_text(&dir.join("macros"))
        ));
    }
    config.push_str(
        "resources:
  api: {}
",
    );
    let cfg = dir.join("cfg.yaml");
    std::fs::write(&cfg, config).expect("write config");
    cfg
}

/// `write_events_project` plus the echo fixture plugin, whose steps report
/// evidence. Sequential: the echo plugin is `shared`.
fn write_evidence_project(
    name: &str,
    features: &[(&str, &str)],
    macros: Option<&str>,
) -> std::path::PathBuf {
    let cfg = write_events_project(name, features, macros);
    let dir = cfg.parent().expect("project dir");
    std::fs::create_dir_all(dir.join(".bddkit")).expect("mkdir .bddkit");
    std::fs::write(
        dir.join(".bddkit/plugins.yaml"),
        format!(
            "plugin:\n  - name: echo\n    path: {}\n",
            common::build_fixture_plugin().display()
        ),
    )
    .expect("write lock");
    let mut config = std::fs::read_to_string(&cfg).expect("read config");
    config.push_str("concurrency: 1\n");
    std::fs::write(&cfg, config).expect("write config");
    cfg
}

fn path_text(path: &std::path::Path) -> String {
    path.display().to_string().replace('\\', "/")
}

fn read_stream(path: &std::path::Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .expect("the stream exists")
        .lines()
        .map(|line| serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}: {line:?}")))
        .collect()
}

/// `bddkit run --config <cfg> --events <dir>/events.ndjson <extra…>`, and the
/// stream it wrote.
/// `--bddkit-dir <project>/.bddkit` when the project carries its own plugins,
/// so a developer's installed plugins never decide what a test sees.
fn project_bddkit_dir(cfg: &std::path::Path) -> Vec<std::ffi::OsString> {
    let dir = cfg.with_file_name(".bddkit");
    if dir.is_dir() {
        vec!["--bddkit-dir".into(), dir.into()]
    } else {
        vec![]
    }
}

fn run_with_events(
    cfg: &std::path::Path,
    extra: &[&str],
) -> (std::process::Output, Vec<Value>, std::path::PathBuf) {
    let events = cfg.with_file_name("events.ndjson");
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(cfg)
        .arg("--events")
        .arg(&events)
        .args(project_bddkit_dir(cfg))
        .env_remove("BDDKIT_ARTIFACTS_DIR")
        // The developer's own choice must not decide what a test records.
        .env_remove("BDDKIT_EVIDENCE")
        .env_remove("BDDKIT_EVIDENCE_BUFFER")
        .args(extra)
        .output()
        .expect("run bddkit");
    let stream = read_stream(&events);
    (out, stream, events)
}

fn of_type<'a>(stream: &'a [Value], kind: &str) -> Vec<&'a Value> {
    stream.iter().filter(|e| e["type"] == kind).collect()
}

#[test]
fn an_events_path_is_truncated_before_the_config_is_read() {
    let cfg = write_events_project("broken", &[], None);
    std::fs::write(
        &cfg,
        "paths: [features]\nresources:\n  api: {}\nmacro_paths: [${BDDKIT_ABSENT_VAR}]\n",
    )
    .expect("write config");
    std::fs::write(
        cfg.with_file_name("events.ndjson"),
        "{\"type\":\"yesterday\"}\n",
    )
    .expect("stale stream");

    let (out, stream, _) = run_with_events(&cfg, &[]);

    assert_eq!(out.status.code(), Some(2));
    assert!(stream.is_empty(), "{stream:?}");
}

#[test]
fn a_refused_run_leaves_an_empty_events_file() {
    let cfg = write_events_project(
        "refused",
        &[(
            "a.feature",
            "Feature: a\n  Scenario: s\n    Given a step nobody defined\n",
        )],
        None,
    );
    std::fs::write(
        cfg.with_file_name("events.ndjson"),
        "{\"type\":\"yesterday\"}\n",
    )
    .expect("stale stream");

    let (out, stream, _) = run_with_events(&cfg, &[]);

    assert_eq!(out.status.code(), Some(2));
    assert!(stream.is_empty(), "{stream:?}");
}

#[test]
fn every_line_of_the_stream_is_one_json_object_with_a_rising_seq() {
    let cfg = write_report_project("events-lines");
    let (_, stream, _) = run_with_events(&cfg, &[]);

    assert!(stream.len() > 6, "{stream:?}");
    for (index, event) in stream.iter().enumerate() {
        assert!(event.is_object(), "{event}");
        assert_eq!(event["seq"], index, "{event}");
        assert!(event["type"].is_string(), "{event}");
        if index > 0 {
            assert!(
                event["t"].as_u64() >= stream[index - 1]["t"].as_u64(),
                "t goes back: {event}"
            );
        }
    }
}

#[test]
fn run_started_names_the_artifact_root() {
    let cfg = write_report_project("events-artifact-root");
    let (_, stream, _) = run_with_events(&cfg, &[]);
    let run_id = stream[0]["run_id"].as_str().expect("run_id");
    assert_eq!(
        stream[0]["artifacts_dir"],
        std::env::temp_dir()
            .join(format!("bddkit-{run_id}"))
            .display()
            .to_string(),
        "the default root, absolute"
    );

    // A relative flag value is reported as resolved against the working directory.
    let (_, stream, _) = run_with_events(&cfg, &["--artifacts-dir", "evidence"]);
    let root = stream[0]["artifacts_dir"].as_str().expect("artifacts_dir");
    assert!(std::path::Path::new(root).is_absolute(), "{root}");
    assert_eq!(
        std::path::Path::new(root),
        std::env::current_dir().expect("cwd").join("evidence")
    );
}

#[test]
fn the_stream_opens_with_run_started_and_closes_with_run_finished() {
    let cfg = write_report_project("events-ends");
    let (out, stream, _) = run_with_events(&cfg, &[]);

    let stdout = String::from_utf8_lossy(&out.stdout);
    let first = &stream[0];
    assert_eq!(first["type"], "run_started");
    assert_eq!(first["schema"], 1);
    assert_eq!(first["bddkit"], env!("CARGO_PKG_VERSION"));
    assert_eq!(first["files"], 1);
    assert!(first["started_at_unix_ms"].as_u64().unwrap_or(0) > 1_700_000_000_000);
    let run_id = first["run_id"].as_str().expect("run_id");
    assert!(stdout.contains(&format!("run {run_id}")), "{stdout}");

    let last = stream.last().expect("lines");
    assert_eq!(last["type"], "run_finished");
    assert_eq!(last["exit"], 1, "one scenario of the project fails");
    assert_eq!(last.get("signal"), None);
    assert_eq!(of_type(&stream, "run_finished").len(), 1);
}

#[test]
fn feature_background_rule_and_examples_descriptions_reach_the_stream() {
    let cfg = write_events_project(
        "descriptions-file",
        &[
            (
                "a.feature",
                "@ft1\n# c\n@ft2\nFeature: Described\n  What the file is for.\n\n    Indented line.\n\n  Background: Setup\n    Runs first.\n    Given set variable \"a\" to \"1\"\n\n  Scenario: s\n    Given set variable \"b\" to \"2\"\n",
            ),
            (
                "b.feature",
                "Feature: Bare\n  Background:\n    Given set variable \"a\" to \"1\"\n  Scenario: s\n    Given set variable \"b\" to \"2\"\n",
            ),
            (
                "d.feature",
                "Feature:\n  Scenario: s\n    Given set variable \"a\" to \"1\"\n",
            ),
            (
                "c.feature",
                "Feature: Ruled\n  Scenario: Loose\n    Given set variable \"a\" to \"1\"\n  Rule: Money\n    Everything about money.\n    Background: Prep\n      Prep text.\n      Given set variable \"a\" to \"1\"\n    Scenario Outline: pay\n      Given set variable \"n\" to \"<n>\"\n      Examples: Amounts\n        Rows of amounts.\n        | n |\n        | 1 |\n        | 2 |\n",
            ),
        ],
        None,
    );
    let (out, stream, _) = run_with_events(&cfg, &[]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");

    // A scenario inside a rule names the rule, its background and, for an
    // outline row, the Examples block it came from.
    let scenarios = of_type(&stream, "scenario_started");
    let rows: Vec<_> = scenarios
        .iter()
        .filter(|e| e["name"].as_str().is_some_and(|n| n.starts_with("pay")))
        .collect();
    assert_eq!(rows.len(), 2, "{scenarios:?}");
    for row in rows {
        assert_eq!(
            row["rule"],
            serde_json::json!({
                "line": 4, "name": "Money", "description": "Everything about money.",
                "background": {"line": 6, "name": "Prep", "description": "Prep text."},
            })
        );
        assert_eq!(
            row["examples"],
            serde_json::json!({"line": 11, "name": "Amounts", "description": "Rows of amounts."})
        );
    }
    let loose = scenarios
        .iter()
        .find(|e| e["name"] == "Loose")
        .expect("Loose");
    assert!(
        loose.get("rule").is_none() && loose.get("examples").is_none(),
        "{loose}"
    );

    let started = of_type(&stream, "file_started");
    let file = |name: &str| started.iter().find(|e| e["name"] == name).expect(name);

    let described = file("Described");
    assert_eq!(
        described["description"],
        "What the file is for.\n\n  Indented line."
    );
    assert_eq!(described["tags"], serde_json::json!(["@ft1", "@ft2"]));
    assert_eq!(
        described["background"],
        serde_json::json!({"line": 9, "name": "Setup", "description": "Runs first."})
    );

    // An unnamed feature keeps the `name` it always had.
    assert_eq!(file("")["name"], "");

    // Nothing to say is nothing in the stream, except `tags`, always a list.
    let bare = file("Bare");
    assert_eq!(bare["tags"], serde_json::json!([]));
    assert_eq!(bare["background"], serde_json::json!({"line": 2}));
    assert!(bare.get("description").is_none(), "{bare}");
}

#[test]
fn a_scenario_description_reaches_the_stream_with_its_paragraphs() {
    let cfg = write_events_project(
        "descriptions-scenario",
        &[(
            "a.feature",
            "Feature: f\n  Scenario: Described\n    Why this exists.\n\n      Indented bit.\n\n    Second paragraph.\n    Given set variable \"a\" to \"1\"\n  Scenario: Bare\n    Given set variable \"a\" to \"1\"\n",
        )],
        None,
    );
    let (out, stream, _) = run_with_events(&cfg, &[]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let started = of_type(&stream, "scenario_started");
    let by = |name: &str| started.iter().find(|e| e["name"] == name).expect(name);
    assert_eq!(
        by("Described")["description"],
        "Why this exists.\n\n  Indented bit.\n\nSecond paragraph."
    );
    assert!(by("Bare").get("description").is_none(), "{}", by("Bare"));
}

#[test]
fn scenario_started_carries_the_tags_the_filter_saw() {
    let cfg = write_events_project(
        "descriptions-tags",
        &[
            (
                "a.feature",
                "@f @dup\nFeature: Tagged\n  Scenario: Untagged\n    Given set variable \"a\" to \"1\"\n  @r\n  Rule: R\n    @s @dup\n    Scenario: Picked\n      Given set variable \"a\" to \"1\"\n",
            ),
            (
                "b.feature",
                "Feature: Plain\n  Scenario: Plain one\n    Given set variable \"a\" to \"1\"\n",
            ),
        ],
        None,
    );

    // Own tags, then the rule's, then the feature's, each name once.
    let (_, stream, _) = run_with_events(&cfg, &["--tag", "s"]);
    let started = of_type(&stream, "scenario_started");
    assert_eq!(
        started.len(),
        1,
        "only what the filter selected: {started:?}"
    );
    assert_eq!(
        started[0]["tags"],
        serde_json::json!(["@s", "@dup", "@r", "@f"])
    );

    let (_, stream, _) = run_with_events(&cfg, &[]);
    let started = of_type(&stream, "scenario_started");
    let tags = |name: &str| started.iter().find(|e| e["name"] == name).expect(name)["tags"].clone();
    assert_eq!(tags("Untagged"), serde_json::json!(["@f", "@dup"]));
    assert_eq!(tags("Plain one"), serde_json::json!([]), "always present");
}

#[test]
fn a_row_carries_the_tags_of_its_examples_block() {
    let cfg = write_events_project(
        "examples-tags",
        &[(
            "a.feature",
            "@f\nFeature: Outline\n  @r\n  Rule: R\n    @s @slow\n    Scenario Outline: Row <n>\n      Given set variable \"a\" to \"<n>\"\n\n      @slow @fast\n      Examples:\n        | n |\n        | 1 |\n        | 2 |\n\n      Examples:\n        | n |\n        | 3 |\n",
        )],
        None,
    );
    let report = cfg.with_file_name("cucumber.json");
    let (out, stream, _) = run_with_events(
        &cfg,
        &["--tag", "fast", "--cucumber-json", report.to_str().unwrap()],
    );
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let started = of_type(&stream, "scenario_started");
    let names: Vec<_> = started
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["Row <n> [1]", "Row <n> [2]"],
        "only the tagged block"
    );
    // Scenario, rule, feature, then the block; `@slow` is written twice, kept once.
    assert_eq!(
        started[0]["tags"],
        serde_json::json!(["@s", "@slow", "@r", "@f", "@fast"])
    );

    let json: Value = serde_json::from_slice(&std::fs::read(&report).expect("report")).unwrap();
    let tags: Vec<_> = json[0]["elements"][0]["tags"]
        .as_array()
        .expect("tags")
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(tags, ["@s", "@slow", "@r", "@f", "@fast"]);

    // A tag on the outline still selects every row; the untagged block's row
    // does not carry the other block's tags.
    let (_, stream, _) = run_with_events(&cfg, &["--tag", "s"]);
    let started = of_type(&stream, "scenario_started");
    assert_eq!(started.len(), 3, "{started:?}");
    assert_eq!(
        started[2]["tags"],
        serde_json::json!(["@s", "@slow", "@r", "@f"])
    );
}

#[test]
fn an_examples_tag_alone_selects_its_rows_and_their_rules_background() {
    let cfg = write_events_project(
        "examples-tags-rule-background",
        &[(
            "a.feature",
            "Feature: Outline\n  Rule: R\n    Background:\n      Given no such step\n\n    Scenario Outline: Row\n      Given set variable \"a\" to \"<n>\"\n\n      @only\n      Examples:\n        | n |\n        | 1 |\n",
        )],
        None,
    );
    let (out, _, _) = run_with_events(&cfg, &["--tag", "only"]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("no such step"),
        "{out:?}"
    );
}

#[test]
fn validation_skips_the_rows_an_examples_tag_left_out() {
    let cfg = write_events_project(
        "examples-tags-validation",
        &[(
            "a.feature",
            "Feature: Outline\n  Scenario Outline: Row\n    Given <step>\n\n    @fast\n    Examples:\n      | step |\n      | set variable \"a\" to \"1\" |\n\n    Examples:\n      | step |\n      | no such step |\n",
        )],
        None,
    );
    let (out, _, _) = run_with_events(&cfg, &["--tag", "fast"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
}

#[test]
fn the_cucumber_json_report_carries_descriptions_tags_and_the_feature_line() {
    let cfg = write_events_project(
        "descriptions-json",
        &[(
            "a.feature",
            "# language: en\n@ft @dup\nFeature: Reported\n  Feature text.\n  Scenario: Described\n    What it is for.\n    Given set variable \"a\" to \"1\"\n  @own @dup\n  Scenario Outline: Row\n    Outline text.\n    Given set variable \"n\" to \"<n>\"\n    Examples:\n      | n |\n      | 1 |\n      | 2 |\n  Scenario: Bare\n    Given set variable \"a\" to \"1\"\n",
        )],
        None,
    );
    let json = cfg.with_file_name("cucumber.json");
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(&cfg)
        .arg("--cucumber-json")
        .arg(&json)
        .output()
        .expect("run bddkit");
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let report: Value =
        serde_json::from_str(&std::fs::read_to_string(&json).expect("report")).expect("json");

    let feature = &report[0];
    assert_eq!(feature["line"], 3, "the keyword line, not 1");
    assert_eq!(feature["description"], "Feature text.");
    let ft = serde_json::json!([{"name": "@ft", "line": 2}, {"name": "@dup", "line": 2}]);
    assert_eq!(feature["tags"], ft);

    let elements = feature["elements"].as_array().expect("elements");
    let by = |prefix: &str| -> Vec<&Value> {
        elements
            .iter()
            .filter(|e| e["name"].as_str().is_some_and(|n| n.starts_with(prefix)))
            .collect()
    };
    assert_eq!(by("Described")[0]["description"], "What it is for.");
    assert_eq!(
        by("Described")[0]["tags"],
        ft,
        "inherited, with their own lines"
    );

    // Every row of an outline carries the outline's text and tags; a name
    // written at two levels is listed once, on the line of the closest.
    let rows = by("Row");
    assert_eq!(rows.len(), 2);
    for row in rows {
        assert_eq!(row["description"], "Outline text.");
        assert_eq!(
            row["tags"],
            serde_json::json!([
                {"name": "@own", "line": 8}, {"name": "@dup", "line": 8}, {"name": "@ft", "line": 2}
            ])
        );
    }

    // Cucumber's own formatter writes "" for no description.
    assert_eq!(by("Bare")[0]["description"], "");
}

#[test]
fn a_description_is_masked_in_the_json_report_with_the_final_set_but_not_rewritten_in_the_stream() {
    let feature = "Feature: Late\n  Scenario: first\n    Quotes late-value-98765 before it is known.\n    Given set variable \"x\" to \"1\"\n  Scenario: second\n    Given set variable \"secret_late\" to \"late-value-98765\"\n";
    let cfg = masking_project("mask-description-json", feature, "");
    let json = cfg.with_file_name("cucumber.json");
    let events = cfg.with_file_name("events.ndjson");
    let (code, _, stderr) = run_masking(
        &cfg,
        &[
            "--cucumber-json",
            json.to_str().expect("utf-8"),
            "--events",
            events.to_str().expect("utf-8"),
        ],
        &[],
    );
    assert_eq!(code, 0, "{stderr}");

    let report: Value =
        serde_json::from_str(&std::fs::read_to_string(&json).expect("report")).expect("json");
    assert_eq!(
        report[0]["elements"][0]["description"],
        "Quotes *** before it is known."
    );
    // The stream is never rewritten: the value was not a secret yet.
    assert_eq!(
        of_type(&read_stream(&events), "scenario_started")[0]["description"],
        "Quotes late-value-98765 before it is known."
    );
}

#[test]
fn run_started_carries_the_config_and_the_selection() {
    let cfg = write_events_project(
        "run-started-selection",
        &[(
            "a.feature",
            "Feature: f\n  @smoke\n  Scenario: s\n    Given set variable \"a\" to \"1\"\n",
        )],
        None,
    );
    let features = cfg.with_file_name("features");
    let features_text = path_text(&features);

    // Named on the command line, with a selection.
    let (out, stream, _) =
        run_with_events(&cfg, &["--tag", "smoke", "--tag", "@x", &features_text]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(stream[0]["config"], path_text(&cfg));
    assert_eq!(stream[0]["config_source"], "flag");
    assert_eq!(
        stream[0]["selection"],
        serde_json::json!({"tags": ["smoke", "@x"], "paths": [features_text]}),
        "the values as typed"
    );

    // Nothing selected: both lists are there, empty.
    let (_, stream, _) = run_with_events(&cfg, &[]);
    assert_eq!(
        stream[0]["selection"],
        serde_json::json!({"tags": [], "paths": []})
    );

    // Named by the environment, and found in the working directory.
    let events = cfg.with_file_name("events.ndjson");
    let run = |dir: &std::path::Path, env: Option<&std::path::Path>| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_bddkit"));
        command
            .current_dir(dir)
            .env_remove("BDDKIT_CONFIG")
            .env_remove("BDDKIT_ARTIFACTS_DIR")
            .args(["run", "--events"])
            .arg(&events);
        if let Some(env) = env {
            command.env("BDDKIT_CONFIG", env);
        }
        assert_eq!(command.output().expect("run bddkit").status.code(), Some(0));
        read_stream(&events).remove(0)
    };
    let first = run(&std::env::temp_dir(), Some(&cfg));
    assert_eq!(first["config_source"], "env");
    assert_eq!(first["config"], path_text(&cfg));

    std::fs::copy(&cfg, cfg.with_file_name("bddkit.yaml")).expect("copy config");
    let first = run(cfg.parent().expect("dir"), None);
    assert_eq!(first["config_source"], "discovered");
    assert_eq!(
        first["config"], "./bddkit.yaml",
        "the path as it was resolved"
    );
}

#[test]
fn two_files_running_together_interleave_in_the_stream() {
    // The barrier opens only when both steps are in flight, so both
    // `step_started` lines must precede both `step_finished` ones — which a
    // stream buffered until its file ends could never show.
    let dir = std::env::temp_dir().join(format!("bddkit-events-barrier-{}", std::process::id()));
    let cfg = write_parallel_fixture(&dir, 2, "");
    let (out, stream, _) = run_with_events(&cfg, &[]);

    assert!(
        out.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let barrier = |kind: &str| -> Vec<u64> {
        of_type(&stream, kind)
            .into_iter()
            .filter(|e| {
                e["step"] == 1 // the barrier; step 0 sets a variable
            })
            .map(|e| e["seq"].as_u64().expect("seq"))
            .collect()
    };
    let (started, finished) = (barrier("step_started"), barrier("step_finished"));
    assert_eq!((started.len(), finished.len()), (2, 2), "{stream:?}");
    assert!(
        started.iter().max() < finished.iter().min(),
        "the second file started its request only after the first one finished: {stream:?}"
    );
    let files: std::collections::HashSet<_> = of_type(&stream, "file_started")
        .iter()
        .map(|e| e["file"].as_str().expect("file").to_string())
        .collect();
    assert_eq!(files.len(), 2);
}

#[test]
fn steps_after_a_failure_are_reported_as_skipped() {
    let cfg = write_report_project("events-skipped");
    let (_, stream, _) = run_with_events(&cfg, &[]);

    let second: Vec<_> = stream.iter().filter(|e| e["scenario"] == 1).collect();
    let kinds: Vec<_> = second
        .iter()
        .map(|e| (e["type"].as_str().unwrap_or(""), e["step"].as_u64()))
        .collect();
    assert_eq!(
        kinds,
        [
            ("scenario_started", None),
            ("step_started", Some(0)),
            ("step_finished", Some(0)),
            ("step_started", Some(1)),
            ("step_finished", Some(1)),
            ("step_started", Some(2)),
            ("step_finished", Some(2)),
            ("step_skipped", Some(3)),
            ("scenario_finished", None),
        ]
    );
    assert_eq!(second[6]["status"], "failed");
    assert_eq!(
        second[7]["text"],
        "variable \"ping\" should be equal to \"ok\""
    );
    assert_eq!(second[8]["status"], "failed");
}

#[test]
fn outline_rows_get_distinct_example_ordinals() {
    let cfg = write_events_project(
        "outline",
        &[(
            "o.feature",
            "Feature: o\n  Scenario: plain\n    Given set variable \"x\" to \"1\"\n\n  Scenario Outline: rows\n    Given set variable \"x\" to \"<n>\"\n\n    Examples:\n      | n |\n      | 1 |\n      | 2 |\n\n    Examples:\n      | n |\n      | 3 |\n",
        )],
        None,
    );
    let (out, stream, _) = run_with_events(&cfg, &[]);

    assert_eq!(out.status.code(), Some(0));
    let started = of_type(&stream, "scenario_started");
    assert_eq!(started.len(), 4);
    assert_eq!(started[0].get("example"), None, "a plain scenario has none");
    let rows: Vec<_> = started[1..]
        .iter()
        .map(|e| {
            (
                e["scenario"].as_u64(),
                e["example"].as_u64(),
                e["line"].as_u64(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            (Some(1), Some(0), Some(5)),
            (Some(2), Some(1), Some(5)),
            (Some(3), Some(2), Some(5))
        ]
    );
}

#[test]
fn a_macro_body_step_is_an_event_with_its_caller_as_parent() {
    let cfg = write_events_project(
        "macro",
        &[(
            "m.feature",
            "Feature: m\n  Scenario: s\n    Given I remember two things\n",
        )],
        Some(
            "# a comment, so the definition is not on line 1\n\n- step: 'I remember two things'\n  do:\n    - set variable \"a\" to \"1\"\n    - set variable \"b\" to \"2\"\n",
        ),
    );
    let (out, stream, _) = run_with_events(&cfg, &[]);

    assert_eq!(out.status.code(), Some(0));
    let started = of_type(&stream, "step_started");
    assert_eq!(started.len(), 3, "{stream:?}");
    assert_eq!(
        (started[0]["step"].as_u64(), started[0].get("parent")),
        (Some(0), None)
    );
    for (position, child) in started[1..].iter().enumerate() {
        assert_eq!(child["step"], position + 1);
        assert_eq!(child["parent"], 0);
        assert_eq!(child["index"], position);
        assert_eq!(child["keyword"], "");
        assert_eq!(child["line"], 3, "the definition's line: {child}");
        assert!(
            child["source"]
                .as_str()
                .expect("source")
                .ends_with("macros/m.yaml")
        );
    }
    // The caller closes after its body.
    let finished: Vec<_> = of_type(&stream, "step_finished")
        .iter()
        .map(|e| e["step"].as_u64())
        .collect();
    assert_eq!(finished, [Some(1), Some(2), Some(0)]);
}

#[test]
fn an_included_step_is_an_event_with_its_own_source_and_line() {
    let dir = build_include_project(
        "events-include",
        &[
            (
                "features/target.feature",
                "tests/features/include/target.feature",
            ),
            (
                "features/caller.feature",
                "tests/features/include/caller.feature",
            ),
        ],
    );
    let events = dir.join("events.ndjson");
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config", "cfg.yaml", "--events"])
        .arg(&events)
        .current_dir(&dir)
        .output()
        .expect("run bddkit");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );

    // caller.feature sorts before target.feature, and runs as its own file.
    let stream = read_stream(&events);
    let caller = of_type(&stream, "step_started")
        .into_iter()
        .filter(|e| {
            e["file"]
                .as_str()
                .is_some_and(|f| f.ends_with("caller.feature"))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        caller.len(),
        3,
        "include, the included step, the assertion: {caller:?}"
    );
    let included = caller[1];
    assert_eq!(included["parent"], 0);
    assert_eq!(included["line"], 4, "its own line in the included file");
    assert!(
        included["source"]
            .as_str()
            .expect("source")
            .ends_with("target.feature")
    );
    assert_eq!(included["text"], "set variable \"userId\" to \"abc123\"");
    assert_eq!(caller[2].get("parent"), None, "back at the top level");
}

#[test]
fn the_stream_and_the_junit_report_agree_on_every_total() {
    let cfg = write_report_project("events-totals");
    std::fs::write(
        cfg.with_file_name("features/second.feature"),
        "Feature: second\n  Scenario: one\n    Given set variable \"ping\" to \"ok\"\n    Then variable \"ping\" should be equal to \"nope\"\n\n  Scenario: two\n    Given set variable \"ping\" to \"ok\"\n",
    )
    .expect("second feature");
    let junit = cfg.with_file_name("junit.xml");
    let (_, stream, _) = run_with_events(&cfg, &["--junit", junit.to_str().expect("UTF-8")]);

    let xml = std::fs::read_to_string(&junit).expect("junit");
    let package = sxd_document::parser::parse(&xml).expect("junit parses");
    let value = |xpath: &str| {
        sxd_xpath::evaluate_xpath(&package.as_document(), xpath)
            .expect("xpath")
            .string()
    };
    let last = stream.last().expect("lines");
    assert_eq!(last["files"].to_string(), value("count(//testsuite)"));
    assert_eq!(last["scenarios"].to_string(), value("count(//testcase)"));
    assert_eq!(last["failed"].to_string(), value("count(//failure)"));
    assert_eq!(
        (
            last["files"].as_u64(),
            last["scenarios"].as_u64(),
            last["failed"].as_u64()
        ),
        (Some(2), Some(4), Some(2))
    );
    for suite in of_type(&stream, "file_finished") {
        let file = suite["file"].as_str().expect("file");
        let name = file.rsplit('/').next().expect("name");
        assert_eq!(
            suite["failed"].to_string(),
            value(&format!(
                "string(//testsuite[contains(@name,'{name}')]/@failures)"
            )),
            "{suite}"
        );
    }
}

#[test]
fn a_run_without_events_prints_the_same_console_output() {
    let cfg = write_report_project("events-console");
    let events = cfg.with_file_name("events.ndjson");
    let normalised = |out: &std::process::Output| {
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let run_id = stdout
            .lines()
            .find_map(|line| line.strip_prefix("run "))
            .expect("the run id line")
            .to_string();
        (stdout.replace(&run_id, "<run>"), out.status.code())
    };
    let plain = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(&cfg)
        .output()
        .expect("run bddkit");
    let streamed = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(&cfg)
        .arg("--events")
        .arg(&events)
        .output()
        .expect("run bddkit");

    assert_eq!(normalised(&plain), normalised(&streamed));
}

#[test]
fn a_null_sentinel_in_a_failure_is_written_as_the_null_slot() {
    let cfg = write_report_project("events-null");
    let (_, stream, events) = run_with_events(&cfg, &[]);

    let failure = of_type(&stream, "scenario_finished")
        .into_iter()
        .find_map(|e| e["failure"].as_str())
        .expect("the failing scenario carries its failure");
    assert!(failure.contains("<<null>>"), "{failure}");
    let raw = std::fs::read(&events).expect("read");
    assert!(!raw.contains(&0), "no NUL byte reaches the stream");
    assert!(
        !String::from_utf8_lossy(&raw).contains("\\u0000"),
        "nor its JSON escape"
    );
}

#[test]
fn the_events_path_can_come_from_the_environment_and_the_flag_wins() {
    let cfg = write_report_project("events-env");
    let from_env = cfg.with_file_name("from-env.ndjson");
    let from_flag = cfg.with_file_name("from-flag.ndjson");

    Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(&cfg)
        .env("BDDKIT_EVENTS", &from_env)
        .output()
        .expect("run bddkit");
    assert_eq!(read_stream(&from_env)[0]["type"], "run_started");

    let _ = std::fs::remove_file(&from_env);
    Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(&cfg)
        .arg("--events")
        .arg(&from_flag)
        .env("BDDKIT_EVENTS", &from_env)
        .output()
        .expect("run bddkit");
    assert_eq!(read_stream(&from_flag)[0]["type"], "run_started");
    assert!(
        !from_env.exists(),
        "the flag wins; the variable is not opened"
    );

    // A templated `BDDKIT_EVENTS=` is "off", not a path to fail on.
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(&cfg)
        .env("BDDKIT_EVENTS", "")
        .output()
        .expect("run bddkit");
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The reader opens first and blocks until the run opens its end; end-of-file
/// would come early if the path were opened, closed and opened again.
#[cfg(unix)]
#[test]
fn events_can_be_written_to_a_fifo() {
    let cfg = write_report_project("events-fifo");
    let fifo = cfg.with_file_name("events.fifo");
    mkfifo(&fifo);

    let reader = {
        let fifo = fifo.clone();
        std::thread::spawn(move || std::fs::read_to_string(fifo).expect("read the fifo"))
    };
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(&cfg)
        .arg("--events")
        .arg(&fifo)
        .output()
        .expect("run bddkit");
    let text = reader.join().expect("reader");

    assert_eq!(out.status.code(), Some(1));
    let lines: Vec<Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).expect("one JSON object per line"))
        .collect();
    assert_eq!(lines[0]["type"], "run_started");
    assert_eq!(lines.last().expect("lines")["type"], "run_finished");
}

/// `/dev/full` opens for writing and fails every write with ENOSPC.
#[cfg(target_os = "linux")]
#[test]
fn a_stream_that_cannot_be_written_turns_the_exit_code_to_two() {
    let cfg = write_events_project(
        "full",
        &[(
            "a.feature",
            "Feature: a\n  Scenario: s\n    Given set variable \"x\" to \"1\"\n",
        )],
        None,
    );
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(&cfg)
        .args(["--events", "/dev/full"])
        .output()
        .expect("run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stdout}\n{stderr}");
    assert!(stderr.contains("cannot write the events file"), "{stderr}");
    assert!(
        stdout.contains("failed: 0"),
        "the run itself completed: {stdout}"
    );
}

/// A run held in a `sleep` is interrupted once its first step is on disk.
/// Whatever the files in flight emit after the signal, the last line of the
/// stream is the terminator, and there is exactly one.
#[cfg(unix)]
#[test]
fn an_interrupted_run_ends_its_stream_with_the_signal() {
    for (signal, code, name) in [
        ("INT", 130, "SIGINT"),
        ("TERM", 143, "SIGTERM"),
        ("HUP", 129, "SIGHUP"),
    ] {
        let feature = "Feature: held\n  Scenario: s\n    Given I sleep \"10\" seconds\n";
        let cfg = write_events_project(
            &format!("signal-{signal}"),
            &[("a.feature", feature), ("b.feature", feature)],
            None,
        );
        let events = cfg.with_file_name("events.ndjson");
        let mut child = Command::new(env!("CARGO_BIN_EXE_bddkit"))
            .args(["run", "--config"])
            .arg(&cfg)
            .arg("--events")
            .arg(&events)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn bddkit");

        // Both files are inside their sleep when the signal lands.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while std::fs::read_to_string(&events)
            .map_or(0, |text| text.matches("step_started").count())
            < 2
        {
            assert!(
                std::time::Instant::now() < deadline,
                "the steps never started"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let sent = Command::new("kill")
            .args([&format!("-{signal}"), &child.id().to_string()])
            .status()
            .expect("kill");
        assert!(sent.success());
        let status = child.wait().expect("wait");

        assert_eq!(status.code(), Some(code), "{name}");
        let stream = read_stream(&events);
        let last = stream.last().expect("lines");
        assert_eq!(last["type"], "run_finished", "{name}: {stream:?}");
        assert_eq!(last["signal"], name);
        assert_eq!(last["exit"], code);
        assert_eq!(of_type(&stream, "run_finished").len(), 1, "{name}");
    }
}

#[test]
fn doctor_reports_the_events_path() {
    let cfg = write_doctor_project(
        "doctor-events",
        "Feature: f\n  Scenario: s\n    Given set variable \"x\" to \"1\"\n",
        "",
    );
    let events = cfg.with_file_name("out/events.ndjson");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config"])
        .arg(&cfg)
        .arg("--events")
        .arg(&events)
        .output()
        .expect("run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("reports"), "{stdout}");
    assert!(stdout.contains("events.ndjson"), "{stdout}");
    assert_eq!(std::fs::read_to_string(&events).expect("created"), "");

    // A path that cannot be created is a failed row, exit 1.
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config"])
        .arg(&cfg)
        .arg("--events")
        .arg(cfg.join("events.ndjson"))
        .output()
        .expect("run bddkit");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("✗ reports"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// `run` refuses a malformed evidence variable, and `$BDDKIT_EVIDENCE`
/// without a stream; `doctor` must not be the more lenient of the two.
#[test]
fn doctor_fails_on_an_evidence_variable_run_would_refuse() {
    let cfg = write_doctor_project(
        "doctor-evidence",
        "Feature: f\n  Scenario: s\n    Given set variable \"x\" to \"1\"\n",
        "",
    );
    let doctor = |vars: &[(&str, &str)], stream: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_bddkit"));
        command
            .args(["doctor", "--config"])
            .arg(&cfg)
            .env_remove("BDDKIT_EVENTS")
            .env_remove("BDDKIT_EVIDENCE")
            .env_remove("BDDKIT_EVIDENCE_BUFFER")
            .envs(vars.iter().copied());
        if stream {
            command.arg("--events").arg(cfg.with_file_name("e.ndjson"));
        }
        let out = command.output().expect("run bddkit");
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
        )
    };
    for (vars, stream, says) in [
        (&[("BDDKIT_EVIDENCE", "bogus")][..], true, "BDDKIT_EVIDENCE"),
        (
            &[("BDDKIT_EVIDENCE", "failures")][..],
            false,
            "requires --events",
        ),
        (
            &[
                ("BDDKIT_EVIDENCE", "failures"),
                ("BDDKIT_EVIDENCE_BUFFER", "lots"),
            ][..],
            true,
            "BDDKIT_EVIDENCE_BUFFER",
        ),
    ] {
        let (code, stdout) = doctor(vars, stream);
        assert_eq!(code, Some(1), "{vars:?}: {stdout}");
        assert!(
            stdout.contains("✗ evidence") && stdout.contains(says),
            "{stdout}"
        );
    }
    // A buffer nothing would use is not read, as in `run`.
    let (code, stdout) = doctor(&[("BDDKIT_EVIDENCE_BUFFER", "lots")], true);
    assert_eq!(code, Some(0), "{stdout}");
    let (code, stdout) = doctor(&[("BDDKIT_EVIDENCE", "marked,failures")], true);
    assert_eq!(code, Some(0), "{stdout}");
    assert!(
        stdout.contains("marked, failures ($BDDKIT_EVIDENCE)"),
        "{stdout}"
    );
}

/// Opening a FIFO for writing blocks until a reader shows up, and there is
/// none: a doctor that opened it would hang here.
#[cfg(unix)]
#[test]
fn doctor_leaves_an_events_path_that_is_not_a_regular_file_unopened() {
    let cfg = write_doctor_project(
        "doctor-events-fifo",
        "Feature: f\n  Scenario: s\n    Given set variable \"x\" to \"1\"\n",
        "",
    );
    let fifo = cfg.with_file_name("events.fifo");
    mkfifo(&fifo);

    let mut child = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config"])
        .arg(&cfg)
        .arg("--events")
        .arg(&fifo)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("spawn bddkit");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while child.try_wait().expect("poll").is_none() {
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            panic!("doctor is blocked opening the FIFO");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let out = child.wait_with_output().expect("output");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(stdout.contains("left unopened"), "{stdout}");
}

fn mkfifo(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
    assert!(
        Command::new("mkfifo")
            .arg(path)
            .status()
            .expect("mkfifo")
            .success()
    );
}

/// `run` would exit 2 on a directory, so `doctor` must not call it fine.
#[test]
fn doctor_refuses_an_events_path_that_is_a_directory() {
    let cfg = write_doctor_project(
        "doctor-events-dir",
        "Feature: f\n  Scenario: s\n    Given set variable \"x\" to \"1\"\n",
        "",
    );
    let dir = cfg.with_file_name("a-directory");
    std::fs::create_dir_all(&dir).expect("mkdir");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config"])
        .arg(&cfg)
        .arg("--events")
        .arg(&dir)
        .output()
        .expect("run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("✗ reports"), "{stdout}");
}

/// A reader that holds the FIFO open and never reads stalls the writer once the
/// pipe's buffer is full. The run still completes — the channel is unbounded —
/// but its last step, waiting for the writer, must not make it unkillable.
#[cfg(unix)]
#[test]
fn a_second_signal_stops_a_run_whose_stream_reader_has_stalled() {
    use std::io::BufRead;

    let steps = "    Given set variable \"x\" to \"1\"\n".repeat(400);
    let cfg = write_events_project(
        "stalled",
        &[("a.feature", &format!("Feature: a\n  Scenario: s\n{steps}"))],
        None,
    );
    let fifo = cfg.with_file_name("events.fifo");
    mkfifo(&fifo);
    // `exec` keeps the pid: killing the child kills the sleep itself.
    let mut reader = Command::new("sh")
        .args(["-c", "exec sleep 120 < \"$0\""])
        .arg(&fifo)
        .spawn()
        .expect("spawn the stalled reader");
    let mut run = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(&cfg)
        .arg("--events")
        .arg(&fifo)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn bddkit");

    // The summary is printed before the stream is closed: the run is now in
    // the tail that waits for the writer.
    let stdout = run.stdout.take().expect("stdout");
    let summary = std::io::BufReader::new(stdout)
        .lines()
        .map_while(Result::ok)
        .any(|line| line.starts_with("files:"));
    assert!(summary, "the run never printed its summary");
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(
        run.try_wait().expect("poll").is_none(),
        "the stream is over 64 KiB, so the writer is stalled and the run waits for it"
    );

    // A first signal only says "still writing": the queued lines are not
    // thrown away on one Ctrl-C.
    let pid = run.id().to_string();
    let signal = |name: &str| {
        let sent = Command::new("kill")
            .args([name, &pid])
            .status()
            .expect("kill");
        assert!(sent.success());
    };
    signal("-TERM");
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(
        run.try_wait().expect("poll").is_none(),
        "one signal must not drop the stream still being written"
    );
    // A second one is "stop now", with the first one's code.
    signal("-INT");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let status = loop {
        if let Some(status) = run.try_wait().expect("poll") {
            break Some(status);
        }
        if std::time::Instant::now() > deadline {
            let _ = run.kill();
            break None;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let _ = reader.kill();
    let _ = reader.wait();
    let _ = run.wait();

    assert_eq!(
        status.and_then(|s| s.code()),
        Some(143),
        "a second signal must stop a run that is only waiting for its stream"
    );
}

/// The handlers are registered before `run <id>` is printed, so a signal sent
/// the moment that line is readable is queued even if the handler task has not
/// been polled yet — it must end the stream with the signal, never leave it
/// holding `run_started` alone. A race cannot be forced, so this repeats it:
/// before the handlers were registered up front, a SIGTERM landing in that
/// window killed the process outright.
#[cfg(unix)]
#[test]
fn a_signal_right_after_the_run_id_still_terminates_the_stream() {
    use std::io::BufRead;

    let cfg = write_events_project(
        "early-signal",
        &[(
            "a.feature",
            "Feature: a\n  Scenario: s\n    Given I sleep \"10\" seconds\n",
        )],
        None,
    );
    let events = cfg.with_file_name("events.ndjson");
    for attempt in 0..3 {
        let mut child = Command::new(env!("CARGO_BIN_EXE_bddkit"))
            .args(["run", "--config"])
            .arg(&cfg)
            .arg("--events")
            .arg(&events)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn bddkit");
        let mut first = String::new();
        std::io::BufReader::new(child.stdout.take().expect("stdout"))
            .read_line(&mut first)
            .expect("the run id line");
        assert!(first.starts_with("run "), "{first:?}");
        let sent = Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status()
            .expect("kill");
        assert!(sent.success());
        let status = child.wait().expect("wait");

        assert_eq!(status.code(), Some(143), "attempt {attempt}");
        let stream = read_stream(&events);
        let last = stream.last().expect("a stream");
        assert_eq!(
            last["type"], "run_finished",
            "attempt {attempt}: {stream:?}"
        );
        assert_eq!(last["signal"], "SIGTERM", "attempt {attempt}");
    }
}

/// `files` feature files of `scenarios` scenarios of `steps` steps each, none
/// of which touches the network: the cost is the runner and the stream.
fn load_features(files: usize, scenarios: usize, steps: usize) -> Vec<(String, String)> {
    let body = "    Given set variable \"x\" to \"1\"\n".repeat(steps);
    (0..files)
        .map(|file| {
            let mut text = format!("Feature: load {file}\n");
            for scenario in 0..scenarios {
                text.push_str(&format!("  Scenario: s{scenario}\n{body}"));
            }
            (format!("f{file:04}.feature"), text)
        })
        .collect()
}

/// Four workers, not the default eight: a load test must not take the CPU the
/// timing-sensitive tests running beside it are counting on.
fn write_load_project(name: &str, features: &[(String, String)]) -> std::path::PathBuf {
    let pairs: Vec<(&str, &str)> = features
        .iter()
        .map(|(file, text)| (file.as_str(), text.as_str()))
        .collect();
    let cfg = write_events_project(name, &pairs, None);
    let mut config = std::fs::read_to_string(&cfg).expect("read config");
    config.push_str("concurrency: 4\n");
    std::fs::write(&cfg, config).expect("write config");
    cfg
}

/// What holds for any stream, even one cut short: every line parsed (the
/// reader did that), `seq` has no gap and `t` never goes back.
fn assert_stream_is_ordered(stream: &[Value]) {
    for (index, event) in stream.iter().enumerate() {
        assert_eq!(event["seq"], index, "a gap or a repeat in seq: {event}");
        if index > 0 {
            assert!(
                event["t"].as_u64() >= stream[index - 1]["t"].as_u64(),
                "t goes back: {event}"
            );
        }
    }
}

/// A stream that ended: the only `run_finished` is the last line.
fn assert_stream_is_sound(stream: &[Value]) {
    assert_stream_is_ordered(stream);
    assert_eq!(of_type(stream, "run_finished").len(), 1);
    assert_eq!(stream.last().expect("lines")["type"], "run_finished");
}

/// 40 files x 2 scenarios x 20 steps, four at a time: enough volume for the
/// ordering to mean something, a fraction of a second to run. That the files
/// really overlap is the barrier test's job, which proves it without timing.
#[test]
fn a_wide_run_streams_every_event_in_order() {
    let (files, scenarios, steps) = (40, 2, 20);
    let cfg = write_load_project("load", &load_features(files, scenarios, steps));
    let (out, stream, _) = run_with_events(&cfg, &[]);

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert_stream_is_sound(&stream);
    assert!(
        stdout.contains(&format!(
            "files: {files}, scenarios: {}, failed: 0",
            files * scenarios
        )),
        "{stdout}"
    );
    let last = stream.last().expect("lines");
    assert_eq!(
        (
            last["files"].as_u64(),
            last["scenarios"].as_u64(),
            last["failed"].as_u64()
        ),
        (
            Some(files as u64),
            Some((files * scenarios) as u64),
            Some(0)
        )
    );
    let total_steps = files * scenarios * steps;
    assert_eq!(of_type(&stream, "step_started").len(), total_steps);
    assert_eq!(of_type(&stream, "step_finished").len(), total_steps);

    // One file's lines are in causal order, whatever the others were doing.
    let mut by_file: std::collections::HashMap<&str, Vec<&Value>> =
        std::collections::HashMap::new();
    for event in &stream {
        if let Some(file) = event["file"].as_str() {
            by_file.entry(file).or_default().push(event);
        }
    }
    assert_eq!(by_file.len(), files);
    for (file, events) in &by_file {
        assert_eq!(events[0]["type"], "file_started", "{file}");
        assert_eq!(
            events.last().expect("events")["type"],
            "file_finished",
            "{file}"
        );
        let (mut in_scenario, mut open) = (false, 0_i32);
        for event in events {
            match event["type"].as_str().expect("type") {
                "scenario_started" => {
                    assert!(!in_scenario, "{file}: a scenario inside a scenario");
                    in_scenario = true;
                }
                "step_started" => {
                    assert!(in_scenario, "{file}: a step outside a scenario");
                    open += 1;
                }
                "step_finished" => {
                    assert!(open > 0, "{file}: a step closed twice");
                    open -= 1;
                }
                "scenario_finished" => {
                    assert!(in_scenario && open == 0, "{file}: closed with a step open");
                    in_scenario = false;
                }
                _ => {}
            }
        }
    }
}

/// An anchor file holds the run in a sleep, so when the first busy file has
/// finished the signal always finds the handler first to the claim and other
/// files still emitting. Nothing here depends on how fast the machine is.
#[cfg(unix)]
#[test]
fn interrupting_a_busy_run_leaves_a_terminated_stream() {
    let mut features = load_features(40, 1, 50);
    features.push((
        "anchor.feature".into(),
        "@priority(1)\nFeature: anchor\n  Scenario: hold\n    Given I sleep \"10\" seconds\n"
            .into(),
    ));
    let cfg = write_load_project("load-signal", &features);
    let events = cfg.with_file_name("events.ndjson");
    let mut child = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(&cfg)
        .arg("--events")
        .arg(&events)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn bddkit");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !std::fs::read_to_string(&events).is_ok_and(|text| text.contains("file_finished")) {
        assert!(
            std::time::Instant::now() < deadline,
            "no file ever finished"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let sent = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .expect("kill");
    assert!(sent.success());
    let status = child.wait().expect("wait");

    assert_eq!(status.code(), Some(143));
    let stream = read_stream(&events);
    assert_stream_is_sound(&stream);
    let last = stream.last().expect("lines");
    assert_eq!(
        (last["signal"].as_str(), last["exit"].as_u64()),
        (Some("SIGTERM"), Some(143))
    );
    assert_eq!(
        last["files"].as_u64(),
        Some(of_type(&stream, "file_finished").len() as u64),
        "the totals are those of the files written before the terminator"
    );
    assert!(
        of_type(&stream, "file_started").len() > of_type(&stream, "file_finished").len(),
        "the anchor was still in flight"
    );
}

// ---- privacy: the layers that decide what is masked (spec §7.7, §7.9) ----

/// A project whose config carries `privacy_yaml` as extra top-level text, run
/// with an events stream, `args` and `envs`.
fn run_privacy(
    name: &str,
    privacy_yaml: &str,
    args: &[&str],
    envs: &[(&str, &str)],
) -> (std::process::Output, Vec<Value>, std::path::PathBuf) {
    let cfg = write_report_project(name);
    let mut text = std::fs::read_to_string(&cfg).expect("read config");
    text.push_str(privacy_yaml);
    std::fs::write(&cfg, text).expect("write config");
    let events = cfg.with_file_name("events.ndjson");
    let mut command = Command::new(env!("CARGO_BIN_EXE_bddkit"));
    command
        .args(["run", "--config"])
        .arg(&cfg)
        .arg("--events")
        .arg(&events)
        .args(args);
    for key in [
        "BDDKIT_PRIVACY_WORDS",
        "BDDKIT_PRIVACY_WORDS_ONLY",
        "BDDKIT_PRIVACY_FIELDS",
        "BDDKIT_PRIVACY_FIELDS_ONLY",
        "BDDKIT_PRIVACY_MIN_LENGTH",
        "BDDKIT_PRIVACY_REVEAL",
        "BDDKIT_ARTIFACTS_DIR",
    ] {
        command.env_remove(key);
    }
    let out = command
        .envs(envs.iter().copied())
        .output()
        .expect("run bddkit");
    let stream = read_stream(&events);
    (out, stream, cfg)
}

#[test]
fn run_started_records_the_privacy_vocabulary_in_force() {
    let (_, stream, _) = run_privacy("privacy-default", "", &[], &[]);
    assert_eq!(
        stream[0]["privacy"],
        serde_json::json!({
            "masking": true,
            "words": ["secret"],
            "fields": ["password", "secret", "token"],
            "min_length": 7,
        })
    );
}

#[test]
fn the_flag_beats_the_environment_beats_the_config_for_min_length() {
    let config = "privacy:\n  min_length: 3\n";
    let min_length = |args: &[&str], envs: &[(&str, &str)]| {
        let (_, stream, _) = run_privacy("privacy-min", config, args, envs);
        stream[0]["privacy"]["min_length"].clone()
    };
    assert_eq!(min_length(&[], &[]), 3);
    assert_eq!(min_length(&[], &[("BDDKIT_PRIVACY_MIN_LENGTH", "5")]), 5);
    let both = [("BDDKIT_PRIVACY_MIN_LENGTH", "5")];
    assert_eq!(min_length(&["--privacy-min-length", "9"], &both), 9);
}

#[test]
fn the_lists_extend_or_replace_across_the_layers() {
    let (_, stream, _) = run_privacy(
        "privacy-lists",
        "privacy:\n  fields: [otp]\n  words_only: [gdpr]\n",
        &["--privacy-fields", "pin,cvv"],
        &[("BDDKIT_PRIVACY_FIELDS_ONLY", "iban")],
    );
    // The environment resets the list below it, the flag then adds to it.
    assert_eq!(
        stream[0]["privacy"]["fields"],
        serde_json::json!(["iban", "pin", "cvv"])
    );
    assert_eq!(stream[0]["privacy"]["words"], serde_json::json!(["gdpr"]));
}

#[test]
fn reveal_is_announced_on_stderr_and_recorded_in_the_stream() {
    let (out, stream, _) = run_privacy("privacy-reveal", "", &["--reveal"], &[]);
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("masking is off"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(stream[0]["privacy"]["masking"], false);
}

#[test]
fn the_environment_overrides_a_reveal_committed_in_the_config() {
    let config = "privacy:\n  reveal: true\n";
    let (out, stream, _) = run_privacy("privacy-committed", config, &[], &[]);
    assert_eq!(stream[0]["privacy"]["masking"], false);
    assert!(String::from_utf8_lossy(&out.stderr).contains("masking is off"));

    let ci = [("BDDKIT_PRIVACY_REVEAL", "false")];
    let (out, stream, _) = run_privacy("privacy-ci", config, &[], &ci);
    assert_eq!(stream[0]["privacy"]["masking"], true);
    assert!(!String::from_utf8_lossy(&out.stderr).contains("masking is off"));
}

#[test]
fn a_bad_privacy_value_is_exit_two_in_run_and_a_failed_row_in_doctor() {
    let bad = [("BDDKIT_PRIVACY_REVEAL", "yes")];
    let (out, _, cfg) = run_privacy("privacy-bad", "", &[], &bad);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("BDDKIT_PRIVACY_REVEAL"));

    let doctor = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config"])
        .arg(&cfg)
        .envs(bad)
        .output()
        .expect("run doctor");
    let stdout = String::from_utf8_lossy(&doctor.stdout);
    assert_eq!(doctor.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("BDDKIT_PRIVACY_REVEAL"), "{stdout}");
}

#[test]
fn an_unknown_key_under_privacy_is_refused() {
    let typo = "privacy:\n  wrods: [x]\n";
    let (out, _, cfg) = run_privacy("privacy-typo", typo, &[], &[]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("wrods"));

    let doctor = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config"])
        .arg(&cfg)
        .output()
        .expect("run doctor");
    let stdout = String::from_utf8_lossy(&doctor.stdout);
    assert_eq!(doctor.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("wrods"), "{stdout}");
}

// ---- privacy: what the failure text shows (spec §7.8) ----

/// A one-file project; `top_extra` is appended to the config.
fn masking_project(name: &str, feature: &str, top_extra: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("bddkit-{name}-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(dir.join("features/mask.feature"), feature).expect("write feature");
    let config = dir.join("cfg.yaml");
    std::fs::write(
        &config,
        format!(
            "paths: [{}]\nresources:\n  api: {{}}\n{top_extra}",
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("write config");
    config
}

/// Runs the project and returns (exit code, stdout, stderr).
fn run_masking(
    cfg: &std::path::Path,
    args: &[&str],
    envs: &[(&str, &str)],
) -> (i32, String, String) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_bddkit"));
    command.args(["run", "--config"]).arg(cfg).args(args);
    for key in [
        "BDDKIT_PRIVACY_WORDS",
        "BDDKIT_PRIVACY_WORDS_ONLY",
        "BDDKIT_PRIVACY_FIELDS",
        "BDDKIT_PRIVACY_FIELDS_ONLY",
        "BDDKIT_PRIVACY_MIN_LENGTH",
        "BDDKIT_PRIVACY_REVEAL",
    ] {
        command.env_remove(key);
    }
    let out = command
        .envs(envs.iter().copied())
        .output()
        .expect("run bddkit");
    (
        out.status.code().expect("exit code"),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn the_junit_report_carries_the_masked_variable_text() {
    let feature = variable_feature(
        &[("access_secret", "junit-secret-value-3")],
        "access_secret",
    );
    let cfg = masking_project("mask-junit-var", &feature, "");
    let junit = cfg.with_file_name("junit.xml");
    let (code, _, _) = run_masking(&cfg, &["--junit", junit.to_str().expect("utf-8")], &[]);
    assert_eq!(code, 1);
    let xml = std::fs::read_to_string(&junit).expect("junit report");
    assert!(xml.contains("actual:   ***"), "{xml}");
    assert!(!xml.contains("junit-secret-value-3"), "{xml}");
}

#[test]
fn reveal_on_a_variable_turns_masking_off_and_says_so() {
    let feature = variable_feature(
        &[("access_secret", "reveal-secret-value-9")],
        "access_secret",
    );
    let cfg = masking_project("mask-reveal-var", &feature, "");
    let (_, stdout, stderr) = run_masking(&cfg, &["--reveal"], &[]);
    assert!(stdout.contains("reveal-secret-value-9"), "{stdout}");
    assert!(stderr.contains("masking is off"), "{stderr}");
}

fn variable_feature(set: &[(&str, &str)], check: &str) -> String {
    let mut text = String::from("Feature: masking\n  Scenario: a failing assertion\n");
    for (name, value) in set {
        text.push_str(&format!(
            "    Given set variable \"{name}\" to \"{value}\"\n"
        ));
    }
    text.push_str(&format!(
        "    Then variable \"{check}\" should be equal to \"nothing\"\n"
    ));
    text
}

#[test]
fn a_variable_with_a_privacy_word_is_masked_wherever_its_value_appears() {
    for name in ["access_token_secret", "secret_reg_verifier"] {
        let feature = variable_feature(&[(name, "verifier-value-77")], name);
        let cfg = masking_project("mask-var", &feature, "");
        let (code, stdout, _) = run_masking(&cfg, &[], &[]);
        assert_eq!(code, 1, "{stdout}");
        assert!(!stdout.contains("verifier-value-77"), "{name}: {stdout}");
        assert!(stdout.contains("***"), "{name}: {stdout}");
    }
}

/// `as "reg_secret"` plus an exported `verifier` is what makes this shape.
#[test]
fn a_privacy_word_in_the_middle_of_a_name_still_masks() {
    let name = "reg_secret_verifier";
    let feature = variable_feature(&[(name, "middle-value-4321")], name);
    let cfg = masking_project("mask-var-middle", &feature, "");
    let (code, stdout, _) = run_masking(&cfg, &[], &[]);
    assert_eq!(code, 1, "{stdout}");
    assert!(!stdout.contains("middle-value-4321"), "{stdout}");
    assert!(stdout.contains("***"), "{stdout}");
}

#[test]
fn a_name_that_only_contains_a_privacy_word_does_not_mask() {
    for name in ["secretary_id", "sessionSecret"] {
        let feature = variable_feature(&[(name, "plain-value-88")], name);
        let cfg = masking_project("mask-var-no", &feature, "");
        let (_, stdout, _) = run_masking(&cfg, &[], &[]);
        assert!(stdout.contains("plain-value-88"), "{name}: {stdout}");
    }
}

#[test]
fn privacy_words_only_replaces_the_default_word() {
    let feature = "Feature: masking
  Scenario: the new word
    Given set variable \"gdpr_note\" to \"gdpr-value-12345\"
    Then variable \"gdpr_note\" should be equal to \"nothing\"

  Scenario: the old word
    Given set variable \"secret_note\" to \"secret-value-1234\"
    Then variable \"secret_note\" should be equal to \"nothing\"
";
    let cfg = masking_project(
        "mask-words-only",
        feature,
        "privacy:\n  words_only: [gdpr]\n",
    );
    let (_, stdout, _) = run_masking(&cfg, &[], &[]);
    assert!(!stdout.contains("gdpr-value-12345"), "{stdout}");
    assert!(stdout.contains("secret-value-1234"), "{stdout}");
    // Without the key the default word applies: `secret_` masks, `gdpr_` does not.
    let cfg = masking_project("mask-words-default", feature, "");
    let (_, stdout, _) = run_masking(&cfg, &[], &[]);
    assert!(stdout.contains("gdpr-value-12345"), "{stdout}");
    assert!(!stdout.contains("secret-value-1234"), "{stdout}");
}

#[test]
fn a_short_secret_is_not_added_to_the_value_filter() {
    let feature = variable_feature(&[("secret_pin", "tiny")], "secret_pin");
    let cfg = masking_project("mask-short", &feature, "");
    let (_, stdout, _) = run_masking(&cfg, &[], &[]);
    assert!(stdout.contains("tiny"), "{stdout}");
    // The same value is masked once the minimum length allows it.
    let (_, stdout, _) = run_masking(&cfg, &["--privacy-min-length", "4"], &[]);
    assert!(!stdout.contains("tiny"), "{stdout}");
}

#[test]
fn a_longer_secret_is_masked_before_one_it_contains() {
    let feature = variable_feature(
        &[
            ("secret_short", "abcdefgh"),
            ("secret_long", "abcdefgh-and-more-1"),
        ],
        "secret_long",
    );
    let cfg = masking_project("mask-longer", &feature, "");
    let (_, stdout, _) = run_masking(&cfg, &[], &[]);
    assert!(!stdout.contains("and-more"), "{stdout}");
    assert!(!stdout.contains("abcdefgh"), "{stdout}");
}

/// An `I include` swaps the whole variable stack, but not what the file has
/// learned to hide: a secret saved inside the included scenario is still masked
/// when the caller fails later.
#[test]
fn a_secret_saved_inside_an_include_is_masked_in_the_callers_failure() {
    let dir = std::env::temp_dir().join(format!("bddkit-mask-include-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/target.feature"),
        "Feature: setup\n  @exports(leak)\n  Scenario: signs in\n    Given set variable \"inner_secret\" to \"included-value-1\"\n    And set variable \"leak\" to \"included-value-1\"\n",
    )
    .expect("write target");
    std::fs::write(
        dir.join("features/caller.feature"),
        "Feature: caller\n  Scenario: fails afterwards\n    Given I include \"target.feature\"\n    Then variable \"leak\" should be equal to \"nothing\"\n",
    )
    .expect("write caller");
    std::fs::write(
        dir.join("cfg.yaml"),
        "paths: [features]\nresources:\n  api: {}\n",
    )
    .expect("write config");
    let (code, stdout, _) = run_bddkit_in("run", &dir);
    assert_eq!(code, Some(1), "{stdout}");
    assert!(!stdout.contains("included-value-1"), "{stdout}");
}

// ---- privacy: output with no World around it (spec §7.6, §7.8) ----

#[test]
fn doctor_says_so_on_stderr_when_masking_is_off() {
    let cfg = write_doctor_project("mask-doctor-reveal", "", "");
    for (args, envs) in [
        (&["--reveal"][..], &[][..]),
        (&[][..], &[("BDDKIT_PRIVACY_REVEAL", "1")][..]),
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
            .args(["doctor", "--config"])
            .arg(&cfg)
            .args(args)
            .envs(envs.iter().copied())
            .output()
            .expect("run doctor");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("masking is off"), "{stderr}");
    }
}

#[test]
fn the_event_stream_carries_the_masked_failure_text() {
    let feature = variable_feature(&[("secret_note", "stream-value-1234")], "secret_note");
    let cfg = masking_project("mask-stream", &feature, "");
    let events = cfg.with_file_name("events.ndjson");
    let (code, _, _) = run_masking(&cfg, &["--events", events.to_str().expect("utf-8")], &[]);
    assert_eq!(code, 1);
    let failure = of_type(&read_stream(&events), "scenario_finished")[0]["failure"]
        .as_str()
        .expect("failure text")
        .to_string();
    assert!(failure.contains("***"), "{failure}");
    assert!(!failure.contains("stream-value-1234"), "{failure}");
}

// ---- privacy: the `secret` parameter type (spec §7.2) ----

const AES_LITERAL: &str = r#"Feature: masking
  Scenario: encrypted with a literal key
    Given I encrypt "555555" with AES using key "aes-literal-key-1" as "c"
    Then variable "c" should be equal to "nothing"
"#;

#[test]
fn a_literal_secret_is_masked_in_the_step_text_of_a_report() {
    let cfg = masking_project("mask-literal", AES_LITERAL, "");
    let junit = cfg.with_file_name("junit.xml");
    let events = cfg.with_file_name("events.ndjson");
    let args = [
        "--junit",
        junit.to_str().expect("utf-8"),
        "--events",
        events.to_str().expect("utf-8"),
    ];
    let (code, stdout, _) = run_masking(&cfg, &args, &[]);
    assert_eq!(code, 1, "{stdout}");
    let xml = std::fs::read_to_string(&junit).expect("junit report");
    assert!(xml.contains("using key &quot;***&quot;"), "{xml}");
    assert!(!xml.contains("aes-literal-key-1"), "{xml}");
    let stream = std::fs::read_to_string(&events).expect("stream");
    assert!(!stream.contains("aes-literal-key-1"), "{stream}");
    assert!(stream.contains(r#"using key \"***\""#), "{stream}");
}

#[test]
fn a_secret_parameter_is_masked_in_the_failure_dump() {
    // The failing step is the one that takes the secret.
    let feature = r#"Feature: masking
  Scenario: a key of the wrong size
    Given I encrypt "payload" with AES using key "short-literal-key" as "c"
"#;
    let cfg = masking_project("mask-aes", feature, "");
    let (code, stdout, _) = run_masking(&cfg, &[], &[]);
    assert_eq!(code, 1, "{stdout}");
    assert!(stdout.contains(r#"using key "***" as "c""#), "{stdout}");
    assert!(!stdout.contains("short-literal-key"), "{stdout}");
}

/// One parser serves builtin declarations and macro templates, so a macro's
/// `{pw:secret}` marks its argument without any other change.
#[test]
fn a_macro_template_can_declare_a_secret_parameter() {
    let dir = std::env::temp_dir().join(format!("bddkit-mask-macro-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/m.feature"),
        "Feature: f\n  Scenario: s\n    Given I log in as \"ann\" with password \"macro-literal-pw1\"\n",
    )
    .expect("write feature");
    std::fs::write(
        dir.join("macros.yaml"),
        "- step: 'I log in as \"{user}\" with password \"{pw:secret}\"'\n  do:\n    - variable \"pw\" should be equal to \"nothing\"\n",
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
                .replace('\\', "/"),
        ),
    )
    .expect("write config");
    let (code, stdout, _) = run_masking(&dir.join("cfg.yaml"), &[], &[]);
    assert_eq!(code, 1, "{stdout}");
    assert!(stdout.contains(r#"with password "***""#), "{stdout}");
    assert!(!stdout.contains("macro-literal-pw1"), "{stdout}");
}

/// A literal shorter than `min_length` never joins the set, so only the
/// step text itself can hide it: the console, the report and the stream must
/// each show `***`, and never the value.
#[test]
fn a_short_literal_secret_is_hidden_by_position_everywhere() {
    let feature = r#"Feature: masking
  Scenario: a short key
    Given I encrypt "payload" with AES using key "q7z" as "c"
"#;
    let cfg = masking_project("mask-short-literal", feature, "");
    let junit = cfg.with_file_name("junit.xml");
    let events = cfg.with_file_name("events.ndjson");
    let args = [
        "--junit",
        junit.to_str().expect("utf-8"),
        "--events",
        events.to_str().expect("utf-8"),
    ];
    let (code, stdout, _) = run_masking(&cfg, &args, &[]);
    assert_eq!(code, 1, "{stdout}");
    let xml = std::fs::read_to_string(&junit).expect("junit report");
    let stream = std::fs::read_to_string(&events).expect("stream");
    for (place, text) in [("console", &stdout), ("junit", &xml), ("stream", &stream)] {
        assert!(!text.contains("q7z"), "{place}: {text}");
        assert!(text.contains("***"), "{place}: {text}");
    }
}

#[test]
fn the_debug_line_of_a_macro_call_hides_a_short_literal_secret() {
    let dir = std::env::temp_dir().join(format!("bddkit-mask-macro-debug-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/m.feature"),
        "Feature: f\n  Scenario: s\n    Given I am in debug mode\n    And I log in as \"ann\" with password \"q7z\"\n",
    )
    .expect("write feature");
    std::fs::write(
        dir.join("macros.yaml"),
        "- step: 'I log in as \"{user}\" with password \"{pw:secret}\"'\n  do:\n    - set variable \"user_seen\" to \"<<user>>\"\n",
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
                .replace('\\', "/"),
        ),
    )
    .expect("write config");
    let (code, _, stderr) = run_masking(&dir.join("cfg.yaml"), &[], &[]);
    assert_eq!(code, 0, "{stderr}");
    assert!(stderr.contains("macro"), "{stderr}");
    assert!(!stderr.contains("q7z"), "{stderr}");
}

// ---- privacy: sensitive fields found by the name the system gave them (spec §7.5) ----

#[test]
fn doctor_lists_sensitive_looking_names_without_failing() {
    let feature = r#"Feature: names
  Scenario: s
    Given set variable "session_token" to "x"
    And set variable "token_secret" to "y"
    And set variable "primary_key" to "z"
    And set variable "monkey" to "w"
    Then variable "session_token" should be equal to "<<session_token>>"
    And variable "token_secret" should be equal to "<<token_secret>>"
    And variable "primary_key" should be equal to "<<primary_key>>"
    And variable "monkey" should be equal to "<<monkey>>"
"#;
    let cfg = masking_project("mask-doctor-hint", feature, "");
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config"])
        .arg(&cfg)
        .output()
        .expect("run doctor");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "never a failure: {stdout}");
    let hint = stdout
        .split_once("look sensitive")
        .unwrap_or_else(|| panic!("no hint in: {stdout}"))
        .1;
    assert!(
        hint.contains("session_token") && hint.contains("primary_key"),
        "{hint}"
    );
    // A privacy word is in `token_secret`; `monkey` has no word of its own.
    assert!(
        !hint.contains("token_secret") && !hint.contains("monkey"),
        "{hint}"
    );
}

#[test]
fn doctor_hint_skips_a_secret_macro_parameter_and_says_where_it_found_the_rest() {
    let feature = "Feature: f\n  Scenario: s\n    Given I log in as \"ann\" with password \"x\"\n    And I log in plain as \"ann\" with password \"x\"\n";
    let cfg = masking_project("mask-doctor-macro", feature, "");
    let dir = cfg.parent().expect("project dir").to_path_buf();
    let macros = dir.join("macros.yaml");
    std::fs::write(
        &macros,
        r#"- step: 'I log in as "{user}" with password "{password:secret}"'
  do:
    - set variable "body" to "<<user>>:<<password>>"
- step: 'I log in plain as "{user}" with password "{password}"'
  do:
    - set variable "body" to "<<user>>:<<password>>"
"#,
    )
    .expect("write macros");
    let mut config = std::fs::read_to_string(&cfg).expect("config");
    config.push_str(&format!("macro_paths: [{}]\n", macros.display()));
    std::fs::write(&cfg, config).expect("write config");
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config"])
        .arg(&cfg)
        .output()
        .expect("run doctor");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout
        .lines()
        .find(|line| line.contains("password —"))
        .unwrap_or_else(|| panic!("no located hint in: {stdout}"));
    assert!(line.contains("macros.yaml:4"), "{line}");
    assert!(!line.contains("macros.yaml:1"), "{line}");
}

/// `Show all variables` prints a name and a value, so the name decides: a value
/// too short to join the set is still hidden by the privacy word in its name.
#[test]
fn show_variables_hides_a_short_value_by_its_variables_name() {
    let feature = r#"Feature: masking
  Scenario: show
    Given set variable "secret_pin" to "q8z"
    And set variable "plain_note" to "kept-visible"
    And I am in debug mode
    Then Show all variables
    And Show "secret_pin" variable
"#;
    let cfg = masking_project("mask-show", feature, "");
    let (code, _, stderr) = run_masking(&cfg, &[], &[]);
    assert_eq!(code, 0, "{stderr}");
    assert!(stderr.contains("secret_pin: ***"), "{stderr}");
    assert!(stderr.contains("plain_note: kept-visible"), "{stderr}");
    assert!(!stderr.contains("q8z"), "{stderr}");
}

// ---- privacy: leaks found in review ----

/// `--reveal` turns masking off, the step text of a `secret` parameter included.
#[test]
fn reveal_also_shows_a_literal_secret_parameter_in_step_text() {
    let feature = r#"Feature: masking
  Scenario: a key of the wrong size
    Given I encrypt "payload" with AES using key "short-literal-key" as "c"
"#;
    let cfg = masking_project("mask-reveal-literal", feature, "");
    let (_, stdout, _) = run_masking(&cfg, &["--reveal"], &[]);
    assert!(
        stdout.contains(r#"using key "short-literal-key""#),
        "{stdout}"
    );
}

// ---------------------------------------------------------------------------
// `run --events --evidence`
// ---------------------------------------------------------------------------

/// One scenario of `steps`, run with the evidence on; the stream it wrote.
fn evidence_run(name: &str, steps: &str, extra: &[&str]) -> Vec<Value> {
    let cfg = write_evidence_project(
        name,
        &[("e.feature", &format!("Feature: e\n  Scenario: s\n{steps}"))],
        None,
    );
    let mut args = vec!["--evidence"];
    args.extend_from_slice(extra);
    run_with_events(&cfg, &args).1
}

fn step_text<'a>(stream: &'a [Value], text: &str) -> &'a Value {
    of_type(stream, "step_started")
        .into_iter()
        .find(|e| e["text"].as_str().is_some_and(|t| t.contains(text)))
        .unwrap_or_else(|| panic!("no step {text:?} in {stream:?}"))
}

#[test]
fn evidence_requires_events() {
    let cfg = write_events_project("evidence-alone", &[], None);
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--evidence", "--config"])
        .arg(&cfg)
        .env_remove("BDDKIT_EVENTS")
        .output()
        .expect("run bddkit");
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--evidence requires --events"), "{stderr}");

    // The variable counts as `--events`; the check is not clap's `requires`.
    let events = cfg.with_file_name("from-env.ndjson");
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--evidence", "--config"])
        .arg(&cfg)
        .env("BDDKIT_EVENTS", &events)
        .output()
        .expect("run bddkit");
    assert_eq!(out.status.code(), Some(2), "no feature files");
    assert!(
        !String::from_utf8_lossy(&out.stderr).contains("requires --events"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn without_evidence_the_stream_holds_only_lifecycle_events() {
    let cfg = write_evidence_project(
        "no-evidence",
        &[(
            "e.feature",
            "Feature: e\n  Scenario: s\n    Given set variable \"a\" to \"<<unique(token)>>\"\n    Given I echo one piece of evidence\n    Then variable \"a\" should not be equal to \"\"\n",
        )],
        None,
    );
    let (_, stream, _) = run_with_events(&cfg, &[]);

    let lifecycle = [
        "run_started",
        "file_started",
        "scenario_started",
        "step_started",
        "step_finished",
        "step_skipped",
        "scenario_finished",
        "file_finished",
        "run_finished",
    ];
    for event in &stream {
        assert!(
            lifecycle.contains(&event["type"].as_str().expect("type")),
            "{event}"
        );
        assert!(event.get("vars").is_none(), "{event}");
    }
    assert_eq!(stream[0]["evidence"], false);
}

#[test]
fn a_step_reports_its_resolved_slots_beside_the_raw_text() {
    let stream = evidence_run(
        "evidence-vars",
        "    Given set variable \"who\" to \"ann\"\n    And set variable \"greeting\" to \"hi <<who>>\"\n",
        &[],
    );
    assert_eq!(stream[0]["evidence"], true);
    let step = step_text(&stream, "\"greeting\"");
    assert_eq!(step["text"], "set variable \"greeting\" to \"hi <<who>>\"");
    assert_eq!(step["vars"], json!([["who", "ann"]]));
    assert!(
        step_text(&stream, "\"who\"").get("vars").is_none(),
        "no slot, no key"
    );
}

#[test]
fn a_repeated_slot_appears_once_per_occurrence() {
    let stream = evidence_run(
        "evidence-repeated",
        "    Given set variable \"pair\" to \"<<unique(token)>>|<<unique(token)>>\"\n    Then variable \"pair\" should not be equal to \"x\"\n",
        &[],
    );
    let vars = step_text(&stream, "\"pair\" to")["vars"]
        .as_array()
        .expect("vars")
        .clone();
    assert_eq!(vars.len(), 2);
    assert_eq!(vars[0][0], "unique(token)");
    assert_eq!(vars[1][0], "unique(token)");
    assert_ne!(vars[0][1], vars[1][1], "a new value each time");
}

#[test]
fn a_null_slot_is_reported_as_json_null() {
    let stream = evidence_run(
        "evidence-null",
        "    Given set variable \"n\" to \"<<null>>\"\n",
        &[],
    );
    assert_eq!(step_text(&stream, "\"n\"")["vars"], json!([["null", null]]));
}

#[test]
fn a_secret_slot_is_masked_in_vars() {
    let stream = evidence_run(
        "evidence-secret-slot",
        // `ab` is shorter than `min_length`: only position can hide it.
        "    Given set variable \"my_secret\" to \"ab\"\n    And set variable \"copy\" to \"<<my_secret>>\"\n    And set variable \"k\" to \"long-key-value-1\"\n    And I encrypt \"<<k>>\" with AES using key \"<<k>>\" as \"c\"\n",
        &[],
    );
    assert_eq!(
        step_text(&stream, "\"copy\"")["vars"],
        json!([["my_secret", "***"]]),
        "a sensitive name"
    );
    assert_eq!(
        step_text(&stream, "AES")["vars"],
        json!([["k", "***"], ["k", "***"]]),
        "a secret parameter, and a value another parameter also quotes"
    );
}

#[test]
fn evidence_inside_a_macro_attaches_to_the_innermost_step() {
    let cfg = write_evidence_project(
        "evidence-macro",
        &[(
            "m.feature",
            "Feature: m\n  Scenario: s\n    Given I ping the service\n",
        )],
        Some("- step: 'I ping the service'\n  do:\n    - I echo one piece of evidence\n"),
    );
    let (_, stream, _) = run_with_events(&cfg, &["--evidence"]);

    let evidence = of_type(&stream, "evidence");
    assert_eq!(evidence.len(), 1, "{stream:?}");
    assert_eq!(evidence[0]["step"], 1, "the body step, not the caller (0)");
}

// ---------------------------------------------------------------------------
// `--evidence <sources>`: marked steps and failed scenarios
// ---------------------------------------------------------------------------

/// `features` run with `--events` and then `args`.
fn modes_run(
    name: &str,
    features: &[(&str, &str)],
    macros: Option<&str>,
    args: &[&str],
) -> Vec<Value> {
    let cfg = write_evidence_project(name, features, macros);
    run_with_events(&cfg, args).1
}

/// The `step` of every `evidence` event of scenario `scenario`, in order.
fn evidence_steps(stream: &[Value], scenario: u64) -> Vec<u64> {
    of_type(stream, "evidence")
        .into_iter()
        .filter(|e| e["scenario"] == scenario)
        .map(|e| e["step"].as_u64().expect("step"))
        .collect()
}

const PASS_AND_FAIL: &str = "Feature: e
  Scenario: passes
    Given set variable \"who\" to \"ann\"
    When I echo one piece of evidence for \"<<who>>\"
  Scenario: fails
    Given set variable \"who\" to \"bob\"
    When I echo one piece of evidence for \"<<who>>\"
    Then variable \"who\" should be equal to \"zed\"
";

#[test]
fn marked_records_only_the_steps_between_the_two_steps() {
    let stream = modes_run(
        "marked-between",
        &[(
            "e.feature",
            "Feature: e\n  Scenario: s\n    Given I echo one piece of evidence\n    And I am collecting evidence\n    Given I echo one piece of evidence\n    And I am not collecting evidence\n    Then I echo one piece of evidence\n",
        )],
        None,
        &["--evidence=marked"],
    );
    assert_eq!(evidence_steps(&stream, 0), [2], "{stream:?}");
    assert!(
        of_type(&stream, "evidence")[0].get("deferred").is_none(),
        "a marked step is written live"
    );
}

#[test]
fn the_evidence_tag_marks_a_whole_scenario_and_is_inherited_from_feature_and_rule() {
    let stream = modes_run(
        "marked-tag",
        &[
            (
                "a.feature",
                "Feature: a\n  Background:\n    Given I echo one piece of evidence\n\n  @evidence\n  Scenario: tagged\n    Given I echo one piece of evidence\n    And I am not collecting evidence\n    Then I echo one piece of evidence\n\n  Scenario: plain\n    Given I echo one piece of evidence\n\n  @evidence\n  Rule: r\n    Scenario: in the rule\n      Given I echo one piece of evidence\n",
            ),
            (
                "b.feature",
                "@evidence\nFeature: b\n  Scenario: s\n    Given I echo one piece of evidence\n",
            ),
        ],
        None,
        &["--evidence=marked"],
    );
    let of = |file: &str, scenario: u64| -> Vec<u64> {
        of_type(&stream, "evidence")
            .into_iter()
            .filter(|e| e["file"].as_str().is_some_and(|f| f.ends_with(file)))
            .filter(|e| e["scenario"] == scenario)
            .map(|e| e["step"].as_u64().expect("step"))
            .collect()
    };
    assert_eq!(
        of("a.feature", 0),
        [0, 1, 3],
        "the Background too, and a step cannot switch the tag off"
    );
    assert_eq!(of("a.feature", 1), Vec::<u64>::new());
    assert_eq!(of("a.feature", 2), [0, 1], "inherited from the rule");
    assert_eq!(of("b.feature", 0), [0], "inherited from the feature");
}

#[test]
fn a_mark_ends_with_the_scenario() {
    let stream = modes_run(
        "marked-reset",
        &[(
            "e.feature",
            "Feature: e\n  Scenario: marks\n    Given I am collecting evidence\n    Given I echo one piece of evidence\n  Scenario: next\n    Given I echo one piece of evidence\n",
        )],
        None,
        &["--evidence=marked"],
    );
    assert_eq!(evidence_steps(&stream, 0), [1]);
    assert_eq!(evidence_steps(&stream, 1), Vec::<u64>::new());
}

#[test]
fn a_mark_covers_macro_and_include_bodies_called_inside_it() {
    let cfg = write_evidence_project(
        "marked-nested",
        &[(
            "e.feature",
            "Feature: e\n  Scenario: s\n    Given I am collecting evidence\n    When I ping the service\n    And I include \"../lib/inc.feature\"\n",
        )],
        Some("- step: 'I ping the service'\n  do:\n    - I echo one piece of evidence\n"),
    );
    std::fs::create_dir_all(cfg.with_file_name("lib")).expect("mkdir");
    std::fs::write(
        cfg.with_file_name("lib/inc.feature"),
        "Feature: inc\n  Scenario: i\n    Given I echo one piece of evidence\n",
    )
    .expect("include");
    let (_, stream, _) = run_with_events(&cfg, &["--evidence=marked"]);
    // 0 marks, 1 the macro, 2 its body, 3 the include, 4 its step.
    assert_eq!(evidence_steps(&stream, 0), [2, 4], "{stream:?}");
}

#[test]
fn marks_record_nothing_without_the_marked_source() {
    let stream = modes_run(
        "marked-unused",
        &[(
            "e.feature",
            "Feature: e\n  @evidence\n  Scenario: s\n    Given I am collecting evidence\n    Given I echo one piece of evidence\n",
        )],
        None,
        &["--evidence=failures"],
    );
    assert!(of_type(&stream, "evidence").is_empty(), "{stream:?}");
}

#[test]
fn failures_records_every_step_of_a_failed_scenario_and_nothing_of_a_passing_one() {
    let stream = modes_run(
        "failures",
        &[("e.feature", PASS_AND_FAIL)],
        None,
        &["--evidence=failures"],
    );
    assert_eq!(evidence_steps(&stream, 0), Vec::<u64>::new());
    assert_eq!(evidence_steps(&stream, 1), [1]);
    assert!(
        of_type(&stream, "step_started")
            .iter()
            .all(|e| e.get("vars").is_none()),
        "a deferred step's values cannot ride on step_started"
    );
}

#[test]
fn deferred_events_carry_their_step_and_arrive_before_scenario_finished() {
    let stream = modes_run(
        "failures-order",
        &[("e.feature", PASS_AND_FAIL)],
        None,
        &["--evidence=failures"],
    );
    let seq = |e: &Value| e["seq"].as_u64().expect("seq");
    let evidence = of_type(&stream, "evidence");
    assert_eq!(evidence.len(), 1);
    assert_eq!(evidence[0]["deferred"], true);
    assert_eq!(evidence[0]["step"], 1);
    let finished = of_type(&stream, "scenario_finished")[1];
    let last_step = of_type(&stream, "step_finished")
        .into_iter()
        .filter(|e| e["scenario"] == 1)
        .map(seq)
        .max()
        .expect("steps");
    assert!(last_step < seq(evidence[0]) && seq(evidence[0]) < seq(finished));
}

#[test]
fn deferred_vars_arrive_as_a_vars_event() {
    let stream = modes_run(
        "failures-vars",
        &[("e.feature", PASS_AND_FAIL)],
        None,
        &["--evidence=failures"],
    );
    let vars = of_type(&stream, "vars");
    assert_eq!(vars.len(), 1, "only the failed scenario's: {vars:?}");
    assert_eq!(vars[0]["scenario"], 1);
    assert_eq!(vars[0]["step"], 1);
    assert_eq!(vars[0]["vars"], json!([["who", "bob"]]));
    assert_eq!(vars[0]["deferred"], true);
}

#[test]
fn marked_and_failures_combine() {
    let stream = modes_run(
        "marked-failures",
        &[(
            "e.feature",
            &PASS_AND_FAIL.replace(
                "  Scenario: passes\n",
                "  Scenario: passes\n    Given I am collecting evidence\n",
            ),
        )],
        None,
        &["--evidence=marked,failures"],
    );
    let evidence = of_type(&stream, "evidence");
    assert_eq!(evidence.len(), 2, "{evidence:?}");
    assert_eq!(evidence[0]["scenario"], 0);
    assert!(evidence[0].get("deferred").is_none(), "marked: live");
    assert_eq!(evidence[1]["scenario"], 1);
    assert_eq!(evidence[1]["deferred"], true, "failed: deferred");
    assert_eq!(stream[0]["evidence_sources"], json!(["marked", "failures"]));
}

#[test]
fn bare_evidence_means_all() {
    let stream = evidence_run(
        "bare-evidence",
        "    Given set variable \"who\" to \"ann\"\n    And set variable \"hi\" to \"<<who>>\"\n",
        &[],
    );
    assert_eq!(stream[0]["evidence_sources"], json!(["all"]));
    assert_eq!(
        step_text(&stream, "\"hi\"")["vars"],
        json!([["who", "ann"]])
    );
}

/// Sources take an `=`, so a bare `--evidence` before a feature path is still
/// the flag and the path, as when the flag took no value.
#[test]
fn a_bare_evidence_before_a_path_keeps_its_meaning() {
    let cfg = write_events_project(
        "bare-before-path",
        &[(
            "e.feature",
            "Feature: e\n  Scenario: s\n    Given set variable \"a\" to \"1\"\n",
        )],
        None,
    );
    let feature = cfg.with_file_name("features/e.feature");
    let (out, stream, _) = run_with_events(&cfg, &["--evidence", feature.to_str().expect("UTF-8")]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(stream[0]["evidence_sources"], json!(["all"]));

    // Without the `=`, a source name is a path that does not exist.
    let (out, _, _) = run_with_events(&cfg, &["--evidence", "marked"]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn an_unknown_source_is_refused() {
    let cfg = write_events_project(
        "unknown-source",
        &[(
            "e.feature",
            "Feature: e\n  Scenario: s\n    Given set variable \"a\" to \"1\"\n",
        )],
        None,
    );
    let events = cfg.with_file_name("events.ndjson");
    let run = |args: &[&str], env: Option<&str>| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_bddkit"));
        command
            .args(["run", "--config"])
            .arg(&cfg)
            .arg("--events")
            .arg(&events)
            .args(args)
            .env_remove("BDDKIT_EVIDENCE");
        if let Some(value) = env {
            command.env("BDDKIT_EVIDENCE", value);
        }
        command.output().expect("run bddkit")
    };
    for out in [
        run(&["--evidence=marked,bogus"], None),
        run(&[], Some("bogus")),
    ] {
        assert_eq!(out.status.code(), Some(2));
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("\"bogus\""), "{stderr}");
        assert!(stderr.contains("marked, failures or all"), "{stderr}");
    }

    let text = std::fs::read_to_string(&cfg).expect("config");
    std::fs::write(&cfg, format!("{text}evidence: [bogus]\n")).expect("config");
    let out = run(&[], None);
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("marked, failures or all"), "{stderr}");
}

#[test]
fn the_config_key_without_a_stream_is_ignored_and_the_flag_without_a_stream_is_refused() {
    let cfg = write_events_project(
        "config-evidence",
        &[(
            "e.feature",
            "Feature: e\n  Scenario: s\n    Given set variable \"a\" to \"1\"\n",
        )],
        None,
    );
    let text = std::fs::read_to_string(&cfg).expect("config");
    std::fs::write(&cfg, format!("{text}evidence: [marked]\n")).expect("config");
    let bare = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_bddkit"));
        command
            .args(["run", "--config"])
            .arg(&cfg)
            .env_remove("BDDKIT_EVENTS")
            .env_remove("BDDKIT_EVIDENCE");
        command
    };

    let out = bare().output().expect("run bddkit");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let out = bare()
        .env("BDDKIT_EVIDENCE", "all")
        .output()
        .expect("run bddkit");
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("$BDDKIT_EVIDENCE requires --events"),
        "{stderr}"
    );

    // With a stream: the config applies, the variable beats it, the flag both.
    let sources = |args: &[&str], env: Option<&str>| {
        let events = cfg.with_file_name("events.ndjson");
        let mut command = bare();
        command.arg("--events").arg(&events).args(args);
        if let Some(value) = env {
            command.env("BDDKIT_EVIDENCE", value);
        }
        command.output().expect("run bddkit");
        read_stream(&events)[0]["evidence_sources"].clone()
    };
    assert_eq!(sources(&[], None), json!(["marked"]));
    assert_eq!(sources(&[], Some("failures")), json!(["failures"]));
    assert_eq!(sources(&["--evidence"], Some("failures")), json!(["all"]));
}

/// A scenario whose evidence is deferred, held in a `sleep` when the signal
/// lands: nothing of it reaches the stream. This pins the outcome end to end
/// only; the guard for a scenario that FAILS after the signal (a plugin
/// instance dropped by the shutdown) is pinned by the unit test
/// `a_scenario_that_fails_after_an_interrupt_drops_its_deferred_evidence`.
#[cfg(unix)]
#[test]
fn an_interrupted_scenario_writes_no_deferred_evidence() {
    let cfg = write_events_project(
        "deferred-interrupt",
        &[(
            "a.feature",
            "Feature: held\n  Scenario: s\n    Given set variable \"who\" to \"ann\"\n    And set variable \"hi\" to \"<<who>>\"\n    And I sleep \"10\" seconds\n",
        )],
        None,
    );
    let events = cfg.with_file_name("events.ndjson");
    let mut child = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(&cfg)
        .arg("--events")
        .arg(&events)
        .args(["--evidence=failures"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn bddkit");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !std::fs::read_to_string(&events).is_ok_and(|text| text.contains("I sleep")) {
        assert!(
            std::time::Instant::now() < deadline,
            "the sleep never started"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let sent = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .expect("kill");
    assert!(sent.success());
    assert_eq!(child.wait().expect("wait").code(), Some(130));

    let stream = read_stream(&events);
    assert_eq!(stream.last().expect("lines")["type"], "run_finished");
    assert!(of_type(&stream, "vars").is_empty(), "{stream:?}");
}

#[test]
fn scenario_finished_reports_evidence_coverage() {
    let feature = PASS_AND_FAIL.replace(
        "  Scenario: passes\n",
        "  Scenario: passes\n    Given I am collecting evidence\n",
    );
    let coverage = |args: &[&str]| -> Vec<Value> {
        let stream = modes_run("coverage", &[("e.feature", &feature)], None, args);
        of_type(&stream, "scenario_finished")
            .into_iter()
            .map(|e| e["evidence"].clone())
            .collect()
    };
    assert_eq!(coverage(&[]), [json!("none"), json!("none")]);
    assert_eq!(coverage(&["--evidence"]), [json!("full"), json!("full")]);
    assert_eq!(
        coverage(&["--evidence=marked,failures"]),
        [json!("partial"), json!("full")],
        "the marking step itself is not marked; the failed scenario is whole"
    );
    assert_eq!(
        coverage(&["--evidence=failures"]),
        [json!("none"), json!("full")]
    );
}

/// A run refused over an evidence variable empties a stale stream like any
/// other refusal; a buffer nothing would use is never read.
#[test]
fn an_evidence_variable_refusal_empties_the_stream_and_an_unused_buffer_is_ignored() {
    let cfg = write_events_project(
        "evidence-refused",
        &[(
            "e.feature",
            "Feature: e\n  Scenario: s\n    Given set variable \"a\" to \"1\"\n",
        )],
        None,
    );
    let events = cfg.with_file_name("events.ndjson");
    let run = |vars: &[(&str, &str)], args: &[&str]| {
        std::fs::write(&events, "{\"type\":\"yesterday\"}\n").expect("stale stream");
        let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
            .args(["run", "--config"])
            .arg(&cfg)
            .arg("--events")
            .arg(&events)
            .args(args)
            .env_remove("BDDKIT_EVIDENCE")
            .env_remove("BDDKIT_EVIDENCE_BUFFER")
            .envs(vars.iter().copied())
            .output()
            .expect("run bddkit");
        (out.status.code(), read_stream(&events))
    };
    for (vars, args) in [
        (&[("BDDKIT_EVIDENCE", "bogus")][..], &[][..]),
        (
            &[("BDDKIT_EVIDENCE_BUFFER", "64M")][..],
            &["--evidence=failures"][..],
        ),
    ] {
        let (code, stream) = run(vars, args);
        assert_eq!(code, Some(2), "{vars:?}");
        assert!(stream.is_empty(), "{vars:?}: {stream:?}");
    }
    let (code, _) = run(&[("BDDKIT_EVIDENCE_BUFFER", "64M")], &[]);
    assert_eq!(code, Some(0), "no evidence, no buffer to read");
}

#[test]
fn run_started_keeps_the_evidence_boolean_and_adds_the_sources() {
    let cfg = write_events_project(
        "run-started-sources",
        &[(
            "e.feature",
            "Feature: e\n  Scenario: s\n    Given set variable \"a\" to \"1\"\n",
        )],
        None,
    );
    let started = |args: &[&str]| run_with_events(&cfg, args).1[0].clone();
    let off = started(&[]);
    assert_eq!(off["evidence"], false);
    assert_eq!(off["evidence_sources"], json!([]));
    let on = started(&["--evidence=failures,marked,failures"]);
    assert_eq!(on["evidence"], true);
    assert_eq!(on["evidence_sources"], json!(["marked", "failures"]));
}

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

// Issue #109: source annotations must survive the parser without changing execution.
fn annotation_stream(name: &str, feature: &str) -> Vec<Value> {
    let cfg = write_events_project(name, &[("a.feature", feature)], None);
    let (out, stream, _) = run_with_events(&cfg, &[]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    stream
}

#[test]
fn annotations_a_comment_directly_above_a_step_reaches_its_step_started() {
    let stream = annotation_stream(
        "annotation-comment",
        "Feature: f\n  Scenario: s\n    # first paragraph\n    #\n    # second paragraph\n    Given set variable \"x\" to \"1\"\n",
    );
    assert_eq!(
        of_type(&stream, "step_started")[0]["comment"],
        "first paragraph\n\nsecond paragraph"
    );
}

#[test]
fn annotations_a_commented_step_that_matches_the_registry_is_a_disabled_step() {
    let stream = annotation_stream(
        "annotation-disabled",
        "Feature: f\n  Scenario: s\n    # Disabled until fixed.\n    # Then variable \"missing\" should be equal to \"<<undefined>>\"\n    Given set variable \"x\" to \"1\"\n",
    );
    let disabled = of_type(&stream, "step_disabled");
    assert_eq!(disabled.len(), 1, "{stream:?}");
    assert_eq!(disabled[0]["comment"], "Disabled until fixed.");
    assert_eq!(disabled[0]["keyword"], "Then ");
    assert_eq!(
        disabled[0]["text"],
        "variable \"missing\" should be equal to \"<<undefined>>\""
    );
    assert_eq!(disabled[0]["line"], 4);
    assert!(disabled[0].get("step").is_none());
    assert_eq!(of_type(&stream, "step_started")[0]["step"], 0);
}

#[test]
fn annotations_a_commented_line_that_starts_with_a_keyword_but_matches_nothing_is_a_comment() {
    let stream = annotation_stream(
        "annotation-prose",
        "Feature: f\n  Scenario: s\n    # Then we wait for the queue\n    Given set variable \"x\" to \"1\"\n",
    );
    assert!(of_type(&stream, "step_disabled").is_empty());
    assert_eq!(
        of_type(&stream, "step_started")[0]["comment"],
        "Then we wait for the queue"
    );
}

#[test]
fn annotations_a_block_followed_by_a_blank_line_starts_a_section_with_a_title_and_a_note() {
    let stream = annotation_stream(
        "annotation-section",
        "Feature: f\n  Scenario: s\n    #\n    # Setup\n    # An order paid by card.\n    #\n\n    Given set variable \"x\" to \"1\"\n    # End note\n",
    );
    let sections = of_type(&stream, "section");
    assert_eq!(sections.len(), 2, "{stream:?}");
    assert_eq!(sections[0]["title"], "Setup");
    assert_eq!(sections[0]["note"], "An order paid by card.");
    assert_eq!(sections[0]["line"], 3);
    assert_eq!(sections[1]["title"], "End note");
    assert!(sections[1].get("note").is_none());
    assert!(
        sections[1]["seq"].as_u64().unwrap()
            < of_type(&stream, "scenario_finished")[0]["seq"]
                .as_u64()
                .unwrap()
    );
}

#[test]
fn annotations_sections_and_disabled_steps_after_a_failure_are_still_emitted_in_order() {
    let cfg = write_events_project(
        "annotation-failure",
        &[(
            "a.feature",
            "Feature: f\n  Scenario: s\n    Given set variable \"x\" to \"1\"\n    Then variable \"x\" should be equal to \"2\"\n    # Later\n\n    # a skipped step's note\n    And set variable \"y\" to \"2\"\n    # Then variable \"undefined\" should be equal to \"1\"\n    # Closing\n",
        )],
        None,
    );
    let (out, stream, _) = run_with_events(&cfg, &[]);
    assert_eq!(out.status.code(), Some(1));
    let tail: Vec<_> = stream
        .iter()
        .filter(|e| {
            matches!(
                e["type"].as_str(),
                Some("section" | "step_skipped" | "step_disabled" | "scenario_finished")
            )
        })
        .map(|e| e["type"].as_str().unwrap())
        .collect();
    assert_eq!(
        tail,
        [
            "section",
            "step_skipped",
            "step_disabled",
            "section",
            "scenario_finished"
        ]
    );
    assert_eq!(
        of_type(&stream, "step_skipped")[0]["comment"],
        "a skipped step's note"
    );
    assert_eq!(of_type(&stream, "step_skipped")[0]["step"], 2);
}

#[test]
fn annotations_element_comments_attach_above_tags_and_never_include_the_language_line() {
    let stream = annotation_stream(
        "annotation-elements",
        "# file note\n# language: en\n@file\nFeature: f\n  # background note\n  Background:\n    Given set variable \"x\" to \"1\"\n  # first note\n  @first\n  Scenario: first\n    Given set variable \"y\" to \"2\"\n  # rule note\n  Rule: r\n    # rule background note\n    Background:\n      Given set variable \"z\" to \"3\"\n    # outline note\n    Scenario Outline: row\n      Given set variable \"n\" to \"<n>\"\n      # examples note\n      Examples:\n        | n |\n        | 1 |\n",
    );
    let file = of_type(&stream, "file_started")[0];
    assert_eq!(file["comment"], "file note");
    assert_eq!(file["background"]["comment"], "background note");
    let scenarios = of_type(&stream, "scenario_started");
    assert_eq!(scenarios[0]["comment"], "first note");
    assert_eq!(scenarios[1]["comment"], "outline note");
    assert_eq!(scenarios[1]["rule"]["comment"], "rule note");
    assert_eq!(
        scenarios[1]["rule"]["background"]["comment"],
        "rule background note"
    );
    assert_eq!(scenarios[1]["examples"]["comment"], "examples note");
    assert!(of_type(&stream, "section").is_empty(), "{stream:?}");
}

#[test]
fn annotations_a_disabled_outline_step_is_matched_after_placeholder_substitution() {
    let stream = annotation_stream(
        "annotation-outline",
        "Feature: f\n  Scenario Outline: row\n    # Then variable \"x\" should be equal to \"<n>\"\n    Given set variable \"n\" to \"<n>\"\n    Examples:\n      | n |\n      | 200 |\n      | 201 |\n",
    );
    let disabled = of_type(&stream, "step_disabled");
    assert_eq!(disabled.len(), 2, "{stream:?}");
    assert_eq!(
        disabled[0]["text"],
        "variable \"x\" should be equal to \"200\""
    );
    assert_eq!(
        disabled[1]["text"],
        "variable \"x\" should be equal to \"201\""
    );
}

#[test]
fn annotations_a_russian_feature_file_recognises_its_own_keywords() {
    let stream = annotation_stream(
        "annotation-ru",
        "# language: ru\nФункционал: f\n  Сценарий: s\n    # Тогда variable \"x\" should be equal to \"200\"\n    # Then variable \"x\" should be equal to \"201\"\n    Допустим set variable \"x\" to \"1\"\n",
    );
    let disabled = of_type(&stream, "step_disabled");
    assert_eq!(disabled.len(), 1, "{stream:?}");
    assert_eq!(disabled[0]["keyword"], "Тогда ");
    assert_eq!(
        of_type(&stream, "step_started")[0]["comment"],
        "Then variable \"x\" should be equal to \"201\""
    );
    assert!(of_type(&stream, "file_started")[0].get("comment").is_none());
}

#[test]
fn annotations_background_sections_repeat_before_every_scenario() {
    let stream = annotation_stream(
        "annotation-background",
        "Feature: f\n  Background:\n    # Feature setup\n\n    Given set variable \"x\" to \"1\"\n    # Feature closing\n\n  Scenario: loose\n    Given set variable \"y\" to \"1\"\n  Rule: r\n    Background:\n      # Rule setup\n\n      Given set variable \"z\" to \"1\"\n    Scenario: ruled\n      Given set variable \"y\" to \"1\"\n",
    );
    let sections = of_type(&stream, "section");
    let titles: Vec<_> = sections
        .iter()
        .map(|e| e["title"].as_str().unwrap())
        .collect();
    assert_eq!(
        titles,
        [
            "Feature setup",
            "Feature closing",
            "Feature setup",
            "Feature closing",
            "Rule setup"
        ]
    );
    assert_eq!(sections[2]["scenario"], 1);
}

#[test]
fn annotations_sections_inside_an_included_scenario_carry_parent_and_source() {
    let cfg = write_events_project(
        "annotation-include",
        &[
            (
                "caller.feature",
                "Feature: f\n  Scenario: caller\n    Given I include \"included.feature\"\n",
            ),
            (
                "included.feature",
                "Feature: included\n  Background:\n    # Background\n\n    Given set variable \"x\" to \"1\"\n  Scenario: child\n    # Child\n\n    # Then variable \"missing\" should be equal to \"1\"\n    Given set variable \"y\" to \"1\"\n    # Closing\n",
            ),
        ],
        None,
    );
    let (out, stream, _) = run_with_events(&cfg, &[]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let caller = of_type(&stream, "scenario_started")
        .into_iter()
        .find(|e| e["name"] == "caller")
        .unwrap();
    let sections: Vec<_> = of_type(&stream, "section")
        .into_iter()
        .filter(|e| e["file"] == caller["file"])
        .collect();
    assert_eq!(sections.len(), 3, "{stream:?}");
    for item in sections.into_iter().chain(
        of_type(&stream, "step_disabled")
            .into_iter()
            .filter(|e| e["file"] == caller["file"]),
    ) {
        assert_eq!(item["parent"], 0, "{item}");
        assert!(
            item["source"]
                .as_str()
                .unwrap()
                .ends_with("included.feature")
        );
    }
}

#[test]
fn annotations_a_secret_in_a_comment_is_masked() {
    let cfg = write_events_project(
        "annotation-secrets",
        &[(
            "a.feature",
            "Feature: f\n  Scenario: s\n    Given set variable \"secret_value\" to \"annotation-secret-9876\"\n    # annotation-secret-9876\n\n    # annotation-secret-9876\n    # Then variable \"x\" should be equal to \"annotation-secret-9876\"\n    # annotation-secret-9876\n    Given set variable \"y\" to \"1\"\n",
        )],
        None,
    );
    let (out, stream, _) = run_with_events(&cfg, &[]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(of_type(&stream, "section")[0]["title"], "***");
    let disabled = of_type(&stream, "step_disabled");
    assert_eq!(disabled[0]["comment"], "***");
    assert!(!disabled[0].to_string().contains("annotation-secret-9876"));
    assert_eq!(of_type(&stream, "step_started")[1]["comment"], "***");
}

#[test]
fn annotations_doctor_counts_disabled_steps_and_lists_lines_that_look_like_steps() {
    let cfg = write_evidence_project(
        "annotation-doctor",
        &[
            (
                "a.feature",
                "Feature: f\n  Background:\n    # Given set variable \"disabled\" to \"1\"\n    Given set variable \"x\" to \"1\"\n  Scenario Outline: rows\n    # Then variable \"x\" should be equal to \"<n>\"\n    # Then we wait for the queue\n    Given set variable \"n\" to \"<n>\"\n    Examples:\n      | n |\n      | 200 |\n      | 201 |\n",
            ),
            (
                "b.feature",
                "Feature: g\n  Scenario: s\n    # Given set variable \"disabled\" to \"2\"\n    Given set variable \"y\" to \"1\"\n",
            ),
        ],
        None,
    );
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--json", "--config"])
        .arg(&cfg)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    let steps = report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["stage"] == "steps")
        .unwrap();
    assert_eq!(steps["status"], "ok");
    let detail = steps["detail"].as_str().unwrap();
    assert!(
        detail.contains("2 file(s), every step matched; 3 disabled step(s) in 2 file(s)"),
        "{detail}"
    );
    assert!(detail.contains("a.feature:7"), "{detail}");
    assert_eq!(
        detail.matches("a.feature:7").count(),
        1,
        "outline rows must not duplicate a warning: {detail}"
    );
}

#[test]
fn annotations_a_hash_inside_a_docstring_or_a_table_is_content() {
    for fence in ["\"\"\"", "```"] {
        let feature = format!(
            "Feature: f\n  Scenario: s\n    Given I echo the plain doc string:\n      {fence}\n      # Then variable \"x\" should be equal to \"200\"\n      # A false section\n\n      # Another false section\n      {fence}\n    And Print table:\n      | name | value |\n      | field | # Then variable \"x\" should be equal to \"201\" |\n    # a real note\n    Given set variable \"x\" to \"1\"\n"
        );
        let cfg = write_evidence_project(
            &format!(
                "annotation-content-{}",
                if fence == "```" { "backtick" } else { "quote" }
            ),
            &[("a.feature", &feature)],
            None,
        );
        let (out, stream, _) = run_with_events(&cfg, &[]);
        assert_eq!(out.status.code(), Some(0), "{out:?}");
        assert!(of_type(&stream, "section").is_empty(), "{stream:?}");
        assert!(of_type(&stream, "step_disabled").is_empty(), "{stream:?}");
        assert_eq!(of_type(&stream, "step_started").len(), 3);
        assert_eq!(
            of_type(&stream, "step_started")[2]["comment"],
            "a real note"
        );
    }
}

#[test]
fn annotations_disabled_macro_calls_match_without_executing_or_validating_them() {
    let cfg = write_events_project(
        "annotation-macro",
        &[(
            "a.feature",
            "Feature: f\n  Scenario: s\n    # Given my disabled action\n    Given set variable \"x\" to \"1\"\n",
        )],
        Some(
            "- step: 'my disabled action'\n  do:\n    - 'variable \"missing\" should be equal to \"never\"'\n",
        ),
    );
    let (out, stream, _) = run_with_events(&cfg, &[]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(
        of_type(&stream, "step_disabled")[0]["text"],
        "my disabled action"
    );
    assert_eq!(of_type(&stream, "step_started").len(), 1);
}

#[test]
fn annotations_include_failures_still_emit_nested_sections_disabled_and_skipped_steps() {
    let cfg = write_evidence_project(
        "annotation-include-failure",
        &[(
            "a.feature",
            "Feature: f\n  Scenario: caller\n    Given I include \"child.feature\"\n",
        )],
        None,
    );
    std::fs::write(cfg.parent().unwrap().join("features/child.feature"), "Feature: child\n  Scenario: child\n    Given set variable \"x\" to \"1\"\n    Then variable \"x\" should be equal to \"2\"\n    # Later\n\n    # Then variable \"x\" should be equal to \"200\"\n    # skipped note\n    Given set variable \"y\" to \"1\"\n    # Closing\n").unwrap();
    let (out, stream, _) = run_with_events(&cfg, &[]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let nested: Vec<_> = stream
        .iter()
        .filter(|e| {
            e.get("parent") == Some(&serde_json::json!(0))
                && matches!(
                    e["type"].as_str(),
                    Some("section" | "step_disabled" | "step_skipped")
                )
        })
        .collect();
    assert_eq!(
        nested
            .iter()
            .map(|e| e["type"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["section", "step_disabled", "step_skipped", "section"]
    );
    assert_eq!(nested[2]["comment"], "skipped note");
    assert_eq!(nested[2]["step"], 3);
    for event in nested {
        assert!(event["source"].as_str().unwrap().ends_with("child.feature"));
    }
}

#[test]
fn annotations_the_junit_and_cucumber_reports_do_not_change() {
    let feature = "# File note\nFeature: f\n  Description\n  Background:\n    # Setup\n\n    Given set variable \"x\" to \"1\"\n  # Scenario note\n  Scenario: s\n    # step note\n    Given set variable \"y\" to \"1\"\n    # Disabled reason\n    # Then variable \"missing\" should be equal to \"<<undefined>>\"\n\n    # Checks\n\n    Then variable \"y\" should be equal to \"1\"\n    # Closing\n";
    let cfg = write_events_project("annotation-report-compat", &[("a.feature", feature)], None);
    let dir = cfg.parent().unwrap();
    let junit = dir.join("junit.xml");
    let cucumber = dir.join("cucumber.json");
    let reports = || {
        let (out, stream, _) = run_with_events(
            &cfg,
            &[
                "--junit",
                junit.to_str().unwrap(),
                "--cucumber-json",
                cucumber.to_str().unwrap(),
            ],
        );
        assert_eq!(out.status.code(), Some(0), "{out:?}");
        let timing = regex::Regex::new(r#"time="[0-9.]+"|\([0-9.]+s\)"#).unwrap();
        let xml = std::fs::read_to_string(&junit).unwrap();
        let xml = timing.replace_all(&xml, "TIMING").into_owned();
        fn normalize_times(value: &mut Value) {
            match value {
                Value::Object(map) => {
                    map.remove("duration");
                    for child in map.values_mut() {
                        normalize_times(child);
                    }
                }
                Value::Array(values) => {
                    for child in values {
                        normalize_times(child);
                    }
                }
                _ => {}
            }
        }
        let mut json: Value = serde_json::from_slice(&std::fs::read(&cucumber).unwrap()).unwrap();
        normalize_times(&mut json);
        (xml, json, stream)
    };
    let (annotated_xml, annotated_json, stream) = reports();
    assert_eq!(of_type(&stream, "step_disabled").len(), 1);
    assert_eq!(of_type(&stream, "section").len(), 3);
    let blanked = feature
        .lines()
        .map(|line| {
            if line.trim_start().starts_with('#') {
                ""
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(dir.join("features/a.feature"), blanked).unwrap();
    let (plain_xml, plain_json, _) = reports();
    assert_eq!(annotated_xml, plain_xml);
    assert_eq!(annotated_json, plain_json);
}

#[test]
fn annotations_a_disabled_step_masks_short_secret_parameters_by_position() {
    let stream = annotation_stream(
        "annotation-short-secret",
        "Feature: f\n  Scenario: s\n    # Given I encrypt \"payload\" with AES using key \"x\" as \"reg\"\n    Given set variable \"x\" to \"1\"\n",
    );
    let disabled = of_type(&stream, "step_disabled");
    assert_eq!(disabled.len(), 1, "{stream:?}");
    assert!(
        disabled[0]["text"]
            .as_str()
            .unwrap()
            .contains("key \"***\"")
    );
}

#[test]
fn annotations_docstring_fences_never_hide_executable_assertions() {
    for (name, body) in [
        ("inline", "      \"\"\"hello\"\"\"\n"),
        (
            "closing-comment",
            "      \"\"\"\n      hello\n      \"\"\" # body ends\n",
        ),
        ("backtick-inline", "      ```hello```\n"),
        (
            "backtick-comment",
            "      ```\n      hello\n      ``` # body ends\n",
        ),
    ] {
        let feature = format!(
            "Feature: f\n  Scenario: s\n    Given I echo the plain doc string:\n{body}    # A real section\n\n    # A real assertion's comment\n    Then variable \"missing\" should be equal to \"wrong\"\n"
        );
        let cfg = write_evidence_project(
            &format!("annotation-fence-{name}"),
            &[("a.feature", &feature)],
            None,
        );
        let (out, stream, _) = run_with_events(&cfg, &[]);
        assert_eq!(out.status.code(), Some(1), "{name}: {out:?}");
        assert_eq!(
            of_type(&stream, "step_started").len(),
            2,
            "{name}: {stream:?}"
        );
        assert_eq!(of_type(&stream, "section")[0]["title"], "A real section");
        assert_eq!(
            of_type(&stream, "step_started")[1]["comment"],
            "A real assertion's comment"
        );
    }
}

#[test]
fn annotations_closing_outline_blocks_after_examples_repeat_for_every_row_of_that_block() {
    let stream = annotation_stream(
        "annotation-outline-closing",
        "Feature: f\n  Scenario Outline: rows\n    Given set variable \"n\" to \"<n>\"\n    Examples: first\n      | n |\n      | 200 |\n      | 201 |\n    # Then variable \"x\" should be equal to \"<n>\"\n    # Closing <n>\n",
    );
    let disabled = of_type(&stream, "step_disabled");
    let sections = of_type(&stream, "section");
    assert_eq!(disabled.len(), 2, "{stream:?}");
    assert_eq!(
        disabled[0]["text"],
        "variable \"x\" should be equal to \"200\""
    );
    assert_eq!(
        disabled[1]["text"],
        "variable \"x\" should be equal to \"201\""
    );
    assert_eq!(sections.len(), 2, "{stream:?}");
    assert_eq!(sections[0]["title"], "Closing <n>");
    assert_eq!(sections[1]["title"], "Closing <n>");
    for (index, section) in sections.iter().enumerate() {
        assert_eq!(section["scenario"], index);
        assert!(
            section["seq"].as_u64().unwrap()
                < of_type(&stream, "scenario_finished")[index]["seq"]
                    .as_u64()
                    .unwrap()
        );
    }
}

#[test]
fn annotations_a_docstring_comment_containing_a_fence_never_hides_a_failure() {
    for fence in ["\"\"\"", "```"] {
        let feature = format!(
            "Feature: f\n  Scenario: s\n    Given set variable \"x\" to \"1\"\n    Then variable \"x\" should be equal to \"2\"\n    Given I echo the plain doc string:\n      {fence}\n      # A literal {fence} in a body comment\n      # Then variable \"x\" should be equal to \"200\"\n      ordinary body content\n      {fence}\n    Given set variable \"y\" to \"1\"\n"
        );
        let cfg = write_evidence_project(
            &format!(
                "annotation-comment-fence-{}",
                if fence == "```" { "backtick" } else { "quote" }
            ),
            &[("a.feature", &feature)],
            None,
        );
        let (out, stream, _) = run_with_events(&cfg, &[]);
        assert_eq!(out.status.code(), Some(1), "{out:?}");
        assert_eq!(of_type(&stream, "step_started").len(), 2, "{stream:?}");
        assert_eq!(of_type(&stream, "step_skipped").len(), 2, "{stream:?}");
        assert!(of_type(&stream, "step_disabled").is_empty(), "{stream:?}");
        assert!(of_type(&stream, "section").is_empty(), "{stream:?}");
    }
}

#[test]
fn annotations_comments_between_table_rows_are_not_step_annotations() {
    let feature = "Feature: f\n  Scenario: s\n    Given Print table:\n      | name | value |\n      | a | 1 |\n      # Then variable \"x\" should be equal to \"200\"\n      # note about the next row\n      | b | 2 |\n    Given set variable \"x\" to \"1\"\n";
    let cfg = write_evidence_project("annotation-table-comments", &[("a.feature", feature)], None);
    let (out, stream, _) = run_with_events(&cfg, &[]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(of_type(&stream, "step_disabled").is_empty(), "{stream:?}");
    assert!(of_type(&stream, "section").is_empty(), "{stream:?}");
    assert!(
        of_type(&stream, "step_started")[1].get("comment").is_none(),
        "{stream:?}"
    );
}

#[test]
fn annotations_outline_substitution_changes_disabled_steps_and_preserves_prose() {
    let stream = annotation_stream(
        "annotation-outline-prose",
        "Feature: f\n  Scenario Outline: rows\n    # Section <email>\n    # Note <email>\n\n    # the service sends <email> a link\n    Given set variable \"email\" to \"<email>\"\n    # Then the service sends <email> a link\n    # Then variable \"x\" should be equal to \"<code>\"\n\n    Examples:\n      | email | code |\n      | a@b.c | 200 |\n",
    );
    let section = of_type(&stream, "section")[0];
    assert_eq!(section["title"], "Section <email>");
    assert_eq!(section["note"], "Note <email>");
    assert_eq!(
        of_type(&stream, "step_started")[0]["comment"],
        "the service sends <email> a link"
    );
    let disabled = of_type(&stream, "step_disabled")[0];
    assert_eq!(
        disabled["text"],
        "variable \"x\" should be equal to \"200\""
    );
    assert_eq!(disabled["comment"], "Then the service sends <email> a link");
}

#[test]
fn annotations_examples_notes_only_follow_rows_from_their_own_table() {
    let stream = annotation_stream(
        "annotation-examples-scope",
        "Feature: f\n  Scenario Outline: rows\n    Given set variable \"n\" to \"<n>\"\n    Examples: first\n      | n |\n      | 200 |\n\n    # Closing first\n\n    Examples: second\n      | n |\n      | 201 |\n\n    # Closing second\n",
    );
    let sections = of_type(&stream, "section");
    assert_eq!(sections.len(), 2, "{stream:?}");
    assert_eq!(sections[0]["title"], "Closing first");
    assert_eq!(sections[0]["scenario"], 0);
    assert_eq!(sections[1]["title"], "Closing second");
    assert_eq!(sections[1]["scenario"], 1);
}

#[test]
fn annotations_comments_outside_step_regions_are_ignored_and_scenario_closing_notes_are_kept() {
    let stream = annotation_stream(
        "annotation-region-boundaries",
        "Feature: f\n  # outside any step region\n\n  Scenario: first\n    Given set variable \"x\" to \"1\"\n\n    # closing first\n\n  Scenario: second\n    Given set variable \"y\" to \"1\"\n  Rule: r\n    # outside the rule's scenarios\n\n    Scenario: third\n      Given set variable \"z\" to \"1\"\n",
    );
    let sections = of_type(&stream, "section");
    assert_eq!(sections.len(), 1, "{stream:?}");
    assert_eq!(sections[0]["title"], "closing first");
    assert_eq!(sections[0]["scenario"], 0);
}

#[test]
fn annotations_a_note_after_a_table_belongs_to_the_next_step() {
    let cfg = write_evidence_project(
        "annotation-after-table",
        &[(
            "a.feature",
            "Feature: f\n  Scenario: s\n    Given Print table:\n      | name | value |\n      | a | 1 |\n      # table-only note\n      | b | 2 |\n    # real step note\n    Given set variable \"x\" to \"1\"\n",
        )],
        None,
    );
    let (out, stream, _) = run_with_events(&cfg, &[]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(
        of_type(&stream, "step_started")[1]["comment"],
        "real step note"
    );
}

#[test]
fn annotations_doctor_reports_a_disabled_step_that_fails_to_match_in_one_outline_row() {
    let cfg = write_evidence_project(
        "annotation-partial-match",
        &[(
            "a.feature",
            "Feature: f\n  Scenario Outline: rows\n    # Then I sleep <code> seconds\n    Given set variable \"code\" to \"<code>\"\n    Examples:\n      | code |\n      | 200 |\n      | nope |\n",
        )],
        None,
    );
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--json", "--config"])
        .arg(&cfg)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    let steps = report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["stage"] == "steps")
        .unwrap();
    assert_eq!(steps["status"], "ok");
    let detail = steps["detail"].as_str().unwrap();
    assert!(
        detail.contains("1 disabled step(s) in 1 file(s)"),
        "{detail}"
    );
    assert!(
        detail.contains("a.feature:3"),
        "the unmatched row is still a finding: {detail}"
    );
}

#[test]
fn annotations_an_included_docstring_comment_never_hides_a_failure() {
    let cfg = write_evidence_project(
        "annotation-include-fence",
        &[(
            "a.feature",
            "Feature: f\n  Scenario: caller\n    Given I include \"../child.feature\"\n",
        )],
        None,
    );
    std::fs::write(cfg.parent().unwrap().join("child.feature"), "Feature: child\n  Scenario: child\n    Given set variable \"x\" to \"1\"\n    Then variable \"x\" should be equal to \"2\"\n    Given I echo the plain doc string:\n      \"\"\"\n      # A literal \"\"\" in a body comment\n      # Then variable \"x\" should be equal to \"200\"\n      ordinary body content\n      \"\"\"\n    Given set variable \"y\" to \"1\"\n").unwrap();
    let (out, stream, _) = run_with_events(&cfg, &[]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let skipped = of_type(&stream, "step_skipped");
    assert_eq!(skipped.len(), 2, "{stream:?}");
    assert!(skipped.iter().all(|event| event["parent"] == 0));
    assert!(of_type(&stream, "step_disabled").is_empty(), "{stream:?}");
}

/// Flag, then environment, then config key, for the options the config also
/// holds. Three files on one worker, the first fails: how many run says which
/// source `fail_fast` came from.
#[test]
fn fail_fast_and_concurrency_follow_flag_then_environment_then_config() {
    let dir = std::env::temp_dir().join(format!("bddkit-option-forms-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/a_broken.feature"),
        "Feature: broken\n  Scenario: s\n    \
         Given set variable \"x\" to \"1\"\n    Then variable \"x\" should be equal to \"2\"\n",
    )
    .expect("write feature");
    for name in ["b_ok", "c_ok"] {
        std::fs::write(
            dir.join(format!("features/{name}.feature")),
            format!("Feature: {name}\n  Scenario: s\n    Given set variable \"x\" to \"1\"\n"),
        )
        .expect("write feature");
    }
    let features = dir
        .join("features")
        .display()
        .to_string()
        .replace('\\', "/");
    let write_config = |extra: &str| {
        std::fs::write(
            dir.join("cfg.yaml"),
            format!("{extra}paths: [{features}]\nresources:\n  api: {{}}\n"),
        )
        .expect("write config");
    };
    let run = |args: &[&str], envs: &[(&str, &str)]| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_bddkit"));
        cmd.args(["run", "--config"]).arg(dir.join("cfg.yaml"));
        cmd.args(args).env_remove("BDDKIT_FAIL_FAST");
        cmd.env_remove("BDDKIT_CONCURRENCY")
            .envs(envs.iter().copied());
        let out = cmd.output().expect("run bddkit");
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };

    // The config key alone.
    write_config("concurrency: 1\nfail_fast: true\n");
    let (code, stdout, _) = run(&[], &[]);
    assert_eq!(code, Some(1));
    assert!(stdout.contains("files: 1"), "{stdout}");
    // The flag outranks the key, the variable outranks the key too.
    let (_, stdout, _) = run(&["--fail-fast=false"], &[]);
    assert!(stdout.contains("files: 3"), "{stdout}");
    let (_, stdout, _) = run(&[], &[("BDDKIT_FAIL_FAST", "0")]);
    assert!(stdout.contains("files: 3"), "{stdout}");
    // The variable alone, then a flag over the variable.
    write_config("concurrency: 4\n");
    let (_, stdout, _) = run(&["--concurrency", "1"], &[("BDDKIT_FAIL_FAST", "true")]);
    assert!(stdout.contains("files: 1"), "{stdout}");
    let (_, stdout, _) = run(
        &["--fail-fast=false"],
        &[("BDDKIT_FAIL_FAST", "true"), ("BDDKIT_CONCURRENCY", "1")],
    );
    assert!(stdout.contains("files: 3"), "{stdout}");
    // A malformed variable is a refusal before the first request; doctor reports it.
    let (code, _, stderr) = run(&[], &[("BDDKIT_FAIL_FAST", "yes")]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains("BDDKIT_FAIL_FAST"), "{stderr}");
    let doctor = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config"])
        .arg(dir.join("cfg.yaml"))
        .env("BDDKIT_CONCURRENCY", "many")
        .output()
        .expect("run doctor");
    assert_eq!(doctor.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&doctor.stdout).contains("BDDKIT_CONCURRENCY"));
}

/// `tags` is a config key, `BDDKIT_TAG` a variable, `--tag` a flag; a selection
/// that matches nothing is exit 2 in `run` and a failed row in `doctor`.
#[test]
fn tags_follow_flag_then_environment_then_config_and_doctor_agrees() {
    let dir = std::env::temp_dir().join(format!("bddkit-tags-forms-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/a.feature"),
        "Feature: a\n  @smoke\n  Scenario: s\n    Given set variable \"x\" to \"1\"\n",
    )
    .expect("write feature");
    let features = dir
        .join("features")
        .display()
        .to_string()
        .replace('\\', "/");
    let write_config = |tags: &str| {
        std::fs::write(
            dir.join("cfg.yaml"),
            format!("{tags}paths: [{features}]\nresources:\n  api: {{}}\n"),
        )
        .expect("write config");
    };
    let bddkit = |command: &str, args: &[&str], envs: &[(&str, &str)]| {
        let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
            .args([command, "--config"])
            .arg(dir.join("cfg.yaml"))
            .args(args)
            .env_remove("BDDKIT_TAG")
            .envs(envs.iter().copied())
            .output()
            .expect("run bddkit");
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
        )
    };

    write_config("tags: [smoke]\n");
    assert_eq!(bddkit("run", &[], &[]).0, Some(0));
    // The variable outranks the key, the flag outranks the variable.
    assert_eq!(bddkit("run", &[], &[("BDDKIT_TAG", "other")]).0, Some(2));
    assert_eq!(
        bddkit("run", &["--tag", "smoke"], &[("BDDKIT_TAG", "other")]).0,
        Some(0)
    );
    // A key that selects nothing: run refuses, doctor says so.
    write_config("tags: [other]\n");
    assert_eq!(bddkit("run", &[], &[]).0, Some(2));
    let (code, stdout) = bddkit("doctor", &[], &[]);
    assert_eq!(code, Some(1), "{stdout}");
    assert!(stdout.contains("no scenario to run"), "{stdout}");
    // Tags decide whether a run has anything to do, not which files `doctor` reads.
    std::fs::write(
        dir.join("features/b.feature"),
        "Feature: b\n  @slow\n  Scenario: s\n    When I frobnicate\n",
    )
    .expect("write feature");
    write_config("tags: [smoke]\n");
    let (code, stdout) = bddkit("doctor", &[], &[]);
    assert_eq!(code, Some(1), "{stdout}");
    assert!(stdout.contains("frobnicate"), "{stdout}");
}
