//! The model written as Markdown that `grep`, `sed` and `awk` can cut apart.
//!
//! Every part sits between two marker lines (`docs/report-markdown.md`), and
//! the index points at each scenario by its line range in this very file. The
//! index precedes what it points at, so the body is rendered first and the
//! header twice: once to learn how many lines it takes, once with the ranges
//! shifted by that count. Nothing a number says changes a line count, so the
//! second pass cannot move what the first measured.

use super::html;
use super::model::{File, Item, Run, Scenario};
use clap::ValueEnum;
use serde_json::Value;
use std::fmt::Write as _;

/// Bumped when a tool reading the file could be broken by a change.
const TEMPLATE_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Detail {
    /// failed scenarios in full, a count of the rest
    Failed,
    /// every scenario in full
    All,
}

impl Detail {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Failed => "failed",
            Self::All => "all",
        }
    }

    /// A scenario that did not pass is shown either way, `open` included: a
    /// run cut short must not count what never finished as a pass.
    fn shows(self, scenario: &Scenario) -> bool {
        self == Self::All || scenario.status != "passed"
    }
}

// ---- small pieces, each with its own test ---------------------------------

fn longest_run(text: &str, of: char) -> usize {
    let mut longest = 0;
    let mut run = 0;
    for c in text.chars() {
        run = if c == of { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    longest
}

/// A fence one backtick longer than any run in `content`, three at least.
fn fence(content: &str) -> String {
    "`".repeat((longest_run(content, '`') + 1).max(3))
}

/// A newline would end the line a `grep` hit is meant to hold whole.
fn one_line(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\n', "\\n")
}

/// An inline code span that survives any backticks in its text.
fn code(text: &str) -> String {
    let text = one_line(text);
    let ticks = "`".repeat(longest_run(&text, '`') + 1);
    if text.is_empty() {
        "` `".into()
    } else if text.starts_with('`') || text.ends_with('`') {
        format!("{ticks} {text} {ticks}")
    } else {
        format!("{ticks}{text}{ticks}")
    }
}

/// A marker attribute value: JSON string contents, and no two hyphens in a
/// row, which an HTML comment cannot hold.
fn attr(value: &str) -> String {
    let mut out = String::new();
    let mut hyphen = false;
    for c in value.chars() {
        match c {
            '-' if hyphen => {
                out.push_str("\\u002d");
                hyphen = false;
                continue;
            }
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => out.push(c),
        }
        hyphen = c == '-';
    }
    out
}

fn marker(edge: &str, kind: &str, attrs: &[(&str, &str)]) -> String {
    let attrs = attrs.iter().fold(String::new(), |mut out, (key, value)| {
        let _ = write!(out, " {key}=\"{}\"", attr(value));
        out
    });
    format!("<!-- bddkit:{edge} {kind}{attrs} -->")
}

/// The one change bddkit makes to content: a line that would read as a marker
/// gets a leading space.
fn guard(line: &str) -> String {
    if line.starts_with("<!-- bddkit:") {
        format!(" {line}")
    } else {
        line.to_owned()
    }
}

fn cell(text: &str) -> String {
    one_line(text).replace('|', "\\|")
}

#[expect(
    clippy::cast_precision_loss,
    reason = "a duration shown with two decimals"
)]
fn duration(us: u64) -> String {
    match us {
        0..1_000 => format!("{us} µs"),
        1_000..1_000_000 => format!("{} ms", us / 1_000),
        _ => format!("{:.2} s", us as f64 / 1e6),
    }
}

/// RFC 3339, UTC, from the stream's milliseconds (days-to-civil, H. Hinnant).
fn rfc3339(ms: u64) -> String {
    let days = i64::try_from(ms / 86_400_000).unwrap_or(0);
    let rest = ms % 86_400_000;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rest / 3_600_000,
        rest / 60_000 % 60,
        rest / 1_000 % 60,
        rest % 1_000
    )
}

/// Single-quoted for `sh`, unless the text is plainly safe.
fn shell(text: &str) -> String {
    let plain = !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-/".contains(c));
    if plain {
        text.to_owned()
    } else {
        format!("'{}'", text.replace('\'', "'\\''"))
    }
}

fn id_of(file: &File, scenario: &Scenario) -> String {
    match scenario.example {
        Some(row) => format!("{}:{}#{row}", file.file, scenario.line),
        None => format!("{}:{}", file.file, scenario.line),
    }
}

// ---- the document -----------------------------------------------------------

