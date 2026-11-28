use super::*;

/// A file that lives in memory: the stream is read back from here, and no
/// test leaves anything in the temporary directory.
#[derive(Clone, Default)]
struct Sink(Arc<Mutex<Vec<u8>>>);

impl Sink {
    fn bytes(&self) -> Vec<u8> {
        self.0.lock().expect("sink").clone()
    }

    fn lines(&self) -> Vec<Value> {
        String::from_utf8(self.bytes())
            .expect("UTF-8")
            .lines()
            .map(|line| serde_json::from_str(line).expect("every line is one JSON object"))
            .collect()
    }
}

impl Write for Sink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().expect("sink").extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn the_null_sentinel_is_written_as_the_null_slot_wherever_it_nests() {
    let sink = Sink::default();
    let events = Events::start(sink.clone());
    events.emit(
        "step_finished",
        json!({"warnings": [format!("x = {NULL_SENTINEL}")], "failure": NULL_SENTINEL}),
    );
    events.finish(json!({"exit": 0})).expect("finish");

    let raw = String::from_utf8(sink.bytes()).expect("UTF-8");
    assert!(!raw.contains("\\u0000") && !raw.contains('\0'), "{raw}");
    let first = &sink.lines()[0];
    assert_eq!(first["failure"], "<<null>>");
    assert_eq!(first["warnings"][0], "x = <<null>>");
}

/// Four threads emit a fixed number of events; all of them meet at a
/// barrier, and the terminator is written while they carry on. Whatever
/// they send after it must neither panic, nor wait, nor reach the sink.
#[test]
fn nothing_follows_the_terminator_whatever_is_still_emitting() {
    let sink = Sink::default();
    let events = Events::start(sink.clone());
    let barrier = Arc::new(std::sync::Barrier::new(5));
    let spammers: Vec<_> = (0..4)
        .map(|_| {
            let (events, barrier) = (events.clone(), Arc::clone(&barrier));
            std::thread::spawn(move || {
                for _ in 0..2_000 {
                    events.emit("step_started", json!({"text": "x"}));
                }
                barrier.wait();
                for _ in 0..2_000 {
                    events.emit("step_started", json!({"text": "x"}));
                }
            })
        })
        .collect();
    barrier.wait();
    events
        .finish(json!({"exit": 130, "signal": "SIGINT"}))
        .expect("finish");
    let written = sink.bytes().len();
    for spammer in spammers {
        spammer.join().expect("an emit site never panics");
    }

    let lines = sink.lines();
    let last = lines.last().expect("lines");
    assert_eq!(last["type"], "run_finished");
    assert_eq!(last["signal"], "SIGINT");
    assert_eq!(
        lines.iter().filter(|l| l["type"] == "run_finished").count(),
        1
    );
    assert_eq!(
        sink.bytes().len(),
        written,
        "something was written after it"
    );
}

/// A producer that never lets the queue run dry must not keep the writer
/// from writing: with everything queued up front, the old loop made ONE write.
#[test]
fn a_backlog_is_written_in_bounded_batches() {
    struct Writes(Arc<Mutex<Vec<usize>>>);
    impl Write for Writes {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().expect("sizes").push(bytes.len());
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let (tx, rx) = channel();
    let line = json!({"type": "step_started", "t": 0, "text": "x".repeat(100)});
    for _ in 0..5_000 {
        tx.send(line.clone()).expect("queue");
    }
    tx.send(json!({"type": TERMINATOR, "t": 0})).expect("queue");
    let sizes = Arc::new(Mutex::new(Vec::new()));

    write_loop(Writes(Arc::clone(&sizes)), &rx).expect("write");

    let sizes = sizes.lock().expect("sizes");
    assert!(sizes.len() > 5, "{} writes for ~700 KB", sizes.len());
    assert!(
        sizes.iter().all(|size| *size < BATCH_LIMIT + 1_024),
        "{sizes:?}"
    );
}

/// One failed scenario whose step ran a statement, its evidence deferred;
/// the types of the lines written once the scenario finished.
fn deferred_scenario(interrupted: bool) -> Vec<Value> {
    let sink = Sink::default();
    let events = Events::start(sink.clone());
    let mut trace = Trace {
        events: Some(events.clone()),
        file: "a.feature".into(),
        ..Trace::off()
    };
    let sources = Sources::parse("failures").expect("parses");
    trace.collect_evidence(Artifacts::for_test(), sources, 0);
    let origin = Origin {
        line: 3,
        index: None,
        comment: None,
    };
    trace.step_started(
        "Given ",
        "s",
        origin,
        Path::new("a.feature"),
        Vec::new(),
        None,
    );
    assert!(trace.evidence_on(), "a plugin is told evidence is wanted");
    let statement = json!({"sql": "select 1", "binds": [], "duration_us": 0});
    let statement = SqlDiagnostic::from_plugin(&statement, "main").expect("its shape");
    trace.sql_in("db", &statement, None);
    trace.step_finished(Some(FailureKind::Assertion), Duration::ZERO, &[]);
    let failure = Some(("boom", FailureKind::Assertion));
    trace.scenario_finished(failure, Duration::ZERO, interrupted);
    events.finish(json!({"exit": 1})).expect("finish");
    sink.lines()
}

#[test]
fn a_scenario_that_fails_after_an_interrupt_drops_its_deferred_evidence() {
    let kinds = |lines: Vec<Value>| -> Vec<String> {
        lines
            .iter()
            .map(|line| line["type"].as_str().expect("type").to_owned())
            .collect()
    };
    assert_eq!(
        kinds(deferred_scenario(false)),
        [
            "step_started",
            "step_finished",
            "evidence",
            "scenario_finished",
            "run_finished"
        ]
    );
    // A plugin instance dropped by the interrupt's shutdown fails the step:
    // that scenario was stopped, not failed.
    let lines = deferred_scenario(true);
    assert_eq!(
        kinds(lines.clone()),
        [
            "step_started",
            "step_finished",
            "scenario_finished",
            "run_finished"
        ]
    );
    assert_eq!(lines[2]["evidence"], "none");
}

#[test]
fn sources_are_named_once_in_canonical_order_and_an_unknown_one_is_refused() {
    let sources = Sources::parse("all, failures,marked,all").expect("parses");
    assert_eq!(sources.names(), ["marked", "failures", "all"]);
    assert!(!Sources::default().any());
    let error = Sources::parse("marked,").expect_err("an empty name");
    assert!(
        error.contains("expected marked, failures or all"),
        "{error}"
    );
    let from_yaml: Sources = serde_yaml_ng::from_str("[failures]").expect("a list");
    assert_eq!(from_yaml.names(), ["failures"]);
    assert!(serde_yaml_ng::from_str::<Sources>("[nope]").is_err());
}

#[test]
fn a_failed_write_is_kept_and_the_emit_sites_never_notice() {
    struct Broken;
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("no space left"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let events = Events::start(Broken);
    for _ in 0..100 {
        events.emit("step_started", json!({}));
    }
    let error = events.finish(json!({"exit": 0})).expect_err("kept");
    assert!(error.to_string().contains("no space left"), "{error}");
}
