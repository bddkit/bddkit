use super::{ECHO_GROUP, assert_green, evidence_stream, project, run, run_with};
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
