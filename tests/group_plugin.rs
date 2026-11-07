//! `handle_scope: "group"`, `eager_init` and `deprecated`, end to end over the
//! `group-plugin` fixture (`tests/fixtures/group-plugin`, whose header lists
//! the environment switches). The calls the plugin receives are read from its
//! log, so a test sees `init`, `select`, `reset` and `drop` from outside.

mod common;

use std::path::PathBuf;
use std::process::Command;

struct Project {
    dir: PathBuf,
    log: PathBuf,
}

/// `hooks` picks the build that exports `reset_scenario` and `select_instance`.
fn project(
    name: &str,
    hooks: bool,
    concurrency: usize,
    files: &[(&str, &str)],
    config_tail: &str,
) -> Project {
    let dir = std::env::temp_dir().join(format!("bddkit-group-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("features")).expect("mkdir features");
    std::fs::create_dir_all(dir.join("macros")).expect("mkdir macros");
    std::fs::create_dir_all(dir.join(".bddkit")).expect("mkdir .bddkit");
    for (file, body) in files {
        std::fs::write(dir.join("features").join(file), body).expect("write feature");
    }
    std::fs::write(
        dir.join(".bddkit/plugins.yaml"),
        format!(
            "plugin:\n  - name: group\n    path: {}\n",
            common::build_group_plugin(hooks).display()
        ),
    )
    .expect("write lock");
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "paths: [features]\nmacro_paths: [macros]\nconcurrency: {concurrency}\n\
             default_gone: a\nresources:\n  gone:\n    a: {{}}\n    b: {{}}\n{config_tail}"
        ),
    )
    .expect("write config");
    let log = dir.join("calls.log");
    Project { dir, log }
}

