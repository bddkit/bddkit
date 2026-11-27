use super::{
    ABI_INDEX, CURRENT, GROUPED_ABI_INDEX, NEEDS_ECHO, Sandbox, Stub, abi_upgrade_ready,
    sandbox_suite, spawn, text,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn upgrade_installs_a_plugin_the_suite_needs_and_rollback_takes_it_back() {
    let (stub, github) = spawn().await;
    let sandbox = Sandbox::new("upgrade-needs");
    sandbox_suite(&sandbox);
    stub.index(GROUPED_ABI_INDEX);
    stub.publish("bddkit/echo-plugin", "0.2.0");
    stub.publish_bddkit_needing("99.0.0", 6, 0, NEEDS_ECHO);

    let checked = sandbox.upgrade(&github, &["--check"]);
    assert!(checked.status.success(), "{}", text(&checked));
    assert!(
        text(&checked).contains("echo not installed → 0.2.0"),
        "{}",
        text(&checked)
    );
    assert_eq!(sandbox.lock(), "", "--check writes nothing");

    let out = sandbox.upgrade(&github, &["--yes"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        sandbox.lock().contains("name: echo") && sandbox.lock().contains("version: 0.2.0"),
        "{}",
        sandbox.lock()
    );
    assert_eq!(sandbox.receipt_version(), "99.0.0");

    // The binary was replaced by a script: roll back with the kept real one.
    let kept = sandbox.data().join("previous/bddkit");
    let out = sandbox.upgrade_as(&kept, &github, &["--rollback"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        !sandbox.lock().contains("echo"),
        "the new entry is gone: {}",
        sandbox.lock()
    );
    assert_eq!(sandbox.receipt_version(), CURRENT);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_needed_plugin_already_moving_is_planned_once() {
    let (stub, github) = spawn().await;
    let sandbox = Sandbox::new("upgrade-needs-moving");
    sandbox_suite(&sandbox);
    sandbox.install_echo(&github, &stub);
    stub.index(GROUPED_ABI_INDEX);
    stub.publish("bddkit/echo-plugin", "0.2.0");
    stub.publish_bddkit_needing("99.0.0", 6, 0, NEEDS_ECHO);

    let out = sandbox.upgrade(&github, &["--yes"]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(
        sandbox.lock().matches("name: echo").count(),
        1,
        "{}",
        sandbox.lock()
    );
    assert!(
        sandbox.lock().contains("version: 0.2.0"),
        "{}",
        sandbox.lock()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_needed_plugin_with_no_release_for_the_new_abi_blocks_the_upgrade() {
    let (stub, github) = spawn().await;
    let sandbox = Sandbox::new("upgrade-needs-blocked");
    sandbox_suite(&sandbox);
    stub.index("plugin:\n  - name: echo\n    repo: bddkit/echo-plugin\n    groups: [echo]\n    abi:\n      \"4\": 0.1.0\n");
    stub.publish_bddkit_needing("99.0.0", 6, 0, NEEDS_ECHO);

    let out = sandbox.upgrade(&github, &["--yes"]);
    assert!(!out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("nothing was changed"), "{}", text(&out));
    assert_eq!(sandbox.bin_text(), sandbox.original);
    assert_eq!(sandbox.lock(), "");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_staged_bddkit_that_cannot_say_what_is_needed_means_nothing_with_a_warning() {
    let (stub, github) = spawn().await;
    let sandbox = Sandbox::new("upgrade-needs-unknown");
    sandbox_suite(&sandbox);
    stub.index(GROUPED_ABI_INDEX);
    // An older release: `plugin needs` is an unrecognized subcommand.
    stub.publish_bddkit("99.0.0", 6, 0);

    let out = sandbox.upgrade(&github, &["--yes"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        text(&out).contains("warning: could not ask the new bddkit"),
        "{}",
        text(&out)
    );
    assert_eq!(sandbox.receipt_version(), "99.0.0");
    assert_eq!(sandbox.lock(), "");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_abi_upgrade_with_a_plugin_lacking_a_release_changes_nothing() {
    let (stub, github) = spawn().await;
    let sandbox = Sandbox::new("upgrade-blocked");
    abi_upgrade_ready(&sandbox, &github, &stub, 0);
    // The index knows no release of echo for ABI 6.
    stub.index(&ABI_INDEX.replace("      \"6\": 0.2.0\n", ""));
    let lock = sandbox.lock();

    let out = sandbox.upgrade(&github, &["--yes"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(text(&out).contains("echo (override)"), "{}", text(&out));
    assert!(text(&out).contains("--force"), "{}", text(&out));
    assert_eq!(sandbox.bin_text(), sandbox.original);
    assert_eq!(sandbox.lock(), lock);
    assert!(sandbox.bin_dir_is_clean(), "staging left behind");
    assert!(!sandbox.plug.join("plugins/echo-plugin/0.2.0").exists());
    assert!(!sandbox.data().join("previous").exists());
    assert_eq!(sandbox.receipt_version(), CURRENT);
}

/// Echo 0.1.0 installed for ABI 4, and a bddkit 99.0.0 for ABI `max` that says it accepts `min..=max`.
fn ranged_upgrade_ready(
    sandbox: &Sandbox,
    github: &str,
    stub: &Stub,
    index: &str,
    (min, max): (u32, u32),
) {
    sandbox.install_echo(github, stub);
    stub.index(index);
    stub.publish("bddkit/echo-plugin", "0.2.0");
    stub.publish_bddkit_ranged("99.0.0", max, min, "");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_plugin_with_a_release_inside_the_staged_range_keeps_the_upgrade_going() {
    let (stub, github) = spawn().await;
    let sandbox = Sandbox::new("upgrade-range-keeps");
    let index =
        "plugin:\n  - name: echo\n    repo: bddkit/echo-plugin\n    abi:\n      \"4\": 0.1.0\n";
    ranged_upgrade_ready(&sandbox, &github, &stub, index, (4, 6));
    let lock = sandbox.lock();

    let out = sandbox.upgrade(&github, &["--yes"]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(sandbox.lock(), lock, "the plugin stays where it is");
    assert_eq!(sandbox.receipt_version(), "99.0.0");
    assert_ne!(sandbox.bin_text(), sandbox.original);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_plugin_with_a_newer_release_inside_the_staged_range_moves_to_the_highest() {
    let (stub, github) = spawn().await;
    let sandbox = Sandbox::new("upgrade-range-moves");
    ranged_upgrade_ready(&sandbox, &github, &stub, ABI_INDEX, (4, 6));

    let out = sandbox.upgrade(&github, &["--yes"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("echo 0.1.0 → 0.2.0"), "{}", text(&out));
    assert!(
        sandbox.lock().contains("version: 0.2.0"),
        "{}",
        sandbox.lock()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_release_outside_the_index_that_declares_an_abi_in_the_range_keeps_the_upgrade_going() {
    let (stub, github) = spawn().await;
    let sandbox = Sandbox::new("upgrade-range-declared");
    sandbox.install_echo(&github, &stub);
    stub.latest("bddkit/echo-plugin", "0.1.0");
    stub.declare_abi("bddkit/echo-plugin", "0.1.0", 4);
    stub.publish_bddkit_ranged("99.0.0", 6, 4, "");
    let lock = sandbox.lock();

    let out = sandbox.upgrade(&github, &["--yes"]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(sandbox.lock(), lock);
    assert_eq!(sandbox.receipt_version(), "99.0.0");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_staged_range_above_the_installed_release_blocks_the_upgrade() {
    let (stub, github) = spawn().await;
    let sandbox = Sandbox::new("upgrade-range-blocks");
    let index =
        "plugin:\n  - name: echo\n    repo: bddkit/echo-plugin\n    abi:\n      \"4\": 0.1.0\n";
    ranged_upgrade_ready(&sandbox, &github, &stub, index, (5, 6));
    let lock = sandbox.lock();

    let out = sandbox.upgrade(&github, &["--yes"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(text(&out).contains("ABI 5 to 6"), "{}", text(&out));
    assert!(text(&out).contains("--force"), "{}", text(&out));
    assert_eq!(sandbox.bin_text(), sandbox.original);
    assert_eq!(sandbox.lock(), lock);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_needed_plugin_installs_the_highest_release_in_the_staged_range() {
    let (stub, github) = spawn().await;
    let sandbox = Sandbox::new("upgrade-range-needs");
    sandbox_suite(&sandbox);
    stub.index(GROUPED_ABI_INDEX);
    stub.publish("bddkit/echo-plugin", "0.1.0");
    stub.publish("bddkit/echo-plugin", "0.2.0");
    stub.publish_bddkit_ranged("99.0.0", 6, 4, NEEDS_ECHO);
    let out = sandbox.upgrade(&github, &["--yes"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        sandbox.lock().contains("version: 0.2.0"),
        "{}",
        sandbox.lock()
    );

    // Only the ABI 4 release exists: it is within the range, so it is installed.
    let sandbox = Sandbox::new("upgrade-range-needs-low");
    sandbox_suite(&sandbox);
    stub.index(&GROUPED_ABI_INDEX.replace("      \"6\": 0.2.0\n", ""));
    let out = sandbox.upgrade(&github, &["--yes"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        sandbox.lock().contains("version: 0.1.0"),
        "{}",
        sandbox.lock()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn force_upgrades_and_leaves_the_blocking_plugin() {
    let (stub, github) = spawn().await;
    let sandbox = Sandbox::new("upgrade-force");
    abi_upgrade_ready(&sandbox, &github, &stub, 0);
    stub.index(&ABI_INDEX.replace("      \"6\": 0.2.0\n", ""));
    let lock = sandbox.lock();

    let out = sandbox.upgrade(&github, &["--yes", "--force"]);
    assert!(out.status.success(), "{}", text(&out));
    assert_ne!(sandbox.bin_text(), sandbox.original);
    assert_eq!(sandbox.lock(), lock, "the blocking plugin stays as it was");
    assert_eq!(sandbox.receipt_version(), "99.0.0");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_plugin_the_new_binary_refuses_changes_nothing() {
    let (stub, github) = spawn().await;
    let sandbox = Sandbox::new("upgrade-check-refused");
    abi_upgrade_ready(&sandbox, &github, &stub, 3);
    let lock = sandbox.lock();

    let out = sandbox.upgrade(&github, &["--yes"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(text(&out).contains("refuses it"), "{}", text(&out));
    assert_eq!(sandbox.bin_text(), sandbox.original);
    assert_eq!(sandbox.lock(), lock);
    assert!(sandbox.bin_dir_is_clean(), "staging left behind");
    assert!(
        !sandbox.plug.join("plugins/echo-plugin/0.2.0").exists(),
        "the directory this run created is removed"
    );
    assert!(sandbox.plug.join("plugins/echo-plugin/0.1.0").is_dir());
    assert!(!sandbox.data().join("previous/pending.json").exists());

    // --force treats the refusal like a missing release: the plugin stays.
    let out = sandbox.upgrade(&github, &["--yes", "--force"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("left as it is: echo"), "{}", text(&out));
    assert_eq!(sandbox.lock(), lock);
    assert_ne!(sandbox.bin_text(), sandbox.original);
    assert!(!sandbox.plug.join("plugins/echo-plugin/0.2.0").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_plugin_that_publishes_nothing_newer_blocks_an_abi_upgrade() {
    let (stub, github) = spawn().await;
    let sandbox = Sandbox::new("upgrade-no-newer");
    // No index map, no bddkit-plugin.json, and the latest release is the one
    // installed: nothing says it can be loaded under ABI 6.
    sandbox.install_echo(&github, &stub);
    stub.latest("bddkit/echo-plugin", "0.1.0");
    stub.publish_bddkit("99.0.0", 6, 0);
    let lock = sandbox.lock();

    let out = sandbox.upgrade(&github, &["--yes"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(text(&out).contains("newest release"), "{}", text(&out));
    assert_eq!(sandbox.bin_text(), sandbox.original);
    assert_eq!(sandbox.lock(), lock);
    assert_eq!(sandbox.receipt_version(), CURRENT);
}
