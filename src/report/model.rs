//! The event stream folded into the tree every report renders.
//!
//! A pure function of the stream: the same lines give the same `Run`, and
//! nothing here reads the clock, the environment or the disk. Everything that
//! is serialised is a `Vec` or a `serde_json::Value` (a sorted map), so the
//! bytes are deterministic too.

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::io::BufRead;

#[derive(Debug, Serialize)]
pub struct Run {
    pub run_id: String,
    pub bddkit: String,
    pub started_at_unix_ms: u64,
    pub config: Option<String>,
    pub selection: Value,
    pub concurrency: Option<u64>,
    /// `--evidence` was given. A stream that predates the field reads as off.
    pub evidence: bool,
    /// What it recorded: `marked`, `failures`, `all`. A stream that predates
    /// the field and has `evidence` recorded everything.
    pub evidence_sources: Vec<String>,
    /// False under `--reveal`.
    pub masking: bool,
    pub artifacts_dir: Option<String>,
    pub planned_files: u64,
    /// No `run_finished`: the process was killed, or a second signal cut it.
    pub incomplete: bool,
    pub finished: Option<Finished>,
    pub files: Vec<File>,
}

#[derive(Debug, Serialize)]
pub struct Finished {
    pub exit: i64,
    pub signal: Option<String>,
    pub files: u64,
    pub scenarios: u64,
    pub failed: u64,
    pub duration_ms: u64,
}

#[derive(Debug, Serialize)]
pub struct File {
    pub file: String,
    pub name: String,
    pub tags: Vec<String>,
    pub description: Option<String>,
    pub comment: Option<String>,
    pub background: Option<Value>,
    pub panicked: bool,
    pub scenarios: Vec<Scenario>,
}

#[derive(Debug, Serialize)]
pub struct Scenario {
    pub name: String,
    pub line: u64,
    pub example: Option<u64>,
    pub tags: Vec<String>,
    pub description: Option<String>,
    pub comment: Option<String>,
    pub rule: Option<Value>,
    pub examples: Option<Value>,
    /// `passed`, `failed`, or `open` when the stream ended inside it.
    pub status: String,
    pub duration_us: Option<u64>,
    pub failure: Option<String>,
    /// The class of `failure`; absent in a stream older than failure classes.
    pub failure_kind: Option<String>,
    /// How many of its steps have evidence: `full`, `partial` or `none`;
    /// absent in a stream older than evidence sources.
    pub evidence: Option<String>,
    pub items: Vec<Item>,
}

/// One entry of a step list: a step, a disabled step or a section heading.
#[derive(Debug, Default, Serialize)]
pub struct Item {
    /// `step`, `disabled` or `section`.
    pub kind: &'static str,
    pub line: u64,
    pub keyword: Option<String>,
    pub text: Option<String>,
    /// A section's heading.
    pub title: Option<String>,
    pub note: Option<String>,
    pub comment: Option<String>,
    /// The file a nested step is written in.
    pub source: Option<String>,
    /// `passed`, `failed`, `skipped`, or `open` when the stream ended inside it.
    pub status: Option<&'static str>,
    pub duration_us: Option<u64>,
    pub warnings: Vec<String>,
    pub vars: Option<Value>,
    pub evidence: Vec<Value>,
    pub attempts: Vec<Value>,
    pub children: Vec<Item>,
}

/// Where one scenario's items are being collected, flat, until the stream
/// ends and `parent` can be turned into nesting.
#[derive(Default)]
struct Work {
    items: Vec<Item>,
    parents: Vec<Option<usize>>,
    /// Step number to position in `items`.
    by_step: Vec<usize>,
}

struct Builder {
    run: Run,
    files: HashMap<String, usize>,
    work: HashMap<(usize, usize), Work>,
}

fn text(event: &Value, key: &str) -> Option<String> {
    event[key].as_str().map(str::to_owned)
}

fn tags(event: &Value) -> Vec<String> {
    event["tags"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|tag| tag.as_str().map(str::to_owned))
        .collect()
}

fn object(event: &Value, key: &str) -> Option<Value> {
    event.get(key).filter(|value| value.is_object()).cloned()
}

