use super::{mkfifo, of_type, read_stream, write_events_project};
use std::process::Command;

/// A run held in a `sleep` is interrupted once its first step is on disk.
/// Whatever the files in flight emit after the signal, the last line of the
/// stream is the terminator, and there is exactly one.
#[cfg(unix)]
#[test]
fn an_interrupted_run_ends_its_stream_with_the_signal() {
    for (signal, code, name) in [
        ("INT", 130, "SIGINT"),
        ("TERM", 143, "SIGTERM"),
        ("HUP", 129, "SIGHUP"),
    ] {
        let feature = "Feature: held\n  Scenario: s\n    Given I sleep \"10\" seconds\n";
        let cfg = write_events_project(
            &format!("signal-{signal}"),
            &[("a.feature", feature), ("b.feature", feature)],
            None,
        );
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

        // Both files are inside their sleep when the signal lands.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while std::fs::read_to_string(&events)
            .map_or(0, |text| text.matches("step_started").count())
            < 2
        {
            assert!(
                std::time::Instant::now() < deadline,
                "the steps never started"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let sent = Command::new("kill")
            .args([&format!("-{signal}"), &child.id().to_string()])
            .status()
            .expect("kill");
        assert!(sent.success());
        let status = child.wait().expect("wait");

        assert_eq!(status.code(), Some(code), "{name}");
        let stream = read_stream(&events);
        let last = stream.last().expect("lines");
        assert_eq!(last["type"], "run_finished", "{name}: {stream:?}");
        assert_eq!(last["signal"], name);
        assert_eq!(last["exit"], code);
        assert_eq!(of_type(&stream, "run_finished").len(), 1, "{name}");
    }
}

/// A reader that holds the FIFO open and never reads stalls the writer once the
/// pipe's buffer is full. The run still completes — the channel is unbounded —
/// but its last step, waiting for the writer, must not make it unkillable.
#[cfg(unix)]
#[test]
fn a_second_signal_stops_a_run_whose_stream_reader_has_stalled() {
    use std::io::BufRead;

    let steps = "    Given set variable \"x\" to \"1\"\n".repeat(400);
    let cfg = write_events_project(
        "stalled",
        &[("a.feature", &format!("Feature: a\n  Scenario: s\n{steps}"))],
        None,
    );
    let fifo = cfg.with_file_name("events.fifo");
    mkfifo(&fifo);
    // `exec` keeps the pid: killing the child kills the sleep itself.
    let mut reader = Command::new("sh")
        .args(["-c", "exec sleep 120 < \"$0\""])
        .arg(&fifo)
        .spawn()
        .expect("spawn the stalled reader");
    let mut run = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(&cfg)
        .arg("--events")
        .arg(&fifo)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn bddkit");

    // The summary is printed before the stream is closed: the run is now in
    // the tail that waits for the writer.
    let stdout = run.stdout.take().expect("stdout");
    let summary = std::io::BufReader::new(stdout)
        .lines()
        .map_while(Result::ok)
        .any(|line| line.starts_with("files:"));
    assert!(summary, "the run never printed its summary");
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(
        run.try_wait().expect("poll").is_none(),
        "the stream is over 64 KiB, so the writer is stalled and the run waits for it"
    );

    // A first signal only says "still writing": the queued lines are not
    // thrown away on one Ctrl-C.
    let pid = run.id().to_string();
    let signal = |name: &str| {
        let sent = Command::new("kill")
            .args([name, &pid])
            .status()
            .expect("kill");
        assert!(sent.success());
    };
    signal("-TERM");
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(
        run.try_wait().expect("poll").is_none(),
        "one signal must not drop the stream still being written"
    );
    // A second one is "stop now", with the first one's code.
    signal("-INT");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let status = loop {
        if let Some(status) = run.try_wait().expect("poll") {
            break Some(status);
        }
        if std::time::Instant::now() > deadline {
            let _ = run.kill();
            break None;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let _ = reader.kill();
    let _ = reader.wait();
    let _ = run.wait();

    assert_eq!(
        status.and_then(|s| s.code()),
        Some(143),
        "a second signal must stop a run that is only waiting for its stream"
    );
}

/// The handlers are registered before `run <id>` is printed, so a signal sent
/// the moment that line is readable is queued even if the handler task has not
/// been polled yet — it must end the stream with the signal, never leave it
/// holding `run_started` alone. A race cannot be forced, so this repeats it:
/// before the handlers were registered up front, a SIGTERM landing in that
/// window killed the process outright.
#[cfg(unix)]
#[test]
fn a_signal_right_after_the_run_id_still_terminates_the_stream() {
    use std::io::BufRead;

    let cfg = write_events_project(
        "early-signal",
        &[(
            "a.feature",
            "Feature: a\n  Scenario: s\n    Given I sleep \"10\" seconds\n",
        )],
        None,
    );
    let events = cfg.with_file_name("events.ndjson");
    for attempt in 0..3 {
        let mut child = Command::new(env!("CARGO_BIN_EXE_bddkit"))
            .args(["run", "--config"])
            .arg(&cfg)
            .arg("--events")
            .arg(&events)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn bddkit");
        let mut first = String::new();
        std::io::BufReader::new(child.stdout.take().expect("stdout"))
            .read_line(&mut first)
            .expect("the run id line");
        assert!(first.starts_with("run "), "{first:?}");
        let sent = Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status()
            .expect("kill");
        assert!(sent.success());
        let status = child.wait().expect("wait");

        assert_eq!(status.code(), Some(143), "attempt {attempt}");
        let stream = read_stream(&events);
        let last = stream.last().expect("a stream");
        assert_eq!(
            last["type"], "run_finished",
            "attempt {attempt}: {stream:?}"
        );
        assert_eq!(last["signal"], "SIGTERM", "attempt {attempt}");
    }
}
