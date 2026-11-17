use super::{ECHO_GROUP, project, run};

#[test]
fn a_plugin_action_runs_and_publishes_a_variable() {
    let dir = project(
        "action",
        r#"Feature: plugin steps
  Scenario: an action publishes a variable
    When I echo "x" as "greeting"
    Then variable "greeting" should be equal to "p-x"
"#,
        ECHO_GROUP,
    );
    let out = run(&dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(stdout.contains("failed: 0"), "{stdout}");
}

#[test]
fn a_plugin_assertion_is_polled_by_the_host() {
    // `not_yet` is the plugin's half of the eventual-assertion contract: one
    // fresh observation per dispatch, the sleep loop owned by the host.
    let dir = project(
        "polling",
        r#"Feature: plugin assertions
  Scenario: the host retries until the plugin is ready
    Given I expect the next assertion to pass within "2" seconds, checking every "10" milliseconds
    Then the echo counter should reach 3
    And variable "echo_attempts" should be equal to "3"
"#,
        ECHO_GROUP,
    );
    let out = run(&dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("failed: 0"), "{stdout}");
}

#[test]
fn an_unarmed_assertion_that_answers_not_yet_fails_immediately() {
    let dir = project(
        "unarmed",
        r"Feature: plugin assertions
  Scenario: no modifier means no second attempt
    Then the echo counter should reach 3
",
        ECHO_GROUP,
    );
    let out = run(&dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("counter is 1 of 3"), "{stdout}");
}

#[test]
fn plugin_diagnostics_appear_in_the_failure_dump() {
    // Invariant 6 generalised past HTTP: evidence always lands in the dump,
    // never behind a debug flag, and inside the file's single output string.
    let dir = project(
        "diagnostics",
        r"Feature: plugin failures
  Scenario: a fatal result carries its evidence
    Then the echo should fail
",
        ECHO_GROUP,
    );
    let out = run(&dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(
        stdout.contains("the echo step was asked to fail"),
        "{stdout}"
    );
    assert!(stdout.contains("echo state"), "{stdout}");
    assert!(stdout.contains("prefix=p-"), "{stdout}");
}

/// Task 11 made a plugin reporting a failed reset into that scenario's
/// failure; nothing exercised it before this test. The fixture fails its
/// reset when its instance config carries `fail_reset: true` — driven by
/// config, not an env var, since `std::env::set_var` is `unsafe` in edition
/// 2024 and process-global, and would race every other test in this binary.
#[test]
fn a_failed_reset_scenario_fails_the_scenario() {
    let dir = project(
        "reset-failure",
        r#"Feature: plugin reset
  Scenario: the first scenario primes the instance
    When I echo "x" as "greeting"

  Scenario: the second scenario's reset fails
    When I echo "y" as "greeting"
"#,
        "  echo:\n    main:\n      prefix: \"p-\"\n      fail_reset: true\n",
    );
    let out = run(&dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(
        stdout.contains("the echo instance refuses to reset"),
        "{stdout}"
    );
    // Exactly the second scenario failed — not both, which Some(1) alone
    // would equally allow.
    assert!(stdout.contains("failed: 1"), "{stdout}");
}

#[test]
fn a_fatal_plugin_reply_is_a_plugin_failure() {
    let dir = project(
        "class-fatal",
        "Feature: f\n  Scenario: s\n    Then the echo should fail\n",
        ECHO_GROUP,
    );
    let stdout = String::from_utf8_lossy(&run(&dir).stdout).into_owned();
    assert!(stdout.contains("failed: 1 (1 plugin)"), "{stdout}");
}

#[test]
fn a_failed_reset_is_a_reset_failure() {
    let dir = project(
        "class-reset",
        "Feature: f\n  Scenario: a\n    When I echo \"x\" as \"g\"\n\n  Scenario: b\n    When I echo \"y\" as \"g\"\n",
        "  echo:\n    main:\n      prefix: \"p-\"\n      fail_reset: true\n",
    );
    let stdout = String::from_utf8_lossy(&run(&dir).stdout).into_owned();
    assert!(stdout.contains("failed: 1 (1 reset)"), "{stdout}");
}

/// The only plugin invariant no other suite can see: `Plugins::shutdown` must
/// run on every exit path, including a failed run, so a plugin's instance
/// never outlives the process. `shutdown_drops_every_live_instance` (unit
/// test, `src/plugin/mod.rs`) calls `shutdown()` directly and so cannot
/// notice a missing call site in `main.rs` — only a real subprocess run can.
/// The fixture writes a marker file from `bddkit_drop_instance` when its
/// instance config carries `drop_log`, the same config-driven trick as
/// `fail_reset`, for the same reason (no env var, no process-global race).
/// The run is made to fail so this covers the path that matters, not only
/// the happy one.
#[test]
fn shutdown_drops_the_instance_even_after_a_failed_run() {
    let dir = project(
        "shutdown",
        r"Feature: plugin failures
  Scenario: a fatal result still leaves shutdown to run
    Then the echo should fail
",
        ECHO_GROUP,
    );
    let marker = dir.join("dropped.marker");
    let _ = std::fs::remove_file(&marker);
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "paths: [features]\nconcurrency: 1\nresources:\n  api: {{}}\n  echo:\n    main:\n      prefix: \"p-\"\n      drop_log: \"{}\"\n",
            marker.display()
        ),
    )
    .expect("rewrite config with drop_log");

    let out = run(&dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(
        marker.exists(),
        "bddkit_drop_instance must run during Plugins::shutdown even after a failed scenario"
    );
}

#[test]
fn a_null_sentinel_argument_crosses_the_boundary_intact() {
    // `<<null>>` interpolates to two literal NUL bytes inside a String. They
    // reach the plugin through `serde_json`, which escapes them on the wire, so
    // the host's own `CString::new(request)` never sees a raw NUL. That was
    // established twice by reading and never by running; if the escaping ever
    // stopped holding, every step whose argument came from `<<null>>` would
    // fail with a bare "payload contains a NUL byte" and nothing would connect
    // it back to the sentinel.
    let dir = project(
        "null-sentinel",
        r#"Feature: the null sentinel
  Scenario: a NUL-bearing argument reaches the plugin
    When I echo "<<null>>" as "echoed"
    Then variable "echoed" should not be equal to ""
"#,
        ECHO_GROUP,
    );
    let out = run(&dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(stdout.contains("failed: 0"), "{stdout}");
    assert!(
        !stderr.contains("NUL byte"),
        "the request must not be rejected for its NUL bytes: {stderr}"
    );
}
