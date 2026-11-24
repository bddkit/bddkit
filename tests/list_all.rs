//! `bddkit list-all`: many configs, one process (issue #299).

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    reason = "`allow-*-in-tests` covers only `#[test]` functions, not the helpers here"
)]

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("bddkit-list-all-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("no-plugins")).expect("mkdir");
        Self(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    /// A suite `<name>.yaml` over `<name>/features/s.feature`, `paths` absolute
    /// so the fixture means the same wherever the command runs.
    fn suite(&self, name: &str, extra: &str, scenario: &str) -> PathBuf {
        let features = self.0.join(name).join("features");
        std::fs::create_dir_all(&features).expect("mkdir");
        std::fs::write(
            features.join("s.feature"),
            format!("Feature: f\n  Scenario: {scenario}\n    When set variable \"x\" to \"1\"\n"),
        )
        .expect("write feature");
        let config = self.path(&format!("{name}.yaml"));
        std::fs::write(
            &config,
            format!(
                "paths: [{}]\nresources: {{}}\n{extra}",
                features.display().to_string().replace('\\', "/")
            ),
        )
        .expect("write config");
        config
    }

    fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.path(name);
        std::fs::write(&path, text).expect("write");
        path
    }

    fn run(&self, args: &[&Path], flags: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_bddkit"))
            .arg("list-all")
            .args(flags)
            .args(args)
            // No plugins anywhere: `list-all` must not need one.
            .env("BDDKIT_DIR", self.path("no-plugins"))
            .output()
            .expect("failed to run bddkit")
    }

    fn json(&self, args: &[&Path]) -> Value {
        let out = self.run(args, &["--json"]);
        assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
        serde_json::from_slice(&out.stdout).expect("stdout is JSON")
    }

    fn list_json(&self, config: &Path) -> Value {
        let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
            .args(["list", "--json", "--config"])
            .arg(config)
            .env("BDDKIT_DIR", self.path("no-plugins"))
            .output()
            .expect("failed to run bddkit");
        assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
        serde_json::from_slice(&out.stdout).expect("stdout is JSON")
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn configs(json: &Value) -> &Vec<Value> {
    json["configs"].as_array().expect("configs is an array")
}

#[test]
fn list_all_matches_list_for_each_config() {
    let d = Dir::new("same");
    let a = d.suite("a", "", "alpha");
    let b = d.suite("b", "tags: [x]\n", "beta");
    let all = d.json(&[&a, &b]);
    for (entry, config) in configs(&all).iter().zip([&a, &b]) {
        let single = d.list_json(config);
        assert_eq!(entry["config"], single["config"]);
        assert_eq!(entry["files"], single["files"]);
        assert_eq!(entry["selection"]["tags"], single["selection"]["tags"]);
    }
    assert_eq!(configs(&all).len(), 2);
}

#[test]
fn list_all_marks_a_foreign_yaml_not_a_suite() {
    let d = Dir::new("foreign");
    let compose = d.write(
        "docker-compose.yml",
        "services:\n  web:\n    image: nginx\n",
    );
    let openapi = d.write("openapi.yaml", "openapi: 3.0.0\npaths:\n  /x: {}\n");
    let ci = d.write("ci.yml", "- build\n- test\n");
    let multi = d.write("k8s.yaml", "a: 1\n---\nb: 2\n");
    let json = d.json(&[&compose, &openapi, &ci, &multi]);
    for entry in configs(&json) {
        assert_eq!(entry["error"]["kind"], "not_a_suite", "{entry}");
        assert!(entry.get("files").is_none(), "{entry}");
    }
}

#[test]
fn list_all_marks_a_broken_suite_invalid() {
    let d = Dir::new("invalid");
    let missing_folder = d.write("m.yaml", "paths: [/no/such/folder]\nresources: {}\n");
    let bad_value = d.write("v.yaml", "paths: 5\nresources: {}\n");
    let json = d.json(&[&missing_folder, &bad_value]);
    for entry in configs(&json) {
        assert_eq!(entry["error"]["kind"], "invalid", "{entry}");
        assert!(!entry["error"]["message"].as_str().unwrap().is_empty());
    }
}

#[test]
fn list_all_keeps_going_after_a_failure() {
    let d = Dir::new("goon");
    let bad = d.write("bad.yaml", "paths: [/no/such/folder]\nresources: {}\n");
    let good = d.suite("good", "", "alpha");
    let json = d.json(&[&bad, &good]);
    assert_eq!(configs(&json)[0]["error"]["kind"], "invalid");
    assert_eq!(
        configs(&json)[1]["files"][0]["scenarios"][0]["name"],
        "alpha"
    );
}

#[test]
fn list_all_keeps_argument_order_and_drops_duplicates() {
    let d = Dir::new("order");
    let a = d.suite("a", "", "alpha");
    let b = d.suite("b", "", "beta");
    let json = d.json(&[&b, &a, &b]);
    let names: Vec<&str> = configs(&json)
        .iter()
        .map(|entry| entry["config"].as_str().unwrap())
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");
    assert!(names[0].ends_with("b.yaml") && names[1].ends_with("a.yaml"));
}

#[test]
fn list_all_answers_an_array_for_one_config() {
    let d = Dir::new("one");
    let a = d.suite("a", "", "alpha");
    assert_eq!(configs(&d.json(&[&a])).len(), 1);
}

#[test]
fn list_all_does_not_share_env_or_secrets_between_configs() {
    let d = Dir::new("isolation");
    // A holds the secret; B names it in a scenario.
    let a = d.suite("a", "", "x");
    std::fs::write(
        &a,
        format!(
            "paths: [{}]\nresources:\n  db:\n    main:\n      password: hunter2hunter2\n",
            d.path("a/features")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("config");
    let b = d.suite("b", "", "log in as hunter2hunter2");
    let orders: [[&Path; 2]; 2] = [[&a, &b], [&b, &a]];
    for order in orders {
        let json = d.json(&order);
        let entry = configs(&json)
            .iter()
            .find(|e| e["config"].as_str().unwrap().ends_with("b.yaml"))
            .expect("b is listed");
        let name = entry["files"][0]["scenarios"][0]["name"].as_str().unwrap();
        assert_eq!(name, "log in as hunter2hunter2", "{order:?}");
    }
    // And A alone still masks its own secret.
    std::fs::write(
        d.path("a/features/s.feature"),
        "Feature: f\n  Scenario: log in as hunter2hunter2\n    When set variable \"x\" to \"1\"\n",
    )
    .expect("feature");
    let json = d.json(&[&a]);
    let name = configs(&json)[0]["files"][0]["scenarios"][0]["name"]
        .as_str()
        .unwrap();
    assert_eq!(name, "log in as ***");
}

#[test]
fn list_all_expands_each_config_with_its_own_env_layer() {
    let d = Dir::new("envlayer");
    let a = d.suite("a", "", "alpha");
    std::fs::write(d.path(".env"), "NAME_FROM_ENV=alpha\n").expect("env");
    // `${NAME_FROM_ENV}` resolves from the config's own folder (here shared by
    // a.yaml and b.yaml); `c` lives in another folder with no `.env` and fails.
    let other = d.path("other");
    std::fs::create_dir_all(&other).expect("mkdir");
    let features = d.path("a/features");
    let text = |name: &str| {
        format!(
            "paths: [{}]\nresources: {{}}\n# {name}\nprivacy:\n  words: ['${{NAME_FROM_ENV}}']\n",
            features.display().to_string().replace('\\', "/")
        )
    };
    let b = d.write("b.yaml", &text("b"));
    let c = other.join("c.yaml");
    std::fs::write(&c, text("c")).expect("c");
    let json = d.json(&[&a, &b, &c]);
    assert!(configs(&json)[1].get("files").is_some(), "{json}");
    assert_eq!(configs(&json)[2]["error"]["kind"], "invalid", "{json}");
}

#[test]
fn list_all_works_without_plugins_installed() {
    let d = Dir::new("noplugins");
    let a = d.suite("a", "", "alpha");
    assert_eq!(d.run(&[&a], &[]).status.code(), Some(0));
}

#[test]
fn list_all_without_a_config_is_a_usage_error() {
    let d = Dir::new("noargs");
    assert_eq!(d.run(&[], &[]).status.code(), Some(2));
}

#[test]
fn the_human_output_has_a_heading_per_config_and_errors_inline() {
    let d = Dir::new("human");
    let a = d.suite("a", "", "alpha");
    let compose = d.write("c.yml", "services: {}\n");
    let out = d.run(&[&a, &compose], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("# ") && text.contains("s.feature:2  alpha"),
        "{text}"
    );
    assert!(
        text.contains("error (not_a_suite): missing field `paths`"),
        "{text}"
    );
}

#[test]
fn a_flow_style_env_reference_is_still_a_suite() {
    let d = Dir::new("flow");
    let a = d.suite("a", "", "alpha");
    let features = d.path("a/features");
    let dir = d.path("flow");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(
        dir.join(".env"),
        format!(
            "ROOT_299={}\n",
            features.display().to_string().replace('\\', "/")
        ),
    )
    .expect("env");
    // `{` cannot start a plain scalar inside a flow sequence, so the raw text is
    // not valid YAML until `${ROOT_299}` is expanded.
    let flow = dir.join("flow.yaml");
    std::fs::write(&flow, "paths: [${ROOT_299}]\nresources: {}\n").expect("config");
    let foreign = d.write("foreign.txt", "{ not yaml at all: [\n");
    let json = d.json(&[&a, &flow, &foreign]);
    assert!(configs(&json)[1].get("files").is_some(), "{json}");
    assert_eq!(configs(&json)[2]["error"]["kind"], "not_a_suite", "{json}");
}

#[test]
fn a_typo_in_a_real_config_is_invalid_not_skipped() {
    let d = Dir::new("typo");
    let broken = d.write("t.yaml", "paths: [unclosed\nresources: {}\n");
    let json = d.json(&[&broken]);
    assert_eq!(configs(&json)[0]["error"]["kind"], "invalid", "{json}");
}

#[test]
fn a_missing_config_file_is_invalid() {
    let d = Dir::new("missing");
    let json = d.json(&[&d.path("nope.yaml")]);
    assert_eq!(configs(&json)[0]["error"]["kind"], "invalid", "{json}");
}

#[test]
fn tag_applies_to_every_config() {
    let d = Dir::new("tags");
    let a = d.suite("a", "", "alpha");
    let b = d.suite("b", "", "beta");
    for (name, tag) in [("a", "@one"), ("b", "@two")] {
        std::fs::write(
            d.path(&format!("{name}/features/s.feature")),
            format!("Feature: f\n  {tag}\n  Scenario: tagged\n    When set variable \"x\" to \"1\"\n\n  Scenario: plain\n    When set variable \"x\" to \"1\"\n"),
        )
        .expect("feature");
    }
    let out = d.run(&[&a, &b], &["--json", "--tag", "@one"]);
    let json: Value = serde_json::from_slice(&out.stdout).expect("JSON");
    let count = |i: usize| {
        configs(&json)[i]["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|file| file["scenarios"].as_array().unwrap().len())
            .sum::<usize>()
    };
    assert_eq!((count(0), count(1)), (1, 0), "{json}");
    assert_eq!(configs(&json)[0]["selection"]["tags"][0], "@one");
}

#[test]
fn each_config_reads_the_env_layer_of_its_own_folder() {
    let d = Dir::new("ownenv");
    let mut suites = Vec::new();
    for name in ["e1", "e2"] {
        let dir = d.path(name);
        let features = dir.join("features");
        std::fs::create_dir_all(&features).expect("mkdir");
        std::fs::write(
            features.join("s.feature"),
            format!("Feature: f\n  Scenario: only {name}\n    When set variable \"x\" to \"1\"\n"),
        )
        .expect("feature");
        std::fs::write(
            dir.join(".env"),
            format!(
                "OWN_DIR_299={}\n",
                features.display().to_string().replace('\\', "/")
            ),
        )
        .expect("env");
        let config = dir.join("bddkit.yaml");
        std::fs::write(&config, "paths:\n  - ${OWN_DIR_299}\nresources: {}\n").expect("config");
        suites.push(config);
    }
    let orders: [[&Path; 2]; 2] = [[&suites[0], &suites[1]], [&suites[1], &suites[0]]];
    for order in orders {
        let json = d.json(&order);
        for (entry, config) in configs(&json).iter().zip(order) {
            let expected = if config.to_string_lossy().contains("e1") {
                "only e1"
            } else {
                "only e2"
            };
            assert_eq!(
                entry["files"][0]["scenarios"][0]["name"], expected,
                "{json}"
            );
        }
    }
}

#[test]
fn list_is_unchanged() {
    let d = Dir::new("unchanged");
    let a = d.suite("a", "", "alpha");
    let single = d.list_json(&a);
    assert!(single.get("configs").is_none() && single.get("config_source").is_some());
    assert!(single["selection"].get("paths").is_some());
    let broken = d.write("b.yaml", "paths: [/no/such/folder]\nresources: {}\n");
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["list", "--json", "--config"])
        .arg(&broken)
        .env("BDDKIT_DIR", d.path("no-plugins"))
        .output()
        .expect("run");
    let error: Value = serde_json::from_slice(&out.stdout).expect("JSON");
    assert_eq!(out.status.code(), Some(2));
    assert!(error["error"].get("kind").is_none(), "{error}");
}