/// Lines, none holding a newline: a part's range is a count of them.
#[derive(Default)]
struct Doc {
    lines: Vec<String>,
}

impl Doc {
    /// A line that is already safe: a marker, or text with its own prefix.
    fn raw(&mut self, line: impl Into<String>) {
        let line = line.into();
        debug_assert!(!line.contains('\n'), "a line holds no newline");
        self.lines.push(line);
    }

    fn blank(&mut self) {
        self.lines.push(String::new());
    }

    /// Content: every line of it passes through `guard`.
    fn text(&mut self, text: &str) {
        for line in text.lines() {
            self.lines.push(guard(line));
        }
    }

    fn quote(&mut self, text: &str) {
        for line in text.lines() {
            self.raw(if line.is_empty() {
                ">".to_owned()
            } else {
                format!("> {line}")
            });
        }
    }

    fn fenced(&mut self, info: &str, content: &str) {
        let fence = fence(content);
        self.raw(format!("{fence}{info}"));
        self.text(content);
        self.raw(fence);
    }

    fn append(&mut self, other: Self) {
        self.lines.extend(other.lines);
    }
}

/// A listed scenario and where it sits in the body, 0-based and inclusive.
struct Entry {
    id: String,
    status: String,
    name: String,
    start: usize,
    end: usize,
}

struct Ctx<'a> {
    run: &'a Run,
    /// Output file name, as the howto's commands spell it.
    name: &'a str,
    /// Prefix of a link from this file to the artifacts root.
    base: &'a str,
    temporary: bool,
    detail: Detail,
}

pub fn render(
    run: &Run,
    name: &str,
    artifacts_base: &str,
    artifacts_temporary: bool,
    detail: Detail,
) -> String {
    let ctx = Ctx {
        run,
        name,
        base: artifacts_base,
        temporary: artifacts_temporary,
        detail,
    };
    let (body, entries) = body(&ctx);
    let height = header(&ctx, &entries, 0, 0).lines.len();
    let mut doc = header(&ctx, &entries, height, height + body.lines.len());
    doc.append(body);
    let mut out = doc.lines.join("\n");
    out.push('\n');
    out
}

// ---- header: front matter, howto, run, index -------------------------------

fn header(ctx: &Ctx<'_>, entries: &[Entry], offset: usize, total: usize) -> Doc {
    let run = ctx.run;
    let mut doc = Doc::default();
    front_matter(&mut doc, ctx, total);
    doc.blank();
    howto(&mut doc, ctx, entries, offset);
    doc.blank();
    run_part(&mut doc, ctx);
    doc.blank();
    index(&mut doc, ctx, entries, offset);
    doc.blank();
    debug_assert!(!run.files.is_empty() || entries.is_empty());
    doc
}

fn totals(run: &Run) -> (usize, usize, usize) {
    let scenarios = run.files.iter().map(|f| f.scenarios.len()).sum();
    let failed = run
        .files
        .iter()
        .flat_map(|f| &f.scenarios)
        .filter(|s| s.status == "failed")
        .count();
    // A panicked file is a failure even when no scenario of it says so.
    let panicked = run
        .files
        .iter()
        .filter(|f| f.panicked && !f.scenarios.iter().any(|s| s.status == "failed"))
        .count();
    (run.files.len(), scenarios, failed + panicked)
}

#[expect(clippy::expect_used, reason = "a string always serializes")]
fn front_matter(doc: &mut Doc, ctx: &Ctx<'_>, total: usize) {
    let run = ctx.run;
    let quoted = |text: &str| serde_json::to_string(text).expect("a string is JSON");
    let (files, scenarios, failed) = totals(run);
    doc.raw("---");
    doc.raw(format!("bddkit_report: {TEMPLATE_VERSION}"));
    doc.raw(format!("run_id: {}", quoted(&run.run_id)));
    doc.raw(format!("bddkit: {}", quoted(&run.bddkit)));
    if run.started_at_unix_ms > 0 {
        doc.raw(format!("started_at: {}", rfc3339(run.started_at_unix_ms)));
    }
    if let Some(done) = &run.finished {
        doc.raw(format!("duration_ms: {}", done.duration_ms));
        doc.raw(format!("exit: {}", done.exit));
        if let Some(signal) = &done.signal {
            doc.raw(format!("signal: {}", quoted(signal)));
        }
    }
    doc.raw(format!("files: {files}"));
    doc.raw(format!("scenarios: {scenarios}"));
    doc.raw(format!("failed: {failed}"));
    doc.raw(format!("evidence: {}", run.evidence));
    if !run.evidence_sources.is_empty() {
        let names: Vec<String> = run.evidence_sources.iter().map(|n| quoted(n)).collect();
        doc.raw(format!("evidence_sources: [{}]", names.join(", ")));
    }
    doc.raw(format!("masking: {}", run.masking));
    doc.raw(format!("complete: {}", !run.incomplete));
    doc.raw(format!("detail: {}", ctx.detail.name()));
    doc.raw(format!("lines: {total}"));
    doc.raw("---");
}

