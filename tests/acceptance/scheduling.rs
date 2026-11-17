use super::write_parallel_fixture;
use std::process::Command;

/// Runs the fixture from its own directory, where the plugin lock lives.
fn run_parallel_fixture(dir: &std::path::Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config", "cfg.yaml", "--bddkit-dir"])
        .arg(dir.join(".bddkit"))
        .current_dir(dir)
        .output()
        .expect("failed to run bddkit")
}

/// Gate M5: two files must run at the same time, or the barrier never opens.
#[test]
fn two_feature_files_run_at_the_same_time() {
    let dir = std::env::temp_dir().join(format!("bddkit-parallel-{}", std::process::id()));
    write_parallel_fixture(&dir, 2, "");
    let out = run_parallel_fixture(&dir);

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "both files must run in parallel\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(stdout.contains("failed: 0"), "{stdout}");
    assert!(stdout.contains("files: 2"), "{stdout}");
}

/// Negative control: with `concurrency: 1` the barrier never opens and the
/// run fails. Without this test, the first one could pass green for any
/// reason other than real parallelism.
#[test]
fn a_single_worker_cannot_release_the_barrier() {
    let dir = std::env::temp_dir().join(format!("bddkit-sequential-{}", std::process::id()));
    write_parallel_fixture(&dir, 1, "");
    let out = run_parallel_fixture(&dir);

    assert_eq!(
        out.status.code(),
        Some(1),
        "a single worker must hit the barrier timeout, not pass"
    );
}

/// Files in one chain must never be in flight at the same time — the
/// two-party barrier never opens, and the run must fail on timeout. This
/// mirrors the parallelism test: the same pair of files, just tagged.
#[test]
fn two_files_in_one_serial_chain_never_run_together() {
    let dir = std::env::temp_dir().join(format!("bddkit-serial-{}", std::process::id()));
    write_parallel_fixture(&dir, 2, "@serial(shared)\n");
    let out = run_parallel_fixture(&dir);

    assert_eq!(
        out.status.code(),
        Some(1),
        "the chain must serialize the files, the two-party barrier must not open\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// A broken scheduling tag — rejected before the first request, code 2.
#[test]
fn a_file_in_two_chains_fails_the_startup() {
    let dir = std::env::temp_dir().join(format!("bddkit-two-chains-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/bad.feature"),
        "@serial(a)\nFeature: f\n  @serial(b)\n  Scenario: s\n    Given set variable \"x\" to \"1\"\n",
    )
    .expect("write feature");
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "paths: [{}]\nresources:\n  api: {{}}\n",
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("write config");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "run",
            "--config",
            dir.join("cfg.yaml").to_str().expect("path is UTF-8"),
        ])
        .output()
        .expect("failed to run bddkit");

    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("two chains"), "{stderr}");
}

/// The order files print in with one worker is the queue order.
/// Checking that `@priority` sets it: higher goes earlier.
#[test]
fn higher_priority_files_run_first() {
    let dir = std::env::temp_dir().join(format!("bddkit-priority-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    // Names are deliberately alphabetical in the reverse of the desired order:
    // without the tag the queue would be low → mid → high.
    for (name, tag) in [
        ("a_low", "@priority(-1)\n"),
        ("b_mid", ""),
        ("c_high", "@priority(5)\n"),
    ] {
        std::fs::write(
            dir.join(format!("features/{name}.feature")),
            format!("{tag}Feature: {name}\n  Scenario: s\n    Given set variable \"x\" to \"1\"\n"),
        )
        .expect("write feature");
    }
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "concurrency: 1\npaths: [{}]\nresources:\n  api: {{}}\n",
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("write config");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "run",
            "--config",
            dir.join("cfg.yaml").to_str().expect("path is UTF-8"),
        ])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let high = stdout.find("c_high").expect("file c_high in the output");
    let mid = stdout.find("b_mid").expect("file b_mid in the output");
    let low = stdout.find("a_low").expect("file a_low in the output");
    assert!(
        high < mid && mid < low,
        "queue order by priority:\n{stdout}"
    );
}

