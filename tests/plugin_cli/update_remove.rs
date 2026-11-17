use super::{installed, lib_file, plugin, spawn, temp, text};
use crate::common;
use std::process::Command;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_moves_to_the_latest_release_and_drops_the_old_one() {
    let (stub, github) = spawn().await;
    let dir = temp("update");
    installed(&dir, &github, &stub, "0.1.0");
    stub.publish("bddkit/echo-plugin", "0.2.0");
    stub.latest("bddkit/echo-plugin", "0.2.0");

    let out = plugin(&dir, &github, &["update", "--dry-run"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("0.1.0 → 0.2.0"), "{}", text(&out));
    let lock = std::fs::read_to_string(dir.join("plugins.yaml")).expect("lock");
    assert!(
        lock.contains("version: 0.1.0"),
        "dry run writes nothing:\n{lock}"
    );
    assert!(
        !dir.join("plugins/echo-plugin/0.2.0").exists(),
        "a dry run unpacks nothing"
    );

    let out = plugin(&dir, &github, &["update"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        text(&out).contains("updated 0.1.0 → 0.2.0"),
        "{}",
        text(&out)
    );
    let lock = std::fs::read_to_string(dir.join("plugins.yaml")).expect("lock");
    assert!(lock.contains("version: 0.2.0"), "{lock}");
    assert!(!dir.join("plugins/echo-plugin/0.1.0").exists());
    assert!(
        dir.join("plugins/echo-plugin/0.2.0")
            .join(lib_file())
            .is_file()
    );

    let out = plugin(&dir, &github, &["update"]);
    assert!(text(&out).contains("up to date"), "{}", text(&out));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_skips_a_hand_written_entry() {
    let (_stub, github) = spawn().await;
    let dir = temp("update-hand");
    std::fs::write(
        dir.join("plugins.yaml"),
        format!(
            "plugin:\n  - name: echo\n    path: {}\n",
            common::build_fixture_plugin().display()
        ),
    )
    .expect("lock");
    let out = plugin(&dir, &github, &["update"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        text(&out).contains("skipped: not installed by bddkit"),
        "{}",
        text(&out)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remove_deletes_what_install_put_there() {
    let (stub, github) = spawn().await;
    let dir = temp("remove");
    installed(&dir, &github, &stub, "0.1.0");

    let out = plugin(&dir, &github, &["remove", "echo"]);
    assert!(out.status.success(), "{}", text(&out));
    let lock = std::fs::read_to_string(dir.join("plugins.yaml")).expect("lock");
    assert!(!lock.contains("echo"), "{lock}");
    assert!(!dir.join("plugins/echo-plugin").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remove_leaves_a_hand_written_entrys_file_alone() {
    let (_stub, github) = spawn().await;
    let dir = temp("remove-hand");
    let own = dir.join(lib_file());
    std::fs::copy(common::build_fixture_plugin(), &own).expect("copy");
    std::fs::write(
        dir.join("plugins.yaml"),
        format!("plugin:\n  - name: echo\n    path: {}\n", own.display()),
    )
    .expect("lock");

    let out = plugin(&dir, &github, &["remove", "echo"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        own.is_file(),
        "a file install did not put there is never deleted"
    );
    let lock = std::fs::read_to_string(dir.join("plugins.yaml")).expect("lock");
    assert!(!lock.contains("echo"), "the entry is gone:\n{lock}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remove_refuses_to_guess_between_layers() {
    let (_stub, github) = spawn().await;
    let root = temp("remove-layers");
    let (user, project) = (root.join("config/bddkit"), root.join("suite/.bddkit"));
    for dir in [&user, &project] {
        std::fs::create_dir_all(dir).expect("mkdir");
        std::fs::write(
            dir.join("plugins.yaml"),
            "plugin:\n  - name: echo\n    path: /opt/libecho.so\n",
        )
        .expect("lock");
    }
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["plugin", "remove", "echo"])
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("BDDKIT_GITHUB_URL", &github)
        .env_remove("BDDKIT_DIR")
        .env_remove("BDDKIT_CONFIG")
        .current_dir(root.join("suite"))
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(text(&out).contains("user, project"), "{}", text(&out));
    assert!(text(&out).contains("--layer"), "{}", text(&out));
}
