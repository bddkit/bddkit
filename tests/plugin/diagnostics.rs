use super::{
    ECHO_GROUP, answers, assert_green, macro_project, project, read_stream, run, run_with,
    stdout_of,
};

fn http_content(request_body: &str, response_body: &str) -> serde_json::Value {
    serde_json::json!({
        "method": "POST",
        "url": "http://svc.test/login?token=query-secret-1&page=2",
        "request": {
            "headers": [["Authorization", "Bearer bearer-secret-1"], ["X-Plain", "ok"]],
            "body": request_body,
        },
        "status": 401,
        "response": {
            "headers": [["Set-Cookie", "sid=cookie-secret-1; Path=/"], ["Content-Type", "application/json"]],
            "body": response_body,
        },
        "duration_us": 7,
        "sensitive_headers": ["authorization", "set-cookie"],
        "request_form": "raw",
    })
}

#[test]
fn an_http_object_is_dumped_and_emitted_masked_by_position() {
    let reply = serde_json::json!({
        "status": "fatal",
        "error": "login refused",
        "diagnostics": [{
            "kind": "http",
            "content": http_content(
                r#"{"password":"pw-secret-1","name":"ann"}"#,
                r#"{"token":"tok-secret-1","id":42}"#,
            ),
        }],
    });
    let dir = project("structured-http", &answers(&reply), ECHO_GROUP);
    let out = run_with(&dir, &["--events", "events.ndjson", "--evidence"], &[]);
    let text = stdout_of(&out);
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert!(text.contains("login refused"), "{text}");
    assert!(
        text.contains("POST http://svc.test/login?token=***&page=2"),
        "{text}"
    );
    assert!(text.contains("Authorization: ***"), "{text}");
    assert!(text.contains("X-Plain: ok"), "{text}");
    assert!(text.contains("← 401"), "{text}");
    for secret in [
        "query-secret-1",
        "bearer-secret-1",
        "pw-secret-1",
        "tok-secret-1",
        "cookie-secret-1",
    ] {
        assert!(!text.contains(secret), "{secret} leaked: {text}");
    }
    let stream = read_stream(&dir);
    let evidence = stream
        .iter()
        .find(|e| e["type"] == "evidence")
        .expect("an evidence event");
    assert_eq!(evidence["kind"], "http");
    assert_eq!(
        (evidence["group"].as_str(), evidence["resource"].as_str()),
        (Some("echo"), Some("main"))
    );
    assert!(
        evidence.get("title").is_none(),
        "title is optional: {evidence}"
    );
    let content = &evidence["content"];
    assert_eq!(content["status"], 401);
    for stripped in ["sensitive_headers", "request_form", "in_dump", "resource"] {
        assert!(
            content.get(stripped).is_none(),
            "{stripped} stays out: {content}"
        );
    }
    assert_eq!(
        content["request"]["headers"][0],
        serde_json::json!(["Authorization", "***"])
    );
    let raw = std::fs::read_to_string(dir.join("events.ndjson")).unwrap();
    for secret in [
        "query-secret-1",
        "bearer-secret-1",
        "pw-secret-1",
        "tok-secret-1",
        "cookie-secret-1",
    ] {
        assert!(
            !raw.contains(secret),
            "{secret} leaked into the stream: {raw}"
        );
    }

    let revealed = run_with(&dir, &["--reveal"], &[]);
    let text = stdout_of(&revealed);
    assert!(
        text.contains("POST http://svc.test/login?token=query-secret-1&page=2"),
        "{text}"
    );
    assert!(text.contains("Bearer bearer-secret-1"), "{text}");
}

