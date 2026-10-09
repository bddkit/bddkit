//! `bddkit run --events <path>`: one JSON object per line while the run happens.
//!
//! Emit sites build a `serde_json::Map` and send it into an unbounded channel;
//! ONE thread owns the file, assigns `seq`, tallies the totals, serialises and
//! writes. A send never waits and never fails loudly (the receiver is gone
//! after the terminator, and a file still in flight must not panic over it),
//! so a slow reader of a FIFO cannot change a test's timing.

use crate::feature::display_path;
use crate::vars::NULL_SENTINEL;
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Opened exactly once, for the whole run, and never through `report::prepare`:
/// that one closes the file again, and a FIFO's reader would see end-of-file
/// before the first event. On a regular file the open truncates, so a run that
/// is refused leaves an empty stream.
pub fn open(path: &Path) -> Result<File> {
    path.parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| {
            OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(path)
        })
        .with_context(|| format!("cannot open the events file {}", path.display()))
}

type Writer = Arc<Mutex<Option<JoinHandle<io::Result<()>>>>>;

#[derive(Clone)]
pub struct Events {
    tx: Sender<Value>,
    start: Instant,
    writer: Writer,
}

impl Events {
    pub fn start(out: impl Write + Send + 'static) -> Self {
        let (tx, rx) = channel();
        let writer = std::thread::Builder::new()
            .name("bddkit-events".into())
            .spawn(move || write_loop(out, &rx))
            .expect("spawn the events writer");
        Self {
            tx,
            start: Instant::now(),
            writer: Arc::new(Mutex::new(Some(writer))),
        }
    }

    fn millis(&self) -> u64 {
        u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// `fields` is a JSON object, as every caller's `json!({..})` is; `type`
    /// and `t` are added here, at the emit site, and `seq` by the writer.
    fn envelope(&self, kind: &str, mut fields: Value) -> Value {
        fields["type"] = kind.into();
        fields["t"] = self.millis().into();
        fields
    }

    pub fn emit(&self, kind: &str, fields: Value) {
        let _ = self.tx.send(self.envelope(kind, fields));
    }

    /// Writes `run_finished` and waits until it is on disk. The first caller
    /// wins: the writer closes after the first terminator, so a second call
    /// (the interrupt handler and `run` can both get here) neither adds a line
    /// nor returns before the first has been flushed. Blocking — call it from
    /// `spawn_blocking`. The error is the first write that failed.
    pub fn finish(&self, fields: Value) -> io::Result<()> {
        let _ = self.tx.send(self.envelope(TERMINATOR, fields));
        let mut writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        match writer.take() {
            Some(handle) => handle
                .join()
                .unwrap_or_else(|_| Err(io::Error::other("the events writer panicked"))),
            None => Ok(()),
        }
    }
}

/// Bytes encoded before the writer stops draining to write and flush.
const BATCH_LIMIT: usize = 64 * 1024;

/// The last line. Whatever arrives after it is dropped.
const TERMINATOR: &str = "run_finished";

fn write_loop(mut out: impl Write, rx: &Receiver<Value>) -> io::Result<()> {
    let mut tally = Tally::new();
    let mut failed = None;
    let mut batch = Vec::new();
    while let Ok(first) = rx.recv() {
        let mut next = Some(first);
        let mut closing = false;
        while let Some(event) = next {
            closing = tally.encode(event, &mut batch);
            // Capped: under load the queue never runs dry, and a batch that
            // waited for that would put nothing on disk until the run is over.
            if closing || batch.len() >= BATCH_LIMIT {
                break;
            }
            next = rx.try_recv().ok();
        }
        // Once a write has failed the writer keeps draining and writes nothing:
        // the run is not aborted, `run` reports the error after it.
        if failed.is_none()
            && let Err(error) = out.write_all(&batch).and_then(|()| out.flush())
        {
            failed = Some(error);
        }
        batch.clear();
        if closing {
            break;
        }
    }
    failed.map_or(Ok(()), Err)
}

struct Tally {
    seq: u64,
    t: u64,
    files: u64,
    scenarios: u64,
    failed: u64,
    /// `NULL_SENTINEL` as `serde_json` writes it: a NUL is always `\u0000`.
    null_escaped: String,
}

impl Tally {
    fn new() -> Self {
        Self {
            seq: 0,
            t: 0,
            files: 0,
            scenarios: 0,
            failed: 0,
            null_escaped: NULL_SENTINEL.replace('\0', "\\u0000"),
        }
    }

