use super::{ECHO_GROUP, project, run};
use std::process::Command;

/// A run's lock directory can be pointed at explicitly. The project has a
/// working lock in `.bddkit/`; every assertion below is that the override
/// wins over it, alone.
#[test]
fn the_bddkit_dir_override_reads_only_that_directory() {
    let dir = project(
        "override",
        "Feature: f\n  Scenario: s\n    When I echo \"x\" as \"greeting\"\n",
        ECHO_GROUP,
    );
    let empty = dir.join("empty");
    std::fs::create_dir_all(&empty).expect("mkdir empty");

    // Flag: the project's own `.bddkit/plugins.yaml` is skipped, so no plugin
    // serves the `echo` group.
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config", "cfg.yaml", "--bddkit-dir", "empty"])
        .current_dir(&dir)
        .output()
        .expect("run bddkit");
    assert_eq!(
        out.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("echo"));

    // Variable: same effect.
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config", "cfg.yaml"])
        .env("BDDKIT_DIR", &empty)
        .current_dir(&dir)
        .output()
        .expect("run bddkit");
    assert_eq!(
        out.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Flag over variable: the variable names the empty directory, the flag the
    // real one, and the run passes.
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config", "cfg.yaml", "--bddkit-dir", ".bddkit"])
        .env("BDDKIT_DIR", &empty)
        .current_dir(&dir)
        .output()
        .expect("run bddkit");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn a_missing_bddkit_dir_is_an_error_not_an_empty_plugin_list() {
    let dir = project(
        "override-missing",
        "Feature: f\n  Scenario: s\n    Given I am not in debug mode\n",
        "",
    );
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "run",
            "--config",
            "cfg.yaml",
            "--bddkit-dir",
            "does-not-exist",
        ])
        .current_dir(&dir)
        .output()
        .expect("run bddkit");
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("does-not-exist") && stderr.contains("does not exist"),
        "{stderr}"
    );
}

#[test]
fn plugins_local_yaml_overrides_the_committed_entry() {
    let dir = project(
        "local",
        "Feature: f\n  Scenario: s\n    When I echo \"x\" as \"greeting\"\n",
        ECHO_GROUP,
    );
    // The committed file points at a vendored path that does not exist here;
    // the developer's local file points at the real build.
    std::fs::rename(
        dir.join(".bddkit/plugins.yaml"),
        dir.join(".bddkit/plugins.local.yaml"),
    )
    .expect("rename to local");
    std::fs::write(
        dir.join(".bddkit/plugins.yaml"),
        "plugin:\n  - name: echo\n    path: vendor/libecho_plugin.so\n",
    )
    .expect("write committed lock");
    let out = run(&dir);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn the_project_layer_is_found_walking_up_from_the_config() {
    let dir = project(
        "walkup",
        "Feature: f\n  Scenario: s\n    When I echo \"x\" as \"greeting\"\n",
        ECHO_GROUP,
    );
    // Move the suite one level down; `.bddkit/` stays at the root.
    let suite = dir.join("suites/a");
    std::fs::create_dir_all(&suite).expect("mkdir suite");
    std::fs::rename(dir.join("features"), suite.join("features")).expect("move features");
    std::fs::rename(dir.join("cfg.yaml"), suite.join("cfg.yaml")).expect("move config");
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config", "cfg.yaml"])
        .current_dir(&suite)
        .output()
        .expect("run bddkit");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
