use super::*;
use crate::plugin::tests::{entry, instance};
use crate::steps::{OptionsSource, StepKind};

/// Same build as tests/common::build_fixture_plugin; a unit test cannot
/// reach the integration test helpers, and six lines beat a shared crate.
fn fixture() -> std::path::PathBuf {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let target = root.join("target/fixture-plugin");
    let out = std::process::Command::new(env!("CARGO"))
        .args(["build", "--manifest-path"])
        .arg(root.join("tests/fixtures/echo-plugin/Cargo.toml"))
        .arg("--target-dir")
        .arg(&target)
        .output()
        .expect("cargo runs");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    target
        .join("debug")
        .join(crate::plugin::install::dll_name("echo_plugin"))
}

fn prepared(step: &str) -> Result<Vec<StepSpec>> {
    let mut steps: Vec<StepSpec> =
        serde_json::from_str(&format!(r#"[{{"group":"w","kind":"action",{step}}}]"#))
            .expect("steps parse");
    prepare_steps("widget", &mut steps)?;
    Ok(steps)
}

#[test]
fn a_step_declares_exactly_one_of_pattern_and_expression() {
    prepared(r#""pattern":"^a$""#).expect("a pattern alone");
    let steps = prepared(r#""expression":"a {n:uint}""#).expect("an expression alone");
    assert!(steps[0].compiled.is_some());
    for (step, needle) in [
        (r#""pattern":"^a$","expression":"a""#, "both"),
        (r#""description":"d""#, "neither"),
        (r#""expression":"a {n:nope}""#, "unknown type"),
        (
            r#""expression":"a {n}","secret_groups":[1]"#,
            "secret_groups",
        ),
    ] {
        let error = prepared(step).expect_err("refused").to_string();
        assert!(error.contains(needle), "{step}: {error}");
    }
}

#[test]
fn a_builtins_expression_compiles_as_it_does_for_the_builtin() {
    // The API and DB plugins declare built-in expressions verbatim.
    for (def, builtin) in crate::steps::BUILTIN_STEPS
        .iter()
        .zip(crate::steps::COMPILED_BUILTINS.iter())
    {
        let expression = serde_json::to_string(def.expression).expect("encodes");
        let steps = prepared(&format!(r#""expression":{expression}"#)).expect("compiles");
        let compiled = steps[0].compiled.as_ref().expect("compiled");
        assert_eq!(compiled.regex.as_str(), builtin.regex.as_str());
        assert_eq!(compiled.template, builtin.template);
    }
}

#[test]
fn an_effect_may_name_only_an_argument_the_step_has_and_shows() {
    prepared(r#""expression":"a {n}","sets_vars":["{n}_*"],"reads":["s:{n}"],"writes":["w"]"#)
        .expect("n is a parameter");
    prepared(r#""pattern":"^a (?P<n>x)$","writes":["{n}"]"#).expect("n is a named group");
    for step in [
        r#""expression":"a {n}","sets_vars":["{m}"]"#,
        r#""expression":"a {n}","reads":["{m}"]"#,
        r#""expression":"a {n}","writes":["{m}"]"#,
        r#""pattern":"^a (x)$","sets_vars":["{m}"]"#,
        r#""expression":"a {key:secret}","reads":["{key}"]"#,
        r#""pattern":"^a (?P<key>x)$","secret_groups":[1],"reads":["{key}"]"#,
        r#""pattern":"^a (?P<key>x(?P<inner>y))$","secret_groups":[1],"reads":["{inner}"]"#,
        r#""pattern":"^a (?P<key>x) (?P<n>y)$","secret_groups":[1],"reads":["{n}"]"#,
    ] {
        let error = prepared(step).expect_err("refused").to_string();
        assert!(error.contains("no parameter"), "{step}: {error}");
    }
}

#[test]
fn a_plugin_step_without_negative_is_not_negative() {
    let kind = |declared: &str| {
        let spec: StepSpec =
            serde_json::from_str(&format!(r#"{{"pattern":"^a$","group":"w",{declared}}}"#))
                .expect("a step parses");
        StepKind::from(&spec)
    };
    assert_eq!(
        kind(r#""kind":"assertion""#),
        StepKind::Assertion {
            source: OptionsSource::Global,
            negative: false
        }
    );
    assert_eq!(kind(r#""kind":"action""#), StepKind::Action);
}

#[test]
fn a_plugin_step_declares_negative() {
    // A loaded library, then `Plugins::steps`, then the registry: the
    // production path.
    let plugins = crate::plugin::Plugins::load(
        vec![entry()],
        &[instance("a", Some("p-"))],
        &["echo".into()],
        1,
        &crate::options::Options::default(),
    )
    .expect("the fixture plugin loads");
    let mut steps = plugins.steps();
    // The fixture has no negative `expression` step.
    let spec = prepared_assertion(r#""expression":"b {n}","negative":true"#);
    let compiled = spec.compiled.clone().expect("compiled");
    let declared = crate::plugin::Declared::Expression(compiled);
    steps.push((1, 0, declared, StepKind::from(&spec)));
    let registry = crate::steps::Registry::with_macros_and_plugins(
        crate::macros::MacroCatalog::default(),
        &steps,
        &["echo".to_string()],
        None,
    )
    .expect("builds");
    // The resolved step carries the flag, however the step was declared.
    for (text, expected) in [
        ("the echo should not be heard", true),
        (r#"b "x""#, true),
        ("the echo should fail", false),
    ] {
        let (target, _) = registry.find(text).expect("one match").expect("found");
        let crate::steps::StepTarget::Plugin {
            kind: StepKind::Assertion { negative, .. },
            ..
        } = target
        else {
            panic!("{text}: {target:?}");
        };
        assert_eq!(negative, expected, "{text}");
    }
}

/// `prepared` for an assertion: it declares actions only.
fn prepared_assertion(declared: &str) -> StepSpec {
    let mut steps: Vec<StepSpec> = serde_json::from_str(&format!(
        r#"[{{"group":"w","kind":"assertion",{declared}}}]"#
    ))
    .expect("steps parse");
    prepare_steps("widget", &mut steps).expect("a negative assertion is fine");
    steps.remove(0)
}

#[test]
fn negative_on_an_action_step_is_a_load_error() {
    // Whatever the declaration, a pattern that does not compile included.
    for declared in [
        r#""pattern":"^zz$""#,
        r#""expression":"zz""#,
        r#""pattern":"^(zz$""#,
    ] {
        let error = prepared(&format!(r#"{declared},"negative":true"#))
            .expect_err("refused")
            .to_string();
        for needle in ["widget", "negative", "zz"] {
            assert!(error.contains(needle), "{declared}: {error}");
        }
    }
    prepared(r#""pattern":"^a$","negative":false"#).expect("an explicit false is fine");
}

#[test]
fn a_secret_group_no_pattern_group_has_is_refused() {
    let step = |groups: &str| -> Vec<StepSpec> {
        serde_json::from_str(&format!(
            r#"[{{"pattern":"^a \"(.*)\" \"(.*)\"$","group":"w","kind":"action","secret_groups":{groups}}}]"#
        ))
        .expect("steps parse")
    };
    check_secret_groups("widget", &step("[1,2]")).expect("both groups exist");
    for bad in ["[0]", "[3]"] {
        let error = check_secret_groups("widget", &step(bad)).expect_err("refused");
        assert!(
            error.to_string().contains("groups count from 1"),
            "{bad}: {error}"
        );
    }
}

#[test]
fn loads_the_fixture_and_reads_its_manifest() {
    let lib = Library::load("echo", &fixture()).expect("loads");
    assert_eq!(lib.manifest.name, "echo");
    assert_eq!(lib.manifest.groups, vec!["echo".to_string()]);
}

#[test]
fn loads_under_any_label_and_keeps_the_manifest_name() {
    // `plugin install owner/repo` loads before the name is known; the
    // lock-driven path checks the name itself (`Plugins::load`).
    let lib = Library::load("someone/echo-plugin", &fixture()).expect("loads");
    assert_eq!(lib.manifest.name, "echo");
}

#[test]
fn lists_the_steps() {
    let lib = Library::load("echo", &fixture()).expect("loads");
    assert_eq!(lib.steps.len(), 30);
    assert!(!lib.steps[0].is_assertion());
    assert!(lib.steps[1].is_assertion());
    assert_eq!(lib.steps[0].group, "echo");
}

#[test]
fn a_missing_file_names_the_path() {
    let error = Library::load("ghost", std::path::Path::new("/nope/libghost.so"))
        .expect_err("no such file");
    assert!(
        format!("{error:#}").contains("/nope/libghost.so"),
        "{error:#}"
    );
}

#[test]
fn validate_config_reaches_the_plugin() {
    let lib = Library::load("echo", &fixture()).expect("loads");
    let ok = lib
        .validate_config(r#"{"group":"echo","instance":"a","config":{"prefix":"p-"}}"#)
        .expect("call succeeds");
    assert!(ok.is_ok(), "{ok:?}");

    let bad = lib
        .validate_config(r#"{"group":"echo","instance":"a","config":{}}"#)
        .expect("call succeeds");
    assert!(bad.unwrap_err().contains("prefix"));
}

#[test]
fn an_instance_round_trips_through_init_dispatch_and_drop() {
    let lib = Library::load("echo", &fixture()).expect("loads");
    let handle = lib
        .init_instance(r#"{"group":"echo","instance":"a","config":{"prefix":"p-"}}"#)
        .expect("call succeeds")
        .expect("instance created");
    let result = lib
        .dispatch(handle, 0, r#"{"args":["x","name"],"debug":false}"#)
        .expect("call succeeds");
    assert_eq!(result.status, crate::plugin::abi::Status::Passed);
    assert_eq!(result.vars.get("name").map(String::as_str), Some("p-x"));
    lib.drop_instance(handle)
        .expect("call succeeds")
        .expect("dropped");
}

#[test]
fn an_optional_symbol_that_is_absent_is_not_an_error() {
    // The fixture exports reset_scenario, so the absent case has to be
    // constructed: a plugin without it must still answer Ok, not fail.
    let mut lib = Library::load("echo", &fixture()).expect("loads");
    assert!(lib.has_reset_scenario());
    lib.reset_scenario = None;
    assert!(!lib.has_reset_scenario());
    // Resolves to the method; the field would need parentheses.
    assert!(lib.reset_scenario(1).expect("call succeeds").is_ok());
}

#[test]
fn an_abi_mismatch_names_both_versions() {
    let text = |reported| {
        format!(
            "{:#}",
            check_abi_version("odd", reported).expect_err("refused")
        )
    };
    let newer = text(ABI_VERSION + 1);
    assert!(
        newer.contains("odd") && newer.contains(&(ABI_VERSION + 1).to_string()),
        "{newer}"
    );
    assert!(newer.contains("upgrade bddkit"), "{newer}");
    let older = text(ABI_MIN - 1);
    assert!(older.contains(&(ABI_MIN - 1).to_string()), "{older}");
    assert!(older.contains("not updated"), "{older}");
    for accepted in ABI_MIN..=ABI_VERSION {
        check_abi_version("current", accepted).expect("inside the range");
    }
}

#[test]
fn a_step_in_an_unclaimed_group_is_refused() {
    // Answering for a group it never claimed would shadow the steps of the
    // plugin that legitimately owns it.
    let manifest: Manifest =
        serde_json::from_str(r#"{"name":"widget","version":"1.0.0","groups":["widget"]}"#)
            .expect("manifest parses");
    let steps: Vec<StepSpec> = serde_json::from_str(
        r#"[{"pattern":"^I upload$","group":"widget","kind":"action"},
            {"pattern":"^I click$","group":"browser","kind":"action"}]"#,
    )
    .expect("steps parse");
    check_step_groups("widget", &manifest, &steps[..1]).expect("a claimed group is fine");
    let error = check_step_groups("widget", &manifest, &steps).expect_err("refused");
    let text = format!("{error:#}");
    assert!(text.contains("browser"), "{text}");
    assert!(text.contains("I click"), "{text}");
}

#[test]
fn a_shared_plugin_with_a_reset_is_still_refused_in_parallel() {
    let error = check_reset_scenario("echo", Concurrency::Shared, true, 8).expect_err("refused");
    let text = format!("{error:#}");
    assert!(text.contains("echo"), "{text}");
    assert!(text.contains("bddkit_reset_scenario"), "{text}");
    assert!(text.contains("concurrency: 1"), "{text}");
    assert!(text.contains("per_worker"), "the remedy is named: {text}");
    // At the boundary too: the rule is "more than one worker", not "many".
    check_reset_scenario("echo", Concurrency::Shared, true, 2).expect_err("refused at 2");
}

#[test]
fn a_per_worker_plugin_with_a_reset_loads_in_parallel() {
    // This is the whole point of the milestone.
    check_reset_scenario("browser", Concurrency::PerWorker, true, 8).expect("allowed");
}

#[test]
fn a_shared_plugin_with_a_reset_loads_sequentially() {
    check_reset_scenario("echo", Concurrency::Shared, true, 1).expect("allowed");
}

#[test]
fn a_plugin_without_a_reset_loads_at_any_concurrency() {
    check_reset_scenario("echo", Concurrency::Shared, false, 8).expect("allowed");
}

#[test]
fn probe_config_reaches_the_plugin() {
    let lib = Library::load("echo", &fixture()).expect("loads");
    let ok = lib
        .probe_config(r#"{"group":"echo","instance":"a","config":{"prefix":"p-"}}"#)
        .expect("the fixture exports a probe")
        .expect("call succeeds");
    assert!(ok.is_ok(), "{ok:?}");

    let bad = lib
        .probe_config(
            r#"{"group":"echo","instance":"a","config":{"prefix":"p-","probe_error":"endpoint refused the connection"}}"#,
        )
        .expect("the fixture exports a probe")
        .expect("call succeeds");
    assert_eq!(bad.unwrap_err(), "endpoint refused the connection");
}

#[test]
fn a_plugin_without_a_probe_reports_no_probe_rather_than_a_failure() {
    // The distinction is the whole point of the outer Option: an absent
    // live check is "not available", never "the configuration is broken".
    // `reset_scenario` can answer Ok when absent because "nothing to
    // reset" really is success; a probe that never ran proved nothing.
    let mut lib = Library::load("echo", &fixture()).expect("loads");
    assert!(lib.has_probe_config());
    lib.probe_config = None;
    assert!(!lib.has_probe_config());
    assert!(
        lib.probe_config(r#"{"group":"echo","instance":"a","config":{}}"#)
            .is_none()
    );
}

#[test]
fn describing_a_group_it_does_not_claim_is_refused() {
    // Same reason as `check_step_groups`: a plugin describing a group it
    // does not serve would shadow the description of whichever plugin
    // legitimately owns it.
    let manifest: Manifest = serde_json::from_str(
        r#"{"name":"widget","version":"1.0.0","groups":["widget"],
            "fields":{"widget":[{"name":"selector"}],
                      "browser":[{"name":"headless"}]}}"#,
    )
    .expect("manifest parses");
    let error = check_field_groups("widget", &manifest).expect_err("refused");
    let text = format!("{error:#}");
    assert!(text.contains("widget"), "{text}");
    assert!(text.contains("browser"), "{text}");
}

/// A typo in a declared type must not degrade into "string": that is the
/// exact failure the key exists to prevent, and a silent fallback would
/// write `'false'` for a boolean key and leave the plugin's own
/// `validate_config` to take the blame for refusing it.
#[test]
fn declaring_a_type_the_host_does_not_know_is_refused() {
    let manifest: Manifest = serde_json::from_str(
        r#"{"name":"widget","version":"1.0.0","groups":["widget"],
            "fields":{"widget":[{"name":"headless","type":"boolena"}]}}"#,
    )
    .expect("manifest parses");
    let error = check_field_types("widget", &manifest).expect_err("refused");
    let text = format!("{error:#}");
    assert!(text.contains("headless"), "{text}");
    assert!(text.contains("boolena"), "{text}");
    assert!(
        text.contains("boolean"),
        "the accepted values are named: {text}"
    );
}

/// The backward-compatibility half: a manifest written before `type`
/// existed declares none, and that is not a finding.
#[test]
fn declaring_no_type_at_all_is_fine() {
    let manifest: Manifest = serde_json::from_str(
        r#"{"name":"widget","version":"1.0.0","groups":["widget"],
            "fields":{"widget":[{"name":"selector"},{"name":"headless","type":"boolean"}]}}"#,
    )
    .expect("manifest parses");
    check_field_types("widget", &manifest).expect("string is the default, boolean is known");
}

#[test]
fn an_implicit_instance_of_a_group_it_does_not_claim_is_refused() {
    // Same shadowing hazard as `fields`: the body would stand in for one
    // belonging to whichever plugin serves that group.
    let manifest: Manifest = serde_json::from_str(
        r#"{"name":"widget","version":"1.0.0","groups":["widget"],
            "implicit_instance":{"browser":{}}}"#,
    )
    .expect("manifest parses");
    let error = check_implicit_groups("widget", &manifest).expect_err("refused");
    let text = format!("{error:#}");
    assert!(text.contains("widget"), "{text}");
    assert!(text.contains("browser"), "{text}");
    assert!(text.contains("implicit_instance"), "{text}");
}

#[test]
fn describing_only_claimed_groups_is_fine() {
    let manifest: Manifest = serde_json::from_str(
        r#"{"name":"widget","version":"1.0.0","groups":["widget"],
            "fields":{"widget":[{"name":"selector"}]}}"#,
    )
    .expect("manifest parses");
    check_field_groups("widget", &manifest).expect("its own group is fine");
}

#[test]
fn a_shared_plugin_with_a_select_instance_is_refused_in_parallel_only() {
    let error = check_select_instance("db", Concurrency::Shared, true, 2).expect_err("refused");
    assert!(
        format!("{error}").contains("bddkit_select_instance"),
        "{error}"
    );
    check_select_instance("db", Concurrency::Shared, true, 1).expect("sequential");
    check_select_instance("api", Concurrency::PerWorker, true, 8).expect("per_worker");
    check_select_instance("db", Concurrency::Shared, false, 8).expect("no export");
}

#[test]
fn a_shared_plugin_with_a_failure_context_is_refused_in_parallel_only() {
    let error = check_failure_context("api", Concurrency::Shared, true, 2).expect_err("refused");
    assert!(
        format!("{error}").contains("bddkit_failure_context"),
        "{error}"
    );
    check_failure_context("api", Concurrency::Shared, true, 1).expect("sequential");
    check_failure_context("api", Concurrency::PerWorker, true, 8).expect("per_worker");
    check_failure_context("db", Concurrency::Shared, false, 8).expect("no export");
}
