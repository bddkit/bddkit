//! End-to-end acceptance for the plugin layer: the real `bddkit` binary, run
//! as a subprocess, against the fixture plugin built by `common::build_fixture_plugin`.
//! No test here needs the axum stub, so plain `#[test]` is right — nothing
//! spawns a server for `Command::output()` to starve.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    reason = "`allow-*-in-tests` covers only `#[test]` functions, not the helpers here"
)]

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

/// A throwaway project: config, feature file, and the hand-written lock file
/// the run-path loader reads — the hand-written form `plugin install` also
/// writes, exercised by `tests/plugin_cli.rs` instead of here.
fn project(name: &str, feature: &str, config_tail: &str) -> PathBuf {
    project_at(name, feature, config_tail, 1)
}

/// `project` with the run's worker count spelled out. Every test that does not
/// care runs sequentially through `project`; the parallel case needs its own
/// value, and a suite that can only express `concurrency: 1` is exactly why the
/// broadcast reset survived thirteen reviews.
fn project_at(name: &str, feature: &str, config_tail: &str, concurrency: usize) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bddkit-plugin-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("features")).expect("mkdir features");
    std::fs::create_dir_all(dir.join(".bddkit")).expect("mkdir .bddkit");
    std::fs::write(dir.join("features/plugin.feature"), feature).expect("write feature");
    std::fs::write(
        dir.join(".bddkit/plugins.yaml"),
        format!(
            "plugin:\n  - name: echo\n    path: {}\n",
            common::build_fixture_plugin().display()
        ),
    )
    .expect("write lock");
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "paths: [features]\nconcurrency: {concurrency}\nresources:\n  api: {{}}\n{config_tail}"
        ),
    )
    .expect("write config");
    dir
}

/// `project_at` for the `per_worker` fixture: several feature files instead of
/// one, and a lock naming the worker plugin. A separate helper rather than a
/// parameter on `project_at`, so the tests that predate this milestone keep
/// exactly the layout they were written against.
fn worker_project(
    name: &str,
    files: &[(&str, &str)],
    config_tail: &str,
    concurrency: usize,
) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bddkit-plugin-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("features")).expect("mkdir features");
    std::fs::create_dir_all(dir.join(".bddkit")).expect("mkdir .bddkit");
    for (file, body) in files {
        std::fs::write(dir.join("features").join(file), body).expect("write feature");
    }
    std::fs::write(
        dir.join(".bddkit/plugins.yaml"),
        format!(
            "plugin:\n  - name: worker\n    path: {}\n",
            common::build_worker_plugin().display()
        ),
    )
    .expect("write lock");
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "paths: [features]\nconcurrency: {concurrency}\nresources:\n  api: {{}}\n{config_tail}"
        ),
    )
    .expect("write config");
    dir
}

/// A project with BOTH fixtures loaded at once — the only configuration that
/// can catch a registry keyed by a bare handle instead of by `(library, handle)`.
/// Sequential, because echo is `shared` and exports a reset, which `concurrency
/// > 1` refuses at load.
fn two_plugin_project(name: &str, feature: &str, config_tail: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bddkit-plugin-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("features")).expect("mkdir features");
    std::fs::create_dir_all(dir.join(".bddkit")).expect("mkdir .bddkit");
    std::fs::write(dir.join("features/plugin.feature"), feature).expect("write feature");
    std::fs::write(
        dir.join(".bddkit/plugins.yaml"),
        format!(
            "plugin:\n  - name: echo\n    path: {}\n  - name: worker\n    path: {}\n",
            common::build_fixture_plugin().display(),
            common::build_worker_plugin().display()
        ),
    )
    .expect("write lock");
    std::fs::write(
        dir.join("cfg.yaml"),
        format!("paths: [features]\nconcurrency: 1\nresources:\n  api: {{}}\n{config_tail}"),
    )
    .expect("write config");
    dir
}

fn run(dir: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config", "cfg.yaml"])
        // `--config cfg.yaml` has no parent directory component, so the lock
        // is looked for next to it — i.e. inside this project directory.
        .current_dir(dir)
        .output()
        .expect("failed to run bddkit")
}

/// `run` with extra arguments and environment, for the tests that steer the
/// artifact root.
fn run_with(dir: &Path, args: &[&str], envs: &[(&str, &str)]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config", "cfg.yaml"])
        .args(args)
        // A developer's own export must not steer the default-root assertions.
        .env_remove("BDDKIT_ARTIFACTS_DIR")
        .envs(envs.iter().copied())
        .current_dir(dir)
        .output()
        .expect("failed to run bddkit")
}

const DIRECTORIES: &str = r"Feature: directories
  Scenario: two dispatches write evidence
    When I echo the directories
    And I echo the directories
";

fn assert_green(out: &std::process::Output) {
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
}

const ECHO_GROUP: &str = "  echo:\n    main:\n      prefix: \"p-\"\n";

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

#[test]
fn a_group_switch_selects_another_instance_and_resets_per_scenario() {
    let dir = project(
        "switch",
        r#"Feature: instance selection
  Scenario: switch to the other instance
    Given I use "other" echo
    When I echo "x" as "greeting"
    Then variable "greeting" should be equal to "o-x"

  Scenario: the next scenario is back on the default
    When I echo "x" as "greeting"
    Then variable "greeting" should be equal to "p-x"
"#,
        "  echo:\n    main:\n      prefix: \"p-\"\n    other:\n      prefix: \"o-\"\ndefault_echo: main\n",
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

/// The instance a scenario starts on, read back through the echo prefix.
const WHICH_INSTANCE: &str = r#"Feature: default
  Scenario: s
    When I echo "x" as "greeting"
    Then variable "greeting" should be equal to "o-x"
"#;
const TWO_INSTANCES: &str = "  echo:\n    main:\n      prefix: \"p-\"\n    other:\n      prefix: \"o-\"\ndefault_echo: main\n";

#[test]
fn a_default_flag_beats_the_variable_and_the_variable_beats_the_config() {
    let dir = project("default-flag", WHICH_INSTANCE, TWO_INSTANCES);
    // The config says `main`, so the scenario fails until something else decides.
    assert_eq!(run(&dir).status.code(), Some(1));
    let out = run_with(&dir, &[], &[("BDDKIT_DEFAULT_ECHO", "other")]);
    assert_green(&out);
    let out = run_with(
        &dir,
        &["--default", "echo=other"],
        &[("BDDKIT_DEFAULT_ECHO", "main")],
    );
    assert_green(&out);
}

#[test]
fn a_default_for_a_group_or_an_instance_nothing_declares_exits_2() {
    let dir = project("default-flag-unknown", WHICH_INSTANCE, TWO_INSTANCES);
    for (args, named) in [
        (["--default", "mail=inbox"], "mail"),
        (["--default", "echo=third"], "third"),
    ] {
        let out = run_with(&dir, &args, &[]);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(2), "{stderr}");
        assert!(stderr.contains(named), "{stderr}");
    }
}

#[test]
fn doctor_reads_the_default_flag_and_variable_as_run_does() {
    let dir = project("default-doctor", WHICH_INSTANCE, TWO_INSTANCES);
    let doctor = |args: &[&str], envs: &[(&str, &str)]| {
        let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
            .args(["doctor", "--config", "cfg.yaml"])
            .args(args)
            .envs(envs.iter().copied())
            .current_dir(&dir)
            .output()
            .expect("run bddkit");
        (
            String::from_utf8_lossy(&out.stdout).into_owned(),
            out.status.code(),
        )
    };
    let (text, code) = doctor(&["--default", "echo=other"], &[]);
    assert_eq!(code, Some(0), "{text}");
    let (text, code) = doctor(&[], &[("BDDKIT_DEFAULT_ECHO", "third")]);
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("third"), "{text}");
}

#[test]
fn a_config_group_no_plugin_serves_exits_2() {
    let dir = project(
        "unknown-group",
        "Feature: f\n  Scenario: s\n    When I echo \"x\" as \"g\"\n",
        "  echo:\n    main:\n      prefix: \"p-\"\n  mail:\n    inbox:\n      host: localhost\n",
    );
    let out = run(&dir);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("mail"), "{stderr}");
    assert!(
        stderr.contains("no installed plugin serves the group"),
        "{stderr}"
    );
    assert!(stderr.contains("run not started"), "{stderr}");
}

#[test]
fn an_instance_the_plugin_rejects_exits_2_before_the_first_request() {
    let dir = project(
        "bad-instance",
        "Feature: f\n  Scenario: s\n    When I echo \"x\" as \"g\"\n",
        "  echo:\n    main:\n      bucket: no-prefix-here\n",
    );
    let out = run(&dir);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("prefix"), "{stderr}");
    assert!(stderr.contains("resources.echo.main"), "{stderr}");
}

#[test]
fn an_unknown_plugin_step_is_reported_before_the_run_starts() {
    // This test only proves the second half of invariant 1: a step nothing
    // declares — plugin included — is rejected by validate::check before the
    // first request. It does NOT prove the plugin's own patterns are among
    // what validate::check can match; that ordering is covered instead by
    // every test above that runs a real plugin step (e.g.
    // a_plugin_action_runs_and_publishes_a_variable), which would fail
    // validation first if plugin patterns were missing from the registry.
    let dir = project(
        "unknown-step",
        "Feature: f\n  Scenario: s\n    When I yodel \"x\"\n",
        ECHO_GROUP,
    );
    let out = run(&dir);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("unknown step"), "{stderr}");
    assert!(stderr.contains("I yodel"), "{stderr}");
}

#[test]
fn a_near_miss_of_a_plugin_step_suggests_its_template() {
    let dir = project(
        "near-miss",
        "Feature: f\n  Scenario: s\n    When I echo \"x\" to \"greeting\"\n",
        ECHO_GROUP,
    );
    let out = run(&dir);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains(r#"did you mean: I echo "<value>" as "<variable>""#),
        "{stderr}"
    );
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

/// The Critical of the final branch review. A `shared` instance is one per run,
/// so `Plugins::reset_scenario` at one worker's scenario boundary reaches every
/// instance — including one another worker is mid-scenario with. Reproduced
/// against the built binary as an assertion whose counter kept being zeroed by
/// an unrelated file, and as a file with no plugin steps at all failing on
/// another file's reset. `per_worker` scopes an instance to one file and is
/// therefore allowed at any concurrency — it is the remedy this refusal names;
/// `shared` has no such escape, so the pairing is still refused at load.
#[test]
fn a_plugin_with_a_per_scenario_reset_is_refused_under_parallelism() {
    let dir = project_at(
        "parallel-reset",
        r#"Feature: plugin steps
  Scenario: an action publishes a variable
    When I echo "x" as "greeting"
"#,
        ECHO_GROUP,
        2,
    );
    // A second file, so the run really would have two workers resetting one
    // shared instance had it started.
    std::fs::write(
        dir.join("features/second.feature"),
        "Feature: no plugin steps at all\n  Scenario: one\n    Given I am not in debug mode\n",
    )
    .expect("write second feature");

    let out = run(&dir);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("\"echo\""), "{stderr}");
    assert!(stderr.contains("bddkit_reset_scenario"), "{stderr}");
    assert!(stderr.contains("concurrency: 1"), "{stderr}");
    // The other half of the message, and the half this milestone made true:
    // the mode that fixes this is named, not merely hinted at.
    assert!(
        stderr.contains("per_worker"),
        "the remedy is named: {stderr}"
    );
    assert!(stderr.contains("run not started"), "{stderr}");
}

