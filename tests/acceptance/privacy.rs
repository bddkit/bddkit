use super::{
    masking_project, of_type, read_stream, run_bddkit_in, run_masking, write_doctor_project,
    write_report_project,
};
use serde_json::Value;
use std::process::Command;

// ---- privacy: the layers that decide what is masked (spec §7.7, §7.9) ----

/// A project whose config carries `privacy_yaml` as extra top-level text, run
/// with an events stream, `args` and `envs`.
fn run_privacy(
    name: &str,
    privacy_yaml: &str,
    args: &[&str],
    envs: &[(&str, &str)],
) -> (std::process::Output, Vec<Value>, std::path::PathBuf) {
    let cfg = write_report_project(name);
    let mut text = std::fs::read_to_string(&cfg).expect("read config");
    text.push_str(privacy_yaml);
    std::fs::write(&cfg, text).expect("write config");
    let events = cfg.with_file_name("events.ndjson");
    let mut command = Command::new(env!("CARGO_BIN_EXE_bddkit"));
    command
        .args(["run", "--config"])
        .arg(&cfg)
        .arg("--events")
        .arg(&events)
        .args(args);
    for key in [
        "BDDKIT_PRIVACY_WORDS",
        "BDDKIT_PRIVACY_WORDS_ONLY",
        "BDDKIT_PRIVACY_FIELDS",
        "BDDKIT_PRIVACY_FIELDS_ONLY",
        "BDDKIT_PRIVACY_MIN_LENGTH",
        "BDDKIT_PRIVACY_REVEAL",
        "BDDKIT_ARTIFACTS_DIR",
    ] {
        command.env_remove(key);
    }
    let out = command
        .envs(envs.iter().copied())
        .output()
        .expect("run bddkit");
    let stream = read_stream(&events);
    (out, stream, cfg)
}

#[test]
fn run_started_records_the_privacy_vocabulary_in_force() {
    let (_, stream, _) = run_privacy("privacy-default", "", &[], &[]);
    assert_eq!(
        stream[0]["privacy"],
        serde_json::json!({
            "masking": true,
            "words": ["secret"],
            "fields": ["password", "secret", "token"],
            "min_length": 7,
        })
    );
}

#[test]
fn the_flag_beats_the_environment_beats_the_config_for_min_length() {
    let config = "privacy:\n  min_length: 3\n";
    let min_length = |args: &[&str], envs: &[(&str, &str)]| {
        let (_, stream, _) = run_privacy("privacy-min", config, args, envs);
        stream[0]["privacy"]["min_length"].clone()
    };
    assert_eq!(min_length(&[], &[]), 3);
    assert_eq!(min_length(&[], &[("BDDKIT_PRIVACY_MIN_LENGTH", "5")]), 5);
    let both = [("BDDKIT_PRIVACY_MIN_LENGTH", "5")];
    assert_eq!(min_length(&["--privacy-min-length", "9"], &both), 9);
}

#[test]
fn the_lists_extend_or_replace_across_the_layers() {
    let (_, stream, _) = run_privacy(
        "privacy-lists",
        "privacy:\n  fields: [otp]\n  words_only: [gdpr]\n",
        &["--privacy-fields", "pin,cvv"],
        &[("BDDKIT_PRIVACY_FIELDS_ONLY", "iban")],
    );
    // The environment resets the list below it, the flag then adds to it.
    assert_eq!(
        stream[0]["privacy"]["fields"],
        serde_json::json!(["iban", "pin", "cvv"])
    );
    assert_eq!(stream[0]["privacy"]["words"], serde_json::json!(["gdpr"]));
}

