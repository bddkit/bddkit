//! The stages of `doctor` that read the feature files: `steps` and `scheduling`,
//! and the hints under the `steps` row.

use super::{
    Report, Status, Unmarked, deprecated_hints, exported, include_call, left_behind, load_included,
    unmarked_names, unserved_group, written_names,
};
use crate::steps::StepTarget;
use crate::{config, feature, validate};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

type Loaded = Vec<Arc<feature::LoadedFeature>>;

/// The two stages that read the feature files: every step matched against the
/// registry, and the scheduling tags `run` parses after it. The files are
/// discovered and loaded once for both.
///
/// A `.feature` that will not parse is one finding, not the end of the stage:
/// a suite of forty files must not hide thirty-nine behind the first bad one.
///
/// `registry` is `Err(reason)` when the step vocabulary is not trustworthy —
/// the registry failed to build, or a plugin owning some of the steps did not
/// load. The reason comes from the caller because only the caller knows which
/// stage to send the reader to. Only the step MATCHING is skipped then: a
/// parse error and a scheduling tag are answers this stage still owes.
///
/// The one finding here that `run` never makes: a step of a resource group
/// with nothing declared — `resources.api: {}` under an HTTP step, a DB step
/// with no `resources.db`, a plugin group emptied to switch its implicit
/// instance off. `run` accepts that config and fails in the scenario, because
/// a `--tag` may keep the scenario out; `doctor` applies no filter, so here
/// the failure is certain and is reported as one.
/// Returns the `<<…>>` slots that look sensitive and carry no privacy word,
/// each with the places it was found, for the `privacy` row.
pub(super) fn check_features(
    report: &mut Report,
    cfg: &config::Config,
    registry: Result<&crate::steps::Registry, &'static str>,
    plugins: Option<&crate::plugin::Plugins>,
    words: &[String],
) -> Unmarked {
    let paths = match feature::discover(&cfg.paths) {
        Ok(paths) => paths,
        Err(error) => {
            report.push("steps", None, Status::Failed, &format!("{error:#}"));
            // Nothing was loaded, so scheduling has nothing to say — but a
            // consumer reading the JSON should not have to guess why a stage
            // it expects is simply absent.
            report.push(
                "scheduling",
                None,
                Status::Skipped,
                "no feature file was discovered",
            );
            return BTreeMap::default();
        }
    };
    let (mut loaded, mut problems) = load_all(paths);
    // No tag filter on the steps: `doctor` answers "is this suite whole", not
    // "is this tag green", so every scenario of every file is checked whatever
    // a run would select. The config's `tags` only decide whether a run would
    // have anything to do at all.
    let filter = feature::TagFilter::new(&[]);
    loaded.retain(|lf| lf.has_selected_scenario(&filter));
    let selection = feature::TagFilter::new(&cfg.tags);
    let selects_nothing = !loaded.iter().any(|lf| lf.has_selected_scenario(&selection));
    let unmarked = unmarked_names(&loaded, registry.ok(), words);

    let skipped = match registry {
        Ok(registry) => {
            problems.extend(step_problems(&loaded, registry, &filter, plugins));
            None
        }
        Err(reason) => Some(reason),
    };

    // Parse errors and an empty selection are reported whether or not the
    // vocabulary was usable — neither has anything to do with it, and a
    // finding this stage already computed must not be thrown away.
    if !problems.is_empty() {
        // An Outline row repeats the same line once per Examples row.
        let mut seen = std::collections::HashSet::new();
        problems.retain(|problem| seen.insert(problem.clone()));
        for (target, detail) in &problems {
            report.push("steps", Some(target), Status::Failed, detail);
        }
    } else if selects_nothing {
        // `run` exits 2 here. A green "0 file(s), every step matched" would be
        // the most misleading line the command can print: a tick on a suite
        // that cannot run.
        report.push(
            "steps",
            None,
            Status::Failed,
            "no scenario to run: the config's `paths` and `tags` select no scenario",
        );
    } else if let Some(reason) = skipped {
        report.push("steps", None, Status::Skipped, reason);
    } else {
        report.push("steps", None, Status::Ok, &steps_detail(&loaded, registry));
    }

    // The one pre-run check that lives past `validate::check`. `run` calls
    // `build_chains` and exits 2 on a malformed `@priority`/`@serial`, so a
    // `doctor` that stops at the steps would certify a suite that cannot start.
    match crate::runner::build_chains(loaded) {
        Ok(chains) => {
            let detail = format!("{} chain(s)", chains.len());
            report.push("scheduling", None, Status::Ok, &detail);
        }
        Err(error) => report.push("scheduling", None, Status::Failed, &error),
    }
    unmarked
}

