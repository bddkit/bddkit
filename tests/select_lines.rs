//! `bddkit run path:LINE`: one scenario, one example row, or a report id,
//! chosen by where it sits in the file (issue #169).

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    reason = "`allow-*-in-tests` covers only `#[test]` functions, not the helpers here"
)]

use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;

/// Line numbers are part of every test below: keep the file and `cat -n` in step.
const FEATURE: &str = "\
Feature: sel
  Background:
    Given set variable \"b\" to \"1\"

  Scenario: alpha
    When set variable \"x\" to \"1\"
    Then variable \"x\" should be equal to \"1\"

  @fast
  Scenario Outline: beta <n>
    When set variable \"x\" to \"<n>\"
    Then variable \"x\" should be equal to \"<n>\"

    Examples: first
      | n |
      | 1 |
      # a comment between two rows
      | 2 |

    @slow
    Examples: second
      | n |
      | 3 |

  Rule: gamma
    Scenario: delta
      When set variable \"x\" to \"1\"

    Scenario: epsilon
      When set variable \"x\" to \"1\"

  Scenario: zeta
    When set variable \"x\" to \"1\"
";

/// The `scenario_started` events of a run: name and `row_line`.
type Started = Vec<(String, Option<u64>)>;

struct Project(PathBuf);

impl Project {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("bddkit-lines-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("features")).expect("mkdir");
        std::fs::write(dir.join("features/sel.feature"), FEATURE).expect("write feature");
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
        Self(dir)
    }

    fn feature(&self) -> String {
        self.0.join("features/sel.feature").display().to_string()
    }

    /// Runs with the arguments, returns the exit code, stderr and the
    /// `scenario_started` events (name, row_line) in the order they arrived.
    fn run(&self, args: &[&str]) -> (Option<i32>, String, Started) {
        let events = self.0.join("events.jsonl");
        let _ = std::fs::remove_file(&events);
        let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
            .args(["run", "--config"])
            .arg(self.0.join("cfg.yaml"))
            .arg("--events")
            .arg(&events)
            .args(args)
            .output()
            .expect("failed to run bddkit");
        let started = std::fs::read_to_string(&events)
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str::<Value>(l).expect("event is JSON"))
            .filter(|e| e["type"] == "scenario_started")
            .map(|e| {
                (
                    e["name"].as_str().expect("name").to_string(),
                    e["row_line"].as_u64(),
                )
            })
            .collect();
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
            started,
        )
    }

    fn names(&self, args: &[&str]) -> Vec<String> {
        let (code, stderr, started) = self.run(args);
        assert_eq!(code, Some(0), "{stderr}");
        started.into_iter().map(|(name, _)| name).collect()
    }
}

#[test]
fn a_scenario_line_runs_that_scenario_only() {
    let p = Project::new("scenario");
    let at = |line: u32| format!("{}:{line}", p.feature());
    assert_eq!(p.names(&[&at(5)]), ["alpha"]);
    // An outline line selects all its rows, both blocks.
    assert_eq!(
        p.names(&[&at(10)]),
        ["beta <n> [1]", "beta <n> [2]", "beta <n> [3]"]
    );
}

#[test]
fn several_lines_select_their_union_in_file_order() {
    let p = Project::new("union");
    let arg = format!("{}:32:5", p.feature());
    assert_eq!(p.names(&[&arg]), ["alpha", "zeta"]);
}

#[test]
fn a_row_line_runs_that_row_only() {
    let p = Project::new("row");
    let (code, stderr, started) = p.run(&[&format!("{}:18", p.feature())]);
    assert_eq!(code, Some(0), "{stderr}");
    // Line 18 sits after a comment line: the row is found by its own line.
    assert_eq!(started, [("beta <n> [2]".to_string(), Some(18))]);
}

#[test]
fn a_report_id_with_a_row_runs_that_row() {
    let p = Project::new("id");
    // `example` counts across the blocks: 2 is the row of the second block.
    assert_eq!(
        p.names(&[&format!("{}:10#2", p.feature())]),
        ["beta <n> [3]"]
    );
    assert_eq!(
        p.names(&[&format!("{}:10#0", p.feature())]),
        ["beta <n> [1]"]
    );
}

#[test]
fn a_report_id_that_names_no_row_is_refused() {
    let p = Project::new("idbad");
    for arg in [
        format!("{}:10#9", p.feature()),
        format!("{}:5#0", p.feature()),
    ] {
        let (code, stderr, started) = p.run(&[&arg]);
        assert_eq!(code, Some(2), "{arg}: {stderr}");
        assert!(stderr.contains("has no example"), "{stderr}");
        assert!(started.is_empty());
    }
}

#[test]
fn an_examples_line_runs_its_block() {
    let p = Project::new("examples");
    assert_eq!(
        p.names(&[&format!("{}:14", p.feature())]),
        ["beta <n> [1]", "beta <n> [2]"]
    );
    assert_eq!(p.names(&[&format!("{}:21", p.feature())]), ["beta <n> [3]"]);
}

#[test]
fn a_rule_line_runs_its_scenarios() {
    // A scenario after a Rule belongs to it, so `zeta` is gamma's too.
    let p = Project::new("rule");
    assert_eq!(
        p.names(&[&format!("{}:25", p.feature())]),
        ["delta", "epsilon", "zeta"]
    );
}