#[test]
fn reveal_is_announced_on_stderr_and_recorded_in_the_stream() {
    let (out, stream, _) = run_privacy("privacy-reveal", "", &["--reveal"], &[]);
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("masking is off"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(stream[0]["privacy"]["masking"], false);
}

#[test]
fn the_environment_overrides_a_reveal_committed_in_the_config() {
    let config = "privacy:\n  reveal: true\n";
    let (out, stream, _) = run_privacy("privacy-committed", config, &[], &[]);
    assert_eq!(stream[0]["privacy"]["masking"], false);
    assert!(String::from_utf8_lossy(&out.stderr).contains("masking is off"));

    let ci = [("BDDKIT_PRIVACY_REVEAL", "false")];
    let (out, stream, _) = run_privacy("privacy-ci", config, &[], &ci);
    assert_eq!(stream[0]["privacy"]["masking"], true);
    assert!(!String::from_utf8_lossy(&out.stderr).contains("masking is off"));
}

#[test]
fn a_bad_privacy_value_is_exit_two_in_run_and_a_failed_row_in_doctor() {
    let bad = [("BDDKIT_PRIVACY_REVEAL", "yes")];
    let (out, _, cfg) = run_privacy("privacy-bad", "", &[], &bad);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("BDDKIT_PRIVACY_REVEAL"));

    let doctor = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config"])
        .arg(&cfg)
        .envs(bad)
        .output()
        .expect("run doctor");
    let stdout = String::from_utf8_lossy(&doctor.stdout);
    assert_eq!(doctor.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("BDDKIT_PRIVACY_REVEAL"), "{stdout}");
}

#[test]
fn an_unknown_key_under_privacy_is_refused() {
    let typo = "privacy:\n  wrods: [x]\n";
    let (out, _, cfg) = run_privacy("privacy-typo", typo, &[], &[]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("wrods"));

    let doctor = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config"])
        .arg(&cfg)
        .output()
        .expect("run doctor");
    let stdout = String::from_utf8_lossy(&doctor.stdout);
    assert_eq!(doctor.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("wrods"), "{stdout}");
}

#[test]
fn the_junit_report_carries_the_masked_variable_text() {
    let feature = variable_feature(
        &[("access_secret", "junit-secret-value-3")],
        "access_secret",
    );
    let cfg = masking_project("mask-junit-var", &feature, "");
    let junit = cfg.with_file_name("junit.xml");
    let (code, _, _) = run_masking(&cfg, &["--junit", junit.to_str().expect("utf-8")], &[]);
    assert_eq!(code, 1);
    let xml = std::fs::read_to_string(&junit).expect("junit report");
    assert!(xml.contains("actual:   ***"), "{xml}");
    assert!(!xml.contains("junit-secret-value-3"), "{xml}");
}

#[test]
fn reveal_on_a_variable_turns_masking_off_and_says_so() {
    let feature = variable_feature(
        &[("access_secret", "reveal-secret-value-9")],
        "access_secret",
    );
    let cfg = masking_project("mask-reveal-var", &feature, "");
    let (_, stdout, stderr) = run_masking(&cfg, &["--reveal"], &[]);
    assert!(stdout.contains("reveal-secret-value-9"), "{stdout}");
    assert!(stderr.contains("masking is off"), "{stderr}");
}

fn variable_feature(set: &[(&str, &str)], check: &str) -> String {
    let mut text = String::from("Feature: masking\n  Scenario: a failing assertion\n");
    for (name, value) in set {
        text.push_str(&format!(
            "    Given set variable \"{name}\" to \"{value}\"\n"
        ));
    }
    text.push_str(&format!(
        "    Then variable \"{check}\" should be equal to \"nothing\"\n"
    ));
    text
}

#[test]
fn a_variable_with_a_privacy_word_is_masked_wherever_its_value_appears() {
    for name in ["access_token_secret", "secret_reg_verifier"] {
        let feature = variable_feature(&[(name, "verifier-value-77")], name);
        let cfg = masking_project("mask-var", &feature, "");
        let (code, stdout, _) = run_masking(&cfg, &[], &[]);
        assert_eq!(code, 1, "{stdout}");
        assert!(!stdout.contains("verifier-value-77"), "{name}: {stdout}");
        assert!(stdout.contains("***"), "{name}: {stdout}");
    }
}

