use super::{ECHO_GROUP, assert_green, project, project_at, run, run_with, worker_project};
use crate::common;
use std::path::PathBuf;
use std::process::Command;

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

    let out = run_with(&dir, &["--bddkit-dir", ".bddkit"], &[]); // not the user layer
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
