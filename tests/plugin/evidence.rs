use super::{ECHO_GROUP, assert_green, evidence_stream, project, run, run_with};
use serde_json::{Value, json};
use std::path::Path;

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

// ---- #303: which dispatch of a polled step wrote the evidence ----

const ARMED: &str = "    When I expect the next assertion to pass within \"1\" seconds, checking every \"400\" milliseconds\n";

/// A call of the echo assertion that answers `replies`, one per dispatch.
fn by_attempt(replies: &Value) -> String {
    format!("    Then the echo answers by attempt:\n      \"\"\"\n      {replies}\n      \"\"\"\n")
}

fn reply(status: &str) -> Value {
    let diagnostic = json!({"title": "echo state", "kind": "text", "content": status});
    json!({"status": status, "diagnostics": [diagnostic]})
}

/// The stream of one scenario of `steps`, whatever the run's outcome.
fn stream_of(name: &str, steps: &str, evidence: &str) -> Vec<Value> {
    let feature = format!("Feature: f\n  Scenario: s\n{steps}");
    let dir = project(name, &feature, ECHO_GROUP);
    let out = run_with(&dir, &["--events", "events.ndjson", evidence], &[]);
    assert!(
        matches!(out.status.code(), Some(0 | 1)),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::read_to_string(dir.join("events.ndjson"))
        .expect("the stream exists")
        .lines()
        .map(|line| serde_json::from_str(line).expect("one JSON object per line"))
        .collect()
}

/// `key` of every event of `kind`, in stream order.
fn numbers(stream: &[Value], kind: &str, key: &str) -> Vec<Option<u64>> {
    stream
        .iter()
        .filter(|e| e["type"] == kind)
        .map(|e| e[key].as_u64())
        .collect()
}

#[test]
fn polled_http_evidence_carries_the_attempt_number() {
    let exchange = |status: &str| {
        json!({"status": status, "diagnostics": [{"kind": "http", "content": {
            "method": "GET", "url": "http://svc.test/job",
            "request": {"headers": [], "body": ""}, "status": 200,
            "response": {"headers": [], "body": status},
            "sensitive_headers": [], "request_form": "raw",
        }}]})
    };
    let replies = json!([exchange("not_yet"), exchange("not_yet"), exchange("passed")]);
    let steps = format!("{ARMED}{}", by_attempt(&replies));
    let stream = stream_of("attempt-http", &steps, "--evidence");

    assert_eq!(
        numbers(&stream, "evidence", "attempt"),
        [Some(1), Some(2), Some(3)]
    );
    assert_eq!(numbers(&stream, "attempt", "n"), [Some(1), Some(2)]);
}

#[test]
fn the_last_dispatch_is_numbered_when_it_times_out() {
    let steps = format!("{ARMED}{}", by_attempt(&json!([reply("not_yet")])));
    let stream = stream_of("attempt-timeout", &steps, "--evidence");

    let evidence = numbers(&stream, "evidence", "attempt");
    assert!(evidence.len() > 1, "{evidence:?}");
    let expected: Vec<_> = (1..=evidence.len() as u64).map(Some).collect();
    assert_eq!(evidence, expected);
    // The dispatch that ran out of time answered `not_yet` like the others.
    assert_eq!(numbers(&stream, "attempt", "n"), expected);
}

#[test]
fn the_last_dispatch_is_numbered_when_it_fails_fatally() {
    let replies = json!([reply("not_yet"), reply("fatal")]);
    let steps = format!("{ARMED}{}", by_attempt(&replies));
    let stream = stream_of("attempt-fatal", &steps, "--evidence");

    assert_eq!(numbers(&stream, "evidence", "attempt"), [Some(1), Some(2)]);
    assert_eq!(numbers(&stream, "attempt", "n"), [Some(1)]);
}

#[test]
fn plugin_polling_diagnostics_carry_the_attempt_number() {
    let replies = json!([reply("not_yet"), reply("passed")]);
    let steps = format!("{ARMED}{}", by_attempt(&replies));
    let stream = stream_of("attempt-plain", &steps, "--evidence");

    assert_eq!(numbers(&stream, "evidence", "attempt"), [Some(1), Some(2)]);
    // A polled step that passes at once is still polled.
    let steps = format!("{ARMED}{}", by_attempt(&json!([reply("passed")])));
    let stream = stream_of("attempt-first", &steps, "--evidence");
    assert_eq!(numbers(&stream, "evidence", "attempt"), [Some(1)]);
}

#[test]
fn evidence_of_a_step_that_is_not_polled_has_no_attempt() {
    // Before a polled step, after it, and in the next scenario.
    let single = by_attempt(&json!([reply("passed")]));
    let steps = format!(
        "{single}{ARMED}{}{single}  Scenario: t\n{single}",
        by_attempt(&json!([reply("passed")])),
    );
    let stream = stream_of("attempt-none", &steps, "--evidence");

    let attempts: Vec<_> = stream
        .iter()
        .filter(|e| e["type"] == "evidence")
        .map(|e| e.get("attempt").cloned())
        .collect();
    assert_eq!(attempts, [None, Some(json!(1)), None, None]);
}

#[test]
fn deferred_evidence_keeps_its_attempt_number() {
    let replies = json!([reply("not_yet"), reply("fatal")]);
    let steps = format!("{ARMED}{}", by_attempt(&replies));
    let stream = stream_of("attempt-deferred", &steps, "--evidence=failures");

    assert_eq!(numbers(&stream, "evidence", "attempt"), [Some(1), Some(2)]);
    assert!(
        stream
            .iter()
            .filter(|e| e["type"] == "evidence")
            .all(|e| e["deferred"] == true)
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
