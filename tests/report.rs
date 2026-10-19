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

fn embedded_json(html: &str) -> serde_json::Value {
    let open = "<script type=\"application/json\" id=\"run\">";
    let start = html.find(open).expect("element") + open.len();
    let end = start + html[start..].find("</script>").expect("closed");
    serde_json::from_str(&html[start..end]).expect("valid JSON")
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

#[test]
fn files_are_embedded_as_they_are_marked_or_left_out() {
    let dir = scratch("plain");
    std::fs::create_dir_all(dir.join("art")).expect("art");
    std::fs::write(dir.join("art/a.txt"), b"hello").expect("file");
    let stream = stream_with_shots(&dir, &["a.txt", "gone.txt", "../x"]);
    let s = stream.to_str().expect("utf8");
    let out = run_report(&[s, "--embed-artifacts"]);
    assert_eq!(out.status.code(), Some(0));
    let page = embedded_json(&std::fs::read_to_string(dir.join("run.html")).expect("html"));
    assert_eq!(page["embedded"]["a.txt"]["encoding"], "original");
    assert_eq!(page["embedded"]["a.txt"]["data"], "aGVsbG8=");
    assert_eq!(page["not_embedded"]["gone.txt"]["reason"], "not found");
    assert_eq!(page["not_embedded"]["../x"]["reason"], "unsafe path");
    run_report(&[
        s,
        "--embed-artifacts",
        "--embed-limit",
        "0",
        "-o",
        dir.join("z.html").to_str().expect("utf8"),
    ]);
    let page = embedded_json(&std::fs::read_to_string(dir.join("z.html")).expect("html"));
    assert!(page.get("embedded").is_none());
    assert_eq!(page["not_embedded"]["a.txt"]["reason"], "limit");
}

fn blocks(w: u32, h: u32) -> Vec<u8> {
    let mut data = Vec::new();
    for y in 0..h as usize {
        for x in 0..w as usize {
            data.extend_from_slice(&[(x / 8 % 256) as u8, (y / 8 % 256) as u8, 40, 255]);
        }
    }
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .expect("header")
        .write_image_data(&data)
        .expect("data");
    out
}

#[test]
fn embedding_carries_the_images_and_leaves_the_files_and_exit_code_alone() {
    let dir = scratch("embed");
    std::fs::create_dir_all(dir.join("art/1")).expect("art");
    let big = blocks(400, 300);
    std::fs::write(dir.join("art/1/big.png"), &big).expect("png");
    std::fs::write(dir.join("art/1/small.txt"), b"hi").expect("txt");
    let stream = stream_with_shots(
        &dir,
        &[
            "1/big.png",
            "1/small.txt",
            "1/gone.png",
            "../x",
            "1/big.png",
        ],
    );
    let s = stream.to_str().expect("utf8");

    let out = run_report(&[s, "--embed-artifacts"]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("2 files embedded (lossless,") && stdout.contains("2 left as links"),
        "{stdout}"
    );
    assert!(out.stderr.is_empty(), "no terminal, nothing drawn");
    let page = embedded_json(&std::fs::read_to_string(dir.join("run.html")).expect("html"));
    assert_eq!(page["embedded"]["1/big.png"]["encoding"], "webp-lossless");
    assert_eq!(page["embedded"]["1/small.txt"]["encoding"], "original");
    assert_eq!(page["not_embedded"]["1/gone.png"]["reason"], "not found");
    assert_eq!(page["not_embedded"]["../x"]["reason"], "unsafe path");
    assert_eq!(std::fs::read(dir.join("art/1/big.png")).expect("png"), big);

    // `original` carries the exact bytes; a limit of 0 carries nothing.
    run_report(&[
        s,
        "--embed-artifacts",
        "--embed-images",
        "original",
        "-o",
        dir.join("o.html").to_str().expect("utf8"),
    ]);
    let page = embedded_json(&std::fs::read_to_string(dir.join("o.html")).expect("html"));
    assert_eq!(page["embedded"]["1/big.png"]["encoding"], "original");
    run_report(&[
        s,
        "--embed-artifacts",
        "--embed-limit",
        "0",
        "-o",
        dir.join("z.html").to_str().expect("utf8"),
    ]);
    let page = embedded_json(&std::fs::read_to_string(dir.join("z.html")).expect("html"));
    assert!(page.get("embedded").is_none());
    assert_eq!(page["not_embedded"]["1/big.png"]["reason"], "limit");
    assert_eq!(std::fs::read(dir.join("art/1/big.png")).expect("png"), big);
}

#[test]
fn the_same_stream_and_options_give_the_same_bytes_and_no_flag_adds_nothing() {
    let dir = scratch("same");
    std::fs::create_dir_all(dir.join("art")).expect("art");
    std::fs::write(dir.join("art/a.png"), blocks(300, 200)).expect("png");
    let stream = stream_with_shots(&dir, &["a.png"]);
    let s = stream.to_str().expect("utf8");
    let (a, b) = (dir.join("a.html"), dir.join("b.html"));
    for out in [&a, &b] {
        run_report(&[
            s,
            "--embed-artifacts",
            "--embed-images",
            "lossy",
            "-o",
            out.to_str().expect("utf8"),
        ]);
    }
    assert_eq!(std::fs::read(&a).expect("a"), std::fs::read(&b).expect("b"));
    let plain = run_report(&[s]);
    let stdout = String::from_utf8_lossy(&plain.stdout);
    assert!(
        stdout.contains(" scenarios") && !stdout.contains("embedded"),
        "{stdout}"
    );
    let page = embedded_json(&std::fs::read_to_string(dir.join("run.html")).expect("html"));
    assert!(page.get("embedded").is_none() && page.get("not_embedded").is_none());
}
