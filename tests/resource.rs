//! `bddkit resource fields` — what a `resources.<kind>.<name>` body takes,
//! for the two kinds the host serves. The plugin half needs a loaded
//! `cdylib` and lives in `tests/plugin.rs`.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    reason = "`allow-*-in-tests` covers only `#[test]` functions, not the helpers here"
)]

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

#[test]
fn an_unknown_kind_is_a_nothing_listed_failure() {
    let (_out, code) = resource(&["resource", "fields", "nope"]);
    assert_eq!(code, Some(2));
}

#[test]
fn the_family_help_names_the_fields_subcommand() {
    let (out, code) = resource(&["resource"]);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("Usage: bddkit resource"), "{out}");
    assert!(out.contains("bddkit resource fields"), "{out}");
}
