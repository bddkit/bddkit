use super::{mkfifo, write_doctor_project};
use std::process::Command;

#[test]
fn doctor_reports_the_events_path() {
    let cfg = write_doctor_project(
        "doctor-events",
        "Feature: f\n  Scenario: s\n    Given set variable \"x\" to \"1\"\n",
        "",
    );
    let events = cfg.with_file_name("out/events.ndjson");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config"])
        .arg(&cfg)
        .arg("--events")
        .arg(&events)
        .output()
        .expect("run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("reports"), "{stdout}");
    assert!(stdout.contains("events.ndjson"), "{stdout}");
    assert_eq!(std::fs::read_to_string(&events).expect("created"), "");

    // A path that cannot be created is a failed row, exit 1.
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config"])
        .arg(&cfg)
        .arg("--events")
        .arg(cfg.join("events.ndjson"))
        .output()
        .expect("run bddkit");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("✗ reports"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// `run` refuses a malformed evidence variable, and `$BDDKIT_EVIDENCE`
/// without a stream; `doctor` must not be the more lenient of the two.
#[test]
fn doctor_fails_on_an_evidence_variable_run_would_refuse() {
    let cfg = write_doctor_project(
        "doctor-evidence",
        "Feature: f\n  Scenario: s\n    Given set variable \"x\" to \"1\"\n",
        "",
    );
    let doctor = |vars: &[(&str, &str)], stream: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_bddkit"));
        command
            .args(["doctor", "--config"])
            .arg(&cfg)
            .env_remove("BDDKIT_EVENTS")
            .env_remove("BDDKIT_EVIDENCE")
            .env_remove("BDDKIT_EVIDENCE_BUFFER")
            .envs(vars.iter().copied());
        if stream {
            command.arg("--events").arg(cfg.with_file_name("e.ndjson"));
        }
        let out = command.output().expect("run bddkit");
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
        )
    };
    for (vars, stream, says) in [
        (&[("BDDKIT_EVIDENCE", "bogus")][..], true, "BDDKIT_EVIDENCE"),
        (
            &[("BDDKIT_EVIDENCE", "failures")][..],
            false,
            "requires --events",
        ),
        (
            &[
                ("BDDKIT_EVIDENCE", "failures"),
                ("BDDKIT_EVIDENCE_BUFFER", "lots"),
            ][..],
            true,
            "BDDKIT_EVIDENCE_BUFFER",
        ),
    ] {
        let (code, stdout) = doctor(vars, stream);
        assert_eq!(code, Some(1), "{vars:?}: {stdout}");
        assert!(
            stdout.contains("✗ evidence") && stdout.contains(says),
            "{stdout}"
        );
    }
    // A buffer nothing would use is not read, as in `run`.
    let (code, stdout) = doctor(&[("BDDKIT_EVIDENCE_BUFFER", "lots")], true);
    assert_eq!(code, Some(0), "{stdout}");
    let (code, stdout) = doctor(&[("BDDKIT_EVIDENCE", "marked,failures")], true);
    assert_eq!(code, Some(0), "{stdout}");
    assert!(
        stdout.contains("marked, failures ($BDDKIT_EVIDENCE)"),
        "{stdout}"
    );
}

/// Opening a FIFO for writing blocks until a reader shows up, and there is
/// none: a doctor that opened it would hang here.
#[cfg(unix)]
#[test]
fn doctor_leaves_an_events_path_that_is_not_a_regular_file_unopened() {
    let cfg = write_doctor_project(
        "doctor-events-fifo",
        "Feature: f\n  Scenario: s\n    Given set variable \"x\" to \"1\"\n",
        "",
    );
    let fifo = cfg.with_file_name("events.fifo");
    mkfifo(&fifo);

    let mut child = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config"])
        .arg(&cfg)
        .arg("--events")
        .arg(&fifo)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("spawn bddkit");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while child.try_wait().expect("poll").is_none() {
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            panic!("doctor is blocked opening the FIFO");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let out = child.wait_with_output().expect("output");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(stdout.contains("left unopened"), "{stdout}");
}

/// `run` would exit 2 on a directory, so `doctor` must not call it fine.
#[test]
fn doctor_refuses_an_events_path_that_is_a_directory() {
    let cfg = write_doctor_project(
        "doctor-events-dir",
        "Feature: f\n  Scenario: s\n    Given set variable \"x\" to \"1\"\n",
        "",
    );
    let dir = cfg.with_file_name("a-directory");
    std::fs::create_dir_all(&dir).expect("mkdir");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config"])
        .arg(&cfg)
        .arg("--events")
        .arg(&dir)
        .output()
        .expect("run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("✗ reports"), "{stdout}");
}
