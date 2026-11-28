//! `bddkit plugin …`, `bddkit upgrade` and the plugin half of `bddkit init` end to end: the real binary against an axum stub that
//! plays both the plugin index and GitHub Releases, installing the echo
//! fixture packed as a release archive. Everything lands in a temporary
//! `--bddkit-dir` unless a test is about the layer chain itself.
#![cfg(not(windows))] // the stub packs .tar.gz; Windows assets are .zip
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    reason = "`allow-*-in-tests` covers only `#[test]` functions, not the helpers here"
)]

mod abi_selection;
#[path = "../common/mod.rs"]
mod common;
mod init;
mod install;
mod layers;
mod prune;
mod telemetry;
mod ui_install;
mod update_remove;
mod upgrade;
mod upgrade_needs;
mod upgrade_rollback;

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
        self.publish_archive(repo, version, pack);
    }

    /// The same release with whatever archive `make` builds from its top directory.
    fn publish_archive(&self, repo: &str, version: &str, make: impl Fn(&str) -> Vec<u8>) {
        let basename = repo.rsplit('/').next().unwrap();
        let asset = format!("{basename}-v{version}-{}.tar.gz", host_target());
        let archive = make(&format!("{basename}-v{version}-{}", host_target()));
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
        self.publish_bddkit_needing(version, abi, check_exit, "");
    }

    /// The same release whose `plugin needs` prints `needs` (a shell command);
    /// empty means a bddkit that does not know the subcommand.
    fn publish_bddkit_needing(&self, version: &str, abi: u32, check_exit: i32, needs: &str) {
        self.publish_bddkit_full(version, abi, None, check_exit, needs);
    }

    /// A release for ABI `abi` that also says it accepts plugins down to `min`.
    fn publish_bddkit_ranged(&self, version: &str, abi: u32, min: u32, needs: &str) {
        self.publish_bddkit_full(version, abi, Some(min), 0, needs);
    }

    fn publish_bddkit_full(
        &self,
        version: &str,
        abi: u32,
        min: Option<u32>,
        check_exit: i32,
        needs: &str,
    ) {
        let min = min.map_or(String::new(), |m| format!(",\"plugin_abi_min\":{m}"));
        let needs = if needs.is_empty() {
            "echo \"error: unrecognized subcommand 'needs'\" >&2; exit 2".to_string()
        } else {
            needs.to_string()
        };
        let name = format!("bddkit-v{version}-{}", host_target());
        let script = format!(
            "#!/bin/sh\ncase \"$1 $2\" in\n\"version --json\") echo '{{\"version\":\"{version}\",\"target\":\"{}\",\"plugin_abi\":{abi}{min}}}' ;;\n\"plugin check\") if [ {check_exit} -ne 0 ]; then echo refused >&2; exit {check_exit}; fi; echo echo ;;\n\"plugin needs\") {needs} ;;\n*) echo staged-bddkit-{version} ;;\nesac\n",
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
    common::dll_name("echo_plugin")
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

/// A scratch directory, removed when the test ends.
struct Temp(PathBuf);

impl std::ops::Deref for Temp {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for Temp {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<std::ffi::OsStr> for Temp {
    fn as_ref(&self) -> &std::ffi::OsStr {
        self.0.as_os_str()
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp(name: &str) -> Temp {
    Temp(scratch(name))
}

/// The same directory with no one to remove it, for what cleans up itself.
fn scratch(name: &str) -> PathBuf {
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

/// The same, with `envs` set on top.
fn init_with(dir: &Path, github: &str, args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_bddkit"));
    common::isolate_telemetry(&mut command)
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
        .envs(envs.iter().copied())
        .stdin(std::process::Stdio::null())
        .current_dir(dir)
        .output()
        .expect("run bddkit")
}

/// A suite using `echo`, with an empty `mail` group that needs no plugin.
fn suite_config(dir: &Path, extra: &str) {
    std::fs::write(
        dir.join("bddkit.yaml"),
        format!("paths: [features]\nresources:\n  api: {{}}\n  echo:\n    main:\n      prefix: \"p-\"\n{extra}"),
    )
    .expect("config");
}

fn installed(dir: &Path, github: &str, stub: &Stub, version: &str) {
    stub.index(INDEX);
    stub.publish("bddkit/echo-plugin", version);
    let out = plugin(dir, github, &["install", &format!("echo@{version}")]);
    assert!(out.status.success(), "{}", text(&out));
}

const ABI_INDEX: &str = "plugin:\n  - name: echo\n    repo: bddkit/echo-plugin\n    abi:\n      \"4\": 0.1.0\n      \"6\": 0.2.0\n";

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
        let root = scratch(name);
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

/// Echo 0.1.0 installed for ABI 4 and a 0.2.0 for ABI 6 published, with a
/// bddkit 99.0.0 for ABI 6 as the latest release.
fn abi_upgrade_ready(sandbox: &Sandbox, github: &str, stub: &Stub, check_exit: i32) {
    sandbox.install_echo(github, stub);
    stub.index(ABI_INDEX);
    stub.publish("bddkit/echo-plugin", "0.2.0");
    stub.publish_bddkit("99.0.0", 6, check_exit);
}

const NEEDS_ECHO: &str = "echo '{\"plugins\":[{\"name\":\"echo\",\"repo\":\"bddkit/echo-plugin\",\"later\":1}],\"x\":2}'";

const GROUPED_ABI_INDEX: &str = "plugin:\n  - name: echo\n    repo: bddkit/echo-plugin\n    groups: [echo]\n    abi:\n      \"4\": 0.1.0\n      \"6\": 0.2.0\n";

/// A suite in the sandbox's working directory that uses `echo`, found the way `run` finds it.
fn sandbox_suite(sandbox: &Sandbox) {
    suite_config(&sandbox.root, "");
}

fn payloads(dir: &Path) -> Vec<serde_json::Value> {
    common::read_payloads(&dir.join("t.log"))
}