/// Folds the stream. The first line must be `run_started`; an event type the
/// model does not know is skipped, as the stream's contract says. A last line
/// that is not JSON is a stream cut mid-write and renders as incomplete; a
/// broken line anywhere else is not a stream. Errors name a line number and
/// never quote the line, which may hold a secret the filter has not seen.
pub fn build(input: impl BufRead) -> Result<Run> {
    let mut builder: Option<Builder> = None;
    let mut broken: Option<usize> = None;
    for (index, line) in input.lines().enumerate() {
        let number = index + 1;
        // Not UTF-8 is a broken line like any other; any other read error ends the report.
        let line = match line {
            Ok(line) => line,
            Err(error) if error.kind() == std::io::ErrorKind::InvalidData => String::from("\u{0}"),
            Err(error) => return Err(error).context("cannot read the stream"),
        };
        if line.trim().is_empty() && builder.is_some() {
            continue;
        }
        if let Some(first) = broken {
            bail!("line {first} is not valid JSON");
        }
        let event = match serde_json::from_str::<Value>(&line) {
            Ok(event) if event.is_object() => event,
            _ if builder.is_none() => bail!("line {number} is not a bddkit event stream line"),
            _ => {
                broken = Some(number);
                continue;
            }
        };
        match &mut builder {
            None => {
                if event["type"] != "run_started" {
                    bail!("the first line is not a run_started event: not a bddkit event stream");
                }
                builder = Some(Builder::new(&event));
            }
            Some(builder) => builder.event(&event),
        }
    }
    let Some(builder) = builder else {
        bail!("the stream is empty");
    };
    Ok(builder.finish(broken.is_some()))
}

impl Builder {
    fn new(event: &Value) -> Self {
        let run = Run {
            run_id: text(event, "run_id").unwrap_or_default(),
            bddkit: text(event, "bddkit").unwrap_or_default(),
            started_at_unix_ms: event["started_at_unix_ms"].as_u64().unwrap_or(0),
            config: text(event, "config"),
            selection: object(event, "selection").unwrap_or(Value::Null),
            concurrency: event["concurrency"].as_u64(),
            evidence: event["evidence"].as_bool().unwrap_or(false),
            evidence_sources: match event["evidence_sources"].as_array() {
                Some(names) => names
                    .iter()
                    .filter_map(|name| name.as_str().map(str::to_owned))
                    .collect(),
                None if event["evidence"] == true => vec!["all".to_owned()],
                None => Vec::new(),
            },
            masking: event["privacy"]["masking"].as_bool().unwrap_or(true),
            artifacts_dir: text(event, "artifacts_dir"),
            planned_files: event["files"].as_u64().unwrap_or(0),
            incomplete: true,
            finished: None,
            files: Vec::new(),
        };
        Self {
            run,
            files: HashMap::new(),
            work: HashMap::new(),
        }
    }

    fn event(&mut self, event: &Value) {
        if self.run.finished.is_some() {
            return; // nothing follows run_finished
        }
        match event["type"].as_str().unwrap_or("") {
            "file_started" => self.file_started(event),
            "scenario_started" => self.scenario_started(event),
            "step_started" | "step_skipped" => self.step(event),
            "step_disabled" => self.annotation(event, "disabled"),
            "section" => self.annotation(event, "section"),
            "step_finished" => self.step_finished(event),
            "evidence" => self.attach(event, |item, value| item.evidence.push(value)),
            "attempt" => self.attach(event, |item, value| item.attempts.push(value)),
            // A deferred step's resolved values, written after it finished.
            "vars" => {
                if let Some(item) = self.item_of(event) {
                    item.vars = event.get("vars").cloned();
                }
            }
            "scenario_finished" => self.scenario_finished(event),
            "file_finished" => {
                if let Some(file) = self.file_of(event) {
                    self.run.files[file].panicked = event["panicked"].as_bool().unwrap_or(false);
                }
            }
            "run_finished" => {
                self.run.incomplete = false;
                self.run.finished = Some(Finished {
                    exit: event["exit"].as_i64().unwrap_or(0),
                    signal: text(event, "signal"),
                    files: event["files"].as_u64().unwrap_or(0),
                    scenarios: event["scenarios"].as_u64().unwrap_or(0),
                    failed: event["failed"].as_u64().unwrap_or(0),
                    duration_ms: event["duration_ms"].as_u64().unwrap_or(0),
                });
            }
            _ => {}
        }
    }

    fn file_of(&self, event: &Value) -> Option<usize> {
        self.files.get(event["file"].as_str()?).copied()
    }

    /// The `(file, scenario)` an event belongs to; `None` for one that names a
    /// scenario the stream never started.
    fn scenario_of(&self, event: &Value) -> Option<(usize, usize)> {
        let file = self.file_of(event)?;
        let scenario = usize::try_from(event["scenario"].as_u64()?).ok()?;
        (scenario < self.run.files[file].scenarios.len()).then_some((file, scenario))
    }

