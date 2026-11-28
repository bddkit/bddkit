use super::{
    CURRENT, INDEX, Reply, Sandbox, abi_upgrade_ready, host_target, installed, output_retrying,
    payloads, spawn, temp, text,
};
use crate::common;
use std::path::Path;
use std::process::{Command, Output};

/// `plugin()` with telemetry on and its payloads logged to `<dir>/t.log`.
fn telemetry_plugin(dir: &Path, github: &str, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_bddkit"));
    common::isolate_telemetry(&mut command)
        .arg("plugin")
        .args(args)
        .arg("--bddkit-dir")
        .arg(dir)
        .env("BDDKIT_PLUGIN_REGISTRY", format!("{github}/registry.yaml"))
        .env("BDDKIT_GITHUB_URL", github)
        .env("BDDKIT_TELEMETRY", "1")
        .env("BDDKIT_TELEMETRY_LOG", dir.join("t.log"))
        .current_dir(dir);
    command.envs(env.iter().copied());
    command.output().expect("run bddkit")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_plugin_install_records_one_payload_naming_only_an_official_plugin() {
    let (stub, github) = spawn().await;
    stub.index(INDEX);
    stub.publish("bddkit/echo-plugin", "0.1.0");
    stub.latest("bddkit/echo-plugin", "0.1.0");
    stub.publish("someone/their-plugin", "0.4.0");
    let dir = temp("telemetry-install");

    let out = telemetry_plugin(&dir, &github, &["install", "echo"], &[]);
    assert!(out.status.success(), "{}", text(&out));
    let all = payloads(&dir);
    assert_eq!(all.len(), 1, "{all:?}");
    assert_eq!(all[0]["command"], "plugin_install");
    assert_eq!(all[0]["exit"], 0);
    assert_eq!(
        all[0]["plugin"],
        serde_json::json!([{"name": "echo", "version": "0.1.0", "from": "registry", "layer": "override", "result": "ok"}])
    );
    for run_only in ["files", "scenarios", "usage"] {
        assert!(all[0].get(run_only).is_none(), "{run_only}");
    }

    // Any other plugin is counted and never named, repository included.
    let out = telemetry_plugin(
        &dir,
        &github,
        &["install", "someone/their-plugin@0.4.0"],
        &[],
    );
    assert!(out.status.success(), "{}", text(&out));
    let second = payloads(&dir).pop().expect("a second payload");
    assert_eq!(
        second["plugin"],
        serde_json::json!([{"other": true, "from": "repository", "layer": "override", "result": "ok"}])
    );
    let raw = std::fs::read_to_string(dir.join("t.log")).unwrap();
    let last = raw.lines().last().unwrap();
    assert!(
        !last.contains("someone") && !last.contains("their"),
        "{last}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_plugin_install_reports_its_failure_class() {
    let (stub, github) = spawn().await;
    stub.index(INDEX);
    // checksum: the release's checksum file does not match the archive.
    stub.publish("bddkit/echo-plugin", "0.1.0");
    let asset = format!("echo-plugin-v0.1.0-{}.tar.gz", host_target());
    stub.set(
        &format!("/bddkit/echo-plugin/releases/download/v0.1.0/{asset}.sha256"),
        Reply::Body(format!("{}  {asset}\n", "0".repeat(64)).into_bytes()),
    );
    // no_build: a release that has no archive for this platform.
    stub.set(
        "/bddkit/echo-plugin/releases/tag/v0.2.0",
        Reply::Body(b"release".to_vec()),
    );
    let dir = temp("telemetry-failures");

    let class = |args: &[&str], github: &str, env: &[(&str, &str)]| {
        let out = telemetry_plugin(&dir, github, args, env);
        assert_eq!(out.status.code(), Some(1), "{args:?}: {}", text(&out));
        let payload = payloads(&dir).pop().expect("a payload");
        assert_eq!(payload["exit"], 1);
        payload["plugin"][0]["result"].as_str().unwrap().to_string()
    };
    assert_eq!(class(&["install", "echo@0.1.0"], &github, &[]), "checksum");
    assert_eq!(
        class(&["install", "nosuchplugin"], &github, &[]),
        "not_found"
    );
    assert_eq!(class(&["install", "echo@0.2.0"], &github, &[]), "no_build");
    // network: nothing listens there.
    assert_eq!(
        class(&["install", "echo"], "http://127.0.0.1:1", &[]),
        "network"
    );
    // refused: the library says it was built for another plugin ABI.
    stub.publish("bddkit/echo-plugin", "0.3.0");
    assert_eq!(
        class(
            &["install", "echo@0.3.0"],
            &github,
            &[("BDDKIT_ECHO_FIXTURE_ABI", "6")]
        ),
        "refused"
    );
    // The name of what failed is still an official one, or nothing.
    let raw = std::fs::read_to_string(dir.join("t.log")).unwrap();
    assert!(!raw.contains("nosuchplugin"), "{raw}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_and_remove_record_what_they_acted_on() {
    let (stub, github) = spawn().await;
    let dir = temp("telemetry-update-remove");
    installed(&dir, &github, &stub, "0.1.0");
    stub.publish("bddkit/echo-plugin", "0.2.0");
    stub.latest("bddkit/echo-plugin", "0.2.0");

    let out = telemetry_plugin(&dir, &github, &["update"], &[]);
    assert!(out.status.success(), "{}", text(&out));
    let update = payloads(&dir).pop().expect("a payload");
    assert_eq!(update["command"], "plugin_update");
    assert_eq!(update["plugin"][0]["version"], "0.2.0");
    assert_eq!(update["plugin"][0]["result"], "ok");

    let out = telemetry_plugin(&dir, &github, &["remove", "echo"], &[]);
    assert!(out.status.success(), "{}", text(&out));
    let remove = payloads(&dir).pop().expect("a payload");
    assert_eq!(remove["command"], "plugin_remove");
    assert!(remove["plugin"][0].get("from").is_none());
    assert_eq!(remove["plugin"][0]["name"], "echo");

    // Nothing to remove any more: a class, and a name nobody has to guess at.
    let out = telemetry_plugin(&dir, &github, &["remove", "echo"], &[]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    let again = payloads(&dir).pop().expect("a payload");
    assert_eq!(again["plugin"][0]["result"], "not_found");
    assert_eq!(again["plugin"][0]["other"], true);

    // A dry run changes nothing and reports nothing.
    let before = payloads(&dir).len();
    telemetry_plugin(&dir, &github, &["update", "--dry-run"], &[]);
    assert_eq!(payloads(&dir).len(), before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_plugin_command_without_a_config_follows_the_layers_and_a_broken_one_means_off() {
    let (stub, github) = spawn().await;
    stub.index(INDEX);
    stub.publish("bddkit/echo-plugin", "0.1.0");
    stub.latest("bddkit/echo-plugin", "0.1.0");

    // No config in the working directory, a yes in the layer, no variable.
    let dir = temp("telemetry-no-config");
    std::fs::write(dir.join("telemetry.yaml"), "enabled: true\n").unwrap();
    let out = telemetry_plugin(
        &dir,
        &github,
        &["install", "echo"],
        &[("BDDKIT_TELEMETRY", "")],
    );
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(payloads(&dir).len(), 1);

    // A config that is there and cannot be read: off, whatever the layer says.
    let dir = temp("telemetry-broken-config");
    std::fs::write(dir.join("telemetry.yaml"), "enabled: true\n").unwrap();
    std::fs::write(dir.join("bddkit.yaml"), "paths: [oops\n").unwrap();
    let out = telemetry_plugin(
        &dir,
        &github,
        &["install", "echo"],
        &[("BDDKIT_TELEMETRY", "")],
    );
    assert!(out.status.success(), "{}", text(&out));
    assert!(payloads(&dir).is_empty());
}

/// `bddkit upgrade <args>` as the sandboxed binary with telemetry on, logging
/// to `<root>/t.log`.
fn telemetry_upgrade(sandbox: &Sandbox, github: &str, args: &[&str]) -> Output {
    telemetry_upgrade_as(sandbox, &sandbox.bin, github, args)
}

fn telemetry_upgrade_as(sandbox: &Sandbox, bin: &Path, github: &str, args: &[&str]) -> Output {
    let mut command = sandbox.command(bin, github, args);
    common::isolate_telemetry(&mut command)
        .arg("--bddkit-dir")
        .arg(&sandbox.plug)
        .env("BDDKIT_TELEMETRY", "1")
        .env("BDDKIT_TELEMETRY_LOG", sandbox.root.join("t.log"));
    output_retrying(&mut command)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_upgrade_records_what_moved_and_a_refusal_is_not_a_failure() {
    let (stub, github) = spawn().await;
    let sandbox = Sandbox::new("telemetry-upgrade");
    abi_upgrade_ready(&sandbox, &github, &stub, 0);

    let out = telemetry_upgrade(&sandbox, &github, &["--check"]);
    assert!(out.status.success(), "{}", text(&out));
    let out = telemetry_upgrade(&sandbox, &github, &["--yes"]);
    assert!(out.status.success(), "{}", text(&out));
    // The binary the switch replaced is the one that can undo it.
    let previous = sandbox.data().join("previous/bddkit");
    let out = telemetry_upgrade_as(&sandbox, &previous, &github, &["--rollback"]);
    assert!(out.status.success(), "{}", text(&out));

    let all = payloads(&sandbox.root);
    let results: Vec<_> = all.iter().map(|p| p["upgrade"].clone()).collect();
    assert_eq!(results.len(), 3, "{all:?}");
    assert!(
        all.iter()
            .all(|p| p["command"] == "upgrade" && p["exit"] == 0)
    );
    assert_eq!(
        results[0],
        serde_json::json!({
            "from": CURRENT, "to": "99.0.0", "abi_changed": true,
            "plugins_switched": 0, "plugins_refused": 0, "plugins_left": 0,
            "result": "ok", "check": true
        })
    );
    assert_eq!(results[1]["result"], "ok");
    assert_eq!(results[1]["plugins_switched"], 1);
    assert_eq!(results[1]["check"], false);
    assert_eq!(results[2]["result"], "rolled_back");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_upgrade_that_is_refused_or_fails_says_which() {
    let (stub, github) = spawn().await;
    let sandbox = Sandbox::new("telemetry-upgrade-refused");
    // The new binary will not accept the plugin: nothing changes, and it is a failure.
    abi_upgrade_ready(&sandbox, &github, &stub, 1);
    let out = telemetry_upgrade(&sandbox, &github, &["--yes"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert_eq!(payloads(&sandbox.root)[0]["upgrade"]["result"], "failed");

    // Not installed by the install script: refused, with the version typed
    // after --version never named, because nothing proved it exists.
    std::fs::remove_file(sandbox.data().join("install.json")).expect("rm");
    let out = telemetry_upgrade(&sandbox, &github, &["--yes", "--version", "typed-by-hand"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    let refused = payloads(&sandbox.root).pop().expect("a payload");
    assert_eq!(refused["upgrade"]["result"], "refused");
    assert!(refused["upgrade"]["to"].is_null());
    let raw = std::fs::read_to_string(sandbox.root.join("t.log")).unwrap();
    assert!(!raw.contains("typed-by-hand"), "{raw}");
}
