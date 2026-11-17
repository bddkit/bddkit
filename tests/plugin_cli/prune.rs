use super::{installed, lib_file, plugin, spawn, temp, text};
use std::path::{Path, PathBuf};

/// A version directory `install` made earlier, which no lock entry names.
fn stale_version(dir: &Path) -> PathBuf {
    let old = dir.join("plugins/echo-plugin/0.0.9");
    std::fs::create_dir_all(&old).expect("mkdir");
    std::fs::write(old.join(lib_file()), b"old").expect("write");
    old
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prune_removes_only_unreferenced_versions_installed_by_bddkit() {
    let (stub, github) = spawn().await;
    let dir = temp("prune");
    installed(&dir, &github, &stub, "0.1.0");
    let old = stale_version(&dir);

    let out = plugin(&dir, &github, &["prune"]);
    assert!(
        !out.status.success(),
        "no terminal and no --yes: {}",
        text(&out)
    );
    assert!(old.exists());

    let out = plugin(&dir, &github, &["prune", "--yes"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(!old.exists(), "{}", text(&out));
    assert!(
        dir.join("plugins/echo-plugin/0.1.0")
            .join(lib_file())
            .is_file(),
        "the referenced version stays"
    );

    let out = plugin(&dir, &github, &["prune", "--yes"]);
    assert!(text(&out).contains("nothing to prune"), "{}", text(&out));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prune_dry_run_changes_nothing() {
    let (stub, github) = spawn().await;
    let dir = temp("prune-dry");
    installed(&dir, &github, &stub, "0.1.0");
    let old = stale_version(&dir);

    let out = plugin(&dir, &github, &["prune", "--dry-run"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("0.0.9"), "{}", text(&out));
    assert!(text(&out).contains("3 B"), "{}", text(&out));
    assert!(old.join(lib_file()).is_file());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prune_keeps_what_a_rollback_would_restore() {
    let (stub, github) = spawn().await;
    let dir = temp("prune-backup");
    installed(&dir, &github, &stub, "0.1.0");
    let old = stale_version(&dir);
    let lock = std::fs::read_to_string(dir.join("plugins.yaml")).expect("lock");
    std::fs::write(
        dir.join("plugins.yaml.before-upgrade"),
        lock.replace("0.1.0", "0.0.9"),
    )
    .expect("backup");

    let out = plugin(&dir, &github, &["prune", "--yes"]);
    assert!(text(&out).contains("nothing to prune"), "{}", text(&out));
    assert!(old.join(lib_file()).is_file());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prune_leaves_what_install_did_not_make() {
    let (stub, github) = spawn().await;
    let dir = temp("prune-shape");
    installed(&dir, &github, &stub, "0.1.0");
    let dev = dir.join("plugins/mylib/dev");
    std::fs::create_dir_all(&dev).expect("mkdir");
    std::fs::write(dev.join(lib_file()), b"x").expect("write");
    std::fs::write(dev.join("notes.txt"), b"x").expect("write");
    // An unreferenced, install-shaped directory outside plugins/, reached
    // only through a link inside it.
    #[cfg(unix)]
    let target = {
        let target = temp("prune-link-target");
        std::fs::write(target.join(lib_file()), b"x").expect("write");
        std::os::unix::fs::symlink(&target, dir.join("plugins/echo-plugin/link")).expect("symlink");
        target
    };

    let out = plugin(&dir, &github, &["prune", "--yes"]);
    assert!(text(&out).contains("nothing to prune"), "{}", text(&out));
    assert!(dev.join(lib_file()).is_file());
    #[cfg(unix)]
    assert!(target.join(lib_file()).is_file());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prune_keeps_what_only_a_local_lock_references() {
    let (stub, github) = spawn().await;
    let dir = temp("prune-local");
    installed(&dir, &github, &stub, "0.1.0");
    let old = stale_version(&dir);
    let lock = std::fs::read_to_string(dir.join("plugins.yaml")).expect("lock");
    std::fs::write(
        dir.join("plugins.local.yaml"),
        lock.replace("0.1.0", "0.0.9")
            .replace("name: echo\n", "name: echo2\n"),
    )
    .expect("local lock");

    let out = plugin(&dir, &github, &["prune", "--yes"]);
    assert!(text(&out).contains("nothing to prune"), "{}", text(&out));
    assert!(old.join(lib_file()).is_file());
}