/// The files that load, and one `(file, message)` per file that does not.
fn load_all(paths: Vec<std::path::PathBuf>) -> (Loaded, Vec<(String, String)>) {
    let mut loaded = Vec::new();
    let mut problems = Vec::new();
    for path in paths {
        match feature::load(&path) {
            Ok(lf) => loaded.push(Arc::new(lf)),
            Err(error) => problems.push((feature::display_path(&path), format!("{error:#}"))),
        }
    }
    (loaded, problems)
}

/// What `validate::check` and the unserved-resource check find, as
/// `(file:line, message)`.
fn step_problems(
    loaded: &Loaded,
    registry: &crate::steps::Registry,
    filter: &feature::TagFilter,
    plugins: Option<&crate::plugin::Plugins>,
) -> Vec<(String, String)> {
    let borrowed: Vec<&feature::LoadedFeature> = loaded.iter().map(Arc::as_ref).collect();
    let mut found = validate::check(&borrowed, registry, filter);
    found.extend(validate::unserved_resources(
        &borrowed,
        registry,
        filter,
        |target| unserved_group(plugins, target),
    ));
    found
        .into_iter()
        .map(|p| {
            (
                format!("{}:{}", feature::display_path(&p.file), p.line),
                p.message,
            )
        })
        .collect()
}

/// The `ok` detail of the `steps` row: the count, and, when the registry is
/// usable, what was found beside the steps.
fn steps_detail(
    loaded: &Loaded,
    registry: Result<&crate::steps::Registry, &'static str>,
) -> String {
    let mut detail = format!("{} file(s), every step matched", loaded.len());
    let Ok(registry) = registry else {
        return detail;
    };
    let mut disabled = BTreeSet::new();
    let mut disabled_files = BTreeSet::new();
    let mut unmatched = BTreeSet::new();
    for lf in loaded {
        scan_annotations(
            lf,
            registry,
            &mut disabled,
            &mut disabled_files,
            &mut unmatched,
        );
    }
    detail.push_str(&format!(
        "; {} disabled step(s) in {} file(s)",
        disabled.len(),
        disabled_files.len()
    ));
    if !unmatched.is_empty() {
        detail.push_str("\ncommented lines that look like steps but match none:");
        for (path, line) in unmatched {
            detail.push_str(&format!("\n{}:{line}", feature::display_path(path)));
        }
    }
    let hints = include_hints(loaded, registry);
    if !hints.is_empty() {
        detail.push_str(&format!("\ninclude hints:\n{}", hints.join("\n")));
    }
    let deprecated = deprecated_hints(loaded, registry);
    if !deprecated.is_empty() {
        detail.push_str(&format!("\ndeprecated:\n{}", deprecated.join("\n")));
    }
    detail
}

