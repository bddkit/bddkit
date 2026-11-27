use super::{ECHO_GROUP, project};
use std::process::Command;

/// `bddkit steps list` without a config knows only the builtins — a plugin's
/// vocabulary lives inside a `cdylib` the host has to load to see at all.
#[test]
fn steps_list_shows_a_plugins_group_only_once_a_config_names_it() {
    let dir = project("steps-list", "Feature: f\n", ECHO_GROUP);

    let bare = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["steps", "list"])
        .current_dir(&dir)
        .output()
        .expect("run bddkit");
    let bare_out = String::from_utf8_lossy(&bare.stdout);
    assert_eq!(bare.status.code(), Some(0), "{bare_out}");
    assert!(
        !bare_out.contains("echo:"),
        "without --config no plugin is loaded, so it has no steps to show:\n{bare_out}"
    );

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["steps", "list", "echo", "--config", "cfg.yaml", "-v"])
        .current_dir(&dir)
        .output()
        .expect("run bddkit");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stdout}\n{stderr}");
    assert!(stdout.contains("echo:"), "{stdout}");
    assert!(
        stdout.contains(r#"I echo "<value>" as "<variable>""#),
        "a plugin's named groups render like a builtin's:\n{stdout}"
    );
    assert!(
        stdout.contains("echoes its argument"),
        "the description the plugin declared must be shown:\n{stdout}"
    );
    assert!(
        stdout.contains(r#"I use "<name>" echo"#),
        "the group-switch step exists once a plugin claims the group:\n{stdout}"
    );
    // The fixture's third step declares neither a name nor a description, which
    // is what an older plugin looks like: it must list, not break the listing.
    assert!(
        stdout.contains("  the echo should fail\n  "),
        "a step without a description is followed by the next step, not a blank line:\n{stdout}"
    );
}

#[test]
fn steps_list_shows_a_plugins_negative_step() {
    let dir = project("steps-list-negative", "Feature: f\n", ECHO_GROUP);
    let list = |extra: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
            .args(["steps", "list", "echo", "--config", "cfg.yaml"])
            .args(extra)
            .current_dir(&dir)
            .output()
            .expect("run bddkit");
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let text = list(&[]);
    assert!(
        text.contains("  the echo should not be heard  [assertion, negative]\n"),
        "{text}"
    );
    assert_eq!(text.matches("[assertion, negative]").count(), 1, "{text}");

    let rows: Vec<serde_json::Value> = serde_json::from_str(&list(&["--json"])).expect("JSON");
    for row in &rows {
        let marked = row["template"] == "the echo should not be heard";
        assert_eq!(row["negative"], marked, "{row}");
    }
}

#[test]
fn resource_fields_describes_a_plugins_group_only_with_a_config() {
    let dir = project("resource-fields", "Feature: f\n", ECHO_GROUP);

    let bare = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["resource", "fields"])
        .current_dir(&dir)
        .output()
        .expect("run bddkit");
    let bare_out = String::from_utf8_lossy(&bare.stdout);
    assert_eq!(bare.status.code(), Some(0), "{bare_out}");
    assert!(
        !bare_out.contains("echo:"),
        "without --config no plugin is loaded, so it describes nothing:\n{bare_out}"
    );

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["resource", "fields", "echo", "--config", "cfg.yaml"])
        .current_dir(&dir)
        .output()
        .expect("run bddkit");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stdout}\n{stderr}");
    assert!(stdout.contains("echo:"), "{stdout}");
    assert!(
        stdout.contains("prefix") && stdout.contains("required"),
        "the manifest's own description reaches the reader:\n{stdout}"
    );
    assert!(
        stdout.contains("(e.g. p-)"),
        "an example is worth more than the sentence describing it:\n{stdout}"
    );
}

