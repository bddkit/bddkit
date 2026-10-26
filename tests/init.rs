//! `bddkit init` without a terminal: every answer from a flag. The prompts are a thin layer
//! over `dialoguer` that nobody unit-tests; what they produce goes through the same
//! `from_flags` and `apply`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bddkit-init-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

/// `bddkit <args>` in `dir`, with no terminal and nothing of this machine's
/// plugins, config or CI environment.
fn bddkit(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .env("HOME", dir)
        .env("XDG_CONFIG_HOME", dir.join("xdg-config"))
        .env("XDG_DATA_HOME", dir.join("xdg-data"))
        .env_remove("BDDKIT_CONFIG")
        .env_remove("BDDKIT_DIR")
        .env_remove("CI")
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

fn config(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("bddkit.yaml")).expect("bddkit.yaml written")
}

#[test]
fn init_with_flags_writes_a_config_that_doctor_accepts() {
    let dir = temp("doctor");
    let out = bddkit(
        &dir,
        &[
            "init",
            "--api-url",
            "http://127.0.0.1:8080",
            "--db",
            "postgres",
            "--dsn",
            "postgres://user:pass@localhost/app",
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let config = config(&dir);
    assert!(
        config.contains("base_url: http://127.0.0.1:8080"),
        "{config}"
    );
    assert!(
        config.contains("dsn: postgres://user:pass@localhost/app"),
        "{config}"
    );
    assert!(
        config.contains("# "),
        "every key carries a comment:\n{config}"
    );
    let doctor = bddkit(&dir, &["doctor"]);
    assert_eq!(doctor.status.code(), Some(0), "{}", text(&doctor));
}

#[test]
fn the_example_feature_passes_without_a_network() {
    let dir = temp("example");
    // Nothing listens on this port: the request in the feature is a comment.
    let out = bddkit(&dir, &["init", "--api-url", "http://127.0.0.1:9"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let feature = std::fs::read_to_string(dir.join("features/example.feature")).expect("feature");
    assert!(feature.contains("# When I request \"/\""), "{feature}");
    let run = bddkit(&dir, &["run"]);
    assert_eq!(run.status.code(), Some(0), "{}", text(&run));
}

#[test]
fn init_refuses_when_a_config_exists_and_names_the_alternatives() {
    let dir = temp("refuse");
    std::fs::write(dir.join("bddkit.yml"), "paths: [x]\nresources: {api: {}}\n").unwrap();
    let out = bddkit(&dir, &["init"]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    assert!(text(&out).contains("resource add"), "{}", text(&out));
    assert!(text(&out).contains("plugin install"), "{}", text(&out));
    assert!(!dir.join("features").exists());
    assert_eq!(
        std::fs::read_to_string(dir.join("bddkit.yml")).unwrap(),
        "paths: [x]\nresources: {api: {}}\n"
    );
}

#[test]
fn an_empty_api_answer_writes_no_api_resource() {
    let dir = temp("no-api");
    let out = bddkit(&dir, &["init", "--no-api"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(!config(&dir).contains("base_url"), "{}", config(&dir));
    let feature = std::fs::read_to_string(dir.join("features/example.feature")).unwrap();
    assert!(!feature.contains("request"), "{feature}");
    // No flags at all means the same.
    let bare = temp("bare");
    assert_eq!(bddkit(&bare, &["init"]).status.code(), Some(0));
    assert!(!config(&bare).contains("base_url"));
}

#[test]
fn parallel_false_writes_concurrency_1() {
    let dir = temp("serial");
    let out = bddkit(&dir, &["init", "--parallel", "false"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(config(&dir).contains("concurrency: 1"), "{}", config(&dir));
    let default = temp("parallel");
    assert_eq!(bddkit(&default, &["init"]).status.code(), Some(0));
    assert!(
        !config(&default).contains("concurrency"),
        "{}",
        config(&default)
    );
}

#[test]
fn a_non_interactive_init_without_telemetry_writes_no_key() {
    let dir = temp("telemetry-none");
    assert_eq!(bddkit(&dir, &["init"]).status.code(), Some(0));
    assert!(!config(&dir).contains("telemetry"), "{}", config(&dir));
    let answered = temp("telemetry-no");
    let out = bddkit(&answered, &["init", "--telemetry", "false"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(
        config(&answered).contains("telemetry: false"),
        "{}",
        config(&answered)
    );
}

#[test]
fn mismatched_flags_are_refused() {
    for args in [
        &["init", "--dsn", "postgres://x/y"][..],
        &["init", "--db", "none", "--dsn", "postgres://x/y"],
        &["init", "--db", "postgres"],
        &["init", "--layer", "project"],
        &["init", "--api-url", "http://x", "--no-api"],
        &["init", "--api-url", ""],
        &[
            "init",
            "--plugin",
            "x",
            "--layer",
            "project",
            "--bddkit-dir",
            ".",
        ],
    ] {
        let dir = temp("mismatch");
        let out = bddkit(&dir, args);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", text(&out));
        assert!(!dir.join("bddkit.yaml").exists(), "{args:?}");
        assert!(!dir.join("features").exists(), "{args:?}");
    }
}

#[test]
fn gitignore_gains_only_missing_lines_and_only_in_a_git_work_tree() {
    // `.git` as a file is what a linked worktree has.
    let dir = temp("gitignore");
    std::fs::write(dir.join(".git"), "gitdir: elsewhere\n").unwrap();
    std::fs::write(dir.join(".gitignore"), "target\n.env.local").unwrap();
    assert_eq!(bddkit(&dir, &["init"]).status.code(), Some(0));
    assert_eq!(
        std::fs::read_to_string(dir.join(".gitignore")).unwrap(),
        "target\n.env.local\n.bddkit/*.local.yaml\n"
    );

    let created = temp("gitignore-new");
    std::fs::create_dir(created.join(".git")).unwrap();
    assert_eq!(bddkit(&created, &["init"]).status.code(), Some(0));
    assert_eq!(
        std::fs::read_to_string(created.join(".gitignore")).unwrap(),
        ".bddkit/*.local.yaml\n.env.local\n"
    );

    let outside = temp("gitignore-none");
    assert_eq!(bddkit(&outside, &["init"]).status.code(), Some(0));
    assert!(!outside.join(".gitignore").exists());
}

#[test]
fn a_dollar_typed_in_a_value_reaches_the_run_unchanged() {
    // The config is `$`-expanded when it is read: `$$` must survive the
    // round trip, and a `${NAME}` reference must stay a reference.
    let dir = temp("dollar");
    let out = bddkit(
        &dir,
        &[
            "init",
            "--api-url",
            "http://127.0.0.1:9/?q=$id&p=$$",
            "--db",
            "postgres",
            "--dsn",
            "postgres://u:pa$$w@h/db?x=${BDDKIT_INIT_TEST_UNSET:-d}",
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let doctor = bddkit(&dir, &["doctor", "--json"]);
    assert_eq!(doctor.status.code(), Some(0), "{}", text(&doctor));
    assert!(
        text(&doctor).contains("http://127.0.0.1:9/?q=$id&p=$$"),
        "{}",
        text(&doctor)
    );
    assert!(config(&dir).contains("pa$$$$w"), "{}", config(&dir));
    assert!(
        config(&dir).contains("${BDDKIT_INIT_TEST_UNSET:-d}"),
        "{}",
        config(&dir)
    );
}

#[test]
fn a_dsn_of_another_engine_is_refused() {
    let dir = temp("scheme");
    let out = bddkit(
        &dir,
        &["init", "--db", "mysql", "--dsn", "postgres://u:p@h/db"],
    );
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    assert!(!dir.join("bddkit.yaml").exists());
    let out = bddkit(
        &dir,
        &["init", "--db", "mariadb", "--dsn", "mysql://u:p@h/db"],
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
}

#[test]
fn an_explicit_config_beside_the_other_spelling_is_refused() {
    let dir = temp("sibling");
    std::fs::write(dir.join("bddkit.yml"), "paths: [x]\nresources: {api: {}}\n").unwrap();
    let out = bddkit(&dir, &["init", "--config", "bddkit.yaml"]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    assert!(!dir.join("bddkit.yaml").exists());
}

#[test]
fn a_gitignore_that_cannot_be_read_as_text_is_left_alone() {
    let dir = temp("gitignore-latin1");
    std::fs::create_dir(dir.join(".git")).unwrap();
    let original = b"target\n# caf\xe9\nsecret.txt\n";
    std::fs::write(dir.join(".gitignore"), original).unwrap();
    let out = bddkit(&dir, &["init"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert_eq!(std::fs::read(dir.join(".gitignore")).unwrap(), original);
}

#[test]
fn the_config_is_written_atomically() {
    let dir = temp("atomic");
    assert_eq!(bddkit(&dir, &["init"]).status.code(), Some(0));
    let leftovers: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".bddkit-tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");

    // A target that cannot be written leaves neither it nor a temporary file.
    let failing = temp("atomic-fail");
    let out = bddkit(&failing, &["init", "--config", "missing-dir/bddkit.yaml"]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    assert!(!failing.join("missing-dir").exists());
    assert!(!failing.join("features").exists());
}

#[test]
fn a_failure_after_the_config_is_written_is_exit_1_not_a_refusal() {
    let dir = temp("after-config");
    // `features` is a file: the example cannot be written.
    std::fs::write(dir.join("features"), "in the way").unwrap();
    let out = bddkit(&dir, &["init"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(dir.join("bddkit.yaml").is_file());
    assert!(!text(&out).contains("nothing written"), "{}", text(&out));
}