/// Collects the disabled steps and the commented-out lines that look like
/// steps in one file.
fn scan_annotations<'a>(
    lf: &'a feature::LoadedFeature,
    registry: &crate::steps::Registry,
    disabled: &mut BTreeSet<(&'a std::path::PathBuf, usize)>,
    disabled_files: &mut BTreeSet<&'a std::path::PathBuf>,
    unmatched: &mut BTreeSet<(&'a std::path::PathBuf, usize)>,
) {
    for sc in lf.scenarios() {
        for expanded in feature::expand_outlines(sc.scenario) {
            let annotations = crate::annotate::scenario(lf, sc, &expanded, registry);
            for item in annotations.items {
                if let crate::annotate::Item::Disabled { line, .. } = item {
                    disabled.insert((&lf.path, line));
                    disabled_files.insert(&lf.path);
                }
            }
            unmatched.extend(
                annotations
                    .unmatched
                    .into_iter()
                    .map(|line| (&lf.path, line)),
            );
        }
    }
}

/// Hints on `I include`, shown under the `steps` row and never failing it (the
/// privacy row's footing): `doctor` only reads, and a plugin step can publish
/// variables the host cannot see, so neither finding is certain enough for
/// `validate::check`.
///
/// 1. A plugin step that `reads` some state and `sets_vars`, straight after an
///    include, reads what the helper left behind; it breaks the day the helper
///    sends one more request. Quiet when the helper exports that name.
/// 2. An `@exports` name no step of the helper (or the nested include it
///    exports) writes: `run` finds it out only at the include.
///
/// Anything `validate::check` already refuses is skipped, not repeated.
// ponytail: callers are the loaded feature files only; a macro body that
// includes, or a helper outside `paths`, is not scanned. Walk those too if a
// suite needs it.
fn include_hints(loaded: &Loaded, registry: &crate::steps::Registry) -> Vec<String> {
    let mut extracts = BTreeSet::new();
    let mut unset = BTreeSet::new();
    for lf in loaded {
        let base = lf.path.parent().unwrap_or(std::path::Path::new("."));
        for sc in lf.scenarios() {
            for expanded in feature::expand_outlines(sc.scenario) {
                let steps: Vec<_> = lf
                    .background_steps(sc.rule)
                    .into_iter()
                    .chain(expanded.steps)
                    .collect();
                for i in 0..steps.len() {
                    step_hints(lf, base, &steps, i, registry, &mut extracts, &mut unset);
                }
            }
        }
    }
    extracts.into_iter().chain(unset).collect()
}

/// The hints for `steps[i]`, when it is an include.
fn step_hints(
    lf: &feature::LoadedFeature,
    base: &std::path::Path,
    steps: &[feature::ExpandedStep],
    i: usize,
    registry: &crate::steps::Registry,
    extracts: &mut BTreeSet<String>,
    unset: &mut BTreeSet<String>,
) {
    let Ok(Some((StepTarget::Builtin { id, .. }, caps))) = registry.find(&steps[i].text) else {
        return;
    };
    let Some((name, prefix)) = include_call(id, &caps) else {
        return;
    };
    let Some(included) = load_included(&caps[0], base) else {
        return;
    };
    let Ok(chosen) = crate::include::select_scenario(&included, name) else {
        return;
    };
    let Ok(exports) = feature::exports_of(&included, chosen.scenario) else {
        return;
    };
    if let Some(next) = steps.get(i + 1) {
        extracts.extend(left_behind_hints(lf, next, registry, &exports, prefix));
    }
    if let Some(written) = written_names(&included, chosen, registry) {
        let line = chosen.scenario.position.line;
        unset.extend(unset_exports(&included, line, &exports, &written));
    }
}

/// The step after an include reads state the helper left behind.
fn left_behind_hints(
    lf: &feature::LoadedFeature,
    next: &feature::ExpandedStep,
    registry: &crate::steps::Registry,
    exports: &[String],
    prefix: &str,
) -> Vec<String> {
    let Ok(Some((target, caps))) = registry.find(&next.text) else {
        return Vec::new();
    };
    left_behind(target, &caps, registry)
        .into_iter()
        .filter(|(name, _)| !exported(exports, prefix, name))
        .map(|(name, state)| {
            format!(
                "{}:{}: extract {name:?} reads the {state} the include left behind; \
                 export the value from the helper instead",
                feature::display_path(&lf.path),
                next.line,
            )
        })
        .collect()
}

/// `@exports` names that no step of the included scenario sets.
fn unset_exports(
    included: &feature::LoadedFeature,
    line: usize,
    exports: &[String],
    written: &BTreeSet<String>,
) -> Vec<String> {
    exports
        .iter()
        .filter(|export| {
            !export.ends_with('*')
                && !["last_insert_", "updated_", "deleted_"]
                    .iter()
                    .any(|p| export.starts_with(p))
                && !exported(written, "", export)
        })
        .map(|export| {
            format!(
                "{}:{line}: @exports names {export:?}, but no step of the scenario sets it",
                feature::display_path(&included.path),
            )
        })
        .collect()
}