/// The write half of the same contract: a plugin group's field list is what a
/// `--flag` is checked against, and the plugin's own `bddkit_probe_config` is
/// what decides whether the resource is written.
#[test]
fn resource_add_writes_a_plugin_group_it_can_probe() {
    let dir = project(
        "resource-add",
        "Feature: f\n",
        &format!("{ECHO_GROUP}default_echo: main\n"),
    );
    let cfg = dir.join("cfg.yaml");
    let before = std::fs::read_to_string(&cfg).expect("read config");

    let add = |args: &[&str]| {
        let argv: Vec<&str> = ["resource", "add", "echo"]
            .iter()
            .chain(args.iter())
            .copied()
            .collect();
        let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
            .args(&argv)
            .current_dir(&dir)
            .output()
            .expect("run bddkit");
        (
            format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ),
            out.status.code(),
        )
    };

    // Only the plugin knows what its group takes, so the manifest's `fields`
    // is the list a typo is caught against.
    let (out, code) = add(&["second", "--config", "cfg.yaml", "--prefx", "q-"]);
    assert_eq!(code, Some(1), "{out}");
    assert!(
        out.contains("prefx") && out.contains("prefix"),
        "the typo is named beside the names the manifest declares:\n{out}"
    );

    // The probe runs by default here, and the plugin's own answer is final.
    let (out, code) = add(&[
        "second",
        "--config",
        "cfg.yaml",
        "--prefix",
        "q-",
        "--probe_error",
        "the bucket is not there",
    ]);
    assert_eq!(code, Some(1), "{out}");
    assert!(out.contains("the bucket is not there"), "{out}");
    assert_eq!(
        std::fs::read_to_string(&cfg).expect("read config"),
        before,
        "a failed probe writes nothing"
    );

    let (out, code) = add(&["second", "--config", "cfg.yaml", "--prefix", "q-"]);
    assert_eq!(code, Some(0), "{out}");
    assert_eq!(
        std::fs::read_to_string(&cfg).expect("read config"),
        before.replace("  echo:\n", "  echo:\n    second:\n      prefix: q-\n"),
        "the file gains the block and nothing else moves"
    );
}

/// A resource name is written to the file literally and read back `${VAR}`-
/// expanded, so a name carrying a variable that is set is not in the loaded
/// config under the name being written. Built-in kinds refuse it; a plugin
/// group used to write the literal key and exit 0.
#[test]
fn resource_add_refuses_a_plugin_group_name_that_expands_to_something_else() {
    let dir = project(
        "resource-add-expanding-name",
        "Feature: f\n",
        &format!("{ECHO_GROUP}default_echo: main\n"),
    );
    let cfg = dir.join("cfg.yaml");
    let before = std::fs::read_to_string(&cfg).expect("read config");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "resource",
            "add",
            "echo",
            "${BDDKIT_TEST_NAME}",
            "--config",
            "cfg.yaml",
            "--prefix",
            "q-",
            "--no-check",
        ])
        .env("BDDKIT_TEST_NAME", "expanded")
        .current_dir(&dir)
        .output()
        .expect("run bddkit");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert!(text.contains("${VAR}"), "{text}");
    assert_eq!(
        std::fs::read_to_string(&cfg).expect("read config"),
        before,
        "nothing was written"
    );
}

/// The failure issue #28 is about, end to end: the fixture's `loud` is a
/// boolean and its `validate_config` refuses anything else, so before a
/// manifest could declare a type the host wrote `'false'` and the plugin
/// refused it on every input. The write is the assertion — `loud: false`
/// unquoted — because a string would still be a string with the quotes gone.
#[test]
fn a_declared_boolean_field_is_written_as_a_boolean() {
    let dir = project(
        "resource-add-typed",
        "Feature: f\n",
        &format!("{ECHO_GROUP}default_echo: main\n"),
    );
    let cfg = dir.join("cfg.yaml");
    let before = std::fs::read_to_string(&cfg).expect("read config");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "resource", "add", "echo", "second", "--config", "cfg.yaml", "--prefix", "q-",
            "--loud", "false",
        ])
        .current_dir(&dir)
        .output()
        .expect("run bddkit");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert_eq!(
        std::fs::read_to_string(&cfg).expect("read config"),
        before.replace(
            "  echo:\n",
            "  echo:\n    second:\n      prefix: q-\n      loud: false\n"
        ),
        "the boolean reaches the file as a boolean"
    );

    // And the value still has to be one of the two: the type says what form
    // the value takes, never that any text will do.
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args([
            "resource",
            "add",
            "echo",
            "third",
            "--config",
            "cfg.yaml",
            "--prefix",
            "q-",
            "--loud",
            "sometimes",
        ])
        .current_dir(&dir)
        .output()
        .expect("run bddkit");
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert!(text.contains("loud"), "{text}");
}

