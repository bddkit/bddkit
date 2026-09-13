//! End-to-end acceptance for the plugin layer: the real `bddkit` binary, run
//! as a subprocess, against the fixture plugin built by `common::build_fixture_plugin`.
//! No test here needs the axum stub, so plain `#[test]` is right — nothing
//! spawns a server for `Command::output()` to starve.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

/// A throwaway project: config, feature file, and the hand-written lock file
/// the P1 loader reads. `plugin install` is a later milestone.
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
        r#"Feature: plugin assertions
  Scenario: no modifier means no second attempt
    Then the echo counter should reach 3
"#,
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
        r#"Feature: plugin failures
  Scenario: a fatal result carries its evidence
    Then the echo should fail
"#,
        ECHO_GROUP,
    );
    let out = run(&dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("the echo step was asked to fail"), "{stdout}");
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
    assert!(stdout.contains("the echo instance refuses to reset"), "{stdout}");
    // Exactly the second scenario failed — not both, which Some(1) alone
    // would equally allow.
    assert!(stdout.contains("failed: 1"), "{stdout}");
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
        r#"Feature: plugin failures
  Scenario: a fatal result still leaves shutdown to run
    Then the echo should fail
"#,
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
    assert!(stderr.contains("per_worker"), "the remedy is named: {stderr}");
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
            (
                "b.feature",
                holder.as_str(),
            ),
        ],
        &format!("  worker:\n    main:\n      drop_log: \"{}\"\n", log.display()),
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
        recorded = std::fs::read_to_string(&log)
            .map(|text| text.lines().count())
            .unwrap_or(0);
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
    assert!(!stderr.contains("FAIL"), "the run must be healthy\n{stderr}");
    assert_eq!(
        recorded, 1,
        "the finished file's instance must be dropped while the other file still holds its own\n--- stderr ---\n{stderr}"
    );
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
    assert!(bare_out.contains("api:"), "{bare_out}");
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
            "resource", "add", "echo", "third", "--config", "cfg.yaml", "--prefix", "q-", "--loud",
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
        static_out.contains("plugin echo.main") && static_out.contains("skipped"),
        "a bare doctor must not call the plugin's probe:\n{static_out}"
    );

    let (live_out, code) = doctor(&["doctor", "--config", "cfg.yaml", "--live"]);
    assert_eq!(code, Some(0), "{live_out}");
    assert!(
        live_out.contains("plugin echo.main") && live_out.contains("probed clean"),
        "{live_out}"
    );
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
    assert!(stdout.contains("plugin echo.main"), "{stdout}");
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
    for (name, tail) in [("implicit-off", ECHO_GROUP), ("implicit-off-empty", "  echo: {}\n")] {
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
        &[("a.feature", "Feature: a\n  Scenario: s\n    When I count in the worker as \"n\"\n")],
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
        stdout.contains("plugin echo.default") && stdout.contains("probed clean"),
        "{stdout}"
    );
}
