use super::*;
use crate::report::model;
use regex::Regex;
use serde_json::json;

#[test]
fn a_fence_outgrows_the_longest_backtick_run_in_its_content() {
    assert_eq!(fence("plain"), "```");
    assert_eq!(fence("a ``` b"), "````");
    assert_eq!(fence("`````x"), "``````");
}

#[test]
fn a_code_span_survives_backticks_and_newlines() {
    assert_eq!(code("a"), "`a`");
    assert_eq!(code("a`b"), "``a`b``");
    assert_eq!(code("`a`"), "`` `a` ``");
    assert_eq!(code("a\nb"), "`a\\nb`");
    assert_eq!(code(""), "` `");
}

#[test]
fn a_double_hyphen_in_a_value_is_escaped() {
    assert_eq!(attr("a--b"), "a-\\u002db");
    assert_eq!(attr("---"), "-\\u002d-");
    assert_eq!(attr("a-b"), "a-b");
    assert_eq!(attr("q\"\\\n\t"), "q\\\"\\\\\\n\\t");
    assert!(
        !marker("begin", "file", &[("id", "x--y -->")])
            .replace("<!-- ", "")
            .replace(" -->", "")
            .contains("--")
    );
}

#[test]
fn a_marker_like_line_gets_one_leading_space_and_nothing_else_changes() {
    assert_eq!(guard("<!-- bddkit:end x -->"), " <!-- bddkit:end x -->");
    assert_eq!(guard("<!-- other -->"), "<!-- other -->");
    assert_eq!(guard("  <!-- bddkit:end"), "  <!-- bddkit:end");
}

#[test]
fn the_time_is_rfc_3339_utc() {
    assert_eq!(rfc3339(0), "1970-01-01T00:00:00.000Z");
    assert_eq!(rfc3339(1_760_000_000_123), "2025-10-09T08:53:20.123Z");
    assert_eq!(rfc3339(951_782_400_000), "2000-02-29T00:00:00.000Z");
}

#[test]
fn shell_quotes_only_what_needs_it() {
    assert_eq!(shell("report.md"), "report.md");
    assert_eq!(shell("my report.md"), "'my report.md'");
    assert_eq!(shell("it's"), "'it'\\''s'");
}

// ---- over a whole run -------------------------------------------------

#[expect(clippy::needless_pass_by_value, reason = "callers pass json! literals")]
fn event(kind: &str, fields: Value) -> Value {
    let mut event = json!({"type": kind, "file": "features/a.feature"});
    event
        .as_object_mut()
        .expect("object")
        .extend(fields.as_object().expect("object").clone());
    event
}