#[test]
fn a_line_inside_steps_is_refused_with_the_nearest_scenario() {
    let p = Project::new("refused");
    // Line 6 is a step of `alpha` (line 5); 15 is the Examples header row.
    for (line, nearest) in [(6, 5), (3, 5), (1, 5), (15, 10), (99, 32)] {
        let (code, stderr, started) = p.run(&[&format!("{}:{line}", p.feature())]);
        assert_eq!(code, Some(2), "line {line}: {stderr}");
        assert!(
            stderr.contains(&format!("sel.feature:{line} is not")),
            "{stderr}"
        );
        assert!(
            stderr.contains(&format!("nearest scenario starts on line {nearest}")),
            "line {line}: {stderr}"
        );
        assert!(started.is_empty(), "nothing may run");
    }
}

#[test]
fn a_windows_drive_letter_is_not_a_line() {
    // `C:` is a directory name here; the argument exists as written, so the
    // colon is not a separator and nothing is peeled off.
    let p = Project::new("drive");
    let drive = p.0.join("C:");
    std::fs::create_dir_all(drive.join("suite")).expect("mkdir");
    std::fs::write(drive.join("suite/a.feature"), FEATURE).expect("write");
    let (code, stderr, started) = p.run(&[&drive.join("suite/a.feature").display().to_string()]);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(started.len(), 7);
    // And a file whose name ends in `:12` is still reachable by that name.
    let odd = p.0.join("odd:12");
    std::fs::write(&odd, FEATURE).expect("write");
    let (code, stderr, started) = p.run(&[&odd.display().to_string()]);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(started.len(), 7);
}

#[test]
fn line_and_tag_selections_intersect() {
    let p = Project::new("tags");
    let args = |tag: &'static str, line: u32| {
        [
            format!("{}:{line}", p.feature()),
            "--tag".into(),
            tag.into(),
        ]
    };
    let run = |tag, line| {
        let [a, b, c] = args(tag, line);
        p.run(&[&a, &b, &c])
    };
    // The outline is `@fast`; only its second block is `@slow`.
    let (code, _, started) = run("slow", 10);
    assert_eq!(code, Some(0));
    assert_eq!(started.len(), 1, "only the second block is @slow");
    let (code, _, started) = run("fast", 10);
    assert_eq!(code, Some(0));
    assert_eq!(started.len(), 3, "@fast is on the whole outline");
    // Row 16 is in the untagged first block: `@slow` keeps only the second.
    let (code, _, started) = run("slow", 16);
    assert_eq!(
        code,
        Some(2),
        "line and tag pick disjoint rows of one outline"
    );
    assert!(started.is_empty());
    let (code, _, started) = run("fast", 5);
    assert_eq!(
        code,
        Some(2),
        "alpha has no @fast: empty intersection is exit 2"
    );
    assert!(started.is_empty());
}

#[test]
fn a_file_named_twice_runs_once_with_the_union_of_its_lines() {
    let p = Project::new("twice");
    let f = p.feature();
    assert_eq!(
        p.names(&[&format!("{f}:5"), &format!("{f}:32")]),
        ["alpha", "zeta"]
    );
    // Whole file beats a line of it.
    let (code, stderr, started) = p.run(&[&f, &format!("{f}:5")]);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(started.len(), 7);
}

#[test]
fn scenario_started_carries_row_line() {
    let p = Project::new("rowline");
    let (code, stderr, started) = p.run(&[&p.feature()]);
    assert_eq!(code, Some(0), "{stderr}");
    let rows: Vec<_> = started.iter().map(|(_, row)| *row).collect();
    // alpha, three rows, delta, epsilon, zeta: only a row has the field.
    assert_eq!(rows, [None, Some(16), Some(18), Some(23), None, None, None]);
}

#[test]
fn a_file_spelled_two_ways_runs_once() {
    let p = Project::new("spelling");
    let run = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
            .current_dir(&p.0)
            .args(["run", "--config", "cfg.yaml"])
            .args(args)
            .output()
            .expect("failed to run bddkit");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let out = run(&["features/sel.feature:5", "./features/sel.feature:32"]);
    assert!(out.contains("files: 1, scenarios: 2, failed: 0"), "{out}");
    // A directory named without a line runs the file whole.
    let out = run(&["./features", "features/sel.feature:5"]);
    assert!(out.contains("files: 1, scenarios: 7, failed: 0"), "{out}");
}

#[test]
fn a_bad_line_is_refused_even_when_the_file_is_also_named_whole() {
    let p = Project::new("wholebad");
    let f = p.feature();
    let (code, stderr, _) = p.run(&[&f, &format!("{f}:6")]);
    assert_eq!(code, Some(2), "{stderr}");
}

#[test]
fn a_line_in_the_configs_paths_is_not_split() {
    let p = Project::new("cfgpaths");
    std::fs::write(
        p.0.join("cfg2.yaml"),
        format!("paths: [{}:5]\nresources:\n  api: {{}}\n", p.feature()),
    )
    .expect("write config");
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(p.0.join("cfg2.yaml"))
        .output()
        .expect("failed to run bddkit");
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("does not exist"));
}