/// The same suite, sequentially: the refusal must not cost the mode that works.
#[test]
fn the_same_plugin_runs_when_the_run_is_sequential() {
    let dir = project_at(
        "sequential-reset",
        r#"Feature: plugin steps
  Scenario: an action publishes a variable
    When I echo "x" as "greeting"
    Then variable "greeting" should be equal to "p-x"
"#,
        ECHO_GROUP,
        1,
    );
    let out = run(&dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `check_group_defaults` turns every unread top-level `default_*` key into an
/// exit 2, which is right for catching `default_widgte` where `default_widget` was
/// meant — but `Config` has never had `deny_unknown_fields`, so a suite written
/// before plugins existed may well carry one. The check is gated on a plugin
/// being loaded: with none there are no resource groups for such a key to be a
/// typo of.
#[test]
fn a_plugin_free_config_tolerates_an_unrelated_default_key() {
    let dir = project(
        "no-plugin",
        "Feature: f\n  Scenario: s\n    Given I am not in debug mode\n",
        "",
    );
    std::fs::remove_file(dir.join(".bddkit/plugins.yaml")).expect("no plugin installed");
    std::fs::write(
        dir.join("cfg.yaml"),
        "paths: [features]\nconcurrency: 1\ndefault_timeout: 30\nresources:\n  api: {}\n",
    )
    .expect("write config");

    let out = run(&dir);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "--- stdout ---\n{}\n--- stderr ---\n{stderr}",
        String::from_utf8_lossy(&out.stdout)
    );
}

const WORKER_GROUP: &str = "  worker:\n    main: {}\n";

/// The gate of this milestone. Two files, two workers, one plugin declaring
/// `per_worker`: each file must get its own instance, proven by the instance's
/// own counter rather than by handle arithmetic — a shared counter makes one of
/// the two files read back 2.
#[test]
fn two_files_get_different_per_worker_instances() {
    let dir = worker_project(
        "distinct-instances",
        &[
            (
                // Two plugin steps, not one: with a single step per file the
                // whole suite still passes when `call_step` ignores the cached
                // handle and re-resolves, because each file's one step lands on
                // a fresh instance that counts to 1 either way. The second step
                // is what makes "one instance per file" mean one.
                "a.feature",
                "Feature: a\n  Scenario: s\n    When I count in the worker as \"n\"\n    Then variable \"n\" should be equal to \"1\"\n    When I count in the worker as \"m\"\n    Then variable \"m\" should be equal to \"2\"\n",
            ),
            (
                "b.feature",
                "Feature: b\n  Scenario: s\n    When I count in the worker as \"n\"\n    Then variable \"n\" should be equal to \"1\"\n",
            ),
        ],
        WORKER_GROUP,
        2,
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

/// A `per_worker` instance is dropped when ITS FILE ends, not when the run does.
///
/// Counting the drop records after the process has exited cannot see that
/// difference: `Plugins::shutdown` sweeps every registered handle on the way
/// out, so a host that dropped nothing per file still leaves one record per
/// instance behind. The lifetime is only observable while the run is still in
/// flight, so this test watches the log from outside: one file finishes, a
/// second is still sleeping, and the first file's drop must already be on
/// disk. The child is killed as soon as that is seen, so the sleep costs the
/// suite nothing on the happy path.
#[test]
fn a_per_worker_instance_is_dropped_when_its_file_ends() {
    // The pair is load-bearing and must stay far apart: the observation has to
    // land inside the sleep, or the run ends and proves nothing. Measured cost
    // of the observation is ~0.2s, unchanged under CPU oversubscription and on
    // a single core, so the margin is roughly fifty-fold.
    const HOLD_SECS: u64 = 20;
    const OBSERVE_SECS: u64 = 10;

    let log = std::env::temp_dir().join(format!("bddkit-worker-drops-{}", std::process::id()));
    let _ = std::fs::remove_file(&log);
    let holder = format!(
        "Feature: b\n  Scenario: s\n    When I count in the worker as \"n\"\n    And I sleep \"{HOLD_SECS}\" seconds\n"
    );
    let dir = worker_project(
        "drop-per-file",
        &[
            (
                "a.feature",
                "Feature: a\n  Scenario: s\n    When I count in the worker as \"n\"\n",
            ),
            // Its plugin step runs first, so this file owns an instance too —
            // one it holds for the whole sleep, which is what keeps the run
            // alive long enough for the other file's drop to be visible.
            ("b.feature", holder.as_str()),
        ],
        &format!(
            "  worker:\n    main:\n      drop_log: \"{}\"\n",
            log.display()
        ),
        2,
    );

    let mut child = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config", "cfg.yaml"])
        .current_dir(&dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn bddkit");

    // Well inside the 20-second sleep: a host that only drops at shutdown
    // cannot have written anything by now, and a host that drops per file has
    // written the first file's record within a fraction of a second.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(OBSERVE_SECS);
    let mut recorded = 0;
    let mut ended = false;
    while std::time::Instant::now() < deadline {
        ended = child.try_wait().expect("wait on bddkit").is_some();
        recorded = std::fs::read_to_string(&log).map_or(0, |text| text.lines().count());
        if recorded > 0 || ended {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let _ = child.kill();
    let out = child.wait_with_output().expect("collect bddkit output");
    let _ = std::fs::remove_file(&log);

    assert!(
        !ended,
        "the run ended before its drops could be observed mid-run\n--- stdout ---\n{}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // A failed dispatch also drops the instance it just created
    // (`Plugins::call_step`), so without this the test could go green for the
    // opposite reason: nothing worked, and the cleanup path wrote the record.
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("FAIL"),
        "the run must be healthy\n{stderr}"
    );
    assert_eq!(
        recorded, 1,
        "the finished file's instance must be dropped while the other file still holds its own\n--- stderr ---\n{stderr}"
    );
}

/// Runs one file that holds its plugin instance for 5 seconds, sends it
/// `signals` in order (each `kill -<name>`, a short pause between them) and
/// returns the process output with the number of drop records the worker
/// fixture wrote. The file is deliberately never allowed to reach its own end,
/// so the only thing that can have written a drop record is the interrupt
/// handler's cleanup sweep.
#[cfg(unix)]
fn interrupted_run(
    name: &str,
    signals: &[&str],
    drop_delay_ms: u64,
) -> (std::process::Output, usize) {
    const HOLD_SECS: u64 = 5;

    let log = std::env::temp_dir().join(format!("bddkit-{name}-drops-{}", std::process::id()));
    let _ = std::fs::remove_file(&log);
    let dir = worker_project(
        name,
        &[(
            "a.feature",
            &format!(
                "Feature: a\n  Scenario: s\n    When I count in the worker as \"n\"\n    And I sleep \"{HOLD_SECS}\" seconds\n"
            ),
        )],
        &format!(
            "  worker:\n    main:\n      drop_log: \"{}\"\n      drop_delay_ms: {drop_delay_ms}\n",
            log.display()
        ),
        1,
    );

    let mut child = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config", "cfg.yaml"])
        .current_dir(&dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn bddkit");

    // Comfortably inside the 5-second sleep: the plugin step dispatches in a
    // fraction of that, so this margin only has to outlast process startup
    // (config load, plugin dylib load, worker spawn) before the sleep begins.
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(
        child.try_wait().expect("poll bddkit").is_none(),
        "the run ended before it could be interrupted"
    );

    for (i, signal) in signals.iter().enumerate() {
        if i > 0 {
            // Long enough for the first signal's handler to be inside the
            // cleanup, well short of the fixture's drop delay.
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
        let status = Command::new("kill")
            .args([&format!("-{signal}"), &child.id().to_string()])
            .status()
            .expect("failed to send a signal");
        assert!(status.success(), "kill -{signal} itself failed");
    }

    let out = child.wait_with_output().expect("collect bddkit output");
    let recorded = std::fs::read_to_string(&log).map_or(0, |text| text.lines().count());
    let _ = std::fs::remove_file(&log);
    (out, recorded)
}

/// Issue #62 and #85: a stop signal mid-run must stop promptly with 128 plus
/// the signal number (not the OS's default "killed by signal" disposition)
/// and still run `Plugins::shutdown` on the way out.
#[cfg(unix)]
fn assert_interrupt_drops_instances(name: &str, signal: &str, code: i32) {
    let (out, recorded) = interrupted_run(name, &[signal], 0);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert_eq!(
        out.status.code(),
        Some(code),
        "--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(
        stderr.contains(&format!("interrupted by SIG{signal}")),
        "{stderr}"
    );
    assert_eq!(
        recorded, 1,
        "Plugins::shutdown must drop the in-flight instance on interrupt\n--- stderr ---\n{stderr}"
    );
}

#[test]
#[cfg(unix)]
fn sigint_stops_new_work_and_still_drops_plugin_instances() {
    assert_interrupt_drops_instances("sigint", "INT", 130);
}

#[test]
#[cfg(unix)]
fn sigterm_exits_143_and_still_drops_plugin_instances() {
    assert_interrupt_drops_instances("sigterm", "TERM", 143);
}

#[test]
#[cfg(unix)]
fn sighup_exits_129_and_still_drops_plugin_instances() {
    assert_interrupt_drops_instances("sighup", "HUP", 129);
}

/// A second signal skips the cleanup — the fixture's 10-second drop delay
/// would otherwise outlast the whole test — and the exit code stays the
/// FIRST signal's: SIGTERM ended the run, the SIGINT only hurried it.
#[test]
#[cfg(unix)]
fn a_second_signal_exits_with_the_code_of_the_first() {
    let (out, recorded) = interrupted_run("second-signal", &["TERM", "INT"], 10_000);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert_eq!(out.status.code(), Some(143), "{stderr}");
    assert!(stderr.contains("second interrupt (SIGINT)"), "{stderr}");
    assert_eq!(recorded, 0, "the cleanup must not have finished\n{stderr}");
}

/// The restriction this milestone lifts: under P1 a plugin exporting
/// `bddkit_reset_scenario` refused to load at any concurrency above 1. The
/// second scenario of `a.feature` is what proves the reset actually ran — its
/// counter is back to 1, not 2.
#[test]
fn a_per_worker_plugin_with_a_reset_runs_in_parallel() {
    let dir = worker_project(
        "reset-in-parallel",
        &[
            (
                "a.feature",
                "Feature: a\n  Scenario: one\n    When I count in the worker as \"n\"\n    Then variable \"n\" should be equal to \"1\"\n  Scenario: two\n    When I count in the worker as \"n\"\n    Then variable \"n\" should be equal to \"1\"\n",
            ),
            (
                "b.feature",
                "Feature: b\n  Scenario: one\n    When I count in the worker as \"n\"\n    Then variable \"n\" should be equal to \"1\"\n",
            ),
        ],
        WORKER_GROUP,
        2,
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
fn two_plugins_keep_separate_instances_and_are_both_swept() {
    // Both fixtures start their handle tables at the same number, so each hands
    // back handle 1. Keyed by a bare `u64`, the second registration overwrites
    // the first: the worker's per-file drop then removes the only entry, and
    // `shutdown` finds nothing left to sweep, so echo's instance is never
    // dropped and its log stays empty. Keyed by `(library, handle)` — which is
    // what `Plugins::registry` does — the two never collide. No other
    // configuration in this suite can tell those apart, because no other one
    // loads two plugins.
    let echo_log = std::env::temp_dir().join(format!("bddkit-two-echo-{}", std::process::id()));
    let worker_log = std::env::temp_dir().join(format!("bddkit-two-worker-{}", std::process::id()));
    let _ = std::fs::remove_file(&echo_log);
    let _ = std::fs::remove_file(&worker_log);

    let dir = two_plugin_project(
        "two-plugins",
        r#"Feature: two plugins at once
  Scenario: each group answers from its own instance
    When I echo "x" as "greeting"
    Then variable "greeting" should be equal to "p-x"
    When I count in the worker as "n"
    Then variable "n" should be equal to "1"
"#,
        &format!(
            "  echo:\n    main:\n      prefix: \"p-\"\n      drop_log: \"{}\"\n  worker:\n    main:\n      drop_log: \"{}\"\n",
            echo_log.display(),
            worker_log.display()
        ),
    );
    let out = run(&dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(stdout.contains("failed: 0"), "{stdout}");

    let echo_dropped = std::fs::read_to_string(&echo_log);
    let worker_dropped = std::fs::read_to_string(&worker_log);
    let _ = std::fs::remove_file(&echo_log);
    let _ = std::fs::remove_file(&worker_log);
    assert!(
        echo_dropped.is_ok(),
        "the shared instance must be swept by shutdown, not lost to a handle collision\n{stderr}"
    );
    assert!(
        worker_dropped.is_ok(),
        "the per-file instance must be dropped when its file ends\n{stderr}"
    );
}

/// `bddkit steps list` without a config knows only the builtins — a plugin's
/// vocabulary lives inside a `cdylib` the host has to load to see at all.
#[test]
fn steps_list_shows_a_plugins_group_only_once_a_config_names_it() {
    let dir = project("steps-list", "Feature: f\n", ECHO_GROUP);

    let bare = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["steps", "list"])
        .current_dir(&dir)
        .output()
        .expect("run bddkit");
    let bare_out = String::from_utf8_lossy(&bare.stdout);
    assert_eq!(bare.status.code(), Some(0), "{bare_out}");
    assert!(
        !bare_out.contains("echo:"),
        "without --config no plugin is loaded, so it has no steps to show:\n{bare_out}"
    );

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["steps", "list", "echo", "--config", "cfg.yaml", "-v"])
        .current_dir(&dir)
        .output()
        .expect("run bddkit");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stdout}\n{stderr}");
    assert!(stdout.contains("echo:"), "{stdout}");
    assert!(
        stdout.contains(r#"I echo "<value>" as "<variable>""#),
        "a plugin's named groups render like a builtin's:\n{stdout}"
    );
    assert!(
        stdout.contains("echoes its argument"),
        "the description the plugin declared must be shown:\n{stdout}"
    );
    assert!(
        stdout.contains(r#"I use "<name>" echo"#),
        "the group-switch step exists once a plugin claims the group:\n{stdout}"
    );
    // The fixture's third step declares neither a name nor a description, which
    // is what an older plugin looks like: it must list, not break the listing.
    assert!(
        stdout.contains("  the echo should fail\n  "),
        "a step without a description is followed by the next step, not a blank line:\n{stdout}"
    );
}

#[test]
fn resource_fields_describes_a_plugins_group_only_with_a_config() {
    let dir = project("resource-fields", "Feature: f\n", ECHO_GROUP);

    let bare = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["resource", "fields"])
        .current_dir(&dir)
        .output()
        .expect("run bddkit");
    let bare_out = String::from_utf8_lossy(&bare.stdout);
    assert_eq!(bare.status.code(), Some(0), "{bare_out}");
    assert!(
        !bare_out.contains("echo:"),
        "without --config no plugin is loaded, so it describes nothing:\n{bare_out}"
    );

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["resource", "fields", "echo", "--config", "cfg.yaml"])
        .current_dir(&dir)
        .output()
        .expect("run bddkit");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stdout}\n{stderr}");
    assert!(stdout.contains("echo:"), "{stdout}");
    assert!(
        stdout.contains("prefix") && stdout.contains("required"),
        "the manifest's own description reaches the reader:\n{stdout}"
    );
    assert!(
        stdout.contains("(e.g. p-)"),
        "an example is worth more than the sentence describing it:\n{stdout}"
    );
}

/// The write half of the same contract: a plugin group's field list is what a
/// `--flag` is checked against, and the plugin's own `bddkit_probe_config` is
/// what decides whether the resource is written.
#[test]
fn resource_add_writes_a_plugin_group_it_can_probe() {
    let dir = project(
        "resource-add",
        "Feature: f\n",
        &format!("{ECHO_GROUP}default_echo: main\n"),
    );
    let cfg = dir.join("cfg.yaml");
    let before = std::fs::read_to_string(&cfg).expect("read config");

    let add = |args: &[&str]| {
        let argv: Vec<&str> = ["resource", "add", "echo"]
            .iter()
            .chain(args.iter())
            .copied()
            .collect();
        let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
            .args(&argv)
            .current_dir(&dir)
            .output()
            .expect("run bddkit");
        (
            format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ),
            out.status.code(),
        )
    };

    // Only the plugin knows what its group takes, so the manifest's `fields`
    // is the list a typo is caught against.
    let (out, code) = add(&["second", "--config", "cfg.yaml", "--prefx", "q-"]);
    assert_eq!(code, Some(1), "{out}");
    assert!(
        out.contains("prefx") && out.contains("prefix"),
        "the typo is named beside the names the manifest declares:\n{out}"
    );

    // The probe runs by default here, and the plugin's own answer is final.
    let (out, code) = add(&[
        "second",
        "--config",
        "cfg.yaml",
        "--prefix",
        "q-",
        "--probe_error",
        "the bucket is not there",
    ]);
    assert_eq!(code, Some(1), "{out}");
    assert!(out.contains("the bucket is not there"), "{out}");
    assert_eq!(
        std::fs::read_to_string(&cfg).expect("read config"),
        before,
        "a failed probe writes nothing"
    );

    let (out, code) = add(&["second", "--config", "cfg.yaml", "--prefix", "q-"]);
    assert_eq!(code, Some(0), "{out}");
    assert_eq!(
        std::fs::read_to_string(&cfg).expect("read config"),
        before.replace("  echo:\n", "  echo:\n    second:\n      prefix: q-\n"),
        "the file gains the block and nothing else moves"
    );
}

/// A resource name is written to the file literally and read back `${VAR}`-
/// expanded, so a name carrying a variable that is set is not in the loaded
/// config under the name being written. Built-in kinds refuse it; a plugin
/// group used to write the literal key and exit 0.
#[test]
fn resource_add_refuses_a_plugin_group_name_that_expands_to_something_else() {
    let dir = project(
        "resource-add-expanding-name",
        "Feature: f\n",
        &format!("{ECHO_GROUP}default_echo: main\n"),
    );
    let cfg = dir.join("cfg.yaml");
    let before = std::fs::read_to_string(&cfg).expect("read config");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "resource",
            "add",
            "echo",
            "${BDDKIT_TEST_NAME}",
            "--config",
            "cfg.yaml",
            "--prefix",
            "q-",
            "--no-check",
        ])
        .env("BDDKIT_TEST_NAME", "expanded")
        .current_dir(&dir)
        .output()
        .expect("run bddkit");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert!(text.contains("${VAR}"), "{text}");
    assert_eq!(
        std::fs::read_to_string(&cfg).expect("read config"),
        before,
        "nothing was written"
    );
}

/// The failure issue #28 is about, end to end: the fixture's `loud` is a
/// boolean and its `validate_config` refuses anything else, so before a
/// manifest could declare a type the host wrote `'false'` and the plugin
/// refused it on every input. The write is the assertion — `loud: false`
/// unquoted — because a string would still be a string with the quotes gone.
#[test]
fn a_declared_boolean_field_is_written_as_a_boolean() {
    let dir = project(
        "resource-add-typed",
        "Feature: f\n",
        &format!("{ECHO_GROUP}default_echo: main\n"),
    );
    let cfg = dir.join("cfg.yaml");
    let before = std::fs::read_to_string(&cfg).expect("read config");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "resource", "add", "echo", "second", "--config", "cfg.yaml", "--prefix", "q-",
            "--loud", "false",
        ])
        .current_dir(&dir)
        .output()
        .expect("run bddkit");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert_eq!(
        std::fs::read_to_string(&cfg).expect("read config"),
        before.replace(
            "  echo:\n",
            "  echo:\n    second:\n      prefix: q-\n      loud: false\n"
        ),
        "the boolean reaches the file as a boolean"
    );

    // And the value still has to be one of the two: the type says what form
    // the value takes, never that any text will do.
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "resource",
            "add",
            "echo",
            "third",
            "--config",
            "cfg.yaml",
            "--prefix",
            "q-",
            "--loud",
            "sometimes",
        ])
        .current_dir(&dir)
        .output()
        .expect("run bddkit");
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert!(text.contains("loud"), "{text}");
}

