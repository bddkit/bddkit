//! Why a scenario failed, as data: the class of each failure in the console
//! summary, the event stream and the JUnit report. The class is set where the
//! error is born (`src/failure.rs`); these tests drive each birthplace through
//! the real binary and read the class back out of the output.
//! Multi-thread runtime: the stub must be polled while `Command::output()` blocks.

mod common;

use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Output};

/// A project with one API at `base`, one feature file and, optionally, one
/// macro file.
fn project(name: &str, base: &str, feature: &str, macros: Option<&str>) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bddkit-class-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(dir.join("features/f.feature"), feature).expect("write feature");
    let mut config = String::from("paths: [features]\n");
    if let Some(macros) = macros {
        std::fs::create_dir_all(dir.join("macros")).expect("mkdir");
        std::fs::write(dir.join("macros/m.yaml"), macros).expect("write macros");
        config.push_str("macro_paths: [macros]\n");
    }
    config.push_str(&format!(
        "resources:\n  api:\n    stub:\n      base_url: {base}\n      timeout_secs: 2\n"
    ));
    std::fs::write(dir.join("cfg.yaml"), config).expect("write config");
    dir
}

fn run(dir: &std::path::Path, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .current_dir(dir)
        .args(["run", "--config", "cfg.yaml"])
        .args(extra)
        .output()
        .expect("run bddkit")
}

/// Runs the project and returns the console's summary line.
fn summary(dir: &std::path::Path) -> String {
    let out = run(dir, &[]);
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    stdout
        .lines()
        .find(|line| line.starts_with("files:"))
        .unwrap_or_else(|| panic!("no summary line in {stdout}"))
        .to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_connection_is_a_transport_failure() {
    // Port 1 on loopback: nothing listens, the connection is refused.
    let dir = project(
        "refused",
        "http://127.0.0.1:1",
        "Feature: f\n  Scenario: s\n    When I request \"/ping\"\n",
        None,
    );
    assert!(summary(&dir).ends_with("failed: 1 (1 transport)"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_status_mismatch_is_an_assertion_failure() {
    let base = common::spawn().await;
    let dir = project(
        "status",
        &base,
        "Feature: f\n  Scenario: s\n    When I request \"/ping\"\n    Then the response code is 404\n",
        None,
    );
    assert!(summary(&dir).ends_with("failed: 1 (1 assertion)"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn polling_that_runs_out_is_an_assertion_failure() {
    let base = common::spawn().await;
    let dir = project(
        "polling",
        &base,
        "Feature: f\n  Scenario: s\n    When I request \"/ping\"\n    And I expect the next assertion to pass within \"1\" seconds, checking every \"100\" milliseconds\n    Then the response code is 404\n",
        None,
    );
    assert!(summary(&dir).ends_with("failed: 1 (1 assertion)"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_undefined_variable_is_an_input_failure() {
    let base = common::spawn().await;
    let dir = project(
        "undefined",
        &base,
        "Feature: f\n  Scenario: s\n    When I request \"/<<nowhere>>\"\n",
        None,
    );
    assert!(summary(&dir).ends_with("failed: 1 (1 input)"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_typed_parameter_that_does_not_fit_is_an_input_failure() {
    let base = common::spawn().await;
    let dir = project(
        "typed",
        &base,
        "Feature: f\n  Scenario: s\n    Given set variable \"code\" to \"abc\"\n    When I request \"/ping\"\n    Then the response code is <<code>>\n",
        None,
    );
    assert!(summary(&dir).ends_with("failed: 1 (1 input)"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failure_inside_a_macro_keeps_its_class() {
    let dir = project(
        "macro",
        "http://127.0.0.1:1",
        "Feature: f\n  Scenario: s\n    When I ping twice\n",
        Some("- step: I ping twice\n  do:\n    - I request \"/ping\"\n"),
    );
    assert!(summary(&dir).ends_with("failed: 1 (1 transport)"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_junit_failure_carries_its_type() {
    let dir = project(
        "junit",
        "http://127.0.0.1:1",
        "Feature: f\n  Scenario: s\n    When I request \"/ping\"\n",
        None,
    );
    run(&dir, &["--junit", "junit.xml"]);
    let xml = std::fs::read_to_string(dir.join("junit.xml")).expect("junit written");
    assert!(
        xml.contains("<failure type=\"transport\" message="),
        "{xml}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_stream_names_the_class_of_a_failed_step_and_scenario() {
    let base = common::spawn().await;
    let dir = project(
        "stream",
        &base,
        "Feature: f\n  Scenario: s\n    When I request \"/ping\"\n    Then the response code is 404\n",
        None,
    );
    run(&dir, &["--events", "events.ndjson"]);
    let stream: Vec<Value> = std::fs::read_to_string(dir.join("events.ndjson"))
        .expect("stream written")
        .lines()
        .map(|line| serde_json::from_str(line).expect("one JSON object per line"))
        .collect();
    let of = |kind: &str| -> Vec<&Value> { stream.iter().filter(|e| e["type"] == kind).collect() };
    let steps = of("step_finished");
    assert_eq!(steps[0]["status"], "passed");
    assert!(steps[0].get("kind").is_none(), "{}", steps[0]);
    assert_eq!(steps[1]["kind"], "assertion");
    assert_eq!(of("scenario_finished")[0]["failure_kind"], "assertion");
}
