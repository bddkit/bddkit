use super::{
    ECHO_GROUP, answers, assert_green, macro_project, polled, project, run, run_with, stdout_of,
};

#[test]
fn a_step_that_wants_vars_receives_them_flagged_and_null_aware() {
    let dir = project(
        "wants-vars",
        r#"Feature: f
  Scenario: s
    Given set variable "plain" to "hello"
    And set variable "db_secret" to "hunter22"
    And set variable "empty" to "<<null>>"
    And set variable "x" to "global" global
    And set variable "x" to "frame"
    When I echo the vars
    Then variable "echo_vars" should contain JSON:
      """
      {
        "plain": {"value": "hello", "secret": false},
        "db_secret": {"value": "hunter22", "secret": true},
        "empty": {"value": null, "secret": false},
        "x": {"value": "frame", "secret": false}
      }
      """
"#,
        "",
    );
    assert_green(&run(&dir));
}

#[test]
fn a_short_macro_secret_parameter_is_flagged_secret_in_the_vars() {
    let dir = macro_project(
        "vars-macro-secret",
        r#"Feature: f
  Scenario: s
    When I pin "1234"
    Then variable "echo_vars" should contain JSON:
      """
      {"pin": {"value": "1234", "secret": true}}
      """
"#,
        "- step: 'I pin \"{pin:secret}\"'\n  do: [I echo the vars]\n  exports: [echo_vars]\n",
    );
    assert_green(&run(&dir));
}

#[test]
fn a_step_that_does_not_ask_receives_an_empty_vars() {
    let dir = project(
        "no-vars",
        r#"Feature: f
  Scenario: s
    Given set variable "plain" to "hello"
    When I echo the vars without asking
    Then variable "echo_vars" should be equal to "{}"
"#,
        "",
    );
    assert_green(&run(&dir));
}

#[test]
fn a_raw_docstring_step_receives_its_text_uninterpolated() {
    let dir = project(
        "raw-doc",
        r#"Feature: f
  Scenario: s
    Given set variable "x" to "VALUE"
    When I echo the raw doc string:
      """
      a <<x>> b
      """
    # Expected values are interpolated, so a literal <<x>> cannot be written; [x] keeps the pattern from reading as a slot and still matches the text.
    Then variable "echo_doc" should match "^a <<[x]>> b$"
"#,
        "",
    );
    assert_green(&run(&dir));
}

#[test]
fn an_ordinary_plugin_doc_string_is_still_interpolated() {
    let dir = project(
        "plain-doc",
        r#"Feature: f
  Scenario: s
    Given set variable "x" to "VALUE"
    When I echo the plain doc string:
      """
      a <<x>> b
      """
    Then variable "echo_doc" should be equal to "a VALUE b"
"#,
        "",
    );
    assert_green(&run(&dir));
}

#[test]
fn output_is_printed_to_stderr_masked_and_never_to_stdout() {
    let reply = serde_json::json!({
        "status": "passed",
        "secrets": ["output-secret-value-1"],
        "output": [
            {"text": "{\"user\":\"ann\",\"password\":\"hunter2hunter2\"}", "syntax": "json"},
            {"text": "<a><b>output-secret-value-1</b></a>", "syntax": "xml"},
            {"text": "plain output-secret-value-1", "syntax": null},
            {"title": "== sent ==", "headers": [["Authorization", "ab"], ["X-Plain", "visible"]],
             "sensitive_headers": ["authorization"]},
        ],
    });
    let dir = project("output-forms", &answers(&reply), ECHO_GROUP);
    let out = run(&dir);
    let (stdout, stderr) = (
        stdout_of(&out),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    );
    assert!(out.status.success(), "{stdout}{stderr}");
    assert!(
        stderr.contains("\"user\": \"ann\""),
        "json is pretty-printed: {stderr}"
    );
    assert!(!stderr.contains("hunter2hunter2"), "{stderr}");
    assert!(!stderr.contains("output-secret-value-1"), "{stderr}");
    assert!(stderr.contains("<a>\n  <b>"), "xml is beautified: {stderr}");
    assert!(stderr.contains("Authorization : ***"), "{stderr}");
    assert!(stderr.contains("X-Plain       : visible"), "{stderr}");
    assert!(
        !stdout.contains("== sent =="),
        "output never reaches stdout: {stdout}"
    );

    let out = run_with(&dir, &["--reveal"], &[]);
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        stderr.contains("hunter2hunter2"),
        "--reveal shows it: {stderr}"
    );
    assert!(stderr.contains("Authorization : ab"), "{stderr}");
}

