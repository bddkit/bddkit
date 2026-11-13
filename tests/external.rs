//! `bddkit <name>` runs `bddkit-<name>` (issue #171). Unix only: the tools are
//! shell scripts, and the `exec` is proven by the child's PID.
#![cfg(unix)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    reason = "`allow-*-in-tests` covers only `#[test]` functions, not the helpers here"
)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::Mutex;

/// A script still open for writing in one test while another forks makes that
/// child's `exec` fail with "Text file busy"; writes and spawns take turns.
static FORK: Mutex<()> = Mutex::new(());

/// A directory holding a COPY of bddkit, so "beside the binary" is ours.
struct Sandbox(PathBuf);

impl Sandbox {
    fn new(name: &str) -> Self {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("bddkit-ext-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("bin")).expect("mkdir");
        std::fs::create_dir_all(dir.join("path")).expect("mkdir");
        let _turn = FORK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // A hard link opens no write fd and costs no 200 MB copy.
        let (from, to) = (env!("CARGO_BIN_EXE_bddkit"), dir.join("bin/bddkit"));
        std::fs::hard_link(from, &to)
            .or_else(|_| std::fs::copy(from, &to).map(|_| ()))
            .expect("link or copy bddkit");
        Sandbox(dir)
    }

    fn tool(dir: &Path, name: &str, body: &str) {
        let _turn = FORK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let file = dir.join(format!("bddkit-{name}"));
        std::fs::write(&file, format!("#!/bin/sh\n{body}\n")).expect("write tool");
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }

    fn beside(&self, name: &str, body: &str) {
        Self::tool(&self.0.join("bin"), name, body);
    }

    fn on_path(&self, name: &str, body: &str) {
        Self::tool(&self.0.join("path"), name, body);
    }

    fn spawn(&self, args: &[&str]) -> Child {
        let _turn = FORK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.command()
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn")
    }

    fn run(&self, args: &[&str]) -> Output {
        self.spawn(args).wait_with_output().expect("wait")
    }

    fn command(&self) -> Command {
        let mut command = Command::new(self.0.join("bin/bddkit"));
        // The system directories keep `sh` and its builtins reachable.
        command.env(
            "PATH",
            format!("{}:/usr/bin:/bin", self.0.join("path").display()),
        );
        command
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn an_unknown_command_runs_bddkit_dash_name_from_beside_the_binary() {
    let s = Sandbox::new("beside");
    s.beside("hello", "echo beside");
    let out = s.run(&["hello"]);
    assert!(out.status.success());
    assert_eq!(stdout(&out), "beside");
}

#[test]
fn path_is_searched_after_the_binary_directory() {
    let s = Sandbox::new("path");
    s.on_path("hello", "echo from-path");
    s.on_path("only", "echo only-on-path");
    s.beside("hello", "echo beside");
    let out = s.run(&["hello"]);
    assert_eq!(stdout(&out), "beside");
    let out = s.run(&["only"]);
    assert_eq!(stdout(&out), "only-on-path");
}

#[test]
fn a_builtin_name_is_never_looked_up() {
    let s = Sandbox::new("builtin");
    s.beside("version", "echo hijacked");
    let out = s.run(&["version"]);
    assert!(out.status.success());
    assert!(!stdout(&out).contains("hijacked"));
    assert!(stdout(&out).contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn arguments_and_exit_code_pass_through() {
    let s = Sandbox::new("args");
    s.beside("echoer", "printf '%s|' \"$@\"; exit 7");
    let out = s.run(&["echoer", "--flag", "two words", "-x"]);
    assert_eq!(out.status.code(), Some(7));
    assert_eq!(stdout(&out), "--flag|two words|-x|");
}

#[test]
fn bddkit_bin_names_the_launching_binary() {
    let s = Sandbox::new("env");
    s.beside("env", "echo \"$BDDKIT_BIN $BDDKIT_VERSION\"");
    let out = s.run(&["env"]);
    let expected = s.0.join("bin/bddkit").canonicalize().expect("canonical");
    assert_eq!(
        stdout(&out),
        format!("{} {}", expected.display(), env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn the_tool_replaces_the_process() {
    let s = Sandbox::new("exec");
    s.beside("pid", "echo $$");
    let child = s.spawn(&["pid"]);
    let pid = child.id();
    let out = child.wait_with_output().expect("wait");
    assert_eq!(stdout(&out), pid.to_string());
}

#[test]
fn a_missing_tool_keeps_the_clap_error_and_adds_a_hint() {
    let s = Sandbox::new("missing");
    let out = s.run(&["nothing-here"]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("unrecognized subcommand 'nothing-here'"),
        "{err}"
    );
    assert!(
        err.contains("no built-in command or `bddkit-nothing-here` on PATH"),
        "{err}"
    );
}

#[test]
fn a_name_that_could_leave_the_directory_is_not_looked_up() {
    let s = Sandbox::new("traversal");
    // `bddkit sub/x` would find `bddkit-sub/x` if the name were not refused.
    let sub = s.0.join("path/bddkit-sub");
    std::fs::create_dir_all(&sub).expect("mkdir");
    Sandbox::tool(&sub, "x", "echo escaped");
    std::fs::rename(sub.join("bddkit-x"), sub.join("x")).expect("rename");
    let out = s.run(&["sub/x"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(!stdout(&out).contains("escaped"));
}

#[test]
fn a_typo_of_a_builtin_keeps_clap_s_tip() {
    let s = Sandbox::new("typo");
    let out = s.run(&["lis"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("'list'"), "{err}");
    assert!(
        err.contains("no built-in command or `bddkit-lis` on PATH"),
        "{err}"
    );
}
