use super::{ECHO_GROUP, answers, macro_project, project, read_stream, run, run_with, stdout_of};

// ---- bddkit_failure_context ------------------------------------------------

const TWO_ECHOES: &str = "  echo:\n    main:\n      prefix: \"p-\"\n    other:\n      prefix: \"o-\"\ndefault_echo: main\n";

/// A failing core step: no plugin replied, so the whole dump is contexts.
const CORE_FAILS: &str = "    Then variable \"seen\" should be equal to \"never\"\n";

fn position(text: &str, needle: &str) -> usize {
    text.find(needle)
        .unwrap_or_else(|| panic!("{needle:?} is not in {text}"))
}

#[test]
fn a_failing_core_step_shows_each_used_plugin_in_first_use_order() {
    let dir = project(
        "context-order",
        &format!(
            "Feature: f\n  Scenario: s\n    Given I use \"other\" echo\n    When I echo \"x\" as \"seen\"\n    And I use \"main\" echo\n    And I echo \"y\" as \"seen\"\n{CORE_FAILS}"
        ),
        TWO_ECHOES,
    );
    let text = stdout_of(&run(&dir));
    // First used `other`, then `main`: not alphabetical, not the config's order.
    assert!(
        position(&text, "context of o-") < position(&text, "context of p-"),
        "{text}"
    );
    assert_eq!(text.matches("context of p-").count(), 1, "{text}");
}

#[test]
fn a_context_comes_only_from_a_plugin_this_scenario_used() {
    let dir = project(
        "context-scenario",
        &format!(
            "Feature: f\n  Scenario: main\n    When I echo \"x\" as \"seen\"\n  Scenario: other\n    Given I use \"other\" echo\n    When I echo \"x\" as \"seen\"\n  Scenario: main again\n    When I echo \"x\" as \"seen\"\n{CORE_FAILS}"
        ),
        TWO_ECHOES,
    );
    let text = stdout_of(&run(&dir));
    // The third scenario reuses a handle the first one created, which the
    // file-wide cache already holds: it must still be listed.
    assert!(text.contains("context of p-"), "{text}");
    assert!(!text.contains("context of o-"), "{text}");
}

#[test]
fn a_plugin_step_whose_reply_showed_a_diagnostic_gets_no_context_of_its_own() {
    let dir = project(
        "context-own-covered",
        "Feature: f\n  Scenario: s\n    Given I use \"other\" echo\n    When I echo \"x\" as \"seen\"\n    And I use \"main\" echo\n    Then the echo should fail\n",
        TWO_ECHOES,
    );
    let text = stdout_of(&run(&dir));
    assert!(text.contains("echo state"), "{text}");
    assert!(text.contains("context of o-"), "{text}");
    assert!(!text.contains("context of p-"), "{text}");
}

#[test]
fn a_plugin_step_whose_reply_showed_nothing_gets_its_own_context() {
    let reply = serde_json::json!({"status": "fatal", "error": "refused"});
    let dir = project("context-own-bare", &answers(&reply), ECHO_GROUP);
    let text = stdout_of(&run(&dir));
    assert!(text.contains("refused"), "{text}");
    assert!(text.contains("context of p-"), "{text}");
}

#[test]
fn a_failing_macro_shows_each_context_once() {
    let dir = macro_project(
        "context-macro-bare",
        "Feature: f\n  Scenario: s\n    When I check the echo twice\n",
        "- step: 'I check the echo twice'\n  do:\n    - I echo \"x\" as \"seen\"\n    - variable \"seen\" should be equal to \"never\"\n",
    );
    let text = stdout_of(&run(&dir));
    assert_eq!(text.matches("context of implicit-").count(), 1, "{text}");
}

#[test]
fn a_context_is_masked_unless_revealed() {
    let dir = project(
        "context-mask",
        &format!("Feature: f\n  Scenario: s\n    When I echo \"x\" as \"seen\"\n{CORE_FAILS}"),
        ECHO_GROUP,
    );
    let masked = stdout_of(&run(&dir));
    assert!(masked.contains("context of p-"), "{masked}");
    assert!(!masked.contains("context-pass-123"), "{masked}");
    let revealed = stdout_of(&run_with(&dir, &["--reveal"], &[]));
    assert!(revealed.contains("context-pass-123"), "{revealed}");
}

#[test]
fn a_context_is_in_no_evidence_event() {
    let dir = project(
        "context-evidence",
        &format!("Feature: f\n  Scenario: s\n    When I echo \"x\" as \"seen\"\n{CORE_FAILS}"),
        ECHO_GROUP,
    );
    let out = run_with(&dir, &["--events", "events.ndjson", "--evidence"], &[]);
    assert!(stdout_of(&out).contains("context of p-"));
    // The failure text, dump included, travels in `scenario_finished`; what
    // must not exist is an `evidence` event made from the context.
    for event in read_stream(&dir) {
        if event["type"] == "evidence" {
            assert!(!event.to_string().contains("context of"), "{event}");
        }
    }
}

#[test]
fn a_bad_context_reply_adds_one_line_and_leaves_the_failure_alone() {
    for mode in ["panic", "null", "malformed", "error"] {
        let dir = project(
            &format!("context-bad-{mode}"),
            &format!("Feature: f\n  Scenario: s\n    When I echo \"x\" as \"seen\"\n{CORE_FAILS}"),
            ECHO_GROUP,
        );
        let out = run_with(
            &dir,
            &["--events", "events.ndjson"],
            &[("BDDKIT_ECHO_FIXTURE_CONTEXT", mode)],
        );
        let text = stdout_of(&out);
        assert_eq!(out.status.code(), Some(1), "{mode}: {text}");
        assert_eq!(
            text.matches("failure context of plugin \"echo\" unavailable:")
                .count(),
            1,
            "{mode}: {text}"
        );
        assert!(!text.contains("context of p-"), "{mode}: {text}");
        assert!(!text.contains("context-malformed-secret"), "{mode}: {text}");
        // The plugin's own error is shown, masked by what the same reply declared.
        assert!(!text.contains("context-error-secret"), "{mode}: {text}");
        if mode == "error" {
            assert!(text.contains("the echo context refused ***"), "{text}");
        }
        let stream = read_stream(&dir);
        let finished = stream
            .iter()
            .find(|e| e["type"] == "scenario_finished")
            .unwrap();
        assert_eq!(finished["failure_kind"], "assertion", "{mode}");
    }
}

#[test]
fn an_empty_context_adds_nothing() {
    let dir = project(
        "context-empty",
        &format!("Feature: f\n  Scenario: s\n    When I echo \"x\" as \"seen\"\n{CORE_FAILS}"),
        ECHO_GROUP,
    );
    let text = stdout_of(&run_with(
        &dir,
        &[],
        &[("BDDKIT_ECHO_FIXTURE_CONTEXT", "empty")],
    ));
    assert!(!text.contains("failure context"), "{text}");
    assert!(!text.contains("context of"), "{text}");
}
