use anyhow::{Context, Result};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The three outcomes a step can have once a run has started. Nothing else:
/// an unknown step is refused before the first request and never gets here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepStatus {
    Passed,
    Failed,
    Skipped,
}

impl StepStatus {
    fn as_str(self) -> &'static str {
        match self {
            StepStatus::Passed => "passed",
            StepStatus::Failed => "failed",
            StepStatus::Skipped => "skipped",
        }
    }
}

#[derive(Debug)]
pub struct StepResult {
    pub keyword: String,
    /// The raw feature text, never interpolated: two runs' reports differ
    /// only in results and timings.
    pub text: String,
    pub line: usize,
    pub status: StepStatus,
    pub duration: Duration,
}

#[derive(Debug)]
pub struct ScenarioResult {
    pub name: String,
    pub line: usize,
    pub failure: Option<String>,
    pub steps: Vec<StepResult>,
    pub duration: Duration,
}

#[derive(Debug)]
pub struct FileResult {
    pub path: PathBuf,
    /// The text after `Feature:`.
    pub name: String,
    pub scenarios: Vec<ScenarioResult>,
}

impl FileResult {
    pub fn failed(&self) -> usize {
        self.scenarios
            .iter()
            .filter(|s| s.failure.is_some())
            .count()
    }

    fn duration(&self) -> Duration {
        self.scenarios.iter().map(|s| s.duration).sum()
    }
}

/// Collects a whole file's output into one string. The caller prints it with one
/// `print!`: under a parallel run, line-by-line printing from eight workers
/// gets interleaved right in the middle of a failed request's dump.
pub fn render_file(r: &FileResult) -> String {
    let mark = if r.failed() == 0 { "✓" } else { "✗" };
    let mut out = format!(
        "  {mark} {} — scenarios: {}\n",
        r.path.display(),
        r.scenarios.len()
    );
    for s in &r.scenarios {
        if let Some(f) = &s.failure {
            out.push_str(&format!(
                "\nFAIL  {}:{} › {}\n{f}\n",
                r.path.display(),
                s.line,
                s.name
            ));
        }
    }
    out
}

pub fn print_summary(results: &[FileResult], run_id: &str) -> i32 {
    let files = results.len();
    let scenarios: usize = results.iter().map(|r| r.scenarios.len()).sum();
    let failed: usize = results.iter().map(FileResult::failed).sum();
    println!("\nrun {run_id}");
    println!("files: {files}, scenarios: {scenarios}, failed: {failed}");
    if failed == 0 { 0 } else { 1 }
}

/// Creates (truncates) a report file before anything runs. A run that dies on
/// its config then leaves an empty file a parser rejects loudly, rather than
/// yesterday's green one.
pub fn prepare(path: &Path) -> Result<()> {
    path.parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::File::create(path).map(drop))
        .with_context(|| format!("cannot create the report file {}", path.display()))
}

pub fn write_junit(results: &[FileResult], path: &Path) -> Result<()> {
    save(path, junit(results))
}

pub fn write_cucumber_json(results: &[FileResult], path: &Path) -> Result<()> {
    save(path, serde_json::to_vec_pretty(&cucumber_json(results))?)
}

fn save(path: &Path, bytes: impl AsRef<[u8]>) -> Result<()> {
    std::fs::write(path, bytes)
        .with_context(|| format!("cannot write the report file {}", path.display()))
}

/// Escapes text for an element body or a quoted attribute. A failure text can
/// carry anything a response body can, including the NUL bytes of the
/// `<<null>>` sentinel; XML 1.0 has no way to write those, escaped or not,
/// so they become U+FFFD rather than a file the CI parser rejects.
fn xml(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\t' | '\n' | '\r' => out.push(c),
            c if c < ' ' || c == '\u{FFFE}' || c == '\u{FFFF}' => out.push('\u{FFFD}'),
            c => out.push(c),
        }
    }
    out
}

/// One `<testsuite>` per feature file, one `<testcase>` per scenario — the
/// Cucumber-JVM layout, so a dashboard groups by file. The steps go into
/// `<system-out>`, since JUnit has no level below a test case.
fn junit(results: &[FileResult]) -> String {
    let tests: usize = results.iter().map(|r| r.scenarios.len()).sum();
    let failures: usize = results.iter().map(FileResult::failed).sum();
    let time: f64 = results.iter().map(|r| r.duration().as_secs_f64()).sum();
    let mut out = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuites tests=\"{tests}\" failures=\"{failures}\" time=\"{time:.3}\">\n"
    );
    for r in results {
        let file = xml(&r.path.display().to_string());
        let _ = writeln!(
            out,
            "  <testsuite name=\"{file}\" tests=\"{}\" failures=\"{}\" time=\"{:.3}\">",
            r.scenarios.len(),
            r.failed(),
            r.duration().as_secs_f64()
        );
        for s in &r.scenarios {
            let _ = writeln!(
                out,
                "    <testcase name=\"{}\" classname=\"{file}\" time=\"{:.3}\">",
                xml(&s.name),
                s.duration.as_secs_f64()
            );
            if let Some(f) = &s.failure {
                let message = f.lines().next().unwrap_or_default().trim();
                let _ = writeln!(
                    out,
                    "      <failure message=\"{}\">{}</failure>",
                    xml(message),
                    xml(f)
                );
            }
            out.push_str("      <system-out>");
            for st in &s.steps {
                let _ = write!(
                    out,
                    "\n{}{} ... {} ({:.3}s)",
                    xml(&st.keyword),
                    xml(&st.text),
                    st.status.as_str(),
                    st.duration.as_secs_f64()
                );
            }
            out.push_str("\n      </system-out>\n    </testcase>\n");
        }
        out.push_str("  </testsuite>\n");
    }
    out.push_str("</testsuites>\n");
    out
}

