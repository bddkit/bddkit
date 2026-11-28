//! What the stream says of a step's nature: `role` and `negative` on
//! `step_started`, `attempts` and `poll` on `step_finished`.

use super::{of_type, run_with_events, write_events_project, write_evidence_project};
use serde_json::{Value, json};

/// One run of `features`, without `--evidence`; the stream it wrote.
fn stream(name: &str, features: &[(&str, &str)], macros: Option<&str>) -> Vec<Value> {
    let cfg = write_evidence_project(name, features, macros);
    let config = std::fs::read_to_string(&cfg).expect("read config").replace(
        "  api: {}\n",
        "  api: {}\n  echo:\n    main:\n      prefix: \"p-\"\n",
    );
    std::fs::write(&cfg, config).expect("write config");
    run_with_events(&cfg, &[]).1
}

/// The stream of one scenario made of `steps`.
fn scenario(name: &str, steps: &str) -> Vec<Value> {
    let feature = format!("Feature: f\n  Scenario: s\n{steps}");
    stream(name, &[("f.feature", &feature)], None)
}

/// `step_started` and `step_finished` of the first step with this text.
fn step<'a>(stream: &'a [Value], text: &str) -> (&'a Value, &'a Value) {
    let started = of_type(stream, "step_started")
        .into_iter()
        .find(|e| e["text"] == text)
        .unwrap_or_else(|| panic!("no step {text:?} in {stream:?}"));
    let finished = of_type(stream, "step_finished")
        .into_iter()
        .find(|e| {
            ["file", "scenario", "step"]
                .iter()
                .all(|key| e[key] == started[key])
        })
        .unwrap_or_else(|| panic!("step {text:?} never finished: {stream:?}"));
    (started, finished)
}

fn role<'a>(stream: &'a [Value], text: &str) -> Option<&'a str> {
    step(stream, text).0["role"].as_str()
}

/// `(attempts, poll)` as `step_finished` carries them; `Null` when absent.
fn polled(stream: &[Value], text: &str) -> (Value, Value) {
    let finished = step(stream, text).1;
    (finished["attempts"].clone(), finished["poll"].clone())
}

const WITHIN: &str =
    "I expect the next assertion to pass within \"5\" seconds, checking every \"10\" milliseconds";

#[test]
fn step_started_reports_the_role_of_a_builtin_action_and_assertion() {
    let stream = scenario(
        "role-builtin",
        "    Given set variable \"a\" to \"1\"\n    Then variable \"a\" should be equal to \"1\"\n",
    );

    assert_eq!(role(&stream, "set variable \"a\" to \"1\""), Some("action"));
    assert_eq!(
        role(&stream, "variable \"a\" should be equal to \"1\""),
        Some("assertion")
    );
}

#[test]
fn step_started_reports_the_role_of_a_plugin_step() {
    let stream = scenario(
        "role-plugin",
        "    When I echo evidence\n    Then the echo counter should reach 1\n",
    );

    assert_eq!(role(&stream, "I echo evidence"), Some("action"));
    assert_eq!(
        role(&stream, "the echo counter should reach 1"),
        Some("assertion")
    );
}

#[test]
fn a_macro_call_and_an_include_have_the_role_call() {
    let helper = "Feature: helper\n  Scenario: h\n    Then the echo should not be heard\n";
    let main = "Feature: f\n  Scenario: s\n    When I check quietly\n    And I include \"helper.feature\"\n    And I include \"helper.feature\" scenario \"h\"\n";
    let stream = stream(
        "role-call",
        &[("f.feature", main), ("helper.feature", helper)],
        Some("- step: I check quietly\n  do: [the echo should not be heard]\n"),
    );

    // A call has no nature of its own, though the step it runs is negative.
    for text in [
        "I check quietly",
        "I include \"helper.feature\"",
        "I include \"helper.feature\" scenario \"h\"",
    ] {
        assert_eq!(role(&stream, text), Some("call"), "{text}");
        assert!(step(&stream, text).0.get("negative").is_none(), "{text}");
    }
    let nested: Vec<_> = of_type(&stream, "step_started")
        .into_iter()
        .filter(|e| e.get("parent").is_some())
        .collect();
    assert_eq!(nested.len(), 3, "{stream:?}");
    for inner in nested {
        assert_eq!(inner["role"], "assertion", "{inner}");
        assert_eq!(inner["negative"], true, "{inner}");
    }
}

#[test]
fn an_unresolved_step_has_no_role() {
    let stream = scenario(
        "role-unresolved",
        "    Then variable \"a\" should not be equal to \"<<missing>>\"\n",
    );

    let (started, finished) = step(
        &stream,
        "variable \"a\" should not be equal to \"<<missing>>\"",
    );
    assert_eq!(finished["status"], "failed");
    assert!(started.get("role").is_none(), "{started}");
    assert!(started.get("negative").is_none(), "{started}");
}

