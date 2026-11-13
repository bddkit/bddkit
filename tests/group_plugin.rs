//! `handle_scope: "group"`, `eager_init` and `deprecated`, end to end over the
//! `group-plugin` fixture (`tests/fixtures/group-plugin`, whose header lists
//! the environment switches). The calls the plugin receives are read from its
//! log, so a test sees `init`, `select`, `reset` and `drop` from outside.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    reason = "`allow-*-in-tests` covers only `#[test]` functions, not the helpers here"
)]

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

/// `api`, `srp` and `db` are claimed by the fixture itself, as `bddkit-api` and
/// `bddkit-db` claim them.
const HOST_GROUPS: &[(&str, &str)] = &[("BDDKIT_GROUP_FIXTURE_HOST_GROUPS", "1")];

fn host_groups_project(name: &str, feature: &str, config: &str) -> Project {
    let project = project(
        &format!("host-{name}"),
        false,
        1,
        &[("f.feature", feature)],
        "",
    );
    std::fs::write(project.dir.join("cfg.yaml"), config).expect("write config");
    project
}

/// The fixture claiming `db` alone, as `bddkit-db` does: the databases left
/// the host, so every build accepts it, with `connection` as its deprecated word.
const DB_GROUP: &[(&str, &str)] = &[("BDDKIT_GROUP_FIXTURE_HOST_GROUPS", "db")];

const DB_RESOURCES: &str =
    "paths: [features]\nconcurrency: 1\nresources:\n  db:\n    main: {}\n    other: {}\n";

/// Reads which `db` instance a scenario starts on.
const STARTS_ON: &str = r#"Feature: f
  Scenario: the default decides what a scenario starts on
    When I read the selected instance into "db"
    Then variable "db" should be equal to "other"
"#;

#[test]
fn a_plugin_serves_the_db_group_and_its_switch_steps() {
    let project = host_groups_project(
        "db-switch",
        r#"Feature: f
  Scenario: the generated switch step and the former word both select an instance
    When I use "other" db
    And I read the selected instance into "db"
    Then variable "db" should be equal to "other"
    When I use "main" connection
    And I read the selected instance into "db"
    Then variable "db" should be equal to "main"
"#,
        &format!("{DB_RESOURCES}default_db: main\n"),
    );
    passed(&project.bddkit("run", DB_GROUP));
}

/// Constraint 4: the config key, the flag and the variable a built-in `db`
/// had keep choosing the instance a scenario starts on.
#[test]
fn default_db_the_default_db_flag_and_its_variable_select_the_connection() {
    let project = host_groups_project(
        "db-default-key",
        STARTS_ON,
        &format!("{DB_RESOURCES}default_db: other\n"),
    );
    passed(&project.bddkit("run", DB_GROUP));

    let project = host_groups_project("db-default-flags", STARTS_ON, DB_RESOURCES);
    for args in [&["--default-db", "other"][..], &["--default", "db=other"]] {
        let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
            .arg("run")
            .args(args)
            .args(["--config", "cfg.yaml", "--bddkit-dir"])
            .arg(project.dir.join(".bddkit"))
            .current_dir(&project.dir)
            .envs(DB_GROUP.iter().copied())
            .output()
            .expect("failed to run bddkit");
        passed(&out);
    }
    let variable = [DB_GROUP[0], ("BDDKIT_DEFAULT_DB", "other")];
    passed(&project.bddkit("run", &variable));
}

