//! Telemetry end to end: the binary runs a small suite and the payload it
//! would send is read back from `BDDKIT_TELEMETRY_LOG`. The request itself is
//! caught by a one-connection collector on a loopback port.
//!
//! Every run here is cut off from this machine's own answer, environment and
//! CI variables: `BDDKIT_DIR` replaces the layer chain with a directory of the
//! test's, and the variables that decide are removed before a test sets its own.

mod common;

use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

const PLAIN: &str = "Feature: plain\n  Scenario: s\n    Given set variable \"x\" to \"1\"\n";

struct Suite {
    dir: PathBuf,
}

fn suite(name: &str, config: &str, features: &[(&str, &str)]) -> Suite {
    let dir = std::env::temp_dir().join(format!("bddkit-telemetry-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::create_dir_all(dir.join("state")).expect("mkdir");
    std::fs::write(
        dir.join("bddkit.yaml"),
        format!(
            "paths: [features]\nresources:\n  api:\n    stub:\n      base_url: http://127.0.0.1:9\n{config}"
        ),
    )
    .expect("config");
    for (file, text) in features {
        std::fs::write(dir.join("features").join(file), text).expect("feature");
    }
    Suite { dir }
}

impl Suite {
    fn state(&self) -> PathBuf {
        self.dir.join("state")
    }

    fn log_path(&self) -> PathBuf {
        self.dir.join("telemetry.log")
    }

    fn command(&self, args: &[&str], env: &[(&str, &str)]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_bddkit"));
        common::isolate_telemetry(&mut command)
            .args(args)
            .current_dir(&self.dir)
            .stdin(Stdio::null())
            .env("HOME", &self.dir)
            .env("BDDKIT_DIR", self.state())
            .env("BDDKIT_TELEMETRY_LOG", self.log_path())
            .envs(env.iter().copied());
        command
    }

    fn bddkit(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        self.command(args, env).output().expect("run bddkit")
    }

    /// The payloads recorded so far, one per line.
    fn payloads(&self) -> Vec<Value> {
        common::read_payloads(&self.log_path())
    }

    fn only_payload(&self) -> Value {
        let mut all = self.payloads();
        assert_eq!(all.len(), 1, "one payload per run: {all:?}");
        all.remove(0)
    }
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn keys(payload: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = payload
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    keys
}

#[test]
fn a_run_records_one_payload_with_only_schema_fields() {
    let suite = suite("schema", "", &[("a.feature", PLAIN)]);
    let out = suite.bddkit(&["run", "--telemetry", "true"], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));

    let payload = suite.only_payload();
    assert_eq!(
        keys(&payload),
        [
            "arch",
            "bddkit",
            "ci",
            "command",
            "concurrency",
            "duration_ms",
            "examples",
            "exit",
            "fail_fast",
            "failed",
            "features",
            "files",
            "os",
            "privacy",
            "resources",
            "scenario_ms",
            "scenarios",
            "schema",
            "stage",
            "usage",
        ]
    );
    assert_eq!(payload["schema"], 1);
    assert_eq!(payload["command"], "run");
    assert_eq!(payload["exit"], 0);
    assert_eq!(payload["files"], 1);
    assert_eq!(payload["scenarios"], 1);
    assert_eq!(payload["failed"], 0);
    assert_eq!(payload["resources"]["api"], 1);
    assert_eq!(payload["usage"]["vars"]["steps"], 1);
    assert!(payload["ci"].is_null() && payload["stage"].is_null());
}

#[test]
fn nothing_from_the_suite_reaches_the_payload() {
    // Every name a suite can choose carries the marker.
    let feature = "@marker_feature_tag\nFeature: marker_feature\n\n  @marker_scenario_tag\n  Scenario: marker_scenario\n    Given set variable \"marker_variable\" to \"marker_value\"\n    Then variable \"marker_variable\" should not be equal to \"marker_other\"\n";
    let suite = suite("marker", "", &[("marker_file.feature", feature)]);
    std::fs::write(
        suite.dir.join("bddkit.yaml"),
        "paths: [features]\nresources:\n  api:\n    marker_resource:\n      base_url: http://marker.invalid:9\n",
    )
    .unwrap();
    let out = suite.bddkit(
        &[
            "run",
            "--telemetry",
            "true",
            "--tag",
            "marker_scenario_tag",
            "--env",
            "marker_env",
        ],
        &[],
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let recorded = std::fs::read_to_string(suite.log_path()).expect("a payload");
    assert!(!recorded.contains("marker"), "{recorded}");
    let payload = suite.only_payload();
    assert_eq!(payload["features"]["tags"], true);
    assert_eq!(payload["features"]["env"], true);
}

#[test]
fn every_way_of_saying_no_records_nothing() {
    type Off<'a> = (&'a str, Vec<&'a str>, Vec<(&'a str, &'a str)>);
    let off: [Off; 4] = [
        ("flag", vec!["--telemetry", "false"], vec![]),
        ("variable", vec![], vec![("BDDKIT_TELEMETRY", "0")]),
        (
            "variable-false",
            vec![],
            vec![("BDDKIT_TELEMETRY", "false")],
        ),
        ("do-not-track", vec![], vec![("DO_NOT_TRACK", "1")]),
    ];
    for (name, flags, env) in off {
        let suite = suite(name, "", &[("a.feature", PLAIN)]);
        // A yes in the layer is below all of them.
        std::fs::write(suite.state().join("telemetry.yaml"), "enabled: true\n").unwrap();
        let mut args = vec!["run"];
        args.extend(flags);
        let out = suite.bddkit(&args, &env);
        assert_eq!(out.status.code(), Some(0), "{name}: {}", text(&out));
        assert!(suite.payloads().is_empty(), "{name}");
    }
}

#[test]
fn the_config_key_and_the_layer_file_decide_too() {
    let suite = suite("config-off", "telemetry: false\n", &[("a.feature", PLAIN)]);
    std::fs::write(suite.state().join("telemetry.yaml"), "enabled: true\n").unwrap();
    assert_eq!(suite.bddkit(&["run"], &[]).status.code(), Some(0));
    assert!(suite.payloads().is_empty(), "the suite's own false wins");

    let suite = self::suite("layer-on", "", &[("a.feature", PLAIN)]);
    std::fs::write(suite.state().join("telemetry.yaml"), "enabled: true\n").unwrap();
    assert_eq!(suite.bddkit(&["run"], &[]).status.code(), Some(0));
    assert_eq!(suite.only_payload()["command"], "run");

    let suite = self::suite("layer-off", "", &[("a.feature", PLAIN)]);
    std::fs::write(suite.state().join("telemetry.yaml"), "enabled: false\n").unwrap();
    assert_eq!(suite.bddkit(&["run"], &[]).status.code(), Some(0));
    assert!(suite.payloads().is_empty());
}

#[test]
fn nobody_has_decided_so_nothing_is_recorded_and_nothing_is_asked() {
    let suite = suite("undecided", "", &[("a.feature", PLAIN)]);
    let out = suite.bddkit(&["run"], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(suite.payloads().is_empty());
    let output = text(&out);
    assert!(
        !output.to_lowercase().contains("telemetry") && !output.contains("usage counts"),
        "run prints nothing about telemetry without a terminal: {output}"
    );
}

#[test]
fn bddkit_telemetry_wins_over_do_not_track() {
    let suite = suite("env-wins", "", &[("a.feature", PLAIN)]);
    let out = suite.bddkit(
        &["run"],
        &[("BDDKIT_TELEMETRY", "1"), ("DO_NOT_TRACK", "1")],
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert_eq!(suite.only_payload()["command"], "run");
}

#[test]
fn an_unreadable_config_sends_nothing() {
    let suite = suite("broken", "", &[("a.feature", PLAIN)]);
    std::fs::write(suite.dir.join("bddkit.yaml"), "paths: [features\n").unwrap();
    let out = suite.bddkit(&["run", "--telemetry", "true"], &[]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    assert!(suite.payloads().is_empty());
}

#[test]
fn a_refusal_after_the_config_reports_the_stage_it_stopped_in() {
    let unknown = "Feature: f\n  Scenario: s\n    Given nothing like this is a step\n";
    type Case<'a> = (
        &'a str,
        &'a str,
        &'a str,
        &'a [(&'a str, &'a str)],
        &'a [&'a str],
        &'a str,
    );
    let cases: [Case; 5] = [
        (
            "validation",
            "",
            "validation",
            &[("a.feature", unknown)],
            &[],
            "validation",
        ),
        (
            "selection",
            "",
            "selection",
            &[("a.feature", PLAIN)],
            &["--tag", "nothing"],
            "selection",
        ),
        (
            "scheduling",
            "",
            "scheduling",
            &[(
                "a.feature",
                "@priority(first)\nFeature: f\n  Scenario: s\n    Given set variable \"x\" to \"1\"\n",
            )],
            &[],
            "scheduling",
        ),
        (
            "steps",
            "macro_paths: [broken.yaml]\n",
            "steps",
            &[("a.feature", PLAIN)],
            &[],
            "steps",
        ),
        (
            "resources",
            "      timeout_secs: 0\n",
            "resources",
            &[("a.feature", PLAIN)],
            &[],
            "resources",
        ),
    ];
    for (name, extra, _, features, flags, stage) in cases {
        let suite = suite(name, extra, features);
        std::fs::write(suite.dir.join("broken.yaml"), "macros: [oops\n").unwrap();
        let mut args = vec!["run", "--telemetry", "true"];
        args.extend(flags);
        let out = suite.bddkit(&args, &[]);
        assert_eq!(out.status.code(), Some(2), "{name}: {}", text(&out));
        let payload = suite.only_payload();
        assert_eq!(payload["stage"], stage, "{name}: {payload}");
        assert_eq!(payload["exit"], 2, "{name}");
    }
}

/// `/dev/full` opens for writing and fails every write with ENOSPC.
#[cfg(target_os = "linux")]
#[test]
fn a_report_or_a_stream_that_cannot_be_written_names_its_stage() {
    for (flag, stage) in [("--junit", "reports"), ("--events", "events")] {
        let suite = suite(stage, "", &[("a.feature", PLAIN)]);
        let out = suite.bddkit(&["run", "--telemetry", "true", flag, "/dev/full"], &[]);
        assert_eq!(out.status.code(), Some(2), "{stage}: {}", text(&out));
        assert_eq!(suite.only_payload()["stage"], stage);
    }
}

#[test]
fn outline_rows_and_leaf_steps_are_counted_once() {
    let outline = "Feature: o\n  Scenario Outline: row\n    Given set variable \"x\" to \"<v>\"\n\n    Examples:\n      | v |\n      | 1 |\n      | 2 |\n      | 3 |\n\n  Scenario: plain\n    Given set variable \"y\" to \"1\"\n";
    let suite = suite("counts", "", &[("o.feature", outline)]);
    let out = suite.bddkit(&["run", "--telemetry", "true"], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let payload = suite.only_payload();
    assert_eq!(payload["scenarios"], 4);
    assert_eq!(payload["examples"], 3);
    assert_eq!(payload["usage"]["vars"]["steps"], 4);
    assert_eq!(payload["scenario_ms"]["buckets"][0], 4);
}

#[test]
fn a_macro_counts_the_steps_it_runs_and_not_itself() {
    let suite = suite(
        "macro",
        "macro_paths: [macros.yaml]\n",
        &[(
            "a.feature",
            "Feature: m\n  Scenario: s\n    Given I set two variables\n",
        )],
    );
    std::fs::write(
        suite.dir.join("macros.yaml"),
        "- step: I set two variables\n  do:\n    - set variable \"a\" to \"1\"\n    - set variable \"b\" to \"2\"\n",
    )
    .unwrap();
    let out = suite.bddkit(&["run", "--telemetry", "true"], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let payload = suite.only_payload();
    assert_eq!(payload["usage"]["vars"]["steps"], 2, "{payload}");
    assert!(payload["usage"].get("general").is_none(), "{payload}");
}

#[test]
fn no_two_payloads_share_a_value_that_could_link_them() {
    let suite = suite("linking", "", &[("a.feature", PLAIN)]);
    for _ in 0..2 {
        assert_eq!(
            suite
                .bddkit(&["run", "--telemetry", "true"], &[])
                .status
                .code(),
            Some(0)
        );
    }
    let [mut first, mut second]: [Value; 2] = suite.payloads().try_into().expect("two payloads");
    // What may differ between two runs of one suite is time.
    for payload in [&mut first, &mut second] {
        let map = payload.as_object_mut().unwrap();
        map.remove("duration_ms");
        map.remove("scenario_ms");
        map.remove("usage");
    }
    assert_eq!(first, second);
}

#[test]
fn the_flag_features_and_privacy_say_what_was_on_and_never_a_value() {
    let flags = suite("features", "", &[("a.feature", PLAIN)]);
    std::fs::create_dir_all(flags.dir.join("out")).unwrap();
    let out = flags.bddkit(
        &[
            "run",
            "--telemetry",
            "true",
            "--junit",
            "out/junit.xml",
            "--events",
            "out/events.ndjson",
            "--evidence=failures",
            "--reveal",
            "--fail-fast",
        ],
        &[],
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let payload = flags.only_payload();
    assert_eq!(payload["features"]["junit"], true);
    assert_eq!(payload["features"]["events"], true);
    assert_eq!(payload["features"]["cucumber_json"], false);
    assert_eq!(
        payload["features"]["evidence"],
        serde_json::json!(["failures"])
    );
    assert_eq!(payload["privacy"]["reveal"], true);
    assert_eq!(payload["privacy"]["custom_words"], false);
    assert_eq!(payload["fail_fast"], true);

    // The same two counted when a variable turned them on instead of a flag.
    let variables = suite("features-env", "", &[("a.feature", PLAIN)]);
    let out = variables.bddkit(
        &["run", "--telemetry", "true"],
        &[
            ("BDDKIT_PRIVACY_REVEAL", "1"),
            ("BDDKIT_EVENTS", "events.ndjson"),
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let payload = variables.only_payload();
    assert_eq!(payload["privacy"]["reveal"], true);
    assert_eq!(payload["features"]["events"], true);
}

#[cfg(unix)]
#[test]
fn an_interrupted_run_records_nothing() {
    let suite = suite(
        "interrupt",
        "",
        &[(
            "a.feature",
            "Feature: held\n  Scenario: s\n    Given I sleep \"10\" seconds\n",
        )],
    );
    let (url, rx) = collector();
    let events = suite.dir.join("events.ndjson");
    let mut child = suite
        .command(
            &["run", "--telemetry", "true", "--events", "events.ndjson"],
            &[("BDDKIT_TELEMETRY_URL", &url)],
        )
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !std::fs::read_to_string(&events).is_ok_and(|text| text.contains("step_started")) {
        assert!(
            std::time::Instant::now() < deadline,
            "the step never started"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let sent = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .expect("kill");
    assert!(sent.success());
    assert_eq!(child.wait().expect("wait").code(), Some(130));
    assert!(suite.payloads().is_empty());
    assert!(
        rx.recv_timeout(std::time::Duration::from_millis(300))
            .is_err()
    );
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn status_names_the_source_that_decided() {
    let suite = suite("status", "", &[("a.feature", PLAIN)]);
    let out = suite.bddkit(&["telemetry", "status"], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(
        stdout(&out).contains("telemetry: off (default, nobody has answered)"),
        "{}",
        stdout(&out)
    );
    let out = suite.bddkit(&["telemetry", "status"], &[("DO_NOT_TRACK", "1")]);
    assert!(
        stdout(&out).contains("telemetry: off (decided by DO_NOT_TRACK)"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn on_and_off_write_the_answer_and_status_reads_it_back() {
    let suite = suite("on-off", "", &[("a.feature", PLAIN)]);
    let out = suite.bddkit(&["telemetry", "on"], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(suite.state().join("telemetry.yaml").is_file());
    assert!(!stdout(&out).contains("note:"), "{}", stdout(&out));
    let status = stdout(&suite.bddkit(&["telemetry", "status"], &[]));
    assert!(
        status.contains("telemetry: on (decided by the override layer)"),
        "{status}"
    );

    suite.bddkit(&["telemetry", "off"], &[]);
    let status = stdout(&suite.bddkit(&["telemetry", "status"], &[]));
    assert!(
        status.contains("telemetry: off (decided by the override layer)"),
        "{status}"
    );
}

#[test]
fn an_on_that_a_higher_source_overrides_says_so() {
    let suite = suite(
        "on-overridden",
        "telemetry: false\n",
        &[("a.feature", PLAIN)],
    );
    let out = suite.bddkit(&["telemetry", "on"], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(
        stdout(&out).contains("note: it is off for now"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn a_layer_that_does_not_exist_is_refused() {
    let suite = suite("no-layer", "", &[("a.feature", PLAIN)]);
    let out = suite.bddkit(&["telemetry", "on", "--layer", "project"], &[]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(text(&out).contains("no project layer"), "{}", text(&out));
}

#[test]
fn show_prints_the_last_payload() {
    let suite = suite("show", "", &[("a.feature", PLAIN)]);
    let nothing = suite.bddkit(&["telemetry", "show"], &[]);
    assert_eq!(nothing.status.code(), Some(0));
    assert!(
        stdout(&nothing).starts_with("nothing recorded yet"),
        "{}",
        stdout(&nothing)
    );

    assert_eq!(
        suite
            .bddkit(&["run", "--telemetry", "true"], &[])
            .status
            .code(),
        Some(0)
    );
    let shown = suite.bddkit(&["telemetry", "show"], &[]);
    let payload: Value = serde_json::from_str(&stdout(&shown)).expect("a payload");
    assert_eq!(payload["command"], "run");
    // The file the command reads is the one in the layer directory.
    assert!(suite.state().join("telemetry-last.json").is_file());
}

#[cfg(debug_assertions)]
#[test]
fn a_debug_build_sends_nowhere_without_a_telemetry_url() {
    let suite = suite("endpoint", "", &[("a.feature", PLAIN)]);
    let status = stdout(&suite.bddkit(&["telemetry", "status"], &[]));
    assert!(
        status.contains("endpoint: none (this build sends nowhere)"),
        "{status}"
    );
    let status = stdout(&suite.bddkit(
        &["telemetry", "status"],
        &[("BDDKIT_TELEMETRY_URL", "http://127.0.0.1:1")],
    ));
    assert!(
        status.contains("endpoint: http://127.0.0.1:1/v1/run"),
        "{status}"
    );
}

fn doctor_rows(suite: &Suite, args: &[&str], env: &[(&str, &str)]) -> (Output, Vec<Value>) {
    let mut all = vec!["doctor", "--json"];
    all.extend_from_slice(args);
    let out = suite.bddkit(&all, env);
    let report: Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|error| {
        panic!("doctor --json: {error}\n{}", text(&out));
    });
    let rows = report["checks"].as_array().expect("checks").clone();
    (out, rows)
}

#[test]
fn doctor_reports_the_telemetry_decision() {
    let suite = suite("doctor-row", "", &[("a.feature", PLAIN)]);
    let (_, rows) = doctor_rows(&suite, &[], &[("DO_NOT_TRACK", "1")]);
    let row = rows
        .iter()
        .find(|r| r["stage"] == "telemetry")
        .expect("a telemetry row");
    assert_eq!(row["status"], "ok");
    assert_eq!(row["detail"], "off (decided by DO_NOT_TRACK)");

    let (_, rows) = doctor_rows(&suite, &["--telemetry", "true"], &[]);
    let row = rows
        .iter()
        .find(|r| r["stage"] == "telemetry")
        .expect("a telemetry row");
    assert_eq!(row["status"], "ok");
    let detail = row["detail"].as_str().unwrap();
    assert!(
        detail.starts_with("on (decided by --telemetry)"),
        "{detail}"
    );
}

#[test]
fn doctor_records_its_own_payload_with_the_stages_that_failed() {
    let unknown = "Feature: f\n  Scenario: s\n    Given nothing like this is a step\n";
    let suite = suite("doctor-payload", "", &[("a.feature", unknown)]);
    let (out, _) = doctor_rows(&suite, &["--telemetry", "true"], &[]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    let payload = suite.only_payload();
    assert_eq!(payload["command"], "doctor");
    assert_eq!(payload["exit"], 1);
    assert_eq!(payload["doctor"]["live"], false);
    assert_eq!(
        payload["doctor"]["stages_failed"],
        serde_json::json!(["steps"])
    );
    assert_eq!(payload["resources"]["api"], 1);
    for run_only in ["files", "scenarios", "usage", "scenario_ms"] {
        assert!(payload.get(run_only).is_none(), "{run_only}");
    }
}

#[test]
fn a_doctor_that_cannot_read_its_config_follows_the_broken_config_rule() {
    let suite = suite("doctor-broken", "", &[("a.feature", PLAIN)]);
    std::fs::write(suite.dir.join("bddkit.yaml"), "paths: [features\n").unwrap();
    std::fs::write(suite.state().join("telemetry.yaml"), "enabled: true\n").unwrap();
    let (out, rows) = doctor_rows(&suite, &[], &[]);
    assert_eq!(out.status.code(), Some(1));
    let row = rows
        .iter()
        .find(|r| r["stage"] == "telemetry")
        .expect("a telemetry row");
    assert_eq!(
        row["detail"],
        "off (decided by a config that could not be read)"
    );
    assert!(suite.payloads().is_empty());
}

#[test]
fn a_dsn_and_a_connection_name_never_reach_the_payload() {
    let config = |dsn: &str| {
        format!(
            "paths: [features]\nresources:\n  api:\n    marker_api:\n      base_url: http://marker.invalid:9\n  db:\n    marker_conn:\n      dsn: {dsn}\n"
        )
    };
    let suite = suite("marker-db", "", &[("a.feature", PLAIN)]);

    // A scheme no engine serves is refused when the pools are built, which is
    // after the config was read: a refusal with a stage, and no wait for a
    // connection to time out.
    std::fs::write(
        suite.dir.join("bddkit.yaml"),
        config("marker://marker_user:marker_pass@127.0.0.1:1/marker_db"),
    )
    .unwrap();
    let out = suite.bddkit(&["run", "--telemetry", "true"], &[]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    assert_eq!(suite.only_payload()["stage"], "resources");

    // `doctor` opens no socket without --live, so a real DSN is safe to use.
    std::fs::write(
        suite.dir.join("bddkit.yaml"),
        config("postgres://marker_user:marker_pass@127.0.0.1:1/marker_db"),
    )
    .unwrap();
    doctor_rows(&suite, &["--telemetry", "true"], &[]);
    let payloads = suite.payloads();
    assert_eq!(payloads.len(), 2, "{payloads:?}");
    assert_eq!(payloads[1]["resources"]["db"]["postgres"], 1);
    let recorded = std::fs::read_to_string(suite.log_path()).expect("payloads");
    assert!(!recorded.contains("marker"), "{recorded}");
}

/// A suite whose one feature echoes through the fixture plugin, with the lock
/// entry `entry` (the lines after `path:`) in the state directory.
fn plugin_suite(name: &str, entry: &str) -> Suite {
    let suite = suite(
        name,
        "",
        &[(
            "a.feature",
            "Feature: p\n  Scenario: s\n    When I echo \"x\" as \"greeting\"\n    Then variable \"greeting\" should be equal to \"p-x\"\n",
        )],
    );
    std::fs::write(
        suite.dir.join("bddkit.yaml"),
        "paths: [features]\nconcurrency: 1\nresources:\n  api: {}\n  echo:\n    main:\n      prefix: \"p-\"\n",
    )
    .unwrap();
    std::fs::write(
        suite.state().join("plugins.yaml"),
        format!(
            "plugin:\n  - name: echo\n    path: {}\n{entry}",
            common::build_fixture_plugin().display()
        ),
    )
    .unwrap();
    suite
}

#[test]
fn an_official_plugin_is_named_and_counted() {
    let suite = plugin_suite(
        "plugin-official",
        "    source: bddkit/echo-plugin\n    version: 0.1.0\n",
    );
    let out = suite.bddkit(&["run", "--telemetry", "true"], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let payload = suite.only_payload();
    assert_eq!(
        payload["resources"]["plugins"],
        serde_json::json!({"echo": 1})
    );
    assert_eq!(payload["resources"]["plugins_other"], 0);
    assert_eq!(payload["usage"]["echo"]["steps"], 1, "{payload}");
    assert_eq!(payload["usage"]["vars"]["steps"], 1, "{payload}");
}

#[test]
fn a_plugin_outside_the_bddkit_owner_is_counted_not_named() {
    // Another owner, and a hand-written entry with no source at all.
    for (name, entry) in [
        (
            "plugin-other-owner",
            "    source: someone/echo-plugin\n    version: 0.1.0\n",
        ),
        ("plugin-no-source", ""),
    ] {
        let suite = plugin_suite(name, entry);
        let out = suite.bddkit(&["run", "--telemetry", "true"], &[]);
        assert_eq!(out.status.code(), Some(0), "{name}: {}", text(&out));
        let payload = suite.only_payload();
        assert_eq!(
            payload["resources"]["plugins"],
            serde_json::json!({}),
            "{name}"
        );
        assert_eq!(payload["resources"]["plugins_other"], 1, "{name}");
        // Its step is summed into `other`, under no group name of its own.
        assert_eq!(payload["usage"]["other"]["steps"], 1, "{name}: {payload}");
        let raw = std::fs::read_to_string(suite.log_path()).unwrap();
        assert!(
            !raw.contains("echo") && !raw.contains("someone"),
            "{name}: {raw}"
        );
    }
}

#[test]
fn a_plugin_that_does_not_load_is_the_plugins_stage() {
    let suite = plugin_suite("plugin-missing", "");
    std::fs::write(
        suite.state().join("plugins.yaml"),
        "plugin:\n  - name: echo\n    path: /nonexistent/libecho.so\n",
    )
    .unwrap();
    let out = suite.bddkit(&["run", "--telemetry", "true"], &[]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    assert_eq!(suite.only_payload()["stage"], "plugins");
}

#[test]
fn an_include_counts_the_steps_it_runs_and_not_itself() {
    let suite = suite(
        "include",
        "",
        &[
            (
                "a.feature",
                "Feature: i\n  Scenario: s\n    Given I include \"target.feature\"\n    Then variable \"userId\" should be equal to \"abc123\"\n",
            ),
            (
                "target.feature",
                "Feature: t\n  @exports(userId)\n  Scenario: a user signs up\n    Given set variable \"userId\" to \"abc123\"\n",
            ),
        ],
    );
    std::fs::write(
        suite.dir.join("bddkit.yaml"),
        "paths: [features/a.feature]\nresources:\n  api: {}\n",
    )
    .unwrap();
    let out = suite.bddkit(&["run", "--telemetry", "true"], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let payload = suite.only_payload();
    // The step inside the included scenario and the one after it; the include is neither.
    assert_eq!(payload["usage"]["vars"]["steps"], 2, "{payload}");
    assert!(payload["usage"].get("general").is_none(), "{payload}");
}

#[test]
fn concurrency_is_the_effective_worker_count() {
    // The default is 8, and two files cannot use more than two workers.
    let suite = suite(
        "effective",
        "",
        &[("a.feature", PLAIN), ("b.feature", PLAIN)],
    );
    let out = suite.bddkit(&["run", "--telemetry", "true"], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert_eq!(suite.only_payload()["concurrency"], 2);
}

/// A collector that answers 204 to the first request it gets and hands back
/// the request line and the body. It lives as long as its thread.
fn collector() -> (String, std::sync::mpsc::Receiver<(String, Value)>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}", listener.local_addr().expect("addr"));
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let mut raw = Vec::new();
        let mut buf = [0u8; 4096];
        let body_at = loop {
            let n = stream.read(&mut buf).expect("read");
            raw.extend_from_slice(&buf[..n]);
            if let Some(at) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                break at + 4;
            }
        };
        let head = String::from_utf8_lossy(&raw[..body_at]).to_string();
        let length: usize = head
            .lines()
            .find_map(|l| {
                l.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .map(|v| v.trim().parse().expect("length"))
            })
            .expect("a content-length");
        while raw.len() < body_at + length {
            let n = stream.read(&mut buf).expect("read");
            raw.extend_from_slice(&buf[..n]);
        }
        let _ = stream.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n");
        let body = serde_json::from_slice(&raw[body_at..body_at + length]).expect("json body");
        let _ = tx.send((head.lines().next().unwrap_or("").to_string(), body));
    });
    (url, rx)
}

#[test]
fn a_run_sends_the_payload_it_recorded_to_the_collector() {
    let suite = suite("send", "", &[("a.feature", PLAIN)]);
    let (url, rx) = collector();
    let out = suite.bddkit(
        &["run", "--telemetry", "true"],
        &[("BDDKIT_TELEMETRY_URL", &url)],
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let (line, body) = rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("one request");
    assert_eq!(line, "POST /v1/run HTTP/1.1");
    assert_eq!(body, suite.only_payload());
}

#[test]
fn a_blackholed_endpoint_costs_at_most_the_timeout() {
    let suite = suite("blackhole", "", &[("a.feature", PLAIN)]);
    // Accepts the connection (the kernel does) and never reads or answers.
    let hole = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}", hole.local_addr().expect("addr"));
    let started = std::time::Instant::now();
    let out = suite.bddkit(
        &["run", "--telemetry", "true"],
        &[("BDDKIT_TELEMETRY_URL", &url)],
    );
    let took = started.elapsed();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(took < std::time::Duration::from_millis(3500), "{took:?}");
    assert!(out.stderr.is_empty(), "{}", text(&out));
}

#[cfg(unix)]
#[test]
fn a_signal_while_the_request_is_out_does_not_wait_for_it() {
    let suite = suite("signal-send", "", &[("a.feature", PLAIN)]);
    let hole = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}", hole.local_addr().expect("addr"));
    let mut child = suite
        .command(
            &["run", "--telemetry", "true"],
            &[("BDDKIT_TELEMETRY_URL", &url)],
        )
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");
    // The request is out once the connection arrives; nobody answers it.
    hole.set_nonblocking(true).expect("nonblocking");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let _held = loop {
        match hole.accept() {
            Ok((held, _)) => break held,
            Err(_) => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the request never connected"
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    };
    let signalled = std::time::Instant::now();
    let _ = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status();
    // The run had passed, so its own code is returned, and at once.
    assert_eq!(child.wait().expect("wait").code(), Some(0));
    assert!(signalled.elapsed() < std::time::Duration::from_millis(1000));
}
