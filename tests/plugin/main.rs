//! End-to-end acceptance for the plugin layer: the real `bddkit` binary, run
//! as a subprocess, against the fixture plugin built by `common::build_fixture_plugin`.
//! No test here needs the axum stub, so plain `#[test]` is right — nothing
//! spawns a server for `Command::output()` to starve.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    reason = "`allow-*-in-tests` covers only `#[test]` functions, not the helpers here"
)]

mod abi;
mod artifacts;
mod cli_surfaces;
#[path = "../common/mod.rs"]
mod common;
mod diagnostics;
mod dispatch_keys;
mod evidence;
mod expressions;
mod failure_class;
mod failure_context;
mod groups;
mod includes;
mod layers;
mod lifecycle;
mod polling;
mod privacy;
mod signals;
mod validate;

use std::path::{Path, PathBuf};
use std::process::Command;

/// A throwaway project: config, feature file, and the hand-written lock file
/// the run-path loader reads — the hand-written form `plugin install` also
/// writes, exercised by `tests/plugin_cli/` instead of here.
fn project(name: &str, feature: &str, config_tail: &str) -> PathBuf {
    project_at(name, feature, config_tail, 1)
}

/// `project` with the run's worker count spelled out. Every test that does not
/// care runs sequentially through `project`; the parallel case needs its own
/// value, and a suite that can only express `concurrency: 1` is exactly why the
/// broadcast reset survived thirteen reviews.
fn project_at(name: &str, feature: &str, config_tail: &str, concurrency: usize) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bddkit-plugin-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("features")).expect("mkdir features");
    std::fs::create_dir_all(dir.join(".bddkit")).expect("mkdir .bddkit");
    std::fs::write(dir.join("features/plugin.feature"), feature).expect("write feature");
    std::fs::write(
        dir.join(".bddkit/plugins.yaml"),
        format!(
            "plugin:\n  - name: echo\n    path: {}\n",
            common::build_fixture_plugin().display()
        ),
    )
    .expect("write lock");
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "paths: [features]\nconcurrency: {concurrency}\nresources:\n  api: {{}}\n{config_tail}"
        ),
    )
    .expect("write config");
    dir
}

/// `project_at` for the `per_worker` fixture: several feature files instead of
/// one, and a lock naming the worker plugin. A separate helper rather than a
/// parameter on `project_at`, so the tests that predate this milestone keep
/// exactly the layout they were written against.
fn worker_project(
    name: &str,
    files: &[(&str, &str)],
    config_tail: &str,
    concurrency: usize,
) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bddkit-plugin-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("features")).expect("mkdir features");
    std::fs::create_dir_all(dir.join(".bddkit")).expect("mkdir .bddkit");
    for (file, body) in files {
        std::fs::write(dir.join("features").join(file), body).expect("write feature");
    }
    std::fs::write(
        dir.join(".bddkit/plugins.yaml"),
        format!(
            "plugin:\n  - name: worker\n    path: {}\n",
            common::build_worker_plugin().display()
        ),
    )
    .expect("write lock");
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "paths: [features]\nconcurrency: {concurrency}\nresources:\n  api: {{}}\n{config_tail}"
        ),
    )
    .expect("write config");
    dir
}

fn run(dir: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config", "cfg.yaml"])
        // `--config cfg.yaml` has no parent directory component, so the lock
        // is looked for next to it — i.e. inside this project directory.
        .current_dir(dir)
        .output()
        .expect("failed to run bddkit")
}

/// `run` with extra arguments and environment, for the tests that steer the
/// artifact root.
fn run_with(dir: &Path, args: &[&str], envs: &[(&str, &str)]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config", "cfg.yaml"])
        .args(args)
        // A developer's own export must not steer the default-root assertions.
        .env_remove("BDDKIT_ARTIFACTS_DIR")
        .envs(envs.iter().copied())
        .current_dir(dir)
        .output()
        .expect("failed to run bddkit")
}

fn assert_green(out: &std::process::Output) {
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
}

const ECHO_GROUP: &str = "  echo:\n    main:\n      prefix: \"p-\"\n";

// ---- --evidence: what a plugin's reply puts into the event stream ----

/// The events written by a run with `--events <dir>/events.ndjson --evidence`.
fn evidence_stream(dir: &Path, extra: &[&str]) -> Vec<serde_json::Value> {
    let mut args = vec!["--events", "events.ndjson", "--evidence"];
    args.extend_from_slice(extra);
    assert_green(&run_with(dir, &args, &[]));
    std::fs::read_to_string(dir.join("events.ndjson"))
        .expect("the stream exists")
        .lines()
        .map(|line| serde_json::from_str(line).expect("one JSON object per line"))
        .collect()
}

/// `project` with `macros/m.yaml` holding `yaml`.
fn macro_project(name: &str, feature: &str, yaml: &str) -> PathBuf {
    let dir = project(name, feature, "macro_paths: [macros]\n");
    std::fs::create_dir_all(dir.join("macros")).unwrap();
    std::fs::write(dir.join("macros/m.yaml"), yaml).unwrap();
    dir
}

const MACRO_CALL: &str = "Feature: f\n  Scenario: s\n    When I do the checked thing\n";

const CHECKED_MACRO: &str = "- step: I do the checked thing\n  do: [I echo validated \"bad\"]\n";

fn doctor(dir: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config", "cfg.yaml"])
        .current_dir(dir)
        .output()
        .expect("run doctor")
}

const INCLUDE: &str = "    Given I include \"helper.feature\"\n";

// ---- Structured diagnostics and `scan` in a plugin's reply (ABI 4) ----

/// A feature whose one step makes the echo plugin answer with `reply`.
fn answers(reply: &serde_json::Value) -> String {
    format!(
        "Feature: f\n  Scenario: s\n    Then the echo answers:\n      \"\"\"\n      {reply}\n      \"\"\"\n"
    )
}

fn stdout_of(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn read_stream(dir: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(dir.join("events.ndjson"))
        .expect("the stream exists")
        .lines()
        .map(|line| serde_json::from_str(line).expect("one JSON object per line"))
        .collect()
}

// ---- Dispatch keys of ABI 4: class, attempt, options_instance, output, global_vars, source_dir ----

fn polled(script: &serde_json::Value, within: &str) -> String {
    format!(
        "Feature: f\n  Scenario: s\n    When I expect the next assertion to pass within \"{within}\" seconds, checking every \"20\" milliseconds\n    Then the echo answers by attempt:\n      \"\"\"\n      {script}\n      \"\"\"\n"
    )
}
