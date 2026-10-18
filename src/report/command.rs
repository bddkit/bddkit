//! `bddkit report`: a stream on disk rendered to one or more files.

use super::{html, model};
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Eq)]
enum Format {
    Html,
}

/// The format comes from the extension, compared without case, and from
/// nothing else, so a typo never silently picks one.
fn format_of(path: &Path) -> Result<Format> {
    let extension = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase());
    match extension.as_deref() {
        Some("html" | "htm") => Ok(Format::Html),
        Some("md" | "markdown") => bail!(
            "{}: the Markdown report is not implemented yet; use an .html output",
            path.display()
        ),
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

fn temporary_name(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    target.with_file_name(format!(".{name}.bddkit-{}.tmp", std::process::id()))
}

/// Renders `events` to every output, or to none: each is written beside its
/// target under a temporary name and renamed only once all are complete.
/// Returns the exit code (0), every failure being an error for `main` to turn
/// into exit 2.
pub fn run(events: &Path, outputs: &[PathBuf], artifacts: Option<&Path>) -> Result<i32> {
    let defaulted = outputs.is_empty();
    let outputs = if defaulted {
        vec![default_output(events)]
    } else {
        outputs.to_vec()
    };

    // All of this before the stream is read, so a refusal writes nothing.
    let mut seen: Vec<PathBuf> = Vec::new();
    for output in &outputs {
        format_of(output)?;
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

    let file =
        std::fs::File::open(events).with_context(|| format!("cannot read {}", events.display()))?;
    let run = model::build(std::io::BufReader::new(file))
        .with_context(|| format!("{} is not a usable event stream", events.display()))?;

    let root = artifacts
        .map(Path::to_path_buf)
        .or_else(|| run.artifacts_dir.as_deref().map(PathBuf::from));
    let temporary = artifacts.is_none()
        && run
            .artifacts_dir
            .as_deref()
            .is_some_and(|dir| html::is_temporary_root(dir, &run.run_id));

    let mut written: Vec<(PathBuf, PathBuf)> = Vec::new();
    let result = outputs.iter().try_for_each(|output| {
        let dir = output
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let base = root
            .as_deref()
            .map_or_else(String::new, |root| html::artifacts_base(root, dir));
        let page = html::render(&run, &base, temporary);
        let temp = temporary_name(output);
        written.push((temp.clone(), output.clone()));
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
        written.iter().try_for_each(|(temp, target)| {
            std::fs::rename(temp, target)
                .with_context(|| format!("cannot write {}", target.display()))
        })
    });
    if result.is_err() {
        for (temp, _) in &written {
            let _ = std::fs::remove_file(temp);
        }
    }
    result?;

    let scenarios: usize = run.files.iter().map(|f| f.scenarios.len()).sum();
    for output in &outputs {
        println!("{}: {scenarios} scenarios", output.display());
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
        assert_eq!(run(&d.join("run.ndjson"), &[], None).expect("ok"), 0);
        assert!(d.join("run.html").is_file());
    }

    #[test]
    fn an_unknown_or_markdown_extension_is_refused_and_nothing_is_written() {
        let d = dir();
        std::fs::write(d.join("run.ndjson"), STREAM).expect("stream");
        for bad in ["out.txt", "out", "out.md"] {
            let outputs = [d.join("ok.html"), d.join(bad)];
            assert!(run(&d.join("run.ndjson"), &outputs, None).is_err(), "{bad}");
        }
        assert!(!d.join("ok.html").exists());
    }

    #[test]
    fn a_repeated_output_is_refused() {
        let d = dir();
        std::fs::write(d.join("run.ndjson"), STREAM).expect("stream");
        let outputs = [d.join("a.html"), d.join("./a.html")];
        assert!(run(&d.join("run.ndjson"), &outputs, None).is_err());
    }

    #[test]
    fn a_non_stream_is_refused_and_an_existing_output_is_left_alone() {
        let d = dir();
        std::fs::write(d.join("bad.ndjson"), "hello\n").expect("file");
        std::fs::write(d.join("out.html"), "old").expect("old");
        assert!(run(&d.join("bad.ndjson"), &[d.join("out.html")], None).is_err());
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
        assert!(run(&d.join("run.ndjson"), &outputs, None).is_err());
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
        assert_eq!(run(&d.join("run.ndjson"), &[], None).expect("ok"), 0);
        assert!(d.join("run.html").is_file());
    }

    #[test]
    fn two_outputs_come_from_one_stream() {
        let d = dir();
        std::fs::write(d.join("run.ndjson"), STREAM).expect("stream");
        let outputs = [d.join("a.html"), d.join("b.HTM")];
        run(&d.join("run.ndjson"), &outputs, None).expect("ok");
        assert!(d.join("a.html").is_file() && d.join("b.HTM").is_file());
    }
}
