use std::process::Command;

/// A project with a core-only feature and the given `resources:` body.
fn empty_group_project(name: &str, resources: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("bddkit-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/f.feature"),
        "Feature: f\n  Scenario: s\n    Given set variable \"a\" to \"1\"\n",
    )
    .expect("write feature");
    std::fs::write(
        dir.join("cfg.yaml"),
        format!("paths: [features]\nresources:\n{resources}"),
    )
    .expect("write config");
    // An empty bddkit dir: a developer's installed plugins never decide this.
    std::fs::create_dir_all(dir.join(".bddkit")).expect("mkdir");
    dir
}

fn run_empty_group_project(
    subcommand: &str,
    dir: &std::path::Path,
) -> (Option<i32>, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .current_dir(dir)
        .args([subcommand, "--config", "cfg.yaml", "--bddkit-dir"])
        .arg(dir.join(".bddkit"))
        .output()
        .expect("run bddkit");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// The databases are the `db` plugin's: a connection with no plugin installed
/// is a startup failure that says what to do, not a silent no-op.
#[test]
fn a_database_with_no_plugin_installed_exits_2_and_names_the_install() {
    let dir = empty_group_project(
        "db-no-plugin",
        "  db:\n    main:\n      dsn: postgres://u:p@127.0.0.1:1/x\n",
    );
    let (code, stdout, stderr) = run_empty_group_project("run", &dir);
    assert_eq!(code, Some(2), "{stdout}{stderr}");
    let all = format!("{stdout}{stderr}");
    assert!(
        all.contains("no installed plugin serves the group \"db\""),
        "{all}"
    );
    assert!(all.contains("bddkit plugin install"), "{all}");
    assert!(all.contains("run not started"), "{all}");
}

/// The HTTP API and SRP are the `api` plugin's, the same way.
#[test]
fn an_api_or_srp_with_no_plugin_installed_exits_2_and_names_the_install() {
    for (group, body) in [
        (
            "api",
            "  api:\n    main:\n      base_url: http://127.0.0.1:1\n",
        ),
        ("srp", "  srp:\n    main: {}\n"),
    ] {
        let dir = empty_group_project(&format!("{group}-no-plugin"), body);
        let (code, stdout, stderr) = run_empty_group_project("run", &dir);
        assert_eq!(code, Some(2), "{stdout}{stderr}");
        let all = format!("{stdout}{stderr}");
        assert!(
            all.contains(&format!("no installed plugin serves the group \"{group}\"")),
            "{all}"
        );
        assert!(all.contains("bddkit plugin install"), "{all}");
    }
}

/// A stray `default_db`, `default_api` or `default_srp` was an error while the
/// kinds were built in, and stays one with no plugin installed: `run` and
/// `doctor` both refuse it.
#[test]
fn a_stray_default_is_refused_with_no_plugin_installed() {
    for key in ["default_api", "default_db", "default_srp"] {
        let dir = empty_group_project("stray-default", &format!("  foo: {{}}\n{key}: main\n"));
        let (code, stdout, stderr) = run_empty_group_project("run", &dir);
        assert_eq!(code, Some(2), "{key}: {stdout}{stderr}");
        assert!(
            format!("{stdout}{stderr}").contains(key),
            "{stdout}{stderr}"
        );
        let (code, stdout, stderr) = run_empty_group_project("doctor", &dir);
        assert_eq!(code, Some(1), "{key}: {stdout}{stderr}");
        assert!(stdout.contains(key), "{stdout}{stderr}");
    }
}

#[test]
fn a_config_with_no_resources_api_runs() {
    let dir = empty_group_project("no-api-key", "  db: {}\n");
    let (code, stdout, stderr) = run_empty_group_project("run", &dir);
    assert_eq!(code, Some(0), "{stdout}{stderr}");
}

#[test]
fn an_empty_group_no_plugin_serves_runs_quietly_and_doctor_hints_at_it() {
    let dir = empty_group_project("empty-group", "  foo: {}\n");
    let (code, stdout, stderr) = run_empty_group_project("run", &dir);
    assert_eq!(code, Some(0), "{stdout}{stderr}");
    assert!(
        !stdout.contains("foo") && !stderr.contains("foo"),
        "{stdout}{stderr}"
    );

    let (code, stdout, stderr) = run_empty_group_project("doctor", &dir);
    assert_eq!(code, Some(0), "{stdout}{stderr}");
    assert!(
        stdout.contains("resources.foo is empty") && stdout.contains("can be removed"),
        "{stdout}"
    );
}

#[test]
fn a_group_with_an_instance_no_plugin_serves_still_exits_two() {
    let dir = empty_group_project("unserved-group", "  foo:\n    main:\n      x: 1\n");
    let (code, stdout, stderr) = run_empty_group_project("run", &dir);
    assert_eq!(code, Some(2), "{stdout}{stderr}");
    assert!(
        format!("{stdout}{stderr}").contains("no installed plugin serves the group"),
        "{stdout}{stderr}"
    );
}