/// The shape cucumber-html-reporter and Allure accept. Durations are in
/// nanoseconds, as cucumber-js and Cucumber-JVM write them.
fn cucumber_json(results: &[FileResult]) -> serde_json::Value {
    use serde_json::json;
    let id = |name: &str| name.to_lowercase().replace(' ', "-");
    let nanos = |d: Duration| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX);
    let features: Vec<_> = results
        .iter()
        .map(|r| {
            let elements: Vec<_> = r
                .scenarios
                .iter()
                .map(|s| {
                    let steps: Vec<_> = s
                        .steps
                        .iter()
                        .map(|st| {
                            let mut result = json!({
                                "status": st.status.as_str(),
                                "duration": nanos(st.duration),
                            });
                            if st.status == StepStatus::Failed {
                                result["error_message"] = json!(s.failure);
                            }
                            json!({
                                "keyword": st.keyword,
                                "name": st.text,
                                "line": st.line,
                                "result": result,
                            })
                        })
                        .collect();
                    let mut element = json!({
                        "id": format!("{};{}", id(&r.name), id(&s.name)),
                        "keyword": "Scenario",
                        "name": s.name,
                        "line": s.line,
                        "type": "scenario",
                        "steps": steps,
                    });
                    // A failure no step owns — a plugin reset, a panicked file
                    // — would otherwise leave a scenario whose steps are all
                    // skipped and which a reader counts as skipped: the hook
                    // slot is where such a failure belongs.
                    if s.failure.is_some()
                        && !s.steps.iter().any(|st| st.status == StepStatus::Failed)
                    {
                        element["before"] = json!([{
                            "result": {
                                "status": "failed",
                                "duration": nanos(s.duration),
                                "error_message": s.failure,
                            }
                        }]);
                    }
                    element
                })
                .collect();
            json!({
                "uri": r.path.display().to_string(),
                "id": id(&r.name),
                "keyword": "Feature",
                "name": r.name,
                "line": 1,
                "elements": elements,
            })
        })
        .collect();
    json!(features)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(failure: Option<&str>) -> FileResult {
        FileResult {
            path: PathBuf::from("features/auth.feature"),
            name: "auth".to_string(),
            scenarios: vec![ScenarioResult {
                name: "user login".to_string(),
                line: 34,
                failure: failure.map(str::to_string),
                steps: Vec::new(),
                duration: Duration::from_millis(5),
            }],
        }
    }

    #[test]
    fn a_passing_file_renders_a_single_summary_line() {
        let out = render_file(&result(None));
        assert_eq!(
            out.lines().count(),
            1,
            "extra lines break the atomicity of the output: {out:?}"
        );
        assert!(out.contains("✓"), "{out}");
        assert!(out.contains("features/auth.feature"), "{out}");
    }

    #[test]
    fn a_failing_file_renders_the_mark_scenario_and_dump_in_one_string() {
        // Everything about the file must come back as ONE string:
        // piecemeal printing interleaves output when concurrency > 1.
        let out = render_file(&result(Some("  the response code is 200\nexpected: 200")));
        assert!(out.contains("✗"), "{out}");
        assert!(out.contains("FAIL"), "{out}");
        assert!(out.contains("features/auth.feature:34"), "{out}");
        assert!(out.contains("user login"), "{out}");
        assert!(out.contains("expected: 200"), "{out}");
    }

    #[test]
    fn every_failed_scenario_of_a_file_appears_in_the_same_string() {
        let r = FileResult {
            path: PathBuf::from("f.feature"),
            name: "f".into(),
            scenarios: vec![
                ScenarioResult {
                    name: "first".into(),
                    line: 3,
                    failure: Some("reason A".into()),
                    steps: Vec::new(),
                    duration: Duration::ZERO,
                },
                ScenarioResult {
                    name: "second".into(),
                    line: 9,
                    failure: Some("reason B".into()),
                    steps: Vec::new(),
                    duration: Duration::ZERO,
                },
            ],
        };
        let out = render_file(&r);
        assert!(
            out.contains("reason A") && out.contains("reason B"),
            "{out}"
        );
    }

    #[test]
    fn junit_stays_well_formed_whatever_the_failure_contains() {
        let out = junit(&[result(Some(
            "  ]]> <tag attr=\"q\"> & \u{0}__bddkit_null__\u{0} \u{1}",
        ))]);
        let package = sxd_document::parser::parse(&out)
            .unwrap_or_else(|e| panic!("must parse: {e:?}\n{out}"));
        let failure = sxd_xpath::evaluate_xpath(&package.as_document(), "//failure")
            .expect("xpath")
            .string();
        assert!(failure.contains("]]> <tag attr=\"q\"> &"), "{failure}");
        assert!(
            !failure.contains('\u{0}'),
            "NUL cannot exist in XML: {failure}"
        );
    }

    #[test]
    fn a_failure_no_step_owns_lands_in_the_before_hook() {
        let json = cucumber_json(&[result(Some("panic while running the file"))]);
        let element = &json[0]["elements"][0];
        assert_eq!(element["before"][0]["result"]["status"], "failed", "{json}");
        assert_eq!(json[0]["name"], "auth", "{json}");
    }
}
