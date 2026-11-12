//! `bddkit list`: what a run would run, without running it (issue #170).

use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Output};

/// Line numbers matter: keep the file and `cat -n` in step.
const FEATURE: &str = "\
@auth
Feature: sel
  Scenario: alpha
    When set variable \"x\" to \"1\"

  @fast
  Scenario Outline: beta <n>
    When set variable \"x\" to \"<n>\"

    Examples: first
      | n |
      | 1 |
      | 2 |

  Rule: gamma
    Scenario: delta
      When set variable \"x\" to \"1\"
";

/// A config with no resources: nothing the scenarios use needs one.
const NONE: &str = "resources: {}\n";

struct Project(PathBuf);

impl Project {
    fn new(name: &str, extra_config: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("bddkit-list-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("features")).expect("mkdir");
        std::fs::create_dir_all(dir.join("no-plugins")).expect("mkdir");
        std::fs::write(dir.join("features/sel.feature"), FEATURE).expect("write feature");
        std::fs::write(
            dir.join("cfg.yaml"),
            format!(
                "paths: [{}]\n{extra_config}",
                dir.join("features")
                    .display()
                    .to_string()
                    .replace('\\', "/")
            ),
        )
        .expect("write config");
        Self(dir)
    }

    fn bddkit(&self, command: &str, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_bddkit"))
            .arg(command)
            .arg("--config")
            .arg(self.0.join("cfg.yaml"))
            .args(args)
            // No plugins anywhere: `list` must not need one.
            .env("BDDKIT_DIR", self.0.join("no-plugins"))
            .output()
            .expect("failed to run bddkit")
    }

    fn list_json(&self, args: &[&str]) -> Value {
        let out = self.bddkit("list", &[&["--json"], args].concat());
        assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
        serde_json::from_slice(&out.stdout).expect("stdout is JSON")
    }

    fn feature(&self) -> String {
        self.0.join("features/sel.feature").display().to_string()
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// `(file, line, example)` of every scenario in a `list --json` answer, sorted:
/// `run` starts files in priority and concurrency order, `list` in path order.
fn listed(json: &Value) -> Vec<(String, u64, Option<u64>)> {
    let mut all: Vec<_> = json["files"]
        .as_array()
        .expect("files")
        .iter()
        .flat_map(|f| {
            let file = f["file"].as_str().expect("file").to_string();
            f["scenarios"]
                .as_array()
                .expect("scenarios")
                .iter()
                .map(move |s| {
                    (
                        file.clone(),
                        s["line"].as_u64().expect("line"),
                        s["example"].as_u64(),
                    )
                })
        })
        .collect();
    all.sort();
    all
}

impl Project {
    /// What a run with `args` starts, as `(file, line, example)`, sorted.
    fn ran(&self, args: &[&str]) -> Vec<(String, u64, Option<u64>)> {
        let events = self.0.join("events.jsonl");
        let _ = std::fs::remove_file(&events);
        let run = Command::new(env!("CARGO_BIN_EXE_bddkit"))
            .args(["run", "--config"])
            .arg(self.0.join("cfg.yaml"))
            .env("BDDKIT_DIR", self.0.join("no-plugins"))
            .arg("--events")
            .arg(&events)
            .args(args)
            .output()
            .expect("run");
        assert_eq!(run.status.code(), Some(0), "{}", stderr(&run));
        let mut ran: Vec<_> = std::fs::read_to_string(&events)
            .expect("events")
            .lines()
            .map(|l| serde_json::from_str::<Value>(l).expect("json"))
            .filter(|e| e["type"] == "scenario_started")
            .map(|e| {
                (
                    e["file"].as_str().expect("file").to_string(),
                    e["line"].as_u64().expect("line"),
                    e["example"].as_u64(),
                )
            })
            .collect();
        ran.sort();
        ran
    }
}

#[test]
fn list_selects_exactly_what_run_runs() {
    let p = Project::new("same", NONE);
    // A second file the tag filter keeps out of `--tag fast`.
    std::fs::write(
        p.0.join("features/other.feature"),
        "@other\nFeature: other\n  Scenario: omega\n    When set variable \"x\" to \"1\"\n",
    )
    .expect("write");
    let row = format!("{}:12", p.feature());
    for args in [
        vec![],
        vec!["--tag", "fast"],
        vec!["--tag", "auth"],
        vec!["--tag", "other"],
        vec![row.as_str()],
    ] {
        let ran = p.ran(&args);
        assert!(!ran.is_empty());
        assert_eq!(listed(&p.list_json(&args)), ran, "args {args:?}");
    }
}

#[test]
fn a_printed_line_runs_that_scenario_when_passed_back() {
    let p = Project::new("roundtrip", NONE);
    let out = p.bddkit("list", &[]);
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    // The row of `beta <n> [2]`: the printed line is the row's own.
    let line = text
        .lines()
        .find(|l| l.contains("beta <n> [2]"))
        .expect("listed");
    let argument = line.split("  ").next().expect("file:LINE");
    assert!(argument.ends_with(":13"), "{line}");
    assert_eq!(p.ran(&[argument]).len(), 1);
}

#[test]
fn list_json_uses_the_stream_field_names() {
    let p = Project::new("fields", NONE);
    let json = p.list_json(&[]);
    assert_eq!(json["config_source"], "flag");
    assert_eq!(json["selection"]["tags"], serde_json::json!([]));
    let file = &json["files"][0];
    assert_eq!(file["name"], "sel");
    assert_eq!(file["tags"], serde_json::json!(["@auth"]));
    let scenarios = file["scenarios"].as_array().expect("scenarios");
    assert_eq!(scenarios.len(), 4);
    let row = &scenarios[2];
    assert_eq!(row["name"], "beta <n> [2]");
    assert_eq!(row["line"], 7);
    assert_eq!(row["example"], 1);
    // The stream names the block `examples`, with its line inside.
    assert_eq!(row["examples"]["line"], 10);
    assert_eq!(row["examples"]["name"], "first");
    assert_eq!(row["row_line"], 13);
    assert_eq!(row["tags"], serde_json::json!(["@fast", "@auth"]));
    // Empty fields are left out, as in the stream.
    assert!(scenarios[0].get("rule").is_none());
    assert!(scenarios[0].get("example").is_none());
    assert_eq!(scenarios[3]["rule"]["name"], "gamma");
    assert_eq!(scenarios[3]["rule"]["line"], 15);
}

#[test]
fn an_empty_selection_is_an_empty_list() {
    let p = Project::new("empty", NONE);
    let json = p.list_json(&["--tag", "nothing-has-this"]);
    assert_eq!(json["files"], serde_json::json!([]));
}

#[test]
fn list_works_without_plugins_installed() {
    // A group no plugin serves: `run` refuses it, `list` never asks.
    let p = Project::new("plugins", "resources:\n  db:\n    main: {}\n");
    let out = p.bddkit("list", &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("alpha") && text.contains("beta <n> [2]"),
        "{text}"
    );
}

#[test]
fn list_masks_a_config_secret_in_a_scenario_name() {
    let p = Project::new(
        "mask",
        "resources:\n  db:\n    main:\n      password: hunter2hunter2\n",
    );
    std::fs::write(
        p.0.join("features/sel.feature"),
        "Feature: f\n  Scenario: log in as hunter2hunter2\n    When set variable \"x\" to \"1\"\n",
    )
    .expect("write");
    let out = p.bddkit("list", &["--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(!text.contains("hunter2hunter2"), "{text}");
    assert!(text.contains("log in as ***"), "{text}");
}

#[test]
fn a_broken_config_is_exit_2_with_an_error_object() {
    let p = Project::new("broken", NONE);
    std::fs::write(p.0.join("cfg.yaml"), "paths: [unclosed\n").expect("write");
    let out = p.bddkit("list", &["--json"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(!stderr(&out).is_empty());
    let json: Value = serde_json::from_slice(&out.stdout).expect("stdout is JSON");
    assert!(
        json["error"]["message"]
            .as_str()
            .is_some_and(|m| !m.is_empty()),
        "{json}"
    );
}

#[test]
fn a_missing_path_is_exit_2() {
    let p = Project::new("nopath", NONE);
    let out = p.bddkit("list", &["--json", "no/such/dir"]);
    assert_eq!(out.status.code(), Some(2));
    let json: Value = serde_json::from_slice(&out.stdout).expect("stdout is JSON");
    assert!(json["error"]["message"].is_string(), "{json}");
}

#[test]
fn a_feature_that_does_not_parse_fails_the_whole_list() {
    let p = Project::new("unparsed", NONE);
    std::fs::write(p.0.join("features/bad.feature"), "Not gherkin at all\n").expect("write");
    let out = p.bddkit("list", &["--json"]);
    assert_eq!(out.status.code(), Some(2));
    let json: Value = serde_json::from_slice(&out.stdout).expect("stdout is JSON");
    assert!(
        json["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("bad.feature")),
        "{json}"
    );
}

#[test]
fn reveal_says_so_on_stderr_and_keeps_stdout_json() {
    let p = Project::new("reveal", NONE);
    let out = p.bddkit("list", &["--json", "--reveal"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(stderr(&out).contains("masking is off"), "{}", stderr(&out));
    serde_json::from_slice::<Value>(&out.stdout).expect("stdout is JSON");
}
