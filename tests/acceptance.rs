mod common;

use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::routing::post;
use axum::{Json, Router};
use serde_json::{Value, json};

/// Gate 1: scenarios against the reference stub must be green.
// Multi-thread: the stub runs inside `tokio::spawn`, and the test blocks on
// `Command::output()`. On a single-threaded runtime the block prevents polling
// the server task — the port is bound, but connections aren't accepted, and
// requests hang until timeout. A separate worker thread fixes this.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn all_feature_files_pass_against_the_stub() {
    let base = common::spawn().await;

    let exe = env!("CARGO_BIN_EXE_bddkit");
    let out = Command::new(exe)
        .args(["run", "--config", "tests/acceptance.yaml"])
        .env("BDDKIT_STUB_URL", &base)
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "run must be green\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(stdout.contains("failed: 0"), "{stdout}");
}

async fn spawn_eventual_post_stub(ready_on: Option<usize>) -> (String, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let handler_calls = calls.clone();
    let app = Router::new().route(
        "/eventual",
        post(move |body: String| {
            let handler_calls = handler_calls.clone();
            async move {
                let call = handler_calls.fetch_add(1, Ordering::SeqCst) + 1;
                let state = if ready_on.is_some_and(|ready| call >= ready) {
                    "ready"
                } else {
                    "pending"
                };
                let payload: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                Json(json!({"state": state, "payload": payload}))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind eventual stub");
    let address = listener.local_addr().expect("eventual stub address");
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve eventual stub");
    });
    (format!("http://{address}/"), calls)
}

fn write_eventual_post_project(base: &str, name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("bddkit-{name}-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/eventual.feature"),
        r#"Feature: eventual POST response
  Scenario: replay the saved request until the response is ready
    Given the request body is:
      """
      {"request":"saved"}
      """
    When I request "/eventual" using HTTP POST
    And I expect the next assertion to pass within "1" seconds, checking every "25" milliseconds
    Then the response body equals JSON:
      """
      {"state":"ready","payload":{"request":"saved"}}
      """
"#,
    )
    .expect("write eventual feature");
    let config = dir.join("cfg.yaml");
    std::fs::write(
        &config,
        format!(
            "paths: [{}]\nresources:\n  api:\n    stub:\n      base_url: {base}\n",
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("write eventual config");
    config
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn eventual_post_response_replays_the_saved_method_and_body() {
    let (base, calls) = spawn_eventual_post_stub(Some(2)).await;
    let config = write_eventual_post_project(&base, "eventual-post-success");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "run",
            "--config",
            config.to_str().expect("UTF-8 config path"),
        ])
        .output()
        .expect("run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "eventual POST must pass after one replay\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

async fn spawn_eventual_absence_stub(ready_on: usize) -> String {
    let calls = Arc::new(AtomicUsize::new(0));
    let app = Router::new().route(
        "/eventual-absence",
        post(move |_body: String| {
            let calls = calls.clone();
            async move {
                let call = calls.fetch_add(1, Ordering::SeqCst) + 1;
                if call >= ready_on {
                    Json(json!({"state": "ready"}))
                } else {
                    Json(json!({"state": "pending", "pendingReason": "not ready yet"}))
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind eventual-absence stub");
    let address = listener
        .local_addr()
        .expect("eventual-absence stub address");
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve eventual-absence stub");
    });
    format!("http://{address}/")
}

fn write_eventual_absence_project(base: &str, name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("bddkit-{name}-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/eventual.feature"),
        r#"Feature: eventual absence
  Scenario: poll until a node disappears
    When I request "/eventual-absence" using HTTP POST
    And I expect the next assertion to pass within "1" seconds, checking every "25" milliseconds
    Then the JSON node "pendingReason" should not exist
"#,
    )
    .expect("write eventual feature");
    let config = dir.join("cfg.yaml");
    std::fs::write(
        &config,
        format!(
            "paths: [{}]\nresources:\n  api:\n    stub:\n      base_url: {base}\n",
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("write eventual config");
    config
}

/// Issue #22, item 7: "wait for absence" must be explicitly validated, not just
/// assumed to work by composition with the existing polling mechanism.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn eventual_absence_polls_until_the_node_disappears() {
    let base = spawn_eventual_absence_stub(2).await;
    let config = write_eventual_absence_project(&base, "eventual-absence-success");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "run",
            "--config",
            config.to_str().expect("UTF-8 config path"),
        ])
        .output()
        .expect("run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "polling for absence must pass once the node disappears\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn eventual_post_timeout_reports_last_mismatch_and_final_exchange() {
    let (base, _calls) = spawn_eventual_post_stub(None).await;
    let config = write_eventual_post_project(&base, "eventual-post-timeout");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "run",
            "--config",
            config.to_str().expect("UTF-8 config path"),
        ])
        .output()
        .expect("run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("did not pass within 1s"), "{stdout}");
    assert!(
        stdout.contains("root.state")
            && stdout.contains("expected: \"ready\"")
            && stdout.contains("actual:   \"pending\""),
        "{stdout}"
    );
    assert!(
        stdout.contains("POST http://")
            && stdout.contains("/eventual")
            && stdout.contains("← 200")
            && stdout.contains(r#"{"request":"saved"}"#)
            && stdout.contains(r#""state":"pending""#),
        "{stdout}"
    );
}

/// An unknown step must fail BEFORE the first request, with code 2.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_step_fails_before_running() {
    let base = common::spawn().await;
    let dir = std::env::temp_dir().join("bddkit-validate-test");
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/bad.feature"),
        "Feature: f\n  Scenario: s\n    When I refund the order\n",
    )
    .expect("write feature");
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "paths: [{}]\nresources:\n  api:\n    stub:\n      base_url: {base}\n",
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("write config");

    let exe = env!("CARGO_BIN_EXE_bddkit");
    let out = Command::new(exe)
        .args([
            "run",
            "--config",
            dir.join("cfg.yaml").to_str().expect("path is UTF-8"),
        ])
        .output()
        .expect("failed to run bddkit");

    assert_eq!(out.status.code(), Some(2), "static-check exit code");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("run not started"), "{stderr}");
    assert!(stderr.contains("I refund the order"), "{stderr}");
}

/// A `<<variable>>` in a typed position is checked once its value exists:
/// after interpolation, before dispatch. The wrong type fails the STEP (exit 1),
/// never the validation — nothing about the text was wrong.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_variable_of_the_wrong_type_fails_the_step() {
    let base = common::spawn().await;
    let dir = std::env::temp_dir().join(format!("bddkit-typed-var-test-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/typed.feature"),
        "Feature: f\n  Scenario: s\n    Given set variable \"code\" to \"abc\"\n    When I request \"/ping\"\n    Then the response code is <<code>>\n",
    )
    .expect("write feature");
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "paths: [{}]\nresources:\n  api:\n    stub:\n      base_url: {base}\n",
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("write config");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "run",
            "--config",
            dir.join("cfg.yaml").to_str().expect("path is UTF-8"),
        ])
        .output()
        .expect("failed to run bddkit");

    assert_eq!(
        out.status.code(),
        Some(1),
        "a failed step, not a failed validation"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(r#"parameter <code> expects uint, got "abc""#),
        "{stdout}"
    );
}

/// The same check guards a macro's typed parameter: the value is refused
/// before the macro body runs, and it fails the calling step (exit 1).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_variable_of_the_wrong_type_fails_a_typed_macro_parameter() {
    let base = common::spawn().await;
    let dir = std::env::temp_dir().join(format!("bddkit-typed-macro-test-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/typed.feature"),
        "Feature: f\n  Scenario: s\n    Given set variable \"code\" to \"abc\"\n    Then the \"/ping\" page answers <<code>>\n",
    )
    .expect("write feature");
    std::fs::write(
        dir.join("macros.yaml"),
        "- step: 'the \"{path}\" page answers {code:uint}'\n  do:\n    - I request \"<<path>>\"\n    - the response code is <<code>>\n",
    )
    .expect("write macros");
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "macro_paths: [{}]\npaths: [{}]\nresources:\n  api:\n    stub:\n      base_url: {base}\n",
            dir.join("macros.yaml")
                .display()
                .to_string()
                .replace('\\', "/"),
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("write config");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "run",
            "--config",
            dir.join("cfg.yaml").to_str().expect("path is UTF-8"),
        ])
        .output()
        .expect("failed to run bddkit");

    assert_eq!(
        out.status.code(),
        Some(1),
        "a failed step, not a failed validation"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(r#"parameter <code> expects uint, got "abc""#),
        "{stdout}"
    );
    // Refused before the body: the macro's request was never sent.
    assert!(!stdout.contains("GET "), "{stdout}");
}

/// `resources.api` may be absent entirely — legal for a scenario that makes
/// no HTTP requests (symmetric to `resources.db`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_config_without_any_api_resource_runs_a_non_http_scenario() {
    let dir = std::env::temp_dir().join("bddkit-no-api-ok-test");
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/vars.feature"),
        "Feature: f\n  Scenario: s\n    \
         Given set variable \"x\" to \"1\"\n    Then variable \"x\" should be equal to \"1\"\n",
    )
    .expect("write feature");
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "paths: [{}]\nresources:\n  api: {{}}\n",
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("write config");

    let exe = env!("CARGO_BIN_EXE_bddkit");
    let out = Command::new(exe)
        .args([
            "run",
            "--config",
            dir.join("cfg.yaml").to_str().expect("path is UTF-8"),
        ])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "a config with no API resource must still run if the scenario does not touch HTTP\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
}

/// If no APIs are declared but the scenario sends a request anyway — the
/// failure happens as an ordinary step failure on first use, not a panic
/// and not a startup rejection.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_config_without_any_api_resource_fails_at_first_http_step() {
    let dir = std::env::temp_dir().join("bddkit-no-api-fail-test");
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/http.feature"),
        "Feature: f\n  Scenario: s\n    \
         When I request \"/ping\"\n    Then the response code is 200\n",
    )
    .expect("write feature");
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "paths: [{}]\nresources:\n  api: {{}}\n",
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("write config");

    let exe = env!("CARGO_BIN_EXE_bddkit");
    let out = Command::new(exe)
        .args([
            "run",
            "--config",
            dir.join("cfg.yaml").to_str().expect("path is UTF-8"),
        ])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!out.status.success(), "scenario must fail\n{stdout}");
    assert!(stdout.contains("resources.api"), "{stdout}");
}

/// Shared helper: writes two tagged feature files and a config pointing at
/// the stub into a temp directory, returns the config path.
fn write_tagged_project(base: &str, name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("bddkit-{name}-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/smoke.feature"),
        "Feature: smoke\n  @smoke\n  Scenario: ping\n    When I request \"/ping\"\n    Then the response code is 200\n",
    )
    .expect("write smoke feature");
    std::fs::write(
        dir.join("features/slow.feature"),
        "Feature: slow\n  @slow\n  Scenario: ping slowly\n    When I request \"/ping\"\n    Then the response code is 200\n",
    )
    .expect("write slow feature");
    let cfg = dir.join("cfg.yaml");
    std::fs::write(
        &cfg,
        format!(
            "paths: [{}]\nresources:\n  api:\n    stub:\n      base_url: {base}\n",
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("write config");
    cfg
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tag_filter_runs_only_the_matching_scenarios() {
    let base = common::spawn().await;
    let cfg = write_tagged_project(&base, "tagfilter");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "run",
            "--config",
            cfg.to_str().expect("path is UTF-8"),
            "--tag",
            "smoke",
        ])
        .output()
        .expect("failed to run bddkit");

    // Scenario names print only on failure, so the selection is visible via
    // file names and the final counters.
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("smoke.feature"),
        "the selected scenario must run:\n{stdout}"
    );
    assert!(
        !stdout.contains("slow.feature"),
        "a scenario without the tag must not run:\n{stdout}"
    );
    assert!(
        stdout.contains("files: 1, scenarios: 1, failed: 0"),
        "exactly one scenario must pass:\n{stdout}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tag_matching_nothing_fails_with_exit_code_two() {
    let base = common::spawn().await;
    let cfg = write_tagged_project(&base, "tagempty");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "run",
            "--config",
            cfg.to_str().expect("path is UTF-8"),
            "--tag",
            "absent",
        ])
        .output()
        .expect("failed to run bddkit");

    assert_eq!(
        out.status.code(),
        Some(2),
        "an empty selection is not a green run"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("no scenario selected"), "{stderr}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_positional_path_overrides_the_config_paths() {
    let base = common::spawn().await;
    let cfg = write_tagged_project(&base, "positional");
    let only = cfg
        .parent()
        .expect("parent directory")
        .join("features/smoke.feature");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "run",
            "--config",
            cfg.to_str().expect("path is UTF-8"),
            only.to_str().expect("path is UTF-8"),
        ])
        .output()
        .expect("failed to run bddkit");

    // The counter is required: without it the assertion would pass even if
    // the positional path were completely ignored (the config's `paths` gives two files).
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("slow.feature"),
        "a file outside the given path must not run:\n{stdout}"
    );
    assert!(
        stdout.contains("files: 1, scenarios: 1, failed: 0"),
        "exactly one file from the given path must pass:\n{stdout}"
    );
}

/// Gate M4: one scenario reaches two different APIs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_scenario_can_call_two_different_apis() {
    let primary = common::spawn().await;
    let secondary = common::spawn_secondary().await;

    let dir = std::env::temp_dir().join(format!("bddkit-two-apis-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/switch.feature"),
        r#"Feature: switching between APIs
  Scenario: the request goes to the selected API, the previous response survives the switch
    When I request "/ping"
    Then the response body contains JSON:
      """
      {"version": 3}
      """
    When I use "secondary" api
    Then the response body contains JSON:
      """
      {"version": 3}
      """
    When I request "/ping"
    Then the response body contains JSON:
      """
      {"source": "secondary"}
      """
"#,
    )
    .expect("write feature");
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "paths: [{}]\ndefault_api: primary\nresources:\n  api:\n    primary:\n      base_url: {primary}\n    secondary:\n      base_url: {secondary}\n",
            dir.join("features").display().to_string().replace('\\', "/")
        ),
    )
    .expect("write config");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "run",
            "--config",
            dir.join("cfg.yaml").to_str().expect("path is UTF-8"),
        ])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "a scenario with two APIs must be green\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    // Scenario names print only on failure, so the counters are the only
    // proof that the gate actually ran something.
    assert!(
        stdout.contains("files: 1, scenarios: 1, failed: 0"),
        "exactly one scenario must pass:\n{stdout}"
    );
}

