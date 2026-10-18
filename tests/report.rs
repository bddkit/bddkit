//! `bddkit report` as a user runs it: exit codes and what lands on disk.

use std::process::Command;

const STREAM: &str = concat!(
    r#"{"type":"run_started","run_id":"r1","bddkit":"0.3.1"}"#,
    "\n",
    r#"{"type":"run_finished","exit":0,"files":0,"scenarios":0,"failed":0,"duration_ms":1}"#,
    "\n",
);

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("bddkit-report-it-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn report(args: &[&std::path::Path]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .arg("report")
        .args(args)
        .output()
        .expect("runs")
}

#[test]
fn a_stream_is_rendered_beside_itself_with_exit_0() {
    let dir = scratch("ok");
    std::fs::write(dir.join("run.ndjson"), STREAM).expect("stream");
    let out = report(&[&dir.join("run.ndjson")]);
    assert_eq!(out.status.code(), Some(0));
    let html = std::fs::read_to_string(dir.join("run.html")).expect("report");
    assert!(html.contains("Content-Security-Policy"));
}

#[test]
fn a_file_that_is_not_a_stream_exits_2_and_writes_nothing() {
    let dir = scratch("bad");
    std::fs::write(dir.join("run.ndjson"), "not a stream\n").expect("file");
    let out = report(&[&dir.join("run.ndjson")]);
    assert_eq!(out.status.code(), Some(2));
    assert!(!dir.join("run.html").exists());
}

#[test]
fn an_unknown_extension_exits_2() {
    let dir = scratch("ext");
    std::fs::write(dir.join("run.ndjson"), STREAM).expect("stream");
    let out = report(&[
        &dir.join("run.ndjson"),
        std::path::Path::new("-o"),
        &dir.join("out.txt"),
    ]);
    assert_eq!(out.status.code(), Some(2));
}
