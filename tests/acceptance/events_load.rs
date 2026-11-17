use super::{of_type, read_stream, run_with_events, write_events_project};
use serde_json::Value;
use std::process::Command;

/// `files` feature files of `scenarios` scenarios of `steps` steps each, none
/// of which touches the network: the cost is the runner and the stream.
fn load_features(files: usize, scenarios: usize, steps: usize) -> Vec<(String, String)> {
    let body = "    Given set variable \"x\" to \"1\"\n".repeat(steps);
    (0..files)
        .map(|file| {
            let mut text = format!("Feature: load {file}\n");
            for scenario in 0..scenarios {
                text.push_str(&format!("  Scenario: s{scenario}\n{body}"));
            }
            (format!("f{file:04}.feature"), text)
        })
        .collect()
}

/// Four workers, not the default eight: a load test must not take the CPU the
/// timing-sensitive tests running beside it are counting on.
fn write_load_project(name: &str, features: &[(String, String)]) -> std::path::PathBuf {
    let pairs: Vec<(&str, &str)> = features
        .iter()
        .map(|(file, text)| (file.as_str(), text.as_str()))
        .collect();
    let cfg = write_events_project(name, &pairs, None);
    let mut config = std::fs::read_to_string(&cfg).expect("read config");
    config.push_str("concurrency: 4\n");
    std::fs::write(&cfg, config).expect("write config");
    cfg
}

/// What holds for any stream, even one cut short: every line parsed (the
/// reader did that), `seq` has no gap and `t` never goes back.
fn assert_stream_is_ordered(stream: &[Value]) {
    for (index, event) in stream.iter().enumerate() {
        assert_eq!(event["seq"], index, "a gap or a repeat in seq: {event}");
        if index > 0 {
            assert!(
                event["t"].as_u64() >= stream[index - 1]["t"].as_u64(),
                "t goes back: {event}"
            );
        }
    }
}

/// A stream that ended: the only `run_finished` is the last line.
fn assert_stream_is_sound(stream: &[Value]) {
    assert_stream_is_ordered(stream);
    assert_eq!(of_type(stream, "run_finished").len(), 1);
    assert_eq!(stream.last().expect("lines")["type"], "run_finished");
}

/// 40 files x 2 scenarios x 20 steps, four at a time: enough volume for the
/// ordering to mean something, a fraction of a second to run. That the files
/// really overlap is the barrier test's job, which proves it without timing.
#[test]
fn a_wide_run_streams_every_event_in_order() {
    let (files, scenarios, steps) = (40, 2, 20);
    let cfg = write_load_project("load", &load_features(files, scenarios, steps));
    let (out, stream, _) = run_with_events(&cfg, &[]);

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert_stream_is_sound(&stream);
    assert!(
        stdout.contains(&format!(
            "files: {files}, scenarios: {}, failed: 0",
            files * scenarios
        )),
        "{stdout}"
    );
    let last = stream.last().expect("lines");
    assert_eq!(
        (
            last["files"].as_u64(),
            last["scenarios"].as_u64(),
            last["failed"].as_u64()
        ),
        (
            Some(files as u64),
            Some((files * scenarios) as u64),
            Some(0)
        )
    );
    let total_steps = files * scenarios * steps;
    assert_eq!(of_type(&stream, "step_started").len(), total_steps);
    assert_eq!(of_type(&stream, "step_finished").len(), total_steps);

    // One file's lines are in causal order, whatever the others were doing.
    let mut by_file: std::collections::HashMap<&str, Vec<&Value>> =
        std::collections::HashMap::new();
    for event in &stream {
        if let Some(file) = event["file"].as_str() {
            by_file.entry(file).or_default().push(event);
        }
    }
    assert_eq!(by_file.len(), files);
    for (file, events) in &by_file {
        assert_eq!(events[0]["type"], "file_started", "{file}");
        assert_eq!(
            events.last().expect("events")["type"],
            "file_finished",
            "{file}"
        );
        let (mut in_scenario, mut open) = (false, 0_i32);
        for event in events {
            match event["type"].as_str().expect("type") {
                "scenario_started" => {
                    assert!(!in_scenario, "{file}: a scenario inside a scenario");
                    in_scenario = true;
                }
                "step_started" => {
                    assert!(in_scenario, "{file}: a step outside a scenario");
                    open += 1;
                }
                "step_finished" => {
                    assert!(open > 0, "{file}: a step closed twice");
                    open -= 1;
                }
                "scenario_finished" => {
                    assert!(in_scenario && open == 0, "{file}: closed with a step open");
                    in_scenario = false;
                }
                _ => {}
            }
        }
    }
}

/// An anchor file holds the run in a sleep, so when the first busy file has
/// finished the signal always finds the handler first to the claim and other
/// files still emitting. Nothing here depends on how fast the machine is.
#[cfg(unix)]
#[test]
fn interrupting_a_busy_run_leaves_a_terminated_stream() {
    let mut features = load_features(40, 1, 50);
    features.push((
        "anchor.feature".into(),
        "@priority(1)\nFeature: anchor\n  Scenario: hold\n    Given I sleep \"10\" seconds\n"
            .into(),
    ));
    let cfg = write_load_project("load-signal", &features);
    let events = cfg.with_file_name("events.ndjson");
    let mut child = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(&cfg)
        .arg("--events")
        .arg(&events)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn bddkit");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !std::fs::read_to_string(&events).is_ok_and(|text| text.contains("file_finished")) {
        assert!(
            std::time::Instant::now() < deadline,
            "no file ever finished"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let sent = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .expect("kill");
    assert!(sent.success());
    let status = child.wait().expect("wait");

    assert_eq!(status.code(), Some(143));
    let stream = read_stream(&events);
    assert_stream_is_sound(&stream);
    let last = stream.last().expect("lines");
    assert_eq!(
        (last["signal"].as_str(), last["exit"].as_u64()),
        (Some("SIGTERM"), Some(143))
    );
    assert_eq!(
        last["files"].as_u64(),
        Some(of_type(&stream, "file_finished").len() as u64),
        "the totals are those of the files written before the terminator"
    );
    assert!(
        of_type(&stream, "file_started").len() > of_type(&stream, "file_finished").len(),
        "the anchor was still in flight"
    );
}