    /// Appends one line; true when it is the terminator.
    fn encode(&mut self, mut event: Value, batch: &mut Vec<u8>) -> bool {
        // Taken at the emit site on whichever thread; clamped here so `t`
        // never goes backwards in `seq` order.
        self.t = event["t"].as_u64().unwrap_or(0).max(self.t);
        event["t"] = self.t.into();
        event["seq"] = self.seq.into();
        self.seq += 1;
        let last = event["type"] == TERMINATOR;
        if event["type"] == "file_finished" {
            self.files += 1;
            self.scenarios += event["scenarios"].as_u64().unwrap_or(0);
            self.failed += event["failed"].as_u64().unwrap_or(0);
        }
        // Counted from the lines written, on both ways out, so the totals of
        // an interrupted run are those of the files it had finished.
        if last {
            event["files"] = self.files.into();
            event["scenarios"] = self.scenarios.into();
            event["failed"] = self.failed.into();
            event["duration_ms"] = self.t.into();
        }
        // The SQL NULL sentinel is `<<null>>` in the stream, as
        // `runner::debug_display` already prints it. A literal backslash-u0000
        // in user text serialises as `\\u0000` and cannot match.
        let line = event.to_string();
        let line = if line.contains(&self.null_escaped) {
            line.replace(&self.null_escaped, "<<null>>")
        } else {
            line
        };
        batch.extend_from_slice(line.as_bytes());
        batch.push(b'\n');
        last
    }
}

fn micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

/// Where a step's text is written, when it is not where its caller is.
#[derive(Clone, Copy)]
pub struct Origin {
    pub line: usize,
    /// Position in a macro body, which carries no line of its own.
    pub index: Option<usize>,
}

/// The per-file half of the emitter: the file's name, the scenario ordinal, the
/// step counter and the stack of steps in flight. `World` owns one, so with no
/// `--events` every emit site is one branch on `None`.
pub struct Trace {
    events: Option<Events>,
    file: String,
    scenarios: usize,
    scenario: usize,
    steps: usize,
    open: Vec<usize>,
}

impl Trace {
    pub fn off() -> Self {
        Self {
            events: None,
            file: String::new(),
            scenarios: 0,
            scenario: 0,
            steps: 0,
            open: Vec::new(),
        }
    }

    /// Emits `file_started`.
    pub fn new(events: Option<Events>, path: &Path, name: &str) -> Self {
        let Some(events) = events else {
            return Self::off();
        };
        let file = display_path(path);
        events.emit("file_started", json!({"file": file, "name": name}));
        Self {
            events: Some(events),
            file,
            ..Self::off()
        }
    }

    pub fn scenario_started(&mut self, name: &str, line: usize, example: Option<usize>) {
        let Some(events) = &self.events else { return };
        self.scenario = self.scenarios;
        self.scenarios += 1;
        self.steps = 0;
        self.open.clear();
        let mut fields = json!({
            "file": self.file, "scenario": self.scenario, "name": name, "line": line,
        });
        if let Some(example) = example {
            fields["example"] = example.into();
        }
        events.emit("scenario_started", fields);
    }

    pub fn scenario_finished(&self, failure: Option<&str>, duration: Duration) {
        let Some(events) = &self.events else { return };
        events.emit(
            "scenario_finished",
            json!({
                "file": self.file, "scenario": self.scenario,
                "status": if failure.is_some() { "failed" } else { "passed" },
                "duration_us": micros(duration), "failure": failure,
            }),
        );
    }

    /// Numbers the step, whatever its depth, and makes it the caller of every
    /// step started before it finishes.
    pub fn step_started(&mut self, keyword: &str, text: &str, origin: Origin, source: &Path) {
        let Some(events) = &self.events else { return };
        let step = self.steps;
        self.steps += 1;
        let mut fields = json!({
            "file": self.file, "scenario": self.scenario, "step": step,
            "keyword": keyword, "text": text, "line": origin.line,
        });
        if let Some(parent) = self.open.last() {
            fields["parent"] = (*parent).into();
            fields["source"] = display_path(source).into();
        }
        if let Some(index) = origin.index {
            fields["index"] = index.into();
        }
        self.open.push(step);
        events.emit("step_started", fields);
    }

    /// Closes the innermost step in flight. `warnings` are those raised while
    /// it ran, nested steps' included.
    pub fn step_finished(&mut self, passed: bool, duration: Duration, warnings: &[String]) {
        let Some(events) = &self.events else { return };
        let Some(step) = self.open.pop() else { return };
        events.emit(
            "step_finished",
            json!({
                "file": self.file, "scenario": self.scenario, "step": step,
                "status": if passed { "passed" } else { "failed" },
                "duration_us": micros(duration), "warnings": warnings,
            }),
        );
    }

    pub fn step_skipped(&mut self, keyword: &str, text: &str, line: usize) {
        let Some(events) = &self.events else { return };
        let step = self.steps;
        self.steps += 1;
        events.emit(
            "step_skipped",
            json!({
                "file": self.file, "scenario": self.scenario, "step": step,
                "keyword": keyword, "text": text, "line": line,
            }),
        );
    }
}

#[cfg(test)]
mod tests {
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
                let (events, barrier) = (events.clone(), barrier.clone());
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

        write_loop(Writes(sizes.clone()), &rx).expect("write");

        let sizes = sizes.lock().expect("sizes");
        assert!(sizes.len() > 5, "{} writes for ~700 KB", sizes.len());
        assert!(
            sizes.iter().all(|size| *size < BATCH_LIMIT + 1_024),
            "{sizes:?}"
        );
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
}