/// Gate: API switch inside an include persists after the include returns.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_api_switch_inside_an_include_stays_switched_after_it_returns() {
    let primary = common::spawn().await;
    let secondary = common::spawn_secondary().await;

    let dir =
        std::env::temp_dir().join(format!("bddkit-include-api-switch-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features").join("include")).expect("mkdir");

    // Included scenario: switches to secondary API and makes a request.
    std::fs::write(
        dir.join("features/include/state.feature"),
        r#"Feature: Switches API inside an include
  Scenario: Switches to a secondary API
    Given I use "secondary" api
    When I request "/ping"
    Then the response body contains JSON:
      """
      {"source": "secondary"}
      """
"#,
    )
    .expect("write state.feature");

    // Caller scenario: includes the state.feature, then makes another request.
    // The second request should still go to secondary (not revert to default).
    std::fs::write(
        dir.join("features/caller.feature"),
        r#"Feature: Caller verifies API switch persists
  Scenario: API switch inside include persists after include returns
    Given I include "include/state.feature"
    When I request "/ping"
    Then the response body contains JSON:
      """
      {"source": "secondary"}
      """
"#,
    )
    .expect("write caller.feature");

    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "paths: [{}]\ndefault_api: primary\nresources:\n  api:\n    primary:\n      base_url: {primary}\n    secondary:\n      base_url: {secondary}\n",
            dir.join("features").display().to_string().replace('\\', "/")
        ),
    )
    .expect("write config");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "run",
            "--config",
            dir.join("cfg.yaml").to_str().expect("path is UTF-8"),
        ])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "a scenario that includes another and maintains API switch must be green\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    // Two files: the included state.feature (which switches API and makes a request),
    // and the caller.feature (which includes state.feature and makes another request).
    // Both must pass with failed: 0 to prove the API switch persists through the include.
    assert!(
        stdout.contains("files: 2, scenarios: 2, failed: 0"),
        "two scenarios in two files must pass:\n{stdout}"
    );
}

#[test]
fn macro_cycle_fails_validation_with_exit_code_two() {
    let dir = std::env::temp_dir().join(format!("bddkit-cycle-test-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/cycle.feature"),
        "Feature: f\n  Scenario: s\n    When I do first\n",
    )
    .expect("write feature");
    std::fs::write(
        dir.join("macros.yaml"),
        "- step: I do first\n  do: [I do second]\n- step: I do second\n  do: [I do first]\n",
    )
    .expect("write macros");
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "macro_paths: [{}]\npaths: [{}]\nresources:\n  api:\n    stub:\n      base_url: http://example.test\n",
            dir.join("macros.yaml")
                .display()
                .to_string()
                .replace('\\', "/"),
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("write config");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "run",
            "--config",
            dir.join("cfg.yaml").to_str().expect("path is UTF-8"),
        ])
        .output()
        .expect("failed to run bddkit");

    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("cycle in macros"), "{stderr}");
    assert!(stderr.contains("run not started"), "{stderr}");
}

/// `(`, `)` and `/` in a macro template became Cucumber Expression syntax
/// (#66). A template written for the old literal meaning must fail LOUDLY:
/// the caller's step text can no longer match, so validation stops the run.
#[test]
fn a_macro_template_with_literal_parentheses_no_longer_matches_its_old_text() {
    let dir = std::env::temp_dir().join(format!("bddkit-macro-syntax-test-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/old.feature"),
        "Feature: f\n  Scenario: s\n    When I do (setup) things\n",
    )
    .expect("write feature");
    std::fs::write(
        dir.join("macros.yaml"),
        "- step: I do (setup) things\n  do: [Show all variables]\n",
    )
    .expect("write macros");
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "macro_paths: [{}]\npaths: [{}]\nresources:\n  api:\n    stub:\n      base_url: http://example.test\n",
            dir.join("macros.yaml").display().to_string().replace('\\', "/"),
            dir.join("features").display().to_string().replace('\\', "/")
        ),
    )
    .expect("write config");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "run",
            "--config",
            dir.join("cfg.yaml").to_str().expect("path is UTF-8"),
        ])
        .output()
        .expect("failed to run bddkit");

    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("unknown step"), "{stderr}");
    assert!(stderr.contains("run not started"), "{stderr}");
}

/// `Print table:` fills `<<variables>>` in every cell, the header row too,
/// and aligns the columns on the values after that substitution.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn print_table_aligns_interpolated_cells_on_stderr() {
    let base = common::spawn().await;
    let dir = std::env::temp_dir().join(format!("bddkit-print-table-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/table.feature"),
        "Feature: f\n  Scenario: s\n    Given set variable \"id\" to \"ord-7f3a91\"\n    And set variable \"col\" to \"статус\"\n    Then Print table:\n      | order | <<col>> |\n      | <<id>> | оплачен |\n",
    )
    .expect("write feature");
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "paths: [{}]\nresources:\n  api:\n    stub:\n      base_url: {base}\n",
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("write config");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "run",
            "--config",
            dir.join("cfg.yaml").to_str().expect("path is UTF-8"),
        ])
        .output()
        .expect("failed to run bddkit");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(
        stderr.contains("| order      | статус  |\n| ord-7f3a91 | оплачен |\n"),
        "{stderr}"
    );
}

/// `Print response body as "<path>"` cannot work without structure: for
/// text/plain this is an explicit error, not a silent degradation.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn print_body_as_path_fails_for_plain_content_type() {
    let base = common::spawn().await;
    let dir = std::env::temp_dir().join(format!("bddkit-debug-plain-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/plain.feature"),
        "Feature: f\n  Scenario: s\n    When I request \"/plain\"\n    Then Print response body as \"x\"\n",
    )
    .expect("write feature");
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "paths: [{}]\nresources:\n  api:\n    stub:\n      base_url: {base}\n",
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("write config");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "run",
            "--config",
            dir.join("cfg.yaml").to_str().expect("path is UTF-8"),
        ])
        .output()
        .expect("failed to run bddkit");

    assert_eq!(
        out.status.code(),
        Some(1),
        "scenario must fail, not fail validation"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("is not supported"),
        "expected a message about the unsupported content-type:\n{stdout}"
    );
}

/// The bug this fixes: `Print response body as "<selector>"` used to route
/// HTML through the XML-only XPath engine and fail on anything but strict,
/// fully-closed, fully-quoted XHTML. It must now succeed on real HTML.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn print_body_as_path_succeeds_on_real_world_html() {
    let base = common::spawn().await;
    let dir = std::env::temp_dir().join(format!("bddkit-debug-html-loose-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/html_loose.feature"),
        "Feature: f\n  Scenario: s\n    When I request \"/html-loose\"\n    Then Print response body as \"div.box\"\n",
    )
    .expect("write feature");
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "paths: [{}]\nresources:\n  api:\n    stub:\n      base_url: {base}\n",
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("write config");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "run",
            "--config",
            dir.join("cfg.yaml").to_str().expect("path is UTF-8"),
        ])
        .output()
        .expect("failed to run bddkit");

    assert!(
        out.status.success(),
        "expected success, got: {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("Loose"), "{stderr}");
}

/// Prepares two feature files that write the same variable with different
/// values BEFORE the barrier and check it AFTER. The barrier guarantees that
/// both files have written before either reads: a run-wide `VarStack` would
/// fail this deterministically, not occasionally.
fn write_parallel_fixture(
    dir: &std::path::Path,
    base: &str,
    concurrency: usize,
    tags: &str,
) -> std::path::PathBuf {
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    for who in ["a", "b"] {
        std::fs::write(
            dir.join(format!("features/{who}.feature")),
            format!(
                "{tags}Feature: parallel {who}\n  Scenario: both tasks meet at the barrier\n    \
                 Given set variable \"who\" to \"{who}\"\n    \
                 When I request \"/barrier\"\n    \
                 Then the response code is 200\n    \
                 And variable \"who\" should be equal to \"{who}\"\n"
            ),
        )
        .expect("write feature");
    }
    let cfg = dir.join("cfg.yaml");
    std::fs::write(
        &cfg,
        format!(
            "concurrency: {concurrency}\npaths: [{}]\nresources:\n  api:\n    stub:\n      \
             base_url: {base}\n      timeout_secs: 2\n",
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("write config");
    cfg
}

/// Gate M5: two files must run at the same time, or the barrier never opens.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_feature_files_run_at_the_same_time() {
    let base = common::spawn_barrier(2).await;
    let dir = std::env::temp_dir().join(format!("bddkit-parallel-{}", std::process::id()));
    let cfg = write_parallel_fixture(&dir, &base, 2, "");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config", cfg.to_str().expect("path is UTF-8")])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "both files must run in parallel\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(stdout.contains("failed: 0"), "{stdout}");
    assert!(stdout.contains("files: 2"), "{stdout}");
}

/// Negative control: with `concurrency: 1` the barrier never opens and the
/// run fails. Without this test, the first one could pass green for any
/// reason other than real parallelism.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_single_worker_cannot_release_the_barrier() {
    let base = common::spawn_barrier(2).await;
    let dir = std::env::temp_dir().join(format!("bddkit-sequential-{}", std::process::id()));
    let cfg = write_parallel_fixture(&dir, &base, 1, "");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config", cfg.to_str().expect("path is UTF-8")])
        .output()
        .expect("failed to run bddkit");

    assert_eq!(
        out.status.code(),
        Some(1),
        "a single worker must hit the request timeout, not pass"
    );
}

/// Files in one chain must never be in flight at the same time — the
/// two-party barrier never opens, and the run must fail on timeout. This
/// mirrors the parallelism test: the same pair of files, just tagged.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_files_in_one_serial_chain_never_run_together() {
    let base = common::spawn_barrier(2).await;
    let dir = std::env::temp_dir().join(format!("bddkit-serial-{}", std::process::id()));
    let cfg = write_parallel_fixture(&dir, &base, 2, "@serial(shared)\n");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config", cfg.to_str().expect("path is UTF-8")])
        .output()
        .expect("failed to run bddkit");

    assert_eq!(
        out.status.code(),
        Some(1),
        "the chain must serialize the files, the two-party barrier must not open\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// A broken scheduling tag — rejected before the first request, code 2.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_file_in_two_chains_fails_the_startup() {
    let dir = std::env::temp_dir().join(format!("bddkit-two-chains-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/bad.feature"),
        "@serial(a)\nFeature: f\n  @serial(b)\n  Scenario: s\n    Given set variable \"x\" to \"1\"\n",
    )
    .expect("write feature");
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "paths: [{}]\nresources:\n  api: {{}}\n",
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("write config");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "run",
            "--config",
            dir.join("cfg.yaml").to_str().expect("path is UTF-8"),
        ])
        .output()
        .expect("failed to run bddkit");

    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("two chains"), "{stderr}");
}

/// An unreachable DB — rejected BEFORE the first request, so code 2, not 1.
/// Invariant 6: 1 is reserved for a failed scenario.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unreachable_database_fails_the_startup_with_code_two() {
    let dir = std::env::temp_dir().join(format!("bddkit-startup-exit-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/vars.feature"),
        "Feature: f\n  Scenario: s\n    Given set variable \"x\" to \"1\"\n",
    )
    .expect("write feature");
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "paths: [{}]\nresources:\n  api: {{}}\n  db:\n    main:\n      \
             dsn: postgres://nobody:nobody@127.0.0.1:1/nothing\n",
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("write config");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "run",
            "--config",
            dir.join("cfg.yaml").to_str().expect("path is UTF-8"),
        ])
        .output()
        .expect("failed to run bddkit");

    assert_eq!(
        out.status.code(),
        Some(2),
        "failing before the first request must give 2\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("run not started"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The order files print in with one worker is the queue order.
/// Checking that `@priority` sets it: higher goes earlier.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn higher_priority_files_run_first() {
    let dir = std::env::temp_dir().join(format!("bddkit-priority-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    // Names are deliberately alphabetical in the reverse of the desired order:
    // without the tag the queue would be low → mid → high.
    for (name, tag) in [
        ("a_low", "@priority(-1)\n"),
        ("b_mid", ""),
        ("c_high", "@priority(5)\n"),
    ] {
        std::fs::write(
            dir.join(format!("features/{name}.feature")),
            format!("{tag}Feature: {name}\n  Scenario: s\n    Given set variable \"x\" to \"1\"\n"),
        )
        .expect("write feature");
    }
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "concurrency: 1\npaths: [{}]\nresources:\n  api: {{}}\n",
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("write config");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "run",
            "--config",
            dir.join("cfg.yaml").to_str().expect("path is UTF-8"),
        ])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let high = stdout.find("c_high").expect("file c_high in the output");
    let mid = stdout.find("b_mid").expect("file b_mid in the output");
    let low = stdout.find("a_low").expect("file a_low in the output");
    assert!(
        high < mid && mid < low,
        "queue order by priority:\n{stdout}"
    );
}

/// A non-numeric priority — rejected before the first request, code 2.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_non_numeric_priority_fails_the_startup() {
    let dir = std::env::temp_dir().join(format!("bddkit-bad-priority-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/bad.feature"),
        "@priority(urgent)\nFeature: f\n  Scenario: s\n    Given set variable \"x\" to \"1\"\n",
    )
    .expect("write feature");
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "paths: [{}]\nresources:\n  api: {{}}\n",
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("write config");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "run",
            "--config",
            dir.join("cfg.yaml").to_str().expect("path is UTF-8"),
        ])
        .output()
        .expect("failed to run bddkit");

    assert_eq!(out.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("an integer"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `--fail-fast` stops dispatching NEW work after the first failure.
/// With one worker this is deterministic: the first file fails, the second
/// and third never start at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fail_fast_stops_starting_new_files() {
    let dir = std::env::temp_dir().join(format!("bddkit-fail-fast-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/a_broken.feature"),
        "Feature: broken\n  Scenario: s\n    \
         Given set variable \"x\" to \"1\"\n    Then variable \"x\" should be equal to \"2\"\n",
    )
    .expect("write feature");
    for name in ["b_ok", "c_ok"] {
        std::fs::write(
            dir.join(format!("features/{name}.feature")),
            format!("Feature: {name}\n  Scenario: s\n    Given set variable \"x\" to \"1\"\n"),
        )
        .expect("write feature");
    }
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "concurrency: 1\npaths: [{}]\nresources:\n  api: {{}}\n",
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("write config");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "run",
            "--config",
            dir.join("cfg.yaml").to_str().expect("path is UTF-8"),
            "--fail-fast",
        ])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(
        stdout.contains("a_broken"),
        "the failed file must appear in the report:\n{stdout}"
    );
    assert!(
        !stdout.contains("c_ok"),
        "no new files must start after the failure:\n{stdout}"
    );
    assert!(stdout.contains("files: 1"), "{stdout}");
}