#[test]
fn step_started_marks_a_negative_assertion() {
    let stream = scenario(
        "role-negative",
        "    Given set variable \"a\" to \"1\"\n    Then variable \"a\" should not be equal to \"2\"\n    And the echo should not be heard\n    And variable \"a\" should be equal to \"1\"\n    And the echo counter should reach 1\n",
    );

    for text in [
        "variable \"a\" should not be equal to \"2\"",
        "the echo should not be heard",
    ] {
        assert_eq!(step(&stream, text).0["negative"], json!(true), "{text}");
    }
    for text in [
        "set variable \"a\" to \"1\"",
        "variable \"a\" should be equal to \"1\"",
        "the echo counter should reach 1",
    ] {
        let started = step(&stream, text).0;
        assert!(started.get("negative").is_none(), "{started}");
    }
}

#[test]
fn step_finished_reports_attempts_and_poll_without_evidence() {
    let stream = scenario(
        "attempts",
        &format!(
            "    Given set variable \"a\" to \"1\"\n    When {WITHIN}\n    Then the echo counter should reach 3\n    When I expect the next assertion to pass eventually\n    Then variable \"a\" should be equal to \"1\"\n    When I expect the next assertion to pass within \"1\" seconds, checking every \"200\" milliseconds\n    Then the echo counter should reach 1000\n"
        ),
    );

    assert!(of_type(&stream, "attempt").is_empty(), "evidence is off");
    assert_eq!(
        polled(&stream, "the echo counter should reach 3"),
        (json!(3), json!("within"))
    );
    // A built-in that passes at once was polled all the same.
    assert_eq!(
        polled(&stream, "variable \"a\" should be equal to \"1\""),
        (json!(1), json!("within"))
    );
    // One that ran out of time reports the count its failure names.
    let (attempts, poll) = polled(&stream, "the echo counter should reach 1000");
    assert_eq!(poll, "within");
    let failure = of_type(&stream, "scenario_finished")[0]["failure"]
        .as_str()
        .expect("the scenario failed");
    assert!(
        failure.contains(&format!(" {attempts} attempts")),
        "{attempts} in {failure}"
    );
}

#[test]
fn a_step_that_was_not_polled_has_no_attempts() {
    let stream = stream(
        "attempts-none",
        &[(
            "f.feature",
            &format!(
                "Feature: f\n  Scenario: s\n    When {WITHIN}\n    And set variable \"a\" to \"1\"\n    And I check the counter\n    And set variable \"b\" to \"2\"\n    Then variable \"a\" should be equal to \"1\"\n    And the echo counter should reach 3\n"
            ),
        )],
        Some("- step: I check the counter\n  do: [the echo counter should reach 2]\n"),
    );

    // The modifier is consumed inside the macro: the caller was not polled.
    assert_eq!(
        polled(&stream, "the echo counter should reach 2"),
        (json!(2), json!("within"))
    );
    for text in [
        WITHIN,
        "set variable \"a\" to \"1\"",
        "I check the counter",
        "set variable \"b\" to \"2\"",
        "variable \"a\" should be equal to \"1\"",
        "the echo counter should reach 3",
    ] {
        let finished = step(&stream, text).1;
        assert_eq!(finished["status"], "passed", "{text}: {finished}");
        assert!(finished.get("attempts").is_none(), "{text}: {finished}");
        assert!(finished.get("poll").is_none(), "{text}: {finished}");
    }
}

#[test]
fn a_polled_step_that_fails_before_its_first_attempt_has_no_attempts() {
    // One second cannot hold the configured interval of three: the assertion
    // consumes the modifier and fails before it is tried.
    let cfg = write_events_project(
        "attempts-before-first",
        &[(
            "f.feature",
            "Feature: f\n  Scenario: s\n    Given set variable \"a\" to \"1\"\n    When I expect the next assertion to pass within \"1\" seconds\n    Then variable \"a\" should be equal to \"1\"\n",
        )],
        None,
    );
    let mut config = std::fs::read_to_string(&cfg).expect("read config");
    config.push_str("options:\n  polling:\n    timeout_secs: 5\n    interval_ms: 3000\n");
    std::fs::write(&cfg, config).expect("write config");
    let stream = run_with_events(&cfg, &[]).1;

    let finished = step(&stream, "variable \"a\" should be equal to \"1\"").1;
    assert_eq!(finished["status"], "failed", "{stream:?}");
    assert!(finished.get("attempts").is_none(), "{finished}");
    assert!(finished.get("poll").is_none(), "{finished}");
}