#[test]
fn default_db_without_resources_db_is_refused() {
    let project = host_groups_project(
        "db-stray-default",
        "Feature: f\n  Scenario: s\n    Given I sleep 0 seconds\n",
        "paths: [features]\nresources:\n  gone:\n    a: {}\ndefault_db: main\n",
    );
    let out = project.bddkit("run", &[]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    assert!(text(&out).contains("default_db"), "{}", text(&out));
}

#[test]
fn doctor_gives_each_db_instance_a_row_and_hints_at_the_deprecated_word() {
    let project = host_groups_project(
        "db-doctor",
        "Feature: f\n  Scenario: s\n    When I use \"other\" connection\n",
        &format!("{DB_RESOURCES}default_db: main\n"),
    );
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--json", "--config", "cfg.yaml", "--bddkit-dir"])
        .arg(project.dir.join(".bddkit"))
        .current_dir(&project.dir)
        .envs(DB_GROUP.iter().copied())
        .output()
        .expect("failed to run bddkit");
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).expect("doctor --json");
    let checks = report["checks"].as_array().expect("checks");
    for target in ["main", "other"] {
        assert!(
            checks
                .iter()
                .any(|row| row["stage"] == "db" && row["target"] == target),
            "{target}: {checks:?}"
        );
    }
    let detail = checks
        .iter()
        .find(|row| row["stage"] == "steps")
        .and_then(|row| row["detail"].as_str())
        .expect("a steps row with a detail");
    assert!(
        detail.contains(r#"I use "other" connection is deprecated, write I use "other" db"#),
        "{detail}"
    );
}

mod former_built_ins {
    use super::*;

    impl Project {
        /// A command of its own, with the fixture claiming the host's groups.
        fn bddkit_with(&self, args: &[&str]) -> std::process::Output {
            Command::new(env!("CARGO_BIN_EXE_bddkit"))
                .args(args)
                .args(["--config", "cfg.yaml", "--bddkit-dir"])
                .arg(self.dir.join(".bddkit"))
                .current_dir(&self.dir)
                .envs(HOST_GROUPS.iter().copied())
                .output()
                .expect("failed to run bddkit")
        }
    }

    const RESOURCES: &str = "paths: [features]\nconcurrency: 1\nresources:\n  api:\n    a: {}\n    b: {}\n  db:\n    main: {}\n    other: {}\n  srp:\n    login: {}\ndefault_api: a\ndefault_db: main\n";

    #[test]
    fn a_plugin_claims_the_built_in_groups_and_their_switch_steps_are_generated() {
        let project = host_groups_project(
            "claims",
            r#"Feature: f
  Scenario: the generated switch steps select instances of the claimed groups
    When I use "b" api
    And I read the selected api instance into "who"
    Then variable "who" should be equal to "b"
    When I use "login" srp
    And I use "other" db
    And I read the selected instance into "db"
    Then variable "db" should be equal to "other"
"#,
            RESOURCES,
        );
        passed(&project.bddkit("run", HOST_GROUPS));
    }

    #[test]
    fn resources_api_reaches_the_plugins_validate_config() {
        let project = host_groups_project(
            "validate",
            "Feature: f\n  Scenario: s\n    Given I sleep 0 seconds\n",
            "paths: [features]\nresources:\n  api:\n    main:\n      refuse: not for this plugin\n",
        );
        let out = project.bddkit("run", HOST_GROUPS);
        assert_eq!(out.status.code(), Some(2), "{}", text(&out));
        assert!(text(&out).contains("not for this plugin"), "{}", text(&out));
        assert!(text(&out).contains("resources.api.main"), "{}", text(&out));
    }

    #[test]
    fn the_default_flag_and_variable_reach_a_group_that_was_built_in() {
        let feature = r#"Feature: f
  Scenario: the default decides what a scenario starts on
    When I read the selected api instance into "who"
    Then variable "who" should be equal to "b"
"#;
        let project = host_groups_project("default", feature, RESOURCES);
        passed(&project.bddkit_with(&["run", "--default", "api=b"]));
        let variable = [HOST_GROUPS[0], ("BDDKIT_DEFAULT_API", "b")];
        passed(&project.bddkit("run", &variable));
    }

    #[test]
    fn a_stray_default_of_a_group_nothing_declares_is_refused() {
        let project = host_groups_project(
            "stray",
            "Feature: f\n  Scenario: s\n    Given I sleep 0 seconds\n",
            "paths: [features]\ndefault_api: main\nresources:\n  db:\n    main: {}\n",
        );
        let out = project.bddkit("run", HOST_GROUPS);
        assert_eq!(out.status.code(), Some(2), "{}", text(&out));
        assert!(text(&out).contains("default_api"), "{}", text(&out));
    }

    #[test]
    fn a_built_in_step_is_unknown_unless_the_plugin_declares_it() {
        let project = host_groups_project(
            "unknown",
            "Feature: f\n  Scenario: s\n    When I request \"/x\"\n",
            RESOURCES,
        );
        let out = project.bddkit("run", HOST_GROUPS);
        assert_eq!(out.status.code(), Some(2), "{}", text(&out));
        assert!(text(&out).contains("unknown step"), "{}", text(&out));
    }

    #[test]
    fn a_plugin_may_declare_the_text_of_a_step_that_was_built_in() {
        let project = host_groups_project(
            "declares",
            "Feature: f\n  Scenario: s\n    Then the response code is 200\n",
            RESOURCES,
        );
        passed(&project.bddkit("run", HOST_GROUPS));
    }

    #[test]
    fn doctor_gives_each_instance_of_a_claimed_group_its_own_row() {
        // The `db` rows and the `connection` hint are
        // `doctor_gives_each_db_instance_a_row_and_hints_at_the_deprecated_word`'s.
        let project = host_groups_project(
            "doctor",
            "Feature: f\n  Scenario: s\n    When I use \"b\" api\n",
            RESOURCES,
        );
        let out = project.bddkit_with(&["doctor", "--json"]);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out));
        let report: serde_json::Value = serde_json::from_slice(&out.stdout).expect("doctor --json");
        let checks = report["checks"].as_array().expect("checks");
        for (stage, target) in [("api", "a"), ("api", "b"), ("srp", "login")] {
            assert!(
                checks
                    .iter()
                    .any(|row| row["stage"] == stage && row["target"] == target),
                "{stage} {target}: {checks:?}"
            );
        }
    }

    #[test]
    fn the_old_default_flags_still_name_the_group() {
        let feature = r#"Feature: f
  Scenario: the flag decides what a scenario starts on
    When I read the selected api instance into "who"
    Then variable "who" should be equal to "b"
"#;
        let project = host_groups_project("flag", feature, RESOURCES);
        passed(&project.bddkit_with(&["run", "--default-api", "b"]));
        let srp = host_groups_project(
            "srp-flag",
            "Feature: f\n  Scenario: s\n    Given I sleep 0 seconds\n",
            "paths: [features]\nresources:\n  srp:\n    a: {}\n    b: {}\n",
        );
        passed(&srp.bddkit_with(&["run", "--default-srp", "b"]));
        let variable = [HOST_GROUPS[0], ("BDDKIT_DEFAULT_SRP", "b")];
        passed(&srp.bddkit("run", &variable));
    }

    #[test]
    fn steps_list_shows_no_built_in_group() {
        let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
            .args(["steps", "list", "--no-config"])
            .output()
            .expect("failed to run bddkit");
        let listing = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "{}", text(&out));
        assert!(listing.contains("vars:"), "{listing}");
        for gone in [
            "api:",
            "db:",
            "srp:",
            "I request",
            "the response code",
            "Print response",
        ] {
            assert!(!listing.contains(gone), "{gone} in:\n{listing}");
        }
    }
}
