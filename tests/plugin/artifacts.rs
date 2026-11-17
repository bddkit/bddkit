use super::{ECHO_GROUP, assert_green, project, run_with};

const DIRECTORIES: &str = r"Feature: directories
  Scenario: two dispatches write evidence
    When I echo the directories
    And I echo the directories
";

#[test]
fn artifacts_dir_roots_every_plugin_artifact() {
    let dir = project("artifacts-flag", DIRECTORIES, ECHO_GROUP);
    // Relative: resolved against the working directory.
    assert_green(&run_with(&dir, &["--artifacts-dir", "evidence"], &[]));
    for counter in ["000000", "000001"] {
        let file = dir.join("evidence").join(counter).join("artifact.txt");
        assert!(file.is_file(), "missing {}", file.display());
    }
}

#[test]
fn the_artifacts_dir_can_come_from_the_environment_and_the_flag_wins() {
    let dir = project("artifacts-env", DIRECTORIES, ECHO_GROUP);
    assert_green(&run_with(
        &dir,
        &[],
        &[("BDDKIT_ARTIFACTS_DIR", "from-env")],
    ));
    assert!(dir.join("from-env/000000/artifact.txt").is_file());

    let out = run_with(
        &dir,
        &["--artifacts-dir", "from-flag"],
        &[("BDDKIT_ARTIFACTS_DIR", "ignored")],
    );
    assert_green(&out);
    assert!(dir.join("from-flag/000000/artifact.txt").is_file());
    assert!(
        !dir.join("ignored").exists(),
        "the flag must win over the environment"
    );

    // An empty value is unset, as for BDDKIT_EVENTS: the default root applies.
    let out = run_with(&dir, &[], &[("BDDKIT_ARTIFACTS_DIR", "")]);
    assert_green(&out);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let run_id = stdout
        .lines()
        .find_map(|line| line.strip_prefix("run "))
        .expect("the run id is printed")
        .trim();
    let root = std::env::temp_dir().join(format!("bddkit-{run_id}"));
    assert!(
        root.join("000000/artifact.txt").is_file(),
        "{}",
        root.display()
    );
}

#[test]
fn the_workspace_stays_outside_a_given_artifacts_dir() {
    let dir = project("artifacts-workspace", DIRECTORIES, ECHO_GROUP);
    let out = run_with(&dir, &["--artifacts-dir", "evidence"], &[]);
    assert_green(&out);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let run_id = stdout
        .lines()
        .find_map(|line| line.strip_prefix("run "))
        .expect("the run id is printed")
        .trim();
    let workspace = std::env::temp_dir().join(format!("bddkit-{run_id}/workspace/000000"));
    assert!(
        workspace.join("workspace.txt").is_file(),
        "the workspace stays under the temp dir: {}",
        workspace.display()
    );
    assert!(
        !dir.join("evidence/000000/workspace.txt").exists(),
        "no workspace file under the artifact root"
    );
}
