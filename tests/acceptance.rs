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
        .args(["run", "--config", config.to_str().expect("UTF-8 config path")])
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
    let address = listener.local_addr().expect("eventual-absence stub address");
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
        .args(["run", "--config", config.to_str().expect("UTF-8 config path")])
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
        .args(["run", "--config", config.to_str().expect("UTF-8 config path")])
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
    assert!(
        !out.status.success(),
        "scenario must fail\n{stdout}"
    );
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
    assert!(!stdout.contains("only.feature:4"), "once per file: {stdout}");
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