#[test]
fn doctor_probes_a_declared_instance_only_under_live() {
    let dir = project(
        "doctor-probe",
        "Feature: f\n  Scenario: s\n    Given I am in debug mode\n",
        ECHO_GROUP,
    );

    let doctor = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
            .args(args)
            .current_dir(&dir)
            .output()
            .expect("run bddkit");
        (
            String::from_utf8_lossy(&out.stdout).into_owned(),
            out.status.code(),
        )
    };

    let (static_out, code) = doctor(&["doctor", "--config", "cfg.yaml"]);
    assert_eq!(code, Some(0), "{static_out}");
    assert!(
        static_out.contains("echo main") && static_out.contains("skipped"),
        "a bare doctor must not call the plugin's probe:\n{static_out}"
    );

    let (live_out, code) = doctor(&["doctor", "--config", "cfg.yaml", "--live"]);
    assert_eq!(code, Some(0), "{live_out}");
    assert!(
        live_out.contains("echo main") && live_out.contains("probed clean"),
        "{live_out}"
    );
}

const TWO_REFUSED: &str =
    "  echo:\n    a: {}\n    b: {}\n    c:\n      prefix: \"p-\"\ndefault_echo: c\n";

#[test]
fn doctor_and_run_report_every_refused_instance() {
    let dir = project(
        "refused-all",
        "Feature: f\n  Scenario: s\n    Given I am in debug mode\n",
        TWO_REFUSED,
    );
    let bddkit = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
            .args(args)
            .current_dir(&dir)
            .output()
            .expect("run bddkit");
        (
            format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ),
            out.status.code(),
        )
    };

    let (text, code) = bddkit(&["run", "--config", "cfg.yaml"]);
    assert_eq!(code, Some(2), "{text}");
    assert!(
        text.contains("resources.echo.a") && text.contains("resources.echo.b"),
        "{text}"
    );

    let (text, code) = bddkit(&["doctor", "--config", "cfg.yaml"]);
    assert_eq!(code, Some(1), "{text}");
    assert!(
        text.contains("✗ echo a") && text.contains("✗ echo b"),
        "each refusal has its own row:\n{text}"
    );
    assert!(
        text.contains("steps"),
        "one refused instance must not skip the steps stage:\n{text}"
    );

    let (live, _) = bddkit(&["doctor", "--config", "cfg.yaml", "--live"]);
    assert!(
        live.contains("✓ echo c") && live.contains("probed clean"),
        "an accepted instance beside refused ones is still probed:\n{live}"
    );
    assert!(
        live.contains("not probed: refused"),
        "a refused instance says it was not probed:\n{live}"
    );

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config", "cfg.yaml", "--json"])
        .current_dir(&dir)
        .output()
        .expect("run bddkit");
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("doctor --json");
    let rows = json["checks"].as_array().expect("checks");
    for instance in ["a", "b"] {
        assert!(
            rows.iter().any(|c| c["stage"] == "echo"
                && c["target"] == instance
                && c["status"] == "failed"),
            "stage is the group, target the instance: {json}"
        );
    }
}

#[test]
fn doctor_bddkit_dir_lists_every_candidate_and_names_each_entry_layer() {
    let dir = project(
        "doctor-dirs",
        "Feature: f\n  Scenario: s\n    Given I am not in debug mode\n",
        ECHO_GROUP,
    );
    std::fs::write(
        dir.join(".bddkit/plugins.local.yaml"),
        "plugin:\n  - name: ghost\n    path: /nonexistent/libghost.so\n",
    )
    .expect("write local lock");
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config", "cfg.yaml", "--bddkit-dir", ".bddkit"])
        .current_dir(&dir)
        .output()
        .expect("run bddkit");
    let stdout = String::from_utf8_lossy(&out.stdout);
    // The override directory itself, found.
    assert!(
        stdout.contains("override") && stdout.contains("plugins.yaml"),
        "{stdout}"
    );
    // The local file in the same directory, found.
    assert!(
        stdout.contains("override.local") && stdout.contains("plugins.local.yaml"),
        "{stdout}"
    );
    // The committed echo plugin entry, resolved and present.
    assert!(stdout.contains("echo"), "{stdout}");
    // The local-only ghost entry, present in the listing but its .so is missing.
    assert!(stdout.contains("ghost"), "{stdout}");
}

#[test]
fn a_plugin_that_refuses_the_probe_fails_doctor_naming_the_instance() {
    // The fixture's probe answers with whatever `probe_error` names, which is
    // how a real plugin reports an endpoint that would not have it.
    let dir = project(
        "doctor-probe-fail",
        "Feature: f\n  Scenario: s\n    Given I am in debug mode\n",
        "  echo:\n    main:\n      prefix: \"p-\"\n      probe_error: \"bucket acceptance does not exist\"\n",
    );
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config", "cfg.yaml", "--live"])
        .current_dir(&dir)
        .output()
        .expect("run bddkit");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("echo main"), "{stdout}");
    assert!(
        stdout.contains("bucket acceptance does not exist"),
        "{stdout}"
    );
}

