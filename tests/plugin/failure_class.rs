use super::{ECHO_GROUP, answers, assert_green, polled, project, run, stdout_of};

#[test]
fn a_reply_names_its_failure_class_and_an_unknown_one_counts_as_absent_with_one_warning() {
    for class in [
        "assertion",
        "transport",
        "db_connection",
        "db_statement",
        "input",
        "unclassified",
    ] {
        let reply = serde_json::json!({"status": "fatal", "error": "boom", "class": class});
        let dir = project(&format!("class-{class}"), &answers(&reply), ECHO_GROUP);
        let text = stdout_of(&run(&dir));
        assert!(
            text.contains(&format!("failed: 1 (1 {class})")),
            "{class}: {text}"
        );
    }
    // Absent, and not one the host lets a plugin name: today's class.
    for class in [
        serde_json::json!("bogus"),
        serde_json::json!("reset"),
        serde_json::json!(7),
    ] {
        let reply = serde_json::json!({"status": "fatal", "error": "boom", "class": class});
        let dir = project("class-unknown", &answers(&reply), ECHO_GROUP);
        let text = stdout_of(&run(&dir));
        assert!(text.contains("failed: 1 (1 plugin)"), "{class}: {text}");
    }
}

#[test]
fn an_unknown_class_in_two_steps_warns_once() {
    let failing = serde_json::json!({"status": "fatal", "error": "boom", "class": "bogus"});
    let step = |r: &serde_json::Value| {
        format!("    Then the echo answers:\n      \"\"\"\n      {r}\n      \"\"\"\n")
    };
    // Only a failing reply is classified, so two failing scenarios send it.
    let feature = format!(
        "Feature: f\n  Scenario: a\n{}  Scenario: b\n{}",
        step(&failing),
        step(&failing)
    );
    let dir = project("class-warn-once", &feature, ECHO_GROUP);
    let text = stdout_of(&run(&dir));
    assert_eq!(
        text.matches("which is not one it may name").count(),
        1,
        "{text}"
    );
    assert!(text.contains("\"bogus\""), "{text}");
}

#[test]
fn a_poll_ends_with_the_class_of_its_last_attempt() {
    let script = serde_json::json!([
        {"status": "not_yet", "class": "transport"},
        {"status": "not_yet", "class": "assertion"},
        {"status": "not_yet", "class": "transport"},
    ]);
    let dir = project("class-poll", &polled(&script, "1"), ECHO_GROUP);
    let text = stdout_of(&run(&dir));
    assert!(text.contains("failed: 1 (1 transport)"), "{text}");
}

#[test]
fn a_step_sees_its_attempt_number() {
    let script = serde_json::json!([
        {"status": "not_yet"},
        {"status": "not_yet"},
        {"status": "passed"},
    ]);
    let feature = format!(
        "{}    Then variable \"echo_attempt\" should be equal to \"3\"\n",
        polled(&script, "2")
    );
    let dir = project("attempt-poll", &feature, ECHO_GROUP);
    assert_green(&run(&dir));
    // Not polled: 1.
    let script = serde_json::json!([{"status": "passed"}]);
    let feature = format!(
        "Feature: f\n  Scenario: s\n    Then the echo answers by attempt:\n      \"\"\"\n      {script}\n      \"\"\"\n    And variable \"echo_attempt\" should be equal to \"1\"\n"
    );
    let dir = project("attempt-once", &feature, ECHO_GROUP);
    assert_green(&run(&dir));
}