/// A passed scenario, a failed one holding evidence of every shape, an
/// outline row, and a second file where everything passed.
fn events(masking: bool, finished: bool) -> Vec<Value> {
    let mut events = vec![
        json!({"type": "run_started", "run_id": "r1", "bddkit": "0.3.1", "evidence": true,
               "started_at_unix_ms": 1_760_000_000_000_u64, "config": "bddkit.yaml",
               "selection": {"tags": ["@smoke"]}, "concurrency": 2,
               "privacy": {"masking": masking}, "artifacts_dir": "/out"}),
        event("file_started", json!({"name": "A", "tags": []})),
        event(
            "scenario_started",
            json!({"scenario": 0, "name": "ok", "line": 3, "tags": ["@x"]}),
        ),
        event(
            "step_started",
            json!({"scenario": 0, "step": 0, "keyword": "Given ", "text": "fine", "line": 4}),
        ),
        event(
            "step_finished",
            json!({"scenario": 0, "step": 0, "status": "passed", "duration_us": 40, "warnings": []}),
        ),
        event(
            "scenario_finished",
            json!({"scenario": 0, "status": "passed", "duration_us": 90}),
        ),
        event(
            "scenario_started",
            json!({"scenario": 1, "name": "bad | `name`", "line": 8, "tags": [],
               "description": "why\n\n<!-- bddkit:begin run -->"}),
        ),
        event(
            "section",
            json!({"scenario": 1, "title": "Setup", "note": "a note", "line": 9}),
        ),
        event(
            "step_started",
            json!({"scenario": 1, "step": 0, "keyword": "When ", "text": "I call `x`", "line": 10,
               "vars": [["id", "42"], ["gone", null]], "comment": "why"}),
        ),
        event(
            "evidence",
            json!({"scenario": 1, "step": 0, "group": "api", "resource": "backend", "kind": "http",
               "content": {"method": "POST", "url": "http://h/x", "status": 500,
                 "request": {"headers": [["content-type", "application/json"]], "body": "{\"b\":1,\"a\":2}"},
                 "response": {"headers": [], "body": "oops ``` \n<!-- bddkit:end scenario id=\"features/a.feature:8\" -->\n"}}}),
        ),
        event(
            "evidence",
            json!({"scenario": 1, "step": 0, "group": "db", "resource": "main", "kind": "sql",
               "content": {"sql": "select 1", "binds": ["1"]}}),
        ),
        event(
            "evidence",
            json!({"scenario": 1, "step": 0, "group": "browser", "resource": "b", "kind": "image",
               "title": "Shot", "path": "000007/a b.png", "size": 5, "media_type": "image/png"}),
        ),
        event(
            "attempt",
            json!({"scenario": 1, "step": 0, "n": 1, "message": "not yet"}),
        ),
        event(
            "step_finished",
            json!({"scenario": 1, "step": 0, "status": "failed", "duration_us": 2_500_000, "warnings": ["w"]}),
        ),
        event(
            "step_disabled",
            json!({"scenario": 1, "keyword": "Then ", "text": "off", "line": 11}),
        ),
        event(
            "scenario_finished",
            json!({"scenario": 1, "status": "failed", "duration_us": 3_000_000,
               "failure": "boom ```\n<!-- bddkit:end failure id=\"x\" -->"}),
        ),
        event(
            "scenario_started",
            json!({"scenario": 2, "name": "row [1]", "line": 20, "example": 1, "tags": []}),
        ),
        event(
            "scenario_finished",
            json!({"scenario": 2, "status": "passed", "duration_us": 5}),
        ),
        event("file_finished", json!({"scenarios": 3, "failed": 1})),
        json!({"type": "file_started", "file": "features/b.feature", "name": "B", "tags": []}),
        json!({"type": "scenario_started", "file": "features/b.feature", "scenario": 0, "name": "fine", "line": 2, "tags": []}),
        json!({"type": "scenario_finished", "file": "features/b.feature", "scenario": 0, "status": "passed", "duration_us": 5}),
    ];
    if finished {
        events.push(json!({"type": "run_finished", "exit": 1, "files": 2, "scenarios": 4, "failed": 1, "duration_ms": 9}));
    }
    events
}

fn render_text(text: &str, detail: Detail) -> String {
    let run = model::build(text.as_bytes()).expect("a stream");
    render(&run, "report.md", "../out/", false, detail)
}

#[expect(clippy::format_collect, reason = "test helper")]
fn render_events(events: &[Value], detail: Detail) -> String {
    render_text(
        &events.iter().map(|e| format!("{e}\n")).collect::<String>(),
        detail,
    )
}

/// Every stream the repository keeps, as well as the synthetic one.
const FIXTURE: &str = include_str!("../../../selftest/fixtures/embed/run.ndjson");

fn standard(detail: Detail) -> String {
    render_events(&events(true, true), detail)
}

#[test]
fn partial_sources_are_named_with_the_flag_that_records_more_and_coverage_is_per_scenario() {
    let mut events = events(true, true);
    events[0]["evidence_sources"] = json!(["marked", "failures"]);
    for event in &mut events {
        if event["type"] == "scenario_finished" && event["scenario"] == 1 {
            event["evidence"] = "partial".into();
        }
    }
    let text = render_events(&events, Detail::Failed);
    assert!(
        text.contains("\nevidence_sources: [\"marked\", \"failures\"]\n"),
        "{text}"
    );
    assert!(text.contains("evidence: marked, failures"), "{text}");
    assert!(
        text.contains("only for marked, failures") && text.contains("`--evidence` (`all`)"),
        "{text}"
    );
    assert!(text.contains("· evidence: partial"), "{text}");

    // `all` has nothing to warn about, and an old stream reads as `all`.
    let text = standard(Detail::Failed);
    assert!(!text.contains("evidence was recorded only"), "{text}");
    assert!(text.contains("evidence_sources: [\"all\"]"), "{text}");
}

