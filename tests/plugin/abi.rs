use super::{ECHO_GROUP, project, run_with};

#[test]
fn a_plugin_built_for_abi_2_is_refused() {
    let dir = project(
        "abi2-refused",
        "Feature: f\n  Scenario: s\n    When I echo \"x\" as \"y\"\n",
        ECHO_GROUP,
    );
    let out = run_with(&dir, &[], &[("BDDKIT_ECHO_FIXTURE_ABI", "2")]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("ABI version 2")
            && stderr.contains("older than the 3")
            && stderr.contains("not updated"),
        "{stderr}"
    );
}

/// The oldest generation the host loads. The host does not gate a key on the
/// reported number, so the step the echo fixture marks `negative` runs too.
#[test]
fn an_abi_min_plugin_still_loads() {
    let dir = project(
        "abi3-loads",
        "Feature: f\n  Scenario: s\n    When I echo \"x\" as \"y\"\n    Then the echo should not be heard\n",
        ECHO_GROUP,
    );
    let out = run_with(&dir, &[], &[("BDDKIT_ECHO_FIXTURE_ABI", "3")]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn a_plugin_newer_than_the_host_is_refused() {
    let feature = "Feature: f\n  Scenario: s\n    When I echo \"x\" as \"y\"\n";
    let dir = project("abi6-refused", feature, ECHO_GROUP);
    let out = run_with(&dir, &[], &[("BDDKIT_ECHO_FIXTURE_ABI", "6")]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("ABI version 6") && stderr.contains("upgrade bddkit"),
        "{stderr}"
    );
}