/// A non-numeric priority — rejected before the first request, code 2.
#[test]
fn a_non_numeric_priority_fails_the_startup() {
    let dir = std::env::temp_dir().join(format!("bddkit-bad-priority-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/bad.feature"),
        "@priority(urgent)\nFeature: f\n  Scenario: s\n    Given set variable \"x\" to \"1\"\n",
    )
    .expect("write feature");
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "paths: [{}]\nresources:\n  api: {{}}\n",
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("write config");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "run",
            "--config",
            dir.join("cfg.yaml").to_str().expect("path is UTF-8"),
        ])
        .output()
        .expect("failed to run bddkit");

    assert_eq!(out.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("an integer"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `--fail-fast` stops dispatching NEW work after the first failure.
/// With one worker this is deterministic: the first file fails, the second
/// and third never start at all.
#[test]
fn fail_fast_stops_starting_new_files() {
    let dir = std::env::temp_dir().join(format!("bddkit-fail-fast-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/a_broken.feature"),
        "Feature: broken\n  Scenario: s\n    \
         Given set variable \"x\" to \"1\"\n    Then variable \"x\" should be equal to \"2\"\n",
    )
    .expect("write feature");
    for name in ["b_ok", "c_ok"] {
        std::fs::write(
            dir.join(format!("features/{name}.feature")),
            format!("Feature: {name}\n  Scenario: s\n    Given set variable \"x\" to \"1\"\n"),
        )
        .expect("write feature");
    }
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "concurrency: 1\npaths: [{}]\nresources:\n  api: {{}}\n",
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("write config");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "run",
            "--config",
            dir.join("cfg.yaml").to_str().expect("path is UTF-8"),
            "--fail-fast",
        ])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(
        stdout.contains("a_broken"),
        "the failed file must appear in the report:\n{stdout}"
    );
    assert!(
        !stdout.contains("c_ok"),
        "no new files must start after the failure:\n{stdout}"
    );
    assert!(stdout.contains("files: 1"), "{stdout}");
}

/// `--fail-fast` also stops dispatching new work WITHIN a chain already
/// picked up: three files sharing one `@serial` name run strictly in order
/// on one worker, the first fails, the second and third never start. This is
/// a separate check from `fail_fast_stops_starting_new_files`, which only
/// exercises the "before the next chain" check — there each file was its own chain.
#[test]
fn fail_fast_stops_a_chain_partway_through() {
    let dir = std::env::temp_dir().join(format!("bddkit-fail-fast-chain-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/a_broken.feature"),
        "@serial(chain)\nFeature: broken\n  Scenario: s\n    \
         Given set variable \"x\" to \"1\"\n    Then variable \"x\" should be equal to \"2\"\n",
    )
    .expect("write feature");
    for name in ["b_ok", "c_ok"] {
        std::fs::write(
            dir.join(format!("features/{name}.feature")),
            format!(
                "@serial(chain)\nFeature: {name}\n  Scenario: s\n    Given set variable \"x\" to \"1\"\n"
            ),
        )
        .expect("write feature");
    }
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "concurrency: 1\npaths: [{}]\nresources:\n  api: {{}}\n",
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("write config");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "run",
            "--config",
            dir.join("cfg.yaml").to_str().expect("path is UTF-8"),
            "--fail-fast",
        ])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(
        stdout.contains("a_broken"),
        "the failed file must appear in the report:\n{stdout}"
    );
    assert!(
        !stdout.contains("b_ok") && !stdout.contains("c_ok"),
        "the rest of the chain must not start after the failure:\n{stdout}"
    );
    assert!(stdout.contains("files: 1"), "{stdout}");
}

/// Without the flag, one file's failure does not block the rest: the run
/// must reach the end and show all failures at once.
#[test]
fn without_fail_fast_every_file_still_runs() {
    let dir = std::env::temp_dir().join(format!("bddkit-no-fail-fast-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/a_broken.feature"),
        "Feature: broken\n  Scenario: s\n    \
         Given set variable \"x\" to \"1\"\n    Then variable \"x\" should be equal to \"2\"\n",
    )
    .expect("write feature");
    std::fs::write(
        dir.join("features/c_ok.feature"),
        "Feature: whole\n  Scenario: s\n    Given set variable \"x\" to \"1\"\n",
    )
    .expect("write feature");
    std::fs::write(
        dir.join("cfg.yaml"),
        format!(
            "concurrency: 1\npaths: [{}]\nresources:\n  api: {{}}\n",
            dir.join("features")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("write config");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "run",
            "--config",
            dir.join("cfg.yaml").to_str().expect("path is UTF-8"),
        ])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("files: 2"), "{stdout}");
}