fn howto(doc: &mut Doc, ctx: &Ctx<'_>, entries: &[Entry], offset: usize) {
    let file = shell(ctx.name);
    // The examples name the first listed scenario, which is a failed one when
    // anything failed.
    let first = entries
        .iter()
        .find(|e| e.status != "passed")
        .or(entries.first());
    doc.raw(marker("begin", "howto", &[]));
    doc.raw("# How to read this report");
    doc.raw("Parts are wrapped in marker lines, each alone on its line at column 0:");
    doc.raw(
        "`<!-- bddkit:begin <kind> id=\"<id>\" … -->` … `<!-- bddkit:end <kind> id=\"<id>\" -->`.",
    );
    doc.raw(
        "Kinds: run, index, file, scenario, failure, evidence. A scenario id is `<file>:<line>`, plus `#<row>` for an outline row.",
    );
    doc.blank();
    let item = |doc: &mut Doc, what: &str, command: String| {
        doc.raw(format!("- {what}: {}", code(&command)));
    };
    item(doc, "Totals", format!("head -25 {file}"));
    item(
        doc,
        "The index, with exact line ranges",
        format!("sed -n '/^<!-- bddkit:begin index/,/^<!-- bddkit:end index/p' {file}"),
    );
    if let Some(first) = first {
        item(
            doc,
            "One scenario by its range",
            format!(
                "sed -n '{},{}p' {file}",
                offset + first.start + 1,
                offset + first.end + 1
            ),
        );
    }
    item(
        doc,
        "Failed scenarios",
        format!("grep -n '^<!-- bddkit:begin scenario' {file} | grep 'status=\"failed\"'"),
    );
    if let Some(first) = first {
        let begin = marker("begin", "scenario", &[("id", &first.id)]).replace(" -->", "");
        let end = marker("end", "scenario", &[("id", &first.id)]).replace(" -->", "");
        item(
            doc,
            "One scenario by id",
            format!(
                "b={} e={} awk 'index($0,ENVIRON[\"b\"])==1{{p=1}} p; index($0,ENVIRON[\"e\"])==1{{p=0}}' {file}",
                shell(&begin),
                shell(&end)
            ),
        );
    }
    item(
        doc,
        "All SQL",
        format!("grep -n '^<!-- bddkit:begin evidence' {file} | grep 'kind=\"sql\"'"),
    );
    item(
        doc,
        "Every failure text",
        format!("sed -n '/^<!-- bddkit:begin failure/,/^<!-- bddkit:end failure/p' {file}"),
    );
    if ctx.detail == Detail::Failed {
        doc.blank();
        doc.raw(format!(
            "Passed scenarios are counted, not shown: {} shows them.",
            code(&format!("bddkit report <stream> -o {file} --detail all"))
        ));
    }
    doc.raw(marker("end", "howto", &[]));
}

