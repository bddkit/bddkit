//! Helpers the integration tests share: the telemetry log and the fixture plugins.

#![allow(dead_code)]

use serde_json::Value;

/// The payloads a command recorded to `BDDKIT_TELEMETRY_LOG`, one per line.
pub fn read_payloads(log: &std::path::Path) -> Vec<Value> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).expect("a payload is one JSON object"))
        .collect()
}

/// stdout+stderr as one string — for checking error messages.
pub fn combined(out: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// Cuts a command off from this machine's own telemetry answer, config and CI
/// environment. Call it before setting the variables a test wants.
pub fn isolate_telemetry(command: &mut std::process::Command) -> &mut std::process::Command {
    for name in [
        "BDDKIT_TELEMETRY",
        "DO_NOT_TRACK",
        "BDDKIT_TELEMETRY_URL",
        "BDDKIT_CONFIG",
        "BDDKIT_DIR",
        "CI",
        "GITHUB_ACTIONS",
        "GITLAB_CI",
        "JENKINS_URL",
        "BUILDKITE",
        "CIRCLECI",
        "TF_BUILD",
        "TEAMCITY_VERSION",
        "BITBUCKET_BUILD_NUMBER",
    ] {
        command.env_remove(name);
    }
    command
}

/// `lib<stem>.so` / `lib<stem>.dylib` / `<stem>.dll`.
pub fn dll_name(stem: &str) -> String {
    format!(
        "{}{stem}{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    )
}

/// Builds the fixture plugin and returns the path to its shared library.
/// Cargo caches it, so only the first test in a run pays for the build.
pub fn build_fixture_plugin() -> std::path::PathBuf {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let target = root.join("target/fixture-plugin");
    let out = std::process::Command::new(env!("CARGO"))
        .args(["build", "--manifest-path"])
        .arg(root.join("tests/fixtures/echo-plugin/Cargo.toml"))
        .arg("--target-dir")
        .arg(&target)
        .output()
        .expect("failed to run cargo for the fixture plugin");
    assert!(
        out.status.success(),
        "fixture plugin build failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let path = target.join("debug").join(dll_name("echo_plugin"));
    assert!(
        path.exists(),
        "fixture plugin artifact missing at {}",
        path.display()
    );
    path
}

/// Builds the `per_worker` fixture and returns the path to its shared library.
/// A separate crate because a crate has one `lib` target and both concurrency
/// modes have to be exercised.
pub fn build_worker_plugin() -> std::path::PathBuf {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let target = root.join("target/worker-plugin");
    let out = std::process::Command::new(env!("CARGO"))
        .args(["build", "--manifest-path"])
        .arg(root.join("tests/fixtures/worker-plugin/Cargo.toml"))
        .arg("--target-dir")
        .arg(&target)
        .output()
        .expect("failed to run cargo for the worker plugin");
    assert!(
        out.status.success(),
        "worker plugin build failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let path = target.join("debug").join(dll_name("worker_plugin"));
    assert!(
        path.exists(),
        "worker plugin artifact missing at {}",
        path.display()
    );
    path
}

/// Builds the `group-plugin` fixture. `hooks` adds `bddkit_reset_scenario` and
/// `bddkit_select_instance`, which a build that must load `shared` at
/// concurrency 2 cannot have; each variant has its own target directory.
pub fn build_group_plugin(hooks: bool) -> std::path::PathBuf {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let target = root.join(if hooks {
        "target/group-plugin-hooks"
    } else {
        "target/group-plugin"
    });
    let mut command = std::process::Command::new(env!("CARGO"));
    command
        .args(["build", "--manifest-path"])
        .arg(root.join("tests/fixtures/group-plugin/Cargo.toml"))
        .arg("--target-dir")
        .arg(&target);
    if hooks {
        command.args(["--features", "hooks"]);
    }
    let out = command
        .output()
        .expect("failed to run cargo for the group plugin");
    assert!(
        out.status.success(),
        "group plugin build failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let path = target.join("debug").join(dll_name("group_plugin"));
    assert!(
        path.exists(),
        "group plugin artifact missing at {}",
        path.display()
    );
    path
}
