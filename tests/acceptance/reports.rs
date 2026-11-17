use super::write_report_project;
use serde_json::Value;
use std::process::Command;

#[test]
fn run_junit_writes_a_well_formed_report_for_a_failing_run() {
    let config = write_report_project("junit");
    let report = config.with_file_name("out/junit.xml");
    let _ = std::fs::remove_dir_all(report.parent().expect("parent"));

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(&config)
        .arg("--junit")
        .arg(&report)
        .output()
        .expect("run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    let xml = std::fs::read_to_string(&report).expect("the report is written");
    let package = sxd_document::parser::parse(&xml)
        .unwrap_or_else(|e| panic!("the report must parse as XML: {e:?}\n{xml}"));
    let doc = package.as_document();
    let value = |xpath: &str| {
        sxd_xpath::evaluate_xpath(&doc, xpath)
            .expect("xpath evaluates")
            .string()
    };
    assert_eq!(value("count(//testsuite)"), "1", "{xml}");
    assert_eq!(value("count(//testcase)"), "2", "{xml}");
    assert_eq!(
        value("count(//testcase[@name='fails']/failure)"),
        "1",
        "{xml}"
    );
    assert_eq!(
        value("count(//testcase[@name='passes']/failure)"),
        "0",
        "{xml}"
    );
    let failure = value("//testcase[@name='fails']/failure");
    assert!(failure.contains("expected: ]]>"), "{failure}");
    assert!(
        value("//testcase[@name='passes']/@time")
            .parse::<f64>()
            .expect("time is a float")
            > 0.0,
        "{xml}"
    );
    let steps = value("//testcase[@name='fails']/system-out");
    assert!(
        steps.contains("passed") && steps.contains("failed") && steps.contains("skipped"),
        "{steps}"
    );
}

#[test]
fn run_cucumber_json_writes_the_feature_scenario_step_layout() {
    let config = write_report_project("cucumber-json");
    let report = config.with_file_name("cucumber.json");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(&config)
        .arg("--cucumber-json")
        .arg(&report)
        .output()
        .expect("run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    let json: Value =
        serde_json::from_slice(&std::fs::read(&report).expect("the report is written"))
            .expect("the report parses as JSON");
    let feature = &json[0];
    assert!(
        feature["uri"]
            .as_str()
            .expect("uri")
            .ends_with("report.feature"),
        "{json}"
    );
    assert_eq!(feature["name"], "reported feature", "{json}");
    let scenarios = feature["elements"].as_array().expect("elements");
    assert_eq!(scenarios.len(), 2, "{json}");
    let failing = &scenarios[1];
    assert_eq!(failing["name"], "fails", "{json}");
    assert_eq!(failing["type"], "scenario", "{json}");
    let steps = failing["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 4, "{json}");
    assert_eq!(steps[0]["keyword"], "When ", "{json}");
    assert_eq!(
        steps[0]["name"], "set variable \"ping\" to \"ok\"",
        "{json}"
    );
    assert_eq!(steps[0]["line"], 7, "{json}");
    assert_eq!(steps[0]["result"]["status"], "passed", "{json}");
    assert!(
        steps[0]["result"]["duration"]
            .as_u64()
            .expect("nanoseconds")
            > 0,
        "{json}"
    );
    assert_eq!(
        steps[1]["name"], "set variable \"x\" to \"<<null>>\"",
        "raw step text: {json}"
    );
    assert_eq!(steps[2]["result"]["status"], "failed", "{json}");
    assert!(
        steps[2]["result"]["error_message"]
            .as_str()
            .expect("error_message")
            .contains("expected: ]]>"),
        "{json}"
    );
    assert_eq!(steps[3]["result"]["status"], "skipped", "{json}");
}

#[test]
fn a_report_path_is_truncated_before_the_config_is_read() {
    let dir = std::env::temp_dir().join(format!("bddkit-report-broken-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let cfg = dir.join("cfg.yaml");
    std::fs::write(
        &cfg,
        "paths: [features]\nresources:\n  api: {}\nmacro_paths: [${BDDKIT_ABSENT_VAR}]\n",
    )
    .expect("write config");
    let report = dir.join("junit.xml");
    std::fs::write(&report, "<testsuites>yesterday's green report</testsuites>")
        .expect("stale report");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(&cfg)
        .arg("--junit")
        .arg(&report)
        .output()
        .expect("run bddkit");

    assert_eq!(
        out.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(&report).expect("the file still exists"),
        "",
        "a stale report must not survive a run that never started"
    );
}

#[test]
fn an_unwritable_report_path_stops_the_run_before_any_step_and_doctor_reports_it() {
    let config = write_report_project("unwritable-report-core");
    // A path under a regular file cannot be created.
    let report = config.join("cucumber.json");

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["run", "--config"])
        .arg(&config)
        .arg("--cucumber-json")
        .arg(&report)
        .output()
        .expect("run bddkit");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("cucumber.json"),
        "the path is named: {stderr}"
    );
    assert!(
        stderr.contains("run not started"),
        "no step may run before the report is prepared: {stderr}"
    );

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config"])
        .arg(&config)
        .arg("--cucumber-json")
        .arg(&report)
        .output()
        .expect("run bddkit");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("✗ reports"), "{stdout}");
}
