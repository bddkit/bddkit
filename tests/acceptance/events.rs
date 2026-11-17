use super::{
    masking_project, of_type, path_text, read_stream, run_masking, run_with_events,
    write_events_project, write_report_project,
};
use serde_json::Value;
use std::process::Command;

#[test]
fn an_events_path_is_truncated_before_the_config_is_read() {
    let cfg = write_events_project("broken", &[], None);
    std::fs::write(
        &cfg,
        "paths: [features]\nresources:\n  api: {}\nmacro_paths: [${BDDKIT_ABSENT_VAR}]\n",
    )
    .expect("write config");
    std::fs::write(
        cfg.with_file_name("events.ndjson"),
        "{\"type\":\"yesterday\"}\n",
    )
    .expect("stale stream");

    let (out, stream, _) = run_with_events(&cfg, &[]);

    assert_eq!(out.status.code(), Some(2));
    assert!(stream.is_empty(), "{stream:?}");
}

#[test]
fn a_refused_run_leaves_an_empty_events_file() {
    let cfg = write_events_project(
        "refused",
        &[(
            "a.feature",
            "Feature: a\n  Scenario: s\n    Given a step nobody defined\n",
        )],
        None,
    );
    std::fs::write(
        cfg.with_file_name("events.ndjson"),
        "{\"type\":\"yesterday\"}\n",
    )
    .expect("stale stream");

    let (out, stream, _) = run_with_events(&cfg, &[]);

    assert_eq!(out.status.code(), Some(2));
    assert!(stream.is_empty(), "{stream:?}");
}

#[test]
fn every_line_of_the_stream_is_one_json_object_with_a_rising_seq() {
    let cfg = write_report_project("events-lines");
    let (_, stream, _) = run_with_events(&cfg, &[]);

    assert!(stream.len() > 6, "{stream:?}");
    for (index, event) in stream.iter().enumerate() {
        assert!(event.is_object(), "{event}");
        assert_eq!(event["seq"], index, "{event}");
        assert!(event["type"].is_string(), "{event}");
        if index > 0 {
            assert!(
                event["t"].as_u64() >= stream[index - 1]["t"].as_u64(),
                "t goes back: {event}"
            );
        }
    }
}

#[test]
fn run_started_names_the_artifact_root() {
    let cfg = write_report_project("events-artifact-root");
    let (_, stream, _) = run_with_events(&cfg, &[]);
    let run_id = stream[0]["run_id"].as_str().expect("run_id");
    assert_eq!(
        stream[0]["artifacts_dir"],
        std::env::temp_dir()
            .join(format!("bddkit-{run_id}"))
            .display()
            .to_string(),
        "the default root, absolute"
    );

    // A relative flag value is reported as resolved against the working directory.
    let (_, stream, _) = run_with_events(&cfg, &["--artifacts-dir", "evidence"]);
    let root = stream[0]["artifacts_dir"].as_str().expect("artifacts_dir");
    assert!(std::path::Path::new(root).is_absolute(), "{root}");
    assert_eq!(
        std::path::Path::new(root),
        std::env::current_dir().expect("cwd").join("evidence")
    );
}

#[test]
fn the_stream_opens_with_run_started_and_closes_with_run_finished() {
    let cfg = write_report_project("events-ends");
    let (out, stream, _) = run_with_events(&cfg, &[]);

    let stdout = String::from_utf8_lossy(&out.stdout);
    let first = &stream[0];
    assert_eq!(first["type"], "run_started");
    assert_eq!(first["schema"], 1);
    assert_eq!(first["bddkit"], env!("CARGO_PKG_VERSION"));
    assert_eq!(first["files"], 1);
    assert!(first["started_at_unix_ms"].as_u64().unwrap_or(0) > 1_700_000_000_000);
    let run_id = first["run_id"].as_str().expect("run_id");
    assert!(stdout.contains(&format!("run {run_id}")), "{stdout}");

    let last = stream.last().expect("lines");
    assert_eq!(last["type"], "run_finished");
    assert_eq!(last["exit"], 1, "one scenario of the project fails");
    assert_eq!(last.get("signal"), None);
    assert_eq!(of_type(&stream, "run_finished").len(), 1);
}

