use super::{
    CHECKED_MACRO, ECHO_GROUP, MACRO_CALL, assert_green, doctor, macro_project, project, run,
    worker_project,
};
use crate::common;
use std::process::Command;

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
