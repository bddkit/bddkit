//! What `run_started` says about the environment a run used (#302), and the
//! same names in `doctor --json`.

use super::{path_text, read_stream, write_events_project};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

fn project(name: &str, dotenv: &[(&str, &str)]) -> PathBuf {
    let cfg = write_events_project(
        name,
        &[(
            "a.feature",
            "Feature: a\n  Scenario: s\n    Given set variable \"a\" to \"1\"\n",
        )],
        None,
    );
    for (file, content) in dotenv {
        std::fs::write(cfg.with_file_name(file), content).expect("write .env");
    }
    cfg
}

/// A command that sees nothing of the developer's own choice.
fn bddkit(envs: &[(&str, &str)]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_bddkit"));
    command
        .env_remove("APP_ENV")
        .env_remove("BDDKIT_FAIL_FAST")
        .env_remove("BDDKIT_ARTIFACTS_DIR")
        .env_remove("BDDKIT_EVIDENCE")
        .envs(envs.iter().copied());
    command
}

/// The `run_started` of a run of `cfg`.
fn started_in(dir: Option<&Path>, cfg: &Path, args: &[&str], envs: &[(&str, &str)]) -> Value {
    let events = cfg.with_file_name("events.ndjson");
    let mut command = bddkit(envs);
    if let Some(dir) = dir {
        command.current_dir(dir);
    }
    let out = command
        .args(["run", "--config"])
        .arg(match dir {
            Some(dir) => cfg.strip_prefix(dir).expect("inside"),
            None => cfg,
        })
        .arg("--events")
        .arg(&events)
        .args(args)
        .output()
        .expect("run bddkit");
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    read_stream(&events).remove(0)
}

fn started(cfg: &Path, args: &[&str], envs: &[(&str, &str)]) -> Value {
    started_in(None, cfg, args, envs)
}

#[test]
fn run_started_reports_the_app_env_from_the_flag() {
    let cfg = project("env-flag", &[(".env", "APP_ENV=dotenv\n")]);
    let line = started(&cfg, &["--env", "staging"], &[("APP_ENV", "process")]);
    assert_eq!(line["app_env"], "staging");
}

#[test]
fn run_started_reports_the_app_env_from_the_process_then_dotenv_then_dev() {
    let cfg = project("env-order", &[(".env", "APP_ENV=dotenv\n")]);
    assert_eq!(
        started(&cfg, &[], &[("APP_ENV", "process")])["app_env"],
        "process"
    );
    assert_eq!(started(&cfg, &[], &[])["app_env"], "dotenv");
    std::fs::remove_file(cfg.with_file_name(".env")).expect("remove .env");
    assert_eq!(started(&cfg, &[], &[])["app_env"], "dev");
}

#[test]
fn run_started_lists_only_the_env_layers_that_were_read() {
    let cfg = project(
        "env-layers",
        &[
            (".env", ""),
            (".env.staging.local", "A=1\n"),
            (".env.prod", "A=2\n"),
        ],
    );
    let dir = cfg.parent().expect("dir");
    // Outside the working directory the paths are absolute.
    assert_eq!(
        started(&cfg, &["--env", "staging"], &[])["env_layers"],
        json!([
            path_text(&dir.join(".env")),
            path_text(&dir.join(".env.staging.local"))
        ])
    );
    // Under it they are relative to it, as the config was named or not.
    let above = dir
        .parent()
        .expect("temp dir")
        .canonicalize()
        .expect("real");
    let cfg = above.join(dir.file_name().expect("name")).join("cfg.yaml");
    let folder = path_text(Path::new(dir.file_name().expect("name")));
    assert_eq!(
        started_in(Some(&above), &cfg, &["--env", "staging"], &[])["env_layers"],
        json!([
            format!("{folder}/.env"),
            format!("{folder}/.env.staging.local")
        ])
    );
    let here = cfg.parent().expect("dir");
    assert_eq!(
        started_in(Some(here), &cfg, &["--env", "staging"], &[])["env_layers"],
        json!([".env", ".env.staging.local"])
    );
}

#[test]
fn run_started_has_empty_env_layers_when_no_dotenv_exists() {
    let cfg = project("env-none", &[]);
    assert_eq!(started(&cfg, &[], &[])["env_layers"], json!([]));
}

#[test]
fn run_started_reports_fail_fast_from_flag_env_and_config() {
    let cfg = project("env-fail-fast", &[]);
    let value =
        |args: &[&str], envs: &[(&str, &str)]| started(&cfg, args, envs)["fail_fast"].clone();
    assert_eq!(value(&[], &[]), false);
    assert_eq!(value(&["--fail-fast"], &[]), true);
    assert_eq!(value(&[], &[("BDDKIT_FAIL_FAST", "1")]), true);
    let text = std::fs::read_to_string(&cfg).expect("config");
    std::fs::write(&cfg, format!("fail_fast: true\n{text}")).expect("write config");
    assert_eq!(value(&[], &[]), true);
    assert_eq!(value(&[], &[("BDDKIT_FAIL_FAST", "0")]), false);
    assert_eq!(
        value(&["--fail-fast=false"], &[("BDDKIT_FAIL_FAST", "1")]),
        false
    );
}

#[test]
fn env_layers_never_carry_values() {
    let cfg = project(
        "env-values",
        &[(
            ".env",
            "A=distinct-value-123\nDB_PASSWORD=hidden-value-456\n",
        )],
    );
    let line = started(&cfg, &[], &[]).to_string();
    assert!(line.contains(".env"), "{line}");
    assert!(!line.contains("distinct-value-123"), "{line}");
    assert!(!line.contains("hidden-value-456"), "{line}");
}

#[test]
fn doctor_json_lists_the_env_layers_even_when_the_config_does_not_load() {
    let cfg = project("env-doctor", &[(".env", ""), (".env.ci", "A=1\n")]);
    let doctor = |cfg: &Path| -> Value {
        let out = bddkit(&[])
            .args(["doctor", "--json", "--env", "ci", "--config"])
            .arg(cfg)
            .output()
            .expect("run bddkit");
        serde_json::from_slice(&out.stdout).expect("--json emits JSON only")
    };
    let layers = json!([
        path_text(&cfg.with_file_name(".env")),
        path_text(&cfg.with_file_name(".env.ci"))
    ]);
    let report = doctor(&cfg);
    assert_eq!(report["app_env"], "ci");
    assert_eq!(report["env_layers"], layers);

    std::fs::write(&cfg, "paths: [${BDDKIT_ABSENT_VAR}]\n").expect("write config");
    assert_eq!(doctor(&cfg)["env_layers"], layers);
}
