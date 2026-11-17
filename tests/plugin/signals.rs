use super::worker_project;
use std::process::Command;

/// Runs one file that holds its plugin instance for 5 seconds, sends it
/// `signals` in order (each `kill -<name>`, a short pause between them) and
/// returns the process output with the number of drop records the worker
/// fixture wrote. The file is deliberately never allowed to reach its own end,
/// so the only thing that can have written a drop record is the interrupt
/// handler's cleanup sweep.
#[cfg(unix)]
fn interrupted_run(
    name: &str,
    signals: &[&str],
    drop_delay_ms: u64,
) -> (std::process::Output, usize) {
    const HOLD_SECS: u64 = 5;

    let log = std::env::temp_dir().join(format!("bddkit-{name}-drops-{}", std::process::id()));
    let _ = std::fs::remove_file(&log);
    let dir = worker_project(
        name,
        &[(
            "a.feature",
            &format!(
                "Feature: a\n  Scenario: s\n    When I count in the worker as \"n\"\n    And I sleep \"{HOLD_SECS}\" seconds\n"
            ),
        )],
        &format!(
            "  worker:\n    main:\n      drop_log: \"{}\"\n      drop_delay_ms: {drop_delay_ms}\n",
            log.display()
        ),
        1,
    );

    let mut child = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config", "cfg.yaml"])
        .current_dir(&dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn bddkit");

    // Comfortably inside the 5-second sleep: the plugin step dispatches in a
    // fraction of that, so this margin only has to outlast process startup
    // (config load, plugin dylib load, worker spawn) before the sleep begins.
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(
        child.try_wait().expect("poll bddkit").is_none(),
        "the run ended before it could be interrupted"
    );

    for (i, signal) in signals.iter().enumerate() {
        if i > 0 {
            // Long enough for the first signal's handler to be inside the
            // cleanup, well short of the fixture's drop delay.
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
        let status = Command::new("kill")
            .args([&format!("-{signal}"), &child.id().to_string()])
            .status()
            .expect("failed to send a signal");
        assert!(status.success(), "kill -{signal} itself failed");
    }

    let out = child.wait_with_output().expect("collect bddkit output");
    let recorded = std::fs::read_to_string(&log).map_or(0, |text| text.lines().count());
    let _ = std::fs::remove_file(&log);
    (out, recorded)
}

/// Issue #62 and #85: a stop signal mid-run must stop promptly with 128 plus
/// the signal number (not the OS's default "killed by signal" disposition)
/// and still run `Plugins::shutdown` on the way out.
#[cfg(unix)]
fn assert_interrupt_drops_instances(name: &str, signal: &str, code: i32) {
    let (out, recorded) = interrupted_run(name, &[signal], 0);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert_eq!(
        out.status.code(),
        Some(code),
        "--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(
        stderr.contains(&format!("interrupted by SIG{signal}")),
        "{stderr}"
    );
    assert_eq!(
        recorded, 1,
        "Plugins::shutdown must drop the in-flight instance on interrupt\n--- stderr ---\n{stderr}"
    );
}

#[test]
#[cfg(unix)]
fn sigint_stops_new_work_and_still_drops_plugin_instances() {
    assert_interrupt_drops_instances("sigint", "INT", 130);
}

#[test]
#[cfg(unix)]
fn sigterm_exits_143_and_still_drops_plugin_instances() {
    assert_interrupt_drops_instances("sigterm", "TERM", 143);
}

#[test]
#[cfg(unix)]
fn sighup_exits_129_and_still_drops_plugin_instances() {
    assert_interrupt_drops_instances("sighup", "HUP", 129);
}

/// A second signal skips the cleanup — the fixture's 10-second drop delay
/// would otherwise outlast the whole test — and the exit code stays the
/// FIRST signal's: SIGTERM ended the run, the SIGINT only hurried it.
#[test]
#[cfg(unix)]
fn a_second_signal_exits_with_the_code_of_the_first() {
    let (out, recorded) = interrupted_run("second-signal", &["TERM", "INT"], 10_000);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert_eq!(out.status.code(), Some(143), "{stderr}");
    assert!(stderr.contains("second interrupt (SIGINT)"), "{stderr}");
    assert_eq!(recorded, 0, "the cleanup must not have finished\n{stderr}");
}