#[test]
fn a_polled_step_prints_its_output_once_per_attempt_and_a_bad_item_is_refused() {
    let script = serde_json::json!([
        {"status": "not_yet", "output": [{"text": "tick-marker"}]},
        {"status": "passed", "output": [{"text": "tick-marker"}]},
    ]);
    let dir = project("output-attempts", &polled(&script, "2"), ECHO_GROUP);
    let out = run(&dir);
    assert_green(&out);
    assert_eq!(
        String::from_utf8_lossy(&out.stderr)
            .matches("tick-marker")
            .count(),
        2
    );

    let reply = serde_json::json!({"status": "passed", "output": [{"nonsense": "secret-quote-9"}]});
    let dir = project("output-bad", &answers(&reply), ECHO_GROUP);
    let out = run(&dir);
    let all = format!(
        "{}{}",
        stdout_of(&out),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(all.contains("failed: 1 (1 plugin)"), "{all}");
    assert!(!all.contains("secret-quote-9"), "{all}");
}

#[test]
fn global_vars_write_the_global_frame_and_win_over_vars_for_one_name() {
    let reply = serde_json::json!({
        "status": "passed",
        "vars": {"both": "from-vars", "local": "kept"},
        "global_vars": {"both": "from-global", "only_global": "g"},
    });
    let feature = format!(
        "{}    Then variable \"both\" should be equal to \"from-global\"\n    And variable \"local\" should be equal to \"kept\"\n    And variable \"only_global\" should be equal to \"g\"\n",
        answers(&reply)
    );
    let dir = project("global-vars", &feature, ECHO_GROUP);
    let out = run(&dir);
    assert_green(&out);
    assert!(
        !stdout_of(&out).contains("shadows"),
        "a shadow this reply made is not warned about: {}",
        stdout_of(&out)
    );
}

#[test]
fn global_vars_warn_when_a_file_level_variable_shadows_them_and_a_failure_writes_nothing() {
    let reply = serde_json::json!({"status": "passed", "global_vars": {"seen": "g"}});
    let feature = format!(
        "Feature: f\n  Scenario: s\n    Given set variable \"seen\" to \"file\"\n{}    Then variable \"seen\" should be equal to \"file\"\n",
        answers(&reply).replace("Feature: f\n  Scenario: s\n", "")
    );
    let dir = project("global-vars-shadow", &feature, ECHO_GROUP);
    let out = run(&dir);
    assert_green(&out);
    assert!(
        stdout_of(&out).contains("shadows it"),
        "{}",
        stdout_of(&out)
    );

    let failing =
        serde_json::json!({"status": "fatal", "error": "no", "global_vars": {"ghost": "g"}});
    let feature = format!(
        "{}  Scenario: t\n    Then variable \"ghost\" should be equal to \"g\"\n",
        answers(&failing)
    );
    let dir = project("global-vars-failed", &feature, ECHO_GROUP);
    let text = stdout_of(&run(&dir));
    assert!(
        text.contains("failed: 2"),
        "the second scenario sees no ghost: {text}"
    );
}

#[test]
fn source_dir_is_the_absolute_directory_of_the_file_a_step_is_written_in() {
    let dir = project(
        "source-dir",
        "Feature: f\n  Scenario: s\n    When I echo the source directory as \"ok\"\n    Then variable \"echo_source_dir\" should be equal to \"@HERE@/features\"\n",
        ECHO_GROUP,
    );
    let here = std::path::absolute(&dir).unwrap().display().to_string();
    let feature = std::fs::read_to_string(dir.join("features/plugin.feature")).unwrap();
    std::fs::write(
        dir.join("features/plugin.feature"),
        feature.replace("@HERE@", &here),
    )
    .unwrap();
    // `paths: [features]` is relative: the run was started with a relative path.
    assert_green(&run(&dir));

    // The same value reaches `bddkit_validate_step`, before any request.
    let dir = project(
        "source-dir-validate",
        "Feature: f\n  Scenario: s\n    When I echo the source directory as \"refuse\"\n",
        ECHO_GROUP,
    );
    let out = run(&dir);
    let all = format!(
        "{}{}",
        stdout_of(&out),
        String::from_utf8_lossy(&out.stderr)
    );
    let here = std::path::absolute(&dir).unwrap().display().to_string();
    assert_eq!(out.status.code(), Some(2), "{all}");
    assert!(
        all.contains(&format!("source_dir={here}/features")),
        "{all}"
    );
}

#[test]
fn source_dir_follows_an_included_file_and_a_macro_body() {
    // Outside `paths`, so the include is the only way in; the macro's file
    // lives in `macros/`. Each step must see the directory it is written in.
    let dir = macro_project(
        "source-dir-places",
        "Feature: f\n  Scenario: s\n    When I include \"../inc/inc.feature\"\n    When I report from a macro\n    Then variable \"echo_source_dir\" should be equal to \"@HERE@/macros\"\n",
        "- step: 'I report from a macro'\n  do: [I echo the source directory as \"ok\"]\n  exports: [echo_source_dir]\n",
    );
    std::fs::create_dir_all(dir.join("inc")).unwrap();
    std::fs::write(
        dir.join("inc/inc.feature"),
        "Feature: inc\n  Scenario: i\n    When I echo the source directory as \"ok\"\n    Then variable \"echo_source_dir\" should be equal to \"@HERE@/inc\"\n",
    )
    .unwrap();
    let here = std::path::absolute(&dir).unwrap().display().to_string();
    let inc = dir.join("inc/inc.feature");
    std::fs::write(
        &inc,
        std::fs::read_to_string(&inc)
            .unwrap()
            .replace("@HERE@", &here),
    )
    .unwrap();
    let path = dir.join("features/plugin.feature");
    let feature = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, feature.replace("@HERE@", &here)).unwrap();
    assert_green(&run(&dir));

    // And at validation, for the macro body.
    let dir = macro_project(
        "source-dir-macro-validate",
        "Feature: f\n  Scenario: s\n    When I refuse from a macro\n",
        "- step: 'I refuse from a macro'\n  do: [I echo the source directory as \"refuse\"]\n",
    );
    let out = run(&dir);
    let all = format!(
        "{}{}",
        stdout_of(&out),
        String::from_utf8_lossy(&out.stderr)
    );
    let here = std::path::absolute(&dir).unwrap().display().to_string();
    assert_eq!(out.status.code(), Some(2), "{all}");
    assert!(all.contains(&format!("source_dir={here}/macros")), "{all}");
}

