use super::write_doctor_project;
use std::process::Command;

/// Writes a healthy doctor project (see `write_doctor_project`) under a fresh
/// directory whose name is the default config file (`bddkit.yaml` or
/// `bddkit.yml`), and returns that directory — the default-discovery tests
/// run the binary with `current_dir(dir)` and no `--config` at all.
fn write_default_config_project(name: &str, config_file_name: &str) -> std::path::PathBuf {
    let cfg = write_doctor_project(
        name,
        "Feature: only\n  Scenario: one\n    Given set variable \"ping\" to \"ok\"\n",
        "",
    );
    let dir = cfg.parent().expect("cfg has a parent").to_path_buf();
    std::fs::rename(&cfg, dir.join(config_file_name)).expect("rename to default config name");
    dir
}

#[test]
fn doctor_picks_up_bddkit_yaml_from_the_working_directory() {
    let dir = write_default_config_project("doctor-default-yaml", "bddkit.yaml");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .arg("doctor")
        .current_dir(&dir)
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(
        stdout.contains("bddkit.yaml (found in working directory)"),
        "{stdout}"
    );
}

#[test]
fn doctor_picks_up_the_yml_spelling_too() {
    let dir = write_default_config_project("doctor-default-yml", "bddkit.yml");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .arg("doctor")
        .current_dir(&dir)
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(
        stdout.contains("bddkit.yml (found in working directory)"),
        "{stdout}"
    );
}

#[test]
fn doctor_reports_both_default_spellings_present_as_a_failed_config_row() {
    let dir = write_default_config_project("doctor-default-both", "bddkit.yaml");
    std::fs::copy(dir.join("bddkit.yaml"), dir.join("bddkit.yml")).expect("copy config");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .arg("doctor")
        .current_dir(&dir)
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("bddkit.yaml"), "{stdout}");
    assert!(stdout.contains("bddkit.yml"), "{stdout}");
}

#[test]
fn doctor_reports_no_config_found_as_a_failed_config_row_not_exit_two() {
    let dir =
        std::env::temp_dir().join(format!("bddkit-doctor-default-none-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    // A previous run of this test binary may have left a config behind.
    let _ = std::fs::remove_file(dir.join("bddkit.yaml"));
    let _ = std::fs::remove_file(dir.join("bddkit.yml"));
    let _ = std::fs::remove_file(dir.join("junit.xml"));

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--junit", "junit.xml"])
        .current_dir(&dir)
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "doctor never exits 2: {stdout}");
    assert!(stdout.contains("no config found"), "{stdout}");
    // `run` prepares its report paths before it resolves the config, so
    // `doctor` must too — even when there is no config to resolve.
    assert!(stdout.contains("reports"), "{stdout}");
    assert!(dir.join("junit.xml").is_file(), "{stdout}");
}

#[test]
fn bddkit_config_env_var_is_honoured_when_no_flag_is_given() {
    let cfg = write_doctor_project(
        "doctor-default-env",
        "Feature: only\n  Scenario: one\n    Given set variable \"ping\" to \"ok\"\n",
        "",
    );

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .arg("doctor")
        .env("BDDKIT_CONFIG", &cfg)
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(stdout.contains("(BDDKIT_CONFIG)"), "{stdout}");
}

/// A `--config` naming a file that does not exist must never quietly fall
/// through to a `bddkit.yaml` that happens to sit in the working directory —
/// resolution order step 1 wins outright, existence or not.
#[test]
fn an_explicit_missing_config_flag_is_refused_not_a_fallback_to_the_default() {
    let dir = write_default_config_project("doctor-explicit-missing", "bddkit.yaml");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config", "does-not-exist.yaml"])
        .current_dir(&dir)
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(
        stdout.contains("does-not-exist.yaml"),
        "names the file it was told, not the default: {stdout}"
    );
    assert!(
        !stdout.contains("no problems found"),
        "must not silently succeed against bddkit.yaml instead: {stdout}"
    );
}

/// Same guarantee, for `$BDDKIT_CONFIG` — resolution order step 2.
#[test]
fn a_missing_bddkit_config_env_var_is_refused_not_a_fallback_to_the_default() {
    let dir = write_default_config_project("doctor-env-missing", "bddkit.yaml");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .arg("doctor")
        .env("BDDKIT_CONFIG", "does-not-exist.yaml")
        .current_dir(&dir)
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(
        stdout.contains("does-not-exist.yaml"),
        "names the file it was told, not the default: {stdout}"
    );
    assert!(
        !stdout.contains("no problems found"),
        "must not silently succeed against bddkit.yaml instead: {stdout}"
    );
}

/// Resolution order step 1 over step 2: an explicit `--config` beats
/// `$BDDKIT_CONFIG` even when both name a real, different, healthy project.
#[test]
fn explicit_config_flag_wins_over_the_env_var() {
    let winner = write_doctor_project(
        "doctor-precedence-flag",
        "Feature: only\n  Scenario: one\n    Given set variable \"ping\" to \"ok\"\n",
        "",
    );
    let loser = write_doctor_project(
        "doctor-precedence-env",
        "Feature: only\n  Scenario: one\n    Given set variable \"ping\" to \"ok\"\n",
        "",
    );

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "doctor",
            "--config",
            winner.to_str().expect("path is UTF-8"),
        ])
        .env("BDDKIT_CONFIG", &loser)
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(
        stdout.contains(&format!("{} (--config)", winner.display())),
        "the flag's path and source, not the env var's: {stdout}"
    );
}