    fn file_started(&mut self, event: &Value) {
        let Some(name) = event["file"].as_str() else {
            return;
        };
        self.files.insert(name.to_owned(), self.run.files.len());
        self.run.files.push(File {
            file: name.to_owned(),
            name: text(event, "name").unwrap_or_default(),
            tags: tags(event),
            description: text(event, "description"),
            comment: text(event, "comment"),
            background: object(event, "background"),
            panicked: false,
            scenarios: Vec::new(),
        });
    }

    fn scenario_started(&mut self, event: &Value) {
        let Some(file) = self.file_of(event) else {
            return;
        };
        self.run.files[file].scenarios.push(Scenario {
            name: text(event, "name").unwrap_or_default(),
            line: event["line"].as_u64().unwrap_or(0),
            example: event["example"].as_u64(),
            tags: tags(event),
            description: text(event, "description"),
            comment: text(event, "comment"),
            rule: object(event, "rule"),
            examples: object(event, "examples"),
            status: "open".into(),
            duration_us: None,
            failure: None,
            failure_kind: None,
            evidence: None,
            items: Vec::new(),
        });
    }

    /// Appends an item to its scenario, under the step `parent` names.
    fn push(&mut self, event: &Value, item: Item, step: Option<u64>) {
        let Some(key) = self.scenario_of(event) else {
            return;
        };
        let work = self.work.entry(key).or_default();
        let parent = event["parent"]
            .as_u64()
            .and_then(|p| work.by_step.get(usize::try_from(p).ok()?).copied());
        if step.is_some() {
            work.by_step.push(work.items.len());
        }
        work.parents.push(parent);
        work.items.push(item);
    }

    fn step(&mut self, event: &Value) {
        let skipped = event["type"] == "step_skipped";
        let item = Item {
            kind: "step",
            line: event["line"].as_u64().unwrap_or(0),
            keyword: text(event, "keyword"),
            text: text(event, "text"),
            comment: text(event, "comment"),
            source: text(event, "source"),
            status: Some(if skipped { "skipped" } else { "open" }),
            vars: event.get("vars").cloned(),
            ..Item::default()
        };
        self.push(event, item, Some(0));
    }

    fn annotation(&mut self, event: &Value, kind: &'static str) {
        let item = Item {
            kind,
            line: event["line"].as_u64().unwrap_or(0),
            keyword: text(event, "keyword"),
            text: text(event, "text"),
            title: text(event, "title"),
            note: text(event, "note"),
            comment: text(event, "comment"),
            source: text(event, "source"),
            ..Item::default()
        };
        self.push(event, item, None);
    }

    /// The step an event names, by its number within the scenario.
    fn item_of(&mut self, event: &Value) -> Option<&mut Item> {
        let key = self.scenario_of(event)?;
        let work = self.work.get_mut(&key)?;
        let at = *work
            .by_step
            .get(usize::try_from(event["step"].as_u64()?).ok()?)?;
        work.items.get_mut(at)
    }

    fn step_finished(&mut self, event: &Value) {
        if let Some(item) = self.item_of(event) {
            item.status = Some(if event["status"] == "failed" {
                "failed"
            } else {
                "passed"
            });
            item.duration_us = event["duration_us"].as_u64();
            item.warnings = event["warnings"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|w| w.as_str().map(str::to_owned))
                .collect();
        }
    }

    /// Evidence and attempts keep the event's own fields, less the ones that
    /// only say where it belongs.
    fn attach(&mut self, event: &Value, add: fn(&mut Item, Value)) {
        let mut value = event.clone();
        if let Some(fields) = value.as_object_mut() {
            for key in ["type", "seq", "file", "scenario", "step"] {
                fields.remove(key);
            }
        }
        if let Some(item) = self.item_of(event) {
            add(item, value);
        }
    }

    fn scenario_finished(&mut self, event: &Value) {
        let Some((file, scenario)) = self.scenario_of(event) else {
            return;
        };
        let scenario = &mut self.run.files[file].scenarios[scenario];
        scenario.status = if event["status"] == "failed" {
            "failed"
        } else {
            "passed"
        }
        .into();
        scenario.duration_us = event["duration_us"].as_u64();
        scenario.failure = text(event, "failure");
        scenario.failure_kind = text(event, "failure_kind");
        scenario.evidence = text(event, "evidence");
    }

    fn finish(mut self, cut: bool) -> Run {
        for ((file, scenario), work) in self.work {
            self.run.files[file].scenarios[scenario].items = nest(work);
        }
        self.run.incomplete |= cut;
        self.run
    }
}

