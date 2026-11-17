use super::{
    ABI_INDEX, CURRENT, Reply, Sandbox, abi_upgrade_ready, host_target, lib_file, output_retrying,
    spawn, text,
};
use crate::common;
use std::process::Command;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plugin_check_prints_the_manifest_name_and_refuses_another_abi() {
    let lib = common::build_fixture_plugin();
    let run = |abi: Option<&str>| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_bddkit"));
        command.args(["plugin", "check"]).arg(&lib);
        if let Some(abi) = abi {
            command.env("BDDKIT_ECHO_FIXTURE_ABI", abi);
        }
        command.output().expect("run")
    };
    let out = run(None);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "echo");

    let out = run(Some("5"));
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(
        text(&out).contains("built for ABI version 5"),
        "{}",
        text(&out)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_checksum_mismatch_changes_nothing() {
    let (stub, github) = spawn().await;
    let sandbox = Sandbox::new("upgrade-checksum");
    abi_upgrade_ready(&sandbox, &github, &stub, 0);
    let asset = format!("bddkit-v99.0.0-{}.tar.gz", host_target());
    stub.set(
        &format!("/bddkit/bddkit/releases/download/v99.0.0/{asset}.sha256"),
        Reply::Body(format!("{:064}  {asset}\n", 0).into_bytes()),
    );
    let lock = sandbox.lock();

    let out = sandbox.upgrade(&github, &["--yes"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(text(&out).contains("checksum mismatch"), "{}", text(&out));
    assert_eq!(sandbox.bin_text(), sandbox.original);
    assert_eq!(sandbox.lock(), lock);
    assert!(sandbox.bin_dir_is_clean(), "staging left behind");
    assert!(!sandbox.data().join("previous").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hand_written_and_project_entries_are_listed_not_changed() {
    let (stub, github) = spawn().await;
    let sandbox = Sandbox::new("upgrade-listed");
    stub.index(ABI_INDEX);
    stub.publish("bddkit/echo-plugin", "0.2.0");
    stub.publish_bddkit("99.0.0", 5, 0);

    // The user layer: one entry bddkit installed, one a person wrote.
    let user = sandbox.root.join("cfg/bddkit");
    std::fs::create_dir_all(&user).expect("mkdir");
    let installed_path = sandbox
        .root
        .join("xdg/bddkit/plugins/echo-plugin/0.1.0")
        .join(lib_file());
    std::fs::write(
        user.join("plugins.yaml"),
        format!(
            "plugin:\n  - name: echo\n    path: {}\n    version: 0.1.0\n    source: bddkit/echo-plugin\n  - name: mine\n    path: /opt/libmine.so\n",
            installed_path.display()
        ),
    )
    .expect("user lock");
    // A project layer with an installed-looking entry of its own.
    let project = sandbox.root.join("proj");
    std::fs::create_dir_all(project.join(".bddkit")).expect("mkdir");
    std::fs::write(project.join("bddkit.yaml"), "paths: []\n").expect("config");
    let project_lock = "plugin:\n  - name: other\n    path: plugins/libother.so\n    version: 0.1.0\n    source: bddkit/other\n";
    std::fs::write(project.join(".bddkit/plugins.yaml"), project_lock).expect("project lock");

    let config = project.join("bddkit.yaml");
    let mut command = sandbox.command(&sandbox.bin, &github, &["--yes", "--config"]);
    command.arg(&config);
    let out = output_retrying(&mut command);
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        text(&out).contains("mine (user): hand-written"),
        "{}",
        text(&out)
    );
    assert!(
        text(&out).contains("other (project): project layer"),
        "{}",
        text(&out)
    );
    assert_eq!(
        std::fs::read_to_string(project.join(".bddkit/plugins.yaml")).unwrap(),
        project_lock
    );
    let user_lock = std::fs::read_to_string(user.join("plugins.yaml")).unwrap();
    assert!(user_lock.contains("version: 0.2.0"), "{user_lock}");
    assert!(user_lock.contains("/opt/libmine.so"), "{user_lock}");
    assert_eq!(sandbox.receipt_version(), "99.0.0");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rollback_restores_the_binary_and_the_locks() {
    let (stub, github) = spawn().await;
    let sandbox = Sandbox::new("upgrade-rollback");
    abi_upgrade_ready(&sandbox, &github, &stub, 0);
    let lock = sandbox.lock();
    let out = sandbox.upgrade(&github, &["--yes"]);
    assert!(out.status.success(), "{}", text(&out));
    assert_ne!(sandbox.lock(), lock);

    // The new binary here is a script that cannot roll anything back, which is
    // exactly the case the kept binary exists for: it runs the rollback itself.
    let previous = sandbox.data().join("previous/bddkit");
    let out = sandbox.upgrade_as(&previous, &github, &["--rollback"]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(sandbox.bin_text(), sandbox.original);
    assert_eq!(sandbox.lock(), lock);
    assert_eq!(sandbox.receipt_version(), CURRENT);
    assert!(!sandbox.data().join("previous").exists());
    assert!(!sandbox.plug.join("plugins.yaml.before-upgrade").exists());

    let out = sandbox.upgrade(&github, &["--rollback"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(
        text(&out).contains("nothing to roll back"),
        "{}",
        text(&out)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lock_file_that_cannot_be_written_is_refused_before_anything_changes() {
    use std::os::unix::fs::PermissionsExt;
    let (stub, github) = spawn().await;
    let sandbox = Sandbox::new("upgrade-readonly");
    abi_upgrade_ready(&sandbox, &github, &stub, 0);
    let lock = sandbox.plug.join("plugins.yaml");
    std::fs::set_permissions(&lock, std::fs::Permissions::from_mode(0o444)).expect("chmod");
    // A superuser writes anyway; there is nothing to prove then.
    if std::fs::OpenOptions::new().append(true).open(&lock).is_ok() {
        return;
    }

    let out = sandbox.upgrade(&github, &["--yes"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(text(&out).contains("cannot write"), "{}", text(&out));
    assert_eq!(sandbox.bin_text(), sandbox.original);
    assert!(!sandbox.plug.join("plugins/echo-plugin/0.2.0").exists());
    assert!(!sandbox.data().join("previous/pending.json").exists());
    assert!(sandbox.bin_dir_is_clean(), "staging left behind");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_older_latest_release_is_not_a_silent_downgrade() {
    let (stub, github) = spawn().await;
    let sandbox = Sandbox::new("upgrade-older");
    stub.latest("bddkit/bddkit", "0.0.1");
    stub.set(
        "/bddkit/bddkit/releases/tag/v0.0.1",
        Reply::Body(b"release".to_vec()),
    );

    let out = sandbox.upgrade(&github, &["--yes"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("is older than"), "{}", text(&out));
    assert_eq!(sandbox.bin_text(), sandbox.original);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_lock_files_moving_one_release_share_its_directory() {
    let (stub, github) = spawn().await;
    let sandbox = Sandbox::new("upgrade-shared-dir");
    abi_upgrade_ready(&sandbox, &github, &stub, 0);
    let entry = sandbox.lock();
    std::fs::write(sandbox.plug.join("plugins.local.yaml"), &entry).expect("local lock");

    let out = sandbox.upgrade(&github, &["--yes"]);
    assert!(out.status.success(), "{}", text(&out));
    for file in ["plugins.yaml", "plugins.local.yaml"] {
        let lock = std::fs::read_to_string(sandbox.plug.join(file)).unwrap();
        assert!(lock.contains("version: 0.2.0"), "{file}:\n{lock}");
    }
    assert!(
        sandbox
            .plug
            .join("plugins/echo-plugin/0.2.0")
            .join(lib_file())
            .is_file()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_version_directory_something_else_uses_is_never_wiped() {
    let (stub, github) = spawn().await;
    let sandbox = Sandbox::new("upgrade-in-use");
    abi_upgrade_ready(&sandbox, &github, &stub, 0);
    let used = sandbox.plug.join("plugins/echo-plugin/0.2.0");
    std::fs::create_dir_all(&used).expect("mkdir");
    std::fs::write(used.join("keep"), "mine").expect("file");
    std::fs::write(
        sandbox.plug.join("plugins.local.yaml"),
        // Relative to the lock file, as a hand-written project entry would be.
        "plugin:\n  - name: mine\n    path: plugins/echo-plugin/0.2.0/keep\n",
    )
    .expect("local lock");
    let lock = sandbox.lock();

    let out = sandbox.upgrade(&github, &["--yes"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(
        text(&out).contains("already used by mine"),
        "{}",
        text(&out)
    );
    assert_eq!(std::fs::read_to_string(used.join("keep")).unwrap(), "mine");
    assert_eq!(sandbox.bin_text(), sandbox.original);
    assert_eq!(sandbox.lock(), lock);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rollback_that_stopped_halfway_can_be_run_again() {
    let (stub, github) = spawn().await;
    let sandbox = Sandbox::new("upgrade-rollback-retry");
    abi_upgrade_ready(&sandbox, &github, &stub, 0);
    let lock = sandbox.lock();
    let out = sandbox.upgrade(&github, &["--yes"]);
    assert!(out.status.success(), "{}", text(&out));
    // The state a first attempt leaves when it died after restoring the lock:
    // the backup is gone, the record is still there.
    std::fs::rename(
        sandbox.plug.join("plugins.yaml.before-upgrade"),
        sandbox.plug.join("plugins.yaml"),
    )
    .expect("restore the lock by hand");

    let previous = sandbox.data().join("previous/bddkit");
    let out = sandbox.upgrade_as(&previous, &github, &["--rollback"]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(sandbox.bin_text(), sandbox.original);
    assert_eq!(sandbox.lock(), lock);
    assert_eq!(sandbox.receipt_version(), CURRENT);
}