/// `--fail-fast` also stops dispatching new work WITHIN a chain already
/// picked up: three files sharing one `@serial` name run strictly in order
/// on one worker, the first fails, the second and third never start. This is
/// a separate check from `fail_fast_stops_starting_new_files`, which only
/// exercises the "before the next chain" check — there each file was its own chain.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fail_fast_stops_a_chain_partway_through() {
    let dir = std::env::temp_dir().join(format!("bddkit-fail-fast-chain-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/a_broken.feature"),
        "@serial(chain)\nFeature: broken\n  Scenario: s\n    \
         Given set variable \"x\" to \"1\"\n    Then variable \"x\" should be equal to \"2\"\n",
    )
    .expect("write feature");
    for name in ["b_ok", "c_ok"] {
        std::fs::write(
            dir.join(format!("features/{name}.feature")),
            format!(
                "@serial(chain)\nFeature: {name}\n  Scenario: s\n    Given set variable \"x\" to \"1\"\n"
            ),
        )
        .expect("write feature");
    }
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "concurrency: 1\npaths: [{}]\nresources:\n  api: {{}}\n",
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("write config");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "run",
            "--config",
            dir.join("cfg.yaml").to_str().expect("path is UTF-8"),
            "--fail-fast",
        ])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(
        stdout.contains("a_broken"),
        "the failed file must appear in the report:\n{stdout}"
    );
    assert!(
        !stdout.contains("b_ok") && !stdout.contains("c_ok"),
        "the rest of the chain must not start after the failure:\n{stdout}"
    );
    assert!(stdout.contains("files: 1"), "{stdout}");
}

/// Without the flag, one file's failure does not block the rest: the run
/// must reach the end and show all failures at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_fail_fast_every_file_still_runs() {
    let dir = std::env::temp_dir().join(format!("bddkit-no-fail-fast-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/a_broken.feature"),
        "Feature: broken\n  Scenario: s\n    \
         Given set variable \"x\" to \"1\"\n    Then variable \"x\" should be equal to \"2\"\n",
    )
    .expect("write feature");
    std::fs::write(
        dir.join("features/c_ok.feature"),
        "Feature: whole\n  Scenario: s\n    Given set variable \"x\" to \"1\"\n",
    )
    .expect("write feature");
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "concurrency: 1\npaths: [{}]\nresources:\n  api: {{}}\n",
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("write config");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "run",
            "--config",
            dir.join("cfg.yaml").to_str().expect("path is UTF-8"),
        ])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("files: 2"), "{stdout}");
}

/// `doctor` reaches every check a run makes before its first request, and a
/// bare invocation opens no socket at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doctor_reports_a_healthy_suite_and_exits_zero() {
    let base = common::spawn().await;

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config", "tests/acceptance.yaml"])
        .env("BDDKIT_STUB_URL", &base)
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(0),
        "a sound suite is clean\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(stdout.contains("APP_ENV:"), "{stdout}");
    assert!(
        stdout.contains("--live"),
        "a static run must say what it did not do:\n{stdout}"
    );
}

/// Writes a project whose config points at `base`, returning the config path.
/// An empty `base` declares no API at all, which is how a test makes the exit
/// code come from somewhere else. `feature` is the whole `.feature` file so a
/// caller can plant a typo or a tag; an empty one writes no feature file at
/// all. `extra` is appended after the API block — indented, it adds to
/// `resources:`; at column 0 it adds a top-level key.
fn write_doctor_project(name: &str, base: &str, feature: &str, extra: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("bddkit-{name}-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    let only = dir.join("features/only.feature");
    if feature.is_empty() {
        // A previous run of this test binary may have left one behind.
        let _ = std::fs::remove_file(&only);
    } else {
        std::fs::write(&only, feature).expect("write feature");
    }
    let api = if base.is_empty() {
        "  api: {}\n".to_string()
    } else {
        format!("  api:\n    stub:\n      base_url: {base}\n")
    };
    let cfg = dir.join("cfg.yaml");
    std::fs::write(
        &cfg,
        format!(
            "paths: [{}]\nresources:\n{api}{extra}",
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("write config");
    cfg
}

#[test]
fn doctor_names_the_file_and_line_of_an_undefined_step() {
    let cfg = write_doctor_project(
        "doctor-step",
        "http://127.0.0.1:1/",
        "Feature: only\n  Scenario: one\n    When I frobnicate\n",
        "",
    );

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config", cfg.to_str().expect("path is UTF-8")])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("only.feature:3"), "{stdout}");
    assert!(stdout.contains("I frobnicate"), "{stdout}");
}

/// A step whose resource kind has nothing declared is a guaranteed failure in
/// the scenario, and `run` does not refuse the config — a `--tag` may keep the
/// scenario out. `doctor` applies no filter, so it is the one place to say so.
#[test]
fn doctor_reports_a_step_whose_resource_kind_declares_nothing() {
    let cfg = write_doctor_project(
        "doctor-unserved",
        "",
        "Feature: only\n  Scenario: one\n    When I request \"/ping\"\n    When I request \"/pong\"\n",
        "",
    );

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config", cfg.to_str().expect("path is UTF-8")])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("only.feature:3"), "{stdout}");
    assert!(
        !stdout.contains("only.feature:4"),
        "once per file: {stdout}"
    );
    assert!(stdout.contains("resources.api declares none"), "{stdout}");
}

/// The promise the command is built on: `doctor` without `--live` must reach a
/// verdict on a train. The `base_url` here points at a closed port, and the
/// static run must still come back clean.
#[test]
fn doctor_without_live_leaves_an_unreachable_base_url_alone() {
    let cfg = write_doctor_project(
        "doctor-offline",
        "http://127.0.0.1:1/",
        "Feature: only\n  Scenario: one\n    When I request \"/ping\"\n",
        "  db:\n    primary:\n      dsn: postgres://u:p@127.0.0.1:1/x\n",
    );

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config", cfg.to_str().expect("path is UTF-8")])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(0),
        "a closed port is not a static problem\n{stdout}"
    );
    assert!(stdout.contains("live probe skipped"), "{stdout}");
}

#[test]
fn doctor_live_reports_an_unreachable_base_url() {
    let cfg = write_doctor_project(
        "doctor-live",
        "http://127.0.0.1:1/",
        "Feature: only\n  Scenario: one\n    When I request \"/ping\"\n",
        "",
    );

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "doctor",
            "--config",
            cfg.to_str().expect("path is UTF-8"),
            "--live",
        ])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("api stub"), "{stdout}");
    assert!(
        !stdout.contains("--live"),
        "the hint belongs to a static run only:\n{stdout}"
    );
}

/// `doctor` probes one connection at a time, so a suite with four of them
/// learns which one is dead — and, because the full-map `Db::connect` returns
/// at the first failure, so that a second dead DSN is still probed.
///
/// Asserted through `--json` and with no API declared: the static `db` row
/// carries the same name, and an API pointed at a closed port would supply the
/// exit code on its own, so a laxer test passes with the live probe deleted.
#[test]
fn doctor_live_reports_every_dead_connection_by_name() {
    let cfg = write_doctor_project(
        "doctor-dsn",
        "http://127.0.0.1:1/",
        "Feature: only\n  Scenario: one\n    When I request \"/ping\"\n",
        "  db:\n    primary:\n      dsn: postgres://u:p@127.0.0.1:1/x\n\
         \x20   secondary:\n      dsn: postgres://u:p@127.0.0.1:1/y\n\
         default_db: primary\n",
    );

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "doctor",
            "--config",
            cfg.to_str().expect("path is UTF-8"),
            "--live",
            "--json",
        ])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    let report: Value = serde_json::from_str(&stdout).expect("--json emits JSON only");
    let checks = report["checks"].as_array().expect("checks is an array");
    for name in ["primary", "secondary"] {
        assert!(
            checks.iter().any(|c| {
                c["stage"] == "db"
                    && c["target"] == name
                    && c["status"] == "failed"
                    && c["probe"] == true
            }),
            "the live probe of {name} must be reported failed:\n{stdout}"
        );
    }
}

/// Every declared SRP resource is validated at startup, not just the default
/// one. Otherwise a broken `variant:` in a second block sits there until
/// someone points `default_srp` at it — and `doctor`, which reports on every
/// declared resource, would be stricter than the run it is meant to predict.
#[test]
fn run_refuses_a_malformed_srp_resource_that_is_not_the_default() {
    let cfg = write_doctor_project(
        "run-srp",
        "http://127.0.0.1:1/",
        "Feature: only\n  Scenario: one\n    When I request \"/ping\"\n",
        "  srp:\n    good:\n      variant: hex-string\n    legacy:\n      variant: bogus\n\
         default_srp: good\n",
    );

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config", cfg.to_str().expect("path is UTF-8")])
        .output()
        .expect("failed to run bddkit");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(2),
        "a malformed resource is a startup failure, not a scenario failure\n{stderr}"
    );
    assert!(
        stderr.contains("legacy"),
        "the bad resource is named:\n{stderr}"
    );
}

/// A DSN is more than its scheme, and sqlx parses the whole URL before it
/// opens anything — so a typo past the `://` is a failure `run` reaches
/// offline. The invariant is the pairing, not either message: whatever `run`
/// refuses statically, a static `doctor` must refuse too.
#[test]
fn doctor_and_run_agree_that_a_malformed_dsn_is_a_startup_failure() {
    let cfg = write_doctor_project(
        "doctor-baddsn",
        "http://127.0.0.1:1/",
        "Feature: only\n  Scenario: one\n    When I request \"/ping\"\n",
        "  db:\n    primary:\n      dsn: \"postgres://u:p@127.0.0.1:notaport/x\"\n",
    );
    let path = cfg.to_str().expect("path is UTF-8");

    let doctor = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config", path])
        .output()
        .expect("failed to run bddkit");
    let stdout = String::from_utf8_lossy(&doctor.stdout);
    assert_eq!(
        doctor.status.code(),
        Some(1),
        "no socket is needed to see this\n{stdout}"
    );
    assert!(stdout.contains("db primary"), "{stdout}");

    let run = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config", path])
        .output()
        .expect("failed to run bddkit");
    assert_eq!(
        run.status.code(),
        Some(2),
        "the run never starts: {}",
        String::from_utf8_lossy(&run.stderr)
    );
}

/// A scheduling tag `run` refuses to parse is a "nothing ran" failure like any
/// other, so `doctor` has to see it. It is the one pre-run check that lives
/// past `validate::check`, in `runner::build_chains`.
#[test]
fn doctor_reports_a_malformed_scheduling_tag() {
    let cfg = write_doctor_project(
        "doctor-tag",
        "http://127.0.0.1:1/",
        "Feature: only\n  @priority(soon)\n  Scenario: one\n    When I request \"/ping\"\n",
        "",
    );

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config", cfg.to_str().expect("path is UTF-8")])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("@priority(soon)"), "{stdout}");
}

/// `run` exits 2 on a selection with nothing in it. A green tick reading
/// "0 file(s), every step matched" is the most misleading line the command
/// could print, because it certifies a suite that cannot run.
#[test]
fn doctor_reports_a_suite_with_no_scenario_to_run() {
    let cfg = write_doctor_project("doctor-empty", "", "", "");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config", cfg.to_str().expect("path is UTF-8")])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("no scenario"), "{stdout}");
}

/// The decision a script depends on: `doctor` answers 0 or 1 and never 2, so
/// even a config it cannot parse comes back as an ordinary finding. `run`
/// exits 2 for the same file.
#[test]
fn doctor_reports_an_unparseable_config_as_an_ordinary_finding() {
    let dir = std::env::temp_dir().join(format!("bddkit-doctor-broken-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let cfg = dir.join("cfg.yaml");
    std::fs::write(
        &cfg,
        "paths: [features]\nresources:\n  api:\n    a:\n      base_url: ${BDDKIT_ABSENT_VAR}\n",
    )
    .expect("write config");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config", cfg.to_str().expect("path is UTF-8")])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(1),
        "a broken config is a finding, not a different exit currency\n{stdout}"
    );
    assert!(stdout.contains("BDDKIT_ABSENT_VAR"), "{stdout}");
}

/// The primary caller is a script or an agent, which is what `--json` is for.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doctor_json_carries_the_env_the_status_and_every_check() {
    let base = common::spawn().await;

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config", "tests/acceptance.yaml", "--json"])
        .env("BDDKIT_STUB_URL", &base)
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let report: Value = serde_json::from_str(&stdout).expect("--json emits JSON only");
    assert_eq!(report["app_env"], "dev", "{stdout}");
    assert_eq!(report["live"], false, "{stdout}");
    assert_eq!(
        report["config_source"], "flag",
        "a structured field, not just the `(--config)` suffix on `config`: {stdout}"
    );
    let checks = report["checks"].as_array().expect("checks is an array");
    assert!(
        checks
            .iter()
            .any(|c| c["stage"] == "steps" && c["status"] == "ok"),
        "{stdout}"
    );
    assert!(
        checks
            .iter()
            .any(|c| c["stage"] == "api" && c["target"] == "stub" && c["status"] == "skipped"),
        "a static run reports the probe it did not make:\n{stdout}"
    );
}

/// The flat `bddkit --config x.yaml` form is gone: `steps` has to be a real
/// subcommand, and clap cannot have both a positional path list at the top
/// level and subcommands to disambiguate it against.
#[test]
fn the_run_subcommand_is_how_a_suite_is_started() {
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["--config", "tests/acceptance.yaml"])
        .output()
        .expect("run bddkit");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "the flat form is a usage error");
    assert!(
        stderr.contains("unexpected argument"),
        "the flat form must be refused by the parser, not started:\n{stderr}"
    );

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--help"])
        .output()
        .expect("run bddkit");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains("Usage: bddkit run"), "{stdout}");
    assert!(
        stdout.contains("--fail-fast"),
        "run keeps every flag the flat form had:\n{stdout}"
    );
}

/// The whole promise in one assertion: the file after equals the file before
/// plus exactly the inserted block. Reordered keys, a requoted scalar or a
/// swallowed final newline all fail here.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resource_add_inserts_exactly_the_block_and_probes_the_new_api() {
    let base = common::spawn().await;
    let cfg = write_doctor_project(
        "resource-add-ok",
        &base,
        "Feature: only\n  Scenario: one\n    When I request \"/ping\"\n",
        "default_api: stub\n",
    );
    let before = std::fs::read_to_string(&cfg).expect("read config");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "resource",
            "add",
            "api",
            "staging",
            "--config",
            cfg.to_str().expect("path is UTF-8"),
            "--base_url",
            &base,
            "--timeout_secs",
            "5",
        ])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stdout}\n{stderr}");

    let after = std::fs::read_to_string(&cfg).expect("read config");
    assert_eq!(
        after,
        before.replace(
            "  api:\n",
            &format!("  api:\n    staging:\n      base_url: {base}\n      timeout_secs: 5\n")
        )
    );
}

/// The probe is on by default here — the resource being added is what the
/// invocation is about — and a resource that cannot answer is not written.
#[test]
fn resource_add_writes_nothing_when_the_probe_fails() {
    let cfg = write_doctor_project(
        "resource-add-dead",
        "http://127.0.0.1:1/",
        "Feature: only\n  Scenario: one\n    When I request \"/ping\"\n",
        "default_api: stub\n",
    );
    let before = std::fs::read_to_string(&cfg).expect("read config");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "resource",
            "add",
            "api",
            "staging",
            "--config",
            cfg.to_str().expect("path is UTF-8"),
            "--base_url",
            "http://127.0.0.1:1/",
        ])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    // The resource's own line, not the word `base_url` — which the failure
    // message itself carries, so `contains("base_url")` proves nothing.
    assert!(
        stdout.contains("  base_url: http://127.0.0.1:1/\n"),
        "the block is printed: {stdout}"
    );
    assert_eq!(
        std::fs::read_to_string(&cfg).expect("read config"),
        before,
        "a failed add writes nothing"
    );
}

