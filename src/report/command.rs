//! `bddkit report`: a stream on disk rendered to one or more files.

use super::markdown::{self, Detail};
use super::progress::{Counting, Progress};
use super::{embed, html, model};
use crate::telemetry::payload::ReportInfo;
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Eq)]
enum Format {
    Html,
    Markdown,
}

/// The format comes from the extension, compared without case, and from
/// nothing else, so a typo never silently picks one.
fn format_of(path: &Path) -> Result<Format> {
    let extension = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase());
    match extension.as_deref() {
        Some("html" | "htm") => Ok(Format::Html),
        Some("md" | "markdown") => Ok(Format::Markdown),
        _ => bail!(
            "{}: cannot tell the format from the extension; use .html or .htm (HTML) or .md or .markdown (Markdown)",
            path.display()
        ),
    }
}

/// `run.ndjson` gives `run.html` beside it; a name with no extension gets one.
fn default_output(events: &Path) -> PathBuf {
    events.with_extension("html")
}

/// Directory and name, so two spellings of one file compare equal.
fn identity(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let name = absolute.file_name().map(PathBuf::from).unwrap_or_default();
    let dir = absolute.parent().map(Path::to_path_buf).unwrap_or_default();
    dir.canonicalize().unwrap_or(dir).join(name)
}

pub(crate) fn temporary_name(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    target.with_file_name(format!(".{name}.bddkit-{}.tmp", std::process::id()))
}

/// The outputs `run` writes: the ones named, or the default beside the stream.
pub fn outputs_for(events: &Path, outputs: &[PathBuf]) -> Vec<PathBuf> {
    if outputs.is_empty() {
        vec![default_output(events)]
    } else {
        outputs.to_vec()
    }
}

static CANCELLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Called by the interrupt handler before it cleans up: the render thread is
/// still running, and stops writing and renaming once it sees this.
pub fn cancel() {
    CANCELLED.store(true, std::sync::atomic::Ordering::SeqCst);
}

fn check_cancelled() -> Result<()> {
    if CANCELLED.load(std::sync::atomic::Ordering::SeqCst) {
        bail!("interrupted");
    }
    Ok(())
}

/// What an interrupt leaves behind: the names depend only on the output and
/// the pid, so the signal handler needs no state from the run.
pub fn remove_temporaries(outputs: &[PathBuf]) {
    for output in outputs {
        let _ = std::fs::remove_file(temporary_name(output));
    }
}

/// The format of each output, by its extension; one that is neither is not a
/// format and not listed.
pub fn format_names(outputs: &[PathBuf]) -> Vec<&'static str> {
    let mut names: Vec<&'static str> = outputs
        .iter()
        .filter_map(|output| match format_of(output) {
            Ok(Format::Html) => Some("html"),
            Ok(Format::Markdown) => Some("md"),
            Err(_) => None,
        })
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

