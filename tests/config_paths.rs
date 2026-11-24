//! A relative `paths` / `macro_paths` written in a config resolve against the
//! config's folder; anything typed on the command line or in the environment
//! against the working directory (issue #301).

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    reason = "`allow-*-in-tests` covers only `#[test]` functions, not the helpers here"
)]

use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Output};

/// The example of the issue: a project with a module in a subfolder, started
/// from the project root.
///
/// ```text
/// <root>/features/root.feature
/// <root>/module/bddkit.yaml        paths: [features], macro_paths: [macros.yaml]
/// <root>/module/features/m.feature
/// ```
struct Project(PathBuf);

const FEATURE: &str = "Feature: f\n  Scenario: first\n    When set variable \"x\" to \"1\"\n\n  Scenario: second\n    When set variable \"x\" to \"2\"\n";
const USES_MACRO: &str = "Feature: f\n  Scenario: s\n    When I remember the module value\n";

impl Project {
    fn new(name: &str, module_config: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("bddkit-cfgpaths-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["features", "module/features", "no-plugins"] {
            std::fs::create_dir_all(root.join(dir)).expect("mkdir");
        }
        std::fs::write(root.join("features/root.feature"), FEATURE).expect("write");
        std::fs::write(root.join("module/features/m.feature"), FEATURE).expect("write");
        std::fs::write(root.join("module/features/macro.feature"), USES_MACRO).expect("write");
        std::fs::write(
            root.join("module/macros.yaml"),
            "- step: 'I remember the module value'\n  do:\n    - set variable \"v\" to \"1\"\n",
        )
        .expect("write");
        std::fs::write(root.join("module/bddkit.yaml"), module_config).expect("write");
        Self(root)
    }

    /// `bddkit <command> --config module/bddkit.yaml …` from the project root.
    fn bddkit(&self, command: &str, args: &[&str], envs: &[(&str, &str)]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_bddkit"))
            .arg(command)
            .args(["--config", "module/bddkit.yaml"])
            .args(args)
            .env("BDDKIT_DIR", self.0.join("no-plugins"))
            .env_remove("BDDKIT_PATHS")
            .env_remove("BDDKIT_MACRO_PATHS")
            .envs(envs.iter().copied())
            .current_dir(&self.0)
            .output()
            .expect("failed to run bddkit")
    }

    fn listed(&self, args: &[&str], envs: &[(&str, &str)]) -> Vec<String> {
        let out = self.bddkit("list", &[&["--json"], args].concat(), envs);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        let json: Value = serde_json::from_slice(&out.stdout).expect("stdout is JSON");
        let mut files: Vec<String> = json["files"]
            .as_array()
            .expect("files")
            .iter()
            .map(|f| f["file"].as_str().expect("file").to_string())
            .collect();
        files.sort();
        files
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

const MODULE: &str = "paths: [features]\nresources: {}\n";

#[test]
fn config_paths_resolve_against_the_config_folder() {
    let p = Project::new("paths", MODULE);
    let expected = ["module/features/m.feature", "module/features/macro.feature"];
    // The macro feature needs a macro the config does not name: it is listed
    // all the same, `list` loads no macros.
    assert_eq!(p.listed(&[], &[]), expected);
}

#[test]
fn config_macro_paths_resolve_against_the_config_folder() {
    let p = Project::new("macros", &format!("macro_paths: [macros.yaml]\n{MODULE}"));
    let out = p.bddkit("run", &["module/features/macro.feature"], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stdout));
}

#[test]
fn an_absolute_config_path_is_kept() {
    let p = Project::new("absolute", "");
    let features = p.0.join("features");
    std::fs::write(
        p.0.join("module/bddkit.yaml"),
        format!(
            "paths: ['{}']\nresources: {{}}\n",
            features.display().to_string().replace('\\', "/")
        ),
    )
    .expect("write");
    // Absolute stays as written, so it prints as written.
    let printed = format!("{}/root.feature", features.display()).replace('\\', "/");
    assert_eq!(p.listed(&[], &[]), [printed]);
}

#[test]
fn command_line_paths_resolve_against_the_working_directory() {
    let p = Project::new("cwd", MODULE);
    assert_eq!(
        p.listed(&["features/root.feature:2"], &[]),
        ["features/root.feature"]
    );
    assert_eq!(
        p.listed(&[], &[("BDDKIT_PATHS", "features")]),
        ["features/root.feature"]
    );
    // `--macro-paths` is read from the root too.
    let out = p.bddkit(
        "run",
        &[
            "--macro-paths",
            "module/macros.yaml",
            "module/features/macro.feature",
        ],
        &[],
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stdout));
}