/// `--no-check` skips the probe and nothing else: the same closed port, and
/// the resource is written.
#[test]
fn resource_add_no_check_writes_a_resource_that_is_not_up_yet() {
    let cfg = write_doctor_project(
        "resource-add-nocheck",
        "http://127.0.0.1:1/",
        "Feature: only\n  Scenario: one\n    When I request \"/ping\"\n",
        "default_api: stub\n",
    );

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "resource",
            "add",
            "api",
            "staging",
            "--config",
            cfg.to_str().expect("path is UTF-8"),
            "--base_url",
            "http://127.0.0.1:1/",
            "--no-check",
        ])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(
        std::fs::read_to_string(&cfg)
            .expect("read config")
            .contains("staging:"),
        "the resource is written"
    );
}

/// Insert only, and the refusal is what a script reads: exit 1, the block on
/// stdout, the file untouched.
#[test]
fn resource_add_refuses_a_name_that_is_already_taken() {
    let cfg = write_doctor_project(
        "resource-add-taken",
        "http://127.0.0.1:1/",
        "Feature: only\n  Scenario: one\n    When I request \"/ping\"\n",
        "default_api: stub\n",
    );
    let before = std::fs::read_to_string(&cfg).expect("read config");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "resource",
            "add",
            "api",
            "stub",
            "--config",
            cfg.to_str().expect("path is UTF-8"),
            "--base_url",
            "http://other.local",
            "--no-check",
        ])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert_eq!(std::fs::read_to_string(&cfg).expect("read config"), before);
}

/// A typo in a field name is an error, not a silently stored key — the whole
/// reason the flags are checked against a list at all.
#[test]
fn resource_add_names_a_field_that_does_not_exist() {
    let cfg = write_doctor_project(
        "resource-add-typo",
        "http://127.0.0.1:1/",
        "Feature: only\n  Scenario: one\n    When I request \"/ping\"\n",
        "default_api: stub\n",
    );
    let before = std::fs::read_to_string(&cfg).expect("read config");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "resource",
            "add",
            "api",
            "staging",
            "--config",
            cfg.to_str().expect("path is UTF-8"),
            "--base_rul",
            "http://a.local",
            "--no-check",
        ])
        .output()
        .expect("failed to run bddkit");

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // Exactly 1: this command reports, so `not zero` would also accept a usage
    // error or a panic, which are the two answers it must never give.
    assert_eq!(out.status.code(), Some(1), "{combined}");
    assert!(combined.contains("base_rul"), "{combined}");
    assert_eq!(
        std::fs::read_to_string(&cfg).expect("read config"),
        before,
        "a refused field writes nothing"
    );
}

/// A resource name is written to the file literally and read back `${VAR}`-
/// expanded, so a name carrying a variable that is set is not in the loaded
/// config under the name being written. That is a refusal like any other —
/// looking it up by index made the process panic with exit 101.
#[test]
fn resource_add_refuses_a_name_that_expands_to_something_else() {
    let cfg = write_doctor_project(
        "resource-add-expanding-name",
        "http://127.0.0.1:1/",
        "Feature: only\n  Scenario: one\n    When I request \"/ping\"\n",
        "default_api: stub\n",
    );
    let before = std::fs::read_to_string(&cfg).expect("read config");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "resource",
            "add",
            "api",
            "${BDDKIT_TEST_NAME}",
            "--config",
            cfg.to_str().expect("path is UTF-8"),
            "--base_url",
            "http://a.local",
            "--no-check",
        ])
        .env("BDDKIT_TEST_NAME", "expanded")
        .output()
        .expect("failed to run bddkit");

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out.status.code(), Some(1), "{combined}");
    assert!(combined.contains("${VAR}"), "{combined}");
    assert_eq!(
        std::fs::read_to_string(&cfg).expect("read config"),
        before,
        "nothing was written"
    );
}

/// The block is the whole value of the failure path, so it has to be pasteable
/// where the reader will paste it: under `resources:`, which means carrying the
/// group key whenever the config does not have that group yet.
#[test]
fn resource_add_prints_the_group_key_for_a_group_the_config_lacks() {
    let cfg = write_doctor_project(
        "resource-add-new-group",
        "http://127.0.0.1:1/",
        "Feature: only\n  Scenario: one\n    When I request \"/ping\"\n",
        "default_api: stub\n",
    );

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "resource",
            "add",
            "db",
            "reporting",
            "--config",
            cfg.to_str().expect("path is UTF-8"),
            "--dsn",
            "nope://reporting",
            "--no-check",
        ])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(
        stdout.contains("db:\n  reporting:\n    dsn: nope://reporting\n"),
        "the block names the group it belongs under: {stdout}"
    );
}

/// Writes a healthy doctor project (see `write_doctor_project`) under a fresh
/// directory whose name is the default config file (`bddkit.yaml` or
/// `bddkit.yml`), and returns that directory — the default-discovery tests
/// run the binary with `current_dir(dir)` and no `--config` at all.
fn write_default_config_project(name: &str, config_file_name: &str) -> std::path::PathBuf {
    let cfg = write_doctor_project(
        name,
        "http://127.0.0.1:1/",
        "Feature: only\n  Scenario: one\n    When I request \"/ping\"\n",
        "",
    );
    let dir = cfg.parent().expect("cfg has a parent").to_path_buf();
    std::fs::rename(&cfg, dir.join(config_file_name)).expect("rename to default config name");
    dir
}

#[test]
fn doctor_picks_up_bddkit_yaml_from_the_working_directory() {
    let dir = write_default_config_project("doctor-default-yaml", "bddkit.yaml");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .arg("doctor")
        .current_dir(&dir)
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(
        stdout.contains("bddkit.yaml (found in working directory)"),
        "{stdout}"
    );
}

#[test]
fn doctor_picks_up_the_yml_spelling_too() {
    let dir = write_default_config_project("doctor-default-yml", "bddkit.yml");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .arg("doctor")
        .current_dir(&dir)
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(
        stdout.contains("bddkit.yml (found in working directory)"),
        "{stdout}"
    );
}

#[test]
fn doctor_reports_both_default_spellings_present_as_a_failed_config_row() {
    let dir = write_default_config_project("doctor-default-both", "bddkit.yaml");
    std::fs::copy(dir.join("bddkit.yaml"), dir.join("bddkit.yml")).expect("copy config");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .arg("doctor")
        .current_dir(&dir)
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("bddkit.yaml"), "{stdout}");
    assert!(stdout.contains("bddkit.yml"), "{stdout}");
}

#[test]
fn doctor_reports_no_config_found_as_a_failed_config_row_not_exit_two() {
    let dir =
        std::env::temp_dir().join(format!("bddkit-doctor-default-none-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    // A previous run of this test binary may have left a config behind.
    let _ = std::fs::remove_file(dir.join("bddkit.yaml"));
    let _ = std::fs::remove_file(dir.join("bddkit.yml"));
    let _ = std::fs::remove_file(dir.join("junit.xml"));

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--junit", "junit.xml"])
        .current_dir(&dir)
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "doctor never exits 2: {stdout}");
    assert!(stdout.contains("no config found"), "{stdout}");
    // `run` prepares its report paths before it resolves the config, so
    // `doctor` must too — even when there is no config to resolve.
    assert!(stdout.contains("reports"), "{stdout}");
    assert!(dir.join("junit.xml").is_file(), "{stdout}");
}

#[test]
fn bddkit_config_env_var_is_honoured_when_no_flag_is_given() {
    let cfg = write_doctor_project(
        "doctor-default-env",
        "http://127.0.0.1:1/",
        "Feature: only\n  Scenario: one\n    When I request \"/ping\"\n",
        "",
    );

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .arg("doctor")
        .env("BDDKIT_CONFIG", &cfg)
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(stdout.contains("(BDDKIT_CONFIG)"), "{stdout}");
}

/// A `--config` naming a file that does not exist must never quietly fall
/// through to a `bddkit.yaml` that happens to sit in the working directory —
/// resolution order step 1 wins outright, existence or not.
#[test]
fn an_explicit_missing_config_flag_is_refused_not_a_fallback_to_the_default() {
    let dir = write_default_config_project("doctor-explicit-missing", "bddkit.yaml");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config", "does-not-exist.yaml"])
        .current_dir(&dir)
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(
        stdout.contains("does-not-exist.yaml"),
        "names the file it was told, not the default: {stdout}"
    );
    assert!(
        !stdout.contains("no problems found"),
        "must not silently succeed against bddkit.yaml instead: {stdout}"
    );
}

/// Same guarantee, for `$BDDKIT_CONFIG` — resolution order step 2.
#[test]
fn a_missing_bddkit_config_env_var_is_refused_not_a_fallback_to_the_default() {
    let dir = write_default_config_project("doctor-env-missing", "bddkit.yaml");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .arg("doctor")
        .env("BDDKIT_CONFIG", "does-not-exist.yaml")
        .current_dir(&dir)
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(
        stdout.contains("does-not-exist.yaml"),
        "names the file it was told, not the default: {stdout}"
    );
    assert!(
        !stdout.contains("no problems found"),
        "must not silently succeed against bddkit.yaml instead: {stdout}"
    );
}

/// Resolution order step 1 over step 2: an explicit `--config` beats
/// `$BDDKIT_CONFIG` even when both name a real, different, healthy project.
#[test]
fn explicit_config_flag_wins_over_the_env_var() {
    let winner = write_doctor_project(
        "doctor-precedence-flag",
        "http://127.0.0.1:1/",
        "Feature: only\n  Scenario: one\n    When I request \"/ping\"\n",
        "",
    );
    let loser = write_doctor_project(
        "doctor-precedence-env",
        "http://127.0.0.1:1/",
        "Feature: only\n  Scenario: one\n    When I request \"/ping\"\n",
        "",
    );

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "doctor",
            "--config",
            winner.to_str().expect("path is UTF-8"),
        ])
        .env("BDDKIT_CONFIG", &loser)
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(
        stdout.contains(&format!("{} (--config)", winner.display())),
        "the flag's path and source, not the env var's: {stdout}"
    );
}

#[test]
fn run_with_no_config_anywhere_exits_two_naming_the_files_it_looked_for() {
    let dir = std::env::temp_dir().join(format!("bddkit-run-default-none-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let _ = std::fs::remove_file(dir.join("bddkit.yaml"));
    let _ = std::fs::remove_file(dir.join("bddkit.yml"));
    let _ = std::fs::remove_file(dir.join("junit.xml"));

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--junit", "junit.xml"])
        .current_dir(&dir)
        .output()
        .expect("failed to run bddkit");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("bddkit.yaml"), "{stderr}");
    assert!(stderr.contains("bddkit.yml"), "{stderr}");
    // An empty report a CI parser rejects loudly, never yesterday's green one.
    assert!(dir.join("junit.xml").is_file(), "{stderr}");
}

#[test]
fn resource_add_with_no_config_anywhere_exits_one() {
    let dir = std::env::temp_dir().join(format!("bddkit-add-default-none-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let _ = std::fs::remove_file(dir.join("bddkit.yaml"));
    let _ = std::fs::remove_file(dir.join("bddkit.yml"));

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["resource", "add", "api", "staging"])
        .current_dir(&dir)
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("no config found"), "{stdout}");
}

/// `--no-config` must not even attempt to load `bddkit.yaml`, so a broken one
/// does not block the simplest possible vocabulary lookup — this is the
/// scenario the flag exists for (see issue #48).
#[test]
fn steps_list_no_config_ignores_a_present_bddkit_yaml() {
    let dir = std::env::temp_dir().join(format!("bddkit-steps-no-config-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join("bddkit.yaml"), "not: [valid, yaml: at all").expect("write");

    let without_flag = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["steps", "list"])
        .current_dir(&dir)
        .output()
        .expect("failed to run bddkit");
    assert_eq!(
        without_flag.status.code(),
        Some(2),
        "the broken default config is picked up and fails: {}",
        String::from_utf8_lossy(&without_flag.stdout)
    );

    let with_flag = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["steps", "list", "--no-config"])
        .current_dir(&dir)
        .output()
        .expect("failed to run bddkit");
    let stdout = String::from_utf8_lossy(&with_flag.stdout);
    assert_eq!(with_flag.status.code(), Some(0), "{stdout}");
    assert!(stdout.contains("I request"), "{stdout}");
}

/// The same escape hatch, for `resource fields` — it shares the resolver but
/// is a separate command with its own `--no-config` flag, so it needs its own
/// proof the flag actually skips loading `bddkit.yaml`.
#[test]
fn resource_fields_no_config_ignores_a_present_bddkit_yaml() {
    let dir = std::env::temp_dir().join(format!(
        "bddkit-resource-fields-no-config-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join("bddkit.yaml"), "not: [valid, yaml: at all").expect("write");

    let without_flag = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["resource", "fields"])
        .current_dir(&dir)
        .output()
        .expect("failed to run bddkit");
    assert_eq!(
        without_flag.status.code(),
        Some(2),
        "the broken default config is picked up and fails: {}",
        String::from_utf8_lossy(&without_flag.stdout)
    );

    let with_flag = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["resource", "fields", "--no-config"])
        .current_dir(&dir)
        .output()
        .expect("failed to run bddkit");
    let stdout = String::from_utf8_lossy(&with_flag.stdout);
    assert_eq!(with_flag.status.code(), Some(0), "{stdout}");
    assert!(stdout.contains("dsn"), "{stdout}");
}

/// clap's `conflicts_with` on the real binary, not just declared in
/// `main.rs` — for both commands that carry the flag.
#[test]
fn steps_list_refuses_config_and_no_config_together() {
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["steps", "list", "--config", "x.yaml", "--no-config"])
        .output()
        .expect("failed to run bddkit");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("cannot be used with"), "{stderr}");
}

#[test]
fn resource_fields_refuses_config_and_no_config_together() {
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["resource", "fields", "--config", "x.yaml", "--no-config"])
        .output()
        .expect("failed to run bddkit");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("cannot be used with"), "{stderr}");
}