impl Project {
    fn bddkit(&self, command: &str, envs: &[(&str, &str)]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_bddkit"))
            .args([command, "--config", "cfg.yaml", "--bddkit-dir"])
            .arg(self.dir.join(".bddkit"))
            .current_dir(&self.dir)
            .env("BDDKIT_GROUP_FIXTURE_LOG", &self.log)
            .envs(envs.iter().copied())
            .output()
            .expect("failed to run bddkit")
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

fn count(calls: &[String], prefix: &str) -> usize {
    calls.iter().filter(|c| c.starts_with(prefix)).count()
}

fn text(out: &std::process::Output) -> String {
    format!(
        "--- stdout ---\n{}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn passed(out: &std::process::Output) {
    assert!(out.status.success(), "{}", text(out));
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("failed: 0"),
        "{}",
        text(out)
    );
}

const PER_WORKER: &[(&str, &str)] = &[("BDDKIT_GROUP_FIXTURE_MODE", "per_worker")];

const SWITCHING: &str = r#"Feature: switch
  Scenario: state set before a switch is visible after it
    Given I use "a" gone
    When I set the group value to "kept"
    And I use "b" gone
    And I read the group value into "value"
    Then variable "value" should be equal to "kept"
    When I read the selected instance into "who"
    Then variable "who" should be equal to "b"

  Scenario: a second scenario resets the one handle once
    When I read the group handle into "h"
"#;

#[test]
fn per_worker_two_files_get_two_handles_and_a_switch_keeps_the_state() {
    let p = project(
        "per-worker",
        true,
        2,
        &[("a.feature", SWITCHING), ("b.feature", SWITCHING)],
        "",
    );
    passed(&p.bddkit("run", PER_WORKER));
    let calls = p.calls();
    // One init per file, whatever the number of instances or switches.
    assert_eq!(
        count(&calls, "init gone instances=2 concurrency=2"),
        2,
        "{calls:?}"
    );
    // The first step of the group is a `use` with no handle yet: no notice.
    // The second one, with a handle, is told.
    assert_eq!(count(&calls, "select gone.b"), 2, "{calls:?}");
    assert_eq!(count(&calls, "select"), 2, "{calls:?}");
    // Reset once per scenario per handle, dropped once at the end of its file.
    assert_eq!(count(&calls, "reset"), 2, "{calls:?}");
    assert_eq!(count(&calls, "drop"), 2, "{calls:?}");
}

#[test]
fn a_deprecated_word_notifies_a_group_scoped_handle_like_the_group_step() {
    let p = project(
        "deprecated-select",
        true,
        1,
        &[(
            "a.feature",
            r#"Feature: deprecated select
  Scenario: the old word reaches select_instance
    Given I use "a" gone
    When I set the group value to "kept"
    And I use "b" oldswitch
    And I read the group value into "value"
    Then variable "value" should be equal to "kept"
"#,
        )],
        "",
    );
    passed(&p.bddkit("run", PER_WORKER));
    assert_eq!(count(&p.calls(), "select gone.b"), 1, "{:?}", p.calls());
}

#[test]
fn a_file_whose_first_group_step_is_a_use_makes_no_select_call() {
    let p = project(
        "first-use",
        true,
        1,
        &[(
            "a.feature",
            r#"Feature: first
  Scenario: the selection rides on the first dispatch
    Given I use "b" gone
    When I read the selected instance into "who"
    Then variable "who" should be equal to "b"
"#,
        )],
        "",
    );
    passed(&p.bddkit("run", PER_WORKER));
    assert_eq!(count(&p.calls(), "select"), 0, "{:?}", p.calls());
}

#[test]
fn a_failing_select_instance_fails_the_step_with_the_plugins_error() {
    let p = project("select-fails", true, 1, &[("a.feature", SWITCHING)], "");
    let out = p.bddkit(
        "run",
        &[
            ("BDDKIT_GROUP_FIXTURE_MODE", "per_worker"),
            ("BDDKIT_GROUP_FIXTURE_FAIL_SELECT", "1"),
        ],
    );
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(text(&out).contains("select refused"), "{}", text(&out));
}

#[test]
fn two_groups_of_one_plugin_get_two_handles_each_reset_and_dropped_once() {
    let p = project(
        "two-groups",
        true,
        1,
        &[(
            "a.feature",
            r#"Feature: two groups
  Scenario: one handle per group
    Given I use "a" gone
    And I use "x" gtwo
    When I read the group handle into "h1"
    And I read the gtwo handle into "h2"
    Then variable "h1" should not be equal to "<<h2>>"

  Scenario: a second scenario resets each handle once
    When I read the group handle into "h"
"#,
        )],
        "  gtwo:\n    x: {}\n",
    );
    passed(&p.bddkit("run", PER_WORKER));
    let calls = p.calls();
    assert_eq!(count(&calls, "init gone"), 1, "{calls:?}");
    assert_eq!(count(&calls, "init gtwo instances=1"), 1, "{calls:?}");
    assert_eq!(count(&calls, "reset"), 2, "{calls:?}");
    assert_eq!(count(&calls, "drop"), 2, "{calls:?}");
}

const BARRIER: &str = r#"Feature: barrier
  Scenario: both files are inside the one handle at once
    Given I use "a" gone
    When I wait at the group barrier for 2 parties
    Then I read the group handle into "h"
"#;

#[test]
fn shared_one_handle_serves_two_files_at_the_same_time() {
    let p = project(
        "shared",
        false,
        2,
        &[("a.feature", BARRIER), ("b.feature", BARRIER)],
        "",
    );
    passed(&p.bddkit("run", &[]));
    // Eager init made the one handle; nothing made another afterwards.
    assert_eq!(count(&p.calls(), "init"), 1, "{:?}", p.calls());
}

/// Negative control: one worker cannot release a two-party barrier, so the
/// test above is not green for a reason other than real concurrency.
#[test]
fn one_worker_cannot_release_the_group_barrier() {
    let p = project(
        "shared-sequential",
        false,
        1,
        &[("a.feature", BARRIER), ("b.feature", BARRIER)],
        "",
    );
    assert_eq!(p.bddkit("run", &[]).status.code(), Some(1));
}

#[test]
fn a_shared_plugin_exporting_select_instance_is_refused_in_parallel() {
    let p = project("select-fence", true, 2, &[("a.feature", BARRIER)], "");
    let out = p.bddkit("run", &[]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    assert!(
        text(&out).contains("bddkit_select_instance"),
        "{}",
        text(&out)
    );
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("run "),
        "{}",
        text(&out)
    );
}

#[test]
fn a_failing_eager_init_exits_2_names_the_instance_and_prints_no_run_id() {
    let p = project("eager-fail", false, 1, &[("a.feature", BARRIER)], "");
    let out = p.bddkit("run", &[("BDDKIT_GROUP_FIXTURE_FAIL_INSTANCE", "b")]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    assert!(text(&out).contains("resources.gone.b"), "{}", text(&out));
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("run "),
        "{}",
        text(&out)
    );
}

#[test]
fn doctor_makes_no_eager_init() {
    let p = project("doctor-no-init", false, 1, &[("a.feature", SWITCHING)], "");
    let out = p.bddkit("doctor", &[]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(count(&p.calls(), "init"), 0, "{:?}", p.calls());
    // Control: the same project does init when it runs.
    passed(&p.bddkit("run", &[]));
    assert_eq!(count(&p.calls(), "init"), 1, "{:?}", p.calls());
}

fn write_deprecated_suite(p: &Project) {
    let features = p.dir.join("features");
    std::fs::write(
        features.join("direct.feature"),
        r#"Feature: direct
  Scenario: the deprecated word selects the instance
    Given I use "b" oldswitch
    When I read the selected instance into "who"
    Then variable "who" should be equal to "b"
"#,
    )
    .expect("write");
    std::fs::create_dir_all(p.dir.join("helpers")).expect("mkdir helpers");
    std::fs::write(
        p.dir.join("helpers/helper.feature"),
        r#"Feature: helper
  Scenario: helper
    Given I use "b" oldswitch
"#,
    )
    .expect("write");
    std::fs::write(
        features.join("included.feature"),
        r#"Feature: included
  Scenario: through an include
    Given I include "../helpers/helper.feature"
    When I read the selected instance into "who"
    Then variable "who" should be equal to "b"
"#,
    )
    .expect("write");
    std::fs::write(
        p.dir.join("macros/m.yaml"),
        "- step: 'I pick b the old way'\n  do:\n    - I use \"b\" oldswitch\n",
    )
    .expect("write");
    std::fs::write(
        features.join("macro.feature"),
        r#"Feature: macro
  Scenario: through a macro
    Given I pick b the old way
    When I read the selected instance into "who"
    Then variable "who" should be equal to "b"
"#,
    )
    .expect("write");
}

#[test]
fn the_deprecated_word_selects_the_same_instance_and_only_doctor_says_so() {
    let p = project("deprecated", false, 1, &[], "");
    write_deprecated_suite(&p);
    let run = p.bddkit("run", &[]);
    passed(&run);
    assert!(!text(&run).contains("deprecated"), "{}", text(&run));

    let doctor = p.bddkit("doctor", &[]);
    assert!(doctor.status.success(), "{}", text(&doctor));
    let shown = text(&doctor);
    for place in [
        "direct.feature:3",
        "helper.feature:3",
        r#"macro "I pick b the old way""#,
    ] {
        assert!(shown.contains(place), "no hint for {place}\n{shown}");
    }
    assert!(shown.contains(r#"write I use "b" gone"#), "{shown}");
}

#[test]
fn a_manifest_the_host_cannot_accept_is_exit_2_and_names_the_culprit() {
    for (case, concurrency, wants) in [
        ("target", 1, "elsewhere"),
        ("group", 1, "gtwo"),
        ("host", 1, "\"db\""),
        ("eager_unclaimed", 1, "elsewhere"),
        ("eager_per_worker", 1, "eager_init"),
    ] {
        let p = project(
            &format!("break-{case}"),
            false,
            concurrency,
            &[("a.feature", BARRIER)],
            "",
        );
        let out = p.bddkit("run", &[("BDDKIT_GROUP_FIXTURE_BREAK", case)]);
        assert_eq!(out.status.code(), Some(2), "{case}\n{}", text(&out));
        assert!(text(&out).contains(wants), "{case}\n{}", text(&out));
    }
}

#[test]
fn the_context_is_asked_before_the_reset_of_the_next_scenario() {
    let p = project(
        "context-order",
        true,
        1,
        &[(
            "a.feature",
            r#"Feature: context
  Scenario: fails after using the handle
    When I read the group handle into "h"
    Then variable "h" should be equal to "never"

  Scenario: the next one resets
    When I read the group handle into "h"
"#,
        )],
        "",
    );
    let out = p.bddkit("run", PER_WORKER);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    let calls = p.calls();
    assert_eq!(count(&calls, "context"), 1, "{calls:?}");
    let at = |prefix: &str| calls.iter().position(|c| c.starts_with(prefix)).unwrap();
    assert!(at("context") < at("reset"), "{calls:?}");
}
