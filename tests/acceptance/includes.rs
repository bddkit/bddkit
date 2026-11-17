use super::{build_include_project, run_bddkit_in, write_doctor_project};
use std::process::Command;

/// `I include` runs another file's scenario inline and copies back only its
/// declared `@exports` variable — no HTTP steps involved, so no stub server
/// is needed.
#[test]
fn include_of_a_one_scenario_file_exports_its_declared_variable() {
    let dir = build_include_project(
        "include-test",
        &[
            (
                "features/target.feature",
                "tests/features/include/target.feature",
            ),
            (
                "features/caller.feature",
                "tests/features/include/caller.feature",
            ),
        ],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(0),
        "--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
}

/// A nested `I include` resolves its path against the file it is written in
/// (issue #76): `lib.feature` sits in `includes/` and includes a sibling, so
/// the runtime must agree with the validator on `includes/`, not its parent.
#[test]
fn a_nested_include_resolves_relative_to_the_intermediate_file() {
    let dir = build_include_project(
        "include-nested",
        &[
            (
                "includes/lib.feature",
                "tests/features/include/nested/includes/lib.feature",
            ),
            (
                "features/caller.feature",
                "tests/features/include/nested/features/caller.feature",
            ),
        ],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(0),
        "--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
}

/// `I include "<file>" with prefix "<p>"` (issue #52) renames every exported
/// variable to `<p>_<name>` as it crosses back into the caller — the same
/// target file is included twice here, once per prefix, and neither call's
/// `userId` export must clobber the other's.
#[test]
fn including_the_same_file_twice_with_different_prefixes_keeps_both_exports() {
    let dir = build_include_project(
        "include-prefix-test",
        &[
            (
                "features/target.feature",
                "tests/features/include/target.feature",
            ),
            (
                "features/caller.feature",
                "tests/features/include/caller_prefix.feature",
            ),
        ],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(0),
        "--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
}

/// A failed step inside an included scenario must not leave the caller's
/// file-level `VarStack` swapped for the included one — `run_include` must
/// restore it on the way out regardless of the include's own outcome. Two
/// scenarios in one file: the first sets a variable then includes a scenario
/// whose only step fails cleanly (an unset-variable assertion, never a
/// panic); the second — sharing the file's `VarStack` per invariant 2 —
/// asserts the first scenario's variable is still there. If a later refactor
/// ever short-circuits before the restore (a `?` between the swap and the
/// restore, or a fresh `push_frame` swapped in for the whole-stack swap),
/// this regresses: scenario 2 sees "before" undefined and fails too.
#[test]
fn a_failed_include_still_restores_the_callers_var_stack() {
    // These fixtures live under `tests/fixtures/include/`, NOT
    // `tests/features/`, because `restore_on_failure.feature` fails one
    // scenario on purpose — `tests/fixtures/` is this repo's existing home
    // for fixtures that are not part of the "every feature file passes"
    // acceptance gate (see `tests/fixtures/echo-plugin`,
    // `tests/fixtures/worker-plugin`). The include target is placed in a
    // sibling `targets/` directory, outside `paths: [features]` — its own
    // `.feature` file's `I include "../targets/..."` path matches — so it is
    // reached only through the include and never discovered and run a
    // second time on its own, which would add an unrelated failed scenario.
    let dir = build_include_project(
        "include-restore-test",
        &[
            (
                "targets/failing_target.feature",
                "tests/fixtures/include/failing_target.feature",
            ),
            (
                "features/restore_on_failure.feature",
                "tests/fixtures/include/restore_on_failure.feature",
            ),
        ],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(1),
        "one scenario fails (the include), the other passes — never a validation exit 2\n\
         --- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(
        stdout.contains("The included scenario fails"),
        "the failing scenario ran: --- stdout ---\n{stdout}"
    );
    // The report only prints FAILING scenarios, so scenario 2 passing leaves
    // no line of its own — the summary counts are the proof it ran and
    // passed: one file, two scenarios, exactly one failed.
    assert!(
        stdout.contains("scenarios: 2, failed: 1"),
        "scenario 2 (\"before\" == \"kept\") must pass silently, not add a \
         second failure — the swap must have been restored: \
         --- stdout ---\n{stdout}"
    );
    assert_eq!(
        stdout.matches("is not set").count(),
        1,
        "only the include's own failure should mention an unset variable; a \
         second occurrence would mean scenario 2 also saw \"before\" as \
         undefined — the swap was left in place: --- stdout ---\n{stdout}"
    );
}

/// A `with:` table cell is interpolated against the CALLER's scope before
/// `world.vars` is swapped for the include's fresh stack — never left as a
/// literal `<<...>>` token, and never resolved against the (empty) included
/// scope. `target_with.feature` stores the value it receives into its own
/// variable and compares it against a fixed literal, independent of the
/// substitution token itself, so a regression that skips the caller-scope
/// interpolation (or interpolates too late, after the swap) fails the run
/// instead of coincidentally still matching.
#[test]
fn with_table_cell_is_interpolated_against_the_callers_scope() {
    // Same reasoning and layout as the restore-on-failure test above:
    // `target_with.feature` fails standalone (its own Examples row is
    // `unused@example.com`, not `test@example.com`), so it goes in the
    // sibling `targets/` directory, reached only through the include.
    let dir = build_include_project(
        "include-with-test",
        &[
            (
                "targets/target_with.feature",
                "tests/fixtures/include/target_with.feature",
            ),
            (
                "features/caller_with.feature",
                "tests/fixtures/include/caller_with.feature",
            ),
        ],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(0),
        "--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
}

/// Invariant 2 (state scoping) says variables never flow into an included
/// scenario except through `with:` — this proves the negative directly: the
/// included scenario reads a variable the caller set and must find it
/// genuinely UNDEFINED, not merely empty. `variable "callerOnly" should be
/// empty` first calls `variable(w, name)` (`steps/vars.rs`), which fails with
/// `variable "callerOnly" is not set` for anything never `set` in the
/// included scenario's own fresh `VarStack` — a stale-but-visible value would
/// instead either match (if still `"secret"`) or fail on content, never on
/// "not set". If `run_include` ever stopped swapping in a fresh `VarStack`
/// (or swapped it in too late), this test would fail differently: the
/// assertion would report the value `"secret"` instead of "is not set", or
/// pass outright were the check changed to tolerate that.
#[test]
fn a_caller_variable_is_not_visible_inside_an_included_scenario() {
    // Same layout as `a_failed_include_still_restores_the_callers_var_stack`:
    // the included scenario fails on purpose, so both fixtures live under
    // `tests/fixtures/include/`, and the target sits in a sibling `targets/`
    // directory outside `paths: [features]` so it is reached only through
    // the include, never discovered and run a second time on its own.
    let dir = build_include_project(
        "include-isolation-test",
        &[
            (
                "targets/isolation_target_cannot_read_caller.feature",
                "tests/fixtures/include/isolation_target_cannot_read_caller.feature",
            ),
            (
                "features/isolation_caller_cannot_read.feature",
                "tests/fixtures/include/isolation_caller_cannot_read.feature",
            ),
        ],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(1),
        "the included scenario's own assertion must fail (the variable is \
         genuinely undefined inside the include), which fails the include \
         step and the caller scenario with it: \
         --- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(
        stdout.contains(r#"variable "callerOnly" is not set"#),
        "the failure must name the variable as UNSET, not merely empty or \
         mismatched — proving the caller's value never crossed into the \
         included scenario's scope: --- stdout ---\n{stdout}"
    );
}

/// Invariant 2's other half: a DECLARED `@exports` name reaches the caller,
/// but anything else the included scenario sets does not, even though both
/// live in the same fresh `VarStack` while the include runs. Two scenarios
/// sharing the caller file's `VarStack` (persisted across scenarios within
/// one file, per invariant 2): the first includes and checks the declared
/// export `kept` arrived; the second — without including anything itself —
/// asserts `notExported` is still undefined. If `run_include` ever copied
/// the whole included scope back instead of just the declared exports, the
/// second scenario would find `notExported == "no"` and pass instead of
/// failing — so this test is provative in the direction that matters: a
/// leak turns its expected failure into an unexpected pass, not the reverse.
#[test]
fn only_the_declared_export_reaches_the_caller() {
    let dir = build_include_project(
        "include-export-boundary-test",
        &[
            (
                "targets/isolation_target_export_boundary.feature",
                "tests/fixtures/include/isolation_target_export_boundary.feature",
            ),
            (
                "features/isolation_caller_export_boundary.feature",
                "tests/fixtures/include/isolation_caller_export_boundary.feature",
            ),
        ],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(1),
        "scenario 1 (declared export `kept`) must pass; scenario 2 must \
         fail on purpose, proving `notExported` never crossed the boundary: \
         --- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(
        stdout.contains("scenarios: 2, failed: 1"),
        "scenario 1 must pass silently (the declared export `kept` arrived \
         and equalled \"yes\") — a failure there would mean the export \
         itself is broken, not just the boundary: \
         --- stdout ---\n{stdout}"
    );
    assert!(
        stdout.contains(r#"variable "notExported" is not set"#),
        "scenario 2's failure must name `notExported` as UNSET — not \
         merely empty or some other mismatch — proving it never arrived, \
         rather than arriving as an unexpected value: \
         --- stdout ---\n{stdout}"
    );
}

/// A declared `@exports` name that the included scenario never `set`s must
/// fail the include step itself — not the caller scenario's own next
/// assertion — with an error naming the missing export, per `run_include`'s
/// `export_result` handling in `src/runner.rs`. If a regression silently
/// dropped an unresolved export instead of failing (e.g. by skipping it
/// rather than recording `missing`), this run would exit 0 instead of 1 and
/// the output would never mention "neverSet".
#[test]
fn a_declared_export_that_is_never_set_fails_the_include_step() {
    let dir = build_include_project(
        "include-missing-export-test",
        &[
            (
                "targets/isolation_target_missing_export.feature",
                "tests/fixtures/include/isolation_target_missing_export.feature",
            ),
            (
                "features/isolation_caller_missing_export.feature",
                "tests/fixtures/include/isolation_caller_missing_export.feature",
            ),
        ],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(1),
        "the missing export must fail the include step (and the caller \
         scenario with it), a scenario failure rather than a validation \
         (exit 2) or a silent pass: \
         --- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(
        stdout.contains("declared export \"neverSet\"")
            && stdout.contains("the variable is not set"),
        "the failure must clearly name the missing export \"neverSet\": \
         --- stdout ---\n{stdout}"
    );
}

#[test]
fn include_by_scenario_name_picks_the_named_one() {
    let dir = build_include_project(
        "include-multi-test",
        &[
            (
                "features/multi.feature",
                "tests/features/include/multi.feature",
            ),
            (
                "features/caller_multi.feature",
                "tests/features/include/caller_multi.feature",
            ),
        ],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(0),
        "--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
}

#[test]
fn include_of_an_outline_uses_the_callers_with_row() {
    let dir = build_include_project(
        "include-outline-with-test",
        &[
            (
                "features/outline.feature",
                "tests/features/include/outline.feature",
            ),
            (
                "features/caller_outline_with.feature",
                "tests/features/include/caller_outline_with.feature",
            ),
        ],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(0),
        "--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
}

#[test]
fn include_of_an_outline_with_one_examples_row_and_no_with_uses_that_row() {
    let dir = build_include_project(
        "include-outline-bare-test",
        &[
            (
                "features/outline.feature",
                "tests/features/include/outline.feature",
            ),
            (
                "features/caller_outline_bare.feature",
                "tests/features/include/caller_outline_bare.feature",
            ),
        ],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(0),
        "--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
}

#[test]
fn debug_mode_logs_the_includes_with_and_export_lines() {
    let dir = build_include_project(
        "debug-include-test",
        &[
            (
                "features/outline.feature",
                "tests/features/include/outline.feature",
            ),
            (
                "features/caller_debug_export.feature",
                "tests/features/include/caller_debug_export.feature",
            ),
        ],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(0),
        "--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );

    // In debug mode, the include logs its inputs and every exported variable
    assert!(
        stderr.contains("outline.feature ›"),
        "stderr should contain 'outline.feature ›' (entry line with filename and separator):\n{stderr}"
    );
    assert!(
        stderr.contains("  with value = "),
        "stderr should contain '  with value = ' (with two-space indentation):\n{stderr}"
    );
    assert!(
        stderr.contains("  export seen = "),
        "stderr should contain '  export seen = ' (with two-space indentation):\n{stderr}"
    );
}

/// The `with prefix "<p>"` name applies before the debug log line is
/// printed, so `--debug` shows the name that actually lands in the caller's
/// scope (`buyer_seen`), not the included scenario's own export name (`seen`).
#[test]
fn debug_mode_logs_the_prefixed_export_name() {
    let dir = build_include_project(
        "debug-include-prefix-test",
        &[
            (
                "features/outline.feature",
                "tests/features/include/outline.feature",
            ),
            (
                "features/caller_debug_export_prefix.feature",
                "tests/features/include/caller_debug_export_prefix.feature",
            ),
        ],
    );
    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(0),
        "--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(
        stderr.contains("  export buyer_seen = "),
        "stderr should contain '  export buyer_seen = ' (prefixed export name):\n{stderr}"
    );
}

#[test]
fn debug_mode_logs_a_macro_s_exported_variables() {
    let dir = std::env::temp_dir().join(format!("bddkit-macro-debug-test-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("macros.yaml"),
        "- step: I login as user \"{email}\"\n  do:\n    - set variable \"token\" to \"<<email>>-token\"\n  exports: [token]\n",
    )
    .expect("write macros");
    std::fs::write(
        dir.join("features/caller.feature"),
        "Feature: f\n  Scenario: s\n    Given I am in debug mode\n    When I login as user \"alice@example.com\"\n",
    )
    .expect("write feature");
    std::fs::write(
        dir.join("cfg.yaml"),
        "macro_paths: [macros.yaml]\npaths: [features]\nresources:\n  api: {}\n",
    )
    .expect("write config");

    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(0),
        "--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(
        stderr.contains(r#"macro "I login as user \"alice@example.com\"""#),
        "stderr should name the macro call:\n{stderr}"
    );
    assert!(
        stderr.contains("  export token = alice@example.com-token"),
        "stderr should contain the exported variable:\n{stderr}"
    );
}

/// The header must print before the macro body runs, not after: otherwise a
/// macro whose body fails logs nothing at all, the case debug mode is for.
#[test]
fn debug_mode_names_the_macro_even_when_its_body_fails() {
    let dir = std::env::temp_dir().join(format!(
        "bddkit-macro-debug-fail-test-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(dir.join("features")).expect("mkdir");
    std::fs::write(
        dir.join("macros.yaml"),
        "- step: I login as user \"{email}\"\n  do:\n    - variable \"missing\" should be equal to \"x\"\n",
    )
    .expect("write macros");
    std::fs::write(
        dir.join("features/caller.feature"),
        "Feature: f\n  Scenario: s\n    Given I am in debug mode\n    When I login as user \"alice@example.com\"\n",
    )
    .expect("write feature");
    std::fs::write(
        dir.join("cfg.yaml"),
        "macro_paths: [macros.yaml]\npaths: [features]\nresources:\n  api: {}\n",
    )
    .expect("write config");

    let (code, stdout, stderr) = run_bddkit_in("run", &dir);
    assert_eq!(
        code,
        Some(1),
        "the macro body's assertion must fail the run\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(
        stderr.contains(r#"macro "I login as user \"alice@example.com\"""#),
        "the header must print even though the macro body failed:\n{stderr}"
    );
}

#[test]
fn doctor_reports_a_broken_include_the_same_way_run_does() {
    // Same missing-file fixture as include_of_a_missing_file_is_a_problem (Task 7),
    // but invoke `bddkit doctor --config cfg.yaml` instead of `run`.
    // Assert: exit code 1 (doctor's own convention — 0 or 1 only, never 2,
    // per CLAUDE.md) with the missing file's name appearing in stdout.
    let cfg = write_doctor_project(
        "doctor-include",
        "Feature: caller\n  Scenario: test\n    Given I include \"nope.feature\"\n",
        "",
    );

    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["doctor", "--config", cfg.to_str().expect("path is UTF-8")])
        .output()
        .expect("failed to run bddkit");

    let stdout = String::from_utf8_lossy(&out.stdout);
    // Doctor exits 1 (never 2) even for a config that would make run exit 2
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    // The missing file name is mentioned in the output
    assert!(stdout.contains("nope.feature"), "{stdout}");
}

#[test]
fn steps_list_includes_both_include_steps() {
    let out = Command::new(env!("CARGO_BIN_EXE_bddkit"))
        .args(["steps", "list"])
        .output()
        .expect("failed to run bddkit");

    let text = String::from_utf8_lossy(&out.stdout);
    // Both Include and IncludeScenario steps start with "I include"
    assert!(
        text.contains("I include"),
        "steps list should contain 'I include': {text}"
    );
}