#[test]
fn every_marker_line_matches_the_grammar_and_begins_and_ends_pair_up() {
    let grammar =
        Regex::new(r#"^<!-- bddkit:(begin|end) [a-z]+( [a-z_]+="[^"\\]*(\\.[^"\\]*)*")* -->$"#)
            .expect("regex");
    let mut texts: Vec<String> = [
        (Detail::Failed, true, true),
        (Detail::All, true, true),
        (Detail::All, false, false),
    ]
    .map(|(detail, finished, masking)| render_events(&events(masking, finished), detail))
    .into();
    texts.push(render_text(FIXTURE, Detail::All));
    for text in texts {
        let mut open: Vec<(String, String)> = Vec::new();
        for line in text.lines().filter(|l| l.starts_with("<!-- bddkit:")) {
            assert!(grammar.is_match(line), "{line}");
            let words: Vec<&str> = line.split(' ').collect();
            let kind = words[3].to_owned();
            let id = line
                .split_once(" id=\"")
                .map_or(String::new(), |(_, rest)| rest.to_owned());
            if words[1] == "bddkit:begin" {
                open.push((kind, id));
            } else {
                let (begun, begun_id) = open.pop().expect("an open part");
                assert_eq!(begun, kind, "{line}");
                assert!(
                    id.starts_with(begun_id.split('"').next().unwrap_or("")),
                    "{line}"
                );
            }
        }
        assert!(open.is_empty(), "{open:?}");
    }
}

/// The range an index row names, cut the way `sed -n 'a,bp'` cuts it.
fn ranges(text: &str) -> Vec<(String, Vec<String>)> {
    let lines: Vec<&str> = text.lines().collect();
    let begin = lines
        .iter()
        .position(|l| l.starts_with("<!-- bddkit:begin index"))
        .expect("index");
    let end = lines
        .iter()
        .position(|l| l.starts_with("<!-- bddkit:end index"))
        .expect("index end");
    lines[begin..end]
        .iter()
        .filter(|l| l.starts_with("| `"))
        .filter_map(|row| {
            let cells: Vec<&str> = row.split(" | ").collect();
            let (from, to) = cells.get(2)?.split_once('-')?;
            let (from, to): (usize, usize) = (from.parse().ok()?, to.parse().ok()?);
            let id = cells[0]
                .trim_start_matches("| ")
                .trim_matches('`')
                .to_owned();
            Some((
                id,
                lines[from - 1..to]
                    .iter()
                    .map(|l| (*l).to_owned())
                    .collect(),
            ))
        })
        .collect()
}

#[test]
fn every_index_range_cuts_out_exactly_its_scenario() {
    for detail in [Detail::Failed, Detail::All] {
        let text = standard(detail);
        let cut = ranges(&text);
        assert_eq!(cut.len(), if detail == Detail::All { 4 } else { 1 });
        for (id, lines) in cut {
            let first = lines.first().expect("lines");
            let last = lines.last().expect("lines");
            assert!(
                first.starts_with(&format!("<!-- bddkit:begin scenario id=\"{id}\"")),
                "{first}"
            );
            assert_eq!(last, &format!("<!-- bddkit:end scenario id=\"{id}\" -->"));
        }
    }
}

#[test]
fn the_front_matter_states_the_files_own_length() {
    let text = standard(Detail::All);
    let lines = text.lines().count();
    assert!(text.contains(&format!("\nlines: {lines}\n")));
    assert!(text.lines().take(25).any(|l| l == "---"), "within head -25");
    assert!(
        text.lines().skip(1).take(24).any(|l| l == "---"),
        "closed within head -25"
    );
}

#[test]
fn a_body_with_backticks_and_a_marker_like_line_cannot_break_the_file() {
    let text = standard(Detail::All);
    // The only column-0 markers are bddkit's own: the ones in the
    // response body, the failure and the description were spaced.
    let grammar = Regex::new(r"^<!-- bddkit:(begin|end) [a-z]+ ").expect("regex");
    assert!(text.contains(" <!-- bddkit:end scenario id=\"features/a.feature:8\" -->"));
    assert!(text.contains(" <!-- bddkit:end failure id=\"x\" -->"));
    assert!(
        text.contains("> <!-- bddkit:begin run -->"),
        "a quote prefixes it already"
    );
    let ends = text
        .lines()
        .filter(|l| grammar.is_match(l) && l.starts_with("<!-- bddkit:end scenario"))
        .count();
    assert_eq!(ends, 4, "one end marker per scenario, none from content");
    // The fence around the body is longer than the ``` inside it.
    assert!(text.contains("\n````text\noops ``` \n"));
}

#[test]
fn the_default_detail_shows_failures_and_counts_the_rest() {
    let text = standard(Detail::Failed);
    assert!(text.contains("### [failed] bad | `name`"));
    assert!(!text.contains("### [passed]"));
    assert!(text.contains("| `features/a.feature` | 3 | 2 |"));
    assert!(text.contains("| `features/b.feature` | 1 | 1 |"));
    assert!(
        !text.contains("begin file id=\"features/b.feature\""),
        "nothing to show in b"
    );
    assert!(text.contains("Passed scenarios are counted, not shown"));
    assert!(text.contains("\ndetail: failed\n"));
}

#[test]
fn detail_all_shows_every_scenario_and_lists_it() {
    let text = standard(Detail::All);
    assert_eq!(text.matches("### [passed]").count(), 3);
    let index: Vec<&str> = text
        .lines()
        .filter(|l| l.starts_with("| `features/") && l.contains('-'))
        .collect();
    assert_eq!(index.len(), 4);
    assert!(
        index[0].contains("| failed |"),
        "failed first: {}",
        index[0]
    );
    assert!(!text.contains("| file | scenarios | passed |"));
    assert!(!text.contains("Passed scenarios are counted"));
}

#[test]
fn an_outline_row_has_its_own_id() {
    let text = standard(Detail::All);
    assert!(text.contains("<!-- bddkit:begin scenario id=\"features/a.feature:20#1\" status=\"passed\" file=\"features/a.feature\" line=\"20\" example=\"1\" -->"));
}

#[test]
fn an_incomplete_stream_says_so_in_the_front_matter_and_an_open_scenario_is_not_a_pass() {
    let mut stream = events(true, false);
    stream.truncate(8);
    let text = render_events(&stream, Detail::Failed);
    assert!(text.contains("\ncomplete: false\n"));
    assert!(!text.contains("\nexit:"));
    assert!(text.contains("this stream is incomplete"));
    assert!(
        text.contains("### [open] bad | `name`"),
        "shown under the default detail"
    );
}

#[test]
fn a_revealed_run_is_marked_in_the_front_matter_and_the_run_part() {
    let text = render_events(&events(false, true), Detail::Failed);
    assert!(text.contains("\nmasking: false\n"));
    let run: Vec<&str> = text
        .lines()
        .skip_while(|l| !l.starts_with("<!-- bddkit:begin run"))
        .skip(2)
        .take(1)
        .collect();
    assert!(run[0].contains("masking was off"), "{}", run[0]);
}

#[test]
fn the_same_stream_gives_the_same_bytes() {
    assert_eq!(standard(Detail::All), standard(Detail::All));
}

#[test]
fn steps_carry_status_words_durations_vars_and_sub_items() {
    let text = standard(Detail::Failed);
    assert!(text.contains("1. [failed] `` When I call `x` ``"), "{text}");
    assert!(text.contains(" · 2.50 s · `id` = `42` · `gone` = null\n"));
    assert!(text.contains("   - comment: why\n"));
    assert!(text.contains("   - warning: w\n"));
    assert!(text.contains("   - attempt 1: not yet\n"));
    assert!(text.contains("2. [disabled] `Then off`"));
    assert!(text.contains("\n**Setup**\na note\n"));
}

#[test]
fn evidence_is_a_part_per_item_under_its_step() {
    let text = standard(Detail::Failed);
    assert!(text.contains("<!-- bddkit:begin evidence id=\"features/a.feature:8\" step=\"1\" kind=\"http\" group=\"api\" resource=\"backend\" -->"));
    assert!(text.contains("<!-- bddkit:begin evidence id=\"features/a.feature:8\" step=\"1\" kind=\"sql\" group=\"db\" resource=\"main\" -->"));
    assert!(
        text.contains(
            "kind=\"image\" group=\"browser\" resource=\"b\" path=\"000007/a b.png\" -->"
        )
    );
    assert!(
        text.contains("```json\n{\n  \"a\": 2,\n  \"b\": 1\n}\n```"),
        "sorted and pretty"
    );
    assert!(text.contains("```sql\nselect 1\n```\nbinds: `[\"1\"]`"));
    assert!(text.contains("[`000007/a b.png`](../out/000007/a%20b.png) (5 bytes)"));
}

#[test]
fn a_panicked_file_with_no_scenario_still_has_its_part() {
    let mut stream = events(true, true);
    stream.insert(
        1,
        json!({"type": "file_started", "file": "p.feature", "name": "P", "tags": []}),
    );
    stream.insert(2, json!({"type": "file_finished", "file": "p.feature", "scenarios": 0, "failed": 1, "panicked": true}));
    let text = render_events(&stream, Detail::Failed);
    assert!(
        text.contains("<!-- bddkit:begin file id=\"p.feature\" scenarios=\"0\" failed=\"0\" -->")
    );
    assert!(text.contains("panicked"));
}

#[test]
fn the_checked_in_stream_has_exact_ranges_too() {
    let text = render_text(FIXTURE, Detail::All);
    assert!(!ranges(&text).is_empty());
    for (id, lines) in ranges(&text) {
        assert!(lines[0].contains(&format!("scenario id=\"{id}\"")), "{id}");
    }
}

#[cfg(unix)]
#[test]
fn the_howto_commands_run_as_written() {
    let text = standard(Detail::Failed);
    let dir = std::env::temp_dir().join(format!("bddkit-md-howto-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    std::fs::write(dir.join("report.md"), &text).expect("report");
    let start = text.find("<!-- bddkit:begin howto").expect("howto");
    let end = text.find("<!-- bddkit:end howto").expect("howto end");
    let mut ran = 0;
    for line in text[start..end].lines().filter(|l| l.starts_with("- ")) {
        let command = &line[line.find(": `").expect("a command") + 3..line.len() - 1];
        let out = std::process::Command::new("sh")
            .args(["-c", command])
            .current_dir(&dir)
            .output()
            .expect("sh");
        let stdout = String::from_utf8(out.stdout).expect("utf8");
        assert!(out.status.success(), "{command}");
        let first = stdout.lines().next().unwrap_or("");
        match line.split(':').next().expect("label") {
            "- Totals" => assert_eq!(first, "---"),
            "- The index, with exact line ranges" => {
                assert!(first.starts_with("<!-- bddkit:begin index"), "{first}");
            }
            "- One scenario by its range" | "- One scenario by id" => {
                assert!(
                    first.starts_with("<!-- bddkit:begin scenario id=\"features/a.feature:8\""),
                    "{first}"
                );
                assert!(
                    stdout
                        .trim_end()
                        .ends_with("<!-- bddkit:end scenario id=\"features/a.feature:8\" -->"),
                    "{command}"
                );
            }
            "- Failed scenarios" => assert!(first.contains("status=\"failed\""), "{first}"),
            "- All SQL" => assert!(first.contains("kind=\"sql\""), "{first}"),
            "- Every failure text" => {
                assert!(first.starts_with("<!-- bddkit:begin failure"), "{first}");
            }
            other => panic!("an example the test does not know: {other}"),
        }
        ran += 1;
    }
    assert_eq!(ran, 7);
    let _ = std::fs::remove_dir_all(&dir);
}
