//! Why a scenario failed, as data: the class of each failure in the console
//! summary, the event stream and the JUnit report. The class is set where the
//! error is born (`src/failure.rs`); these tests drive each birthplace through
//! the real binary and read the class back out of the output.
//!
//! The core classes run over core steps only. `a_refused_connection_is_a_transport_failure`
//! needs a real HTTP request and is marked to move with the HTTP API.

mod common;

use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Output};

/// A project with one feature file and, optionally, one macro file.
fn project(name: &str, feature: &str, macros: Option<&str>) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bddkit-class-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(dir.join("features/f.feature"), feature).expect("write feature");
    let mut config = String::from("paths: [features]\nresources: {}\n");
    if let Some(macros) = macros {
        std::fs::create_dir_all(dir.join("macros")).expect("mkdir");
        std::fs::write(dir.join("macros/m.yaml"), macros).expect("write macros");
        config.push_str("macro_paths: [macros]\n");
    }
    std::fs::write(dir.join("cfg.yaml"), config).expect("write config");
    dir
}

fn run(dir: &std::path::Path, extra: &[&str]) -> Output {
    // A project that carries its own plugins never sees the developer's.
    let own = dir.join(".bddkit");
    let bddkit_dir: Vec<&std::ffi::OsStr> = if own.is_dir() {
        vec!["--bddkit-dir".as_ref(), own.as_os_str()]
    } else {
        vec![]
    };
    Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .current_dir(dir)
        .args(["run", "--config", "cfg.yaml"])
        .args(bddkit_dir)
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

/// `project` with an API at `base`, for the one test that needs a real request.
// moves-to: api
#[cfg(not(feature = "dev-unreserve"))]
fn http_project(name: &str, base: &str, feature: &str) -> PathBuf {
    let dir = project(name, feature, None);
    let config = std::fs::read_to_string(dir.join("cfg.yaml")).expect("config");
    let config = config.replace(
        "resources: {}\n",
        &format!("resources:\n  api:\n    stub:\n      base_url: {base}\n      timeout_secs: 2\n"),
    );
    std::fs::write(dir.join("cfg.yaml"), config).expect("write config");
    dir
}

// moves-to: api
#[test]
#[cfg(not(feature = "dev-unreserve"))]
fn a_refused_connection_is_a_transport_failure() {
    // Port 1 on loopback: nothing listens, the connection is refused.
    let dir = http_project(
        "refused",
        "http://127.0.0.1:1",
        "Feature: f\n  Scenario: s\n    When I request \"/ping\"\n",
    );
    assert!(summary(&dir).ends_with("failed: 1 (1 transport)"));
}

const MISMATCH: &str = "Feature: f\n  Scenario: s\n    Given set variable \"a\" to \"1\"\n    Then variable \"a\" should be equal to \"2\"\n";

#[test]
fn a_mismatch_is_an_assertion_failure() {
    let dir = project("status", MISMATCH, None);
    assert!(summary(&dir).ends_with("failed: 1 (1 assertion)"));
}

/// A step that answers `not_yet` is retried until the time is up, and the
/// failure keeps the class of its last attempt. Variable assertions are final
/// at once, so this needs the echo fixture's `not_yet` assertion.
#[test]
fn polling_that_runs_out_is_an_assertion_failure() {
    let dir = project(
        "polling",
        "Feature: f\n  Scenario: s\n    Given I expect the next assertion to pass within \"1\" seconds, checking every \"100\" milliseconds\n    Then the echo should not be ready yet declaring \"x\" secret\n",
        None,
    );
    std::fs::create_dir_all(dir.join(".bddkit")).expect("mkdir .bddkit");
    std::fs::write(
        dir.join(".bddkit/plugins.yaml"),
        format!(
            "plugin:\n  - name: echo\n    path: {}\n",
            common::build_fixture_plugin().display()
        ),
    )
    .expect("write lock");
    let mut config = std::fs::read_to_string(dir.join("cfg.yaml")).expect("config");
    config.push_str("concurrency: 1\n");
    std::fs::write(dir.join("cfg.yaml"), config).expect("write config");
    let started = std::time::Instant::now();
    assert!(summary(&dir).ends_with("failed: 1 (1 assertion)"));
    assert!(
        started.elapsed() >= std::time::Duration::from_secs(1),
        "the assertion was retried until the time was up"
    );
}

#[test]
fn an_undefined_variable_is_an_input_failure() {
    let dir = project(
        "undefined",
        "Feature: f\n  Scenario: s\n    Given set variable \"a\" to \"<<nowhere>>\"\n",
        None,
    );
    assert!(summary(&dir).ends_with("failed: 1 (1 input)"));
}

#[test]
fn a_typed_parameter_that_does_not_fit_is_an_input_failure() {
    let dir = project(
        "typed",
        "Feature: f\n  Scenario: s\n    Given set variable \"code\" to \"abc\"\n    When I sleep <<code>> seconds\n",
        None,
    );
    assert!(summary(&dir).ends_with("failed: 1 (1 input)"));
}

#[test]
fn a_failure_inside_a_macro_keeps_its_class() {
    let dir = project(
        "macro",
        "Feature: f\n  Scenario: s\n    When I check twice\n",
        Some(
            "- step: I check twice\n  do:\n    - set variable \"a\" to \"1\"\n    - set variable \"b\" to \"<<nowhere>>\"\n",
        ),
    );
    // `input`, not the default `assertion`: the class survives the macro wrap.
    assert!(summary(&dir).ends_with("failed: 1 (1 input)"));
}

#[test]
fn the_junit_failure_carries_its_type() {
    // `input`, not the default `assertion`, so a writer that hard-codes it fails.
    let dir = project(
        "junit",
        "Feature: f\n  Scenario: s\n    Given set variable \"a\" to \"<<nowhere>>\"\n",
        None,
    );
    run(&dir, &["--junit", "junit.xml"]);
    let xml = std::fs::read_to_string(dir.join("junit.xml")).expect("junit written");
    assert!(xml.contains("<failure type=\"input\" message="), "{xml}");
}

#[test]
fn the_stream_names_the_class_of_a_failed_step_and_scenario() {
    let dir = project(
        "stream",
        "Feature: f\n  Scenario: s\n    Given set variable \"a\" to \"1\"\n    Then variable \"a\" should be equal to \"2\"\n",
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
