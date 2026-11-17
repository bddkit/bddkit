use super::{of_type, run_with_events, write_events_project, write_evidence_project};
use serde_json::Value;
use std::process::Command;

// Issue #109: source annotations must survive the parser without changing execution.
fn annotation_stream(name: &str, feature: &str) -> Vec<Value> {
    let cfg = write_events_project(name, &[("a.feature", feature)], None);
    let (out, stream, _) = run_with_events(&cfg, &[]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    stream
}

#[test]
fn annotations_a_comment_directly_above_a_step_reaches_its_step_started() {
    let stream = annotation_stream(
        "annotation-comment",
        "Feature: f\n  Scenario: s\n    # first paragraph\n    #\n    # second paragraph\n    Given set variable \"x\" to \"1\"\n",
    );
    assert_eq!(
        of_type(&stream, "step_started")[0]["comment"],
        "first paragraph\n\nsecond paragraph"
    );
}

#[test]
fn annotations_a_commented_step_that_matches_the_registry_is_a_disabled_step() {
    let stream = annotation_stream(
        "annotation-disabled",
        "Feature: f\n  Scenario: s\n    # Disabled until fixed.\n    # Then variable \"missing\" should be equal to \"<<undefined>>\"\n    Given set variable \"x\" to \"1\"\n",
    );
    let disabled = of_type(&stream, "step_disabled");
    assert_eq!(disabled.len(), 1, "{stream:?}");
    assert_eq!(disabled[0]["comment"], "Disabled until fixed.");
    assert_eq!(disabled[0]["keyword"], "Then ");
    assert_eq!(
        disabled[0]["text"],
        "variable \"missing\" should be equal to \"<<undefined>>\""
    );
    assert_eq!(disabled[0]["line"], 4);
    assert!(disabled[0].get("step").is_none());
    assert_eq!(of_type(&stream, "step_started")[0]["step"], 0);
}

#[test]
fn annotations_a_commented_line_that_starts_with_a_keyword_but_matches_nothing_is_a_comment() {
    let stream = annotation_stream(
        "annotation-prose",
        "Feature: f\n  Scenario: s\n    # Then we wait for the queue\n    Given set variable \"x\" to \"1\"\n",
    );
    assert!(of_type(&stream, "step_disabled").is_empty());
    assert_eq!(
        of_type(&stream, "step_started")[0]["comment"],
        "Then we wait for the queue"
    );
}

#[test]
fn annotations_a_block_followed_by_a_blank_line_starts_a_section_with_a_title_and_a_note() {
    let stream = annotation_stream(
        "annotation-section",
        "Feature: f\n  Scenario: s\n    #\n    # Setup\n    # An order paid by card.\n    #\n\n    Given set variable \"x\" to \"1\"\n    # End note\n",
    );
    let sections = of_type(&stream, "section");
    assert_eq!(sections.len(), 2, "{stream:?}");
    assert_eq!(sections[0]["title"], "Setup");
    assert_eq!(sections[0]["note"], "An order paid by card.");
    assert_eq!(sections[0]["line"], 3);
    assert_eq!(sections[1]["title"], "End note");
    assert!(sections[1].get("note").is_none());
    assert!(
        sections[1]["seq"].as_u64().unwrap()
            < of_type(&stream, "scenario_finished")[0]["seq"]
                .as_u64()
                .unwrap()
    );
}

#[test]
fn annotations_sections_and_disabled_steps_after_a_failure_are_still_emitted_in_order() {
    let cfg = write_events_project(
        "annotation-failure",
        &[(
            "a.feature",
            "Feature: f\n  Scenario: s\n    Given set variable \"x\" to \"1\"\n    Then variable \"x\" should be equal to \"2\"\n    # Later\n\n    # a skipped step's note\n    And set variable \"y\" to \"2\"\n    # Then variable \"undefined\" should be equal to \"1\"\n    # Closing\n",
        )],
        None,
    );
    let (out, stream, _) = run_with_events(&cfg, &[]);
    assert_eq!(out.status.code(), Some(1));
    let tail: Vec<_> = stream
        .iter()
        .filter(|e| {
            matches!(
                e["type"].as_str(),
                Some("section" | "step_skipped" | "step_disabled" | "scenario_finished")
            )
        })
        .map(|e| e["type"].as_str().unwrap())
        .collect();
    assert_eq!(
        tail,
        [
            "section",
            "step_skipped",
            "step_disabled",
            "section",
            "scenario_finished"
        ]
    );
    assert_eq!(
        of_type(&stream, "step_skipped")[0]["comment"],
        "a skipped step's note"
    );
    assert_eq!(of_type(&stream, "step_skipped")[0]["step"], 2);
}

#[test]
fn annotations_element_comments_attach_above_tags_and_never_include_the_language_line() {
    let stream = annotation_stream(
        "annotation-elements",
        "# file note\n# language: en\n@file\nFeature: f\n  # background note\n  Background:\n    Given set variable \"x\" to \"1\"\n  # first note\n  @first\n  Scenario: first\n    Given set variable \"y\" to \"2\"\n  # rule note\n  Rule: r\n    # rule background note\n    Background:\n      Given set variable \"z\" to \"3\"\n    # outline note\n    Scenario Outline: row\n      Given set variable \"n\" to \"<n>\"\n      # examples note\n      Examples:\n        | n |\n        | 1 |\n",
    );
    let file = of_type(&stream, "file_started")[0];
    assert_eq!(file["comment"], "file note");
    assert_eq!(file["background"]["comment"], "background note");
    let scenarios = of_type(&stream, "scenario_started");
    assert_eq!(scenarios[0]["comment"], "first note");
    assert_eq!(scenarios[1]["comment"], "outline note");
    assert_eq!(scenarios[1]["rule"]["comment"], "rule note");
    assert_eq!(
        scenarios[1]["rule"]["background"]["comment"],
        "rule background note"
    );
    assert_eq!(scenarios[1]["examples"]["comment"], "examples note");
    assert!(of_type(&stream, "section").is_empty(), "{stream:?}");
}

#[test]
fn annotations_a_disabled_outline_step_is_matched_after_placeholder_substitution() {
    let stream = annotation_stream(
        "annotation-outline",
        "Feature: f\n  Scenario Outline: row\n    # Then variable \"x\" should be equal to \"<n>\"\n    Given set variable \"n\" to \"<n>\"\n    Examples:\n      | n |\n      | 200 |\n      | 201 |\n",
    );
    let disabled = of_type(&stream, "step_disabled");
    assert_eq!(disabled.len(), 2, "{stream:?}");
    assert_eq!(
        disabled[0]["text"],
        "variable \"x\" should be equal to \"200\""
    );
    assert_eq!(
        disabled[1]["text"],
        "variable \"x\" should be equal to \"201\""
    );
}

#[test]
fn annotations_a_russian_feature_file_recognises_its_own_keywords() {
    let stream = annotation_stream(
        "annotation-ru",
        "# language: ru\nФункционал: f\n  Сценарий: s\n    # Тогда variable \"x\" should be equal to \"200\"\n    # Then variable \"x\" should be equal to \"201\"\n    Допустим set variable \"x\" to \"1\"\n",
    );
    let disabled = of_type(&stream, "step_disabled");
    assert_eq!(disabled.len(), 1, "{stream:?}");
    assert_eq!(disabled[0]["keyword"], "Тогда ");
    assert_eq!(
        of_type(&stream, "step_started")[0]["comment"],
        "Then variable \"x\" should be equal to \"201\""
    );
    assert!(of_type(&stream, "file_started")[0].get("comment").is_none());
}

#[test]
fn annotations_background_sections_repeat_before_every_scenario() {
    let stream = annotation_stream(
        "annotation-background",
        "Feature: f\n  Background:\n    # Feature setup\n\n    Given set variable \"x\" to \"1\"\n    # Feature closing\n\n  Scenario: loose\n    Given set variable \"y\" to \"1\"\n  Rule: r\n    Background:\n      # Rule setup\n\n      Given set variable \"z\" to \"1\"\n    Scenario: ruled\n      Given set variable \"y\" to \"1\"\n",
    );
    let sections = of_type(&stream, "section");
    let titles: Vec<_> = sections
        .iter()
        .map(|e| e["title"].as_str().unwrap())
        .collect();
    assert_eq!(
        titles,
        [
            "Feature setup",
            "Feature closing",
            "Feature setup",
            "Feature closing",
            "Rule setup"
        ]
    );
    assert_eq!(sections[2]["scenario"], 1);
}

#[test]
fn annotations_sections_inside_an_included_scenario_carry_parent_and_source() {
    let cfg = write_events_project(
        "annotation-include",
        &[
            (
                "caller.feature",
                "Feature: f\n  Scenario: caller\n    Given I include \"included.feature\"\n",
            ),
            (
                "included.feature",
                "Feature: included\n  Background:\n    # Background\n\n    Given set variable \"x\" to \"1\"\n  Scenario: child\n    # Child\n\n    # Then variable \"missing\" should be equal to \"1\"\n    Given set variable \"y\" to \"1\"\n    # Closing\n",
            ),
        ],
        None,
    );
    let (out, stream, _) = run_with_events(&cfg, &[]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let caller = of_type(&stream, "scenario_started")
        .into_iter()
        .find(|e| e["name"] == "caller")
        .unwrap();
    let sections: Vec<_> = of_type(&stream, "section")
        .into_iter()
        .filter(|e| e["file"] == caller["file"])
        .collect();
    assert_eq!(sections.len(), 3, "{stream:?}");
    for item in sections.into_iter().chain(
        of_type(&stream, "step_disabled")
            .into_iter()
            .filter(|e| e["file"] == caller["file"]),
    ) {
        assert_eq!(item["parent"], 0, "{item}");
        assert!(
            item["source"]
                .as_str()
                .unwrap()
                .ends_with("included.feature")
        );
    }
}

#[test]
fn annotations_a_secret_in_a_comment_is_masked() {
    let cfg = write_events_project(
        "annotation-secrets",
        &[(
            "a.feature",
            "Feature: f\n  Scenario: s\n    Given set variable \"secret_value\" to \"annotation-secret-9876\"\n    # annotation-secret-9876\n\n    # annotation-secret-9876\n    # Then variable \"x\" should be equal to \"annotation-secret-9876\"\n    # annotation-secret-9876\n    Given set variable \"y\" to \"1\"\n",
        )],
        None,
    );
    let (out, stream, _) = run_with_events(&cfg, &[]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(of_type(&stream, "section")[0]["title"], "***");
    let disabled = of_type(&stream, "step_disabled");
    assert_eq!(disabled[0]["comment"], "***");
    assert!(!disabled[0].to_string().contains("annotation-secret-9876"));
    assert_eq!(of_type(&stream, "step_started")[1]["comment"], "***");
}

#[test]
fn annotations_doctor_counts_disabled_steps_and_lists_lines_that_look_like_steps() {
    let cfg = write_evidence_project(
        "annotation-doctor",
        &[
            (
                "a.feature",
                "Feature: f\n  Background:\n    # Given set variable \"disabled\" to \"1\"\n    Given set variable \"x\" to \"1\"\n  Scenario Outline: rows\n    # Then variable \"x\" should be equal to \"<n>\"\n    # Then we wait for the queue\n    Given set variable \"n\" to \"<n>\"\n    Examples:\n      | n |\n      | 200 |\n      | 201 |\n",
            ),
            (
                "b.feature",
                "Feature: g\n  Scenario: s\n    # Given set variable \"disabled\" to \"2\"\n    Given set variable \"y\" to \"1\"\n",
            ),
        ],
        None,
    );
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--json", "--config"])
        .arg(&cfg)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    let steps = report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["stage"] == "steps")
        .unwrap();
    assert_eq!(steps["status"], "ok");
    let detail = steps["detail"].as_str().unwrap();
    assert!(
        detail.contains("2 file(s), every step matched; 3 disabled step(s) in 2 file(s)"),
        "{detail}"
    );
    assert!(detail.contains("a.feature:7"), "{detail}");
    assert_eq!(
        detail.matches("a.feature:7").count(),
        1,
        "outline rows must not duplicate a warning: {detail}"
    );
}

#[test]
fn annotations_a_hash_inside_a_docstring_or_a_table_is_content() {
    for fence in ["\"\"\"", "```"] {
        let feature = format!(
            "Feature: f\n  Scenario: s\n    Given I echo the plain doc string:\n      {fence}\n      # Then variable \"x\" should be equal to \"200\"\n      # A false section\n\n      # Another false section\n      {fence}\n    And Print table:\n      | name | value |\n      | field | # Then variable \"x\" should be equal to \"201\" |\n    # a real note\n    Given set variable \"x\" to \"1\"\n"
        );
        let cfg = write_evidence_project(
            &format!(
                "annotation-content-{}",
                if fence == "```" { "backtick" } else { "quote" }
            ),
            &[("a.feature", &feature)],
            None,
        );
        let (out, stream, _) = run_with_events(&cfg, &[]);
        assert_eq!(out.status.code(), Some(0), "{out:?}");
        assert!(of_type(&stream, "section").is_empty(), "{stream:?}");
        assert!(of_type(&stream, "step_disabled").is_empty(), "{stream:?}");
        assert_eq!(of_type(&stream, "step_started").len(), 3);
        assert_eq!(
            of_type(&stream, "step_started")[2]["comment"],
            "a real note"
        );
    }
}

#[test]
fn annotations_disabled_macro_calls_match_without_executing_or_validating_them() {
    let cfg = write_events_project(
        "annotation-macro",
        &[(
            "a.feature",
            "Feature: f\n  Scenario: s\n    # Given my disabled action\n    Given set variable \"x\" to \"1\"\n",
        )],
        Some(
            "- step: 'my disabled action'\n  do:\n    - 'variable \"missing\" should be equal to \"never\"'\n",
        ),
    );
    let (out, stream, _) = run_with_events(&cfg, &[]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(
        of_type(&stream, "step_disabled")[0]["text"],
        "my disabled action"
    );
    assert_eq!(of_type(&stream, "step_started").len(), 1);
}

#[test]
fn annotations_include_failures_still_emit_nested_sections_disabled_and_skipped_steps() {
    let cfg = write_evidence_project(
        "annotation-include-failure",
        &[(
            "a.feature",
            "Feature: f\n  Scenario: caller\n    Given I include \"child.feature\"\n",
        )],
        None,
    );
    std::fs::write(cfg.parent().unwrap().join("features/child.feature"), "Feature: child\n  Scenario: child\n    Given set variable \"x\" to \"1\"\n    Then variable \"x\" should be equal to \"2\"\n    # Later\n\n    # Then variable \"x\" should be equal to \"200\"\n    # skipped note\n    Given set variable \"y\" to \"1\"\n    # Closing\n").unwrap();
    let (out, stream, _) = run_with_events(&cfg, &[]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let nested: Vec<_> = stream
        .iter()
        .filter(|e| {
            e.get("parent") == Some(&serde_json::json!(0))
                && matches!(
                    e["type"].as_str(),
                    Some("section" | "step_disabled" | "step_skipped")
                )
        })
        .collect();
    assert_eq!(
        nested
            .iter()
            .map(|e| e["type"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["section", "step_disabled", "step_skipped", "section"]
    );
    assert_eq!(nested[2]["comment"], "skipped note");
    assert_eq!(nested[2]["step"], 3);
    for event in nested {
        assert!(event["source"].as_str().unwrap().ends_with("child.feature"));
    }
}

#[test]
fn annotations_the_junit_and_cucumber_reports_do_not_change() {
    let feature = "# File note\nFeature: f\n  Description\n  Background:\n    # Setup\n\n    Given set variable \"x\" to \"1\"\n  # Scenario note\n  Scenario: s\n    # step note\n    Given set variable \"y\" to \"1\"\n    # Disabled reason\n    # Then variable \"missing\" should be equal to \"<<undefined>>\"\n\n    # Checks\n\n    Then variable \"y\" should be equal to \"1\"\n    # Closing\n";
    let cfg = write_events_project("annotation-report-compat", &[("a.feature", feature)], None);
    let dir = cfg.parent().unwrap();
    let junit = dir.join("junit.xml");
    let cucumber = dir.join("cucumber.json");
    let reports = || {
        let (out, stream, _) = run_with_events(
            &cfg,
            &[
                "--junit",
                junit.to_str().unwrap(),
                "--cucumber-json",
                cucumber.to_str().unwrap(),
            ],
        );
        assert_eq!(out.status.code(), Some(0), "{out:?}");
        let timing = regex::Regex::new(r#"time="[0-9.]+"|\([0-9.]+s\)"#).unwrap();
        let xml = std::fs::read_to_string(&junit).unwrap();
        let xml = timing.replace_all(&xml, "TIMING").into_owned();
        #[expect(
            clippy::items_after_statements,
            reason = "the helper reads next to its only use"
        )]
        fn normalize_times(value: &mut Value) {
            match value {
                Value::Object(map) => {
                    map.remove("duration");
                    for child in map.values_mut() {
                        normalize_times(child);
                    }
                }
                Value::Array(values) => {
                    for child in values {
                        normalize_times(child);
                    }
                }
                _ => {}
            }
        }
        let mut json: Value = serde_json::from_slice(&std::fs::read(&cucumber).unwrap()).unwrap();
        normalize_times(&mut json);
        (xml, json, stream)
    };
    let (annotated_xml, annotated_json, stream) = reports();
    assert_eq!(of_type(&stream, "step_disabled").len(), 1);
    assert_eq!(of_type(&stream, "section").len(), 3);
    let blanked = feature
        .lines()
        .map(|line| {
            if line.trim_start().starts_with('#') {
                ""
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(dir.join("features/a.feature"), blanked).unwrap();
    let (plain_xml, plain_json, _) = reports();
    assert_eq!(annotated_xml, plain_xml);
    assert_eq!(annotated_json, plain_json);
}

#[test]
fn annotations_a_disabled_step_masks_short_secret_parameters_by_position() {
    let stream = annotation_stream(
        "annotation-short-secret",
        "Feature: f\n  Scenario: s\n    # Given I encrypt \"payload\" with AES using key \"x\" as \"reg\"\n    Given set variable \"x\" to \"1\"\n",
    );
    let disabled = of_type(&stream, "step_disabled");
    assert_eq!(disabled.len(), 1, "{stream:?}");
    assert!(
        disabled[0]["text"]
            .as_str()
            .unwrap()
            .contains("key \"***\"")
    );
}

#[test]
fn annotations_docstring_fences_never_hide_executable_assertions() {
    for (name, body) in [
        ("inline", "      \"\"\"hello\"\"\"\n"),
        (
            "closing-comment",
            "      \"\"\"\n      hello\n      \"\"\" # body ends\n",
        ),
        ("backtick-inline", "      ```hello```\n"),
        (
            "backtick-comment",
            "      ```\n      hello\n      ``` # body ends\n",
        ),
    ] {
        let feature = format!(
            "Feature: f\n  Scenario: s\n    Given I echo the plain doc string:\n{body}    # A real section\n\n    # A real assertion's comment\n    Then variable \"missing\" should be equal to \"wrong\"\n"
        );
        let cfg = write_evidence_project(
            &format!("annotation-fence-{name}"),
            &[("a.feature", &feature)],
            None,
        );
        let (out, stream, _) = run_with_events(&cfg, &[]);
        assert_eq!(out.status.code(), Some(1), "{name}: {out:?}");
        assert_eq!(
            of_type(&stream, "step_started").len(),
            2,
            "{name}: {stream:?}"
        );
        assert_eq!(of_type(&stream, "section")[0]["title"], "A real section");
        assert_eq!(
            of_type(&stream, "step_started")[1]["comment"],
            "A real assertion's comment"
        );
    }
}

#[test]
fn annotations_closing_outline_blocks_after_examples_repeat_for_every_row_of_that_block() {
    let stream = annotation_stream(
        "annotation-outline-closing",
        "Feature: f\n  Scenario Outline: rows\n    Given set variable \"n\" to \"<n>\"\n    Examples: first\n      | n |\n      | 200 |\n      | 201 |\n    # Then variable \"x\" should be equal to \"<n>\"\n    # Closing <n>\n",
    );
    let disabled = of_type(&stream, "step_disabled");
    let sections = of_type(&stream, "section");
    assert_eq!(disabled.len(), 2, "{stream:?}");
    assert_eq!(
        disabled[0]["text"],
        "variable \"x\" should be equal to \"200\""
    );
    assert_eq!(
        disabled[1]["text"],
        "variable \"x\" should be equal to \"201\""
    );
    assert_eq!(sections.len(), 2, "{stream:?}");
    assert_eq!(sections[0]["title"], "Closing <n>");
    assert_eq!(sections[1]["title"], "Closing <n>");
    for (index, section) in sections.iter().enumerate() {
        assert_eq!(section["scenario"], index);
        assert!(
            section["seq"].as_u64().unwrap()
                < of_type(&stream, "scenario_finished")[index]["seq"]
                    .as_u64()
                    .unwrap()
        );
    }
}

#[test]
fn annotations_a_docstring_comment_containing_a_fence_never_hides_a_failure() {
    for fence in ["\"\"\"", "```"] {
        let feature = format!(
            "Feature: f\n  Scenario: s\n    Given set variable \"x\" to \"1\"\n    Then variable \"x\" should be equal to \"2\"\n    Given I echo the plain doc string:\n      {fence}\n      # A literal {fence} in a body comment\n      # Then variable \"x\" should be equal to \"200\"\n      ordinary body content\n      {fence}\n    Given set variable \"y\" to \"1\"\n"
        );
        let cfg = write_evidence_project(
            &format!(
                "annotation-comment-fence-{}",
                if fence == "```" { "backtick" } else { "quote" }
            ),
            &[("a.feature", &feature)],
            None,
        );
        let (out, stream, _) = run_with_events(&cfg, &[]);
        assert_eq!(out.status.code(), Some(1), "{out:?}");
        assert_eq!(of_type(&stream, "step_started").len(), 2, "{stream:?}");
        assert_eq!(of_type(&stream, "step_skipped").len(), 2, "{stream:?}");
        assert!(of_type(&stream, "step_disabled").is_empty(), "{stream:?}");
        assert!(of_type(&stream, "section").is_empty(), "{stream:?}");
    }
}

#[test]
fn annotations_comments_between_table_rows_are_not_step_annotations() {
    let feature = "Feature: f\n  Scenario: s\n    Given Print table:\n      | name | value |\n      | a | 1 |\n      # Then variable \"x\" should be equal to \"200\"\n      # note about the next row\n      | b | 2 |\n    Given set variable \"x\" to \"1\"\n";
    let cfg = write_evidence_project("annotation-table-comments", &[("a.feature", feature)], None);
    let (out, stream, _) = run_with_events(&cfg, &[]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(of_type(&stream, "step_disabled").is_empty(), "{stream:?}");
    assert!(of_type(&stream, "section").is_empty(), "{stream:?}");
    assert!(
        of_type(&stream, "step_started")[1].get("comment").is_none(),
        "{stream:?}"
    );
}

#[test]
fn annotations_outline_substitution_changes_disabled_steps_and_preserves_prose() {
    let stream = annotation_stream(
        "annotation-outline-prose",
        "Feature: f\n  Scenario Outline: rows\n    # Section <email>\n    # Note <email>\n\n    # the service sends <email> a link\n    Given set variable \"email\" to \"<email>\"\n    # Then the service sends <email> a link\n    # Then variable \"x\" should be equal to \"<code>\"\n\n    Examples:\n      | email | code |\n      | a@b.c | 200 |\n",
    );
    let section = of_type(&stream, "section")[0];
    assert_eq!(section["title"], "Section <email>");
    assert_eq!(section["note"], "Note <email>");
    assert_eq!(
        of_type(&stream, "step_started")[0]["comment"],
        "the service sends <email> a link"
    );
    let disabled = of_type(&stream, "step_disabled")[0];
    assert_eq!(
        disabled["text"],
        "variable \"x\" should be equal to \"200\""
    );
    assert_eq!(disabled["comment"], "Then the service sends <email> a link");
}

#[test]
fn annotations_examples_notes_only_follow_rows_from_their_own_table() {
    let stream = annotation_stream(
        "annotation-examples-scope",
        "Feature: f\n  Scenario Outline: rows\n    Given set variable \"n\" to \"<n>\"\n    Examples: first\n      | n |\n      | 200 |\n\n    # Closing first\n\n    Examples: second\n      | n |\n      | 201 |\n\n    # Closing second\n",
    );
    let sections = of_type(&stream, "section");
    assert_eq!(sections.len(), 2, "{stream:?}");
    assert_eq!(sections[0]["title"], "Closing first");
    assert_eq!(sections[0]["scenario"], 0);
    assert_eq!(sections[1]["title"], "Closing second");
    assert_eq!(sections[1]["scenario"], 1);
}

#[test]
fn annotations_comments_outside_step_regions_are_ignored_and_scenario_closing_notes_are_kept() {
    let stream = annotation_stream(
        "annotation-region-boundaries",
        "Feature: f\n  # outside any step region\n\n  Scenario: first\n    Given set variable \"x\" to \"1\"\n\n    # closing first\n\n  Scenario: second\n    Given set variable \"y\" to \"1\"\n  Rule: r\n    # outside the rule's scenarios\n\n    Scenario: third\n      Given set variable \"z\" to \"1\"\n",
    );
    let sections = of_type(&stream, "section");
    assert_eq!(sections.len(), 1, "{stream:?}");
    assert_eq!(sections[0]["title"], "closing first");
    assert_eq!(sections[0]["scenario"], 0);
}

#[test]
fn annotations_a_note_after_a_table_belongs_to_the_next_step() {
    let cfg = write_evidence_project(
        "annotation-after-table",
        &[(
            "a.feature",
            "Feature: f\n  Scenario: s\n    Given Print table:\n      | name | value |\n      | a | 1 |\n      # table-only note\n      | b | 2 |\n    # real step note\n    Given set variable \"x\" to \"1\"\n",
        )],
        None,
    );
    let (out, stream, _) = run_with_events(&cfg, &[]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(
        of_type(&stream, "step_started")[1]["comment"],
        "real step note"
    );
}

#[test]
fn annotations_doctor_reports_a_disabled_step_that_fails_to_match_in_one_outline_row() {
    let cfg = write_evidence_project(
        "annotation-partial-match",
        &[(
            "a.feature",
            "Feature: f\n  Scenario Outline: rows\n    # Then I sleep <code> seconds\n    Given set variable \"code\" to \"<code>\"\n    Examples:\n      | code |\n      | 200 |\n      | nope |\n",
        )],
        None,
    );
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--json", "--config"])
        .arg(&cfg)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    let steps = report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["stage"] == "steps")
        .unwrap();
    assert_eq!(steps["status"], "ok");
    let detail = steps["detail"].as_str().unwrap();
    assert!(
        detail.contains("1 disabled step(s) in 1 file(s)"),
        "{detail}"
    );
    assert!(
        detail.contains("a.feature:3"),
        "the unmatched row is still a finding: {detail}"
    );
}

#[test]
fn annotations_an_included_docstring_comment_never_hides_a_failure() {
    let cfg = write_evidence_project(
        "annotation-include-fence",
        &[(
            "a.feature",
            "Feature: f\n  Scenario: caller\n    Given I include \"../child.feature\"\n",
        )],
        None,
    );
    std::fs::write(cfg.parent().unwrap().join("child.feature"), "Feature: child\n  Scenario: child\n    Given set variable \"x\" to \"1\"\n    Then variable \"x\" should be equal to \"2\"\n    Given I echo the plain doc string:\n      \"\"\"\n      # A literal \"\"\" in a body comment\n      # Then variable \"x\" should be equal to \"200\"\n      ordinary body content\n      \"\"\"\n    Given set variable \"y\" to \"1\"\n").unwrap();
    let (out, stream, _) = run_with_events(&cfg, &[]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let skipped = of_type(&stream, "step_skipped");
    assert_eq!(skipped.len(), 2, "{stream:?}");
    assert!(skipped.iter().all(|event| event["parent"] == 0));
    assert!(of_type(&stream, "step_disabled").is_empty(), "{stream:?}");
}