fn run_part(doc: &mut Doc, ctx: &Ctx<'_>) {
    let run = ctx.run;
    doc.raw(marker("begin", "run", &[]));
    doc.raw("## Run");
    if !run.masking {
        doc.raw("> **Warning:** masking was off for this run (`--reveal`): this file may contain secrets.");
    }
    if let Some(config) = &run.config {
        doc.raw(format!("- config: {}", code(config)));
    }
    if let Some(app_env) = &run.app_env {
        doc.raw(format!("- APP_ENV: {}", code(app_env)));
    }
    for (key, label) in [("tags", "tags"), ("paths", "paths")] {
        let list: Vec<&str> = run.selection[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        if !list.is_empty() {
            doc.raw(format!("- {label}: {}", code(&list.join(", "))));
        }
    }
    if let Some(concurrency) = run.concurrency {
        doc.raw(format!("- concurrency: {concurrency}"));
    }
    let state = |on: bool| if on { "on" } else { "off" };
    let sources = if run.evidence_sources.is_empty() {
        "off".to_owned()
    } else {
        one_line(&run.evidence_sources.join(", "))
    };
    doc.raw(format!(
        "- masking: {} · evidence: {sources}",
        state(run.masking)
    ));
    if !run.evidence {
        doc.raw("> **Warning:** no evidence was collected. Run with `--evidence` (and `--events`) to see requests, responses and SQL.");
    } else if !run.evidence_sources.iter().any(|source| source == "all") {
        doc.raw(format!("> **Note:** evidence was recorded only for {sources}, so a step outside them shows no exchange. Run with `--evidence` (`all`) to record every step."));
    }
    if run.incomplete {
        doc.raw("> **Warning:** this stream is incomplete: the run was killed or cut short, so the report ends where the stream does.");
    }
    if ctx.temporary {
        doc.raw("> **Warning:** artifacts are under the temporary directory and links will break when it is cleaned. Use `--artifacts-dir` next to where this report is kept.");
    }
    doc.raw(marker("end", "run", &[]));
}

fn index(doc: &mut Doc, ctx: &Ctx<'_>, entries: &[Entry], offset: usize) {
    let mut rows: Vec<&Entry> = entries.iter().collect();
    // Stable: failed first, the rest keeps file order.
    rows.sort_by_key(|entry| entry.status == "passed");
    doc.raw(marker("begin", "index", &[]));
    doc.raw("## Index");
    doc.raw("| id | status | lines | name |");
    doc.raw("|---|---|---|---|");
    for entry in rows {
        doc.raw(format!(
            "| {} | {} | {}-{} | {} |",
            cell(&code(&entry.id)),
            entry.status,
            offset + entry.start + 1,
            offset + entry.end + 1,
            cell(&entry.name)
        ));
    }
    if ctx.detail == Detail::Failed {
        doc.blank();
        doc.raw("| file | scenarios | passed |");
        doc.raw("|---|---|---|");
        for file in &ctx.run.files {
            let passed = file
                .scenarios
                .iter()
                .filter(|s| s.status == "passed")
                .count();
            doc.raw(format!(
                "| {} | {} | {passed} |",
                cell(&code(&file.file)),
                file.scenarios.len()
            ));
        }
    }
    doc.raw(marker("end", "index", &[]));
}

// ---- body: files, scenarios, steps, evidence --------------------------------

fn body(ctx: &Ctx<'_>) -> (Doc, Vec<Entry>) {
    let mut doc = Doc::default();
    let mut entries = Vec::new();
    for file in &ctx.run.files {
        let shown: Vec<&Scenario> = file
            .scenarios
            .iter()
            .filter(|s| ctx.detail.shows(s))
            .collect();
        // A panicked file may have no scenario at all, and is the only trace of the failure.
        if shown.is_empty() && !file.panicked {
            continue;
        }
        let failed = file
            .scenarios
            .iter()
            .filter(|s| s.status == "failed")
            .count();
        doc.raw(marker(
            "begin",
            "file",
            &[
                ("id", &file.file),
                ("scenarios", &file.scenarios.len().to_string()),
                ("failed", &failed.to_string()),
            ],
        ));
        doc.raw(format!("## {}", one_line(&file.file)));
        if !file.name.is_empty() {
            doc.raw(format!("**Feature: {}**", one_line(&file.name)));
        }
        if file.panicked {
            doc.raw("> **The worker running this file panicked.**");
        }
        doc.blank();
        for scenario in shown {
            let start = doc.lines.len();
            let id = id_of(file, scenario);
            self::scenario(&mut doc, ctx, file, scenario, &id);
            entries.push(Entry {
                status: scenario.status.clone(),
                name: scenario.name.clone(),
                start,
                end: doc.lines.len() - 1,
                id,
            });
            doc.blank();
        }
        doc.raw(marker("end", "file", &[("id", &file.file)]));
        doc.blank();
    }
    (doc, entries)
}

fn element(doc: &mut Doc, label: &str, value: &Value) {
    let name = value["name"].as_str().unwrap_or("");
    doc.raw(if name.is_empty() {
        format!("**{label}**")
    } else {
        format!("**{label}: {}**", one_line(name))
    });
    if let Some(text) = value["description"].as_str() {
        doc.quote(text);
    }
    if let Some(text) = value["comment"].as_str() {
        doc.quote(text);
    }
}

fn scenario(doc: &mut Doc, ctx: &Ctx<'_>, file: &File, s: &Scenario, id: &str) {
    let mut attrs = vec![("id", id.to_owned()), ("status", s.status.clone())];
    attrs.push(("file", file.file.clone()));
    attrs.push(("line", s.line.to_string()));
    if let Some(row) = s.example {
        attrs.push(("example", row.to_string()));
    }
    if !s.tags.is_empty() {
        attrs.push(("tags", s.tags.join(" ")));
    }
    let attrs: Vec<(&str, &str)> = attrs.iter().map(|(k, v)| (*k, v.as_str())).collect();
    doc.raw(marker("begin", "scenario", &attrs));
    doc.raw(format!("### [{}] {}", s.status, one_line(&s.name)));
    let mut location = format!("{} · {}", code(id), s.status);
    if let Some(us) = s.duration_us {
        location.push_str(&format!(" in {}", duration(us)));
    }
    if !s.tags.is_empty() {
        location.push_str(&format!(" · {}", one_line(&s.tags.join(" "))));
    }
    if let Some(coverage) = s.evidence.as_ref().filter(|_| ctx.run.evidence) {
        location.push_str(&format!(" · evidence: {}", one_line(coverage)));
    }
    doc.raw(location);
    doc.blank();
    if let Some(rule) = &s.rule {
        element(doc, "Rule", rule);
    }
    if let Some(examples) = &s.examples {
        element(doc, "Examples", examples);
    }
    if let Some(text) = &s.description {
        doc.quote(text);
    }
    if let Some(text) = &s.comment {
        doc.raw("**Comment:**");
        doc.quote(text);
    }
    doc.blank();
    let mut evidence = Vec::new();
    list(doc, &s.items, 0, "", &mut evidence);
    doc.blank();
    if let Some(text) = &s.failure {
        let mut attrs = vec![("id", id)];
        if let Some(kind) = &s.failure_kind {
            attrs.push(("kind", kind));
        }
        doc.raw(marker("begin", "failure", &attrs));
        doc.fenced("text", text);
        doc.raw(marker("end", "failure", &[("id", id)]));
        doc.blank();
    }
    for (step, item) in evidence {
        evidence_part(doc, ctx, id, &step, item);
        doc.blank();
    }
    doc.raw(marker("end", "scenario", &[("id", id)]));
}

/// The numbered step list. A step's number is its position among its
/// siblings, and its evidence is named by the dotted path of those (`2.1`).
fn list<'a>(
    doc: &mut Doc,
    items: &'a [Item],
    depth: usize,
    prefix: &str,
    evidence: &mut Vec<(String, &'a Value)>,
) {
    let pad = " ".repeat(3 * depth);
    let mut number = 0;
    for item in items {
        if item.kind == "section" {
            doc.blank();
            if let Some(title) = &item.title {
                doc.raw(format!("{pad}**{}**", one_line(title)));
            }
            if let Some(note) = &item.note {
                for line in note.lines() {
                    doc.raw(guard(&format!("{pad}{line}")));
                }
            }
            doc.blank();
            continue;
        }
        number += 1;
        let path = if prefix.is_empty() {
            number.to_string()
        } else {
            format!("{prefix}.{number}")
        };
        let status = if item.kind == "disabled" {
            "disabled"
        } else {
            item.status.unwrap_or("open")
        };
        let words = format!(
            "{}{}",
            item.keyword.as_deref().unwrap_or(""),
            item.text.as_deref().unwrap_or("")
        );
        let mut line = format!("{pad}{number}. [{status}] {}", code(words.trim()));
        if let Some(us) = item.duration_us {
            line.push_str(&format!(" · {}", duration(us)));
        }
        for pair in item.vars.iter().filter_map(|v| v.as_array()).flatten() {
            let value = pair[1].as_str().map_or_else(|| "null".to_owned(), code);
            line.push_str(&format!(
                " · {} = {value}",
                code(pair[0].as_str().unwrap_or(""))
            ));
        }
        doc.raw(line);
        let sub = format!("{pad}   - ");
        if let Some(comment) = &item.comment {
            doc.raw(format!("{sub}comment: {}", one_line(comment)));
        }
        for warning in &item.warnings {
            doc.raw(format!("{sub}warning: {}", one_line(warning)));
        }
        for attempt in &item.attempts {
            doc.raw(format!(
                "{sub}attempt {}: {}",
                attempt["n"],
                one_line(attempt["message"].as_str().unwrap_or(""))
            ));
        }
        evidence.extend(item.evidence.iter().map(|e| (path.clone(), e)));
        list(doc, &item.children, depth + 1, &path, evidence);
    }
}

fn link(base: &str, path: &str) -> String {
    let encoded: Vec<String> = path.split('/').map(html::encode).collect();
    format!("[{}]({base}{})", code(path), encoded.join("/"))
}

fn evidence_part(doc: &mut Doc, ctx: &Ctx<'_>, id: &str, step: &str, e: &Value) {
    let text = |key: &str| e[key].as_str().unwrap_or("").to_owned();
    let (kind, group, resource, path) =
        (text("kind"), text("group"), text("resource"), text("path"));
    let mut attrs = vec![("id", id), ("step", step), ("kind", kind.as_str())];
    if !group.is_empty() {
        attrs.push(("group", &group));
    }
    if !resource.is_empty() {
        attrs.push(("resource", &resource));
    }
    if !path.is_empty() {
        attrs.push(("path", &path));
    }
    doc.raw(marker("begin", "evidence", &attrs));
    let c = &e["content"];
    let title = match (kind.as_str(), c) {
        ("http", Value::Object(_)) => format!(
            "{} {} → {}",
            c["method"].as_str().unwrap_or(""),
            c["url"].as_str().unwrap_or(""),
            status(c)
        ),
        ("sql", Value::Object(_)) => "SQL".to_owned(),
        _ => e["title"].as_str().unwrap_or(&kind).to_owned(),
    };
    doc.raw(format!("#### step {step} · {}", one_line(&title)));
    match (kind.as_str(), c) {
        ("http", Value::Object(_)) => http(doc, ctx, c),
        ("sql", Value::Object(_)) => {
            doc.fenced("sql", c["sql"].as_str().unwrap_or(""));
            doc.raw(format!("binds: {}", code(&c["binds"].to_string())));
        }
        _ => {
            if !path.is_empty() {
                let size = e["size"]
                    .as_u64()
                    .map_or_else(String::new, |n| format!(" ({n} bytes)"));
                doc.raw(format!("{}{size}", link(ctx.base, &path)));
            }
            body_block(doc, ctx, c);
        }
    }
    doc.raw(marker("end", "evidence", &[("id", id), ("step", step)]));
}

/// A transport failure has no status; `null` would read as a response.
fn status(c: &Value) -> String {
    c["status"]
        .as_u64()
        .map_or_else(|| "no response".to_owned(), |n| n.to_string())
}

fn headers(doc: &mut Doc, pairs: &Value) {
    for pair in pairs.as_array().into_iter().flatten() {
        doc.raw(format!(
            "- {}: {}",
            one_line(pair[0].as_str().unwrap_or("")),
            code(pair[1].as_str().unwrap_or(""))
        ));
    }
}

fn http(doc: &mut Doc, ctx: &Ctx<'_>, c: &Value) {
    doc.raw(format!(
        "**Request:** {}",
        code(&format!(
            "{} {}",
            c["method"].as_str().unwrap_or(""),
            c["url"].as_str().unwrap_or("")
        ))
    ));
    headers(doc, &c["request"]["headers"]);
    body_block(doc, ctx, &c["request"]["body"]);
    let took = c["duration_us"]
        .as_u64()
        .map_or_else(String::new, |us| format!(" · {}", duration(us)));
    doc.raw(format!("**Response:** {}{took}", status(c)));
    headers(doc, &c["response"]["headers"]);
    body_block(doc, ctx, &c["response"]["body"]);
}

/// A body is JSON pretty-printed (sorted) only when it parses, as received
/// otherwise; a spilled one is a link.
#[expect(clippy::expect_used, reason = "a parsed value always serializes")]
fn body_block(doc: &mut Doc, ctx: &Ctx<'_>, value: &Value) {
    match value {
        Value::Null => {}
        Value::String(text) => match serde_json::from_str::<Value>(text) {
            Ok(parsed) => doc.fenced(
                "json",
                &serde_json::to_string_pretty(&parsed).expect("a value is JSON"),
            ),
            Err(_) => doc.fenced("text", text),
        },
        Value::Object(map) if map.get("path").is_some_and(Value::is_string) => {
            let size = map["size"]
                .as_u64()
                .map_or_else(String::new, |n| format!(" ({n} bytes)"));
            doc.raw(format!(
                "{}{size}",
                link(ctx.base, map["path"].as_str().unwrap_or(""))
            ));
        }
        other => doc.fenced(
            "json",
            &serde_json::to_string_pretty(other).expect("a value is JSON"),
        ),
    }
}

#[cfg(test)]
mod tests;
