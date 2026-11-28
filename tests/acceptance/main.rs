#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    reason = "`allow-*-in-tests` covers only `#[test]` functions, not the helpers here"
)]

mod annotations;
#[path = "../common/mod.rs"]
mod common;
mod config_discovery;
mod doctor;
mod env_local_from;
mod events;
mod events_doctor;
mod events_env;
mod events_interrupts;
mod events_load;
mod events_run;
mod evidence;
mod includes;
mod no_plugin;
mod privacy;
mod reports;
mod rules;
mod scheduling;
mod settings;
mod steps_and_macros;

use serde_json::Value;
use std::process::Command;

/// Prepares two feature files that write the same variable with different
/// values BEFORE the barrier and check it AFTER. The barrier guarantees that
/// both files have written before either reads: a run-wide `VarStack` would
/// fail this deterministically, not occasionally.
fn write_parallel_fixture(
    dir: &std::path::Path,
    concurrency: usize,
    tags: &str,
) -> std::path::PathBuf {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::create_dir_all(dir.join(".bddkit")).expect("mkdir .bddkit");
    std::fs::write(
        dir.join(".bddkit/plugins.yaml"),
        format!(
            "plugin:\n  - name: worker\n    path: {}\n",
            common::build_worker_plugin().display()
        ),
    )
    .expect("write lock");
    for who in ["a", "b"] {
        std::fs::write(
            dir.join(format!("features/{who}.feature")),
            format!(
                "{tags}Feature: parallel {who}\n  Scenario: both tasks meet at the barrier\n    \
                 Given set variable \"who\" to \"{who}\"\n    \
                 When I wait at the worker barrier for 2 parties\n    \
                 And variable \"who\" should be equal to \"{who}\"\n"
            ),
        )
        .expect("write feature");
    }
    let cfg = dir.join("cfg.yaml");
    std::fs::write(
        &cfg,
        format!(
            "concurrency: {concurrency}\npaths: [{}]\nresources:\n  api: {{}}\n",
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("write config");
    cfg
}

/// Writes a project with an empty `api` section, returning the config path.
/// `feature` is the whole `.feature` file so a
/// caller can plant a typo or a tag; an empty one writes no feature file at
/// all. `extra` is appended after the API block — indented, it adds to
/// `resources:`; at column 0 it adds a top-level key.
fn write_doctor_project(name: &str, feature: &str, extra: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("bddkit-{name}-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    let only = dir.join("features/only.feature");
    if feature.is_empty() {
        // A previous run of this test binary may have left one behind.
        let _ = std::fs::remove_file(&only);
    } else {
        std::fs::write(&only, feature).expect("write feature");
    }
    let api = "  api: {}\n";
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

/// A project with one passing and one failing scenario. The failure text
/// carries the two things an XML writer gets wrong first: a `]]>` in the
/// expected value and the NUL bytes of the `<<null>>` sentinel in the actual.
fn write_report_project(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("bddkit-{name}-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/report.feature"),
        r#"Feature: reported feature
  Scenario: passes
    When set variable "ping" to "ok"
    Then variable "ping" should be equal to "ok"

  Scenario: fails
    When set variable "ping" to "ok"
    And set variable "x" to "<<null>>"
    Then variable "x" should be equal to "]]>"
    And variable "ping" should be equal to "ok"
"#,
    )
    .expect("write feature");
    let config = dir.join("cfg.yaml");
    std::fs::write(
        &config,
        format!(
            "paths: [{}]\nresources:\n  api: {{}}\n",
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("write config");
    config
}

/// Builds a temp project for an `I include` acceptance test: copies each
/// `(dest path under the temp dir, source fixture path)` pair, then writes
/// the one `cfg.yaml` every one of these tests shares (an empty `api`,
/// no resource at all — none of them touch a system under test). Returns the
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
        "paths: [features]\nresources:\n  api: {}\n",
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

// ---------------------------------------------------------------------------
// `run --events`
// ---------------------------------------------------------------------------

/// A project under a fresh directory, returning the config path. `features`
/// are `(file name, content)` pairs; `macros` is one YAML file, if any. The
/// config declares an empty `api`.
fn write_events_project(
    name: &str,
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
    config.push_str(
        "resources:
  api: {}
",
    );
    let cfg = dir.join("cfg.yaml");
    std::fs::write(&cfg, config).expect("write config");
    cfg
}

/// `write_events_project` plus the echo fixture plugin, whose steps report
/// evidence. Sequential: the echo plugin is `shared`.
fn write_evidence_project(
    name: &str,
    features: &[(&str, &str)],
    macros: Option<&str>,
) -> std::path::PathBuf {
    let cfg = write_events_project(name, features, macros);
    let dir = cfg.parent().expect("project dir");
    std::fs::create_dir_all(dir.join(".bddkit")).expect("mkdir .bddkit");
    std::fs::write(
        dir.join(".bddkit/plugins.yaml"),
        format!(
            "plugin:\n  - name: echo\n    path: {}\n",
            common::build_fixture_plugin().display()
        ),
    )
    .expect("write lock");
    let mut config = std::fs::read_to_string(&cfg).expect("read config");
    config.push_str("concurrency: 1\n");
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
/// `--bddkit-dir <project>/.bddkit` when the project carries its own plugins,
/// so a developer's installed plugins never decide what a test sees.
fn project_bddkit_dir(cfg: &std::path::Path) -> Vec<std::ffi::OsString> {
    let dir = cfg.with_file_name(".bddkit");
    if dir.is_dir() {
        vec!["--bddkit-dir".into(), dir.into()]
    } else {
        vec![]
    }
}

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
        .args(project_bddkit_dir(cfg))
        .env_remove("BDDKIT_ARTIFACTS_DIR")
        // The developer's own choice must not decide what a test records.
        .env_remove("BDDKIT_EVIDENCE")
        .env_remove("BDDKIT_EVIDENCE_BUFFER")
        .args(extra)
        .output()
        .expect("run bddkit");
    let stream = read_stream(&events);
    (out, stream, events)
}

fn of_type<'a>(stream: &'a [Value], kind: &str) -> Vec<&'a Value> {
    stream.iter().filter(|e| e["type"] == kind).collect()
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

// ---- privacy: what the failure text shows (spec §7.8) ----

/// A one-file project; `top_extra` is appended to the config.
fn masking_project(name: &str, feature: &str, top_extra: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("bddkit-{name}-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(dir.join("features/mask.feature"), feature).expect("write feature");
    let config = dir.join("cfg.yaml");
    std::fs::write(
        &config,
        format!(
            "paths: [{}]\nresources:\n  api: {{}}\n{top_extra}",
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("write config");
    config
}

/// Runs the project and returns (exit code, stdout, stderr).
fn run_masking(
    cfg: &std::path::Path,
    args: &[&str],
    envs: &[(&str, &str)],
) -> (i32, String, String) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_bddkit"));
    command.args(["run", "--config"]).arg(cfg).args(args);
    for key in [
        "BDDKIT_PRIVACY_WORDS",
        "BDDKIT_PRIVACY_WORDS_ONLY",
        "BDDKIT_PRIVACY_FIELDS",
        "BDDKIT_PRIVACY_FIELDS_ONLY",
        "BDDKIT_PRIVACY_MIN_LENGTH",
        "BDDKIT_PRIVACY_REVEAL",
    ] {
        command.env_remove(key);
    }
    let out = command
        .envs(envs.iter().copied())
        .output()
        .expect("run bddkit");
    (
        out.status.code().expect("exit code"),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}