/// `as "reg_secret"` plus an exported `verifier` is what makes this shape.
#[test]
fn a_privacy_word_in_the_middle_of_a_name_still_masks() {
    let name = "reg_secret_verifier";
    let feature = variable_feature(&[(name, "middle-value-4321")], name);
    let cfg = masking_project("mask-var-middle", &feature, "");
    let (code, stdout, _) = run_masking(&cfg, &[], &[]);
    assert_eq!(code, 1, "{stdout}");
    assert!(!stdout.contains("middle-value-4321"), "{stdout}");
    assert!(stdout.contains("***"), "{stdout}");
}

#[test]
fn a_name_that_only_contains_a_privacy_word_does_not_mask() {
    for name in ["secretary_id", "sessionSecret"] {
        let feature = variable_feature(&[(name, "plain-value-88")], name);
        let cfg = masking_project("mask-var-no", &feature, "");
        let (_, stdout, _) = run_masking(&cfg, &[], &[]);
        assert!(stdout.contains("plain-value-88"), "{name}: {stdout}");
    }
}

#[test]
fn privacy_words_only_replaces_the_default_word() {
    let feature = "Feature: masking
  Scenario: the new word
    Given set variable \"gdpr_note\" to \"gdpr-value-12345\"
    Then variable \"gdpr_note\" should be equal to \"nothing\"

  Scenario: the old word
    Given set variable \"secret_note\" to \"secret-value-1234\"
    Then variable \"secret_note\" should be equal to \"nothing\"
";
    let cfg = masking_project(
        "mask-words-only",
        feature,
        "privacy:\n  words_only: [gdpr]\n",
    );
    let (_, stdout, _) = run_masking(&cfg, &[], &[]);
    assert!(!stdout.contains("gdpr-value-12345"), "{stdout}");
    assert!(stdout.contains("secret-value-1234"), "{stdout}");
    // Without the key the default word applies: `secret_` masks, `gdpr_` does not.
    let cfg = masking_project("mask-words-default", feature, "");
    let (_, stdout, _) = run_masking(&cfg, &[], &[]);
    assert!(stdout.contains("gdpr-value-12345"), "{stdout}");
    assert!(!stdout.contains("secret-value-1234"), "{stdout}");
}

#[test]
fn a_short_secret_is_not_added_to_the_value_filter() {
    let feature = variable_feature(&[("secret_pin", "tiny")], "secret_pin");
    let cfg = masking_project("mask-short", &feature, "");
    let (_, stdout, _) = run_masking(&cfg, &[], &[]);
    assert!(stdout.contains("tiny"), "{stdout}");
    // The same value is masked once the minimum length allows it.
    let (_, stdout, _) = run_masking(&cfg, &["--privacy-min-length", "4"], &[]);
    assert!(!stdout.contains("tiny"), "{stdout}");
}

#[test]
fn a_longer_secret_is_masked_before_one_it_contains() {
    let feature = variable_feature(
        &[
            ("secret_short", "abcdefgh"),
            ("secret_long", "abcdefgh-and-more-1"),
        ],
        "secret_long",
    );
    let cfg = masking_project("mask-longer", &feature, "");
    let (_, stdout, _) = run_masking(&cfg, &[], &[]);
    assert!(!stdout.contains("and-more"), "{stdout}");
    assert!(!stdout.contains("abcdefgh"), "{stdout}");
}

