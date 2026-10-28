//! `bddkit plugin …`, `bddkit upgrade` and the plugin half of `bddkit init` end to end: the real binary against an axum stub that
//! plays both the plugin index and GitHub Releases, installing the echo
//! fixture packed as a release archive. Everything lands in a temporary
//! `--bddkit-dir` unless a test is about the layer chain itself.
#![cfg(not(windows))] // the stub packs .tar.gz; Windows assets are .zip

mod common;

use axum::Router;
use axum::extract::State;
use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
enum Reply {
    Body(Vec<u8>),
    Redirect(String),
}

#[derive(Clone, Default)]
struct Stub(Arc<Mutex<HashMap<String, Reply>>>);

impl Stub {
    fn set(&self, path: &str, reply: Reply) {
        self.0.lock().unwrap().insert(path.to_string(), reply);
    }

    fn index(&self, yaml: &str) {
        self.set("/registry.yaml", Reply::Body(yaml.as_bytes().to_vec()));
    }

    /// Publishes the echo fixture as release `v<version>` of `repo`, the way
    /// the plugin release workflows do: a tar.gz holding a top directory with
    /// the library and a README, plus `<asset>.sha256` in sha256sum format.
    fn publish(&self, repo: &str, version: &str) {
        let basename = repo.rsplit('/').next().unwrap();
        let asset = format!("{basename}-v{version}-{}.tar.gz", host_target());
        let archive = pack(&format!("{basename}-v{version}-{}", host_target()));
        let sha = format!("{:x}  {asset}\n", Sha256::digest(&archive));
        let base = format!("/{repo}/releases/download/v{version}/{asset}");
        self.set(&base, Reply::Body(archive));
        self.set(&format!("{base}.sha256"), Reply::Body(sha.into_bytes()));
        self.set(
            &format!("/{repo}/releases/tag/v{version}"),
            Reply::Body(b"release".to_vec()),
        );
    }

    /// The `bddkit-plugin.json` asset a plugin release publishes.
    fn declare_abi(&self, repo: &str, version: &str, abi: u32) {
        self.set(
            &format!("/{repo}/releases/download/v{version}/bddkit-plugin.json"),
            Reply::Body(format!("{{\"abi\": {abi}}}").into_bytes()),
        );
    }

    /// Publishes release `v<version>` of bddkit itself, whose executable is a
    /// shell script that answers like a binary built for plugin ABI `abi` and
    /// whose `plugin check` exits with `check_exit`. Never the real binary:
    /// `upgrade` replaces what it is given, and tests share the built one.
    fn publish_bddkit(&self, version: &str, abi: u32, check_exit: i32) {
        let name = format!("bddkit-v{version}-{}", host_target());
        let script = format!(
            "#!/bin/sh\ncase \"$1 $2\" in\n\"version --json\") echo '{{\"version\":\"{version}\",\"target\":\"{}\",\"plugin_abi\":{abi}}}' ;;\n\"plugin check\") if [ {check_exit} -ne 0 ]; then echo refused >&2; exit {check_exit}; fi; echo echo ;;\n*) echo staged-bddkit-{version} ;;\nesac\n",
            host_target()
        );
        let archive = pack_files(&name, &[("bddkit", script.as_bytes())]);
        let asset = format!("{name}.tar.gz");
        let sha = format!("{:x}  {asset}\n", Sha256::digest(&archive));
        let base = format!("/bddkit/bddkit/releases/download/v{version}/{asset}");
        self.set(&base, Reply::Body(archive));
        self.set(&format!("{base}.sha256"), Reply::Body(sha.into_bytes()));
        self.latest("bddkit/bddkit", version);
        self.set(
            &format!("/bddkit/bddkit/releases/tag/v{version}"),
            Reply::Body(b"release".to_vec()),
        );
    }

    fn latest(&self, repo: &str, version: &str) {
        self.set(
            &format!("/{repo}/releases/latest"),
            Reply::Redirect(format!("/{repo}/releases/tag/v{version}")),
        );
    }
}