/// Turns `parents` into children lists. A parent always starts before its
/// child, so one forward pass sees every parent first.
fn nest(work: Work) -> Vec<Item> {
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); work.items.len()];
    let mut roots = Vec::new();
    for (at, parent) in work.parents.iter().enumerate() {
        match parent {
            Some(parent) => children[*parent].push(at),
            None => roots.push(at),
        }
    }
    let mut slots: Vec<Option<Item>> = work.items.into_iter().map(Some).collect();
    fn take(at: usize, slots: &mut [Option<Item>], children: &[Vec<usize>]) -> Item {
        let mut item = slots[at].take().expect("each item has one parent");
        item.children = children[at]
            .iter()
            .map(|child| take(*child, slots, children))
            .collect();
        item
    }
    roots
        .into_iter()
        .map(|at| take(at, &mut slots, &children))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn stream(events: &[Value]) -> String {
        events
            .iter()
            .map(|event| format!("{event}\n"))
            .collect::<String>()
    }

    fn started() -> Value {
        json!({"type": "run_started", "run_id": "r1", "bddkit": "0.3.1", "evidence": true,
               "artifacts_dir": "/out", "privacy": {"masking": false}, "files": 1})
    }

    fn base() -> Vec<Value> {
        vec![
            started(),
            json!({"type": "file_started", "file": "a.feature", "name": "A", "tags": ["@x"]}),
            json!({"type": "scenario_started", "file": "a.feature", "scenario": 0,
                   "name": "s", "line": 3, "tags": []}),
        ]
    }

    fn model(events: &[Value]) -> Run {
        build(stream(events).as_bytes()).expect("a stream")
    }

    #[test]
    fn steps_nest_by_parent_and_annotations_keep_their_place() {
        let mut events = base();
        events.extend([
            json!({"type": "section", "file": "a.feature", "scenario": 0, "title": "Setup", "line": 4}),
            json!({"type": "step_started", "file": "a.feature", "scenario": 0, "step": 0, "keyword": "Given ", "text": "macro", "line": 5}),
            json!({"type": "step_started", "file": "a.feature", "scenario": 0, "step": 1, "parent": 0, "source": "m.yaml", "keyword": "", "text": "inner", "line": 1}),
            json!({"type": "evidence", "seq": 9, "file": "a.feature", "scenario": 0, "step": 1, "kind": "sql", "content": {"sql": "select 1"}}),
            json!({"type": "step_finished", "file": "a.feature", "scenario": 0, "step": 1, "status": "passed", "duration_us": 5, "warnings": []}),
            json!({"type": "step_finished", "file": "a.feature", "scenario": 0, "step": 0, "status": "failed", "duration_us": 9, "warnings": ["w"]}),
            json!({"type": "step_disabled", "file": "a.feature", "scenario": 0, "keyword": "When ", "text": "off", "line": 6}),
            json!({"type": "step_skipped", "file": "a.feature", "scenario": 0, "step": 2, "keyword": "Then ", "text": "later", "line": 7}),
            json!({"type": "scenario_finished", "file": "a.feature", "scenario": 0, "status": "failed", "duration_us": 20, "failure": "boom", "failure_kind": "transport"}),
            json!({"type": "run_finished", "exit": 1, "files": 1, "scenarios": 1, "failed": 1, "duration_ms": 3}),
        ]);
        let run = model(&events);
        let scenario = &run.files[0].scenarios[0];
        let kinds: Vec<_> = scenario.items.iter().map(|i| i.kind).collect();
        assert_eq!(kinds, ["section", "step", "disabled", "step"]);
        let outer = &scenario.items[1];
        assert_eq!(outer.status, Some("failed"));
        assert_eq!(outer.children[0].text.as_deref(), Some("inner"));
        assert_eq!(outer.children[0].evidence[0]["kind"], "sql");
        assert!(outer.children[0].evidence[0].get("seq").is_none());
        assert_eq!(scenario.items[3].status, Some("skipped"));
        assert_eq!(scenario.failure.as_deref(), Some("boom"));
        assert_eq!(scenario.failure_kind.as_deref(), Some("transport"));
        assert!(!run.incomplete && !run.masking && run.evidence);
    }

    #[test]
    fn deferred_evidence_and_vars_attach_to_their_step_after_it_finished() {
        let mut events = base();
        events[0]["evidence_sources"] = json!(["failures"]);
        events.extend([
            json!({"type": "step_started", "file": "a.feature", "scenario": 0, "step": 0, "keyword": "Given ", "text": "x <<a>>", "line": 4}),
            json!({"type": "step_finished", "file": "a.feature", "scenario": 0, "step": 0, "status": "failed", "duration_us": 1, "warnings": []}),
            json!({"type": "vars", "file": "a.feature", "scenario": 0, "step": 0, "vars": [["a", "1"]], "deferred": true}),
            json!({"type": "evidence", "file": "a.feature", "scenario": 0, "step": 0, "kind": "sql", "deferred": true, "content": {"sql": "select 1"}}),
            json!({"type": "scenario_finished", "file": "a.feature", "scenario": 0, "status": "failed", "evidence": "full"}),
        ]);
        let run = model(&events);
        assert_eq!(run.evidence_sources, ["failures"]);
        let scenario = &run.files[0].scenarios[0];
        assert_eq!(scenario.evidence.as_deref(), Some("full"));
        assert_eq!(scenario.items[0].vars, Some(json!([["a", "1"]])));
        assert_eq!(scenario.items[0].evidence[0]["deferred"], true);

        // A stream older than the sources recorded everything it recorded.
        assert_eq!(model(&base()).evidence_sources, ["all"]);
    }

    #[test]
    fn a_stream_without_run_finished_is_incomplete_and_leaves_open_work_open() {
        let mut events = base();
        events.push(json!({"type": "step_started", "file": "a.feature", "scenario": 0, "step": 0, "keyword": "Given ", "text": "x", "line": 4}));
        let run = model(&events);
        assert!(run.incomplete);
        assert_eq!(run.files[0].scenarios[0].status, "open");
        assert_eq!(run.files[0].scenarios[0].items[0].status, Some("open"));
    }

    #[test]
    fn a_truncated_last_line_is_incomplete_not_an_error() {
        let text = format!("{}{{\"type\":\"step_sta", stream(&base()));
        assert!(build(text.as_bytes()).expect("renders").incomplete);
    }

    #[test]
    fn a_broken_line_in_the_middle_is_refused_by_number_without_quoting_it() {
        let text = format!(
            "{}hunter2 not json\n{}",
            stream(&base()),
            stream(&base()[..1])
        );
        let error = build(text.as_bytes()).expect_err("refused").to_string();
        assert!(error.contains("line 4"), "{error}");
        assert!(!error.contains("hunter2"), "{error}");
    }

    #[test]
    fn a_file_whose_first_line_is_not_run_started_is_refused() {
        let text = "{\"type\":\"file_started\"}\n";
        assert!(build(text.as_bytes()).is_err());
        assert!(build("[1,2]\n".as_bytes()).is_err());
        assert!(build("".as_bytes()).is_err());
    }

    #[test]
    fn an_unknown_event_type_is_skipped() {
        let mut events = base();
        events.insert(1, json!({"type": "from_the_future", "x": 1}));
        assert_eq!(model(&events).files.len(), 1);
    }

    #[test]
    fn a_panicked_file_is_marked_and_its_open_scenario_stays_open() {
        let mut events = base();
        events.push(json!({"type": "file_finished", "file": "a.feature", "scenarios": 1, "failed": 1, "panicked": true}));
        let run = model(&events);
        assert!(run.files[0].panicked);
        assert_eq!(run.files[0].scenarios[0].status, "open");
    }

    #[test]
    fn an_outline_row_keeps_its_example_ordinal_and_examples_block() {
        let mut events = base();
        events[2] = json!({"type": "scenario_started", "file": "a.feature", "scenario": 0, "name": "s [1]",
            "line": 3, "example": 1, "tags": [], "examples": {"line": 9, "name": "E"}});
        let scenario = &model(&events).files[0].scenarios[0];
        assert_eq!(scenario.example, Some(1));
        assert_eq!(scenario.examples.as_ref().expect("block")["name"], "E");
    }

    #[test]
    fn a_line_that_is_not_utf8_is_a_broken_line_not_a_lost_one() {
        let mut bytes = stream(&base()).into_bytes();
        bytes.extend_from_slice(b"\xff\xfe\n");
        bytes.extend_from_slice(stream(&base()[..1]).as_bytes());
        assert!(build(&bytes[..]).is_err());
    }

    #[test]
    fn the_same_stream_gives_the_same_bytes() {
        let events = base();
        let a = serde_json::to_string(&model(&events)).expect("json");
        let b = serde_json::to_string(&model(&events)).expect("json");
        assert_eq!(a, b);
    }
}