/// An `I include` swaps the whole variable stack, but not what the file has
/// learned to hide: a secret saved inside the included scenario is still masked
/// when the caller fails later.
#[test]
fn a_secret_saved_inside_an_include_is_masked_in_the_callers_failure() {
    let dir = std::env::temp_dir().join(format!("bddkit-mask-include-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/target.feature"),
        "Feature: setup\n  @exports(leak)\n  Scenario: signs in\n    Given set variable \"inner_secret\" to \"included-value-1\"\n    And set variable \"leak\" to \"included-value-1\"\n",
    )
    .expect("write target");
    std::fs::write(
        dir.join("features/caller.feature"),
        "Feature: caller\n  Scenario: fails afterwards\n    Given I include \"target.feature\"\n    Then variable \"leak\" should be equal to \"nothing\"\n",
    )
    .expect("write caller");
    std::fs::write(
        dir.join("cfg.yaml"),
        "paths: [features]\nresources:\n  api: {}\n",
    )
    .expect("write config");
    let (code, stdout, _) = run_bddkit_in("run", &dir);
    assert_eq!(code, Some(1), "{stdout}");
    assert!(!stdout.contains("included-value-1"), "{stdout}");
}

// ---- privacy: output with no World around it (spec §7.6, §7.8) ----

#[test]
fn doctor_says_so_on_stderr_when_masking_is_off() {
    let cfg = write_doctor_project("mask-doctor-reveal", "", "");
    for (args, envs) in [
        (&["--reveal"][..], &[][..]),
        (&[][..], &[("BDDKIT_PRIVACY_REVEAL", "1")][..]),
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
            .args(["doctor", "--config"])
            .arg(&cfg)
            .args(args)
            .envs(envs.iter().copied())
            .output()
            .expect("run doctor");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("masking is off"), "{stderr}");
    }
}

#[test]
fn the_event_stream_carries_the_masked_failure_text() {
    let feature = variable_feature(&[("secret_note", "stream-value-1234")], "secret_note");
    let cfg = masking_project("mask-stream", &feature, "");
    let events = cfg.with_file_name("events.ndjson");
    let (code, _, _) = run_masking(&cfg, &["--events", events.to_str().expect("utf-8")], &[]);
    assert_eq!(code, 1);
    let failure = of_type(&read_stream(&events), "scenario_finished")[0]["failure"]
        .as_str()
        .expect("failure text")
        .to_string();
    assert!(failure.contains("***"), "{failure}");
    assert!(!failure.contains("stream-value-1234"), "{failure}");
}

// ---- privacy: the `secret` parameter type (spec §7.2) ----

const AES_LITERAL: &str = r#"Feature: masking
  Scenario: encrypted with a literal key
    Given I encrypt "555555" with AES using key "aes-literal-key-1" as "c"
    Then variable "c" should be equal to "nothing"
"#;