#[test]
fn feature_background_rule_and_examples_descriptions_reach_the_stream() {
    let cfg = write_events_project(
        "descriptions-file",
        &[
            (
                "a.feature",
                "@ft1\n# c\n@ft2\nFeature: Described\n  What the file is for.\n\n    Indented line.\n\n  Background: Setup\n    Runs first.\n    Given set variable \"a\" to \"1\"\n\n  Scenario: s\n    Given set variable \"b\" to \"2\"\n",
            ),
            (
                "b.feature",
                "Feature: Bare\n  Background:\n    Given set variable \"a\" to \"1\"\n  Scenario: s\n    Given set variable \"b\" to \"2\"\n",
            ),
            (
                "d.feature",
                "Feature:\n  Scenario: s\n    Given set variable \"a\" to \"1\"\n",
            ),
            (
                "c.feature",
                "Feature: Ruled\n  Scenario: Loose\n    Given set variable \"a\" to \"1\"\n  Rule: Money\n    Everything about money.\n    Background: Prep\n      Prep text.\n      Given set variable \"a\" to \"1\"\n    Scenario Outline: pay\n      Given set variable \"n\" to \"<n>\"\n      Examples: Amounts\n        Rows of amounts.\n        | n |\n        | 1 |\n        | 2 |\n",
            ),
        ],
        None,
    );
    let (out, stream, _) = run_with_events(&cfg, &[]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");

    // A scenario inside a rule names the rule, its background and, for an
    // outline row, the Examples block it came from.
    let scenarios = of_type(&stream, "scenario_started");
    let rows: Vec<_> = scenarios
        .iter()
        .filter(|e| e["name"].as_str().is_some_and(|n| n.starts_with("pay")))
        .collect();
    assert_eq!(rows.len(), 2, "{scenarios:?}");
    for row in rows {
        assert_eq!(
            row["rule"],
            serde_json::json!({
                "line": 4, "name": "Money", "description": "Everything about money.",
                "background": {"line": 6, "name": "Prep", "description": "Prep text."},
            })
        );
        assert_eq!(
            row["examples"],
            serde_json::json!({"line": 11, "name": "Amounts", "description": "Rows of amounts."})
        );
    }
    let loose = scenarios
        .iter()
        .find(|e| e["name"] == "Loose")
        .expect("Loose");
    assert!(
        loose.get("rule").is_none() && loose.get("examples").is_none(),
        "{loose}"
    );

    let started = of_type(&stream, "file_started");
    let file = |name: &str| started.iter().find(|e| e["name"] == name).expect(name);

    let described = file("Described");
    assert_eq!(
        described["description"],
        "What the file is for.\n\n  Indented line."
    );
    assert_eq!(described["tags"], serde_json::json!(["@ft1", "@ft2"]));
    assert_eq!(
        described["background"],
        serde_json::json!({"line": 9, "name": "Setup", "description": "Runs first."})
    );

    // An unnamed feature keeps the `name` it always had.
    assert_eq!(file("")["name"], "");

    // Nothing to say is nothing in the stream, except `tags`, always a list.
    let bare = file("Bare");
    assert_eq!(bare["tags"], serde_json::json!([]));
    assert_eq!(bare["background"], serde_json::json!({"line": 2}));
    assert!(bare.get("description").is_none(), "{bare}");
}

#[test]
fn a_scenario_description_reaches_the_stream_with_its_paragraphs() {
    let cfg = write_events_project(
        "descriptions-scenario",
        &[(
            "a.feature",
            "Feature: f\n  Scenario: Described\n    Why this exists.\n\n      Indented bit.\n\n    Second paragraph.\n    Given set variable \"a\" to \"1\"\n  Scenario: Bare\n    Given set variable \"a\" to \"1\"\n",
        )],
        None,
    );
    let (out, stream, _) = run_with_events(&cfg, &[]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let started = of_type(&stream, "scenario_started");
    let by = |name: &str| started.iter().find(|e| e["name"] == name).expect(name);
    assert_eq!(
        by("Described")["description"],
        "Why this exists.\n\n  Indented bit.\n\nSecond paragraph."
    );
    assert!(by("Bare").get("description").is_none(), "{}", by("Bare"));
}

#[test]
fn scenario_started_carries_the_tags_the_filter_saw() {
    let cfg = write_events_project(
        "descriptions-tags",
        &[
            (
                "a.feature",
                "@f @dup\nFeature: Tagged\n  Scenario: Untagged\n    Given set variable \"a\" to \"1\"\n  @r\n  Rule: R\n    @s @dup\n    Scenario: Picked\n      Given set variable \"a\" to \"1\"\n",
            ),
            (
                "b.feature",
                "Feature: Plain\n  Scenario: Plain one\n    Given set variable \"a\" to \"1\"\n",
            ),
        ],
        None,
    );

    // Own tags, then the rule's, then the feature's, each name once.
    let (_, stream, _) = run_with_events(&cfg, &["--tag", "s"]);
    let started = of_type(&stream, "scenario_started");
    assert_eq!(
        started.len(),
        1,
        "only what the filter selected: {started:?}"
    );
    assert_eq!(
        started[0]["tags"],
        serde_json::json!(["@s", "@dup", "@r", "@f"])
    );

    let (_, stream, _) = run_with_events(&cfg, &[]);
    let started = of_type(&stream, "scenario_started");
    let tags = |name: &str| started.iter().find(|e| e["name"] == name).expect(name)["tags"].clone();
    assert_eq!(tags("Untagged"), serde_json::json!(["@f", "@dup"]));
    assert_eq!(tags("Plain one"), serde_json::json!([]), "always present");
}

#[test]
fn a_row_carries_the_tags_of_its_examples_block() {
    let cfg = write_events_project(
        "examples-tags",
        &[(
            "a.feature",
            "@f\nFeature: Outline\n  @r\n  Rule: R\n    @s @slow\n    Scenario Outline: Row <n>\n      Given set variable \"a\" to \"<n>\"\n\n      @slow @fast\n      Examples:\n        | n |\n        | 1 |\n        | 2 |\n\n      Examples:\n        | n |\n        | 3 |\n",
        )],
        None,
    );
    let report = cfg.with_file_name("cucumber.json");
    let (out, stream, _) = run_with_events(
        &cfg,
        &["--tag", "fast", "--cucumber-json", report.to_str().unwrap()],
    );
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let started = of_type(&stream, "scenario_started");
    let names: Vec<_> = started
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["Row <n> [1]", "Row <n> [2]"],
        "only the tagged block"
    );
    // Scenario, rule, feature, then the block; `@slow` is written twice, kept once.
    assert_eq!(
        started[0]["tags"],
        serde_json::json!(["@s", "@slow", "@r", "@f", "@fast"])
    );

    let json: Value = serde_json::from_slice(&std::fs::read(&report).expect("report")).unwrap();
    let tags: Vec<_> = json[0]["elements"][0]["tags"]
        .as_array()
        .expect("tags")
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(tags, ["@s", "@slow", "@r", "@f", "@fast"]);

    // A tag on the outline still selects every row; the untagged block's row
    // does not carry the other block's tags.
    let (_, stream, _) = run_with_events(&cfg, &["--tag", "s"]);
    let started = of_type(&stream, "scenario_started");
    assert_eq!(started.len(), 3, "{started:?}");
    assert_eq!(
        started[2]["tags"],
        serde_json::json!(["@s", "@slow", "@r", "@f"])
    );
}

#[test]
fn an_examples_tag_alone_selects_its_rows_and_their_rules_background() {
    let cfg = write_events_project(
        "examples-tags-rule-background",
        &[(
            "a.feature",
            "Feature: Outline\n  Rule: R\n    Background:\n      Given no such step\n\n    Scenario Outline: Row\n      Given set variable \"a\" to \"<n>\"\n\n      @only\n      Examples:\n        | n |\n        | 1 |\n",
        )],
        None,
    );
    let (out, _, _) = run_with_events(&cfg, &["--tag", "only"]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("no such step"),
        "{out:?}"
    );
}

#[test]
fn validation_skips_the_rows_an_examples_tag_left_out() {
    let cfg = write_events_project(
        "examples-tags-validation",
        &[(
            "a.feature",
            "Feature: Outline\n  Scenario Outline: Row\n    Given <step>\n\n    @fast\n    Examples:\n      | step |\n      | set variable \"a\" to \"1\" |\n\n    Examples:\n      | step |\n      | no such step |\n",
        )],
        None,
    );
    let (out, _, _) = run_with_events(&cfg, &["--tag", "fast"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
}

#[test]
fn the_cucumber_json_report_carries_descriptions_tags_and_the_feature_line() {
    let cfg = write_events_project(
        "descriptions-json",
        &[(
            "a.feature",
            "# language: en\n@ft @dup\nFeature: Reported\n  Feature text.\n  Scenario: Described\n    What it is for.\n    Given set variable \"a\" to \"1\"\n  @own @dup\n  Scenario Outline: Row\n    Outline text.\n    Given set variable \"n\" to \"<n>\"\n    Examples:\n      | n |\n      | 1 |\n      | 2 |\n  Scenario: Bare\n    Given set variable \"a\" to \"1\"\n",
        )],
        None,
    );
    let json = cfg.with_file_name("cucumber.json");
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(&cfg)
        .arg("--cucumber-json")
        .arg(&json)
        .output()
        .expect("run bddkit");
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let report: Value =
        serde_json::from_str(&std::fs::read_to_string(&json).expect("report")).expect("json");

    let feature = &report[0];
    assert_eq!(feature["line"], 3, "the keyword line, not 1");
    assert_eq!(feature["description"], "Feature text.");
    let ft = serde_json::json!([{"name": "@ft", "line": 2}, {"name": "@dup", "line": 2}]);
    assert_eq!(feature["tags"], ft);

    let elements = feature["elements"].as_array().expect("elements");
    let by = |prefix: &str| -> Vec<&Value> {
        elements
            .iter()
            .filter(|e| e["name"].as_str().is_some_and(|n| n.starts_with(prefix)))
            .collect()
    };
    assert_eq!(by("Described")[0]["description"], "What it is for.");
    assert_eq!(
        by("Described")[0]["tags"],
        ft,
        "inherited, with their own lines"
    );

    // Every row of an outline carries the outline's text and tags; a name
    // written at two levels is listed once, on the line of the closest.
    let rows = by("Row");
    assert_eq!(rows.len(), 2);
    for row in rows {
        assert_eq!(row["description"], "Outline text.");
        assert_eq!(
            row["tags"],
            serde_json::json!([
                {"name": "@own", "line": 8}, {"name": "@dup", "line": 8}, {"name": "@ft", "line": 2}
            ])
        );
    }

    // Cucumber's own formatter writes "" for no description.
    assert_eq!(by("Bare")[0]["description"], "");
}

#[test]
fn a_description_is_masked_in_the_json_report_with_the_final_set_but_not_rewritten_in_the_stream() {
    let feature = "Feature: Late\n  Scenario: first\n    Quotes late-value-98765 before it is known.\n    Given set variable \"x\" to \"1\"\n  Scenario: second\n    Given set variable \"secret_late\" to \"late-value-98765\"\n";
    let cfg = masking_project("mask-description-json", feature, "");
    let json = cfg.with_file_name("cucumber.json");
    let events = cfg.with_file_name("events.ndjson");
    let (code, _, stderr) = run_masking(
        &cfg,
        &[
            "--cucumber-json",
            json.to_str().expect("utf-8"),
            "--events",
            events.to_str().expect("utf-8"),
        ],
        &[],
    );
    assert_eq!(code, 0, "{stderr}");

    let report: Value =
        serde_json::from_str(&std::fs::read_to_string(&json).expect("report")).expect("json");
    assert_eq!(
        report[0]["elements"][0]["description"],
        "Quotes *** before it is known."
    );
    // The stream is never rewritten: the value was not a secret yet.
    assert_eq!(
        of_type(&read_stream(&events), "scenario_started")[0]["description"],
        "Quotes late-value-98765 before it is known."
    );
}

#[test]
fn run_started_carries_the_config_and_the_selection() {
    let cfg = write_events_project(
        "run-started-selection",
        &[(
            "a.feature",
            "Feature: f\n  @smoke\n  Scenario: s\n    Given set variable \"a\" to \"1\"\n",
        )],
        None,
    );
    let features = cfg.with_file_name("features");
    let features_text = path_text(&features);

    // Named on the command line, with a selection.
    let (out, stream, _) =
        run_with_events(&cfg, &["--tag", "smoke", "--tag", "@x", &features_text]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(stream[0]["config"], path_text(&cfg));
    assert_eq!(stream[0]["config_source"], "flag");
    assert_eq!(
        stream[0]["selection"],
        serde_json::json!({"tags": ["smoke", "@x"], "paths": [features_text]}),
        "the values as typed"
    );

    // Nothing selected: both lists are there, empty.
    let (_, stream, _) = run_with_events(&cfg, &[]);
    assert_eq!(
        stream[0]["selection"],
        serde_json::json!({"tags": [], "paths": []})
    );

    // Named by the environment, and found in the working directory.
    let events = cfg.with_file_name("events.ndjson");
    let run = |dir: &std::path::Path, env: Option<&std::path::Path>| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_bddkit"));
        command
            .current_dir(dir)
            .env_remove("BDDKIT_CONFIG")
            .env_remove("BDDKIT_ARTIFACTS_DIR")
            .args(["run", "--events"])
            .arg(&events);
        if let Some(env) = env {
            command.env("BDDKIT_CONFIG", env);
        }
        assert_eq!(command.output().expect("run bddkit").status.code(), Some(0));
        read_stream(&events).remove(0)
    };
    let first = run(&std::env::temp_dir(), Some(&cfg));
    assert_eq!(first["config_source"], "env");
    assert_eq!(first["config"], path_text(&cfg));

    std::fs::copy(&cfg, cfg.with_file_name("bddkit.yaml")).expect("copy config");
    let first = run(cfg.parent().expect("dir"), None);
    assert_eq!(first["config_source"], "discovered");
    assert_eq!(
        first["config"], "./bddkit.yaml",
        "the path as it was resolved"
    );
}