/// The zero-config case of issue #37: the echo manifest declares an
/// `implicit_instance` body for its group, so a config with no
/// `resources.echo` section at all still has an instance named `default` —
/// built from that body, validated at load, and the group default by the
/// one-instance inference rule. The prefix proves the declared body reached
/// `init_instance`; the explicit switch proves the name.
#[test]
fn a_group_with_no_config_section_runs_on_the_implicit_instance() {
    let dir = project(
        "implicit-instance",
        r#"Feature: implicit instance
  Scenario: the default is synthesized from the manifest
    When I echo "x" as "v"
    Then variable "v" should be equal to "implicit-x"
    When I use "default" echo
    And I echo "y" as "w"
    Then variable "w" should be equal to "implicit-y"
"#,
        "",
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

/// The motivating case, in its motivating shape: a `per_worker` plugin whose
/// every key has a default, two files, two workers, and no `resources.worker`
/// section anywhere.
#[test]
fn a_per_worker_plugin_runs_zero_config_on_its_implicit_instance() {
    let dir = worker_project(
        "implicit-per-worker",
        &[
            (
                "a.feature",
                "Feature: a\n  Scenario: s\n    When I count in the worker as \"n\"\n    Then variable \"n\" should be equal to \"1\"\n",
            ),
            (
                "b.feature",
                "Feature: b\n  Scenario: s\n    When I count in the worker as \"n\"\n    Then variable \"n\" should be equal to \"1\"\n",
            ),
        ],
        "",
        2,
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

/// Any declared section — an empty map included — switches the synthesis off:
/// a suite that names its instances gets exactly the ones it named.
#[test]
fn a_declared_section_disables_the_implicit_instance() {
    let feature = "Feature: f\n  Scenario: s\n    When I use \"default\" echo\n";
    for (name, tail) in [
        ("implicit-off", ECHO_GROUP),
        ("implicit-off-empty", "  echo: {}\n"),
    ] {
        let dir = project(name, feature, tail);
        let out = run(&dir);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert_eq!(out.status.code(), Some(1), "{tail}\n{stdout}");
        assert!(
            stdout.contains("instance \"default\" is not declared in resources.echo"),
            "{tail}\n{stdout}"
        );
    }
}

/// A body the plugin's own `validate_config` refuses is a plugin bug, and
/// load time is when to find it: exit 2, before the first request. The worker
/// fixture refuses every config while this variable is set on the CHILD (it is
/// never set in the test process — `std::env::set_var` is unsafe in edition
/// 2024 and would race every other test).
#[test]
fn an_implicit_instance_the_plugin_rejects_exits_2_before_the_first_request() {
    let dir = worker_project(
        "implicit-rejected",
        &[(
            "a.feature",
            "Feature: a\n  Scenario: s\n    When I count in the worker as \"n\"\n",
        )],
        "",
        1,
    );
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config", "cfg.yaml"])
        .env("BDDKIT_WORKER_FIXTURE_REJECT_CONFIG", "1")
        .current_dir(&dir)
        .output()
        .expect("failed to run bddkit");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("implicit_instance.worker") && stderr.contains("rejected by the fixture"),
        "{stderr}"
    );
}

/// `echo: {}` switches the implicit instance off and leaves the group with
/// nothing to run on. `run` accepts the config and fails at the first echo step;
/// a `doctor` that stays green there certifies a suite that cannot pass.
#[test]
fn doctor_reports_a_plugin_step_whose_group_declares_no_instance() {
    let dir = project(
        "doctor-empty-group",
        "Feature: f\n  Scenario: s\n    When I echo \"x\" as \"greeting\"\n",
        "  echo: {}\n",
    );
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config", "cfg.yaml"])
        .current_dir(&dir)
        .output()
        .expect("run bddkit");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(
        stdout.contains("plugin.feature:3") && stdout.contains("resources.echo declares none"),
        "{stdout}"
    );
}

#[test]
fn doctor_lists_the_implicit_instance_as_declared() {
    let dir = project(
        "doctor-implicit",
        "Feature: f\n  Scenario: s\n    Given I am in debug mode\n",
        "",
    );
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config", "cfg.yaml", "--live"])
        .current_dir(&dir)
        .output()
        .expect("run bddkit");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(
        stdout.contains("echo default") && stdout.contains("probed clean"),
        "{stdout}"
    );
}

/// A run's lock directory can be pointed at explicitly. The project has a
/// working lock in `.bddkit/`; every assertion below is that the override
/// wins over it, alone.
#[test]
fn the_bddkit_dir_override_reads_only_that_directory() {
    let dir = project(
        "override",
        "Feature: f\n  Scenario: s\n    When I echo \"x\" as \"greeting\"\n",
        ECHO_GROUP,
    );
    let empty = dir.join("empty");
    std::fs::create_dir_all(&empty).expect("mkdir empty");

    // Flag: the project's own `.bddkit/plugins.yaml` is skipped, so no plugin
    // serves the `echo` group.
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config", "cfg.yaml", "--bddkit-dir", "empty"])
        .current_dir(&dir)
        .output()
        .expect("run bddkit");
    assert_eq!(
        out.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("echo"));

    // Variable: same effect.
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config", "cfg.yaml"])
        .env("BDDKIT_DIR", &empty)
        .current_dir(&dir)
        .output()
        .expect("run bddkit");
    assert_eq!(
        out.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Flag over variable: the variable names the empty directory, the flag the
    // real one, and the run passes.
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config", "cfg.yaml", "--bddkit-dir", ".bddkit"])
        .env("BDDKIT_DIR", &empty)
        .current_dir(&dir)
        .output()
        .expect("run bddkit");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn a_missing_bddkit_dir_is_an_error_not_an_empty_plugin_list() {
    let dir = project(
        "override-missing",
        "Feature: f\n  Scenario: s\n    Given I am not in debug mode\n",
        "",
    );
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "run",
            "--config",
            "cfg.yaml",
            "--bddkit-dir",
            "does-not-exist",
        ])
        .current_dir(&dir)
        .output()
        .expect("run bddkit");
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("does-not-exist") && stderr.contains("does not exist"),
        "{stderr}"
    );
}

#[test]
fn plugins_local_yaml_overrides_the_committed_entry() {
    let dir = project(
        "local",
        "Feature: f\n  Scenario: s\n    When I echo \"x\" as \"greeting\"\n",
        ECHO_GROUP,
    );
    // The committed file points at a vendored path that does not exist here;
    // the developer's local file points at the real build.
    std::fs::rename(
        dir.join(".bddkit/plugins.yaml"),
        dir.join(".bddkit/plugins.local.yaml"),
    )
    .expect("rename to local");
    std::fs::write(
        dir.join(".bddkit/plugins.yaml"),
        "plugin:\n  - name: echo\n    path: vendor/libecho_plugin.so\n",
    )
    .expect("write committed lock");
    let out = run(&dir);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn the_project_layer_is_found_walking_up_from_the_config() {
    let dir = project(
        "walkup",
        "Feature: f\n  Scenario: s\n    When I echo \"x\" as \"greeting\"\n",
        ECHO_GROUP,
    );
    // Move the suite one level down; `.bddkit/` stays at the root.
    let suite = dir.join("suites/a");
    std::fs::create_dir_all(&suite).expect("mkdir suite");
    std::fs::rename(dir.join("features"), suite.join("features")).expect("move features");
    std::fs::rename(dir.join("cfg.yaml"), suite.join("cfg.yaml")).expect("move config");
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config", "cfg.yaml"])
        .current_dir(&suite)
        .output()
        .expect("run bddkit");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn artifacts_dir_roots_every_plugin_artifact() {
    let dir = project("artifacts-flag", DIRECTORIES, ECHO_GROUP);
    // Relative: resolved against the working directory.
    assert_green(&run_with(&dir, &["--artifacts-dir", "evidence"], &[]));
    for counter in ["000000", "000001"] {
        let file = dir.join("evidence").join(counter).join("artifact.txt");
        assert!(file.is_file(), "missing {}", file.display());
    }
}

#[test]
fn the_artifacts_dir_can_come_from_the_environment_and_the_flag_wins() {
    let dir = project("artifacts-env", DIRECTORIES, ECHO_GROUP);
    assert_green(&run_with(
        &dir,
        &[],
        &[("BDDKIT_ARTIFACTS_DIR", "from-env")],
    ));
    assert!(dir.join("from-env/000000/artifact.txt").is_file());

    let out = run_with(
        &dir,
        &["--artifacts-dir", "from-flag"],
        &[("BDDKIT_ARTIFACTS_DIR", "ignored")],
    );
    assert_green(&out);
    assert!(dir.join("from-flag/000000/artifact.txt").is_file());
    assert!(
        !dir.join("ignored").exists(),
        "the flag must win over the environment"
    );

    // An empty value is unset, as for BDDKIT_EVENTS: the default root applies.
    let out = run_with(&dir, &[], &[("BDDKIT_ARTIFACTS_DIR", "")]);
    assert_green(&out);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let run_id = stdout
        .lines()
        .find_map(|line| line.strip_prefix("run "))
        .expect("the run id is printed")
        .trim();
    let root = std::env::temp_dir().join(format!("bddkit-{run_id}"));
    assert!(
        root.join("000000/artifact.txt").is_file(),
        "{}",
        root.display()
    );
}

#[test]
fn the_workspace_stays_outside_a_given_artifacts_dir() {
    let dir = project("artifacts-workspace", DIRECTORIES, ECHO_GROUP);
    let out = run_with(&dir, &["--artifacts-dir", "evidence"], &[]);
    assert_green(&out);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let run_id = stdout
        .lines()
        .find_map(|line| line.strip_prefix("run "))
        .expect("the run id is printed")
        .trim();
    let workspace = std::env::temp_dir().join(format!("bddkit-{run_id}/workspace/000000"));
    assert!(
        workspace.join("workspace.txt").is_file(),
        "the workspace stays under the temp dir: {}",
        workspace.display()
    );
    assert!(
        !dir.join("evidence/000000/workspace.txt").exists(),
        "no workspace file under the artifact root"
    );
}

// ---- privacy: what a plugin sends back passes through the host's filter ----

#[test]
fn a_sensitive_field_in_a_json_diagnostic_is_masked() {
    let dir = project(
        "mask-json-diagnostic",
        "Feature: f\n  Scenario: s\n    Given the echo should fail with json\n\n  Scenario: later\n    Given set variable \"note\" to \"diag-refresh-token-1\"\n    Then variable \"note\" should be equal to \"nothing\"\n",
        "",
    );
    let out = run(&dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    // What the diagnostic held under a sensitive name joined the file's set: a
    // later scenario that prints the same value, under a plain name, hides it.
    assert!(!stdout.contains("diag-refresh-token-1"), "{stdout}");
    // `Zd1` is shorter than min_length: only its position hides it.
    assert!(stdout.contains(r#""token": "***""#), "{stdout}");
    assert!(!stdout.contains("Zd1"), "{stdout}");
    assert!(stdout.contains("ann"), "{stdout}");
}

#[test]
fn a_plugin_config_secret_is_masked_in_a_validate_config_refusal() {
    let dir = project(
        "mask-validate-config",
        "Feature: f\n  Scenario: s\n    Given the echo should fail\n",
        "  echo:\n    main:\n      prefix: p-\n      password: plugin-secret-1234\n",
    );
    let out = run(&dir);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("refuses the password"), "{stderr}");
    assert!(stderr.contains(r#""***""#), "{stderr}");
    assert!(!stderr.contains("plugin-secret-1234"), "{stderr}");
}

#[test]
fn a_secret_reaches_a_plugin_unmasked_and_its_reply_is_masked() {
    let dir = project(
        "mask-reaches-plugin",
        "Feature: f\n  Scenario: s\n    Given set variable \"secret_note\" to \"plugin-arg-secret-1\"\n    And I echo \"<<secret_note>>\" as \"copy\"\n    And variable \"copy\" should be equal to \"implicit-plugin-arg-secret-1\"\n    Then the echo should fail quoting \"<<secret_note>>\"\n",
        "",
    );
    let out = run(&dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    // The plugin got the real value (the equality step passed and the failing
    // one is the quoting step), and what it replied is hidden.
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("the echo quoted ***"), "{stdout}");
    assert!(!stdout.contains("plugin-arg-secret-1"), "{stdout}");
}

// ---- --evidence: what a plugin's reply puts into the event stream ----

/// The events written by a run with `--events <dir>/events.ndjson --evidence`.
fn evidence_stream(dir: &Path, extra: &[&str]) -> Vec<serde_json::Value> {
    let mut args = vec!["--events", "events.ndjson", "--evidence"];
    args.extend_from_slice(extra);
    assert_green(&run_with(dir, &args, &[]));
    std::fs::read_to_string(dir.join("events.ndjson"))
        .expect("the stream exists")
        .lines()
        .map(|line| serde_json::from_str(line).expect("one JSON object per line"))
        .collect()
}

#[test]
fn a_plugin_diagnostic_becomes_evidence_with_size_and_media_type() {
    let dir = project(
        "evidence-diagnostic",
        "Feature: f\n  Scenario: s\n    When I echo evidence\n",
        ECHO_GROUP,
    );
    let stream = evidence_stream(&dir, &["--artifacts-dir", "evidence"]);

    let evidence: Vec<_> = stream.iter().filter(|e| e["type"] == "evidence").collect();
    assert_eq!(evidence.len(), 3, "a PASSED reply's diagnostics count");
    let text = evidence[0];
    assert_eq!(
        (text["group"].as_str(), text["resource"].as_str()),
        (Some("echo"), Some("main"))
    );
    assert_eq!(text["title"], "echo state");
    assert_eq!(text["kind"], "text");
    assert_eq!(text["content"], "prefix=p-");
    assert_eq!(text["step"], 0);

    let inside = evidence[1];
    assert_eq!(inside["path"], "000000/inside.txt", "relative to the root");
    assert_eq!(inside["size"], 6);
    assert_eq!(inside["media_type"], "text/plain");
    assert!(inside.get("content").is_none());
    assert!(dir.join("evidence/000000/inside.txt").is_file());
}

#[test]
fn a_diagnostic_outside_the_root_is_copied_in() {
    let dir = project(
        "evidence-outside",
        "Feature: f\n  Scenario: s\n    When I echo evidence\n",
        ECHO_GROUP,
    );
    let stream = evidence_stream(&dir, &["--artifacts-dir", "evidence"]);

    let outside = stream
        .iter()
        .find(|e| e["title"] == "outside")
        .expect("the outside diagnostic");
    let path = outside["path"].as_str().expect("a path");
    assert!(
        !Path::new(path).is_absolute() && !path.contains('\\'),
        "{path}"
    );
    assert!(path.ends_with("/outside.txt"), "{path}");
    assert_eq!(
        std::fs::read_to_string(dir.join("evidence").join(path)).expect("copied in"),
        "marker"
    );
}

#[test]
fn a_plugin_not_yet_is_an_attempt() {
    let dir = project(
        "evidence-attempt",
        "Feature: f\n  Scenario: s\n    When I expect the next assertion to pass within \"5\" seconds, checking every \"20\" milliseconds\n    Then the echo counter should reach 3\n",
        ECHO_GROUP,
    );
    let stream = evidence_stream(&dir, &[]);

    let attempts: Vec<_> = stream.iter().filter(|e| e["type"] == "attempt").collect();
    let numbered: Vec<_> = attempts.iter().map(|e| e["n"].as_u64()).collect();
    assert_eq!(numbered, [Some(1), Some(2)]);
    assert!(
        attempts[0]["message"]
            .as_str()
            .is_some_and(|m| m.contains("counter is 1 of 3")),
        "{}",
        attempts[0]
    );
}

// ---- #90: what a plugin can say about evidence and secrets ----

#[test]
fn a_plugin_is_told_whether_evidence_is_wanted() {
    let feature = "Feature: f\n  Scenario: s\n    When I echo the evidence hint\n    Then variable \"echo_hint\" should be equal to \"EXPECTED\"\n";
    let off = project(
        "hint-off",
        &feature.replace("EXPECTED", "false"),
        ECHO_GROUP,
    );
    assert_green(&run(&off));

    // The flag is on for every attempt, so one step is enough to see it.
    let on = project("hint-on", &feature.replace("EXPECTED", "true"), ECHO_GROUP);
    evidence_stream(&on, &[]);
}

#[test]
fn a_plugin_is_told_evidence_is_wanted_for_deferred_steps() {
    let feature = "Feature: f\n  Scenario: s\n    When I echo the evidence hint\n    Then variable \"echo_hint\" should be equal to \"EXPECTED\"\n";
    // Deferred: the scenario may yet fail, so the step's evidence may be kept.
    let deferred = project(
        "hint-deferred",
        &feature.replace("EXPECTED", "true"),
        ECHO_GROUP,
    );
    assert_green(&run_with(
        &deferred,
        &["--events", "events.ndjson", "--evidence=failures"],
        &[],
    ));
    // An unmarked step under `marked` alone can never be kept.
    let off = project(
        "hint-unmarked",
        &feature.replace("EXPECTED", "false"),
        ECHO_GROUP,
    );
    assert_green(&run_with(
        &off,
        &["--events", "events.ndjson", "--evidence=marked"],
        &[],
    ));
}

#[test]
fn a_declared_media_type_replaces_the_guess_and_declared_secrets_are_masked() {
    let dir = project(
        "declared-secret",
        "Feature: f\n  Scenario: s\n    When I echo a declared secret and a typed file\n",
        ECHO_GROUP,
    );
    let stream = evidence_stream(&dir, &["--artifacts-dir", "evidence"]);

    let shot = stream
        .iter()
        .find(|e| e["title"] == "shot")
        .expect("the shot");
    assert_eq!(shot["media_type"], "image/png", "`.bin` would be a guess");
    let quote = stream
        .iter()
        .find(|e| e["title"] == "quote")
        .expect("the quote");
    assert_eq!(quote["content"], "token ***");
    let raw = std::fs::read_to_string(dir.join("events.ndjson")).expect("stream");
    assert!(!raw.contains("declared-secret-value"), "{raw}");
}

#[test]
fn a_secret_a_failing_reply_declares_is_masked_in_its_own_error() {
    let dir = project(
        "declared-secret-on-fatal",
        "Feature: f\n  Scenario: s\n    Then the echo should fail declaring \"fatal-secret-1\" secret\n",
        ECHO_GROUP,
    );
    let out = run(&dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("the echo leaked ***"), "{stdout}");
    assert!(!stdout.contains("fatal-secret-1"), "{stdout}");
}

#[test]
fn a_step_spec_secret_group_is_hidden_in_the_step_and_in_what_the_plugin_quotes() {
    let dir = project(
        "secret-group",
        "Feature: f\n  Scenario: s\n    Given set variable \"pw\" to \"group-secret-1\"\n    Then I echo \"ann\" with password \"<<pw>>\"\n",
        ECHO_GROUP,
    );
    let out = run_with(&dir, &["--events", "events.ndjson", "--evidence"], &[]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("the echo was given ***"), "{stdout}");
    assert!(!stdout.contains("group-secret-1"), "{stdout}");
    // `pw` is not a sensitive name: only the step spec's flag hides the slot.
    let raw = std::fs::read_to_string(dir.join("events.ndjson")).expect("stream");
    assert!(raw.contains(r#"[["pw","***"]]"#), "{raw}");

    // A literal in the group is hidden in the step text itself.
    let literal = project(
        "secret-group-literal",
        "Feature: f\n  Scenario: s\n    Then I echo \"ann\" with password \"pw1\"\n",
        ECHO_GROUP,
    );
    let out = run_with(&literal, &["--events", "events.ndjson"], &[]);
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(stdout.contains("with password \"***\""), "{stdout}");
    assert!(!stdout.contains("\"pw1\""), "{stdout}");
    let raw = std::fs::read_to_string(literal.join("events.ndjson")).expect("stream");
    assert!(raw.contains(r#"with password \"***\""#), "{raw}");
    assert!(!raw.contains(r#"\"pw1\""#), "{raw}");
}

#[test]
fn a_not_yet_reply_declares_secrets_and_sees_the_evidence_flag_on_every_attempt() {
    let dir = project(
        "declared-secret-not-yet",
        "Feature: f\n  Scenario: s\n    When I expect the next assertion to pass within \"1\" seconds, checking every \"20\" milliseconds\n    Then the echo should not be ready yet declaring \"notyet-secret-1\" secret\n",
        ECHO_GROUP,
    );
    let out = run_with(&dir, &["--events", "events.ndjson", "--evidence"], &[]);
    assert_eq!(out.status.code(), Some(1));
    let stream: Vec<serde_json::Value> = std::fs::read_to_string(dir.join("events.ndjson"))
        .expect("stream")
        .lines()
        .map(|line| serde_json::from_str(line).expect("json"))
        .collect();
    let attempts: Vec<_> = stream.iter().filter(|e| e["type"] == "attempt").collect();
    assert!(attempts.len() > 1, "polling replayed the step");
    for attempt in attempts {
        // Masked at emit time, so only an early registration keeps it clean.
        assert_eq!(
            attempt["message"], "not ready *** evidence=true",
            "{attempt}"
        );
    }
}

#[test]
fn a_declared_secret_shorter_than_min_length_is_not_registered() {
    let dir = project(
        "declared-secret-short",
        "Feature: f\n  Scenario: s\n    Then the echo should fail declaring \"short\" secret\n",
        ECHO_GROUP,
    );
    let stdout = String::from_utf8_lossy(&run(&dir).stdout).into_owned();
    assert!(stdout.contains("the echo leaked short"), "{stdout}");
}

#[test]
fn a_secret_group_no_pattern_group_has_stops_the_run_at_load() {
    let dir = project(
        "secret-group-bad-index",
        "Feature: f\n  Scenario: s\n    Then the echo should fail\n",
        ECHO_GROUP,
    );
    let out = run_with(&dir, &[], &[("BDDKIT_ECHO_FIXTURE_BAD_SECRET_GROUP", "1")]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("groups count from 1"), "{stderr}");
}

#[test]
fn a_plugin_built_for_abi_2_is_refused_by_a_generation_4_host() {
    let dir = project(
        "abi2-refused",
        "Feature: f\n  Scenario: s\n    When I echo \"x\" as \"y\"\n",
        ECHO_GROUP,
    );
    let out = run_with(&dir, &[], &[("BDDKIT_ECHO_FIXTURE_ABI", "2")]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("ABI version 2")
            && stderr.contains("older than the 3")
            && stderr.contains("not updated"),
        "{stderr}"
    );
}

#[test]
fn a_plugin_built_for_abi_3_loads_and_one_built_for_abi_5_is_refused() {
    let feature = "Feature: f\n  Scenario: s\n    When I echo \"x\" as \"y\"\n";
    let dir = project("abi3-loads", feature, ECHO_GROUP);
    let out = run_with(&dir, &[], &[("BDDKIT_ECHO_FIXTURE_ABI", "3")]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let dir = project("abi5-refused", feature, ECHO_GROUP);
    let out = run_with(&dir, &[], &[("BDDKIT_ECHO_FIXTURE_ABI", "5")]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("ABI version 5") && stderr.contains("upgrade bddkit"),
        "{stderr}"
    );
}

#[test]
fn commented_plugin_steps_are_disabled_only_while_the_plugin_is_loaded() {
    let dir = project(
        "annotation-plugin",
        "Feature: f\n  Scenario: s\n    # Given I echo \"<<undefined>>\" as \"never\"\n    Given set variable \"x\" to \"1\"\n",
        "",
    );
    let stream_path = dir.join("events.ndjson");
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .current_dir(&dir)
        .args(["run", "--config", "cfg.yaml", "--events"])
        .arg(&stream_path)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let events: Vec<serde_json::Value> = std::fs::read_to_string(&stream_path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let disabled: Vec<_> = events
        .iter()
        .filter(|e| e["type"] == "step_disabled")
        .collect();
    assert_eq!(disabled.len(), 1, "{events:?}");
    assert_eq!(disabled[0]["text"], "I echo \"<<undefined>>\" as \"never\"");

    let empty = dir.join("empty-plugins");
    std::fs::create_dir_all(&empty).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .current_dir(&dir)
        .env("BDDKIT_DIR", &empty)
        .args(["run", "--config", "cfg.yaml", "--events"])
        .arg(&stream_path)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let stream = std::fs::read_to_string(&stream_path).unwrap();
    assert!(!stream.contains("\"type\":\"step_disabled\""), "{stream}");
    let events: Vec<serde_json::Value> = stream
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let step = events.iter().find(|e| e["type"] == "step_started").unwrap();
    assert_eq!(
        step["comment"],
        "Given I echo \"<<undefined>>\" as \"never\""
    );
}

#[test]
fn a_step_that_wants_vars_receives_them_flagged_and_null_aware() {
    let dir = project(
        "wants-vars",
        r#"Feature: f
  Scenario: s
    Given set variable "plain" to "hello"
    And set variable "db_secret" to "hunter22"
    And set variable "empty" to "<<null>>"
    And set variable "x" to "global" global
    And set variable "x" to "frame"
    When I echo the vars
    Then variable "echo_vars" should contain JSON:
      """
      {
        "plain": {"value": "hello", "secret": false},
        "db_secret": {"value": "hunter22", "secret": true},
        "empty": {"value": null, "secret": false},
        "x": {"value": "frame", "secret": false}
      }
      """
"#,
        "",
    );
    assert_green(&run(&dir));
}

/// `project` with `macros/m.yaml` holding `yaml`.
fn macro_project(name: &str, feature: &str, yaml: &str) -> PathBuf {
    let dir = project(name, feature, "macro_paths: [macros]\n");
    std::fs::create_dir_all(dir.join("macros")).unwrap();
    std::fs::write(dir.join("macros/m.yaml"), yaml).unwrap();
    dir
}

#[test]
fn a_short_macro_secret_parameter_is_flagged_secret_in_the_vars() {
    let dir = macro_project(
        "vars-macro-secret",
        r#"Feature: f
  Scenario: s
    When I pin "1234"
    Then variable "echo_vars" should contain JSON:
      """
      {"pin": {"value": "1234", "secret": true}}
      """
"#,
        "- step: 'I pin \"{pin:secret}\"'\n  do: [I echo the vars]\n  exports: [echo_vars]\n",
    );
    assert_green(&run(&dir));
}

#[test]
fn a_step_that_does_not_ask_receives_an_empty_vars() {
    let dir = project(
        "no-vars",
        r#"Feature: f
  Scenario: s
    Given set variable "plain" to "hello"
    When I echo the vars without asking
    Then variable "echo_vars" should be equal to "{}"
"#,
        "",
    );
    assert_green(&run(&dir));
}

#[test]
fn a_raw_docstring_step_receives_its_text_uninterpolated() {
    let dir = project(
        "raw-doc",
        r#"Feature: f
  Scenario: s
    Given set variable "x" to "VALUE"
    When I echo the raw doc string:
      """
      a <<x>> b
      """
    # Expected values are interpolated, so a literal <<x>> cannot be written; [x] keeps the pattern from reading as a slot and still matches the text.
    Then variable "echo_doc" should match "^a <<[x]>> b$"
"#,
        "",
    );
    assert_green(&run(&dir));
}

#[test]
fn an_ordinary_plugin_doc_string_is_still_interpolated() {
    let dir = project(
        "plain-doc",
        r#"Feature: f
  Scenario: s
    Given set variable "x" to "VALUE"
    When I echo the plain doc string:
      """
      a <<x>> b
      """
    Then variable "echo_doc" should be equal to "a VALUE b"
"#,
        "",
    );
    assert_green(&run(&dir));
}

const MACRO_CALL: &str = "Feature: f\n  Scenario: s\n    When I do the checked thing\n";
const CHECKED_MACRO: &str = "- step: I do the checked thing\n  do: [I echo validated \"bad\"]\n";

/// A feature whose one step, at line 3, hands `arg` to the fixture's validator.
fn validated(arg: &str) -> String {
    format!("Feature: f\n  Scenario: s\n    When I echo validated \"{arg}\"\n")
}

/// Every way a plugin step can be refused before the first request, and
/// every way a hook can misbehave, is exit 2 naming the file and line.
#[test]
fn a_rejected_step_exits_2_before_the_first_request() {
    const RAW_DOC: &str = r#"Feature: f
  Scenario: s
    Given set variable "x" to "VALUE"
    When I echo validated doc string:
      """
      a <<x>> b
      """
"#;
    const INCLUDE: &str = "Feature: f\n  Scenario: s\n    When I include \"../inc/inc.feature\"\n";
    let cases = [
        (
            "feature",
            validated("bad"),
            vec![],
            ("the echo refuses \"bad\"", "plugin.feature:3"),
        ),
        // Interpolated, `<<x>>` would be `VALUE` and the fixture would accept it.
        (
            "raw-doc",
            RAW_DOC.into(),
            vec![],
            ("reached validation uninterpolated", "plugin.feature:4"),
        ),
        // Outside `paths`, so the file is reached only through the include.
        (
            "include",
            INCLUDE.into(),
            vec![("inc/inc.feature", validated("bad"))],
            ("the echo refuses", "inc.feature:3"),
        ),
        (
            "macro",
            MACRO_CALL.into(),
            vec![("macros/m.yaml", CHECKED_MACRO.into())],
            ("the echo refuses", "m.yaml:1"),
        ),
        (
            "panic",
            validated("panic"),
            vec![],
            ("the plugin panicked", "plugin.feature:3"),
        ),
        (
            "malformed",
            validated("malformed"),
            vec![],
            ("malformed validate_step", "plugin.feature:3"),
        ),
        (
            "noerror",
            validated("noerror"),
            vec![],
            ("no message", "plugin.feature:3"),
        ),
    ];
    for (name, feature, files, (expect, at)) in cases {
        let macros = files.iter().any(|(path, _)| path.starts_with("macros/"));
        let tail = if macros {
            "macro_paths: [macros]\n"
        } else {
            ""
        };
        let dir = project(&format!("validate-{name}"), &feature, tail);
        for (path, text) in files {
            std::fs::create_dir_all(dir.join(path).parent().unwrap()).unwrap();
            std::fs::write(dir.join(path), text).unwrap();
        }
        let out = run(&dir);
        let all = common::combined(&out);
        assert_eq!(out.status.code(), Some(2), "{name}: {all}");
        assert!(all.contains(expect), "{name}: {all}");
        assert!(all.contains(at), "{name}: {all}");
    }
}

fn doctor(dir: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config", "cfg.yaml"])
        .current_dir(dir)
        .output()
        .expect("run doctor")
}

#[test]
fn doctor_files_a_rejected_macro_body_under_macros() {
    let dir = macro_project("validate-macro-doctor", MACRO_CALL, CHECKED_MACRO);
    let out = doctor(&dir);
    let all = common::combined(&out);
    assert_eq!(out.status.code(), Some(1), "{all}");
    let line = all
        .lines()
        .find(|l| l.contains("the echo refuses"))
        .unwrap_or_else(|| panic!("{all}"));
    assert!(line.contains("macros"), "{all}");
}

#[test]
fn doctor_reports_a_rejected_step() {
    let out = doctor(&project("validate-doctor", &validated("bad"), ""));
    let all = common::combined(&out);
    assert_eq!(out.status.code(), Some(1), "{all}");
    assert!(all.contains("the echo refuses"), "{all}");
}

#[test]
fn a_plugin_without_the_hook_accepts_every_step() {
    // The worker fixture does not export bddkit_validate_step.
    const WORKER_STEP: &str = "I count in the worker as \"n\"";
    let dir = worker_project(
        "validate-absent",
        &[(
            "w.feature",
            &format!("Feature: f\n  Scenario: s\n    When {WORKER_STEP}\n"),
        )],
        "",
        1,
    );
    assert_green(&run(&dir));
}

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

const INCLUDE: &str = "    Given I include \"helper.feature\"\n";
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

// ---- ABI 4: `secrets` and `detail` in a plugin's config replies ----

/// stdout and stderr together, and the exit code, of `bddkit <args>` in `dir`.
fn bddkit_in(dir: &Path, args: &[&str]) -> (String, Option<i32>) {
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run bddkit");
    (
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
        out.status.code(),
    )
}

/// `note` matches no privacy word, so only the reply's `secrets` can hide it.
const REFUSING_NOTE: &str =
    "  echo:\n    main:\n      prefix: p-\n      note: plugin-note-9876\n      note_refuse: true\n";

#[test]
fn a_secret_in_a_refusing_reply_is_masked_in_its_own_refusal_text() {
    let dir = project(
        "cfg-secret-refusal",
        "Feature: f\n  Scenario: s\n    Given I am in debug mode\n",
        REFUSING_NOTE,
    );
    for args in [
        &["run", "--config", "cfg.yaml"][..],
        &["doctor", "--config", "cfg.yaml"][..],
        &["doctor", "--config", "cfg.yaml", "--json"][..],
    ] {
        let (text, code) = bddkit_in(&dir, args);
        assert_ne!(code, Some(0), "{args:?}: {text}");
        assert!(text.contains("refuses the note"), "{args:?}: {text}");
        assert!(!text.contains("plugin-note-9876"), "{args:?}: {text}");
    }
    let (text, _) = bddkit_in(&dir, &["run", "--config", "cfg.yaml", "--reveal"]);
    assert!(
        text.contains("plugin-note-9876"),
        "--reveal shows it: {text}"
    );
}

#[test]
fn a_secret_under_min_length_stays_visible() {
    let dir = project(
        "cfg-secret-short",
        "Feature: f\n  Scenario: s\n    Given I am in debug mode\n",
        "  echo:\n    main:\n      prefix: p-\n      note: abc\n      note_refuse: true\n",
    );
    let (text, _) = bddkit_in(&dir, &["run", "--config", "cfg.yaml"]);
    assert!(text.contains(r#"refuses the note "abc""#), "{text}");
}

#[test]
fn resource_add_masks_a_secret_in_the_refusal_block() {
    let dir = project(
        "cfg-secret-resource-add",
        "Feature: f\n",
        &format!("{ECHO_GROUP}default_echo: main\n"),
    );
    let (text, code) = bddkit_in(
        &dir,
        &[
            "resource",
            "add",
            "echo",
            "second",
            "--config",
            "cfg.yaml",
            "--prefix",
            "q-",
            "--note",
            "plugin-note-9876",
            "--note_refuse",
            "true",
        ],
    );
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("refuses the note"), "{text}");
    assert!(!text.contains("plugin-note-9876"), "{text}");
}

/// The block is the whole value of the failure path, so it has to be pasteable
/// under `resources:`, which means carrying the group key when the config does
/// not have that group yet, and it is masked: a URL password in a value never
/// reaches the screen.
#[test]
fn resource_add_prints_a_masked_block_with_the_group_key_the_config_lacks() {
    let dir = project("resource-add-new-group", "Feature: f\n", "");
    let (text, code) = bddkit_in(
        &dir,
        &[
            "resource",
            "add",
            "echo",
            "second",
            "--config",
            "cfg.yaml",
            "--prefix",
            "http://app:add-pass-5678@127.0.0.1:1/app",
            "--probe_error",
            "the bucket is not there",
        ],
    );
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("The block, for manual insertion"), "{text}");
    assert!(
        text.contains("echo:\n  second:\n    prefix: http://app:***@127.0.0.1:1/app\n"),
        "the block names the group it belongs under, masked: {text}"
    );
    assert!(!text.contains("add-pass-5678"), "{text}");
}

#[test]
fn a_config_secret_is_masked_in_a_file_with_no_plugin_step() {
    let dir = project(
        "cfg-secret-no-step",
        "Feature: f\n  Scenario: s\n    Given set variable \"v\" to \"plugin-note-9876\"\n    Then variable \"v\" should be equal to \"other\"\n",
        "  echo:\n    main:\n      prefix: p-\n      note: plugin-note-9876\n",
    );
    let (text, code) = bddkit_in(&dir, &["run", "--config", "cfg.yaml"]);
    assert_eq!(code, Some(1), "{text}");
    assert!(!text.contains("plugin-note-9876"), "{text}");
}

#[test]
fn doctor_shows_the_detail_of_an_ok_instance_static_and_live() {
    let dir = project(
        "cfg-detail",
        "Feature: f\n  Scenario: s\n    Given I am in debug mode\n",
        "  echo:\n    main:\n      prefix: p-\n      detail: static-detail-text\n      probe_detail: live-detail-text\n    plain:\n      prefix: q-\ndefault_echo: main\n",
    );
    let (text, code) = bddkit_in(&dir, &["doctor", "--config", "cfg.yaml"]);
    assert_eq!(code, Some(0), "{text}");
    assert!(text.contains("static-detail-text"), "{text}");
    assert!(!text.contains("live-detail-text"), "{text}");

    let (live, _) = bddkit_in(&dir, &["doctor", "--config", "cfg.yaml", "--live"]);
    assert!(live.contains("live-detail-text"), "{live}");
    assert!(
        live.contains("probed clean"),
        "an ok instance without a detail keeps today's row:\n{live}"
    );

    let (json, _) = bddkit_in(
        &dir,
        &["doctor", "--config", "cfg.yaml", "--live", "--json"],
    );
    assert!(
        json.contains("static-detail-text") && json.contains("live-detail-text"),
        "{json}"
    );
}

#[test]
fn resource_add_masks_a_secret_only_the_probe_declares() {
    let dir = project(
        "cfg-probe-secret",
        "Feature: f\n",
        &format!("{ECHO_GROUP}default_echo: main\n"),
    );
    let (text, code) = bddkit_in(
        &dir,
        &[
            "resource",
            "add",
            "echo",
            "second",
            "--config",
            "cfg.yaml",
            "--prefix",
            "q-",
            "--probe_note",
            "plugin-probe-5555",
            "--probe_detail",
            "reached with plugin-probe-5555",
        ],
    );
    assert_eq!(code, Some(0), "{text}");
    assert!(text.contains("reached with"), "{text}");
    assert!(!text.contains("plugin-probe-5555"), "{text}");
}

#[test]
fn a_malformed_config_reply_still_has_its_secrets_masked() {
    let dir = project(
        "cfg-malformed-secret",
        "Feature: f\n  Scenario: s\n    Given I am in debug mode\n",
        "  echo:\n    main:\n      prefix: p-\n      note: plugin-note-9876\n      malformed: true\n",
    );
    let (text, code) = bddkit_in(&dir, &["run", "--config", "cfg.yaml"]);
    assert_eq!(code, Some(2), "{text}");
    assert!(text.contains("malformed validate_config reply"), "{text}");
    assert!(!text.contains("plugin-note-9876"), "{text}");
}

// ---- Structured diagnostics and `scan` in a plugin's reply (ABI 4) ----

/// A feature whose one step makes the echo plugin answer with `reply`.
fn answers(reply: &serde_json::Value) -> String {
    format!(
        "Feature: f\n  Scenario: s\n    Then the echo answers:\n      \"\"\"\n      {reply}\n      \"\"\"\n"
    )
}

fn stdout_of(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn read_stream(dir: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(dir.join("events.ndjson"))
        .expect("the stream exists")
        .lines()
        .map(|line| serde_json::from_str(line).expect("one JSON object per line"))
        .collect()
}

fn http_content(request_body: &str, response_body: &str) -> serde_json::Value {
    serde_json::json!({
        "method": "POST",
        "url": "http://svc.test/login?token=query-secret-1&page=2",
        "request": {
            "headers": [["Authorization", "Bearer bearer-secret-1"], ["X-Plain", "ok"]],
            "body": request_body,
        },
        "status": 401,
        "response": {
            "headers": [["Set-Cookie", "sid=cookie-secret-1; Path=/"], ["Content-Type", "application/json"]],
            "body": response_body,
        },
        "duration_us": 7,
        "sensitive_headers": ["authorization", "set-cookie"],
        "request_form": "raw",
    })
}

#[test]
fn an_http_object_is_dumped_and_emitted_masked_by_position() {
    let reply = serde_json::json!({
        "status": "fatal",
        "error": "login refused",
        "diagnostics": [{
            "kind": "http",
            "content": http_content(
                r#"{"password":"pw-secret-1","name":"ann"}"#,
                r#"{"token":"tok-secret-1","id":42}"#,
            ),
        }],
    });
    let dir = project("structured-http", &answers(&reply), ECHO_GROUP);
    let out = run_with(&dir, &["--events", "events.ndjson", "--evidence"], &[]);
    let text = stdout_of(&out);
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert!(text.contains("login refused"), "{text}");
    assert!(
        text.contains("POST http://svc.test/login?token=***&page=2"),
        "{text}"
    );
    assert!(text.contains("Authorization: ***"), "{text}");
    assert!(text.contains("X-Plain: ok"), "{text}");
    assert!(text.contains("← 401"), "{text}");
    for secret in [
        "query-secret-1",
        "bearer-secret-1",
        "pw-secret-1",
        "tok-secret-1",
        "cookie-secret-1",
    ] {
        assert!(!text.contains(secret), "{secret} leaked: {text}");
    }
    let stream = read_stream(&dir);
    let evidence = stream
        .iter()
        .find(|e| e["type"] == "evidence")
        .expect("an evidence event");
    assert_eq!(evidence["kind"], "http");
    assert_eq!(
        (evidence["group"].as_str(), evidence["resource"].as_str()),
        (Some("echo"), Some("main"))
    );
    assert!(
        evidence.get("title").is_none(),
        "title is optional: {evidence}"
    );
    let content = &evidence["content"];
    assert_eq!(content["status"], 401);
    for stripped in ["sensitive_headers", "request_form", "in_dump", "resource"] {
        assert!(
            content.get(stripped).is_none(),
            "{stripped} stays out: {content}"
        );
    }
    assert_eq!(
        content["request"]["headers"][0],
        serde_json::json!(["Authorization", "***"])
    );
    let raw = std::fs::read_to_string(dir.join("events.ndjson")).unwrap();
    for secret in [
        "query-secret-1",
        "bearer-secret-1",
        "pw-secret-1",
        "tok-secret-1",
        "cookie-secret-1",
    ] {
        assert!(
            !raw.contains(secret),
            "{secret} leaked into the stream: {raw}"
        );
    }

    let revealed = run_with(&dir, &["--reveal"], &[]);
    let text = stdout_of(&revealed);
    assert!(
        text.contains("POST http://svc.test/login?token=query-secret-1&page=2"),
        "{text}"
    );
    assert!(text.contains("Bearer bearer-secret-1"), "{text}");
}

#[test]
fn a_form_body_and_a_long_response_follow_the_built_in_rules() {
    let long = format!(r#"{{"token":"tok-long-1","pad":"{}"}}"#, "x".repeat(20_000));
    let mut content = http_content("password=form-secret-1&name=ann", &long);
    content["request_form"] = "form".into();
    let reply = serde_json::json!({
        "status": "fatal", "error": "refused",
        "diagnostics": [{"kind": "http", "title": "login", "content": content}],
    });
    let dir = project("structured-http-form", &answers(&reply), ECHO_GROUP);
    let out = run_with(
        &dir,
        &[
            "--events",
            "events.ndjson",
            "--evidence",
            "--artifacts-dir",
            "art",
        ],
        &[],
    );
    let text = stdout_of(&out);
    assert!(text.contains("password=***&name=ann"), "{text}");
    assert!(
        !text.contains("form-secret-1") && !text.contains("tok-long-1"),
        "{text}"
    );
    // The dump cuts the body to 600 characters, after masking.
    assert!(text.contains('…'), "{text}");
    let stream = read_stream(&dir);
    let evidence = stream.iter().find(|e| e["type"] == "evidence").unwrap();
    assert_eq!(evidence["title"], "login");
    let body = &evidence["content"]["response"]["body"];
    let path = body["path"].as_str().expect("a long body goes to a file");
    assert!(path.contains("response-body."), "{path}");
    let written = std::fs::read_to_string(dir.join("art").join(path)).unwrap();
    assert!(
        !written.contains("tok-long-1") && written.contains("***"),
        "{written}"
    );
}

#[test]
fn an_sql_object_is_masked_by_value_only_and_stays_out_of_the_dump() {
    let reply = serde_json::json!({
        "status": "fatal", "error": "statement refused", "secrets": ["sql-secret-1"],
        "diagnostics": [{
            "kind": "sql", "resource": "orders",
            "content": {"sql": "select * from t where k = ?", "binds": ["sql-secret-1", null], "duration_us": 3},
        }],
    });
    let dir = project("structured-sql", &answers(&reply), ECHO_GROUP);
    let out = run_with(&dir, &["--events", "events.ndjson", "--evidence"], &[]);
    let text = stdout_of(&out);
    assert!(text.contains("statement refused"), "{text}");
    assert!(
        !text.contains("select * from t"),
        "sql is never in the dump: {text}"
    );
    let stream = read_stream(&dir);
    let evidence = stream.iter().find(|e| e["type"] == "evidence").unwrap();
    assert_eq!(evidence["kind"], "sql");
    assert_eq!(evidence["resource"], "orders");
    assert_eq!(
        evidence["content"]["binds"],
        serde_json::json!(["***", null])
    );
}

#[test]
fn a_diagnostic_that_is_not_in_the_dump_is_still_evidence() {
    let reply = serde_json::json!({
        "status": "fatal", "error": "refused",
        "diagnostics": [
            {"kind": "http", "in_dump": false, "content": http_content("", "{}")},
            {"title": "hidden note", "kind": "text", "content": "shown nowhere", "in_dump": false},
            {"title": "shown note", "kind": "text", "content": "shown in the dump"},
        ],
    });
    let dir = project("structured-in-dump", &answers(&reply), ECHO_GROUP);
    let out = run_with(&dir, &["--events", "events.ndjson", "--evidence"], &[]);
    let text = stdout_of(&out);
    assert!(text.contains("shown in the dump"), "{text}");
    assert!(
        !text.contains("svc.test") && !text.contains("shown nowhere"),
        "{text}"
    );
    let kinds: Vec<_> = read_stream(&dir)
        .iter()
        .filter(|e| e["type"] == "evidence")
        .map(|e| e["kind"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(kinds, ["http", "text", "text"]);
}

#[test]
fn without_evidence_a_failing_reply_reaches_the_dump_only() {
    let reply = serde_json::json!({
        "status": "fatal", "error": "refused",
        "diagnostics": [{"kind": "http", "content": http_content("", "{}")}],
    });
    let dir = project("structured-no-evidence", &answers(&reply), ECHO_GROUP);
    let out = run_with(&dir, &["--events", "events.ndjson"], &[]);
    assert!(stdout_of(&out).contains("POST http://svc.test/login"));
    assert!(read_stream(&dir).iter().all(|e| e["type"] != "evidence"));
}

#[test]
fn a_shape_that_does_not_fit_fails_with_class_plugin_and_quotes_nothing() {
    let mut content = http_content("", "{}");
    content["status"] = "shape-secret-1".into();
    let reply = serde_json::json!({
        "status": "passed", "secrets": ["shape-secret-1"],
        "diagnostics": [{"kind": "http", "content": content}],
    });
    let dir = project("structured-bad-shape", &answers(&reply), ECHO_GROUP);
    let out = run_with(&dir, &["--junit", "junit.xml"], &[]);
    let text = stdout_of(&out);
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert!(text.contains("does not fit its shape"), "{text}");
    assert!(!text.contains("shape-secret-1"), "{text}");
    let junit = std::fs::read_to_string(dir.join("junit.xml")).unwrap();
    assert!(junit.contains("plugin"), "{junit}");
}

#[test]
fn a_refused_reply_still_shows_the_plugin_error_masked() {
    let mut content = http_content("", "{}");
    content["status"] = "x".into();
    let reply = serde_json::json!({
        "status": "fatal", "error": "access denied for refused-secret-1",
        "secrets": ["refused-secret-1"],
        "diagnostics": [{"kind": "http", "content": content}],
    });
    let dir = project("structured-refused-error", &answers(&reply), ECHO_GROUP);
    let text = stdout_of(&run(&dir));
    assert!(text.contains("does not fit its shape"), "{text}");
    assert!(text.contains("access denied for ***"), "{text}");
    assert!(!text.contains("refused-secret-1"), "{text}");
}

#[test]
fn a_malformed_reply_still_has_its_diagnostics_and_scan_masked() {
    // `status` is not a status, so the typed parse fails; the salvage keeps
    // what the reply declared and the diagnostics that fit.
    let reply = serde_json::json!({
        "status": 7,
        "scan": [{"kind": "json", "content": {"token": "salvage-scan-1"}}],
        "diagnostics": [
            {"kind": "http", "content": http_content("", "{}")},
            {"kind": 5, "content": http_content("", r#"{"token":"salvage-body-1"}"#)},
        ],
    });
    let dir = project("structured-salvage", &answers(&reply), ECHO_GROUP);
    let text = stdout_of(&run(&dir));
    assert!(text.contains("malformed"), "{text}");
    for leaked in ["bearer-secret-1", "cookie-secret-1", "salvage-body-1"] {
        assert!(!text.contains(leaked), "{leaked} leaked: {text}");
    }
}

#[test]
fn a_scan_item_registers_what_it_finds_on_any_status() {
    // The next step quotes the value; only a registered one is masked.
    for (name, scan, value) in [
        (
            "json",
            serde_json::json!({"kind": "json", "content": r#"{"token":"scan-json-1"}"#}),
            "scan-json-1",
        ),
        (
            "form",
            serde_json::json!({"kind": "form", "pairs": [["password", "a&b=c%41+d"], ["name", "plain-name-1"]]}),
            "a&b=c%41+d",
        ),
        (
            "query",
            serde_json::json!({"kind": "query", "pairs": [["token", "scan-query-1"]]}),
            "scan-query-1",
        ),
    ] {
        for status in ["passed", "fatal"] {
            let reply = serde_json::json!({"status": status, "error": "scanned", "scan": [scan]});
            let mut feature = answers(&reply);
            if status == "passed" {
                feature.push_str(&format!(
                    "    Then the echo should fail quoting \"{value}\"\n"
                ));
            }
            let dir = project(&format!("scan-{name}-{status}"), &feature, ECHO_GROUP);
            let text = stdout_of(&run(&dir));
            if status == "passed" {
                assert!(text.contains("the echo quoted ***"), "{name}: {text}");
            }
            assert!(
                !text.contains(value),
                "{name}/{status}: {value} leaked: {text}"
            );
        }
    }
    // A non-sensitive field is left alone.
    let reply = serde_json::json!({"status": "passed", "scan": [{"kind": "form", "pairs": [["name", "plain-name-1"]]}]});
    let feature = format!(
        "{}    Then the echo should fail quoting \"plain-name-1\"\n",
        answers(&reply)
    );
    let dir = project("scan-plain", &feature, ECHO_GROUP);
    assert!(stdout_of(&run(&dir)).contains("the echo quoted plain-name-1"));
}

#[test]
fn a_not_yet_error_quoting_a_scanned_value_is_masked_and_holds_no_dump() {
    let reply = serde_json::json!({
        "status": "not_yet",
        "error": "waiting for scan-attempt-1",
        "scan": [{"kind": "form", "pairs": [["password", "scan-attempt-1"]]}],
        "diagnostics": [{"title": "state", "kind": "text", "content": "dump-text-1"}],
    });
    let feature = format!(
        "Feature: f\n  Scenario: s\n    When I expect the next assertion to pass within \"1\" seconds, checking every \"20\" milliseconds\n    Then the echo answers:\n      \"\"\"\n      {reply}\n      \"\"\"\n"
    );
    let dir = project("scan-not-yet", &feature, ECHO_GROUP);
    let out = run_with(&dir, &["--events", "events.ndjson", "--evidence"], &[]);
    let text = stdout_of(&out);
    let stream = read_stream(&dir);
    let attempts: Vec<_> = stream.iter().filter(|e| e["type"] == "attempt").collect();
    assert!(attempts.len() > 1, "polling replayed the step");
    for attempt in &attempts {
        assert_eq!(attempt["message"], "waiting for ***", "{attempt}");
    }
    // The timeout carries the mismatch once, and the last reply's dump once.
    assert_eq!(text.matches("dump-text-1").count(), 1, "{text}");
    assert!(!text.contains("scan-attempt-1"), "{text}");
    // Each attempt's diagnostics are emitted before its `attempt` event.
    let types: Vec<_> = stream.iter().map(|e| e["type"].as_str().unwrap()).collect();
    let first_evidence = types.iter().position(|t| *t == "evidence").unwrap();
    let first_attempt = types.iter().position(|t| *t == "attempt").unwrap();
    assert!(first_evidence < first_attempt, "{types:?}");
}

#[test]
fn a_failing_macro_shows_the_plugin_diagnostic_once() {
    let dir = macro_project(
        "structured-macro",
        "Feature: f\n  Scenario: s\n    When I fail through a macro\n",
        "- step: 'I fail through a macro'\n  do: [the echo should fail with json]\n",
    );
    let text = stdout_of(&run(&dir));
    assert_eq!(text.matches("--- response (json) ---").count(), 1, "{text}");
    assert!(text.contains("I fail through a macro"), "{text}");
    assert!(!text.contains("diag-refresh-token-1"), "{text}");
}

#[test]
fn a_text_diagnostic_names_its_own_resource_in_the_stream() {
    let reply = serde_json::json!({
        "status": "passed",
        "diagnostics": [{"title": "note", "kind": "text", "content": "x", "resource": "other"}],
    });
    let dir = project("structured-resource-text", &answers(&reply), ECHO_GROUP);
    assert_green(&run_with(
        &dir,
        &["--events", "events.ndjson", "--evidence"],
        &[],
    ));
    let stream = read_stream(&dir);
    let evidence = stream.iter().find(|e| e["type"] == "evidence").unwrap();
    assert_eq!(evidence["resource"], "other");
}

// ---- Dispatch keys of ABI 4: class, attempt, options_instance, output, global_vars, source_dir ----

fn polled(script: &serde_json::Value, within: &str) -> String {
    format!(
        "Feature: f\n  Scenario: s\n    When I expect the next assertion to pass within \"{within}\" seconds, checking every \"20\" milliseconds\n    Then the echo answers by attempt:\n      \"\"\"\n      {script}\n      \"\"\"\n"
    )
}

#[test]
fn a_reply_names_its_failure_class_and_an_unknown_one_counts_as_absent_with_one_warning() {
    for class in [
        "assertion",
        "transport",
        "db_connection",
        "db_statement",
        "input",
        "unclassified",
    ] {
        let reply = serde_json::json!({"status": "fatal", "error": "boom", "class": class});
        let dir = project(&format!("class-{class}"), &answers(&reply), ECHO_GROUP);
        let text = stdout_of(&run(&dir));
        assert!(
            text.contains(&format!("failed: 1 (1 {class})")),
            "{class}: {text}"
        );
    }
    // Absent, and not one the host lets a plugin name: today's class.
    for class in [
        serde_json::json!("bogus"),
        serde_json::json!("reset"),
        serde_json::json!(7),
    ] {
        let reply = serde_json::json!({"status": "fatal", "error": "boom", "class": class});
        let dir = project("class-unknown", &answers(&reply), ECHO_GROUP);
        let text = stdout_of(&run(&dir));
        assert!(text.contains("failed: 1 (1 plugin)"), "{class}: {text}");
    }
}

#[test]
fn an_unknown_class_in_two_steps_warns_once() {
    let failing = serde_json::json!({"status": "fatal", "error": "boom", "class": "bogus"});
    let step = |r: &serde_json::Value| {
        format!("    Then the echo answers:\n      \"\"\"\n      {r}\n      \"\"\"\n")
    };
    // Only a failing reply is classified, so two failing scenarios send it.
    let feature = format!(
        "Feature: f\n  Scenario: a\n{}  Scenario: b\n{}",
        step(&failing),
        step(&failing)
    );
    let dir = project("class-warn-once", &feature, ECHO_GROUP);
    let text = stdout_of(&run(&dir));
    assert_eq!(
        text.matches("which is not one it may name").count(),
        1,
        "{text}"
    );
    assert!(text.contains("\"bogus\""), "{text}");
}

#[test]
fn a_poll_ends_with_the_class_of_its_last_attempt() {
    let script = serde_json::json!([
        {"status": "not_yet", "class": "transport"},
        {"status": "not_yet", "class": "assertion"},
        {"status": "not_yet", "class": "transport"},
    ]);
    let dir = project("class-poll", &polled(&script, "1"), ECHO_GROUP);
    let text = stdout_of(&run(&dir));
    assert!(text.contains("failed: 1 (1 transport)"), "{text}");
}

#[test]
fn a_step_sees_its_attempt_number() {
    let script = serde_json::json!([
        {"status": "not_yet"},
        {"status": "not_yet"},
        {"status": "passed"},
    ]);
    let feature = format!(
        "{}    Then variable \"echo_attempt\" should be equal to \"3\"\n",
        polled(&script, "2")
    );
    let dir = project("attempt-poll", &feature, ECHO_GROUP);
    assert_green(&run(&dir));
    // Not polled: 1.
    let script = serde_json::json!([{"status": "passed"}]);
    let feature = format!(
        "Feature: f\n  Scenario: s\n    Then the echo answers by attempt:\n      \"\"\"\n      {script}\n      \"\"\"\n    And variable \"echo_attempt\" should be equal to \"1\"\n"
    );
    let dir = project("attempt-once", &feature, ECHO_GROUP);
    assert_green(&run(&dir));
}

const POLL_INSTANCES: &str = "  echo:\n    main:\n      prefix: \"p-\"\n      options:\n        polling:\n          timeout_secs: 1\n          interval_ms: 20\n    slow:\n      prefix: \"s-\"\n      options:\n        polling:\n          timeout_secs: 3\n          interval_ms: 25\ndefault_echo: main\n";

#[test]
fn options_instance_makes_a_poll_use_that_instances_options() {
    // `main` allows 1 s every 20 ms; `slow` allows 3 s every 25 ms. A bare
    // "eventually" modifier layers nothing on top, so the numbers are slow's.
    let script = serde_json::json!([{"status": "not_yet", "options_instance": "slow"}]);
    let feature = format!(
        "Feature: f\n  Scenario: s\n    When I expect the next assertion to pass eventually\n    Then the echo answers by attempt:\n      \"\"\"\n      {script}\n      \"\"\"\n"
    );
    let dir = project("options-instance", &feature, POLL_INSTANCES);
    let text = stdout_of(&run(&dir));
    assert!(text.contains("did not pass within 3s"), "{text}");
    assert!(text.contains("interval 25ms"), "{text}");
}

#[test]
fn the_modifiers_layer_stays_on_top_of_options_instance() {
    let script = serde_json::json!([{"status": "not_yet", "options_instance": "slow"}]);
    let dir = project(
        "options-instance-layer",
        &polled(&script, "2"),
        POLL_INSTANCES,
    );
    let text = stdout_of(&run(&dir));
    assert!(
        text.contains("did not pass within 2s"),
        "the layer's timeout wins: {text}"
    );
    assert!(
        text.contains("interval 20ms"),
        "the layer's interval wins: {text}"
    );
}

#[test]
fn options_instance_takes_the_other_instances_polling_when_the_modifier_is_silent() {
    let script = serde_json::json!([{"status": "not_yet", "options_instance": "slow"}]);
    // A modifier that sets only the timeout: the interval comes from `slow`.
    let feature = format!(
        "Feature: f\n  Scenario: s\n    When I expect the next assertion to pass within \"2\" seconds\n    Then the echo answers by attempt:\n      \"\"\"\n      {script}\n      \"\"\"\n"
    );
    let dir = project("options-instance-interval", &feature, POLL_INSTANCES);
    let text = stdout_of(&run(&dir));
    assert!(text.contains("interval 25ms"), "{text}");
}

#[test]
fn an_undeclared_options_instance_fails_with_class_plugin() {
    let script = serde_json::json!([{"status": "not_yet", "options_instance": "nowhere"}]);
    let dir = project(
        "options-instance-undeclared",
        &polled(&script, "2"),
        POLL_INSTANCES,
    );
    let text = stdout_of(&run(&dir));
    assert!(text.contains("failed: 1 (1 plugin)"), "{text}");
    assert!(text.contains("nowhere"), "{text}");
}

#[test]
fn output_is_printed_to_stderr_masked_and_never_to_stdout() {
    let reply = serde_json::json!({
        "status": "passed",
        "secrets": ["output-secret-value-1"],
        "output": [
            {"text": "{\"user\":\"ann\",\"password\":\"hunter2hunter2\"}", "syntax": "json"},
            {"text": "<a><b>output-secret-value-1</b></a>", "syntax": "xml"},
            {"text": "plain output-secret-value-1", "syntax": null},
            {"title": "== sent ==", "headers": [["Authorization", "ab"], ["X-Plain", "visible"]],
             "sensitive_headers": ["authorization"]},
        ],
    });
    let dir = project("output-forms", &answers(&reply), ECHO_GROUP);
    let out = run(&dir);
    let (stdout, stderr) = (
        stdout_of(&out),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    );
    assert!(out.status.success(), "{stdout}{stderr}");
    assert!(
        stderr.contains("\"user\": \"ann\""),
        "json is pretty-printed: {stderr}"
    );
    assert!(!stderr.contains("hunter2hunter2"), "{stderr}");
    assert!(!stderr.contains("output-secret-value-1"), "{stderr}");
    assert!(stderr.contains("<a>\n  <b>"), "xml is beautified: {stderr}");
    assert!(stderr.contains("Authorization : ***"), "{stderr}");
    assert!(stderr.contains("X-Plain       : visible"), "{stderr}");
    assert!(
        !stdout.contains("== sent =="),
        "output never reaches stdout: {stdout}"
    );

    let out = run_with(&dir, &["--reveal"], &[]);
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        stderr.contains("hunter2hunter2"),
        "--reveal shows it: {stderr}"
    );
    assert!(stderr.contains("Authorization : ab"), "{stderr}");
}

#[test]
fn a_polled_step_prints_its_output_once_per_attempt_and_a_bad_item_is_refused() {
    let script = serde_json::json!([
        {"status": "not_yet", "output": [{"text": "tick-marker"}]},
        {"status": "passed", "output": [{"text": "tick-marker"}]},
    ]);
    let dir = project("output-attempts", &polled(&script, "2"), ECHO_GROUP);
    let out = run(&dir);
    assert_green(&out);
    assert_eq!(
        String::from_utf8_lossy(&out.stderr)
            .matches("tick-marker")
            .count(),
        2
    );

    let reply = serde_json::json!({"status": "passed", "output": [{"nonsense": "secret-quote-9"}]});
    let dir = project("output-bad", &answers(&reply), ECHO_GROUP);
    let out = run(&dir);
    let all = format!(
        "{}{}",
        stdout_of(&out),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(all.contains("failed: 1 (1 plugin)"), "{all}");
    assert!(!all.contains("secret-quote-9"), "{all}");
}

#[test]
fn global_vars_write_the_global_frame_and_win_over_vars_for_one_name() {
    let reply = serde_json::json!({
        "status": "passed",
        "vars": {"both": "from-vars", "local": "kept"},
        "global_vars": {"both": "from-global", "only_global": "g"},
    });
    let feature = format!(
        "{}    Then variable \"both\" should be equal to \"from-global\"\n    And variable \"local\" should be equal to \"kept\"\n    And variable \"only_global\" should be equal to \"g\"\n",
        answers(&reply)
    );
    let dir = project("global-vars", &feature, ECHO_GROUP);
    let out = run(&dir);
    assert_green(&out);
    assert!(
        !stdout_of(&out).contains("shadows"),
        "a shadow this reply made is not warned about: {}",
        stdout_of(&out)
    );
}

#[test]
fn global_vars_warn_when_a_file_level_variable_shadows_them_and_a_failure_writes_nothing() {
    let reply = serde_json::json!({"status": "passed", "global_vars": {"seen": "g"}});
    let feature = format!(
        "Feature: f\n  Scenario: s\n    Given set variable \"seen\" to \"file\"\n{}    Then variable \"seen\" should be equal to \"file\"\n",
        answers(&reply).replace("Feature: f\n  Scenario: s\n", "")
    );
    let dir = project("global-vars-shadow", &feature, ECHO_GROUP);
    let out = run(&dir);
    assert_green(&out);
    assert!(
        stdout_of(&out).contains("shadows it"),
        "{}",
        stdout_of(&out)
    );

    let failing =
        serde_json::json!({"status": "fatal", "error": "no", "global_vars": {"ghost": "g"}});
    let feature = format!(
        "{}  Scenario: t\n    Then variable \"ghost\" should be equal to \"g\"\n",
        answers(&failing)
    );
    let dir = project("global-vars-failed", &feature, ECHO_GROUP);
    let text = stdout_of(&run(&dir));
    assert!(
        text.contains("failed: 2"),
        "the second scenario sees no ghost: {text}"
    );
}

#[test]
fn source_dir_is_the_absolute_directory_of_the_file_a_step_is_written_in() {
    let dir = project(
        "source-dir",
        "Feature: f\n  Scenario: s\n    When I echo the source directory as \"ok\"\n    Then variable \"echo_source_dir\" should be equal to \"@HERE@/features\"\n",
        ECHO_GROUP,
    );
    let here = std::path::absolute(&dir).unwrap().display().to_string();
    let feature = std::fs::read_to_string(dir.join("features/plugin.feature")).unwrap();
    std::fs::write(
        dir.join("features/plugin.feature"),
        feature.replace("@HERE@", &here),
    )
    .unwrap();
    // `paths: [features]` is relative: the run was started with a relative path.
    assert_green(&run(&dir));

    // The same value reaches `bddkit_validate_step`, before any request.
    let dir = project(
        "source-dir-validate",
        "Feature: f\n  Scenario: s\n    When I echo the source directory as \"refuse\"\n",
        ECHO_GROUP,
    );
    let out = run(&dir);
    let all = format!(
        "{}{}",
        stdout_of(&out),
        String::from_utf8_lossy(&out.stderr)
    );
    let here = std::path::absolute(&dir).unwrap().display().to_string();
    assert_eq!(out.status.code(), Some(2), "{all}");
    assert!(
        all.contains(&format!("source_dir={here}/features")),
        "{all}"
    );
}

#[test]
fn source_dir_follows_an_included_file_and_a_macro_body() {
    // Outside `paths`, so the include is the only way in; the macro's file
    // lives in `macros/`. Each step must see the directory it is written in.
    let dir = macro_project(
        "source-dir-places",
        "Feature: f\n  Scenario: s\n    When I include \"../inc/inc.feature\"\n    When I report from a macro\n    Then variable \"echo_source_dir\" should be equal to \"@HERE@/macros\"\n",
        "- step: 'I report from a macro'\n  do: [I echo the source directory as \"ok\"]\n  exports: [echo_source_dir]\n",
    );
    std::fs::create_dir_all(dir.join("inc")).unwrap();
    std::fs::write(
        dir.join("inc/inc.feature"),
        "Feature: inc\n  Scenario: i\n    When I echo the source directory as \"ok\"\n    Then variable \"echo_source_dir\" should be equal to \"@HERE@/inc\"\n",
    )
    .unwrap();
    let here = std::path::absolute(&dir).unwrap().display().to_string();
    let inc = dir.join("inc/inc.feature");
    std::fs::write(
        &inc,
        std::fs::read_to_string(&inc)
            .unwrap()
            .replace("@HERE@", &here),
    )
    .unwrap();
    let path = dir.join("features/plugin.feature");
    let feature = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, feature.replace("@HERE@", &here)).unwrap();
    assert_green(&run(&dir));

    // And at validation, for the macro body.
    let dir = macro_project(
        "source-dir-macro-validate",
        "Feature: f\n  Scenario: s\n    When I refuse from a macro\n",
        "- step: 'I refuse from a macro'\n  do: [I echo the source directory as \"refuse\"]\n",
    );
    let out = run(&dir);
    let all = format!(
        "{}{}",
        stdout_of(&out),
        String::from_utf8_lossy(&out.stderr)
    );
    let here = std::path::absolute(&dir).unwrap().display().to_string();
    assert_eq!(out.status.code(), Some(2), "{all}");
    assert!(all.contains(&format!("source_dir={here}/macros")), "{all}");
}

#[test]
fn output_masks_a_headers_title_and_names_and_a_value_the_reply_just_stored() {
    let reply = serde_json::json!({
        "status": "passed",
        "secrets": ["declared-title-secret-1", "declared-name-secret-2"],
        "vars": {"my_secret": "stored-value-3"},
        "output": [
            {"title": "== declared-title-secret-1 ==",
             "headers": [["declared-name-secret-2", "v"]]},
            {"text": "stored stored-value-3"},
        ],
    });
    let dir = project("output-masks-all", &answers(&reply), ECHO_GROUP);
    let stderr = String::from_utf8_lossy(&run(&dir).stderr).into_owned();
    for leaked in [
        "declared-title-secret-1",
        "declared-name-secret-2",
        "stored-value-3",
    ] {
        assert!(!stderr.contains(leaked), "{leaked}: {stderr}");
    }
    assert!(stderr.contains("== *** =="), "{stderr}");
    let stderr = String::from_utf8_lossy(&run_with(&dir, &["--reveal"], &[]).stderr).into_owned();
    assert!(stderr.contains("declared-title-secret-1"), "{stderr}");
    assert!(stderr.contains("stored-value-3"), "{stderr}");
}

#[test]
fn a_class_reaches_the_junit_type() {
    let reply = serde_json::json!({"status": "fatal", "error": "down", "class": "transport"});
    let dir = project("class-junit", &answers(&reply), ECHO_GROUP);
    run_with(&dir, &["--junit", "junit.xml"], &[]);
    let junit = std::fs::read_to_string(dir.join("junit.xml")).unwrap();
    assert_eq!(junit.matches("type=\"transport\"").count(), 1, "{junit}");
}

#[test]
fn a_global_written_in_a_macro_body_outlives_the_macro() {
    // The macro exports nothing, so a frame value would be gone with it; a
    // `global_vars` one is in the global frame and is not.
    let reply = serde_json::json!({"status": "passed", "global_vars": {"kept": "y"}});
    let dir = macro_project(
        "global-in-macro",
        "Feature: f\n  Scenario: s\n    When I answer from a macro\n    Then variable \"kept\" should be equal to \"y\"\n",
        &format!(
            "- step: 'I answer from a macro'\n  do:\n    - the echo answers: |\n        {reply}\n"
        ),
    );
    assert_green(&run(&dir));
}

#[test]
fn a_refused_output_writes_no_variable_and_a_skipped_var_is_still_masked() {
    let reply = serde_json::json!({
        "status": "passed", "vars": {"written": "w"}, "output": [{"nonsense": 1}],
        "diagnostics": [{"title": "kept-title", "kind": "text", "content": "x"}],
    });
    let feature = format!(
        "{}    Then variable \"written\" should be equal to \"w\"\n",
        answers(&reply)
    );
    let dir = project("output-refused-vars", &feature, ECHO_GROUP);
    let text = stdout_of(&run(&dir));
    assert!(text.contains("fits neither shape"), "{text}");
    assert!(
        text.contains("kept-title"),
        "the diagnostics still show: {text}"
    );
    assert!(
        !text.contains("is not equal") && !text.contains("expected: w"),
        "{text}"
    );

    let reply = serde_json::json!({
        "status": "passed",
        "vars": {"my_secret": "skipped-val-4"}, "global_vars": {"my_secret": "g"},
        "output": [{"text": "quoted skipped-val-4"}],
    });
    let dir = project("output-skipped-var", &answers(&reply), ECHO_GROUP);
    let stderr = String::from_utf8_lossy(&run(&dir).stderr).into_owned();
    assert!(!stderr.contains("skipped-val-4"), "{stderr}");
}

// ---- bddkit_failure_context ------------------------------------------------

const TWO_ECHOES: &str = "  echo:\n    main:\n      prefix: \"p-\"\n    other:\n      prefix: \"o-\"\ndefault_echo: main\n";

/// A failing core step: no plugin replied, so the whole dump is contexts.
const CORE_FAILS: &str = "    Then variable \"seen\" should be equal to \"never\"\n";

fn position(text: &str, needle: &str) -> usize {
    text.find(needle)
        .unwrap_or_else(|| panic!("{needle:?} is not in {text}"))
}

#[test]
fn a_failing_core_step_shows_each_used_plugin_in_first_use_order() {
    let dir = project(
        "context-order",
        &format!(
            "Feature: f\n  Scenario: s\n    Given I use \"other\" echo\n    When I echo \"x\" as \"seen\"\n    And I use \"main\" echo\n    And I echo \"y\" as \"seen\"\n{CORE_FAILS}"
        ),
        TWO_ECHOES,
    );
    let text = stdout_of(&run(&dir));
    // First used `other`, then `main`: not alphabetical, not the config's order.
    assert!(
        position(&text, "context of o-") < position(&text, "context of p-"),
        "{text}"
    );
    assert_eq!(text.matches("context of p-").count(), 1, "{text}");
}

#[test]
fn a_context_comes_only_from_a_plugin_this_scenario_used() {
    let dir = project(
        "context-scenario",
        &format!(
            "Feature: f\n  Scenario: main\n    When I echo \"x\" as \"seen\"\n  Scenario: other\n    Given I use \"other\" echo\n    When I echo \"x\" as \"seen\"\n  Scenario: main again\n    When I echo \"x\" as \"seen\"\n{CORE_FAILS}"
        ),
        TWO_ECHOES,
    );
    let text = stdout_of(&run(&dir));
    // The third scenario reuses a handle the first one created, which the
    // file-wide cache already holds: it must still be listed.
    assert!(text.contains("context of p-"), "{text}");
    assert!(!text.contains("context of o-"), "{text}");
}

#[test]
fn a_plugin_step_whose_reply_showed_a_diagnostic_gets_no_context_of_its_own() {
    let dir = project(
        "context-own-covered",
        "Feature: f\n  Scenario: s\n    Given I use \"other\" echo\n    When I echo \"x\" as \"seen\"\n    And I use \"main\" echo\n    Then the echo should fail\n",
        TWO_ECHOES,
    );
    let text = stdout_of(&run(&dir));
    assert!(text.contains("echo state"), "{text}");
    assert!(text.contains("context of o-"), "{text}");
    assert!(!text.contains("context of p-"), "{text}");
}

#[test]
fn a_plugin_step_whose_reply_showed_nothing_gets_its_own_context() {
    let reply = serde_json::json!({"status": "fatal", "error": "refused"});
    let dir = project("context-own-bare", &answers(&reply), ECHO_GROUP);
    let text = stdout_of(&run(&dir));
    assert!(text.contains("refused"), "{text}");
    assert!(text.contains("context of p-"), "{text}");
}

#[test]
fn a_failing_macro_shows_each_context_once() {
    let dir = macro_project(
        "context-macro-bare",
        "Feature: f\n  Scenario: s\n    When I check the echo twice\n",
        "- step: 'I check the echo twice'\n  do:\n    - I echo \"x\" as \"seen\"\n    - variable \"seen\" should be equal to \"never\"\n",
    );
    let text = stdout_of(&run(&dir));
    assert_eq!(text.matches("context of implicit-").count(), 1, "{text}");
}

#[test]
fn a_context_is_masked_unless_revealed() {
    let dir = project(
        "context-mask",
        &format!("Feature: f\n  Scenario: s\n    When I echo \"x\" as \"seen\"\n{CORE_FAILS}"),
        ECHO_GROUP,
    );
    let masked = stdout_of(&run(&dir));
    assert!(masked.contains("context of p-"), "{masked}");
    assert!(!masked.contains("context-pass-123"), "{masked}");
    let revealed = stdout_of(&run_with(&dir, &["--reveal"], &[]));
    assert!(revealed.contains("context-pass-123"), "{revealed}");
}

#[test]
fn a_context_is_in_no_evidence_event() {
    let dir = project(
        "context-evidence",
        &format!("Feature: f\n  Scenario: s\n    When I echo \"x\" as \"seen\"\n{CORE_FAILS}"),
        ECHO_GROUP,
    );
    let out = run_with(&dir, &["--events", "events.ndjson", "--evidence"], &[]);
    assert!(stdout_of(&out).contains("context of p-"));
    // The failure text, dump included, travels in `scenario_finished`; what
    // must not exist is an `evidence` event made from the context.
    for event in read_stream(&dir) {
        if event["type"] == "evidence" {
            assert!(!event.to_string().contains("context of"), "{event}");
        }
    }
}

#[test]
fn a_bad_context_reply_adds_one_line_and_leaves_the_failure_alone() {
    for mode in ["panic", "null", "malformed", "error"] {
        let dir = project(
            &format!("context-bad-{mode}"),
            &format!("Feature: f\n  Scenario: s\n    When I echo \"x\" as \"seen\"\n{CORE_FAILS}"),
            ECHO_GROUP,
        );
        let out = run_with(
            &dir,
            &["--events", "events.ndjson"],
            &[("BDDKIT_ECHO_FIXTURE_CONTEXT", mode)],
        );
        let text = stdout_of(&out);
        assert_eq!(out.status.code(), Some(1), "{mode}: {text}");
        assert_eq!(
            text.matches("failure context of plugin \"echo\" unavailable:")
                .count(),
            1,
            "{mode}: {text}"
        );
        assert!(!text.contains("context of p-"), "{mode}: {text}");
        assert!(!text.contains("context-malformed-secret"), "{mode}: {text}");
        // The plugin's own error is shown, masked by what the same reply declared.
        assert!(!text.contains("context-error-secret"), "{mode}: {text}");
        if mode == "error" {
            assert!(text.contains("the echo context refused ***"), "{text}");
        }
        let stream = read_stream(&dir);
        let finished = stream
            .iter()
            .find(|e| e["type"] == "scenario_finished")
            .unwrap();
        assert_eq!(finished["failure_kind"], "assertion", "{mode}");
    }
}

#[test]
fn an_empty_context_adds_nothing() {
    let dir = project(
        "context-empty",
        &format!("Feature: f\n  Scenario: s\n    When I echo \"x\" as \"seen\"\n{CORE_FAILS}"),
        ECHO_GROUP,
    );
    let text = stdout_of(&run_with(
        &dir,
        &[],
        &[("BDDKIT_ECHO_FIXTURE_CONTEXT", "empty")],
    ));
    assert!(!text.contains("failure context"), "{text}");
    assert!(!text.contains("context of"), "{text}");
}
