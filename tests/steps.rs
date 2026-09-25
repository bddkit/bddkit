//! `bddkit steps` — the vocabulary a tester (or an agent writing for one)
//! reads before writing a feature file.

use std::process::Command;

fn steps(args: &[&str]) -> (String, Option<i32>) {
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(args)
        .output()
        .expect("run bddkit");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.code(),
    )
}

#[test]
fn the_listing_groups_templates_by_resource_and_shows_no_regex() {
    let (out, code) = steps(&["steps", "list"]);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("api:"), "{out}");
    assert!(out.contains("db:"), "{out}");
    assert!(out.contains(r#"  I request "<path>""#), "{out}");
    assert!(
        !out.contains("[^") && !out.contains("?P<"),
        "no raw regex may leak into the listing:\n{out}"
    );
}

#[test]
fn a_resource_argument_narrows_the_listing() {
    let (out, code) = steps(&["steps", "list", "db"]);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains(r#"I have "<table>" with "<pairs>""#), "{out}");
    assert!(!out.contains("I request"), "{out}");
}

#[test]
fn an_unknown_resource_is_a_nothing_ran_failure() {
    let (_out, code) = steps(&["steps", "list", "nope"]);
    assert_eq!(code, Some(2));
}

#[test]
fn filter_and_verbose_narrow_to_one_step_and_describe_it() {
    // This pair is the "help for a single step" path: no `describe`
    // subcommand, just filter until one step is left.
    let (out, code) = steps(&["steps", "list", "--filter", "response code", "-v"]);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("the response code is <code>"), "{out}");
    assert!(
        out.contains("status code"),
        "the description must appear under -v:\n{out}"
    );
    assert!(!out.contains("I request"), "{out}");
}

#[test]
fn a_filter_matching_nothing_is_empty_and_still_a_success() {
    let (out, code) = steps(&["steps", "list", "--filter", "I refund the order"]);
    assert_eq!(code, Some(0));
    assert!(out.trim().is_empty(), "{out}");
}

#[test]
fn json_is_machine_readable_and_carries_the_raw_pattern() {
    let (out, code) = steps(&["steps", "list", "api", "--json"]);
    assert_eq!(code, Some(0), "{out}");
    let rows: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
    let rows = rows.as_array().expect("an array");
    assert!(!rows.is_empty());
    let first = &rows[0];
    assert_eq!(first["group"], "api");
    assert!(first["template"].is_string());
    assert!(
        first["pattern"].as_str().expect("pattern").starts_with('^'),
        "the raw pattern belongs in the machine form: {first}"
    );
    assert!(first["description"].is_string());
    assert!(rows.iter().any(|row| row["kind"] == "assertion"));
}

#[test]
fn the_family_help_names_the_list_subcommand() {
    let (out, code) = steps(&["steps"]);
    assert_eq!(code, Some(0), "{out}");
    // The word "list" alone appears in the examples and the description too,
    // so it would pass on almost any output. The usage line is the contract:
    // it has to be a command the reader can actually type.
    assert!(out.contains("Usage: bddkit steps"), "{out}");
    assert!(out.contains("bddkit steps list --config"), "{out}");
}

#[test]
fn a_locale_replaces_the_description_but_never_the_template() {
    let (out, code) = steps(&[
        "steps",
        "list",
        "--filter",
        "response code",
        "-v",
        "--lang",
        "ru",
    ]);
    assert_eq!(code, Some(0), "{out}");
    assert!(
        out.contains("the response code is <code>"),
        "the step text a feature file must contain stays English:\n{out}"
    );
    assert!(
        !out.contains("status code"),
        "the description must be translated:\n{out}"
    );
}
