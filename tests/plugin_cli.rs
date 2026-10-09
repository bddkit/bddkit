//! `bddkit plugin …` end to end: the real binary against an axum stub that
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
    let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut builder = tar::Builder::new(gz);
    for (name, data) in [
        (lib_file(), lib.as_slice()),
        ("README.md".to_string(), b"readme".as_slice()),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder
            .append_data(&mut header, format!("{top}/{name}"), data)
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
    for part in ["groups:      echo", "concurrency: shared", "steps:       4"] {
        assert!(text(&out).contains(part), "{part}: {}", text(&out));
    }
    let out = plugin(&dir, &github, &["show", "nope"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
}
