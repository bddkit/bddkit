//! `bddkit list`: what a run would run, without running it (issue #170).

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    reason = "`allow-*-in-tests` covers only `#[test]` functions, not the helpers here"
)]

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
            // A file that did not parse has an `error` and no `scenarios`.
            f.get("scenarios")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
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

/// A feature whose line 4 is not Gherkin: the parser stops there.
const BROKEN: &str = "\
Feature: bad
  Scenario: s
    When set variable \"x\" to \"1\"
  this is not gherkin
";

fn with_broken(name: &str) -> Project {
    let p = Project::new(name, NONE);
    std::fs::write(p.0.join("features/bad.feature"), BROKEN).expect("write");
    p
}

fn file_named<'a>(json: &'a Value, name: &str) -> &'a Value {
    json["files"]
        .as_array()
        .expect("files")
        .iter()
        .find(|f| f["file"].as_str().is_some_and(|p| p.ends_with(name)))
        .unwrap_or_else(|| panic!("{name} is not listed: {json}"))
}

#[test]
fn a_broken_feature_file_is_listed_with_its_error() {
    let clean = Project::new("clean", NONE).list_json(&[]);
    let p = with_broken("unparsed");
    let json = p.list_json(&[]);
    let bad = file_named(&json, "bad.feature");
    assert_eq!(bad["error"]["line"], 4, "{bad}");
    assert!(
        bad["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("failed to parse") && m.contains("4:")),
        "{bad}"
    );
    assert!(bad.get("scenarios").is_none(), "no scenarios: {bad}");
    // The other file is listed exactly as in the same suite without the broken one.
    assert_eq!(
        file_named(&json, "sel.feature")["scenarios"],
        file_named(&clean, "sel.feature")["scenarios"]
    );
    assert_eq!(listed(&json).len(), listed(&clean).len());
}

#[test]
fn a_misspelled_keyword_is_listed_as_an_error_not_dropped() {
    let p = Project::new("typo", NONE);
    std::fs::write(
        p.0.join("features/typo.feature"),
        "Feature: typo\n  Scenaro: s\n    When I sleep 0 seconds\n",
    )
    .expect("write");
    let json = p.list_json(&[]);
    let typo = file_named(&json, "typo.feature");
    assert_eq!(typo["error"]["line"], 2, "{typo}");
    assert!(
        typo["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("`Scenaro:` is not a keyword")),
        "{typo}"
    );
}

#[test]
fn a_bom_does_not_break_a_file() {
    let p = Project::new("bom", NONE);
    std::fs::write(
        p.0.join("features/bom.feature"),
        "\u{feff}Feature: bom\n  Scenario: s\n    When I sleep 0 seconds\n",
    )
    .expect("write");
    let json = p.list_json(&[]);
    assert!(file_named(&json, "bom.feature").get("error").is_none());
}

#[test]
fn an_error_line_stays_in_the_file() {
    // The last line is the bad one: the parser stops one past it.
    let p = Project::new("clamp", NONE);
    std::fs::write(
        p.0.join("features/clamp.feature"),
        "Feature: f\n  Scenario: s\n    Given foo\n  Bogus line\n",
    )
    .expect("write");
    let json = p.list_json(&[]);
    assert_eq!(
        file_named(&json, "clamp.feature")["error"]["line"],
        4,
        "{json}"
    );
}

#[test]
fn a_broken_file_is_listed_whatever_the_tags_and_lines() {
    // Its tags are unknown, so a tag filter cannot drop it; a line argument
    // cannot select in it either.
    let p = with_broken("unparsed-tags");
    let json = p.list_json(&["--tag", "@fast"]);
    assert_eq!(file_named(&json, "bad.feature")["error"]["line"], 4);
    assert_eq!(listed(&json).len(), 2, "only the @fast rows of sel.feature");
    let bad = format!("{}:2", p.0.join("features/bad.feature").display());
    let json = p.list_json(&[&bad]);
    assert_eq!(json["files"].as_array().expect("files").len(), 1);
    assert_eq!(file_named(&json, "bad.feature")["error"]["line"], 4);
}

#[test]
fn the_human_list_names_the_broken_file() {
    let p = with_broken("unparsed-human");
    let out = p.bddkit("list", &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    // stdout stays what `run` takes back: the good file's lines only.
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("sel.feature:") && !text.contains("bad.feature"),
        "{text}"
    );
    assert!(text.lines().all(|l| l.contains(".feature:")), "{text}");
    // The error is on stderr, with the file and the line.
    let err = stderr(&out);
    assert!(err.contains("bad.feature") && err.contains("4:"), "{err}");
}

#[test]
fn run_over_the_same_suite_is_still_exit_2() {
    let p = with_broken("unparsed-run");
    let out = p.bddkit("run", &[]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(
        err.contains("failed to parse") && err.contains("bad.feature"),
        "{err}"
    );
    assert!(err.contains("run not started"), "{err}");
}

#[test]
fn a_file_that_is_not_utf8_is_listed_with_an_error_and_no_line() {
    let p = Project::new("not-utf8", NONE);
    std::fs::write(p.0.join("features/bad.feature"), [0xff, 0xfe, 0xfd]).expect("write");
    let json = p.list_json(&[]);
    let bad = file_named(&json, "bad.feature");
    assert!(bad["error"].get("line").is_none(), "{bad}");
    assert!(
        bad["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("failed to read")),
        "{bad}"
    );
    assert!(bad.get("scenarios").is_none(), "{bad}");
    assert!(listed(&json).len() > 1, "the other file is listed: {json}");
}

#[cfg(unix)]
#[test]
fn a_file_that_cannot_be_read_at_all_is_still_exit_2() {
    let p = Project::new("unreadable", NONE);
    std::os::unix::fs::symlink("no/such/target", p.0.join("features/bad.feature"))
        .expect("symlink");
    let out = p.bddkit("list", &["--json"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
}

#[test]
fn reveal_says_so_on_stderr_and_keeps_stdout_json() {
    let p = Project::new("reveal", NONE);
    let out = p.bddkit("list", &["--json", "--reveal"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(stderr(&out).contains("masking is off"), "{}", stderr(&out));
    serde_json::from_slice::<Value>(&out.stdout).expect("stdout is JSON");
}
