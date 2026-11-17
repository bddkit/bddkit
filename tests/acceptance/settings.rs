use std::process::Command;

/// Flag, then environment, then config key, for the options the config also
/// holds. Three files on one worker, the first fails: how many run says which
/// source `fail_fast` came from.
#[test]
fn fail_fast_and_concurrency_follow_flag_then_environment_then_config() {
    let dir = std::env::temp_dir().join(format!("bddkit-option-forms-{}", std::process::id()));
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
    let features = dir
        .join("features")
        .display()
        .to_string()
        .replace('\\', "/");
    let write_config = |extra: &str| {
        std::fs::write(
            dir.join("cfg.yaml"),
            format!("{extra}paths: [{features}]\nresources:\n  api: {{}}\n"),
        )
        .expect("write config");
    };
    let run = |args: &[&str], envs: &[(&str, &str)]| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_bddkit"));
        cmd.args(["run", "--config"]).arg(dir.join("cfg.yaml"));
        cmd.args(args).env_remove("BDDKIT_FAIL_FAST");
        cmd.env_remove("BDDKIT_CONCURRENCY")
            .envs(envs.iter().copied());
        let out = cmd.output().expect("run bddkit");
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };

    // The config key alone.
    write_config("concurrency: 1\nfail_fast: true\n");
    let (code, stdout, _) = run(&[], &[]);
    assert_eq!(code, Some(1));
    assert!(stdout.contains("files: 1"), "{stdout}");
    // The flag outranks the key, the variable outranks the key too.
    let (_, stdout, _) = run(&["--fail-fast=false"], &[]);
    assert!(stdout.contains("files: 3"), "{stdout}");
    let (_, stdout, _) = run(&[], &[("BDDKIT_FAIL_FAST", "0")]);
    assert!(stdout.contains("files: 3"), "{stdout}");
    // The variable alone, then a flag over the variable.
    write_config("concurrency: 4\n");
    let (_, stdout, _) = run(&["--concurrency", "1"], &[("BDDKIT_FAIL_FAST", "true")]);
    assert!(stdout.contains("files: 1"), "{stdout}");
    let (_, stdout, _) = run(
        &["--fail-fast=false"],
        &[("BDDKIT_FAIL_FAST", "true"), ("BDDKIT_CONCURRENCY", "1")],
    );
    assert!(stdout.contains("files: 3"), "{stdout}");
    // A malformed variable is a refusal before the first request; doctor reports it.
    let (code, _, stderr) = run(&[], &[("BDDKIT_FAIL_FAST", "yes")]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains("BDDKIT_FAIL_FAST"), "{stderr}");
    let doctor = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config"])
        .arg(dir.join("cfg.yaml"))
        .env("BDDKIT_CONCURRENCY", "many")
        .output()
        .expect("run doctor");
    assert_eq!(doctor.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&doctor.stdout).contains("BDDKIT_CONCURRENCY"));
}

/// `tags` is a config key, `BDDKIT_TAG` a variable, `--tag` a flag; a selection
/// that matches nothing is exit 2 in `run` and a failed row in `doctor`.
#[test]
fn tags_follow_flag_then_environment_then_config_and_doctor_agrees() {
    let dir = std::env::temp_dir().join(format!("bddkit-tags-forms-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/a.feature"),
        "Feature: a\n  @smoke\n  Scenario: s\n    Given set variable \"x\" to \"1\"\n",
    )
    .expect("write feature");
    let features = dir
        .join("features")
        .display()
        .to_string()
        .replace('\\', "/");
    let write_config = |tags: &str| {
        std::fs::write(
            dir.join("cfg.yaml"),
            format!("{tags}paths: [{features}]\nresources:\n  api: {{}}\n"),
        )
        .expect("write config");
    };
    let bddkit = |command: &str, args: &[&str], envs: &[(&str, &str)]| {
        let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
            .args([command, "--config"])
            .arg(dir.join("cfg.yaml"))
            .args(args)
            .env_remove("BDDKIT_TAG")
            .envs(envs.iter().copied())
            .output()
            .expect("run bddkit");
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
        )
    };

    write_config("tags: [smoke]\n");
    assert_eq!(bddkit("run", &[], &[]).0, Some(0));
    // The variable outranks the key, the flag outranks the variable.
    assert_eq!(bddkit("run", &[], &[("BDDKIT_TAG", "other")]).0, Some(2));
    assert_eq!(
        bddkit("run", &["--tag", "smoke"], &[("BDDKIT_TAG", "other")]).0,
        Some(0)
    );
    // A key that selects nothing: run refuses, doctor says so.
    write_config("tags: [other]\n");
    assert_eq!(bddkit("run", &[], &[]).0, Some(2));
    let (code, stdout) = bddkit("doctor", &[], &[]);
    assert_eq!(code, Some(1), "{stdout}");
    assert!(stdout.contains("no scenario to run"), "{stdout}");
    // Tags decide whether a run has anything to do, not which files `doctor` reads.
    std::fs::write(
        dir.join("features/b.feature"),
        "Feature: b\n  @slow\n  Scenario: s\n    When I frobnicate\n",
    )
    .expect("write feature");
    write_config("tags: [smoke]\n");
    let (code, stdout) = bddkit("doctor", &[], &[]);
    assert_eq!(code, Some(1), "{stdout}");
    assert!(stdout.contains("frobnicate"), "{stdout}");
}
