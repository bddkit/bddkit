use super::{polled, project, run, stdout_of};

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