#[test]
fn printed_paths_are_relative_to_the_working_directory_and_round_trip() {
    let p = Project::new("roundtrip", MODULE);
    let out = p.bddkit("list", &["--json"], &[]);
    let json: Value = serde_json::from_slice(&out.stdout).expect("JSON");
    let file = json["files"][0]["file"].as_str().expect("file").to_string();
    let line = json["files"][0]["scenarios"][1]["line"]
        .as_u64()
        .expect("line");
    assert_eq!(file, "module/features/m.feature");
    let target = format!("{file}:{line}");
    assert_eq!(p.listed(&[&target], &[]), [file.as_str()]);
    let ran = p.bddkit("run", &[&target], &[]);
    assert_eq!(ran.status.code(), Some(0), "{}", text(&ran.stdout));
    assert!(
        text(&ran.stdout).contains("scenarios: 1"),
        "{}",
        text(&ran.stdout)
    );
}

#[test]
fn a_root_relative_config_in_a_subfolder_is_refused_naming_the_folder() {
    let p = Project::new("refused", "paths: [module/features]\nresources: {}\n");
    for command in ["run", "list"] {
        let out = p.bddkit(command, &[], &[]);
        assert_eq!(out.status.code(), Some(2), "{command}");
        assert!(
            text(&out.stderr)
                .replace('\\', "/")
                .contains("module/module/features"),
            "{command}: {}",
            text(&out.stderr)
        );
    }
}

#[test]
fn run_list_and_doctor_agree_on_the_resolved_paths() {
    let p = Project::new("agree", &format!("macro_paths: [macros.yaml]\n{MODULE}"));
    // A doctor that resolved against the working directory would check this.
    std::fs::write(
        p.0.join("features/root.feature"),
        "Feature: f\n  Scenario: s\n    When no step says this\n",
    )
    .expect("write");
    let run = p.bddkit("run", &[], &[]);
    let doctor = p.bddkit("doctor", &[], &[]);
    assert_eq!(run.status.code(), Some(0), "{}", text(&run.stdout));
    assert_eq!(doctor.status.code(), Some(0), "{}", text(&doctor.stdout));
    let ran = text(&run.stdout);
    for file in p.listed(&[], &[]) {
        assert!(ran.contains(&file), "{file} not run: {ran}");
    }
    assert!(!ran.contains("features/root.feature"), "{ran}");
}

#[test]
fn a_config_outside_the_working_directory_prints_absolute_paths() {
    let p = Project::new("outside", MODULE);
    std::fs::create_dir_all(p.0.join("sub")).expect("mkdir");
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["list", "--json", "--config", "../module/bddkit.yaml"])
        .env("BDDKIT_DIR", p.0.join("no-plugins"))
        .current_dir(p.0.join("sub"))
        .output()
        .expect("failed to run bddkit");
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let json: Value = serde_json::from_slice(&out.stdout).expect("JSON");
    let file = PathBuf::from(json["files"][0]["file"].as_str().expect("file"));
    assert!(file.is_absolute(), "{}", file.display());
    assert!(
        file.ends_with("module/features/m.feature"),
        "{}",
        file.display()
    );
}
