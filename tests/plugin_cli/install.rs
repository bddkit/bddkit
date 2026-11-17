use super::{
    INDEX, Reply, host_target, init_with, lib_file, plugin, spawn, suite_config, temp, text,
};
use crate::common;
use std::path::Path;
use std::process::{Command, Output};

/// `bddkit init <args>` in `dir`, no terminal, the user layer inside `dir`, pointed at the stub.
fn init(dir: &Path, github: &str, args: &[&str]) -> Output {
    init_with(dir, github, args, &[])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plugins_go_to_the_chosen_layer() {
    let (stub, github) = spawn().await;
    stub.index(INDEX);
    stub.publish("bddkit/echo-plugin", "0.1.0");
    stub.latest("bddkit/echo-plugin", "0.1.0");

    let user = temp("init-user");
    // The echo fixture is `shared` and refuses a run with concurrency 8.
    let out = init(&user, &github, &["--plugin", "echo", "--parallel", "false"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(user.join("xdg-config/bddkit/plugins.yaml").is_file());
    assert!(!user.join(".bddkit").exists());

    let project = temp("init-project");
    let out = init(
        &project,
        &github,
        &[
            "--plugin",
            "echo",
            "--layer",
            "project",
            "--parallel",
            "false",
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(project.join(".bddkit/plugins.yaml").is_file());
    assert!(!project.join("xdg-config").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_plugin_install_keeps_the_config_and_exits_1() {
    let (stub, github) = spawn().await;
    stub.index(INDEX);
    stub.publish("bddkit/echo-plugin", "0.1.0");
    stub.latest("bddkit/echo-plugin", "0.1.0");
    let dir = temp("init-failed");

    let out = init(&dir, &github, &["--plugin", "nosuch", "--plugin", "echo"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(text(&out).contains("nosuch"), "{}", text(&out));
    assert!(dir.join("bddkit.yaml").is_file());
    // The failure stopped nobody else.
    assert!(dir.join("xdg-config/bddkit/plugins.yaml").is_file());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_index_install_is_registered_and_runs() {
    let (stub, github) = spawn().await;
    stub.index(INDEX);
    stub.publish("bddkit/echo-plugin", "0.1.0");
    stub.latest("bddkit/echo-plugin", "0.1.0");
    let dir = temp("install");

    let out = plugin(&dir, &github, &["install", "echo"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        text(&out).contains("installed echo 0.1.0 into override"),
        "{}",
        text(&out)
    );
    let lock = std::fs::read_to_string(dir.join("plugins.yaml")).expect("lock written");
    let rel = format!("plugins/echo-plugin/0.1.0/{}", lib_file());
    assert!(
        lock.contains(&format!("path: {rel}")),
        "relative path under override:\n{lock}"
    );
    assert!(lock.contains("source: bddkit/echo-plugin"), "{lock}");
    assert!(lock.contains("version: 0.1.0"), "{lock}");
    assert!(dir.join(&rel).is_file());
    assert!(!dir.join("plugins/echo-plugin/0.1.0/README.md").exists());

    // The installed plugin is the one `run` loads.
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("features/echo.feature"),
        "Feature: f\n  Scenario: s\n    When I echo \"x\" as \"greeting\"\n    Then variable \"greeting\" should be equal to \"p-x\"\n",
    )
    .expect("feature");
    std::fs::write(
        dir.join("cfg.yaml"),
        "paths: [features]\nconcurrency: 1\nresources:\n  api: {}\n  echo:\n    main:\n      prefix: \"p-\"\n",
    )
    .expect("config");
    let run = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config", "cfg.yaml", "--bddkit-dir"])
        .arg(&dir)
        .current_dir(&dir)
        .output()
        .expect("run");
    assert!(run.status.success(), "{}", text(&run));
}

const GROUP_INDEX: &str = "plugin:\n  - name: echo\n    repo: bddkit/echo-plugin\n    groups: [echo]\n  - name: mail\n    repo: bddkit/bddkit-mail\n    groups: [mail]\n    abi:\n      \"5\": 0.2.0\n";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn install_with_no_name_installs_what_the_config_needs_once() {
    let (stub, github) = spawn().await;
    stub.index(GROUP_INDEX);
    stub.publish("bddkit/echo-plugin", "0.1.0");
    stub.latest("bddkit/echo-plugin", "0.1.0");
    let dir = temp("install-needed");
    suite_config(&dir, "  mail: {}\n");

    let before = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config", "bddkit.yaml", "--bddkit-dir"])
        .arg(&*dir)
        .current_dir(&*dir)
        .output()
        .expect("run");
    assert_eq!(before.status.code(), Some(2), "{}", text(&before));
    let before = text(&before);
    assert!(
        before.contains("\"echo\"") && before.contains("bddkit plugin install"),
        "{before}"
    );
    assert!(
        !before.contains("bddkit/echo-plugin"),
        "the plugin is not named: {before}"
    );

    let out = plugin(&dir, &github, &["install"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        text(&out).contains("installed echo 0.1.0"),
        "{}",
        text(&out)
    );
    assert!(
        !text(&out).contains("mail"),
        "an empty group installs nothing: {}",
        text(&out)
    );

    let again = plugin(&dir, &github, &["install"]);
    assert!(again.status.success(), "{}", text(&again));
    assert!(
        text(&again).contains("nothing to install"),
        "{}",
        text(&again)
    );
    assert!(!text(&again).contains("installed echo"), "{}", text(&again));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plugin_needs_prints_the_json_upgrade_reads() {
    let (stub, github) = spawn().await;
    stub.index(GROUP_INDEX);
    let dir = temp("needs-json");
    suite_config(&dir, "");
    let out = plugin(
        &dir,
        &github,
        &["needs", "--json", "--config", "bddkit.yaml"],
    );
    assert!(out.status.success(), "{}", text(&out));
    let answer: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout is one JSON value");
    assert_eq!(
        answer,
        serde_json::json!({"plugins": [{"name": "echo", "repo": "bddkit/echo-plugin"}]})
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_needed_plugin_without_a_release_for_this_abi_is_a_warning() {
    let (stub, github) = spawn().await;
    stub.index(GROUP_INDEX);
    stub.publish("bddkit/echo-plugin", "0.1.0");
    stub.latest("bddkit/echo-plugin", "0.1.0");
    let dir = temp("install-no-build");
    suite_config(&dir, "  mail:\n    inbox:\n      host: localhost\n");

    let out = plugin(&dir, &github, &["install"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("warning: mail"), "{}", text(&out));
    assert!(
        text(&out).contains("installed echo 0.1.0"),
        "{}",
        text(&out)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn install_with_no_name_and_no_config_says_so() {
    let (_stub, github) = spawn().await;
    let dir = temp("install-no-config");
    let out = plugin(&dir, &github, &["install"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(text(&out).contains("no config found"), "{}", text(&out));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_owner_repo_install_takes_the_name_from_the_manifest() {
    let (stub, github) = spawn().await;
    stub.publish("someone/echo-plugin", "0.3.0");
    let dir = temp("owner-repo");

    let out = plugin(&dir, &github, &["install", "someone/echo-plugin@v0.3.0"]);
    assert!(out.status.success(), "{}", text(&out));
    let lock = std::fs::read_to_string(dir.join("plugins.yaml")).expect("lock");
    assert!(lock.contains("name: echo"), "{lock}");
    assert!(lock.contains("source: someone/echo-plugin"), "{lock}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_filters_the_index_and_marks_what_is_installed() {
    let (stub, github) = spawn().await;
    stub.index(INDEX);
    stub.publish("bddkit/echo-plugin", "0.1.0");
    let dir = temp("list");

    let out = plugin(&dir, &github, &["list", "MAILBOX"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("mail"), "{}", text(&out));
    assert!(!text(&out).contains("echo"), "{}", text(&out));

    assert!(
        plugin(&dir, &github, &["install", "echo@0.1.0"])
            .status
            .success()
    );
    let out = plugin(&dir, &github, &["list"]);
    assert!(
        text(&out).contains("installed 0.1.0 (override)"),
        "{}",
        text(&out)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_checksum_mismatch_writes_nothing() {
    let (stub, github) = spawn().await;
    stub.index(INDEX);
    stub.publish("bddkit/echo-plugin", "0.1.0");
    let asset = format!("echo-plugin-v0.1.0-{}.tar.gz", host_target());
    stub.set(
        &format!("/bddkit/echo-plugin/releases/download/v0.1.0/{asset}.sha256"),
        Reply::Body(format!("{}  {asset}\n", "0".repeat(64)).into_bytes()),
    );
    let dir = temp("checksum");

    let out = plugin(&dir, &github, &["install", "echo@0.1.0"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(text(&out).contains("checksum mismatch"), "{}", text(&out));
    assert!(!dir.join("plugins.yaml").exists());
    assert!(!dir.join("plugins").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn install_never_deletes_a_hand_written_entry_it_did_not_create() {
    // A hand-written lock entry has no `source`, so it is never "installed by
    // bddkit" — `manage::owned` must refuse it even though its path sits
    // right under `<data>/plugins`, the directory `plugin install` itself
    // writes into. Before the fix, `owned` only checked the path prefix, so
    // this exact shape (`plugins/<lib>`, one component deep) made a fresh
    // install wipe the whole `plugins/` directory it had just written into.
    let (stub, github) = spawn().await;
    stub.publish("bddkit/echo-plugin", "0.1.0");
    let dir = temp("hand-written");

    let hand_written = dir.join("plugins").join(lib_file());
    std::fs::create_dir_all(hand_written.parent().unwrap()).expect("mkdir");
    std::fs::copy(common::build_fixture_plugin(), &hand_written).expect("copy fixture");
    std::fs::write(
        dir.join("plugins.yaml"),
        format!(
            "plugin:\n  - name: echo\n    path: plugins/{}\n",
            lib_file()
        ),
    )
    .expect("hand-written lock");

    let out = plugin(&dir, &github, &["install", "bddkit/echo-plugin@0.1.0"]);
    assert!(out.status.success(), "{}", text(&out));

    let installed = dir.join(format!("plugins/echo-plugin/0.1.0/{}", lib_file()));
    assert!(installed.is_file(), "the new version was installed");
    assert!(
        hand_written.is_file(),
        "the hand-written entry's own file must survive the install"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_plugin_names_the_way_out() {
    let (stub, github) = spawn().await;
    stub.index(INDEX);
    let dir = temp("unknown");
    let out = plugin(&dir, &github, &["install", "nope"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(text(&out).contains("plugin list"), "{}", text(&out));
}