#[test]
fn doctor_probes_a_declared_instance_only_under_live() {
    let dir = project(
        "doctor-probe",
        "Feature: f\n  Scenario: s\n    Given I am in debug mode\n",
        ECHO_GROUP,
    );

    let doctor = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
            .args(args)
            .current_dir(&dir)
            .output()
            .expect("run bddkit");
        (
            String::from_utf8_lossy(&out.stdout).into_owned(),
            out.status.code(),
        )
    };

    let (static_out, code) = doctor(&["doctor", "--config", "cfg.yaml"]);
    assert_eq!(code, Some(0), "{static_out}");
    assert!(
        static_out.contains("echo main") && static_out.contains("skipped"),
        "a bare doctor must not call the plugin's probe:\n{static_out}"
    );

    let (live_out, code) = doctor(&["doctor", "--config", "cfg.yaml", "--live"]);
    assert_eq!(code, Some(0), "{live_out}");
    assert!(
        live_out.contains("echo main") && live_out.contains("probed clean"),
        "{live_out}"
    );
}

const TWO_REFUSED: &str =
    "  echo:\n    a: {}\n    b: {}\n    c:\n      prefix: \"p-\"\ndefault_echo: c\n";

#[test]
fn doctor_and_run_report_every_refused_instance() {
    let dir = project(
        "refused-all",
        "Feature: f\n  Scenario: s\n    Given I am in debug mode\n",
        TWO_REFUSED,
    );
    let bddkit = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
            .args(args)
            .current_dir(&dir)
            .output()
            .expect("run bddkit");
        (
            format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ),
            out.status.code(),
        )
    };

    let (text, code) = bddkit(&["run", "--config", "cfg.yaml"]);
    assert_eq!(code, Some(2), "{text}");
    assert!(
        text.contains("resources.echo.a") && text.contains("resources.echo.b"),
        "{text}"
    );

    let (text, code) = bddkit(&["doctor", "--config", "cfg.yaml"]);
    assert_eq!(code, Some(1), "{text}");
    assert!(
        text.contains("✗ echo a") && text.contains("✗ echo b"),
        "each refusal has its own row:\n{text}"
    );
    assert!(
        text.contains("steps"),
        "one refused instance must not skip the steps stage:\n{text}"
    );

    let (live, _) = bddkit(&["doctor", "--config", "cfg.yaml", "--live"]);
    assert!(
        live.contains("✓ echo c") && live.contains("probed clean"),
        "an accepted instance beside refused ones is still probed:\n{live}"
    );
    assert!(
        live.contains("not probed: refused"),
        "a refused instance says it was not probed:\n{live}"
    );

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config", "cfg.yaml", "--json"])
        .current_dir(&dir)
        .output()
        .expect("run bddkit");
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("doctor --json");
    let rows = json["checks"].as_array().expect("checks");
    for instance in ["a", "b"] {
        assert!(
            rows.iter().any(|c| c["stage"] == "echo"
                && c["target"] == instance
                && c["status"] == "failed"),
            "stage is the group, target the instance: {json}"
        );
    }
}

#[test]
fn doctor_bddkit_dir_lists_every_candidate_and_names_each_entry_layer() {
    let dir = project(
        "doctor-dirs",
        "Feature: f\n  Scenario: s\n    Given I am not in debug mode\n",
        ECHO_GROUP,
    );
    std::fs::write(
        dir.join(".bddkit/plugins.local.yaml"),
        "plugin:\n  - name: ghost\n    path: /nonexistent/libghost.so\n",
    )
    .expect("write local lock");
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config", "cfg.yaml", "--bddkit-dir", ".bddkit"])
        .current_dir(&dir)
        .output()
        .expect("run bddkit");
    let stdout = String::from_utf8_lossy(&out.stdout);
    // The override directory itself, found.
    assert!(
        stdout.contains("override") && stdout.contains("plugins.yaml"),
        "{stdout}"
    );
    // The local file in the same directory, found.
    assert!(
        stdout.contains("override.local") && stdout.contains("plugins.local.yaml"),
        "{stdout}"
    );
    // The committed echo plugin entry, resolved and present.
    assert!(stdout.contains("echo"), "{stdout}");
    // The local-only ghost entry, present in the listing but its .so is missing.
    assert!(stdout.contains("ghost"), "{stdout}");
}

#[test]
fn a_plugin_that_refuses_the_probe_fails_doctor_naming_the_instance() {
    // The fixture's probe answers with whatever `probe_error` names, which is
    // how a real plugin reports an endpoint that would not have it.
    let dir = project(
        "doctor-probe-fail",
        "Feature: f\n  Scenario: s\n    Given I am in debug mode\n",
        "  echo:\n    main:\n      prefix: \"p-\"\n      probe_error: \"bucket acceptance does not exist\"\n",
    );
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config", "cfg.yaml", "--live"])
        .current_dir(&dir)
        .output()
        .expect("run bddkit");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("echo main"), "{stdout}");
    assert!(
        stdout.contains("bucket acceptance does not exist"),
        "{stdout}"
    );
}
