use super::{
    build_include_project, mkfifo, of_type, read_stream, run_with_events, write_events_project,
    write_parallel_fixture, write_report_project,
};
use serde_json::Value;
use std::process::Command;

#[test]
fn two_files_running_together_interleave_in_the_stream() {
    // The barrier opens only when both steps are in flight, so both
    // `step_started` lines must precede both `step_finished` ones — which a
    // stream buffered until its file ends could never show.
    let dir = std::env::temp_dir().join(format!("bddkit-events-barrier-{}", std::process::id()));
    let cfg = write_parallel_fixture(&dir, 2, "");
    let (out, stream, _) = run_with_events(&cfg, &[]);

    assert!(
        out.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let barrier = |kind: &str| -> Vec<u64> {
        of_type(&stream, kind)
            .into_iter()
            .filter(|e| {
                e["step"] == 1 // the barrier; step 0 sets a variable
            })
            .map(|e| e["seq"].as_u64().expect("seq"))
            .collect()
    };
    let (started, finished) = (barrier("step_started"), barrier("step_finished"));
    assert_eq!((started.len(), finished.len()), (2, 2), "{stream:?}");
    assert!(
        started.iter().max() < finished.iter().min(),
        "the second file started its request only after the first one finished: {stream:?}"
    );
    let files: std::collections::HashSet<_> = of_type(&stream, "file_started")
        .iter()
        .map(|e| e["file"].as_str().expect("file").to_string())
        .collect();
    assert_eq!(files.len(), 2);
}

#[test]
fn steps_after_a_failure_are_reported_as_skipped() {
    let cfg = write_report_project("events-skipped");
    let (_, stream, _) = run_with_events(&cfg, &[]);

    let second: Vec<_> = stream.iter().filter(|e| e["scenario"] == 1).collect();
    let kinds: Vec<_> = second
        .iter()
        .map(|e| (e["type"].as_str().unwrap_or(""), e["step"].as_u64()))
        .collect();
    assert_eq!(
        kinds,
        [
            ("scenario_started", None),
            ("step_started", Some(0)),
            ("step_finished", Some(0)),
            ("step_started", Some(1)),
            ("step_finished", Some(1)),
            ("step_started", Some(2)),
            ("step_finished", Some(2)),
            ("step_skipped", Some(3)),
            ("scenario_finished", None),
        ]
    );
    assert_eq!(second[6]["status"], "failed");
    assert_eq!(
        second[7]["text"],
        "variable \"ping\" should be equal to \"ok\""
    );
    assert_eq!(second[8]["status"], "failed");
}

#[test]
fn outline_rows_get_distinct_example_ordinals() {
    let cfg = write_events_project(
        "outline",
        &[(
            "o.feature",
            "Feature: o\n  Scenario: plain\n    Given set variable \"x\" to \"1\"\n\n  Scenario Outline: rows\n    Given set variable \"x\" to \"<n>\"\n\n    Examples:\n      | n |\n      | 1 |\n      | 2 |\n\n    Examples:\n      | n |\n      | 3 |\n",
        )],
        None,
    );
    let (out, stream, _) = run_with_events(&cfg, &[]);

    assert_eq!(out.status.code(), Some(0));
    let started = of_type(&stream, "scenario_started");
    assert_eq!(started.len(), 4);
    assert_eq!(started[0].get("example"), None, "a plain scenario has none");
    let rows: Vec<_> = started[1..]
        .iter()
        .map(|e| {
            (
                e["scenario"].as_u64(),
                e["example"].as_u64(),
                e["line"].as_u64(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            (Some(1), Some(0), Some(5)),
            (Some(2), Some(1), Some(5)),
            (Some(3), Some(2), Some(5))
        ]
    );
}

#[test]
fn a_macro_body_step_is_an_event_with_its_caller_as_parent() {
    let cfg = write_events_project(
        "macro",
        &[(
            "m.feature",
            "Feature: m\n  Scenario: s\n    Given I remember two things\n",
        )],
        Some(
            "# a comment, so the definition is not on line 1\n\n- step: 'I remember two things'\n  do:\n    - set variable \"a\" to \"1\"\n    - set variable \"b\" to \"2\"\n",
        ),
    );
    let (out, stream, _) = run_with_events(&cfg, &[]);

    assert_eq!(out.status.code(), Some(0));
    let started = of_type(&stream, "step_started");
    assert_eq!(started.len(), 3, "{stream:?}");
    assert_eq!(
        (started[0]["step"].as_u64(), started[0].get("parent")),
        (Some(0), None)
    );
    for (position, child) in started[1..].iter().enumerate() {
        assert_eq!(child["step"], position + 1);
        assert_eq!(child["parent"], 0);
        assert_eq!(child["index"], position);
        assert_eq!(child["keyword"], "");
        assert_eq!(child["line"], 3, "the definition's line: {child}");
        assert!(
            child["source"]
                .as_str()
                .expect("source")
                .ends_with("macros/m.yaml")
        );
    }
    // The caller closes after its body.
    let finished: Vec<_> = of_type(&stream, "step_finished")
        .iter()
        .map(|e| e["step"].as_u64())
        .collect();
    assert_eq!(finished, [Some(1), Some(2), Some(0)]);
}

#[test]
fn an_included_step_is_an_event_with_its_own_source_and_line() {
    let dir = build_include_project(
        "events-include",
        &[
            (
                "features/target.feature",
                "tests/features/include/target.feature",
            ),
            (
                "features/caller.feature",
                "tests/features/include/caller.feature",
            ),
        ],
    );
    let events = dir.join("events.ndjson");
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config", "cfg.yaml", "--events"])
        .arg(&events)
        .current_dir(&dir)
        .output()
        .expect("run bddkit");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );

    // caller.feature sorts before target.feature, and runs as its own file.
    let stream = read_stream(&events);
    let caller = of_type(&stream, "step_started")
        .into_iter()
        .filter(|e| {
            e["file"]
                .as_str()
                .is_some_and(|f| f.ends_with("caller.feature"))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        caller.len(),
        3,
        "include, the included step, the assertion: {caller:?}"
    );
    let included = caller[1];
    assert_eq!(included["parent"], 0);
    assert_eq!(included["line"], 4, "its own line in the included file");
    assert!(
        included["source"]
            .as_str()
            .expect("source")
            .ends_with("target.feature")
    );
    assert_eq!(included["text"], "set variable \"userId\" to \"abc123\"");
    assert_eq!(caller[2].get("parent"), None, "back at the top level");
}

#[test]
fn the_stream_and_the_junit_report_agree_on_every_total() {
    let cfg = write_report_project("events-totals");
    std::fs::write(
        cfg.with_file_name("features/second.feature"),
        "Feature: second\n  Scenario: one\n    Given set variable \"ping\" to \"ok\"\n    Then variable \"ping\" should be equal to \"nope\"\n\n  Scenario: two\n    Given set variable \"ping\" to \"ok\"\n",
    )
    .expect("second feature");
    let junit = cfg.with_file_name("junit.xml");
    let (_, stream, _) = run_with_events(&cfg, &["--junit", junit.to_str().expect("UTF-8")]);

    let xml = std::fs::read_to_string(&junit).expect("junit");
    let package = sxd_document::parser::parse(&xml).expect("junit parses");
    let value = |xpath: &str| {
        sxd_xpath::evaluate_xpath(&package.as_document(), xpath)
            .expect("xpath")
            .string()
    };
    let last = stream.last().expect("lines");
    assert_eq!(last["files"].to_string(), value("count(//testsuite)"));
    assert_eq!(last["scenarios"].to_string(), value("count(//testcase)"));
    assert_eq!(last["failed"].to_string(), value("count(//failure)"));
    assert_eq!(
        (
            last["files"].as_u64(),
            last["scenarios"].as_u64(),
            last["failed"].as_u64()
        ),
        (Some(2), Some(4), Some(2))
    );
    for suite in of_type(&stream, "file_finished") {
        let file = suite["file"].as_str().expect("file");
        let name = file.rsplit('/').next().expect("name");
        assert_eq!(
            suite["failed"].to_string(),
            value(&format!(
                "string(//testsuite[contains(@name,'{name}')]/@failures)"
            )),
            "{suite}"
        );
    }
}

#[test]
fn a_run_without_events_prints_the_same_console_output() {
    let cfg = write_report_project("events-console");
    let events = cfg.with_file_name("events.ndjson");
    let normalised = |out: &std::process::Output| {
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let run_id = stdout
            .lines()
            .find_map(|line| line.strip_prefix("run "))
            .expect("the run id line")
            .to_string();
        (stdout.replace(&run_id, "<run>"), out.status.code())
    };
    let plain = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(&cfg)
        .output()
        .expect("run bddkit");
    let streamed = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(&cfg)
        .arg("--events")
        .arg(&events)
        .output()
        .expect("run bddkit");

    assert_eq!(normalised(&plain), normalised(&streamed));
}

#[test]
fn a_null_sentinel_in_a_failure_is_written_as_the_null_slot() {
    let cfg = write_report_project("events-null");
    let (_, stream, events) = run_with_events(&cfg, &[]);

    let failure = of_type(&stream, "scenario_finished")
        .into_iter()
        .find_map(|e| e["failure"].as_str())
        .expect("the failing scenario carries its failure");
    assert!(failure.contains("<<null>>"), "{failure}");
    let raw = std::fs::read(&events).expect("read");
    assert!(!raw.contains(&0), "no NUL byte reaches the stream");
    assert!(
        !String::from_utf8_lossy(&raw).contains("\\u0000"),
        "nor its JSON escape"
    );
}

#[test]
fn the_events_path_can_come_from_the_environment_and_the_flag_wins() {
    let cfg = write_report_project("events-env");
    let from_env = cfg.with_file_name("from-env.ndjson");
    let from_flag = cfg.with_file_name("from-flag.ndjson");

    Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(&cfg)
        .env("BDDKIT_EVENTS", &from_env)
        .output()
        .expect("run bddkit");
    assert_eq!(read_stream(&from_env)[0]["type"], "run_started");

    let _ = std::fs::remove_file(&from_env);
    Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(&cfg)
        .arg("--events")
        .arg(&from_flag)
        .env("BDDKIT_EVENTS", &from_env)
        .output()
        .expect("run bddkit");
    assert_eq!(read_stream(&from_flag)[0]["type"], "run_started");
    assert!(
        !from_env.exists(),
        "the flag wins; the variable is not opened"
    );

    // A templated `BDDKIT_EVENTS=` is "off", not a path to fail on.
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(&cfg)
        .env("BDDKIT_EVENTS", "")
        .output()
        .expect("run bddkit");
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The reader opens first and blocks until the run opens its end; end-of-file
/// would come early if the path were opened, closed and opened again.
#[cfg(unix)]
#[test]
fn events_can_be_written_to_a_fifo() {
    let cfg = write_report_project("events-fifo");
    let fifo = cfg.with_file_name("events.fifo");
    mkfifo(&fifo);

    let reader = {
        let fifo = fifo.clone();
        std::thread::spawn(move || std::fs::read_to_string(fifo).expect("read the fifo"))
    };
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(&cfg)
        .arg("--events")
        .arg(&fifo)
        .output()
        .expect("run bddkit");
    let text = reader.join().expect("reader");

    assert_eq!(out.status.code(), Some(1));
    let lines: Vec<Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).expect("one JSON object per line"))
        .collect();
    assert_eq!(lines[0]["type"], "run_started");
    assert_eq!(lines.last().expect("lines")["type"], "run_finished");
}

/// `/dev/full` opens for writing and fails every write with ENOSPC.
#[cfg(target_os = "linux")]
#[test]
fn a_stream_that_cannot_be_written_turns_the_exit_code_to_two() {
    let cfg = write_events_project(
        "full",
        &[(
            "a.feature",
            "Feature: a\n  Scenario: s\n    Given set variable \"x\" to \"1\"\n",
        )],
        None,
    );
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(&cfg)
        .args(["--events", "/dev/full"])
        .output()
        .expect("run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stdout}\n{stderr}");
    assert!(stderr.contains("cannot write the events file"), "{stderr}");
    assert!(
        stdout.contains("failed: 0"),
        "the run itself completed: {stdout}"
    );
}
