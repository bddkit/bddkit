use super::{ECHO_GROUP, project, run_with};

#[test]
fn a_plugin_built_for_abi_2_is_refused_by_a_generation_4_host() {
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

#[test]
fn a_plugin_built_for_abi_3_loads_and_one_built_for_abi_5_is_refused() {
    let feature = "Feature: f\n  Scenario: s\n    When I echo \"x\" as \"y\"\n";
    let dir = project("abi3-loads", feature, ECHO_GROUP);
    let out = run_with(&dir, &[], &[("BDDKIT_ECHO_FIXTURE_ABI", "3")]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let dir = project("abi5-refused", feature, ECHO_GROUP);
    let out = run_with(&dir, &[], &[("BDDKIT_ECHO_FIXTURE_ABI", "5")]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("ABI version 5") && stderr.contains("upgrade bddkit"),
        "{stderr}"
    );
}
