use super::{ABI_INDEX, INDEX, installed, plugin, spawn, temp, text};
use std::path::Path;
use std::process::{Command, Output};

/// `plugin()` with the fixture library reporting `abi` to the host.
fn plugin_with_lib_abi(dir: &Path, github: &str, args: &[&str], abi: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .arg("plugin")
        .args(args)
        .arg("--bddkit-dir")
        .arg(dir)
        .env("BDDKIT_PLUGIN_REGISTRY", format!("{github}/registry.yaml"))
        .env("BDDKIT_GITHUB_URL", github)
        .env("BDDKIT_ECHO_FIXTURE_ABI", abi)
        .env_remove("BDDKIT_CONFIG")
        .env_remove("BDDKIT_DIR")
        .current_dir(dir)
        .output()
        .expect("run bddkit")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plugin_install_picks_the_release_for_the_host_abi() {
    let (stub, github) = spawn().await;
    stub.index(ABI_INDEX);
    stub.publish("bddkit/echo-plugin", "0.1.0");
    // The latest release is for a newer ABI; the index says which one fits.
    stub.latest("bddkit/echo-plugin", "0.2.0");
    let dir = temp("abi-pick");

    let out = plugin(&dir, &github, &["install", "echo"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        text(&out).contains("installed echo 0.1.0"),
        "{}",
        text(&out)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plugin_install_takes_the_newest_release_inside_the_accepted_range() {
    let both = "plugin:\n  - name: echo\n    repo: bddkit/echo-plugin\n    abi:\n      \"3\": 0.1.0\n      \"4\": 0.2.0\n";
    let only_older =
        "plugin:\n  - name: echo\n    repo: bddkit/echo-plugin\n    abi:\n      \"3\": 0.1.0\n";
    for (index, expected) in [(both, "0.2.0"), (only_older, "0.1.0")] {
        let (stub, github) = spawn().await;
        stub.index(index);
        stub.publish("bddkit/echo-plugin", "0.1.0");
        stub.publish("bddkit/echo-plugin", "0.2.0");
        let dir = temp("abi-range-pick");

        let out = plugin(&dir, &github, &["install", "echo"]);
        assert!(out.status.success(), "{}", text(&out));
        assert!(
            text(&out).contains(&format!("installed echo {expected}")),
            "{}",
            text(&out)
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pinned_release_declaring_abi_3_is_accepted() {
    let (stub, github) = spawn().await;
    stub.declare_abi("someone/echo-plugin", "0.3.0", 3);
    stub.publish("someone/echo-plugin", "0.3.0");
    let dir = temp("abi-pinned-3");

    let out = plugin(&dir, &github, &["install", "someone/echo-plugin@0.3.0"]);
    assert!(out.status.success(), "{}", text(&out));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_index_without_the_host_abi_names_the_abis_it_has() {
    let (stub, github) = spawn().await;
    stub.index(
        "plugin:\n  - name: echo\n    repo: bddkit/echo-plugin\n    abi:\n      \"6\": 0.2.0\n",
    );
    let dir = temp("abi-none");

    let out = plugin(&dir, &github, &["install", "echo"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(
        text(&out).contains("only for newer ABI 6: a newer bddkit is needed, run `bddkit upgrade`"),
        "{}",
        text(&out)
    );
    assert!(text(&out).contains("ABI 6"), "{}", text(&out));
    assert!(!dir.join("plugins.yaml").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_index_with_only_older_abis_says_the_plugin_is_not_updated_yet() {
    let (stub, github) = spawn().await;
    stub.index(
        "plugin:\n  - name: echo\n    repo: bddkit/echo-plugin\n    abi:\n      \"2\": 0.1.0\n",
    );
    let dir = temp("abi-older");

    let out = plugin(&dir, &github, &["install", "bddkit/echo-plugin"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(
        text(&out).contains("is not updated for this bddkit yet"),
        "{}",
        text(&out)
    );
    assert!(!text(&out).contains("bddkit upgrade"), "{}", text(&out));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mixed_abi_map_names_only_the_newer_abis_and_suggests_upgrade() {
    let (stub, github) = spawn().await;
    stub.index(
        "plugin:\n  - name: echo\n    repo: bddkit/echo-plugin\n    abi:\n      \"2\": 0.1.0\n      \"10\": 0.4.0\n      \"6\": 0.2.0\n",
    );
    let dir = temp("abi-mixed");

    let out = plugin(&dir, &github, &["install", "bddkit/echo-plugin"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(
        text(&out).contains("only for newer ABI 6, 10: a newer bddkit is needed"),
        "{}",
        text(&out)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pinned_repository_install_ignores_the_index() {
    let (stub, github) = spawn().await;
    // The index has no release for this host's ABI, a pin does not care.
    stub.index(
        "plugin:\n  - name: echo\n    repo: bddkit/echo-plugin\n    abi:\n      \"6\": 0.2.0\n",
    );
    stub.publish("bddkit/echo-plugin", "0.1.0");
    let dir = temp("repo-pinned");

    let out = plugin(&dir, &github, &["install", "bddkit/echo-plugin@0.1.0"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        text(&out).contains("installed echo 0.1.0"),
        "{}",
        text(&out)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn install_by_repository_takes_the_version_from_the_index() {
    let (stub, github) = spawn().await;
    stub.index(ABI_INDEX);
    stub.publish("bddkit/echo-plugin", "0.1.0");
    // The latest release is for a newer ABI; the index says which one fits.
    stub.latest("bddkit/echo-plugin", "0.2.0");
    let dir = temp("repo-via-index");

    let out = plugin(&dir, &github, &["install", "bddkit/echo-plugin"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        text(&out).contains("installed echo 0.1.0"),
        "{}",
        text(&out)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn install_by_repository_falls_back_to_the_latest_release_without_an_index() {
    let (stub, github) = spawn().await;
    // No index is served at all: the request for it is a 404.
    stub.publish("bddkit/echo-plugin", "0.1.0");
    stub.latest("bddkit/echo-plugin", "0.1.0");
    let dir = temp("repo-no-index");

    let out = plugin(&dir, &github, &["install", "bddkit/echo-plugin"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        text(&out).contains("installed echo 0.1.0"),
        "{}",
        text(&out)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_repository_outside_the_index_whose_latest_targets_another_abi_suggests_pinning() {
    let (stub, github) = spawn().await;
    stub.index(INDEX);
    stub.publish("someone/echo-plugin", "0.3.0");
    stub.latest("someone/echo-plugin", "0.3.0");
    stub.declare_abi("someone/echo-plugin", "0.3.0", 6);
    let dir = temp("repo-outside-abi");

    let out = plugin(&dir, &github, &["install", "someone/echo-plugin"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(
        text(&out)
            .contains("pin a version built for ABI 3 to 5 with someone/echo-plugin@<version>"),
        "{}",
        text(&out)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plugin_install_refuses_a_release_for_another_abi_before_downloading() {
    let (stub, github) = spawn().await;
    // Declared ABI 6, and NOT published: a download would be a 404, so the
    // ABI message proves the refusal came first.
    stub.declare_abi("someone/echo-plugin", "0.3.0", 6);
    let dir = temp("abi-declared");

    let out = plugin(&dir, &github, &["install", "someone/echo-plugin@0.3.0"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(
        text(&out).contains("targets plugin ABI 6"),
        "{}",
        text(&out)
    );
    assert!(!dir.join("plugins").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_abi_release_is_checked_after_download_and_removed_on_mismatch() {
    let (stub, github) = spawn().await;
    stub.publish("someone/echo-plugin", "0.3.0");
    let dir = temp("abi-unknown");

    let out = plugin_with_lib_abi(
        &dir,
        &github,
        &["install", "someone/echo-plugin@0.3.0"],
        "6",
    );
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(
        text(&out).contains("built for ABI version 6"),
        "{}",
        text(&out)
    );
    assert!(!dir.join("plugins.yaml").exists());
    assert!(!dir.join("plugins/echo-plugin/0.3.0").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plugin_update_does_not_move_to_a_newer_abi() {
    let (stub, github) = spawn().await;
    let dir = temp("abi-update");
    installed(&dir, &github, &stub, "0.1.0");
    // 0.2.0 exists but is built for ABI 6, in a repository the index
    // describes by a map that stops at 0.1.0.
    stub.index(&ABI_INDEX.replace("\"6\": 0.2.0\n", ""));
    stub.publish("bddkit/echo-plugin", "0.2.0");
    stub.latest("bddkit/echo-plugin", "0.2.0");

    let out = plugin(&dir, &github, &["update"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("up to date (0.1.0)"), "{}", text(&out));
    let lock = std::fs::read_to_string(dir.join("plugins.yaml")).expect("lock");
    assert!(lock.contains("version: 0.1.0"), "{lock}");
    assert!(dir.join("plugins/echo-plugin/0.1.0").is_dir());
    assert!(!dir.join("plugins/echo-plugin/0.2.0").exists());

    // Without a map the release itself is asked, with the same outcome.
    stub.index(INDEX);
    stub.declare_abi("bddkit/echo-plugin", "0.2.0", 6);
    let out = plugin(&dir, &github, &["update"]);
    assert!(
        text(&out).contains("targets plugin ABI 6"),
        "{}",
        text(&out)
    );
    assert!(!dir.join("plugins/echo-plugin/0.2.0").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plugin_update_moves_from_abi_3_to_the_newest_release_inside_the_range() {
    let (stub, github) = spawn().await;
    let dir = temp("abi-update-range");
    installed(&dir, &github, &stub, "0.1.0");
    stub.index(
        "plugin:\n  - name: echo\n    repo: bddkit/echo-plugin\n    abi:\n      \"3\": 0.1.0\n      \"4\": 0.2.0\n",
    );
    stub.publish("bddkit/echo-plugin", "0.2.0");

    let out = plugin(&dir, &github, &["update"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("0.1.0 → 0.2.0"), "{}", text(&out));
}