/// The first question an agent asks a binary on `PATH`. `version` and
/// `--version` must be the same answer, and its first line must stay the one
/// line a script greps.
#[test]
fn version_answers_to_both_spellings_and_leads_with_the_semver() {
    let subcommand = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .arg("version")
        .output()
        .expect("failed to run bddkit");
    let flag = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .arg("--version")
        .output()
        .expect("failed to run bddkit");

    let short = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .arg("-V")
        .output()
        .expect("failed to run bddkit");

    assert_eq!(subcommand.status.code(), Some(0));
    assert_eq!(flag.status.code(), Some(0));
    assert_eq!(short.status.code(), Some(0));

    let text = String::from_utf8_lossy(&subcommand.stdout);
    assert_eq!(text, String::from_utf8_lossy(&flag.stdout), "same answer");

    let first = text.lines().next().expect("version prints something");
    assert_eq!(
        first,
        format!("bddkit {}", env!("CARGO_PKG_VERSION")),
        "the first line is the binary and its version, nothing else"
    );
    assert!(text.contains("bddkit steps list"), "signposts: {text}");
    assert!(text.contains("bddkit doctor"), "signposts: {text}");

    // The whole point of the `version` / `long_version` split: a one-token
    // edit that feeds the long form to `-V` too breaks every script grepping
    // it, and every assertion above would stay green.
    assert_eq!(
        String::from_utf8_lossy(&short.stdout),
        format!("bddkit {}\n", env!("CARGO_PKG_VERSION")),
        "-V stays the one line a script greps"
    );
}

