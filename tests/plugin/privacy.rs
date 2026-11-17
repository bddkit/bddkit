use super::{ECHO_GROUP, evidence_stream, project, run, run_with};
use std::path::Path;
use std::process::Command;

// ---- privacy: what a plugin sends back passes through the host's filter ----

#[test]
fn a_sensitive_field_in_a_json_diagnostic_is_masked() {
    let dir = project(
        "mask-json-diagnostic",
        "Feature: f\n  Scenario: s\n    Given the echo should fail with json\n\n  Scenario: later\n    Given set variable \"note\" to \"diag-refresh-token-1\"\n    Then variable \"note\" should be equal to \"nothing\"\n",
        "",
    );
    let out = run(&dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    // What the diagnostic held under a sensitive name joined the file's set: a
    // later scenario that prints the same value, under a plain name, hides it.
    assert!(!stdout.contains("diag-refresh-token-1"), "{stdout}");
    // `Zd1` is shorter than min_length: only its position hides it.
    assert!(stdout.contains(r#""token": "***""#), "{stdout}");
    assert!(!stdout.contains("Zd1"), "{stdout}");
    assert!(stdout.contains("ann"), "{stdout}");
}

#[test]
fn a_plugin_config_secret_is_masked_in_a_validate_config_refusal() {
    let dir = project(
        "mask-validate-config",
        "Feature: f\n  Scenario: s\n    Given the echo should fail\n",
        "  echo:\n    main:\n      prefix: p-\n      password: plugin-secret-1234\n",
    );
    let out = run(&dir);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("refuses the password"), "{stderr}");
    assert!(stderr.contains(r#""***""#), "{stderr}");
    assert!(!stderr.contains("plugin-secret-1234"), "{stderr}");
}

#[test]
fn a_secret_reaches_a_plugin_unmasked_and_its_reply_is_masked() {
    let dir = project(
        "mask-reaches-plugin",
        "Feature: f\n  Scenario: s\n    Given set variable \"secret_note\" to \"plugin-arg-secret-1\"\n    And I echo \"<<secret_note>>\" as \"copy\"\n    And variable \"copy\" should be equal to \"implicit-plugin-arg-secret-1\"\n    Then the echo should fail quoting \"<<secret_note>>\"\n",
        "",
    );
    let out = run(&dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    // The plugin got the real value (the equality step passed and the failing
    // one is the quoting step), and what it replied is hidden.
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("the echo quoted ***"), "{stdout}");
    assert!(!stdout.contains("plugin-arg-secret-1"), "{stdout}");
}

#[test]
fn a_declared_media_type_replaces_the_guess_and_declared_secrets_are_masked() {
    let dir = project(
        "declared-secret",
        "Feature: f\n  Scenario: s\n    When I echo a declared secret and a typed file\n",
        ECHO_GROUP,
    );
    let stream = evidence_stream(&dir, &["--artifacts-dir", "evidence"]);

    let shot = stream
        .iter()
        .find(|e| e["title"] == "shot")
        .expect("the shot");
    assert_eq!(shot["media_type"], "image/png", "`.bin` would be a guess");
    let quote = stream
        .iter()
        .find(|e| e["title"] == "quote")
        .expect("the quote");
    assert_eq!(quote["content"], "token ***");
    let raw = std::fs::read_to_string(dir.join("events.ndjson")).expect("stream");
    assert!(!raw.contains("declared-secret-value"), "{raw}");
}

#[test]
fn a_secret_a_failing_reply_declares_is_masked_in_its_own_error() {
    let dir = project(
        "declared-secret-on-fatal",
        "Feature: f\n  Scenario: s\n    Then the echo should fail declaring \"fatal-secret-1\" secret\n",
        ECHO_GROUP,
    );
    let out = run(&dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("the echo leaked ***"), "{stdout}");
    assert!(!stdout.contains("fatal-secret-1"), "{stdout}");
}

#[test]
fn a_step_spec_secret_group_is_hidden_in_the_step_and_in_what_the_plugin_quotes() {
    let dir = project(
        "secret-group",
        "Feature: f\n  Scenario: s\n    Given set variable \"pw\" to \"group-secret-1\"\n    Then I echo \"ann\" with password \"<<pw>>\"\n",
        ECHO_GROUP,
    );
    let out = run_with(&dir, &["--events", "events.ndjson", "--evidence"], &[]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("the echo was given ***"), "{stdout}");
    assert!(!stdout.contains("group-secret-1"), "{stdout}");
    // `pw` is not a sensitive name: only the step spec's flag hides the slot.
    let raw = std::fs::read_to_string(dir.join("events.ndjson")).expect("stream");
    assert!(raw.contains(r#"[["pw","***"]]"#), "{raw}");

    // A literal in the group is hidden in the step text itself.
    let literal = project(
        "secret-group-literal",
        "Feature: f\n  Scenario: s\n    Then I echo \"ann\" with password \"pw1\"\n",
        ECHO_GROUP,
    );
    let out = run_with(&literal, &["--events", "events.ndjson"], &[]);
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(stdout.contains("with password \"***\""), "{stdout}");
    assert!(!stdout.contains("\"pw1\""), "{stdout}");
    let raw = std::fs::read_to_string(literal.join("events.ndjson")).expect("stream");
    assert!(raw.contains(r#"with password \"***\""#), "{raw}");
    assert!(!raw.contains(r#"\"pw1\""#), "{raw}");
}

#[test]
fn a_not_yet_reply_declares_secrets_and_sees_the_evidence_flag_on_every_attempt() {
    let dir = project(
        "declared-secret-not-yet",
        "Feature: f\n  Scenario: s\n    When I expect the next assertion to pass within \"1\" seconds, checking every \"20\" milliseconds\n    Then the echo should not be ready yet declaring \"notyet-secret-1\" secret\n",
        ECHO_GROUP,
    );
    let out = run_with(&dir, &["--events", "events.ndjson", "--evidence"], &[]);
    assert_eq!(out.status.code(), Some(1));
    let stream: Vec<serde_json::Value> = std::fs::read_to_string(dir.join("events.ndjson"))
        .expect("stream")
        .lines()
        .map(|line| serde_json::from_str(line).expect("json"))
        .collect();
    let attempts: Vec<_> = stream.iter().filter(|e| e["type"] == "attempt").collect();
    assert!(attempts.len() > 1, "polling replayed the step");
    for attempt in attempts {
        // Masked at emit time, so only an early registration keeps it clean.
        assert_eq!(
            attempt["message"], "not ready *** evidence=true",
            "{attempt}"
        );
    }
}

#[test]
fn a_declared_secret_shorter_than_min_length_is_not_registered() {
    let dir = project(
        "declared-secret-short",
        "Feature: f\n  Scenario: s\n    Then the echo should fail declaring \"short\" secret\n",
        ECHO_GROUP,
    );
    let stdout = String::from_utf8_lossy(&run(&dir).stdout).into_owned();
    assert!(stdout.contains("the echo leaked short"), "{stdout}");
}

#[test]
fn a_secret_group_no_pattern_group_has_stops_the_run_at_load() {
    let dir = project(
        "secret-group-bad-index",
        "Feature: f\n  Scenario: s\n    Then the echo should fail\n",
        ECHO_GROUP,
    );
    let out = run_with(&dir, &[], &[("BDDKIT_ECHO_FIXTURE_BAD_SECRET_GROUP", "1")]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("groups count from 1"), "{stderr}");
}

// ---- ABI 4: `secrets` and `detail` in a plugin's config replies ----

/// stdout and stderr together, and the exit code, of `bddkit <args>` in `dir`.
fn bddkit_in(dir: &Path, args: &[&str]) -> (String, Option<i32>) {
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run bddkit");
    (
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
        out.status.code(),
    )
}

/// `note` matches no privacy word, so only the reply's `secrets` can hide it.
const REFUSING_NOTE: &str =
    "  echo:\n    main:\n      prefix: p-\n      note: plugin-note-9876\n      note_refuse: true\n";

#[test]
fn a_secret_in_a_refusing_reply_is_masked_in_its_own_refusal_text() {
    let dir = project(
        "cfg-secret-refusal",
        "Feature: f\n  Scenario: s\n    Given I am in debug mode\n",
        REFUSING_NOTE,
    );
    for args in [
        &["run", "--config", "cfg.yaml"][..],
        &["doctor", "--config", "cfg.yaml"][..],
        &["doctor", "--config", "cfg.yaml", "--json"][..],
    ] {
        let (text, code) = bddkit_in(&dir, args);
        assert_ne!(code, Some(0), "{args:?}: {text}");
        assert!(text.contains("refuses the note"), "{args:?}: {text}");
        assert!(!text.contains("plugin-note-9876"), "{args:?}: {text}");
    }
    let (text, _) = bddkit_in(&dir, &["run", "--config", "cfg.yaml", "--reveal"]);
    assert!(
        text.contains("plugin-note-9876"),
        "--reveal shows it: {text}"
    );
}

#[test]
fn a_secret_under_min_length_stays_visible() {
    let dir = project(
        "cfg-secret-short",
        "Feature: f\n  Scenario: s\n    Given I am in debug mode\n",
        "  echo:\n    main:\n      prefix: p-\n      note: abc\n      note_refuse: true\n",
    );
    let (text, _) = bddkit_in(&dir, &["run", "--config", "cfg.yaml"]);
    assert!(text.contains(r#"refuses the note "abc""#), "{text}");
}

#[test]
fn resource_add_masks_a_secret_in_the_refusal_block() {
    let dir = project(
        "cfg-secret-resource-add",
        "Feature: f\n",
        &format!("{ECHO_GROUP}default_echo: main\n"),
    );
    let (text, code) = bddkit_in(
        &dir,
        &[
            "resource",
            "add",
            "echo",
            "second",
            "--config",
            "cfg.yaml",
            "--prefix",
            "q-",
            "--note",
            "plugin-note-9876",
            "--note_refuse",
            "true",
        ],
    );
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("refuses the note"), "{text}");
    assert!(!text.contains("plugin-note-9876"), "{text}");
}

/// The block is the whole value of the failure path, so it has to be pasteable
/// under `resources:`, which means carrying the group key when the config does
/// not have that group yet, and it is masked: a URL password in a value never
/// reaches the screen.
#[test]
fn resource_add_prints_a_masked_block_with_the_group_key_the_config_lacks() {
    let dir = project("resource-add-new-group", "Feature: f\n", "");
    let (text, code) = bddkit_in(
        &dir,
        &[
            "resource",
            "add",
            "echo",
            "second",
            "--config",
            "cfg.yaml",
            "--prefix",
            "http://app:add-pass-5678@127.0.0.1:1/app",
            "--probe_error",
            "the bucket is not there",
        ],
    );
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("The block, for manual insertion"), "{text}");
    assert!(
        text.contains("echo:\n  second:\n    prefix: http://app:***@127.0.0.1:1/app\n"),
        "the block names the group it belongs under, masked: {text}"
    );
    assert!(!text.contains("add-pass-5678"), "{text}");
}

#[test]
fn a_config_secret_is_masked_in_a_file_with_no_plugin_step() {
    let dir = project(
        "cfg-secret-no-step",
        "Feature: f\n  Scenario: s\n    Given set variable \"v\" to \"plugin-note-9876\"\n    Then variable \"v\" should be equal to \"other\"\n",
        "  echo:\n    main:\n      prefix: p-\n      note: plugin-note-9876\n",
    );
    let (text, code) = bddkit_in(&dir, &["run", "--config", "cfg.yaml"]);
    assert_eq!(code, Some(1), "{text}");
    assert!(!text.contains("plugin-note-9876"), "{text}");
}

#[test]
fn doctor_shows_the_detail_of_an_ok_instance_static_and_live() {
    let dir = project(
        "cfg-detail",
        "Feature: f\n  Scenario: s\n    Given I am in debug mode\n",
        "  echo:\n    main:\n      prefix: p-\n      detail: static-detail-text\n      probe_detail: live-detail-text\n    plain:\n      prefix: q-\ndefault_echo: main\n",
    );
    let (text, code) = bddkit_in(&dir, &["doctor", "--config", "cfg.yaml"]);
    assert_eq!(code, Some(0), "{text}");
    assert!(text.contains("static-detail-text"), "{text}");
    assert!(!text.contains("live-detail-text"), "{text}");

    let (live, _) = bddkit_in(&dir, &["doctor", "--config", "cfg.yaml", "--live"]);
    assert!(live.contains("live-detail-text"), "{live}");
    assert!(
        live.contains("probed clean"),
        "an ok instance without a detail keeps today's row:\n{live}"
    );

    let (json, _) = bddkit_in(
        &dir,
        &["doctor", "--config", "cfg.yaml", "--live", "--json"],
    );
    assert!(
        json.contains("static-detail-text") && json.contains("live-detail-text"),
        "{json}"
    );
}

#[test]
fn resource_add_masks_a_secret_only_the_probe_declares() {
    let dir = project(
        "cfg-probe-secret",
        "Feature: f\n",
        &format!("{ECHO_GROUP}default_echo: main\n"),
    );
    let (text, code) = bddkit_in(
        &dir,
        &[
            "resource",
            "add",
            "echo",
            "second",
            "--config",
            "cfg.yaml",
            "--prefix",
            "q-",
            "--probe_note",
            "plugin-probe-5555",
            "--probe_detail",
            "reached with plugin-probe-5555",
        ],
    );
    assert_eq!(code, Some(0), "{text}");
    assert!(text.contains("reached with"), "{text}");
    assert!(!text.contains("plugin-probe-5555"), "{text}");
}

#[test]
fn a_malformed_config_reply_still_has_its_secrets_masked() {
    let dir = project(
        "cfg-malformed-secret",
        "Feature: f\n  Scenario: s\n    Given I am in debug mode\n",
        "  echo:\n    main:\n      prefix: p-\n      note: plugin-note-9876\n      malformed: true\n",
    );
    let (text, code) = bddkit_in(&dir, &["run", "--config", "cfg.yaml"]);
    assert_eq!(code, Some(2), "{text}");
    assert!(text.contains("malformed validate_config reply"), "{text}");
    assert!(!text.contains("plugin-note-9876"), "{text}");
}
