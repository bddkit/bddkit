use super::{CURRENT, Sandbox, abi_upgrade_ready, host_target, lib_file, spawn, text};
use std::process::Command;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn version_json_reports_the_plugin_abi() {
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["version", "--json"])
        .output()
        .expect("run");
    assert!(out.status.success(), "{}", text(&out));
    let info: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(info["version"], CURRENT);
    assert_eq!(info["plugin_abi"], 4);
    assert_eq!(info["plugin_abi_min"], 3);
    assert_eq!(info["target"], host_target());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn upgrade_refuses_a_binary_without_a_matching_receipt() {
    let (_stub, github) = spawn().await;
    let sandbox = Sandbox::new("upgrade-receipt");

    // No receipt at all.
    std::fs::remove_file(sandbox.data().join("install.json")).expect("rm");
    let out = sandbox.upgrade(&github, &["--yes"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(text(&out).contains("install.sh"), "{}", text(&out));
    assert!(text(&out).contains("cargo install"), "{}", text(&out));

    // A receipt for another binary.
    std::fs::create_dir_all(sandbox.data()).expect("mkdir");
    std::fs::write(
        sandbox.data().join("install.json"),
        serde_json::json!({"path": "/somewhere/else/bddkit", "version": CURRENT, "target": host_target()})
            .to_string(),
    )
    .expect("receipt");
    let out = sandbox.upgrade(&github, &["--yes"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(text(&out).contains("will not replace"), "{}", text(&out));
    assert_eq!(sandbox.bin_text(), sandbox.original);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn upgrade_check_changes_nothing() {
    let (stub, github) = spawn().await;
    let sandbox = Sandbox::new("upgrade-check");
    abi_upgrade_ready(&sandbox, &github, &stub, 0);
    let lock = sandbox.lock();

    // `--check` asks for no confirmation, so it needs neither --yes nor a terminal.
    let out = sandbox.upgrade(&github, &["--check"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        text(&out).contains(&format!("bddkit {CURRENT} → 99.0.0")),
        "{}",
        text(&out)
    );
    assert!(text(&out).contains("echo 0.1.0 → 0.2.0"), "{}", text(&out));
    assert_eq!(sandbox.bin_text(), sandbox.original);
    assert_eq!(sandbox.lock(), lock);
    assert!(sandbox.bin_dir_is_clean(), "staging left behind");
    assert!(!sandbox.plug.join("plugins/echo-plugin/0.2.0").exists());
    assert!(!sandbox.data().join("previous").exists());
    assert_eq!(sandbox.receipt_version(), CURRENT);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_a_terminal_upgrade_requires_yes() {
    let (stub, github) = spawn().await;
    let sandbox = Sandbox::new("upgrade-yes");
    stub.publish_bddkit("99.0.0", 4, 0);

    let out = sandbox.upgrade(&github, &[]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(text(&out).contains("--yes"), "{}", text(&out));
    assert_eq!(sandbox.bin_text(), sandbox.original);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_same_abi_upgrade_replaces_only_the_binary() {
    let (stub, github) = spawn().await;
    let sandbox = Sandbox::new("upgrade-same-abi");
    sandbox.install_echo(&github, &stub);
    stub.publish("bddkit/echo-plugin", "0.2.0");
    stub.latest("bddkit/echo-plugin", "0.2.0");
    stub.publish_bddkit("99.0.0", 4, 0);
    let lock = sandbox.lock();

    let out = sandbox.upgrade(&github, &["--yes"]);
    assert!(out.status.success(), "{}", text(&out));
    let bin = String::from_utf8(sandbox.bin_text()).expect("the staged script");
    assert!(bin.contains("staged-bddkit-99.0.0"), "{bin}");
    assert_eq!(sandbox.lock(), lock, "plugins are left alone");
    assert_eq!(
        std::fs::read(sandbox.data().join("previous/bddkit")).expect("kept"),
        sandbox.original
    );
    assert_eq!(sandbox.receipt_version(), "99.0.0");
    assert!(sandbox.bin_dir_is_clean(), "staging left behind");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_abi_upgrade_switches_every_installed_plugin_with_the_binary() {
    let (stub, github) = spawn().await;
    let sandbox = Sandbox::new("upgrade-abi");
    abi_upgrade_ready(&sandbox, &github, &stub, 0);

    let out = sandbox.upgrade(&github, &["--yes"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("echo 0.1.0 → 0.2.0"), "{}", text(&out));
    assert!(
        sandbox.lock().contains("version: 0.2.0"),
        "{}",
        sandbox.lock()
    );
    // Side by side: nothing was deleted, which is what makes rollback possible.
    for version in ["0.1.0", "0.2.0"] {
        assert!(
            sandbox
                .plug
                .join("plugins/echo-plugin")
                .join(version)
                .join(lib_file())
                .is_file(),
            "{version}"
        );
    }
    let backup =
        std::fs::read_to_string(sandbox.plug.join("plugins.yaml.before-upgrade")).expect("backup");
    assert!(backup.contains("version: 0.1.0"), "{backup}");
    assert_eq!(sandbox.receipt_version(), "99.0.0");
    assert_ne!(sandbox.bin_text(), sandbox.original);
}