#[test]
fn output_masks_a_headers_title_and_names_and_a_value_the_reply_just_stored() {
    let reply = serde_json::json!({
        "status": "passed",
        "secrets": ["declared-title-secret-1", "declared-name-secret-2"],
        "vars": {"my_secret": "stored-value-3"},
        "output": [
            {"title": "== declared-title-secret-1 ==",
             "headers": [["declared-name-secret-2", "v"]]},
            {"text": "stored stored-value-3"},
        ],
    });
    let dir = project("output-masks-all", &answers(&reply), ECHO_GROUP);
    let stderr = String::from_utf8_lossy(&run(&dir).stderr).into_owned();
    for leaked in [
        "declared-title-secret-1",
        "declared-name-secret-2",
        "stored-value-3",
    ] {
        assert!(!stderr.contains(leaked), "{leaked}: {stderr}");
    }
    assert!(stderr.contains("== *** =="), "{stderr}");
    let stderr = String::from_utf8_lossy(&run_with(&dir, &["--reveal"], &[]).stderr).into_owned();
    assert!(stderr.contains("declared-title-secret-1"), "{stderr}");
    assert!(stderr.contains("stored-value-3"), "{stderr}");
}

#[test]
fn a_class_reaches_the_junit_type() {
    let reply = serde_json::json!({"status": "fatal", "error": "down", "class": "transport"});
    let dir = project("class-junit", &answers(&reply), ECHO_GROUP);
    run_with(&dir, &["--junit", "junit.xml"], &[]);
    let junit = std::fs::read_to_string(dir.join("junit.xml")).unwrap();
    assert_eq!(junit.matches("type=\"transport\"").count(), 1, "{junit}");
}

#[test]
fn a_global_written_in_a_macro_body_outlives_the_macro() {
    // The macro exports nothing, so a frame value would be gone with it; a
    // `global_vars` one is in the global frame and is not.
    let reply = serde_json::json!({"status": "passed", "global_vars": {"kept": "y"}});
    let dir = macro_project(
        "global-in-macro",
        "Feature: f\n  Scenario: s\n    When I answer from a macro\n    Then variable \"kept\" should be equal to \"y\"\n",
        &format!(
            "- step: 'I answer from a macro'\n  do:\n    - the echo answers: |\n        {reply}\n"
        ),
    );
    assert_green(&run(&dir));
}

#[test]
fn a_refused_output_writes_no_variable_and_a_skipped_var_is_still_masked() {
    let reply = serde_json::json!({
        "status": "passed", "vars": {"written": "w"}, "output": [{"nonsense": 1}],
        "diagnostics": [{"title": "kept-title", "kind": "text", "content": "x"}],
    });
    let feature = format!(
        "{}    Then variable \"written\" should be equal to \"w\"\n",
        answers(&reply)
    );
    let dir = project("output-refused-vars", &feature, ECHO_GROUP);
    let text = stdout_of(&run(&dir));
    assert!(text.contains("fits neither shape"), "{text}");
    assert!(
        text.contains("kept-title"),
        "the diagnostics still show: {text}"
    );
    assert!(
        !text.contains("is not equal") && !text.contains("expected: w"),
        "{text}"
    );

    let reply = serde_json::json!({
        "status": "passed",
        "vars": {"my_secret": "skipped-val-4"}, "global_vars": {"my_secret": "g"},
        "output": [{"text": "quoted skipped-val-4"}],
    });
    let dir = project("output-skipped-var", &answers(&reply), ECHO_GROUP);
    let stderr = String::from_utf8_lossy(&run(&dir).stderr).into_owned();
    assert!(!stderr.contains("skipped-val-4"), "{stderr}");
}