#[test]
fn run_with_no_config_anywhere_exits_two_naming_the_files_it_looked_for() {
    let dir = std::env::temp_dir().join(format!("bddkit-run-default-none-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let _ = std::fs::remove_file(dir.join("bddkit.yaml"));
    let _ = std::fs::remove_file(dir.join("bddkit.yml"));
    let _ = std::fs::remove_file(dir.join("junit.xml"));

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--junit", "junit.xml"])
        .current_dir(&dir)
        .output()
        .expect("failed to run bddkit");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("bddkit.yaml"), "{stderr}");
    assert!(stderr.contains("bddkit.yml"), "{stderr}");
    // An empty report a CI parser rejects loudly, never yesterday's green one.
    assert!(dir.join("junit.xml").is_file(), "{stderr}");
}

#[test]
fn resource_add_with_no_config_anywhere_exits_one() {
    let dir = std::env::temp_dir().join(format!("bddkit-add-default-none-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let _ = std::fs::remove_file(dir.join("bddkit.yaml"));
    let _ = std::fs::remove_file(dir.join("bddkit.yml"));

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["resource", "add", "api", "staging"])
        .current_dir(&dir)
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("no config found"), "{stdout}");
}

/// `--no-config` must not even attempt to load `bddkit.yaml`, so a broken one
/// does not block the simplest possible vocabulary lookup — this is the
/// scenario the flag exists for (see issue #48).
#[test]
fn steps_list_no_config_ignores_a_present_bddkit_yaml() {
    let dir = std::env::temp_dir().join(format!("bddkit-steps-no-config-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join("bddkit.yaml"), "not: [valid, yaml: at all").expect("write");

    let without_flag = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["steps", "list"])
        .current_dir(&dir)
        .output()
        .expect("failed to run bddkit");
    assert_eq!(
        without_flag.status.code(),
        Some(2),
        "the broken default config is picked up and fails: {}",
        String::from_utf8_lossy(&without_flag.stdout)
    );

    let with_flag = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["steps", "list", "--no-config"])
        .current_dir(&dir)
        .output()
        .expect("failed to run bddkit");
    let stdout = String::from_utf8_lossy(&with_flag.stdout);
    assert_eq!(with_flag.status.code(), Some(0), "{stdout}");
    assert!(stdout.contains("I sleep"), "{stdout}");
}

/// clap's `conflicts_with` on the real binary, not just declared in
/// `main.rs` — for both commands that carry the flag.
#[test]
fn steps_list_refuses_config_and_no_config_together() {
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["steps", "list", "--config", "x.yaml", "--no-config"])
        .output()
        .expect("failed to run bddkit");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("cannot be used with"), "{stderr}");
}

#[test]
fn resource_fields_refuses_config_and_no_config_together() {
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["resource", "fields", "--config", "x.yaml", "--no-config"])
        .output()
        .expect("failed to run bddkit");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("cannot be used with"), "{stderr}");
}

/// The first question an agent asks a binary on `PATH`. `version` and
/// `--version` must be the same answer, and its first line must stay the one
/// line a script greps.
#[test]
fn version_answers_to_both_spellings_and_leads_with_the_semver() {
    let subcommand = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .arg("version")
        .output()
        .expect("failed to run bddkit");
    let flag = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .arg("--version")
        .output()
        .expect("failed to run bddkit");

    let short = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .arg("-V")
        .output()
        .expect("failed to run bddkit");

    assert_eq!(subcommand.status.code(), Some(0));
    assert_eq!(flag.status.code(), Some(0));
    assert_eq!(short.status.code(), Some(0));

    let text = String::from_utf8_lossy(&subcommand.stdout);
    assert_eq!(text, String::from_utf8_lossy(&flag.stdout), "same answer");

    let first = text.lines().next().expect("version prints something");
    assert_eq!(
        first,
        format!("bddkit {}", env!("CARGO_PKG_VERSION")),
        "the first line is the binary and its version, nothing else"
    );
    assert!(text.contains("bddkit steps list"), "signposts: {text}");
    assert!(text.contains("bddkit doctor"), "signposts: {text}");

    // The whole point of the `version` / `long_version` split: a one-token
    // edit that feeds the long form to `-V` too breaks every script grepping
    // it, and every assertion above would stay green.
    assert_eq!(
        String::from_utf8_lossy(&short.stdout),
        format!("bddkit {}\n", env!("CARGO_PKG_VERSION")),
        "-V stays the one line a script greps"
    );
}