/// A project with one passing and one failing scenario. The failure text
/// carries the two things an XML writer gets wrong first: a `]]>` in the
/// expected value and the NUL bytes of the `<<null>>` sentinel in the actual.
fn write_report_project(base: &str, name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("bddkit-{name}-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/report.feature"),
        r#"Feature: reported feature
  Scenario: passes
    When I request "/ping"
    Then the response code is 200

  Scenario: fails
    When I request "/ping"
    And set variable "x" to "<<null>>"
    Then variable "x" should be equal to "]]>"
    And the response code is 200
"#,
    )
    .expect("write feature");
    let config = dir.join("cfg.yaml");
    std::fs::write(
        &config,
        format!(
            "paths: [{}]\nresources:\n  api:\n    stub:\n      base_url: {base}\n",
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("write config");
    config
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_junit_writes_a_well_formed_report_for_a_failing_run() {
    let base = common::spawn().await;
    let config = write_report_project(&base, "junit");
    let report = config.with_file_name("out/junit.xml");
    let _ = std::fs::remove_dir_all(report.parent().expect("parent"));

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(&config)
        .arg("--junit")
        .arg(&report)
        .output()
        .expect("run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    let xml = std::fs::read_to_string(&report).expect("the report is written");
    let package = sxd_document::parser::parse(&xml)
        .unwrap_or_else(|e| panic!("the report must parse as XML: {e:?}\n{xml}"));
    let doc = package.as_document();
    let value = |xpath: &str| {
        sxd_xpath::evaluate_xpath(&doc, xpath)
            .expect("xpath evaluates")
            .string()
    };
    assert_eq!(value("count(//testsuite)"), "1", "{xml}");
    assert_eq!(value("count(//testcase)"), "2", "{xml}");
    assert_eq!(
        value("count(//testcase[@name='fails']/failure)"),
        "1",
        "{xml}"
    );
    assert_eq!(
        value("count(//testcase[@name='passes']/failure)"),
        "0",
        "{xml}"
    );
    let failure = value("//testcase[@name='fails']/failure");
    assert!(failure.contains("expected: ]]>"), "{failure}");
    assert!(
        failure.contains("GET http://"),
        "the HTTP exchange is part of the failure:\n{failure}"
    );
    assert!(
        value("//testcase[@name='passes']/@time")
            .parse::<f64>()
            .expect("time is a float")
            > 0.0,
        "{xml}"
    );
    let steps = value("//testcase[@name='fails']/system-out");
    assert!(
        steps.contains("passed") && steps.contains("failed") && steps.contains("skipped"),
        "{steps}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_cucumber_json_writes_the_feature_scenario_step_layout() {
    let base = common::spawn().await;
    let config = write_report_project(&base, "cucumber-json");
    let report = config.with_file_name("cucumber.json");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(&config)
        .arg("--cucumber-json")
        .arg(&report)
        .output()
        .expect("run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    let json: Value =
        serde_json::from_slice(&std::fs::read(&report).expect("the report is written"))
            .expect("the report parses as JSON");
    let feature = &json[0];
    assert!(
        feature["uri"]
            .as_str()
            .expect("uri")
            .ends_with("report.feature"),
        "{json}"
    );
    assert_eq!(feature["name"], "reported feature", "{json}");
    let scenarios = feature["elements"].as_array().expect("elements");
    assert_eq!(scenarios.len(), 2, "{json}");
    let failing = &scenarios[1];
    assert_eq!(failing["name"], "fails", "{json}");
    assert_eq!(failing["type"], "scenario", "{json}");
    let steps = failing["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 4, "{json}");
    assert_eq!(steps[0]["keyword"], "When ", "{json}");
    assert_eq!(steps[0]["name"], "I request \"/ping\"", "{json}");
    assert_eq!(steps[0]["line"], 7, "{json}");
    assert_eq!(steps[0]["result"]["status"], "passed", "{json}");
    assert!(
        steps[0]["result"]["duration"]
            .as_u64()
            .expect("nanoseconds")
            > 0,
        "{json}"
    );
    assert_eq!(
        steps[1]["name"], "set variable \"x\" to \"<<null>>\"",
        "raw step text: {json}"
    );
    assert_eq!(steps[2]["result"]["status"], "failed", "{json}");
    assert!(
        steps[2]["result"]["error_message"]
            .as_str()
            .expect("error_message")
            .contains("expected: ]]>"),
        "{json}"
    );
    assert_eq!(steps[3]["result"]["status"], "skipped", "{json}");
}

#[test]
fn a_report_path_is_truncated_before_the_config_is_read() {
    let dir = std::env::temp_dir().join(format!("bddkit-report-broken-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let cfg = dir.join("cfg.yaml");
    std::fs::write(
        &cfg,
        "paths: [features]\nresources:\n  api:\n    a:\n      base_url: ${BDDKIT_ABSENT_VAR}\n",
    )
    .expect("write config");
    let report = dir.join("junit.xml");
    std::fs::write(&report, "<testsuites>yesterday's green report</testsuites>")
        .expect("stale report");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(&cfg)
        .arg("--junit")
        .arg(&report)
        .output()
        .expect("run bddkit");

    assert_eq!(
        out.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(&report).expect("the file still exists"),
        "",
        "a stale report must not survive a run that never started"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unwritable_report_path_is_a_startup_failure_and_doctor_reports_it() {
    let (base, calls) = spawn_eventual_post_stub(Some(1)).await;
    let config = write_eventual_post_project(&base, "unwritable-report");
    // A path under a regular file cannot be created.
    let report = config.join("cucumber.json");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(&config)
        .arg("--cucumber-json")
        .arg(&report)
        .output()
        .expect("run bddkit");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("cucumber.json"),
        "the path is named: {stderr}"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "no request may leave before the report is prepared"
    );

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config"])
        .arg(&config)
        .arg("--cucumber-json")
        .arg(&report)
        .output()
        .expect("run bddkit");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("✗ reports"), "{stdout}");
}

/// Builds a temp project for an `I include` acceptance test: copies each
/// `(dest path under the temp dir, source fixture path)` pair, then writes
/// the one `cfg.yaml` every one of these tests shares (a single `stub` API,
/// no live server needed — none of them make an HTTP request). Returns the
/// project directory.
fn build_include_project(dir_slug: &str, fixtures: &[(&str, &str)]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("bddkit-{dir_slug}-{}", std::process::id()));
    for (dest, src) in fixtures {
        let dest_path = dir.join(dest);
        std::fs::create_dir_all(dest_path.parent().expect("dest has a parent")).expect("mkdir");
        std::fs::copy(src, &dest_path).unwrap_or_else(|e| panic!("copy {src} to {dest}: {e}"));
    }
    std::fs::write(
        dir.join("cfg.yaml"),
        "paths: [features]\nresources:\n  api:\n    stub:\n      base_url: http://example.test\n",
    )
    .expect("write config");
    dir
}

/// Runs `bddkit <subcommand> --config cfg.yaml` inside a project directory
/// built by `build_include_project`, and decodes the result for assertions.
fn run_bddkit_in(subcommand: &str, dir: &std::path::Path) -> (Option<i32>, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([subcommand, "--config", "cfg.yaml"])
        .current_dir(dir)
        .output()
        .expect("failed to run bddkit");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// `I include` runs another file's scenario inline and copies back only its
/// declared `@exports` variable — no HTTP steps involved, so no stub server
/// is needed.
#[test]
fn include_of_a_one_scenario_file_exports_its_declared_variable() {
    let dir = build_include_project(
        "include-test",
        &[
            (
                "features/target.feature",
                "tests/features/include/target.feature",
            ),
            (
                "features/caller.feature",
                "tests/features/include/caller.feature",
            ),
        ],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(0),
        "--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
}

/// A nested `I include` resolves its path against the file it is written in
/// (issue #76): `lib.feature` sits in `includes/` and includes a sibling, so
/// the runtime must agree with the validator on `includes/`, not its parent.
#[test]
fn a_nested_include_resolves_relative_to_the_intermediate_file() {
    let dir = build_include_project(
        "include-nested",
        &[
            (
                "includes/lib.feature",
                "tests/features/include/nested/includes/lib.feature",
            ),
            (
                "features/caller.feature",
                "tests/features/include/nested/features/caller.feature",
            ),
        ],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(0),
        "--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
}

/// `I include "<file>" with prefix "<p>"` (issue #52) renames every exported
/// variable to `<p>_<name>` as it crosses back into the caller — the same
/// target file is included twice here, once per prefix, and neither call's
/// `userId` export must clobber the other's.
#[test]
fn including_the_same_file_twice_with_different_prefixes_keeps_both_exports() {
    let dir = build_include_project(
        "include-prefix-test",
        &[
            (
                "features/target.feature",
                "tests/features/include/target.feature",
            ),
            (
                "features/caller.feature",
                "tests/features/include/caller_prefix.feature",
            ),
        ],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(0),
        "--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
}

/// A failed step inside an included scenario must not leave the caller's
/// file-level `VarStack` swapped for the included one — `run_include` must
/// restore it on the way out regardless of the include's own outcome. Two
/// scenarios in one file: the first sets a variable then includes a scenario
/// whose only step fails cleanly (an unset-variable assertion, never a
/// panic); the second — sharing the file's `VarStack` per invariant 2 —
/// asserts the first scenario's variable is still there. If a later refactor
/// ever short-circuits before the restore (a `?` between the swap and the
/// restore, or a fresh `push_frame` swapped in for the whole-stack swap),
/// this regresses: scenario 2 sees "before" undefined and fails too.
#[test]
fn a_failed_include_still_restores_the_callers_var_stack() {
    // These fixtures live under `tests/fixtures/include/`, NOT
    // `tests/features/`, because `restore_on_failure.feature` fails one
    // scenario on purpose — `tests/fixtures/` is this repo's existing home
    // for fixtures that are not part of the "every feature file passes"
    // acceptance gate (see `tests/fixtures/echo-plugin`,
    // `tests/fixtures/worker-plugin`). The include target is placed in a
    // sibling `targets/` directory, outside `paths: [features]` — its own
    // `.feature` file's `I include "../targets/..."` path matches — so it is
    // reached only through the include and never discovered and run a
    // second time on its own, which would add an unrelated failed scenario.
    let dir = build_include_project(
        "include-restore-test",
        &[
            (
                "targets/failing_target.feature",
                "tests/fixtures/include/failing_target.feature",
            ),
            (
                "features/restore_on_failure.feature",
                "tests/fixtures/include/restore_on_failure.feature",
            ),
        ],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(1),
        "one scenario fails (the include), the other passes — never a validation exit 2\n\
         --- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(
        stdout.contains("The included scenario fails"),
        "the failing scenario ran: --- stdout ---\n{stdout}"
    );
    // The report only prints FAILING scenarios, so scenario 2 passing leaves
    // no line of its own — the summary counts are the proof it ran and
    // passed: one file, two scenarios, exactly one failed.
    assert!(
        stdout.contains("scenarios: 2, failed: 1"),
        "scenario 2 (\"before\" == \"kept\") must pass silently, not add a \
         second failure — the swap must have been restored: \
         --- stdout ---\n{stdout}"
    );
    assert_eq!(
        stdout.matches("is not set").count(),
        1,
        "only the include's own failure should mention an unset variable; a \
         second occurrence would mean scenario 2 also saw \"before\" as \
         undefined — the swap was left in place: --- stdout ---\n{stdout}"
    );
}

/// A `with:` table cell is interpolated against the CALLER's scope before
/// `world.vars` is swapped for the include's fresh stack — never left as a
/// literal `<<...>>` token, and never resolved against the (empty) included
/// scope. `target_with.feature` stores the value it receives into its own
/// variable and compares it against a fixed literal, independent of the
/// substitution token itself, so a regression that skips the caller-scope
/// interpolation (or interpolates too late, after the swap) fails the run
/// instead of coincidentally still matching.
#[test]
fn with_table_cell_is_interpolated_against_the_callers_scope() {
    // Same reasoning and layout as the restore-on-failure test above:
    // `target_with.feature` fails standalone (its own Examples row is
    // `unused@example.com`, not `test@example.com`), so it goes in the
    // sibling `targets/` directory, reached only through the include.
    let dir = build_include_project(
        "include-with-test",
        &[
            (
                "targets/target_with.feature",
                "tests/fixtures/include/target_with.feature",
            ),
            (
                "features/caller_with.feature",
                "tests/fixtures/include/caller_with.feature",
            ),
        ],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(0),
        "--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
}

/// Invariant 2 (state scoping) says variables never flow into an included
/// scenario except through `with:` — this proves the negative directly: the
/// included scenario reads a variable the caller set and must find it
/// genuinely UNDEFINED, not merely empty. `variable "callerOnly" should be
/// empty` first calls `variable(w, name)` (`steps/vars.rs`), which fails with
/// `variable "callerOnly" is not set` for anything never `set` in the
/// included scenario's own fresh `VarStack` — a stale-but-visible value would
/// instead either match (if still `"secret"`) or fail on content, never on
/// "not set". If `run_include` ever stopped swapping in a fresh `VarStack`
/// (or swapped it in too late), this test would fail differently: the
/// assertion would report the value `"secret"` instead of "is not set", or
/// pass outright were the check changed to tolerate that.
#[test]
fn a_caller_variable_is_not_visible_inside_an_included_scenario() {
    // Same layout as `a_failed_include_still_restores_the_callers_var_stack`:
    // the included scenario fails on purpose, so both fixtures live under
    // `tests/fixtures/include/`, and the target sits in a sibling `targets/`
    // directory outside `paths: [features]` so it is reached only through
    // the include, never discovered and run a second time on its own.
    let dir = build_include_project(
        "include-isolation-test",
        &[
            (
                "targets/isolation_target_cannot_read_caller.feature",
                "tests/fixtures/include/isolation_target_cannot_read_caller.feature",
            ),
            (
                "features/isolation_caller_cannot_read.feature",
                "tests/fixtures/include/isolation_caller_cannot_read.feature",
            ),
        ],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(1),
        "the included scenario's own assertion must fail (the variable is \
         genuinely undefined inside the include), which fails the include \
         step and the caller scenario with it: \
         --- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(
        stdout.contains(r#"variable "callerOnly" is not set"#),
        "the failure must name the variable as UNSET, not merely empty or \
         mismatched — proving the caller's value never crossed into the \
         included scenario's scope: --- stdout ---\n{stdout}"
    );
}

/// Invariant 2's other half: a DECLARED `@exports` name reaches the caller,
/// but anything else the included scenario sets does not, even though both
/// live in the same fresh `VarStack` while the include runs. Two scenarios
/// sharing the caller file's `VarStack` (persisted across scenarios within
/// one file, per invariant 2): the first includes and checks the declared
/// export `kept` arrived; the second — without including anything itself —
/// asserts `notExported` is still undefined. If `run_include` ever copied
/// the whole included scope back instead of just the declared exports, the
/// second scenario would find `notExported == "no"` and pass instead of
/// failing — so this test is provative in the direction that matters: a
/// leak turns its expected failure into an unexpected pass, not the reverse.
#[test]
fn only_the_declared_export_reaches_the_caller() {
    let dir = build_include_project(
        "include-export-boundary-test",
        &[
            (
                "targets/isolation_target_export_boundary.feature",
                "tests/fixtures/include/isolation_target_export_boundary.feature",
            ),
            (
                "features/isolation_caller_export_boundary.feature",
                "tests/fixtures/include/isolation_caller_export_boundary.feature",
            ),
        ],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(1),
        "scenario 1 (declared export `kept`) must pass; scenario 2 must \
         fail on purpose, proving `notExported` never crossed the boundary: \
         --- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(
        stdout.contains("scenarios: 2, failed: 1"),
        "scenario 1 must pass silently (the declared export `kept` arrived \
         and equalled \"yes\") — a failure there would mean the export \
         itself is broken, not just the boundary: \
         --- stdout ---\n{stdout}"
    );
    assert!(
        stdout.contains(r#"variable "notExported" is not set"#),
        "scenario 2's failure must name `notExported` as UNSET — not \
         merely empty or some other mismatch — proving it never arrived, \
         rather than arriving as an unexpected value: \
         --- stdout ---\n{stdout}"
    );
}

/// A declared `@exports` name that the included scenario never `set`s must
/// fail the include step itself — not the caller scenario's own next
/// assertion — with an error naming the missing export, per `run_include`'s
/// `export_result` handling in `src/runner.rs`. If a regression silently
/// dropped an unresolved export instead of failing (e.g. by skipping it
/// rather than recording `missing`), this run would exit 0 instead of 1 and
/// the output would never mention "neverSet".
#[test]
fn a_declared_export_that_is_never_set_fails_the_include_step() {
    let dir = build_include_project(
        "include-missing-export-test",
        &[
            (
                "targets/isolation_target_missing_export.feature",
                "tests/fixtures/include/isolation_target_missing_export.feature",
            ),
            (
                "features/isolation_caller_missing_export.feature",
                "tests/fixtures/include/isolation_caller_missing_export.feature",
            ),
        ],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(1),
        "the missing export must fail the include step (and the caller \
         scenario with it), a scenario failure rather than a validation \
         (exit 2) or a silent pass: \
         --- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(
        stdout.contains("declared export \"neverSet\"")
            && stdout.contains("the variable is not set"),
        "the failure must clearly name the missing export \"neverSet\": \
         --- stdout ---\n{stdout}"
    );
}

#[test]
fn include_by_scenario_name_picks_the_named_one() {
    let dir = build_include_project(
        "include-multi-test",
        &[
            (
                "features/multi.feature",
                "tests/features/include/multi.feature",
            ),
            (
                "features/caller_multi.feature",
                "tests/features/include/caller_multi.feature",
            ),
        ],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(0),
        "--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
}

#[test]
fn include_of_an_outline_uses_the_callers_with_row() {
    let dir = build_include_project(
        "include-outline-with-test",
        &[
            (
                "features/outline.feature",
                "tests/features/include/outline.feature",
            ),
            (
                "features/caller_outline_with.feature",
                "tests/features/include/caller_outline_with.feature",
            ),
        ],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(0),
        "--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
}

#[test]
fn include_of_an_outline_with_one_examples_row_and_no_with_uses_that_row() {
    let dir = build_include_project(
        "include-outline-bare-test",
        &[
            (
                "features/outline.feature",
                "tests/features/include/outline.feature",
            ),
            (
                "features/caller_outline_bare.feature",
                "tests/features/include/caller_outline_bare.feature",
            ),
        ],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(0),
        "--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
}

#[test]
fn debug_mode_logs_the_includes_with_and_export_lines() {
    let dir = build_include_project(
        "debug-include-test",
        &[
            (
                "features/outline.feature",
                "tests/features/include/outline.feature",
            ),
            (
                "features/caller_debug_export.feature",
                "tests/features/include/caller_debug_export.feature",
            ),
        ],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(0),
        "--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );

    // In debug mode, the include logs its inputs and every exported variable
    assert!(
        stderr.contains("outline.feature ›"),
        "stderr should contain 'outline.feature ›' (entry line with filename and separator):\n{stderr}"
    );
    assert!(
        stderr.contains("  with value = "),
        "stderr should contain '  with value = ' (with two-space indentation):\n{stderr}"
    );
    assert!(
        stderr.contains("  export seen = "),
        "stderr should contain '  export seen = ' (with two-space indentation):\n{stderr}"
    );
}

/// The `with prefix "<p>"` name applies before the debug log line is
/// printed, so `--debug` shows the name that actually lands in the caller's
/// scope (`buyer_seen`), not the included scenario's own export name (`seen`).
#[test]
fn debug_mode_logs_the_prefixed_export_name() {
    let dir = build_include_project(
        "debug-include-prefix-test",
        &[
            (
                "features/outline.feature",
                "tests/features/include/outline.feature",
            ),
            (
                "features/caller_debug_export_prefix.feature",
                "tests/features/include/caller_debug_export_prefix.feature",
            ),
        ],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(0),
        "--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(
        stderr.contains("  export buyer_seen = "),
        "stderr should contain '  export buyer_seen = ' (prefixed export name):\n{stderr}"
    );
}

#[test]
fn debug_mode_logs_a_macro_s_exported_variables() {
    let dir = std::env::temp_dir().join(format!("bddkit-macro-debug-test-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("macros.yaml"),
        "- step: I login as user \"{email}\"\n  do:\n    - set variable \"token\" to \"<<email>>-token\"\n  exports: [token]\n",
    )
    .expect("write macros");
    std::fs::write(
        dir.join("features/caller.feature"),
        "Feature: f\n  Scenario: s\n    Given I am in debug mode\n    When I login as user \"alice@example.com\"\n",
    )
    .expect("write feature");
    std::fs::write(
        dir.join("cfg.yaml"),
        "macro_paths: [macros.yaml]\npaths: [features]\nresources:\n  api:\n    stub:\n      base_url: http://example.test\n",
    )
    .expect("write config");

    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(0),
        "--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(
        stderr.contains(r#"macro "I login as user \"alice@example.com\"""#),
        "stderr should name the macro call:\n{stderr}"
    );
    assert!(
        stderr.contains("  export token = alice@example.com-token"),
        "stderr should contain the exported variable:\n{stderr}"
    );
}

/// The header must print before the macro body runs, not after: otherwise a
/// macro whose body fails logs nothing at all, the case debug mode is for.
#[test]
fn debug_mode_names_the_macro_even_when_its_body_fails() {
    let dir = std::env::temp_dir().join(format!(
        "bddkit-macro-debug-fail-test-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("macros.yaml"),
        "- step: I login as user \"{email}\"\n  do:\n    - variable \"missing\" should be equal to \"x\"\n",
    )
    .expect("write macros");
    std::fs::write(
        dir.join("features/caller.feature"),
        "Feature: f\n  Scenario: s\n    Given I am in debug mode\n    When I login as user \"alice@example.com\"\n",
    )
    .expect("write feature");
    std::fs::write(
        dir.join("cfg.yaml"),
        "macro_paths: [macros.yaml]\npaths: [features]\nresources:\n  api:\n    stub:\n      base_url: http://example.test\n",
    )
    .expect("write config");

    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(1),
        "the macro body's assertion must fail the run\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(
        stderr.contains(r#"macro "I login as user \"alice@example.com\"""#),
        "the header must print even though the macro body failed:\n{stderr}"
    );
}

#[test]
fn doctor_reports_a_broken_include_the_same_way_run_does() {
    // Same missing-file fixture as include_of_a_missing_file_is_a_problem (Task 7),
    // but invoke `bddkit doctor --config cfg.yaml` instead of `run`.
    // Assert: exit code 1 (doctor's own convention — 0 or 1 only, never 2,
    // per CLAUDE.md) with the missing file's name appearing in stdout.
    let cfg = write_doctor_project(
        "doctor-include",
        "http://127.0.0.1:1/",
        "Feature: caller\n  Scenario: test\n    Given I include \"nope.feature\"\n",
        "",
    );

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config", cfg.to_str().expect("path is UTF-8")])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    // Doctor exits 1 (never 2) even for a config that would make run exit 2
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    // The missing file name is mentioned in the output
    assert!(stdout.contains("nope.feature"), "{stdout}");
}

#[test]
fn steps_list_includes_both_include_steps() {
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["steps", "list"])
        .output()
        .expect("failed to run bddkit");

    let text = String::from_utf8_lossy(&out.stdout);
    // Both Include and IncludeScenario steps start with "I include"
    assert!(
        text.contains("I include"),
        "steps list should contain 'I include': {text}"
    );
}

/// Answers what an upload endpoint would care about: was it multipart, and
/// did the file and the text field arrive.
async fn spawn_upload_stub() -> String {
    let app = Router::new().route(
        "/upload",
        post(
            |headers: axum::http::HeaderMap, body: axum::body::Bytes| async move {
                let content_type = headers
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("");
                let text = String::from_utf8_lossy(&body);
                Json(json!({
                    "multipart": content_type.starts_with("multipart/form-data; boundary="),
                    "has_file": text.contains("filename=\"id_front.png\"") && text.contains("PNGDATA"),
                    "has_field": text.contains("name=\"kind\"") && text.contains("ID_CARD"),
                }))
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind upload stub");
    let address = listener.local_addr().expect("upload stub address");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve upload stub");
    });
    format!("http://{address}/")
}

const UPLOAD_FEATURE: &str = r#"Feature: upload
  Scenario: a file and a field go out as multipart
    Given the request form parameters are:
      | name | value   |
      | kind | ID_CARD |
    And I attach the file "fixtures/id_front.png" to the request as "content"
    When I request "/upload" using HTTP POST
    Then the response body contains JSON:
      """
      {"multipart": true, "has_file": true, "has_field": true}
      """
"#;

fn write_upload_fixture(cfg: &std::path::Path) {
    let fixtures = cfg.parent().expect("config dir").join("features/fixtures");
    std::fs::create_dir_all(&fixtures).expect("mkdir fixtures");
    std::fs::write(fixtures.join("id_front.png"), b"PNGDATA").expect("write fixture");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_file_is_uploaded_as_multipart_with_its_form_fields() {
    let base = spawn_upload_stub().await;
    let cfg = write_doctor_project("upload-ok", &base, UPLOAD_FEATURE, "");
    write_upload_fixture(&cfg);

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config", cfg.to_str().expect("path is UTF-8")])
        .output()
        .expect("failed to run bddkit");

    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn a_missing_literal_attachment_stops_the_run_before_any_request_with_exit_two() {
    let cfg = write_doctor_project("upload-missing", "http://127.0.0.1:1/", UPLOAD_FEATURE, "");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config", cfg.to_str().expect("path is UTF-8")])
        .output()
        .expect("failed to run bddkit");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("fixtures/id_front.png"), "{stderr}");
}

#[test]
fn doctor_reports_a_missing_literal_attachment() {
    let cfg = write_doctor_project("upload-doctor", "http://127.0.0.1:1/", UPLOAD_FEATURE, "");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config", cfg.to_str().expect("path is UTF-8")])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("no such file"), "{stdout}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_missing_attachment_behind_a_variable_fails_the_step_not_the_startup() {
    let base = spawn_upload_stub().await;
    let feature = r#"Feature: upload
  Scenario: the path is only known at run time
    Given set variable "file" to "fixtures/gone.png"
    And I attach the file "<<file>>" to the request as "content"
"#;
    let cfg = write_doctor_project("upload-var-missing", &base, feature, "");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config", cfg.to_str().expect("path is UTF-8")])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("fixtures/gone.png"), "{stdout}");
}

// ---------------------------------------------------------------------------
// `run --events`
// ---------------------------------------------------------------------------

/// A project under a fresh directory, returning the config path. `features`
/// are `(file name, content)` pairs; `macros` is one YAML file, if any. With a
/// `base` the config declares one API; without, none.
fn write_events_project(
    name: &str,
    base: &str,
    features: &[(&str, &str)],
    macros: Option<&str>,
) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("bddkit-events-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    for (file, content) in features {
        std::fs::write(dir.join("features").join(file), content).expect("write feature");
    }
    let mut config = format!("paths: [{}]\n", path_text(&dir.join("features")));
    if let Some(macros) = macros {
        std::fs::create_dir_all(dir.join("macros")).expect("mkdir");
        std::fs::write(dir.join("macros/m.yaml"), macros).expect("write macros");
        config.push_str(&format!(
            "macro_paths: [{}]\n",
            path_text(&dir.join("macros"))
        ));
    }
    if base.is_empty() {
        config.push_str("resources:\n  api: {}\n");
    } else {
        config.push_str(&format!(
            "resources:\n  api:\n    stub:\n      base_url: {base}\n      timeout_secs: 2\n"
        ));
    }
    let cfg = dir.join("cfg.yaml");
    std::fs::write(&cfg, config).expect("write config");
    cfg
}

fn path_text(path: &std::path::Path) -> String {
    path.display().to_string().replace('\\', "/")
}

fn read_stream(path: &std::path::Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .expect("the stream exists")
        .lines()
        .map(|line| serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}: {line:?}")))
        .collect()
}

/// `bddkit run --config <cfg> --events <dir>/events.ndjson <extra…>`, and the
/// stream it wrote.
fn run_with_events(
    cfg: &std::path::Path,
    extra: &[&str],
) -> (std::process::Output, Vec<Value>, std::path::PathBuf) {
    let events = cfg.with_file_name("events.ndjson");
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(cfg)
        .arg("--events")
        .arg(&events)
        .env_remove("BDDKIT_ARTIFACTS_DIR")
        .args(extra)
        .output()
        .expect("run bddkit");
    let stream = read_stream(&events);
    (out, stream, events)
}

fn of_type<'a>(stream: &'a [Value], kind: &str) -> Vec<&'a Value> {
    stream.iter().filter(|e| e["type"] == kind).collect()
}

#[test]
fn an_events_path_is_truncated_before_the_config_is_read() {
    let cfg = write_events_project("broken", "", &[], None);
    std::fs::write(
        &cfg,
        "paths: [features]\nresources:\n  api:\n    a:\n      base_url: ${BDDKIT_ABSENT_VAR}\n",
    )
    .expect("write config");
    std::fs::write(
        cfg.with_file_name("events.ndjson"),
        "{\"type\":\"yesterday\"}\n",
    )
    .expect("stale stream");

    let (out, stream, _) = run_with_events(&cfg, &[]);

    assert_eq!(out.status.code(), Some(2));
    assert!(stream.is_empty(), "{stream:?}");
}

#[test]
fn a_refused_run_leaves_an_empty_events_file() {
    let cfg = write_events_project(
        "refused",
        "",
        &[(
            "a.feature",
            "Feature: a\n  Scenario: s\n    Given a step nobody defined\n",
        )],
        None,
    );
    std::fs::write(
        cfg.with_file_name("events.ndjson"),
        "{\"type\":\"yesterday\"}\n",
    )
    .expect("stale stream");

    let (out, stream, _) = run_with_events(&cfg, &[]);

    assert_eq!(out.status.code(), Some(2));
    assert!(stream.is_empty(), "{stream:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_line_of_the_stream_is_one_json_object_with_a_rising_seq() {
    let base = common::spawn().await;
    let cfg = write_report_project(&base, "events-lines");
    let (_, stream, _) = run_with_events(&cfg, &[]);

    assert!(stream.len() > 6, "{stream:?}");
    for (index, event) in stream.iter().enumerate() {
        assert!(event.is_object(), "{event}");
        assert_eq!(event["seq"], index, "{event}");
        assert!(event["type"].is_string(), "{event}");
        if index > 0 {
            assert!(
                event["t"].as_u64() >= stream[index - 1]["t"].as_u64(),
                "t goes back: {event}"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_started_names_the_artifact_root() {
    let base = common::spawn().await;
    let cfg = write_report_project(&base, "events-artifact-root");
    let (_, stream, _) = run_with_events(&cfg, &[]);
    let run_id = stream[0]["run_id"].as_str().expect("run_id");
    assert_eq!(
        stream[0]["artifacts_dir"],
        std::env::temp_dir()
            .join(format!("bddkit-{run_id}"))
            .display()
            .to_string(),
        "the default root, absolute"
    );

    // A relative flag value is reported as resolved against the working directory.
    let (_, stream, _) = run_with_events(&cfg, &["--artifacts-dir", "evidence"]);
    let root = stream[0]["artifacts_dir"].as_str().expect("artifacts_dir");
    assert!(std::path::Path::new(root).is_absolute(), "{root}");
    assert_eq!(
        std::path::Path::new(root),
        std::env::current_dir().expect("cwd").join("evidence")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_stream_opens_with_run_started_and_closes_with_run_finished() {
    let base = common::spawn().await;
    let cfg = write_report_project(&base, "events-ends");
    let (out, stream, _) = run_with_events(&cfg, &[]);

    let stdout = String::from_utf8_lossy(&out.stdout);
    let first = &stream[0];
    assert_eq!(first["type"], "run_started");
    assert_eq!(first["schema"], 1);
    assert_eq!(first["bddkit"], env!("CARGO_PKG_VERSION"));
    assert_eq!(first["files"], 1);
    assert!(first["started_at_unix_ms"].as_u64().unwrap_or(0) > 1_700_000_000_000);
    let run_id = first["run_id"].as_str().expect("run_id");
    assert!(stdout.contains(&format!("run {run_id}")), "{stdout}");

    let last = stream.last().expect("lines");
    assert_eq!(last["type"], "run_finished");
    assert_eq!(last["exit"], 1, "one scenario of the project fails");
    assert_eq!(last.get("signal"), None);
    assert_eq!(of_type(&stream, "run_finished").len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_files_running_together_interleave_in_the_stream() {
    // The barrier opens only when both requests are in flight, so both
    // `step_started` lines must precede both `step_finished` ones — which a
    // stream buffered until its file ends could never show.
    let base = common::spawn_barrier(2).await;
    let dir = std::env::temp_dir().join(format!("bddkit-events-barrier-{}", std::process::id()));
    let cfg = write_parallel_fixture(&dir, &base, 2, "");
    let (out, stream, _) = run_with_events(&cfg, &[]);

    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let barrier = |kind: &str| -> Vec<u64> {
        of_type(&stream, kind)
            .into_iter()
            .filter(|e| {
                e["step"] == 1 // the request; step 0 sets a variable
            })
            .map(|e| e["seq"].as_u64().expect("seq"))
            .collect()
    };
    let (started, finished) = (barrier("step_started"), barrier("step_finished"));
    assert_eq!((started.len(), finished.len()), (2, 2), "{stream:?}");
    assert!(
        started.iter().max() < finished.iter().min(),
        "the second file started its request only after the first one finished: {stream:?}"
    );
    let files: std::collections::HashSet<_> = of_type(&stream, "file_started")
        .iter()
        .map(|e| e["file"].as_str().expect("file").to_string())
        .collect();
    assert_eq!(files.len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn steps_after_a_failure_are_reported_as_skipped() {
    let base = common::spawn().await;
    let cfg = write_report_project(&base, "events-skipped");
    let (_, stream, _) = run_with_events(&cfg, &[]);

    let second: Vec<_> = stream.iter().filter(|e| e["scenario"] == 1).collect();
    let kinds: Vec<_> = second
        .iter()
        .map(|e| (e["type"].as_str().unwrap_or(""), e["step"].as_u64()))
        .collect();
    assert_eq!(
        kinds,
        [
            ("scenario_started", None),
            ("step_started", Some(0)),
            ("step_finished", Some(0)),
            ("step_started", Some(1)),
            ("step_finished", Some(1)),
            ("step_started", Some(2)),
            ("step_finished", Some(2)),
            ("step_skipped", Some(3)),
            ("scenario_finished", None),
        ]
    );
    assert_eq!(second[6]["status"], "failed");
    assert_eq!(second[7]["text"], "the response code is 200");
    assert_eq!(second[8]["status"], "failed");
}

#[test]
fn outline_rows_get_distinct_example_ordinals() {
    let cfg = write_events_project(
        "outline",
        "",
        &[(
            "o.feature",
            "Feature: o\n  Scenario: plain\n    Given set variable \"x\" to \"1\"\n\n  Scenario Outline: rows\n    Given set variable \"x\" to \"<n>\"\n\n    Examples:\n      | n |\n      | 1 |\n      | 2 |\n\n    Examples:\n      | n |\n      | 3 |\n",
        )],
        None,
    );
    let (out, stream, _) = run_with_events(&cfg, &[]);

    assert_eq!(out.status.code(), Some(0));
    let started = of_type(&stream, "scenario_started");
    assert_eq!(started.len(), 4);
    assert_eq!(started[0].get("example"), None, "a plain scenario has none");
    let rows: Vec<_> = started[1..]
        .iter()
        .map(|e| {
            (
                e["scenario"].as_u64(),
                e["example"].as_u64(),
                e["line"].as_u64(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            (Some(1), Some(0), Some(5)),
            (Some(2), Some(1), Some(5)),
            (Some(3), Some(2), Some(5))
        ]
    );
}

#[test]
fn a_macro_body_step_is_an_event_with_its_caller_as_parent() {
    let cfg = write_events_project(
        "macro",
        "",
        &[(
            "m.feature",
            "Feature: m\n  Scenario: s\n    Given I remember two things\n",
        )],
        Some(
            "# a comment, so the definition is not on line 1\n\n- step: 'I remember two things'\n  do:\n    - set variable \"a\" to \"1\"\n    - set variable \"b\" to \"2\"\n",
        ),
    );
    let (out, stream, _) = run_with_events(&cfg, &[]);

    assert_eq!(out.status.code(), Some(0));
    let started = of_type(&stream, "step_started");
    assert_eq!(started.len(), 3, "{stream:?}");
    assert_eq!(
        (started[0]["step"].as_u64(), started[0].get("parent")),
        (Some(0), None)
    );
    for (position, child) in started[1..].iter().enumerate() {
        assert_eq!(child["step"], position + 1);
        assert_eq!(child["parent"], 0);
        assert_eq!(child["index"], position);
        assert_eq!(child["keyword"], "");
        assert_eq!(child["line"], 3, "the definition's line: {child}");
        assert!(
            child["source"]
                .as_str()
                .expect("source")
                .ends_with("macros/m.yaml")
        );
    }
    // The caller closes after its body.
    let finished: Vec<_> = of_type(&stream, "step_finished")
        .iter()
        .map(|e| e["step"].as_u64())
        .collect();
    assert_eq!(finished, [Some(1), Some(2), Some(0)]);
}

#[test]
fn an_included_step_is_an_event_with_its_own_source_and_line() {
    let dir = build_include_project(
        "events-include",
        &[
            (
                "features/target.feature",
                "tests/features/include/target.feature",
            ),
            (
                "features/caller.feature",
                "tests/features/include/caller.feature",
            ),
        ],
    );
    let events = dir.join("events.ndjson");
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config", "cfg.yaml", "--events"])
        .arg(&events)
        .current_dir(&dir)
        .output()
        .expect("run bddkit");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );

    // caller.feature sorts before target.feature, and runs as its own file.
    let stream = read_stream(&events);
    let caller = of_type(&stream, "step_started")
        .into_iter()
        .filter(|e| {
            e["file"]
                .as_str()
                .is_some_and(|f| f.ends_with("caller.feature"))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        caller.len(),
        3,
        "include, the included step, the assertion: {caller:?}"
    );
    let included = caller[1];
    assert_eq!(included["parent"], 0);
    assert_eq!(included["line"], 4, "its own line in the included file");
    assert!(
        included["source"]
            .as_str()
            .expect("source")
            .ends_with("target.feature")
    );
    assert_eq!(included["text"], "set variable \"userId\" to \"abc123\"");
    assert_eq!(caller[2].get("parent"), None, "back at the top level");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_stream_and_the_junit_report_agree_on_every_total() {
    let base = common::spawn().await;
    let cfg = write_report_project(&base, "events-totals");
    std::fs::write(
        cfg.with_file_name("features/second.feature"),
        "Feature: second\n  Scenario: one\n    When I request \"/ping\"\n    Then the response code is 404\n\n  Scenario: two\n    When I request \"/ping\"\n",
    )
    .expect("second feature");
    let junit = cfg.with_file_name("junit.xml");
    let (_, stream, _) = run_with_events(&cfg, &["--junit", junit.to_str().expect("UTF-8")]);

    let xml = std::fs::read_to_string(&junit).expect("junit");
    let package = sxd_document::parser::parse(&xml).expect("junit parses");
    let value = |xpath: &str| {
        sxd_xpath::evaluate_xpath(&package.as_document(), xpath)
            .expect("xpath")
            .string()
    };
    let last = stream.last().expect("lines");
    assert_eq!(last["files"].to_string(), value("count(//testsuite)"));
    assert_eq!(last["scenarios"].to_string(), value("count(//testcase)"));
    assert_eq!(last["failed"].to_string(), value("count(//failure)"));
    assert_eq!(
        (
            last["files"].as_u64(),
            last["scenarios"].as_u64(),
            last["failed"].as_u64()
        ),
        (Some(2), Some(4), Some(2))
    );
    for suite in of_type(&stream, "file_finished") {
        let file = suite["file"].as_str().expect("file");
        let name = file.rsplit('/').next().expect("name");
        assert_eq!(
            suite["failed"].to_string(),
            value(&format!(
                "string(//testsuite[contains(@name,'{name}')]/@failures)"
            )),
            "{suite}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_run_without_events_prints_the_same_console_output() {
    let base = common::spawn().await;
    let cfg = write_report_project(&base, "events-console");
    let events = cfg.with_file_name("events.ndjson");
    let normalised = |out: &std::process::Output| {
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let run_id = stdout
            .lines()
            .find_map(|line| line.strip_prefix("run "))
            .expect("the run id line")
            .to_string();
        (stdout.replace(&run_id, "<run>"), out.status.code())
    };
    let plain = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(&cfg)
        .output()
        .expect("run bddkit");
    let streamed = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(&cfg)
        .arg("--events")
        .arg(&events)
        .output()
        .expect("run bddkit");

    assert_eq!(normalised(&plain), normalised(&streamed));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_null_sentinel_in_a_failure_is_written_as_the_null_slot() {
    let base = common::spawn().await;
    let cfg = write_report_project(&base, "events-null");
    let (_, stream, events) = run_with_events(&cfg, &[]);

    let failure = of_type(&stream, "scenario_finished")
        .into_iter()
        .find_map(|e| e["failure"].as_str())
        .expect("the failing scenario carries its failure");
    assert!(failure.contains("<<null>>"), "{failure}");
    let raw = std::fs::read(&events).expect("read");
    assert!(!raw.contains(&0), "no NUL byte reaches the stream");
    assert!(
        !String::from_utf8_lossy(&raw).contains("\\u0000"),
        "nor its JSON escape"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_events_path_can_come_from_the_environment_and_the_flag_wins() {
    let base = common::spawn().await;
    let cfg = write_report_project(&base, "events-env");
    let from_env = cfg.with_file_name("from-env.ndjson");
    let from_flag = cfg.with_file_name("from-flag.ndjson");

    Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(&cfg)
        .env("BDDKIT_EVENTS", &from_env)
        .output()
        .expect("run bddkit");
    assert_eq!(read_stream(&from_env)[0]["type"], "run_started");

    let _ = std::fs::remove_file(&from_env);
    Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(&cfg)
        .arg("--events")
        .arg(&from_flag)
        .env("BDDKIT_EVENTS", &from_env)
        .output()
        .expect("run bddkit");
    assert_eq!(read_stream(&from_flag)[0]["type"], "run_started");
    assert!(
        !from_env.exists(),
        "the flag wins; the variable is not opened"
    );

    // A templated `BDDKIT_EVENTS=` is "off", not a path to fail on.
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(&cfg)
        .env("BDDKIT_EVENTS", "")
        .output()
        .expect("run bddkit");
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The reader opens first and blocks until the run opens its end; end-of-file
/// would come early if the path were opened, closed and opened again.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn events_can_be_written_to_a_fifo() {
    let base = common::spawn().await;
    let cfg = write_report_project(&base, "events-fifo");
    let fifo = cfg.with_file_name("events.fifo");
    mkfifo(&fifo);

    let reader = {
        let fifo = fifo.clone();
        std::thread::spawn(move || std::fs::read_to_string(fifo).expect("read the fifo"))
    };
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(&cfg)
        .arg("--events")
        .arg(&fifo)
        .output()
        .expect("run bddkit");
    let text = reader.join().expect("reader");

    assert_eq!(out.status.code(), Some(1));
    let lines: Vec<Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).expect("one JSON object per line"))
        .collect();
    assert_eq!(lines[0]["type"], "run_started");
    assert_eq!(lines.last().expect("lines")["type"], "run_finished");
}

/// `/dev/full` opens for writing and fails every write with ENOSPC.
#[cfg(target_os = "linux")]
#[test]
fn a_stream_that_cannot_be_written_turns_the_exit_code_to_two() {
    let cfg = write_events_project(
        "full",
        "",
        &[(
            "a.feature",
            "Feature: a\n  Scenario: s\n    Given set variable \"x\" to \"1\"\n",
        )],
        None,
    );
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(&cfg)
        .args(["--events", "/dev/full"])
        .output()
        .expect("run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stdout}\n{stderr}");
    assert!(stderr.contains("cannot write the events file"), "{stderr}");
    assert!(
        stdout.contains("failed: 0"),
        "the run itself completed: {stdout}"
    );
}

/// A run held in a `sleep` is interrupted once its first step is on disk.
/// Whatever the files in flight emit after the signal, the last line of the
/// stream is the terminator, and there is exactly one.
#[cfg(unix)]
#[test]
fn an_interrupted_run_ends_its_stream_with_the_signal() {
    for (signal, code, name) in [
        ("INT", 130, "SIGINT"),
        ("TERM", 143, "SIGTERM"),
        ("HUP", 129, "SIGHUP"),
    ] {
        let feature = "Feature: held\n  Scenario: s\n    Given I sleep \"10\" seconds\n";
        let cfg = write_events_project(
            &format!("signal-{signal}"),
            "",
            &[("a.feature", feature), ("b.feature", feature)],
            None,
        );
        let events = cfg.with_file_name("events.ndjson");
        let mut child = Command::new(env!("CARGO_BIN_EXE_bddkit"))
            .args(["run", "--config"])
            .arg(&cfg)
            .arg("--events")
            .arg(&events)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn bddkit");

        // Both files are inside their sleep when the signal lands.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while std::fs::read_to_string(&events)
            .map_or(0, |text| text.matches("step_started").count())
            < 2
        {
            assert!(
                std::time::Instant::now() < deadline,
                "the steps never started"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let sent = Command::new("kill")
            .args([&format!("-{signal}"), &child.id().to_string()])
            .status()
            .expect("kill");
        assert!(sent.success());
        let status = child.wait().expect("wait");

        assert_eq!(status.code(), Some(code), "{name}");
        let stream = read_stream(&events);
        let last = stream.last().expect("lines");
        assert_eq!(last["type"], "run_finished", "{name}: {stream:?}");
        assert_eq!(last["signal"], name);
        assert_eq!(last["exit"], code);
        assert_eq!(of_type(&stream, "run_finished").len(), 1, "{name}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doctor_reports_the_events_path() {
    let base = common::spawn().await;
    let cfg = write_doctor_project(
        "doctor-events",
        &base,
        "Feature: f\n  Scenario: s\n    Given set variable \"x\" to \"1\"\n",
        "",
    );
    let events = cfg.with_file_name("out/events.ndjson");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config"])
        .arg(&cfg)
        .arg("--events")
        .arg(&events)
        .output()
        .expect("run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("reports"), "{stdout}");
    assert!(stdout.contains("events.ndjson"), "{stdout}");
    assert_eq!(std::fs::read_to_string(&events).expect("created"), "");

    // A path that cannot be created is a failed row, exit 1.
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config"])
        .arg(&cfg)
        .arg("--events")
        .arg(cfg.join("events.ndjson"))
        .output()
        .expect("run bddkit");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("✗ reports"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// Opening a FIFO for writing blocks until a reader shows up, and there is
/// none: a doctor that opened it would hang here.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doctor_leaves_an_events_path_that_is_not_a_regular_file_unopened() {
    let base = common::spawn().await;
    let cfg = write_doctor_project(
        "doctor-events-fifo",
        &base,
        "Feature: f\n  Scenario: s\n    Given set variable \"x\" to \"1\"\n",
        "",
    );
    let fifo = cfg.with_file_name("events.fifo");
    mkfifo(&fifo);

    let mut child = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config"])
        .arg(&cfg)
        .arg("--events")
        .arg(&fifo)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("spawn bddkit");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while child.try_wait().expect("poll").is_none() {
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            panic!("doctor is blocked opening the FIFO");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let out = child.wait_with_output().expect("output");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(stdout.contains("left unopened"), "{stdout}");
}

fn mkfifo(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
    assert!(
        Command::new("mkfifo")
            .arg(path)
            .status()
            .expect("mkfifo")
            .success()
    );
}

/// `run` would exit 2 on a directory, so `doctor` must not call it fine.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doctor_refuses_an_events_path_that_is_a_directory() {
    let base = common::spawn().await;
    let cfg = write_doctor_project(
        "doctor-events-dir",
        &base,
        "Feature: f\n  Scenario: s\n    Given set variable \"x\" to \"1\"\n",
        "",
    );
    let dir = cfg.with_file_name("a-directory");
    std::fs::create_dir_all(&dir).expect("mkdir");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config"])
        .arg(&cfg)
        .arg("--events")
        .arg(&dir)
        .output()
        .expect("run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("✗ reports"), "{stdout}");
}

/// A reader that holds the FIFO open and never reads stalls the writer once the
/// pipe's buffer is full. The run still completes — the channel is unbounded —
/// but its last step, waiting for the writer, must not make it unkillable.
#[cfg(unix)]
#[test]
fn a_second_signal_stops_a_run_whose_stream_reader_has_stalled() {
    use std::io::BufRead;

    let steps = "    Given set variable \"x\" to \"1\"\n".repeat(400);
    let cfg = write_events_project(
        "stalled",
        "",
        &[("a.feature", &format!("Feature: a\n  Scenario: s\n{steps}"))],
        None,
    );
    let fifo = cfg.with_file_name("events.fifo");
    mkfifo(&fifo);
    // `exec` keeps the pid: killing the child kills the sleep itself.
    let mut reader = Command::new("sh")
        .args(["-c", "exec sleep 120 < \"$0\""])
        .arg(&fifo)
        .spawn()
        .expect("spawn the stalled reader");
    let mut run = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(&cfg)
        .arg("--events")
        .arg(&fifo)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn bddkit");

    // The summary is printed before the stream is closed: the run is now in
    // the tail that waits for the writer.
    let stdout = run.stdout.take().expect("stdout");
    let summary = std::io::BufReader::new(stdout)
        .lines()
        .map_while(Result::ok)
        .any(|line| line.starts_with("files:"));
    assert!(summary, "the run never printed its summary");
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(
        run.try_wait().expect("poll").is_none(),
        "the stream is over 64 KiB, so the writer is stalled and the run waits for it"
    );

    // A first signal only says "still writing": the queued lines are not
    // thrown away on one Ctrl-C.
    let pid = run.id().to_string();
    let signal = |name: &str| {
        let sent = Command::new("kill")
            .args([name, &pid])
            .status()
            .expect("kill");
        assert!(sent.success());
    };
    signal("-TERM");
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(
        run.try_wait().expect("poll").is_none(),
        "one signal must not drop the stream still being written"
    );
    // A second one is "stop now", with the first one's code.
    signal("-INT");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let status = loop {
        if let Some(status) = run.try_wait().expect("poll") {
            break Some(status);
        }
        if std::time::Instant::now() > deadline {
            let _ = run.kill();
            break None;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let _ = reader.kill();
    let _ = reader.wait();
    let _ = run.wait();

    assert_eq!(
        status.and_then(|s| s.code()),
        Some(143),
        "a second signal must stop a run that is only waiting for its stream"
    );
}

/// The handlers are registered before `run <id>` is printed, so a signal sent
/// the moment that line is readable is queued even if the handler task has not
/// been polled yet — it must end the stream with the signal, never leave it
/// holding `run_started` alone. A race cannot be forced, so this repeats it:
/// before the handlers were registered up front, a SIGTERM landing in that
/// window killed the process outright.
#[cfg(unix)]
#[test]
fn a_signal_right_after_the_run_id_still_terminates_the_stream() {
    use std::io::BufRead;

    let cfg = write_events_project(
        "early-signal",
        "",
        &[(
            "a.feature",
            "Feature: a\n  Scenario: s\n    Given I sleep \"10\" seconds\n",
        )],
        None,
    );
    let events = cfg.with_file_name("events.ndjson");
    for attempt in 0..3 {
        let mut child = Command::new(env!("CARGO_BIN_EXE_bddkit"))
            .args(["run", "--config"])
            .arg(&cfg)
            .arg("--events")
            .arg(&events)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn bddkit");
        let mut first = String::new();
        std::io::BufReader::new(child.stdout.take().expect("stdout"))
            .read_line(&mut first)
            .expect("the run id line");
        assert!(first.starts_with("run "), "{first:?}");
        let sent = Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status()
            .expect("kill");
        assert!(sent.success());
        let status = child.wait().expect("wait");

        assert_eq!(status.code(), Some(143), "attempt {attempt}");
        let stream = read_stream(&events);
        let last = stream.last().expect("a stream");
        assert_eq!(
            last["type"], "run_finished",
            "attempt {attempt}: {stream:?}"
        );
        assert_eq!(last["signal"], "SIGTERM", "attempt {attempt}");
    }
}

/// `files` feature files of `scenarios` scenarios of `steps` steps each, none
/// of which touches the network: the cost is the runner and the stream.
fn load_features(files: usize, scenarios: usize, steps: usize) -> Vec<(String, String)> {
    let body = "    Given set variable \"x\" to \"1\"\n".repeat(steps);
    (0..files)
        .map(|file| {
            let mut text = format!("Feature: load {file}\n");
            for scenario in 0..scenarios {
                text.push_str(&format!("  Scenario: s{scenario}\n{body}"));
            }
            (format!("f{file:04}.feature"), text)
        })
        .collect()
}

/// Four workers, not the default eight: a load test must not take the CPU the
/// timing-sensitive tests running beside it are counting on.
fn write_load_project(name: &str, features: &[(String, String)]) -> std::path::PathBuf {
    let pairs: Vec<(&str, &str)> = features
        .iter()
        .map(|(file, text)| (file.as_str(), text.as_str()))
        .collect();
    let cfg = write_events_project(name, "", &pairs, None);
    let mut config = std::fs::read_to_string(&cfg).expect("read config");
    config.push_str("concurrency: 4\n");
    std::fs::write(&cfg, config).expect("write config");
    cfg
}

/// What holds for any stream, even one cut short: every line parsed (the
/// reader did that), `seq` has no gap and `t` never goes back.
fn assert_stream_is_ordered(stream: &[Value]) {
    for (index, event) in stream.iter().enumerate() {
        assert_eq!(event["seq"], index, "a gap or a repeat in seq: {event}");
        if index > 0 {
            assert!(
                event["t"].as_u64() >= stream[index - 1]["t"].as_u64(),
                "t goes back: {event}"
            );
        }
    }
}

/// A stream that ended: the only `run_finished` is the last line.
fn assert_stream_is_sound(stream: &[Value]) {
    assert_stream_is_ordered(stream);
    assert_eq!(of_type(stream, "run_finished").len(), 1);
    assert_eq!(stream.last().expect("lines")["type"], "run_finished");
}

/// 40 files x 2 scenarios x 20 steps, four at a time: enough volume for the
/// ordering to mean something, a fraction of a second to run. That the files
/// really overlap is the barrier test's job, which proves it without timing.
#[test]
fn a_wide_run_streams_every_event_in_order() {
    let (files, scenarios, steps) = (40, 2, 20);
    let cfg = write_load_project("load", &load_features(files, scenarios, steps));
    let (out, stream, _) = run_with_events(&cfg, &[]);

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert_stream_is_sound(&stream);
    assert!(
        stdout.contains(&format!(
            "files: {files}, scenarios: {}, failed: 0",
            files * scenarios
        )),
        "{stdout}"
    );
    let last = stream.last().expect("lines");
    assert_eq!(
        (
            last["files"].as_u64(),
            last["scenarios"].as_u64(),
            last["failed"].as_u64()
        ),
        (
            Some(files as u64),
            Some((files * scenarios) as u64),
            Some(0)
        )
    );
    let total_steps = files * scenarios * steps;
    assert_eq!(of_type(&stream, "step_started").len(), total_steps);
    assert_eq!(of_type(&stream, "step_finished").len(), total_steps);

    // One file's lines are in causal order, whatever the others were doing.
    let mut by_file: std::collections::HashMap<&str, Vec<&Value>> =
        std::collections::HashMap::new();
    for event in &stream {
        if let Some(file) = event["file"].as_str() {
            by_file.entry(file).or_default().push(event);
        }
    }
    assert_eq!(by_file.len(), files);
    for (file, events) in &by_file {
        assert_eq!(events[0]["type"], "file_started", "{file}");
        assert_eq!(
            events.last().expect("events")["type"],
            "file_finished",
            "{file}"
        );
        let (mut in_scenario, mut open) = (false, 0_i32);
        for event in events {
            match event["type"].as_str().expect("type") {
                "scenario_started" => {
                    assert!(!in_scenario, "{file}: a scenario inside a scenario");
                    in_scenario = true;
                }
                "step_started" => {
                    assert!(in_scenario, "{file}: a step outside a scenario");
                    open += 1;
                }
                "step_finished" => {
                    assert!(open > 0, "{file}: a step closed twice");
                    open -= 1;
                }
                "scenario_finished" => {
                    assert!(in_scenario && open == 0, "{file}: closed with a step open");
                    in_scenario = false;
                }
                _ => {}
            }
        }
    }
}

/// An anchor file holds the run in a sleep, so when the first busy file has
/// finished the signal always finds the handler first to the claim and other
/// files still emitting. Nothing here depends on how fast the machine is.
#[cfg(unix)]
#[test]
fn interrupting_a_busy_run_leaves_a_terminated_stream() {
    let mut features = load_features(40, 1, 50);
    features.push((
        "anchor.feature".into(),
        "@priority(1)\nFeature: anchor\n  Scenario: hold\n    Given I sleep \"10\" seconds\n"
            .into(),
    ));
    let cfg = write_load_project("load-signal", &features);
    let events = cfg.with_file_name("events.ndjson");
    let mut child = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(&cfg)
        .arg("--events")
        .arg(&events)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn bddkit");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !std::fs::read_to_string(&events).is_ok_and(|text| text.contains("file_finished")) {
        assert!(
            std::time::Instant::now() < deadline,
            "no file ever finished"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let sent = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .expect("kill");
    assert!(sent.success());
    let status = child.wait().expect("wait");

    assert_eq!(status.code(), Some(143));
    let stream = read_stream(&events);
    assert_stream_is_sound(&stream);
    let last = stream.last().expect("lines");
    assert_eq!(
        (last["signal"].as_str(), last["exit"].as_u64()),
        (Some("SIGTERM"), Some(143))
    );
    assert_eq!(
        last["files"].as_u64(),
        Some(of_type(&stream, "file_finished").len() as u64),
        "the totals are those of the files written before the terminator"
    );
    assert!(
        of_type(&stream, "file_started").len() > of_type(&stream, "file_finished").len(),
        "the anchor was still in flight"
    );
}
