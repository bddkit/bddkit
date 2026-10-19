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

#[cfg(target_os = "linux")]
#[test]
fn a_sigint_during_embedding_exits_130_and_leaves_nothing_beside_the_target() {
    let dir = scratch("sigint");
    std::fs::create_dir_all(dir.join("art")).expect("art");
    let names: Vec<String> = (0..8).map(|i| format!("{i}.png")).collect();
    for name in &names {
        std::fs::write(dir.join("art").join(name), blocks(2000, 1500)).expect("png");
    }
    let stream = stream_with_shots(&dir, &names.iter().map(String::as_str).collect::<Vec<_>>());
    let mut child = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "report",
            stream.to_str().expect("utf8"),
            "--embed-artifacts",
            "--embed-limit",
            "1GB",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawns");
    // Signalled only once the handler is installed (SIGINT is bit 1 of SigCgt),
    // before that the default action would kill it with no exit code.
    let caught = |pid: u32| {
        std::fs::read_to_string(format!("/proc/{pid}/status"))
            .ok()
            .and_then(|s| {
                s.lines()
                    .find_map(|l| l.strip_prefix("SigCgt:").map(|v| v.trim().to_owned()))
            })
            .and_then(|v| u64::from_str_radix(&v, 16).ok())
            .is_some_and(|mask| mask & 0b10 != 0)
    };
    let started = std::time::Instant::now();
    while !caught(child.id()) {
        assert!(
            started.elapsed().as_secs() < 20,
            "the handler never came up"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    // Past reading the stream, into the encoders. Timing-based: the encoding of
    // these files takes seconds in a debug build, so there is ample margin.
    std::thread::sleep(std::time::Duration::from_millis(300));
    Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .expect("kill");
    let status = child.wait().expect("waits");
    assert_eq!(status.code(), Some(130));
    assert!(!dir.join("run.html").exists());
    let stray: Vec<_> = std::fs::read_dir(&dir)
        .expect("dir")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".tmp"))
        .collect();
    assert!(stray.is_empty(), "{stray:?}");
}

/// One failed scenario and one passed, in one file.
const TWO_SCENARIOS: &str = concat!(
    r#"{"type":"run_started","run_id":"r1","bddkit":"0.3.1","files":1}"#,
    "\n",
    r#"{"type":"file_started","file":"f.feature","name":"F","tags":[]}"#,
    "\n",
    r#"{"type":"scenario_started","file":"f.feature","scenario":0,"name":"good","line":3,"tags":[]}"#,
    "\n",
    r#"{"type":"scenario_finished","file":"f.feature","scenario":0,"status":"passed","duration_us":5}"#,
    "\n",
    r#"{"type":"scenario_started","file":"f.feature","scenario":1,"name":"bad","line":7,"tags":[]}"#,
    "\n",
    r#"{"type":"scenario_finished","file":"f.feature","scenario":1,"status":"failed","duration_us":5,"failure":"boom"}"#,
    "\n",
    r#"{"type":"run_finished","exit":1,"files":1,"scenarios":2,"failed":1,"duration_ms":1}"#,
    "\n",
);

#[test]
fn the_format_follows_the_extension() {
    let dir = scratch("md");
    std::fs::write(dir.join("run.ndjson"), TWO_SCENARIOS).expect("stream");
    let s = dir.join("run.ndjson");
    let (md, html, markdown) = (dir.join("r.MD"), dir.join("r.html"), dir.join("r.markdown"));
    let out = report(&[
        &s,
        "-o".as_ref(),
        &md,
        "-o".as_ref(),
        &html,
        "-o".as_ref(),
        &markdown,
    ]);
    assert_eq!(out.status.code(), Some(0));
    let text = std::fs::read_to_string(&md).expect("md");
    assert!(text.starts_with("---\nbddkit_report: 1\n"));
    assert!(text.contains("### [failed] bad"));
    assert!(!text.contains("### [passed]"), "failures only by default");
    assert_eq!(
        text,
        std::fs::read_to_string(&markdown)
            .expect("markdown")
            .replace("r.markdown", "r.MD")
    );
    assert!(
        std::fs::read_to_string(&html)
            .expect("html")
            .contains("Content-Security-Policy")
    );
}

#[test]
fn detail_all_shows_the_passed_scenarios_too() {
    let dir = scratch("md-all");
    std::fs::write(dir.join("run.ndjson"), TWO_SCENARIOS).expect("stream");
    let md = dir.join("r.md");
    let out = report(&[
        &dir.join("run.ndjson"),
        "-o".as_ref(),
        &md,
        "--detail".as_ref(),
        "all".as_ref(),
    ]);
    assert_eq!(out.status.code(), Some(0));
    assert!(
        std::fs::read_to_string(&md)
            .expect("md")
            .contains("### [passed] good")
    );
}

#[test]
fn detail_without_a_markdown_output_is_refused() {
    let dir = scratch("md-refuse");
    std::fs::write(dir.join("run.ndjson"), TWO_SCENARIOS).expect("stream");
    let s = dir.join("run.ndjson");
    for extra in [
        &["--detail", "all"][..],
        &["--detail", "all", "-o", "x.html"],
        &["--detail", "everything", "-o", "x.md"],
    ] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_bddkit"));
        command.current_dir(&dir).arg("report").arg(&s).args(extra);
        let out = command.output().expect("runs");
        assert_eq!(out.status.code(), Some(2), "{extra:?}");
        assert!(
            !dir.join("run.html").exists() && !dir.join("x.md").exists(),
            "{extra:?}"
        );
    }
}

#[test]
fn embedding_without_an_html_output_is_refused_with_nothing_written() {
    let dir = scratch("md-embed");
    std::fs::write(dir.join("run.ndjson"), TWO_SCENARIOS).expect("stream");
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .current_dir(&dir)
        .args(["report", "run.ndjson", "--embed-artifacts", "-o", "x.md"])
        .output()
        .expect("runs");
    assert_eq!(out.status.code(), Some(2));
    assert!(!dir.join("x.md").exists());
}
