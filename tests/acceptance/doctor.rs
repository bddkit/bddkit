use super::write_doctor_project;
use serde_json::Value;
use std::process::Command;

/// `doctor` reaches every check a run makes before its first request, and a
/// bare invocation opens no socket at all.
#[test]
fn doctor_reports_a_healthy_suite_and_exits_zero() {
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config", "tests/acceptance.yaml"])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(0),
        "a sound suite is clean\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(stdout.contains("APP_ENV:"), "{stdout}");
    assert!(
        stdout.contains("--live"),
        "a static run must say what it did not do:\n{stdout}"
    );
}

#[test]
fn doctor_names_the_file_and_line_of_an_undefined_step() {
    let cfg = write_doctor_project(
        "doctor-step",
        "Feature: only\n  Scenario: one\n    When I frobnicate\n",
        "",
    );

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config", cfg.to_str().expect("path is UTF-8")])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("only.feature:3"), "{stdout}");
    assert!(stdout.contains("I frobnicate"), "{stdout}");
}

const THREE_BAD_STEPS: &str = "Feature: only\n  Scenario: one\n    When I frobnicate\n    Then I sleep a while\n    And I sleep abc seconds\n";

#[test]
fn doctor_reports_one_finding_per_bad_step() {
    let cfg = write_doctor_project("doctor-three", THREE_BAD_STEPS, "");
    let cfg = cfg.to_str().expect("path is UTF-8");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config", cfg])
        .output()
        .expect("failed to run bddkit");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    for line in [3, 4, 5] {
        assert!(stdout.contains(&format!("only.feature:{line}")), "{stdout}");
    }
    assert!(stdout.contains("3 problem(s)"), "{stdout}");
    assert!(
        stdout.contains("did you mean: I sleep <seconds> seconds"),
        "{stdout}"
    );
    assert!(
        stdout.contains(r#"parameter <seconds> expects uint, got "abc""#),
        "{stdout}"
    );

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--json", "--config", cfg])
        .output()
        .expect("failed to run bddkit");
    let report: Value = serde_json::from_slice(&out.stdout).expect("--json emits JSON only");
    let failed = report["checks"]
        .as_array()
        .expect("checks is an array")
        .iter()
        .filter(|c| c["stage"] == "steps" && c["status"] == "failed")
        .count();
    assert_eq!(failed, 3, "{report}");
}

#[test]
fn run_prints_the_suggestion_for_an_unknown_step() {
    let cfg = write_doctor_project("run-three", THREE_BAD_STEPS, "");
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config", cfg.to_str().expect("path is UTF-8")])
        .output()
        .expect("failed to run bddkit");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("did you mean: I sleep <seconds> seconds"),
        "{stderr}"
    );
}

