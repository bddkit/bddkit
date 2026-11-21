//! `bddkit ui` on a machine without `bddkit-ui` (issue #287): the offer, the
//! download from the stub's `bddkit/bddkit-ui-releases`, and what lands next to
//! the binary. The prompt needs a terminal, so the answering tests run under
//! util-linux `script`, which gives the child a real pty; BSD `script` takes
//! other flags, hence Linux only.

use super::{Reply, Stub, common, host_target, pack_files, spawn, text};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const REPO: &str = "bddkit/bddkit-ui-releases";

/// A directory holding a hard link of bddkit: the UI is installed next to the
/// running binary, which must never be the one shared by every test.
struct Home(PathBuf);

impl Home {
    fn new(name: &str) -> Self {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("bddkit-ui-install-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("bin")).expect("mkdir");
        let (from, to) = (env!("CARGO_BIN_EXE_bddkit"), dir.join("bin/bddkit"));
        std::fs::hard_link(from, &to)
            .or_else(|_| std::fs::copy(from, &to).map(|_| ()))
            .expect("link or copy bddkit");
        Home(dir)
    }

    /// Everything in `bin/` but bddkit itself.
    fn others(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.0.join("bin"))
            .expect("read bin")
            .map(|entry| entry.expect("entry").file_name().to_string_lossy().into())
            .filter(|name| name != "bddkit")
            .collect();
        names.sort();
        names
    }

    fn command(&self, github: &str) -> Command {
        let mut command = Command::new(self.0.join("bin/bddkit"));
        common::isolate_telemetry(&mut command);
        // System directories only: a `bddkit-ui` of the developer's own must not be found.
        command
            .env("PATH", "/usr/bin:/bin")
            .env("BDDKIT_GITHUB_URL", github);
        command
    }

    /// `bddkit ui --flag` with no terminal.
    fn run(&self, github: &str) -> Output {
        self.command(github)
            .args(["ui", "--flag"])
            .stdin(Stdio::null())
            .output()
            .expect("run")
    }

    /// The same under a pty, answering the prompt with `answer`.
    #[cfg(target_os = "linux")]
    fn run_tty(&self, github: &str, answer: &str) -> Output {
        self.run_tty_with(github, answer, &[])
    }

    /// The same with `envs` set on top, to put a CI variable back on purpose.
    #[cfg(target_os = "linux")]
    fn run_tty_with(&self, github: &str, answer: &str, envs: &[(&str, &str)]) -> Output {
        use std::io::Write;
        let mut script = Command::new("script");
        common::isolate_telemetry(&mut script);
        // `timeout`: a prompt that waits for more than one line must fail, not hang.
        script
            .args([
                "-qec",
                &format!(
                    "timeout 60 {} ui --flag",
                    self.0.join("bin/bddkit").display()
                ),
            ])
            .envs(envs.iter().copied())
            .arg("/dev/null")
            .env("PATH", "/usr/bin:/bin")
            .env("BDDKIT_GITHUB_URL", github)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = script.spawn().expect("script");
        let mut stdin = child.stdin.take().expect("stdin");
        stdin.write_all(answer.as_bytes()).expect("answer");
        // Held open until the child ends: `script` stops on end of input.
        let out = child.wait_with_output().expect("wait");
        drop(stdin);
        out
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `v<version>` of the UI as the latest release: `bddkit-ui` prints its arguments.
/// `sha` overrides the checksum, so a release can be corrupt.
fn publish(stub: &Stub, version: &str, sha: Option<&str>) {
    let asset = format!("bddkit-ui-v{version}-{}.tar.gz", host_target());
    let archive = pack_files(
        &format!("bddkit-ui-v{version}-{}", host_target()),
        &[("bddkit-ui", b"#!/bin/sh\necho \"ui-ran $*\"\n")],
    );
    let digest = sha.map_or_else(|| format!("{:x}", Sha256::digest(&archive)), str::to_string);
    let base = format!("/{REPO}/releases/download/v{version}/{asset}");
    stub.set(&base, Reply::Body(archive));
    stub.set(
        &format!("{base}.sha256"),
        Reply::Body(format!("{digest}  {asset}\n").into_bytes()),
    );
    stub.latest(REPO, version);
    stub.set(
        &format!("/{REPO}/releases/tag/v{version}"),
        Reply::Body(b"release".to_vec()),
    );
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn yes_installs_the_ui_next_to_bddkit_and_runs_it() {
    let (stub, github) = spawn().await;
    publish(&stub, "1.2.3", None);
    let home = Home::new("yes");
    let out = home.run_tty(&github, "y\n");
    let shown = text(&out);
    assert!(out.status.success(), "{shown}");
    assert!(shown.contains("closed-source"), "{shown}");
    assert!(shown.contains("Install it? [y/N]"), "{shown}");
    assert!(shown.contains("ui-ran --flag"), "{shown}");
    assert_eq!(home.others(), ["bddkit-ui"], "no staging directory is left");

    // Found beside the binary from now on: no terminal, no question.
    let out = home.run(&github);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("ui-ran --flag"), "{}", text(&out));
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_installs_nothing_and_exits_2() {
    let (stub, github) = spawn().await;
    publish(&stub, "1.2.3", None);
    let home = Home::new("no");
    for answer in ["n\n", "\n"] {
        let out = home.run_tty(&github, answer);
        assert_eq!(out.status.code(), Some(2), "{}", text(&out));
        assert!(text(&out).contains("was not installed"), "{}", text(&out));
        assert!(home.others().is_empty());
    }
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bad_checksum_installs_nothing() {
    let (stub, github) = spawn().await;
    publish(&stub, "1.2.3", Some(&"0".repeat(64)));
    let home = Home::new("checksum");
    let out = home.run_tty(&github, "y\n");
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    assert!(text(&out).contains("checksum mismatch"), "{}", text(&out));
    assert!(home.others().is_empty(), "{:?}", home.others());
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_release_without_a_build_for_this_host_exits_2() {
    let (stub, github) = spawn().await;
    // The release exists; its archive for this target does not.
    stub.latest(REPO, "1.2.3");
    stub.set(
        &format!("/{REPO}/releases/tag/v1.2.3"),
        Reply::Body(b"release".to_vec()),
    );
    let home = Home::new("nobuild");
    let out = home.run_tty(&github, "y\n");
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    assert!(
        text(&out).contains(&format!("has no build for {}", host_target())),
        "{}",
        text(&out)
    );
    assert!(home.others().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_a_terminal_it_says_how_to_install_and_asks_the_network_nothing() {
    // No stub behind the URL: any request would fail with a different message.
    let home = Home::new("notty");
    let out = home.run("http://127.0.0.1:9");
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    assert!(
        text(&out).contains("run `bddkit ui` in an interactive terminal"),
        "{}",
        text(&out)
    );
    assert!(home.others().is_empty());
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_ci_variable_stops_the_question_even_on_a_terminal() {
    let (stub, github) = spawn().await;
    publish(&stub, "1.2.3", None);
    let home = Home::new("ci");
    let out = home.run_tty_with(&github, "y\n", &[("CI", "true")]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    assert!(
        text(&out).contains("run `bddkit ui` in an interactive terminal"),
        "{}",
        text(&out)
    );
    assert!(!text(&out).contains("Install it?"), "{}", text(&out));
    assert!(home.others().is_empty());
}
