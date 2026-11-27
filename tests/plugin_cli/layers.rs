use super::{INDEX, Stub, installed, lib_file, pack_files, plugin, spawn, temp, text};
use crate::common;
use std::path::Path;
use std::process::{Command, Output};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_layer_flag_is_refused_under_bddkit_dir_on_every_subcommand() {
    // `install` already refused this combination; `update` printed nothing
    // and exited 0, `remove` said "not installed in any lock file" instead of
    // naming the real conflict. Both must now refuse it the same way.
    let (_stub, github) = spawn().await;
    let dir = temp("layer-under-override");

    let out = plugin(&dir, &github, &["update", "--layer", "user"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(text(&out).contains("--bddkit-dir"), "{}", text(&out));

    let out = plugin(&dir, &github, &["remove", "echo", "--layer", "user"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(text(&out).contains("--bddkit-dir"), "{}", text(&out));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_layer_flag_is_refused_before_install_touches_the_network() {
    // The stub has no index at all: reaching it would fail with a 404 instead.
    let (_stub, github) = spawn().await;
    let dir = temp("install-layer-first");
    let out = plugin(&dir, &github, &["install", "echo", "--layer", "user"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(text(&out).contains("--bddkit-dir"), "{}", text(&out));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_malformed_lock_file_is_refused_before_anything_is_downloaded() {
    let (stub, github) = spawn().await;
    stub.index(INDEX);
    stub.publish("bddkit/echo-plugin", "0.1.0");
    stub.latest("bddkit/echo-plugin", "0.1.0");
    let dir = temp("install-bad-lock");
    std::fs::write(dir.join("plugins.yaml"), "plugin: 5\n").expect("lock");
    let out = plugin(&dir, &github, &["install", "echo"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(text(&out).contains("not a list"), "{}", text(&out));
    assert!(!dir.join("plugins").exists(), "nothing was unpacked");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_says_so_when_an_installed_entry_has_no_version() {
    let (_stub, github) = spawn().await;
    let dir = temp("update-no-version");
    std::fs::write(
        dir.join("plugins.yaml"),
        format!(
            "plugin:\n  - name: echo\n    path: {}\n    source: bddkit/echo-plugin\n",
            common::build_fixture_plugin().display()
        ),
    )
    .expect("lock");
    let out = plugin(&dir, &github, &["update"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("no version recorded"), "{}", text(&out));
    assert!(
        !text(&out).contains("not installed by bddkit"),
        "{}",
        text(&out)
    );
}

/// `plugin <args>` in a suite directory with a user layer beside it, no
/// `--bddkit-dir`: the layer chain is the real one.
fn layered(root: &Path, github: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .arg("plugin")
        .args(args)
        .env("BDDKIT_PLUGIN_REGISTRY", format!("{github}/registry.yaml"))
        .env("BDDKIT_GITHUB_URL", github)
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env_remove("BDDKIT_DIR")
        .env_remove("BDDKIT_CONFIG")
        .current_dir(root.join("suite"))
        .output()
        .expect("run bddkit")
}

/// A hand-written `echo` entry in `<dir>/plugins.yaml` pointing at the fixture.
fn declare_echo(dir: &Path) {
    std::fs::create_dir_all(dir).expect("mkdir");
    std::fs::write(
        dir.join("plugins.yaml"),
        format!(
            "plugin:\n  - name: echo\n    path: {}\n",
            common::build_fixture_plugin().display()
        ),
    )
    .expect("lock");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn installing_under_a_project_entry_warns_and_show_names_the_shadow() {
    let (stub, github) = spawn().await;
    stub.index(INDEX);
    stub.publish("bddkit/echo-plugin", "0.1.0");
    stub.latest("bddkit/echo-plugin", "0.1.0");
    let root = temp("shadow");
    declare_echo(&root.join("suite/.bddkit"));

    let out = layered(&root, &github, &["install", "echo", "--layer", "user"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("takes precedence"), "{stderr}");
    assert!(stderr.contains("project"), "{stderr}");

    // The project entry wins, and `show` says what it hides.
    let out = layered(&root, &github, &["show", "echo"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("shadowed:    user"), "{}", text(&out));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_first_project_install_is_not_reported_as_shadowed() {
    let (stub, github) = spawn().await;
    stub.index(INDEX);
    stub.publish("bddkit/echo-plugin", "0.1.0");
    stub.latest("bddkit/echo-plugin", "0.1.0");
    let root = temp("no-shadow");
    declare_echo(&root.join("config/bddkit"));
    std::fs::create_dir_all(root.join("suite")).expect("suite");

    let out = layered(&root, &github, &["install", "echo", "--layer", "project"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(!text(&out).contains("takes precedence"), "{}", text(&out));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn user_and_user_local_share_a_release_until_the_last_one_lets_go() {
    let (stub, github) = spawn().await;
    stub.index(INDEX);
    stub.publish("bddkit/echo-plugin", "0.1.0");
    stub.publish("bddkit/echo-plugin", "0.2.0");
    let root = temp("shared-data-dir");
    std::fs::create_dir_all(root.join("suite")).expect("suite");
    let release = root.join("data/bddkit/plugins/echo-plugin/0.1.0");
    for layer in ["user", "user.local"] {
        let out = layered(&root, &github, &["install", "echo@0.1.0", "--layer", layer]);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    }

    // Replacing one entry must not delete what the other still runs.
    let out = layered(
        &root,
        &github,
        &["install", "echo@0.2.0", "--layer", "user"],
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(release.is_dir(), "user.local still points into 0.1.0");

    // Removing it again: the other layer's entry is still there.
    let out = layered(&root, &github, &["remove", "echo", "--layer", "user"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(release.is_dir(), "{}", text(&out));

    let out = layered(&root, &github, &["remove", "echo", "--layer", "user.local"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(!release.exists(), "the last reference is gone");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_remove_that_cannot_check_the_other_lock_files_keeps_the_release() {
    let (stub, github) = spawn().await;
    let root = temp("remove-unchecked");
    std::fs::create_dir_all(root.join("suite")).expect("suite");
    installed_in(&root, &github, &stub);
    // A sibling lock file nothing can parse.
    std::fs::write(root.join("config/bddkit/plugins.local.yaml"), "plugin: 5\n").expect("lock");

    let out = layered(&root, &github, &["remove", "echo", "--layer", "user"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(
        text(&out).contains("could not finish removing"),
        "{}",
        text(&out)
    );
    assert!(root.join("data/bddkit/plugins/echo-plugin/0.1.0").is_dir());
}

/// `echo` 0.1.0 installed into the `user` layer of `root`.
fn installed_in(root: &Path, github: &str, stub: &Stub) {
    stub.index(INDEX);
    stub.publish("bddkit/echo-plugin", "0.1.0");
    let out = layered(root, github, &["install", "echo@0.1.0", "--layer", "user"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_broken_republished_tag_leaves_the_working_copy_in_place() {
    let (stub, github) = spawn().await;
    let dir = temp("republish");
    installed(&dir, &github, &stub, "0.1.0");
    let release = dir.join("plugins/echo-plugin/0.1.0");
    let library = release.join(lib_file());
    let before = std::fs::read(&library).expect("installed");
    let lock = std::fs::read_to_string(dir.join("plugins.yaml")).expect("lock");

    // A library that will not load, then an archive with no library at all.
    stub.publish_archive("bddkit/echo-plugin", "0.1.0", |top| {
        pack_files(top, &[(&lib_file(), b"not a library")])
    });
    let out = plugin(&dir, &github, &["install", "echo@0.1.0"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert_eq!(std::fs::read(&library).expect("kept"), before);
    assert!(
        text(&out).contains("echo-plugin"),
        "names the release: {}",
        text(&out)
    );

    stub.publish_archive("bddkit/echo-plugin", "0.1.0", |top| pack_files(top, &[]));
    let out = plugin(&dir, &github, &["install", "echo@0.1.0"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert_eq!(std::fs::read(&library).expect("kept"), before);

    // Nothing was left beside it, and the entry still loads.
    let names: Vec<_> = std::fs::read_dir(release.parent().unwrap())
        .expect("ls")
        .map(|e| e.expect("entry").file_name())
        .collect();
    assert_eq!(names, ["0.1.0"]);
    assert_eq!(
        std::fs::read_to_string(dir.join("plugins.yaml")).unwrap(),
        lock
    );
    let out = plugin(&dir, &github, &["show", "echo"]);
    assert!(out.status.success(), "{}", text(&out));
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lock_write_that_fails_puts_the_old_copy_back() {
    use std::os::unix::fs::PermissionsExt;
    let (stub, github) = spawn().await;
    let dir = temp("lock-write-fails");
    installed(&dir, &github, &stub, "0.1.0");
    let library = dir.join("plugins/echo-plugin/0.1.0").join(lib_file());
    let before = std::fs::read(&library).expect("installed");
    // Same library with a different tail, so a copy that was not restored shows.
    stub.publish_archive("bddkit/echo-plugin", "0.1.0", |top| {
        let mut lib = std::fs::read(common::build_fixture_plugin()).expect("fixture");
        lib.extend_from_slice(b"\0second build");
        pack_files(top, &[(&lib_file(), lib.as_slice())])
    });
    // The lock is replaced by a rename inside `dir`, which a read-only `dir`
    // refuses; `plugins/` below it stays writable, so the release still unpacks.
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).expect("chmod");
    let writable = std::fs::File::create(dir.join("probe")).is_ok();
    let out = plugin(&dir, &github, &["install", "echo@0.1.0"]);
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).expect("chmod back");
    if writable {
        return; // root ignores the mode, so there is nothing to fail
    }
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert_eq!(std::fs::read(&library).expect("kept"), before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_good_reinstall_of_the_same_version_replaces_it_and_leaves_nothing_beside() {
    let (stub, github) = spawn().await;
    let dir = temp("reinstall");
    installed(&dir, &github, &stub, "0.1.0");
    let out = plugin(&dir, &github, &["install", "echo@0.1.0"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let names: Vec<_> = std::fs::read_dir(dir.join("plugins/echo-plugin"))
        .expect("ls")
        .map(|e| e.expect("entry").file_name())
        .collect();
    assert_eq!(names, ["0.1.0"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_failing_update_does_not_stop_the_others() {
    let (stub, github) = spawn().await;
    let dir = temp("update-partial");
    installed(&dir, &github, &stub, "0.1.0");
    stub.publish("bddkit/echo-plugin", "0.2.0");
    stub.latest("bddkit/echo-plugin", "0.2.0");
    // An entry installed from a repository that publishes nothing.
    let mut lock = std::fs::read_to_string(dir.join("plugins.yaml")).expect("lock");
    lock.push_str(&format!(
        "- name: gone\n  path: {}\n  version: 1.0.0\n  source: nobody/gone\n",
        common::build_fixture_plugin().display()
    ));
    std::fs::write(dir.join("plugins.yaml"), lock).expect("lock");

    let out = plugin(&dir, &github, &["update"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(
        text(&out).contains("gone (override): failed"),
        "{}",
        text(&out)
    );
    assert!(
        text(&out).contains("updated 0.1.0 → 0.2.0"),
        "{}",
        text(&out)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_user_layer_lock_records_an_absolute_library_path() {
    let (stub, github) = spawn().await;
    let root = temp("absolute-path");
    std::fs::create_dir_all(root.join("suite")).expect("suite");
    installed_in(&root, &github, &stub);
    let lock = std::fs::read_to_string(root.join("config/bddkit/plugins.yaml")).expect("lock");
    let line = lock.lines().find(|l| l.contains("path:")).expect("path");
    let path = Path::new(line.split("path:").nth(1).expect("value").trim());
    assert!(path.is_absolute(), "{lock}");
    assert!(path.starts_with(root.join("data/bddkit")), "{lock}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_names_a_typo_that_matched_nothing() {
    let (_stub, github) = spawn().await;
    let dir = temp("update-typo");
    let out = plugin(&dir, &github, &["update", "nope"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(
        text(&out).contains("nope: failed: not installed in any lock file"),
        "{}",
        text(&out)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn show_lists_installed_plugins_and_one_in_detail() {
    let (stub, github) = spawn().await;
    let dir = temp("show");
    installed(&dir, &github, &stub, "0.1.0");

    let out = plugin(&dir, &github, &["show"]);
    assert!(out.status.success(), "{}", text(&out));
    for part in ["echo", "0.1.0", "override", "bddkit/echo-plugin"] {
        assert!(text(&out).contains(part), "{part}: {}", text(&out));
    }
    let out = plugin(&dir, &github, &["show", "echo"]);
    assert!(out.status.success(), "{}", text(&out));
    for part in [
        "groups:      echo",
        "concurrency: shared",
        "steps:       30",
    ] {
        assert!(text(&out).contains(part), "{part}: {}", text(&out));
    }
    let out = plugin(&dir, &github, &["show", "nope"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
}