/// A caller and a helper side by side; returns `doctor`'s exit code and stdout.
fn doctor_include_pair(name: &str, caller: &str, helper: &str) -> (Option<i32>, String) {
    let cfg = write_doctor_project(name, caller, "");
    let features = cfg.parent().expect("config has a parent").join("features");
    std::fs::write(features.join("helper.feature"), helper).expect("write helper");
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config", cfg.to_str().expect("path is UTF-8")])
        .output()
        .expect("failed to run bddkit");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

#[test]
fn doctor_hints_at_an_export_the_helper_never_sets() {
    let (code, stdout) = doctor_include_pair(
        "doctor-include-export",
        "Feature: only\n  Scenario: one\n    Given I include \"helper.feature\"\n",
        "Feature: helper\n  @exports(team_idd)\n  Scenario: make\n    Given set variable \"team_id\" to \"1\"\n",
    );
    assert_eq!(code, Some(0), "{stdout}");
    assert!(stdout.contains("helper.feature:3"), "{stdout}");
    assert!(stdout.contains("team_idd"), "{stdout}");
}

/// A scheduling tag `run` refuses to parse is a "nothing ran" failure like any
/// other, so `doctor` has to see it. It is the one pre-run check that lives
/// past `validate::check`, in `runner::build_chains`.
#[test]
fn doctor_reports_a_malformed_scheduling_tag() {
    let cfg = write_doctor_project(
        "doctor-tag",
        "Feature: only\n  @priority(soon)\n  Scenario: one\n    Given set variable \"ping\" to \"ok\"\n",
        "",
    );

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config", cfg.to_str().expect("path is UTF-8")])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("@priority(soon)"), "{stdout}");
}

/// `run` exits 2 on a selection with nothing in it. A green tick reading
/// "0 file(s), every step matched" is the most misleading line the command
/// could print, because it certifies a suite that cannot run.
#[test]
fn doctor_reports_a_suite_with_no_scenario_to_run() {
    let cfg = write_doctor_project("doctor-empty", "", "");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config", cfg.to_str().expect("path is UTF-8")])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("no scenario"), "{stdout}");
}

/// The decision a script depends on: `doctor` answers 0 or 1 and never 2, so
/// even a config it cannot parse comes back as an ordinary finding. `run`
/// exits 2 for the same file.
#[test]
fn doctor_reports_an_unparseable_config_as_an_ordinary_finding() {
    let dir = std::env::temp_dir().join(format!("bddkit-doctor-broken-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let cfg = dir.join("cfg.yaml");
    std::fs::write(
        &cfg,
        "paths: [features]\nresources:\n  api: {}\nmacro_paths: [${BDDKIT_ABSENT_VAR}]\n",
    )
    .expect("write config");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config", cfg.to_str().expect("path is UTF-8")])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(1),
        "a broken config is a finding, not a different exit currency\n{stdout}"
    );
    assert!(stdout.contains("BDDKIT_ABSENT_VAR"), "{stdout}");
}

/// The primary caller is a script or an agent, which is what `--json` is for.
#[test]
fn doctor_json_carries_the_env_the_status_and_every_check() {
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config", "tests/acceptance.yaml", "--json"])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let report: Value = serde_json::from_str(&stdout).expect("--json emits JSON only");
    assert_eq!(report["app_env"], "dev", "{stdout}");
    assert_eq!(report["live"], false, "{stdout}");
    assert_eq!(
        report["config_source"], "flag",
        "a structured field, not just the `(--config)` suffix on `config`: {stdout}"
    );
    let checks = report["checks"].as_array().expect("checks is an array");
    assert!(
        checks
            .iter()
            .any(|c| c["stage"] == "steps" && c["status"] == "ok"),
        "{stdout}"
    );
}

/// The flat `bddkit --config x.yaml` form is gone: `steps` has to be a real
/// subcommand, and clap cannot have both a positional path list at the top
/// level and subcommands to disambiguate it against.
#[test]
fn the_run_subcommand_is_how_a_suite_is_started() {
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["--config", "tests/acceptance.yaml"])
        .output()
        .expect("run bddkit");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "the flat form is a usage error");
    assert!(
        stderr.contains("unexpected argument"),
        "the flat form must be refused by the parser, not started:\n{stderr}"
    );

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--help"])
        .output()
        .expect("run bddkit");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains("Usage: bddkit run"), "{stdout}");
    assert!(
        stdout.contains("--fail-fast"),
        "run keeps every flag the flat form had:\n{stdout}"
    );
}

