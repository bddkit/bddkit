//! `bddkit resource fields` — what a `resources.<kind>.<name>` body takes,
//! for the two kinds the host serves. The plugin half needs a loaded
//! `cdylib` and lives in `tests/plugin.rs`.

use std::process::Command;

fn resource(args: &[&str]) -> (String, Option<i32>) {
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(args)
        .output()
        .expect("run bddkit");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.code(),
    )
}

// moves-to: api
#[test]
#[cfg(not(feature = "dev-unreserve"))]
fn the_listing_covers_every_kind_the_host_serves() {
    let (out, code) = resource(&["resource", "fields"]);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("api:"), "{out}");
    assert!(out.contains("srp:"), "{out}");
    assert!(out.contains("base_url"), "{out}");
    assert!(out.contains("variant"), "{out}");
    // The databases are the `db` plugin's: no host kind of their own.
    assert!(!out.contains("db:"), "{out}");
    assert!(
        out.contains("required"),
        "a reader has to be able to tell the mandatory keys apart:\n{out}"
    );
}

// moves-to: api
#[test]
#[cfg(not(feature = "dev-unreserve"))]
fn a_kind_argument_narrows_the_listing() {
    let (out, code) = resource(&["resource", "fields", "srp"]);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("variant"), "{out}");
    assert!(!out.contains("base_url"), "{out}");
}

#[test]
fn an_unknown_kind_is_a_nothing_listed_failure() {
    let (_out, code) = resource(&["resource", "fields", "nope"]);
    assert_eq!(code, Some(2));
}

// moves-to: api
#[test]
#[cfg(not(feature = "dev-unreserve"))]
fn json_carries_every_key_of_the_field_description() {
    let (out, code) = resource(&["resource", "fields", "api", "--json"]);
    assert_eq!(code, Some(0), "{out}");
    let kinds: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
    let kinds = kinds.as_array().expect("an array");
    assert_eq!(kinds.len(), 1);
    let api = &kinds[0];
    assert_eq!(api["kind"], "api");
    assert_eq!(api["source"], "host");
    let fields = api["fields"].as_array().expect("an array of fields");
    let base_url = fields
        .iter()
        .find(|f| f["name"] == "base_url")
        .expect("base_url is described");
    assert_eq!(base_url["required"], true);
    assert!(base_url["description"].is_string());
}

#[test]
fn the_family_help_names_the_fields_subcommand() {
    let (out, code) = resource(&["resource"]);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("Usage: bddkit resource"), "{out}");
    assert!(out.contains("bddkit resource fields"), "{out}");
}