async fn serve(State(stub): State<Stub>, uri: Uri) -> Response {
    match stub.0.lock().unwrap().get(uri.path()).cloned() {
        Some(Reply::Body(body)) => body.into_response(),
        Some(Reply::Redirect(to)) => (StatusCode::FOUND, [(header::LOCATION, to)]).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn spawn() -> (Stub, String) {
    let stub = Stub::default();
    let app = Router::new().fallback(serve).with_state(stub.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    (stub, format!("http://{addr}"))
}

/// Mirrors `install::host_target` for the hosts CI and developers run on.
fn host_target() -> &'static str {
    match (std::env::consts::ARCH, std::env::consts::OS) {
        ("x86_64", "linux") => "x86_64-unknown-linux-gnu",
        ("aarch64", "linux") => "aarch64-unknown-linux-gnu",
        ("x86_64", "macos") => "x86_64-apple-darwin",
        ("aarch64", "macos") => "aarch64-apple-darwin",
        other => panic!("no release target for {other:?}"),
    }
}

fn lib_file() -> String {
    format!(
        "{}echo_plugin{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    )
}

fn pack(top: &str) -> Vec<u8> {
    let lib = std::fs::read(common::build_fixture_plugin()).expect("fixture");
    pack_files(top, &[(&lib_file(), lib.as_slice())])
}

/// A release archive: `files` and a README under one top directory.
fn pack_files(top: &str, files: &[(&str, &[u8])]) -> Vec<u8> {
    let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut builder = tar::Builder::new(gz);
    for (name, data) in files.iter().chain(&[("README.md", b"readme".as_slice())]) {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder
            .append_data(&mut header, format!("{top}/{name}"), *data)
            .expect("append");
    }
    builder.into_inner().expect("tar").finish().expect("gz")
}

const INDEX: &str = "plugin:\n  - name: echo\n    repo: bddkit/echo-plugin\n    description: Echo values back\n  - name: mail\n    repo: bddkit/bddkit-mail\n    description: Read a mailbox\n";

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bddkit-plugin-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

/// `bddkit plugin <args> --bddkit-dir <dir>`, pointed at the stub.
fn plugin(dir: &Path, github: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .arg("plugin")
        .args(args)
        .arg("--bddkit-dir")
        .arg(dir)
        .env("BDDKIT_PLUGIN_REGISTRY", format!("{github}/registry.yaml"))
        .env("BDDKIT_GITHUB_URL", github)
        .env_remove("BDDKIT_CONFIG")
        .env_remove("BDDKIT_DIR")
        .current_dir(dir)
        .output()
        .expect("run bddkit")
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// `bddkit init <args>` in `dir`, no terminal, the user layer inside `dir`, pointed at the stub.
fn init(dir: &Path, github: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .arg("init")
        .args(args)
        .env("BDDKIT_PLUGIN_REGISTRY", format!("{github}/registry.yaml"))
        .env("BDDKIT_GITHUB_URL", github)
        .env("HOME", dir)
        .env("XDG_CONFIG_HOME", dir.join("xdg-config"))
        .env("XDG_DATA_HOME", dir.join("xdg-data"))
        .env_remove("BDDKIT_CONFIG")
        .env_remove("BDDKIT_DIR")
        .env_remove("CI")
        .stdin(std::process::Stdio::null())
        .current_dir(dir)
        .output()
        .expect("run bddkit")
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

fn installed(dir: &Path, github: &str, stub: &Stub, version: &str) {
    stub.index(INDEX);
    stub.publish("bddkit/echo-plugin", version);
    let out = plugin(dir, github, &["install", &format!("echo@{version}")]);
    assert!(out.status.success(), "{}", text(&out));
}

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
        "steps:       12",
    ] {
        assert!(text(&out).contains(part), "{part}: {}", text(&out));
    }
    let out = plugin(&dir, &github, &["show", "nope"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
}

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

const ABI_INDEX: &str = "plugin:\n  - name: echo\n    repo: bddkit/echo-plugin\n    abi:\n      \"2\": 0.1.0\n      \"3\": 0.2.0\n";

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
async fn an_index_without_the_host_abi_names_the_abis_it_has() {
    let (stub, github) = spawn().await;
    stub.index(
        "plugin:\n  - name: echo\n    repo: bddkit/echo-plugin\n    abi:\n      \"3\": 0.2.0\n",
    );
    let dir = temp("abi-none");

    let out = plugin(&dir, &github, &["install", "echo"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(
        text(&out).contains("no release for plugin ABI 2"),
        "{}",
        text(&out)
    );
    assert!(text(&out).contains("ABI 3"), "{}", text(&out));
    assert!(!dir.join("plugins.yaml").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plugin_install_refuses_a_release_for_another_abi_before_downloading() {
    let (stub, github) = spawn().await;
    // Declared ABI 3, and NOT published: a download would be a 404, so the
    // ABI message proves the refusal came first.
    stub.declare_abi("someone/echo-plugin", "0.3.0", 3);
    let dir = temp("abi-declared");

    let out = plugin(&dir, &github, &["install", "someone/echo-plugin@0.3.0"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(
        text(&out).contains("targets plugin ABI 3"),
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
        "3",
    );
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(
        text(&out).contains("built for ABI version 3"),
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
    // 0.2.0 exists but is built for ABI 3, in a repository the index
    // describes by a map that stops at 0.1.0.
    stub.index(&ABI_INDEX.replace("\"3\": 0.2.0\n", ""));
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
    stub.declare_abi("bddkit/echo-plugin", "0.2.0", 3);
    let out = plugin(&dir, &github, &["update"]);
    assert!(
        text(&out).contains("targets plugin ABI 3"),
        "{}",
        text(&out)
    );
    assert!(!dir.join("plugins/echo-plugin/0.2.0").exists());
}

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

// ---- bddkit upgrade ----

const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// A copy of the built binary that the install receipt names, with its own
/// user data directory and a plugin directory (`--bddkit-dir`). The built
/// binary itself is never replaced: other tests are running it.
struct Sandbox {
    root: PathBuf,
    bin: PathBuf,
    plug: PathBuf,
    original: Vec<u8>,
}

/// Each sandbox holds a copy of the (debug-sized) binary: leaving a dozen of
/// them per run is how a temp filesystem fills up.
impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Sandbox {
    fn new(name: &str) -> Self {
        let root = temp(name);
        let bin_dir = root.join("bin");
        std::fs::create_dir_all(&bin_dir).expect("mkdir");
        let bin = bin_dir.join("bddkit");
        std::fs::copy(env!("CARGO_BIN_EXE_bddkit"), &bin).expect("copy binary");
        let plug = root.join("plug");
        std::fs::create_dir_all(&plug).expect("mkdir");
        let sandbox = Self {
            original: std::fs::read(&bin).expect("read copy"),
            root,
            bin,
            plug,
        };
        sandbox.write_receipt(CURRENT);
        sandbox
    }

    fn data(&self) -> PathBuf {
        self.root.join("xdg/bddkit")
    }

    fn write_receipt(&self, version: &str) {
        std::fs::create_dir_all(self.data()).expect("mkdir");
        std::fs::write(
            self.data().join("install.json"),
            serde_json::json!({"path": self.bin, "version": version, "target": host_target()})
                .to_string(),
        )
        .expect("receipt");
    }

    fn receipt_version(&self) -> String {
        let raw = std::fs::read_to_string(self.data().join("install.json")).expect("receipt");
        serde_json::from_str::<serde_json::Value>(&raw).expect("json")["version"]
            .as_str()
            .expect("version")
            .to_string()
    }

    fn command(&self, bin: &Path, github: &str, args: &[&str]) -> Command {
        let mut command = Command::new(bin);
        command
            .arg("upgrade")
            .args(args)
            .env("BDDKIT_PLUGIN_REGISTRY", format!("{github}/registry.yaml"))
            .env("BDDKIT_GITHUB_URL", github)
            .env("XDG_DATA_HOME", self.root.join("xdg"))
            .env("XDG_CONFIG_HOME", self.root.join("cfg"))
            .env("HOME", self.root.join("home"))
            .env_remove("BDDKIT_CONFIG")
            .env_remove("BDDKIT_DIR")
            .stdin(std::process::Stdio::null())
            .current_dir(&self.root);
        command
    }

    /// `bddkit upgrade <args> --bddkit-dir <plug>` run as the sandboxed binary.
    fn upgrade(&self, github: &str, args: &[&str]) -> Output {
        self.upgrade_as(&self.bin, github, args)
    }

    fn upgrade_as(&self, bin: &Path, github: &str, args: &[&str]) -> Output {
        let mut command = self.command(bin, github, args);
        command.arg("--bddkit-dir").arg(&self.plug);
        output_retrying(&mut command)
    }

    /// Installs echo 0.1.0 into the plugin directory with the real binary.
    fn install_echo(&self, github: &str, stub: &Stub) {
        installed(&self.plug, github, stub, "0.1.0");
    }

    fn lock(&self) -> String {
        std::fs::read_to_string(self.plug.join("plugins.yaml")).unwrap_or_default()
    }

    fn bin_text(&self) -> Vec<u8> {
        std::fs::read(&self.bin).expect("read bin")
    }

    /// Nothing but the binary beside it: no staging directory left behind.
    fn bin_dir_is_clean(&self) -> bool {
        std::fs::read_dir(self.bin.parent().unwrap())
            .unwrap()
            .count()
            == 1
    }
}

/// A freshly copied executable can be "text file busy" for an instant if a
/// parallel test forked while it was open for writing; that is not a failure.
fn output_retrying(command: &mut Command) -> Output {
    for _ in 0..40 {
        match command.output() {
            Err(e) if e.raw_os_error() == Some(26) => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            other => return other.expect("run bddkit"),
        }
    }
    panic!("the sandboxed binary stayed busy");
}

/// Echo 0.1.0 installed for ABI 2 and a 0.2.0 for ABI 3 published, with a
/// bddkit 99.0.0 for ABI 3 as the latest release.
fn abi_upgrade_ready(sandbox: &Sandbox, github: &str, stub: &Stub, check_exit: i32) {
    sandbox.install_echo(github, stub);
    stub.index(ABI_INDEX);
    stub.publish("bddkit/echo-plugin", "0.2.0");
    stub.publish_bddkit("99.0.0", 3, check_exit);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn version_json_reports_the_plugin_abi() {
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["version", "--json"])
        .output()
        .expect("run");
    assert!(out.status.success(), "{}", text(&out));
    let info: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(info["version"], CURRENT);
    assert_eq!(info["plugin_abi"], 2);
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
    stub.publish_bddkit("99.0.0", 2, 0);

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
    stub.publish_bddkit("99.0.0", 2, 0);
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_abi_upgrade_with_a_plugin_lacking_a_release_changes_nothing() {
    let (stub, github) = spawn().await;
    let sandbox = Sandbox::new("upgrade-blocked");
    abi_upgrade_ready(&sandbox, &github, &stub, 0);
    // The index knows no release of echo for ABI 3.
    stub.index(&ABI_INDEX.replace("      \"3\": 0.2.0\n", ""));
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn force_upgrades_and_leaves_the_blocking_plugin() {
    let (stub, github) = spawn().await;
    let sandbox = Sandbox::new("upgrade-force");
    abi_upgrade_ready(&sandbox, &github, &stub, 0);
    stub.index(&ABI_INDEX.replace("      \"3\": 0.2.0\n", ""));
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
    // installed: nothing says it can be loaded under ABI 3.
    sandbox.install_echo(&github, &stub);
    stub.latest("bddkit/echo-plugin", "0.1.0");
    stub.publish_bddkit("99.0.0", 3, 0);
    let lock = sandbox.lock();

    let out = sandbox.upgrade(&github, &["--yes"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(text(&out).contains("newest release"), "{}", text(&out));
    assert_eq!(sandbox.bin_text(), sandbox.original);
    assert_eq!(sandbox.lock(), lock);
    assert_eq!(sandbox.receipt_version(), CURRENT);
}

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

    let out = run(Some("3"));
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(
        text(&out).contains("built for ABI version 3"),
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
    stub.publish_bddkit("99.0.0", 3, 0);

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

fn payloads(dir: &Path) -> Vec<serde_json::Value> {
    common::read_payloads(&dir.join("t.log"))
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
            &[("BDDKIT_ECHO_FIXTURE_ABI", "3")]
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