/// Renders `events` to every output, or to none: each is written beside its
/// target under a temporary name and renamed only once all are complete.
/// Returns the exit code (0), every failure being an error for `main` to turn
/// into exit 2.
pub fn run(
    events: &Path,
    outputs: &[PathBuf],
    artifacts: Option<&Path>,
    embed: Option<&embed::Options>,
    detail: Option<Detail>,
    progress: &Progress,
    info: &mut ReportInfo,
) -> Result<i32> {
    let defaulted = outputs.is_empty();
    let outputs = outputs_for(events, outputs);

    // All of this before the stream is read, so a refusal writes nothing.
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut formats = Vec::new();
    for output in &outputs {
        formats.push(format_of(output)?);
        let id = identity(output);
        if id == identity(events) {
            bail!(
                "{}: the output would overwrite the stream it is made from",
                output.display()
            );
        }
        if seen.contains(&id) {
            bail!("{}: named by more than one -o", output.display());
        }
        seen.push(id);
    }
    // An option no requested format reads is a mistake, not something to ignore.
    if detail.is_some() && !formats.contains(&Format::Markdown) {
        bail!("--detail applies to a Markdown output: name one with -o report.md");
    }
    if embed.is_some() && !formats.contains(&Format::Html) {
        bail!("--embed-artifacts applies to an HTML output: name one with -o report.html");
    }

    let file =
        std::fs::File::open(events).with_context(|| format!("cannot read {}", events.display()))?;
    progress.stage(
        "reading the stream",
        file.metadata().map_or(0, |m| m.len()),
        true,
    );
    let reader = Counting {
        inner: file,
        progress,
    };
    let run = model::build(std::io::BufReader::new(reader))
        .with_context(|| format!("{} is not a usable event stream", events.display()))?;
    // Read before anything is written, so a render that fails still reports them.
    info.events = run.events;
    info.evidence = run.evidence;
    info.complete = !run.incomplete;

    let root = artifacts
        .map(Path::to_path_buf)
        .or_else(|| run.artifacts_dir.as_deref().map(PathBuf::from));
    let temporary = artifacts.is_none()
        && run
            .artifacts_dir
            .as_deref()
            .is_some_and(|dir| html::is_temporary_root(dir, &run.run_id));

    let embeds = match embed {
        Some(options) => {
            let plan = embed::plan(&run, root.as_deref(), options.limit);
            progress.stage("embedding artifacts", plan.files() as u64, false);
            embed::encode(plan, root.as_deref(), options, embed::threads(), &|| {
                progress.inc(1);
            })
        }
        None => embed::Embeds::default(),
    };

    let mut written: Vec<(PathBuf, PathBuf, usize)> = Vec::new();
    let result = outputs
        .iter()
        .zip(&formats)
        .try_for_each(|(output, format)| {
            let dir = output
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            let base = root
                .as_deref()
                .map_or_else(String::new, |root| html::artifacts_base(root, dir));
            check_cancelled()?;
            let page = match format {
                Format::Html => html::render(&run, &base, temporary, &embeds),
                Format::Markdown => markdown::render(
                    &run,
                    &output.file_name().unwrap_or_default().to_string_lossy(),
                    &base,
                    temporary,
                    detail.unwrap_or(Detail::Failed),
                ),
            };
            let temp = temporary_name(output);
            written.push((temp.clone(), output.clone(), page.len()));
            std::fs::write(&temp, page).map_err(|error| {
                let hint = if defaulted {
                    format!(
                        "; the report goes beside the stream unless -o names a place, e.g. -o ./{}",
                        output.file_name().unwrap_or_default().to_string_lossy()
                    )
                } else {
                    String::new()
                };
                anyhow::anyhow!("cannot write {}: {error}{hint}", output.display())
            })
        });
    let result = result.and_then(|()| {
        written.iter().try_for_each(|(temp, target, _)| {
            check_cancelled()?;
            std::fs::rename(temp, target)
                .with_context(|| format!("cannot write {}", target.display()))
        })
    });
    if result.is_err() {
        for (temp, ..) in &written {
            let _ = std::fs::remove_file(temp);
        }
    }
    result?;

    progress.clear();
    let scenarios: usize = run.files.iter().map(|f| f.scenarios.len()).sum();
    let clause = embed.map_or_else(String::new, |o| embeds.summary(o.images));
    for (_, output, size) in &written {
        let clause = if matches!(format_of(output), Ok(Format::Html)) {
            clause.as_str()
        } else {
            ""
        };
        println!(
            "{}: {}, {scenarios} scenarios{clause}",
            output.display(),
            embed::human(*size as u64)
        );
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const STREAM: &str = concat!(
        r#"{"type":"run_started","run_id":"r1","bddkit":"0.3.1","artifacts_dir":"/x/bddkit-r1"}"#,
        "\n",
        r#"{"type":"run_finished","exit":0,"files":0,"scenarios":0,"failed":0,"duration_ms":1}"#,
        "\n",
    );

    fn quiet() -> Progress {
        Progress::new(false, 1)
    }

    fn dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bddkit-report-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        dir
    }

    #[test]
    fn without_o_the_html_lands_beside_the_stream_under_its_name() {
        let d = dir();
        std::fs::write(d.join("run.ndjson"), STREAM).expect("stream");
        assert_eq!(
            run(
                &d.join("run.ndjson"),
                &[],
                None,
                None,
                None,
                &quiet(),
                &mut ReportInfo::default()
            )
            .expect("ok"),
            0
        );
        assert!(d.join("run.html").is_file());
    }

    #[test]
    fn an_unknown_extension_is_refused_and_nothing_is_written() {
        let d = dir();
        std::fs::write(d.join("run.ndjson"), STREAM).expect("stream");
        for bad in ["out.txt", "out", "out.mdx"] {
            let outputs = [d.join("ok.html"), d.join(bad)];
            assert!(
                run(
                    &d.join("run.ndjson"),
                    &outputs,
                    None,
                    None,
                    None,
                    &quiet(),
                    &mut ReportInfo::default()
                )
                .is_err(),
                "{bad}"
            );
        }
        assert!(!d.join("ok.html").exists());
    }

    #[test]
    fn a_repeated_output_is_refused() {
        let d = dir();
        std::fs::write(d.join("run.ndjson"), STREAM).expect("stream");
        let outputs = [d.join("a.html"), d.join("./a.html")];
        assert!(
            run(
                &d.join("run.ndjson"),
                &outputs,
                None,
                None,
                None,
                &quiet(),
                &mut ReportInfo::default()
            )
            .is_err()
        );
    }

    #[test]
    fn a_non_stream_is_refused_and_an_existing_output_is_left_alone() {
        let d = dir();
        std::fs::write(d.join("bad.ndjson"), "hello\n").expect("file");
        std::fs::write(d.join("out.html"), "old").expect("old");
        assert!(
            run(
                &d.join("bad.ndjson"),
                &[d.join("out.html")],
                None,
                None,
                None,
                &quiet(),
                &mut ReportInfo::default()
            )
            .is_err()
        );
        assert_eq!(
            std::fs::read_to_string(d.join("out.html")).expect("read"),
            "old"
        );
    }

    #[test]
    fn when_one_output_cannot_be_written_none_is() {
        let d = dir();
        std::fs::write(d.join("run.ndjson"), STREAM).expect("stream");
        let outputs = [d.join("a.html"), d.join("missing/b.html")];
        assert!(
            run(
                &d.join("run.ndjson"),
                &outputs,
                None,
                None,
                None,
                &quiet(),
                &mut ReportInfo::default()
            )
            .is_err()
        );
        assert!(!d.join("a.html").exists());
        let leftovers = std::fs::read_dir(&d).expect("dir").count();
        assert_eq!(leftovers, 1, "only the stream is left");
    }

    #[test]
    fn an_incomplete_stream_still_renders() {
        let d = dir();
        let cut = STREAM.lines().next().expect("first line");
        std::fs::write(
            d.join("run.ndjson"),
            format!("{cut}\n{{\"type\":\"file_sta"),
        )
        .expect("stream");
        assert_eq!(
            run(
                &d.join("run.ndjson"),
                &[],
                None,
                None,
                None,
                &quiet(),
                &mut ReportInfo::default()
            )
            .expect("ok"),
            0
        );
        assert!(d.join("run.html").is_file());
    }

    #[test]
    fn two_outputs_come_from_one_stream() {
        let d = dir();
        std::fs::write(d.join("run.ndjson"), STREAM).expect("stream");
        let outputs = [d.join("a.html"), d.join("b.HTM")];
        run(
            &d.join("run.ndjson"),
            &outputs,
            None,
            None,
            None,
            &quiet(),
            &mut ReportInfo::default(),
        )
        .expect("ok");
        assert!(d.join("a.html").is_file() && d.join("b.HTM").is_file());
    }

    #[test]
    fn an_interrupt_removes_the_temporary_beside_each_output_and_only_that() {
        let d = dir();
        let outputs = [d.join("a.html"), d.join("b.html")];
        for o in &outputs {
            std::fs::write(temporary_name(o), "half").expect("temp");
        }
        std::fs::write(d.join("a.html"), "kept").expect("target");
        remove_temporaries(&outputs);
        assert!(!temporary_name(&outputs[0]).exists() && !temporary_name(&outputs[1]).exists());
        assert_eq!(
            std::fs::read_to_string(d.join("a.html")).expect("read"),
            "kept"
        );
    }
}