/// Runs `doctor` over a project whose macro file declares `templates`, one per
/// line of the file, with no plugin installed.
fn doctor_over_macros(name: &str, templates: &[&str], args: &[&str]) -> (Option<i32>, String) {
    let dir = std::env::temp_dir().join(format!("bddkit-{name}-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("no-plugins")).expect("mkdir");
    let macros = dir.join("macros.yaml");
    let source = templates
        .iter()
        .map(|template| format!("- {{step: '{template}', do: [Show all variables]}}\n"))
        .collect::<Vec<_>>()
        .concat();
    std::fs::write(&macros, source).expect("write macros");
    let cfg = write_doctor_project(
        name,
        "Feature: only\n  Scenario: one\n    When set variable \"x\" to \"1\"\n",
        &format!(
            "macro_paths: [{}]\n",
            macros.display().to_string().replace('\\', "/")
        ),
    );
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .arg("doctor")
        .args(args)
        .arg("--config")
        .arg(cfg)
        .env("BDDKIT_DIR", dir.join("no-plugins"))
        .output()
        .expect("failed to run bddkit");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

/// The `--json` rows with the `warning` status.
fn warnings(json: &str) -> Vec<Value> {
    let report: Value = serde_json::from_str(json).expect("--json emits JSON only");
    report["checks"]
        .as_array()
        .expect("checks is an array")
        .iter()
        .filter(|check| check["status"] == "warning")
        .cloned()
        .collect()
}

#[test]
fn doctor_warns_about_an_untyped_parameter_in_quotes_in_a_macro() {
    let (_, stdout) = doctor_over_macros(
        "doctor-untyped",
        &["I note it", r#"I open "{session}" as "{user}""#],
        &[],
    );
    assert!(stdout.contains("! macros "), "{stdout}");
    assert!(stdout.contains("macros.yaml:2"), "{stdout}");
    assert!(
        stdout.contains(r#"I open "{session}" as "{user}""#),
        "{stdout}"
    );
    for fix in [
        r#"declare it "{session:text}""#,
        r#"declare it "{user:text}""#,
    ] {
        assert!(stdout.contains(fix), "{stdout}");
    }
}

#[test]
fn doctor_does_not_warn_about_a_typed_parameter_in_quotes() {
    let (code, stdout) = doctor_over_macros(
        "doctor-typed",
        &[r#"I open "{session:text}" as "{user:any}""#],
        &["--json"],
    );
    assert_eq!(code, Some(0), "{stdout}");
    assert_eq!(warnings(&stdout), Vec::<Value>::new(), "{stdout}");
}

#[test]
fn doctor_does_not_warn_about_an_untyped_parameter_outside_quotes() {
    // Bare, inside a longer literal, and between the end of one literal and the
    // start of the next.
    let (code, stdout) = doctor_over_macros(
        "doctor-unquoted",
        &[
            "I open {session}",
            r#"I close "the {door} now""#,
            r#"I join "a"{middle}"b""#,
        ],
        &["--json"],
    );
    assert_eq!(code, Some(0), "{stdout}");
    assert_eq!(warnings(&stdout), Vec::<Value>::new(), "{stdout}");
}

#[test]
fn a_warning_does_not_change_the_exit_code() {
    let (code, stdout) = doctor_over_macros("doctor-warn-code", &[r#"I open "{session}""#], &[]);
    assert_eq!(code, Some(0), "{stdout}");
    assert!(stdout.contains("! macros "), "{stdout}");
    assert!(
        stdout.contains("no problems found, 1 warning(s)\n"),
        "the last lines must not hide the warning:\n{stdout}"
    );
}

#[test]
fn doctor_json_reports_the_warning_status() {
    let (_, stdout) =
        doctor_over_macros("doctor-warn-json", &[r#"I open "{session}""#], &["--json"]);
    let rows = warnings(&stdout);
    assert_eq!(rows.len(), 1, "{stdout}");
    assert_eq!(rows[0]["stage"], "macros", "{stdout}");
    let target = rows[0]["target"].as_str().expect("a target");
    assert!(target.ends_with("macros.yaml:1"), "{target}");
}

/// The conflict an untyped quoted parameter causes is the case the hint is for.
#[test]
fn doctor_warns_about_an_untyped_parameter_when_the_macros_conflict() {
    let (code, stdout) = doctor_over_macros(
        "doctor-warn-conflict",
        &[r#"I send "{what}""#, r#"I send "{what}" for "{whom}""#],
        &[],
    );
    assert_eq!(code, Some(1), "{stdout}");
    assert!(stdout.contains("✗ macros"), "{stdout}");
    assert!(stdout.contains(r#"declare it "{whom:text}""#), "{stdout}");
}