#[test]
fn a_form_body_and_a_long_response_follow_the_built_in_rules() {
    let long = format!(r#"{{"token":"tok-long-1","pad":"{}"}}"#, "x".repeat(20_000));
    let mut content = http_content("password=form-secret-1&name=ann", &long);
    content["request_form"] = "form".into();
    let reply = serde_json::json!({
        "status": "fatal", "error": "refused",
        "diagnostics": [{"kind": "http", "title": "login", "content": content}],
    });
    let dir = project("structured-http-form", &answers(&reply), ECHO_GROUP);
    let out = run_with(
        &dir,
        &[
            "--events",
            "events.ndjson",
            "--evidence",
            "--artifacts-dir",
            "art",
        ],
        &[],
    );
    let text = stdout_of(&out);
    assert!(text.contains("password=***&name=ann"), "{text}");
    assert!(
        !text.contains("form-secret-1") && !text.contains("tok-long-1"),
        "{text}"
    );
    // The dump cuts the body to 600 characters, after masking.
    assert!(text.contains('…'), "{text}");
    let stream = read_stream(&dir);
    let evidence = stream.iter().find(|e| e["type"] == "evidence").unwrap();
    assert_eq!(evidence["title"], "login");
    let body = &evidence["content"]["response"]["body"];
    let path = body["path"].as_str().expect("a long body goes to a file");
    assert!(path.contains("response-body."), "{path}");
    let written = std::fs::read_to_string(dir.join("art").join(path)).unwrap();
    assert!(
        !written.contains("tok-long-1") && written.contains("***"),
        "{written}"
    );
}

#[test]
fn an_sql_object_is_masked_by_value_only_and_stays_out_of_the_dump() {
    let reply = serde_json::json!({
        "status": "fatal", "error": "statement refused", "secrets": ["sql-secret-1"],
        "diagnostics": [{
            "kind": "sql", "resource": "orders",
            "content": {"sql": "select * from t where k = ?", "binds": ["sql-secret-1", null], "duration_us": 3},
        }],
    });
    let dir = project("structured-sql", &answers(&reply), ECHO_GROUP);
    let out = run_with(&dir, &["--events", "events.ndjson", "--evidence"], &[]);
    let text = stdout_of(&out);
    assert!(text.contains("statement refused"), "{text}");
    assert!(
        !text.contains("select * from t"),
        "sql is never in the dump: {text}"
    );
    let stream = read_stream(&dir);
    let evidence = stream.iter().find(|e| e["type"] == "evidence").unwrap();
    assert_eq!(evidence["kind"], "sql");
    assert_eq!(evidence["resource"], "orders");
    assert_eq!(
        evidence["content"]["binds"],
        serde_json::json!(["***", null])
    );
}

#[test]
fn a_diagnostic_that_is_not_in_the_dump_is_still_evidence() {
    let reply = serde_json::json!({
        "status": "fatal", "error": "refused",
        "diagnostics": [
            {"kind": "http", "in_dump": false, "content": http_content("", "{}")},
            {"title": "hidden note", "kind": "text", "content": "shown nowhere", "in_dump": false},
            {"title": "shown note", "kind": "text", "content": "shown in the dump"},
        ],
    });
    let dir = project("structured-in-dump", &answers(&reply), ECHO_GROUP);
    let out = run_with(&dir, &["--events", "events.ndjson", "--evidence"], &[]);
    let text = stdout_of(&out);
    assert!(text.contains("shown in the dump"), "{text}");
    assert!(
        !text.contains("svc.test") && !text.contains("shown nowhere"),
        "{text}"
    );
    let kinds: Vec<_> = read_stream(&dir)
        .iter()
        .filter(|e| e["type"] == "evidence")
        .map(|e| e["kind"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(kinds, ["http", "text", "text"]);
}

#[test]
fn without_evidence_a_failing_reply_reaches_the_dump_only() {
    let reply = serde_json::json!({
        "status": "fatal", "error": "refused",
        "diagnostics": [{"kind": "http", "content": http_content("", "{}")}],
    });
    let dir = project("structured-no-evidence", &answers(&reply), ECHO_GROUP);
    let out = run_with(&dir, &["--events", "events.ndjson"], &[]);
    assert!(stdout_of(&out).contains("POST http://svc.test/login"));
    assert!(read_stream(&dir).iter().all(|e| e["type"] != "evidence"));
}

#[test]
fn a_shape_that_does_not_fit_fails_with_class_plugin_and_quotes_nothing() {
    let mut content = http_content("", "{}");
    content["status"] = "shape-secret-1".into();
    let reply = serde_json::json!({
        "status": "passed", "secrets": ["shape-secret-1"],
        "diagnostics": [{"kind": "http", "content": content}],
    });
    let dir = project("structured-bad-shape", &answers(&reply), ECHO_GROUP);
    let out = run_with(&dir, &["--junit", "junit.xml"], &[]);
    let text = stdout_of(&out);
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert!(text.contains("does not fit its shape"), "{text}");
    assert!(!text.contains("shape-secret-1"), "{text}");
    let junit = std::fs::read_to_string(dir.join("junit.xml")).unwrap();
    assert!(junit.contains("plugin"), "{junit}");
}

#[test]
fn a_refused_reply_still_shows_the_plugin_error_masked() {
    let mut content = http_content("", "{}");
    content["status"] = "x".into();
    let reply = serde_json::json!({
        "status": "fatal", "error": "access denied for refused-secret-1",
        "secrets": ["refused-secret-1"],
        "diagnostics": [{"kind": "http", "content": content}],
    });
    let dir = project("structured-refused-error", &answers(&reply), ECHO_GROUP);
    let text = stdout_of(&run(&dir));
    assert!(text.contains("does not fit its shape"), "{text}");
    assert!(text.contains("access denied for ***"), "{text}");
    assert!(!text.contains("refused-secret-1"), "{text}");
}

#[test]
fn a_malformed_reply_still_has_its_diagnostics_and_scan_masked() {
    // `status` is not a status, so the typed parse fails; the salvage keeps
    // what the reply declared and the diagnostics that fit.
    let reply = serde_json::json!({
        "status": 7,
        "scan": [{"kind": "json", "content": {"token": "salvage-scan-1"}}],
        "diagnostics": [
            {"kind": "http", "content": http_content("", "{}")},
            {"kind": 5, "content": http_content("", r#"{"token":"salvage-body-1"}"#)},
        ],
    });
    let dir = project("structured-salvage", &answers(&reply), ECHO_GROUP);
    let text = stdout_of(&run(&dir));
    assert!(text.contains("malformed"), "{text}");
    for leaked in ["bearer-secret-1", "cookie-secret-1", "salvage-body-1"] {
        assert!(!text.contains(leaked), "{leaked} leaked: {text}");
    }
}

#[test]
fn a_scan_item_registers_what_it_finds_on_any_status() {
    // The next step quotes the value; only a registered one is masked.
    for (name, scan, value) in [
        (
            "json",
            serde_json::json!({"kind": "json", "content": r#"{"token":"scan-json-1"}"#}),
            "scan-json-1",
        ),
        (
            "form",
            serde_json::json!({"kind": "form", "pairs": [["password", "a&b=c%41+d"], ["name", "plain-name-1"]]}),
            "a&b=c%41+d",
        ),
        (
            "query",
            serde_json::json!({"kind": "query", "pairs": [["token", "scan-query-1"]]}),
            "scan-query-1",
        ),
    ] {
        for status in ["passed", "fatal"] {
            let reply = serde_json::json!({"status": status, "error": "scanned", "scan": [scan]});
            let mut feature = answers(&reply);
            if status == "passed" {
                feature.push_str(&format!(
                    "    Then the echo should fail quoting \"{value}\"\n"
                ));
            }
            let dir = project(&format!("scan-{name}-{status}"), &feature, ECHO_GROUP);
            let text = stdout_of(&run(&dir));
            if status == "passed" {
                assert!(text.contains("the echo quoted ***"), "{name}: {text}");
            }
            assert!(
                !text.contains(value),
                "{name}/{status}: {value} leaked: {text}"
            );
        }
    }
    // A non-sensitive field is left alone.
    let reply = serde_json::json!({"status": "passed", "scan": [{"kind": "form", "pairs": [["name", "plain-name-1"]]}]});
    let feature = format!(
        "{}    Then the echo should fail quoting \"plain-name-1\"\n",
        answers(&reply)
    );
    let dir = project("scan-plain", &feature, ECHO_GROUP);
    assert!(stdout_of(&run(&dir)).contains("the echo quoted plain-name-1"));
}

#[test]
fn a_not_yet_error_quoting_a_scanned_value_is_masked_and_holds_no_dump() {
    let reply = serde_json::json!({
        "status": "not_yet",
        "error": "waiting for scan-attempt-1",
        "scan": [{"kind": "form", "pairs": [["password", "scan-attempt-1"]]}],
        "diagnostics": [{"title": "state", "kind": "text", "content": "dump-text-1"}],
    });
    let feature = format!(
        "Feature: f\n  Scenario: s\n    When I expect the next assertion to pass within \"1\" seconds, checking every \"20\" milliseconds\n    Then the echo answers:\n      \"\"\"\n      {reply}\n      \"\"\"\n"
    );
    let dir = project("scan-not-yet", &feature, ECHO_GROUP);
    let out = run_with(&dir, &["--events", "events.ndjson", "--evidence"], &[]);
    let text = stdout_of(&out);
    let stream = read_stream(&dir);
    let attempts: Vec<_> = stream.iter().filter(|e| e["type"] == "attempt").collect();
    assert!(attempts.len() > 1, "polling replayed the step");
    for attempt in &attempts {
        assert_eq!(attempt["message"], "waiting for ***", "{attempt}");
    }
    // The timeout carries the mismatch once, and the last reply's dump once.
    assert_eq!(text.matches("dump-text-1").count(), 1, "{text}");
    assert!(!text.contains("scan-attempt-1"), "{text}");
    // Each attempt's diagnostics are emitted before its `attempt` event.
    let types: Vec<_> = stream.iter().map(|e| e["type"].as_str().unwrap()).collect();
    let first_evidence = types.iter().position(|t| *t == "evidence").unwrap();
    let first_attempt = types.iter().position(|t| *t == "attempt").unwrap();
    assert!(first_evidence < first_attempt, "{types:?}");
}

#[test]
fn a_failing_macro_shows_the_plugin_diagnostic_once() {
    let dir = macro_project(
        "structured-macro",
        "Feature: f\n  Scenario: s\n    When I fail through a macro\n",
        "- step: 'I fail through a macro'\n  do: [the echo should fail with json]\n",
    );
    let text = stdout_of(&run(&dir));
    assert_eq!(text.matches("--- response (json) ---").count(), 1, "{text}");
    assert!(text.contains("I fail through a macro"), "{text}");
    assert!(!text.contains("diag-refresh-token-1"), "{text}");
}

#[test]
fn a_text_diagnostic_names_its_own_resource_in_the_stream() {
    let reply = serde_json::json!({
        "status": "passed",
        "diagnostics": [{"title": "note", "kind": "text", "content": "x", "resource": "other"}],
    });
    let dir = project("structured-resource-text", &answers(&reply), ECHO_GROUP);
    assert_green(&run_with(
        &dir,
        &["--events", "events.ndjson", "--evidence"],
        &[],
    ));
    let stream = read_stream(&dir);
    let evidence = stream.iter().find(|e| e["type"] == "evidence").unwrap();
    assert_eq!(evidence["resource"], "other");
}
