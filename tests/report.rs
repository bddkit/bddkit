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

// ---- --embed-artifacts ------------------------------------------------------

/// A run that names `names` as screenshot evidence under `<dir>/art`.
/// A run that names `names` as screenshot evidence under `<dir>/art`.
fn stream_with_shots(dir: &std::path::Path, names: &[&str]) -> std::path::PathBuf {
    let mut text = format!(
        "{{\"type\":\"run_started\",\"run_id\":\"r1\",\"bddkit\":\"0.3.2\",\"artifacts_dir\":{:?}}}\n\
         {{\"type\":\"file_started\",\"file\":\"a.feature\",\"name\":\"A\",\"tags\":[]}}\n\
         {{\"type\":\"scenario_started\",\"file\":\"a.feature\",\"scenario\":0,\"name\":\"s\",\"line\":1,\"tags\":[]}}\n\
         {{\"type\":\"step_started\",\"file\":\"a.feature\",\"scenario\":0,\"step\":0,\"keyword\":\"Given \",\"text\":\"x\",\"line\":2}}\n",
        dir.join("art").to_string_lossy()
    );
    for name in names {
        text.push_str(&format!(
            "{{\"type\":\"evidence\",\"file\":\"a.feature\",\"scenario\":0,\"step\":0,\"kind\":\"screenshot\",\"path\":{name:?},\"size\":1,\"media_type\":\"image/png\"}}\n"
        ));
    }
    text.push_str(
        r#"{"type":"run_finished","exit":0,"files":1,"scenarios":1,"failed":0,"duration_ms":1}"#,
    );
    text.push('\n');
    let path = dir.join("run.ndjson");
    std::fs::write(&path, text).expect("stream");
    path
}

fn run_report(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .arg("report")
        .args(args)
        .output()
        .expect("runs")
}

#[test]
fn the_embedding_options_need_the_flag_and_a_quality_needs_lossy() {
    let dir = scratch("refuse");
    let stream = stream_with_shots(&dir, &[]);
    let s = stream.to_str().expect("utf8");
    for extra in [
        &["--embed-limit", "1MB"][..],
        &["--embed-images", "lossy"],
        &["--embed-quality", "70"],
        &["--embed-artifacts", "--embed-quality", "70"],
        &[
            "--embed-artifacts",
            "--embed-images",
            "original",
            "--embed-quality",
            "70",
        ],
        &["--embed-artifacts", "--embed-quality", "0"],
        &["--embed-artifacts", "--embed-quality", "101"],
        &["--embed-artifacts", "--embed-limit", "1.5MB"],
        &["--embed-artifacts", "--embed-limit", "20MiB"],
    ] {
        let mut args = vec![s];
        args.extend_from_slice(extra);
        let out = run_report(&args);
        assert_eq!(out.status.code(), Some(2), "{extra:?}");
        assert!(!dir.join("run.html").exists(), "{extra:?}");
    }
}
