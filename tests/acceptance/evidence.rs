use super::{of_type, read_stream, run_with_events, write_events_project, write_evidence_project};
use serde_json::{Value, json};
use std::process::Command;

// ---------------------------------------------------------------------------
// `run --events --evidence`
// ---------------------------------------------------------------------------

/// One scenario of `steps`, run with the evidence on; the stream it wrote.
fn evidence_run(name: &str, steps: &str, extra: &[&str]) -> Vec<Value> {
    let cfg = write_evidence_project(
        name,
        &[("e.feature", &format!("Feature: e\n  Scenario: s\n{steps}"))],
        None,
    );
    let mut args = vec!["--evidence"];
    args.extend_from_slice(extra);
    run_with_events(&cfg, &args).1
}

fn step_text<'a>(stream: &'a [Value], text: &str) -> &'a Value {
    of_type(stream, "step_started")
        .into_iter()
        .find(|e| e["text"].as_str().is_some_and(|t| t.contains(text)))
        .unwrap_or_else(|| panic!("no step {text:?} in {stream:?}"))
}

#[test]
fn evidence_requires_events() {
    let cfg = write_events_project("evidence-alone", &[], None);
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--evidence", "--config"])
        .arg(&cfg)
        .env_remove("BDDKIT_EVENTS")
        .output()
        .expect("run bddkit");
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--evidence requires --events"), "{stderr}");

    // The variable counts as `--events`; the check is not clap's `requires`.
    let events = cfg.with_file_name("from-env.ndjson");
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--evidence", "--config"])
        .arg(&cfg)
        .env("BDDKIT_EVENTS", &events)
        .output()
        .expect("run bddkit");
    assert_eq!(out.status.code(), Some(2), "no feature files");
    assert!(
        !String::from_utf8_lossy(&out.stderr).contains("requires --events"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn without_evidence_the_stream_holds_only_lifecycle_events() {
    let cfg = write_evidence_project(
        "no-evidence",
        &[(
            "e.feature",
            "Feature: e\n  Scenario: s\n    Given set variable \"a\" to \"<<unique(token)>>\"\n    Given I echo one piece of evidence\n    Then variable \"a\" should not be equal to \"\"\n",
        )],
        None,
    );
    let (_, stream, _) = run_with_events(&cfg, &[]);

    let lifecycle = [
        "run_started",
        "file_started",
        "scenario_started",
        "step_started",
        "step_finished",
        "step_skipped",
        "scenario_finished",
        "file_finished",
        "run_finished",
    ];
    for event in &stream {
        assert!(
            lifecycle.contains(&event["type"].as_str().expect("type")),
            "{event}"
        );
        assert!(event.get("vars").is_none(), "{event}");
    }
    assert_eq!(stream[0]["evidence"], false);
}

#[test]
fn a_step_reports_its_resolved_slots_beside_the_raw_text() {
    let stream = evidence_run(
        "evidence-vars",
        "    Given set variable \"who\" to \"ann\"\n    And set variable \"greeting\" to \"hi <<who>>\"\n",
        &[],
    );
    assert_eq!(stream[0]["evidence"], true);
    let step = step_text(&stream, "\"greeting\"");
    assert_eq!(step["text"], "set variable \"greeting\" to \"hi <<who>>\"");
    assert_eq!(step["vars"], json!([["who", "ann"]]));
    assert!(
        step_text(&stream, "\"who\"").get("vars").is_none(),
        "no slot, no key"
    );
}

#[test]
fn a_repeated_slot_appears_once_per_occurrence() {
    let stream = evidence_run(
        "evidence-repeated",
        "    Given set variable \"pair\" to \"<<unique(token)>>|<<unique(token)>>\"\n    Then variable \"pair\" should not be equal to \"x\"\n",
        &[],
    );
    let vars = step_text(&stream, "\"pair\" to")["vars"]
        .as_array()
        .expect("vars")
        .clone();
    assert_eq!(vars.len(), 2);
    assert_eq!(vars[0][0], "unique(token)");
    assert_eq!(vars[1][0], "unique(token)");
    assert_ne!(vars[0][1], vars[1][1], "a new value each time");
}

#[test]
fn a_null_slot_is_reported_as_json_null() {
    let stream = evidence_run(
        "evidence-null",
        "    Given set variable \"n\" to \"<<null>>\"\n",
        &[],
    );
    assert_eq!(step_text(&stream, "\"n\"")["vars"], json!([["null", null]]));
}

#[test]
fn a_secret_slot_is_masked_in_vars() {
    let stream = evidence_run(
        "evidence-secret-slot",
        // `ab` is shorter than `min_length`: only position can hide it.
        "    Given set variable \"my_secret\" to \"ab\"\n    And set variable \"copy\" to \"<<my_secret>>\"\n    And set variable \"k\" to \"long-key-value-1\"\n    And I encrypt \"<<k>>\" with AES using key \"<<k>>\" as \"c\"\n",
        &[],
    );
    assert_eq!(
        step_text(&stream, "\"copy\"")["vars"],
        json!([["my_secret", "***"]]),
        "a sensitive name"
    );
    assert_eq!(
        step_text(&stream, "AES")["vars"],
        json!([["k", "***"], ["k", "***"]]),
        "a secret parameter, and a value another parameter also quotes"
    );
}

#[test]
fn evidence_inside_a_macro_attaches_to_the_innermost_step() {
    let cfg = write_evidence_project(
        "evidence-macro",
        &[(
            "m.feature",
            "Feature: m\n  Scenario: s\n    Given I ping the service\n",
        )],
        Some("- step: 'I ping the service'\n  do:\n    - I echo one piece of evidence\n"),
    );
    let (_, stream, _) = run_with_events(&cfg, &["--evidence"]);

    let evidence = of_type(&stream, "evidence");
    assert_eq!(evidence.len(), 1, "{stream:?}");
    assert_eq!(evidence[0]["step"], 1, "the body step, not the caller (0)");
}

// ---------------------------------------------------------------------------
// `--evidence <sources>`: marked steps and failed scenarios
// ---------------------------------------------------------------------------

/// `features` run with `--events` and then `args`.
fn modes_run(
    name: &str,
    features: &[(&str, &str)],
    macros: Option<&str>,
    args: &[&str],
) -> Vec<Value> {
    let cfg = write_evidence_project(name, features, macros);
    run_with_events(&cfg, args).1
}

/// The `step` of every `evidence` event of scenario `scenario`, in order.
fn evidence_steps(stream: &[Value], scenario: u64) -> Vec<u64> {
    of_type(stream, "evidence")
        .into_iter()
        .filter(|e| e["scenario"] == scenario)
        .map(|e| e["step"].as_u64().expect("step"))
        .collect()
}

const PASS_AND_FAIL: &str = "Feature: e
  Scenario: passes
    Given set variable \"who\" to \"ann\"
    When I echo one piece of evidence for \"<<who>>\"
  Scenario: fails
    Given set variable \"who\" to \"bob\"
    When I echo one piece of evidence for \"<<who>>\"
    Then variable \"who\" should be equal to \"zed\"
";

#[test]
fn marked_records_only_the_steps_between_the_two_steps() {
    let stream = modes_run(
        "marked-between",
        &[(
            "e.feature",
            "Feature: e\n  Scenario: s\n    Given I echo one piece of evidence\n    And I am collecting evidence\n    Given I echo one piece of evidence\n    And I am not collecting evidence\n    Then I echo one piece of evidence\n",
        )],
        None,
        &["--evidence=marked"],
    );
    assert_eq!(evidence_steps(&stream, 0), [2], "{stream:?}");
    assert!(
        of_type(&stream, "evidence")[0].get("deferred").is_none(),
        "a marked step is written live"
    );
}

#[test]
fn the_evidence_tag_marks_a_whole_scenario_and_is_inherited_from_feature_and_rule() {
    let stream = modes_run(
        "marked-tag",
        &[
            (
                "a.feature",
                "Feature: a\n  Background:\n    Given I echo one piece of evidence\n\n  @evidence\n  Scenario: tagged\n    Given I echo one piece of evidence\n    And I am not collecting evidence\n    Then I echo one piece of evidence\n\n  Scenario: plain\n    Given I echo one piece of evidence\n\n  @evidence\n  Rule: r\n    Scenario: in the rule\n      Given I echo one piece of evidence\n",
            ),
            (
                "b.feature",
                "@evidence\nFeature: b\n  Scenario: s\n    Given I echo one piece of evidence\n",
            ),
        ],
        None,
        &["--evidence=marked"],
    );
    let of = |file: &str, scenario: u64| -> Vec<u64> {
        of_type(&stream, "evidence")
            .into_iter()
            .filter(|e| e["file"].as_str().is_some_and(|f| f.ends_with(file)))
            .filter(|e| e["scenario"] == scenario)
            .map(|e| e["step"].as_u64().expect("step"))
            .collect()
    };
    assert_eq!(
        of("a.feature", 0),
        [0, 1, 3],
        "the Background too, and a step cannot switch the tag off"
    );
    assert_eq!(of("a.feature", 1), Vec::<u64>::new());
    assert_eq!(of("a.feature", 2), [0, 1], "inherited from the rule");
    assert_eq!(of("b.feature", 0), [0], "inherited from the feature");
}

#[test]
fn a_mark_ends_with_the_scenario() {
    let stream = modes_run(
        "marked-reset",
        &[(
            "e.feature",
            "Feature: e\n  Scenario: marks\n    Given I am collecting evidence\n    Given I echo one piece of evidence\n  Scenario: next\n    Given I echo one piece of evidence\n",
        )],
        None,
        &["--evidence=marked"],
    );
    assert_eq!(evidence_steps(&stream, 0), [1]);
    assert_eq!(evidence_steps(&stream, 1), Vec::<u64>::new());
}

#[test]
fn a_mark_covers_macro_and_include_bodies_called_inside_it() {
    let cfg = write_evidence_project(
        "marked-nested",
        &[(
            "e.feature",
            "Feature: e\n  Scenario: s\n    Given I am collecting evidence\n    When I ping the service\n    And I include \"../lib/inc.feature\"\n",
        )],
        Some("- step: 'I ping the service'\n  do:\n    - I echo one piece of evidence\n"),
    );
    std::fs::create_dir_all(cfg.with_file_name("lib")).expect("mkdir");
    std::fs::write(
        cfg.with_file_name("lib/inc.feature"),
        "Feature: inc\n  Scenario: i\n    Given I echo one piece of evidence\n",
    )
    .expect("include");
    let (_, stream, _) = run_with_events(&cfg, &["--evidence=marked"]);
    // 0 marks, 1 the macro, 2 its body, 3 the include, 4 its step.
    assert_eq!(evidence_steps(&stream, 0), [2, 4], "{stream:?}");
}

#[test]
fn marks_record_nothing_without_the_marked_source() {
    let stream = modes_run(
        "marked-unused",
        &[(
            "e.feature",
            "Feature: e\n  @evidence\n  Scenario: s\n    Given I am collecting evidence\n    Given I echo one piece of evidence\n",
        )],
        None,
        &["--evidence=failures"],
    );
    assert!(of_type(&stream, "evidence").is_empty(), "{stream:?}");
}

#[test]
fn failures_records_every_step_of_a_failed_scenario_and_nothing_of_a_passing_one() {
    let stream = modes_run(
        "failures",
        &[("e.feature", PASS_AND_FAIL)],
        None,
        &["--evidence=failures"],
    );
    assert_eq!(evidence_steps(&stream, 0), Vec::<u64>::new());
    assert_eq!(evidence_steps(&stream, 1), [1]);
    assert!(
        of_type(&stream, "step_started")
            .iter()
            .all(|e| e.get("vars").is_none()),
        "a deferred step's values cannot ride on step_started"
    );
}

#[test]
fn deferred_events_carry_their_step_and_arrive_before_scenario_finished() {
    let stream = modes_run(
        "failures-order",
        &[("e.feature", PASS_AND_FAIL)],
        None,
        &["--evidence=failures"],
    );
    let seq = |e: &Value| e["seq"].as_u64().expect("seq");
    let evidence = of_type(&stream, "evidence");
    assert_eq!(evidence.len(), 1);
    assert_eq!(evidence[0]["deferred"], true);
    assert_eq!(evidence[0]["step"], 1);
    let finished = of_type(&stream, "scenario_finished")[1];
    let last_step = of_type(&stream, "step_finished")
        .into_iter()
        .filter(|e| e["scenario"] == 1)
        .map(seq)
        .max()
        .expect("steps");
    assert!(last_step < seq(evidence[0]) && seq(evidence[0]) < seq(finished));
}

#[test]
fn deferred_vars_arrive_as_a_vars_event() {
    let stream = modes_run(
        "failures-vars",
        &[("e.feature", PASS_AND_FAIL)],
        None,
        &["--evidence=failures"],
    );
    let vars = of_type(&stream, "vars");
    assert_eq!(vars.len(), 1, "only the failed scenario's: {vars:?}");
    assert_eq!(vars[0]["scenario"], 1);
    assert_eq!(vars[0]["step"], 1);
    assert_eq!(vars[0]["vars"], json!([["who", "bob"]]));
    assert_eq!(vars[0]["deferred"], true);
}

#[test]
fn marked_and_failures_combine() {
    let stream = modes_run(
        "marked-failures",
        &[(
            "e.feature",
            &PASS_AND_FAIL.replace(
                "  Scenario: passes\n",
                "  Scenario: passes\n    Given I am collecting evidence\n",
            ),
        )],
        None,
        &["--evidence=marked,failures"],
    );
    let evidence = of_type(&stream, "evidence");
    assert_eq!(evidence.len(), 2, "{evidence:?}");
    assert_eq!(evidence[0]["scenario"], 0);
    assert!(evidence[0].get("deferred").is_none(), "marked: live");
    assert_eq!(evidence[1]["scenario"], 1);
    assert_eq!(evidence[1]["deferred"], true, "failed: deferred");
    assert_eq!(stream[0]["evidence_sources"], json!(["marked", "failures"]));
}

#[test]
fn bare_evidence_means_all() {
    let stream = evidence_run(
        "bare-evidence",
        "    Given set variable \"who\" to \"ann\"\n    And set variable \"hi\" to \"<<who>>\"\n",
        &[],
    );
    assert_eq!(stream[0]["evidence_sources"], json!(["all"]));
    assert_eq!(
        step_text(&stream, "\"hi\"")["vars"],
        json!([["who", "ann"]])
    );
}

/// Sources take an `=`, so a bare `--evidence` before a feature path is still
/// the flag and the path, as when the flag took no value.
#[test]
fn a_bare_evidence_before_a_path_keeps_its_meaning() {
    let cfg = write_events_project(
        "bare-before-path",
        &[(
            "e.feature",
            "Feature: e\n  Scenario: s\n    Given set variable \"a\" to \"1\"\n",
        )],
        None,
    );
    let feature = cfg.with_file_name("features/e.feature");
    let (out, stream, _) = run_with_events(&cfg, &["--evidence", feature.to_str().expect("UTF-8")]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(stream[0]["evidence_sources"], json!(["all"]));

    // Without the `=`, a source name is a path that does not exist.
    let (out, _, _) = run_with_events(&cfg, &["--evidence", "marked"]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn an_unknown_source_is_refused() {
    let cfg = write_events_project(
        "unknown-source",
        &[(
            "e.feature",
            "Feature: e\n  Scenario: s\n    Given set variable \"a\" to \"1\"\n",
        )],
        None,
    );
    let events = cfg.with_file_name("events.ndjson");
    let run = |args: &[&str], env: Option<&str>| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_bddkit"));
        command
            .args(["run", "--config"])
            .arg(&cfg)
            .arg("--events")
            .arg(&events)
            .args(args)
            .env_remove("BDDKIT_EVIDENCE");
        if let Some(value) = env {
            command.env("BDDKIT_EVIDENCE", value);
        }
        command.output().expect("run bddkit")
    };
    for out in [
        run(&["--evidence=marked,bogus"], None),
        run(&[], Some("bogus")),
    ] {
        assert_eq!(out.status.code(), Some(2));
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("\"bogus\""), "{stderr}");
        assert!(stderr.contains("marked, failures or all"), "{stderr}");
    }

    let text = std::fs::read_to_string(&cfg).expect("config");
    std::fs::write(&cfg, format!("{text}evidence: [bogus]\n")).expect("config");
    let out = run(&[], None);
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("marked, failures or all"), "{stderr}");
}

#[test]
fn the_config_key_without_a_stream_is_ignored_and_the_flag_without_a_stream_is_refused() {
    let cfg = write_events_project(
        "config-evidence",
        &[(
            "e.feature",
            "Feature: e\n  Scenario: s\n    Given set variable \"a\" to \"1\"\n",
        )],
        None,
    );
    let text = std::fs::read_to_string(&cfg).expect("config");
    std::fs::write(&cfg, format!("{text}evidence: [marked]\n")).expect("config");
    let bare = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_bddkit"));
        command
            .args(["run", "--config"])
            .arg(&cfg)
            .env_remove("BDDKIT_EVENTS")
            .env_remove("BDDKIT_EVIDENCE");
        command
    };

    let out = bare().output().expect("run bddkit");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let out = bare()
        .env("BDDKIT_EVIDENCE", "all")
        .output()
        .expect("run bddkit");
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("$BDDKIT_EVIDENCE requires --events"),
        "{stderr}"
    );

    // With a stream: the config applies, the variable beats it, the flag both.
    let sources = |args: &[&str], env: Option<&str>| {
        let events = cfg.with_file_name("events.ndjson");
        let mut command = bare();
        command.arg("--events").arg(&events).args(args);
        if let Some(value) = env {
            command.env("BDDKIT_EVIDENCE", value);
        }
        command.output().expect("run bddkit");
        read_stream(&events)[0]["evidence_sources"].clone()
    };
    assert_eq!(sources(&[], None), json!(["marked"]));
    assert_eq!(sources(&[], Some("failures")), json!(["failures"]));
    assert_eq!(sources(&["--evidence"], Some("failures")), json!(["all"]));
}

/// A scenario whose evidence is deferred, held in a `sleep` when the signal
/// lands: nothing of it reaches the stream. This pins the outcome end to end
/// only; the guard for a scenario that FAILS after the signal (a plugin
/// instance dropped by the shutdown) is pinned by the unit test
/// `a_scenario_that_fails_after_an_interrupt_drops_its_deferred_evidence`.
#[cfg(unix)]
#[test]
fn an_interrupted_scenario_writes_no_deferred_evidence() {
    let cfg = write_events_project(
        "deferred-interrupt",
        &[(
            "a.feature",
            "Feature: held\n  Scenario: s\n    Given set variable \"who\" to \"ann\"\n    And set variable \"hi\" to \"<<who>>\"\n    And I sleep \"10\" seconds\n",
        )],
        None,
    );
    let events = cfg.with_file_name("events.ndjson");
    let mut child = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(&cfg)
        .arg("--events")
        .arg(&events)
        .args(["--evidence=failures"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn bddkit");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !std::fs::read_to_string(&events).is_ok_and(|text| text.contains("I sleep")) {
        assert!(
            std::time::Instant::now() < deadline,
            "the sleep never started"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let sent = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .expect("kill");
    assert!(sent.success());
    assert_eq!(child.wait().expect("wait").code(), Some(130));

    let stream = read_stream(&events);
    assert_eq!(stream.last().expect("lines")["type"], "run_finished");
    assert!(of_type(&stream, "vars").is_empty(), "{stream:?}");
}

#[test]
fn scenario_finished_reports_evidence_coverage() {
    let feature = PASS_AND_FAIL.replace(
        "  Scenario: passes\n",
        "  Scenario: passes\n    Given I am collecting evidence\n",
    );
    let coverage = |args: &[&str]| -> Vec<Value> {
        let stream = modes_run("coverage", &[("e.feature", &feature)], None, args);
        of_type(&stream, "scenario_finished")
            .into_iter()
            .map(|e| e["evidence"].clone())
            .collect()
    };
    assert_eq!(coverage(&[]), [json!("none"), json!("none")]);
    assert_eq!(coverage(&["--evidence"]), [json!("full"), json!("full")]);
    assert_eq!(
        coverage(&["--evidence=marked,failures"]),
        [json!("partial"), json!("full")],
        "the marking step itself is not marked; the failed scenario is whole"
    );
    assert_eq!(
        coverage(&["--evidence=failures"]),
        [json!("none"), json!("full")]
    );
}

/// A run refused over an evidence variable empties a stale stream like any
/// other refusal; a buffer nothing would use is never read.
#[test]
fn an_evidence_variable_refusal_empties_the_stream_and_an_unused_buffer_is_ignored() {
    let cfg = write_events_project(
        "evidence-refused",
        &[(
            "e.feature",
            "Feature: e\n  Scenario: s\n    Given set variable \"a\" to \"1\"\n",
        )],
        None,
    );
    let events = cfg.with_file_name("events.ndjson");
    let run = |vars: &[(&str, &str)], args: &[&str]| {
        std::fs::write(&events, "{\"type\":\"yesterday\"}\n").expect("stale stream");
        let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
            .args(["run", "--config"])
            .arg(&cfg)
            .arg("--events")
            .arg(&events)
            .args(args)
            .env_remove("BDDKIT_EVIDENCE")
            .env_remove("BDDKIT_EVIDENCE_BUFFER")
            .envs(vars.iter().copied())
            .output()
            .expect("run bddkit");
        (out.status.code(), read_stream(&events))
    };
    for (vars, args) in [
        (&[("BDDKIT_EVIDENCE", "bogus")][..], &[][..]),
        (
            &[("BDDKIT_EVIDENCE_BUFFER", "64M")][..],
            &["--evidence=failures"][..],
        ),
    ] {
        let (code, stream) = run(vars, args);
        assert_eq!(code, Some(2), "{vars:?}");
        assert!(stream.is_empty(), "{vars:?}: {stream:?}");
    }
    let (code, _) = run(&[("BDDKIT_EVIDENCE_BUFFER", "64M")], &[]);
    assert_eq!(code, Some(0), "no evidence, no buffer to read");
}

#[test]
fn run_started_keeps_the_evidence_boolean_and_adds_the_sources() {
    let cfg = write_events_project(
        "run-started-sources",
        &[(
            "e.feature",
            "Feature: e\n  Scenario: s\n    Given set variable \"a\" to \"1\"\n",
        )],
        None,
    );
    let started = |args: &[&str]| run_with_events(&cfg, args).1[0].clone();
    let off = started(&[]);
    assert_eq!(off["evidence"], false);
    assert_eq!(off["evidence_sources"], json!([]));
    let on = started(&["--evidence=failures,marked,failures"]);
    assert_eq!(on["evidence"], true);
    assert_eq!(on["evidence_sources"], json!(["marked", "failures"]));
}