#[test]
fn a_literal_secret_is_masked_in_the_step_text_of_a_report() {
    let cfg = masking_project("mask-literal", AES_LITERAL, "");
    let junit = cfg.with_file_name("junit.xml");
    let events = cfg.with_file_name("events.ndjson");
    let args = [
        "--junit",
        junit.to_str().expect("utf-8"),
        "--events",
        events.to_str().expect("utf-8"),
    ];
    let (code, stdout, _) = run_masking(&cfg, &args, &[]);
    assert_eq!(code, 1, "{stdout}");
    let xml = std::fs::read_to_string(&junit).expect("junit report");
    assert!(xml.contains("using key &quot;***&quot;"), "{xml}");
    assert!(!xml.contains("aes-literal-key-1"), "{xml}");
    let stream = std::fs::read_to_string(&events).expect("stream");
    assert!(!stream.contains("aes-literal-key-1"), "{stream}");
    assert!(stream.contains(r#"using key \"***\""#), "{stream}");
}

#[test]
fn a_secret_parameter_is_masked_in_the_failure_dump() {
    // The failing step is the one that takes the secret.
    let feature = r#"Feature: masking
  Scenario: a key of the wrong size
    Given I encrypt "payload" with AES using key "short-literal-key" as "c"
"#;
    let cfg = masking_project("mask-aes", feature, "");
    let (code, stdout, _) = run_masking(&cfg, &[], &[]);
    assert_eq!(code, 1, "{stdout}");
    assert!(stdout.contains(r#"using key "***" as "c""#), "{stdout}");
    assert!(!stdout.contains("short-literal-key"), "{stdout}");
}

/// One parser serves builtin declarations and macro templates, so a macro's
/// `{pw:secret}` marks its argument without any other change.
#[test]
fn a_macro_template_can_declare_a_secret_parameter() {
    let dir = std::env::temp_dir().join(format!("bddkit-mask-macro-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/m.feature"),
        "Feature: f\n  Scenario: s\n    Given I log in as \"ann\" with password \"macro-literal-pw1\"\n",
    )
    .expect("write feature");
    std::fs::write(
        dir.join("macros.yaml"),
        "- step: 'I log in as \"{user}\" with password \"{pw:secret}\"'\n  do:\n    - variable \"pw\" should be equal to \"nothing\"\n",
    )
    .expect("write macros");
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "macro_paths: [{}]\npaths: [{}]\nresources:\n  api: {{}}\n",
            dir.join("macros.yaml")
                .display()
                .to_string()
                .replace('\\', "/"),
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/"),
        ),
    )
    .expect("write config");
    let (code, stdout, _) = run_masking(&dir.join("cfg.yaml"), &[], &[]);
    assert_eq!(code, 1, "{stdout}");
    assert!(stdout.contains(r#"with password "***""#), "{stdout}");
    assert!(!stdout.contains("macro-literal-pw1"), "{stdout}");
}

/// A literal shorter than `min_length` never joins the set, so only the
/// step text itself can hide it: the console, the report and the stream must
/// each show `***`, and never the value.
#[test]
fn a_short_literal_secret_is_hidden_by_position_everywhere() {
    let feature = r#"Feature: masking
  Scenario: a short key
    Given I encrypt "payload" with AES using key "q7z" as "c"
"#;
    let cfg = masking_project("mask-short-literal", feature, "");
    let junit = cfg.with_file_name("junit.xml");
    let events = cfg.with_file_name("events.ndjson");
    let args = [
        "--junit",
        junit.to_str().expect("utf-8"),
        "--events",
        events.to_str().expect("utf-8"),
    ];
    let (code, stdout, _) = run_masking(&cfg, &args, &[]);
    assert_eq!(code, 1, "{stdout}");
    let xml = std::fs::read_to_string(&junit).expect("junit report");
    let stream = std::fs::read_to_string(&events).expect("stream");
    for (place, text) in [("console", &stdout), ("junit", &xml), ("stream", &stream)] {
        assert!(!text.contains("q7z"), "{place}: {text}");
        assert!(text.contains("***"), "{place}: {text}");
    }
}

#[test]
fn the_debug_line_of_a_macro_call_hides_a_short_literal_secret() {
    let dir = std::env::temp_dir().join(format!("bddkit-mask-macro-debug-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/m.feature"),
        "Feature: f\n  Scenario: s\n    Given I am in debug mode\n    And I log in as \"ann\" with password \"q7z\"\n",
    )
    .expect("write feature");
    std::fs::write(
        dir.join("macros.yaml"),
        "- step: 'I log in as \"{user}\" with password \"{pw:secret}\"'\n  do:\n    - set variable \"user_seen\" to \"<<user>>\"\n",
    )
    .expect("write macros");
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "macro_paths: [{}]\npaths: [{}]\nresources:\n  api: {{}}\n",
            dir.join("macros.yaml")
                .display()
                .to_string()
                .replace('\\', "/"),
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/"),
        ),
    )
    .expect("write config");
    let (code, _, stderr) = run_masking(&dir.join("cfg.yaml"), &[], &[]);
    assert_eq!(code, 0, "{stderr}");
    assert!(stderr.contains("macro"), "{stderr}");
    assert!(!stderr.contains("q7z"), "{stderr}");
}

// ---- privacy: sensitive fields found by the name the system gave them (spec §7.5) ----

#[test]
fn doctor_lists_sensitive_looking_names_without_failing() {
    let feature = r#"Feature: names
  Scenario: s
    Given set variable "session_token" to "x"
    And set variable "token_secret" to "y"
    And set variable "primary_key" to "z"
    And set variable "monkey" to "w"
    Then variable "session_token" should be equal to "<<session_token>>"
    And variable "token_secret" should be equal to "<<token_secret>>"
    And variable "primary_key" should be equal to "<<primary_key>>"
    And variable "monkey" should be equal to "<<monkey>>"
"#;
    let cfg = masking_project("mask-doctor-hint", feature, "");
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config"])
        .arg(&cfg)
        .output()
        .expect("run doctor");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "never a failure: {stdout}");
    let hint = stdout
        .split_once("look sensitive")
        .unwrap_or_else(|| panic!("no hint in: {stdout}"))
        .1;
    assert!(
        hint.contains("session_token") && hint.contains("primary_key"),
        "{hint}"
    );
    // A privacy word is in `token_secret`; `monkey` has no word of its own.
    assert!(
        !hint.contains("token_secret") && !hint.contains("monkey"),
        "{hint}"
    );
}

#[test]
fn doctor_hint_skips_a_secret_macro_parameter_and_says_where_it_found_the_rest() {
    let feature = "Feature: f\n  Scenario: s\n    Given I log in as \"ann\" with password \"x\"\n    And I log in plain as \"ann\" with password \"x\"\n";
    let cfg = masking_project("mask-doctor-macro", feature, "");
    let dir = cfg.parent().expect("project dir").to_path_buf();
    let macros = dir.join("macros.yaml");
    std::fs::write(
        &macros,
        r#"- step: 'I log in as "{user}" with password "{password:secret}"'
  do:
    - set variable "body" to "<<user>>:<<password>>"
- step: 'I log in plain as "{user}" with password "{password}"'
  do:
    - set variable "body" to "<<user>>:<<password>>"
"#,
    )
    .expect("write macros");
    let mut config = std::fs::read_to_string(&cfg).expect("config");
    config.push_str(&format!("macro_paths: [{}]\n", macros.display()));
    std::fs::write(&cfg, config).expect("write config");
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config"])
        .arg(&cfg)
        .output()
        .expect("run doctor");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout
        .lines()
        .find(|line| line.contains("password —"))
        .unwrap_or_else(|| panic!("no located hint in: {stdout}"));
    assert!(line.contains("macros.yaml:4"), "{line}");
    assert!(!line.contains("macros.yaml:1"), "{line}");
}

/// `Show all variables` prints a name and a value, so the name decides: a value
/// too short to join the set is still hidden by the privacy word in its name.
#[test]
fn show_variables_hides_a_short_value_by_its_variables_name() {
    let feature = r#"Feature: masking
  Scenario: show
    Given set variable "secret_pin" to "q8z"
    And set variable "plain_note" to "kept-visible"
    And I am in debug mode
    Then Show all variables
    And Show "secret_pin" variable
"#;
    let cfg = masking_project("mask-show", feature, "");
    let (code, _, stderr) = run_masking(&cfg, &[], &[]);
    assert_eq!(code, 0, "{stderr}");
    assert!(stderr.contains("secret_pin: ***"), "{stderr}");
    assert!(stderr.contains("plain_note: kept-visible"), "{stderr}");
    assert!(!stderr.contains("q8z"), "{stderr}");
}

// ---- privacy: leaks found in review ----

/// `--reveal` turns masking off, the step text of a `secret` parameter included.
#[test]
fn reveal_also_shows_a_literal_secret_parameter_in_step_text() {
    let feature = r#"Feature: masking
  Scenario: a key of the wrong size
    Given I encrypt "payload" with AES using key "short-literal-key" as "c"
"#;
    let cfg = masking_project("mask-reveal-literal", feature, "");
    let (_, stdout, _) = run_masking(&cfg, &["--reveal"], &[]);
    assert!(
        stdout.contains(r#"using key "short-literal-key""#),
        "{stdout}"
    );
}
