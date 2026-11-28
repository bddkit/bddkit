//! `bddkit doctor`: every check a run makes before its first request, reachable
//! without starting a run — plus, under `--live`, the one class of check a run
//! does not have: whether the resources the config names actually answer.

use crate::steps::StepTarget;
use crate::{config, dirs, feature};
use serde::Serialize;
use std::path::Path;

mod features;
mod stages;

use features::check_features;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    Failed,
    Skipped,
    /// A hint: worth fixing, but `run` accepts the suite, so it never changes
    /// the exit code.
    Warning,
}

/// One line of the report. `target` names the resource a stage was checking, so
/// a suite with four connections says which one is unreachable.
///
/// `probe` separates the two rows a resource gets — the static one and the
/// live one — which otherwise share a `(stage, target)` and leave a script
/// with two answers to one question.
#[derive(Debug, Serialize)]
pub struct Check {
    pub stage: String,
    /// The stage is a plugin's resource group, a name the team chose: telemetry
    /// reports it as `plugin`, never as written.
    #[serde(skip)]
    pub plugin: bool,
    pub target: Option<String>,
    pub status: Status,
    pub probe: bool,
    pub detail: String,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub config: String,
    /// `None` only when nothing was resolved at all (the `config: "(none)"`
    /// case) — a `--json` consumer that wants how the path was chosen reads
    /// this rather than parsing the `(--config)`/`(BDDKIT_CONFIG)`/`(found in
    /// working directory)` suffix `config` carries for humans.
    pub config_source: Option<config::ConfigSource>,
    pub app_env: String,
    /// The `.env` files read next to the config, in load order.
    pub env_layers: Vec<String>,
    pub live: bool,
    pub checks: Vec<Check>,
    /// What the telemetry payload of this command needs. Not part of the
    /// report a person or a script reads.
    #[serde(skip)]
    pub telemetry: Option<crate::telemetry::doctor::Note>,
}

impl Report {
    /// A static check — one that opened no socket.
    fn push(&mut self, stage: &str, target: Option<&str>, status: Status, detail: &str) {
        self.row(stage, target, status, false, detail);
    }

    /// The live half of a resource's row: the probe itself, or the note that
    /// `--live` was not passed and so it never ran.
    fn push_live(&mut self, stage: &str, target: &str, status: Status, detail: &str) {
        self.row(stage, Some(target), status, true, detail);
    }

    /// The `telemetry` row, and the note the payload is built from. The key is
    /// `Broken` wherever the config could not be read: a team's own `false`
    /// may be the line that did not load.
    fn note_telemetry(
        &mut self,
        flag: Option<bool>,
        key: crate::telemetry::ConfigKey,
        dir_env: &dirs::Env,
        config_dir: &Path,
    ) {
        let layers = dirs::layers(dirs::Os::current(), dir_env, config_dir).unwrap_or_default();
        let decision = crate::telemetry::decide_here(flag, key, &layers);
        self.push(
            "telemetry",
            None,
            Status::Ok,
            &crate::telemetry::doctor::row(&decision),
        );
        self.telemetry = Some(crate::telemetry::doctor::Note {
            decision,
            layers,
            privacy: None,
            resources: None,
        });
    }

    fn row(
        &mut self,
        stage: &str,
        target: Option<&str>,
        status: Status,
        probe: bool,
        detail: &str,
    ) {
        self.checks.push(Check {
            stage: stage.to_string(),
            plugin: false,
            target: target.map(str::to_string),
            status,
            probe,
            // Before the report is rendered or serialised, so a secret holding a
            // quote cannot slip past in its JSON-escaped form.
            detail: crate::privacy::mask_text(detail),
        });
    }

    pub fn failed(&self) -> usize {
        self.count(Status::Failed)
    }

    fn count(&self, status: Status) -> usize {
        self.checks.iter().filter(|c| c.status == status).count()
    }

    /// 0 when clean, 1 when any check failed; a warning is not a failure.
    /// Unlike `run`, `doctor` never exits 2: a config it cannot even parse is
    /// the most ordinary thing it has to report, not a reason to answer in a
    /// different currency.
    pub fn exit_code(&self) -> i32 {
        i32::from(self.failed() != 0)
    }

    /// The whole report as one string, printed with a single `print!`. A
    /// static run ends with the line saying what it did *not* do — one line
    /// against a permanent ambiguity about what a green result meant.
    pub fn render(&self) -> String {
        let mut out = format!("config: {}\nAPP_ENV: {}\n\n", self.config, self.app_env);
        for c in &self.checks {
            let mark = match c.status {
                Status::Ok => "✓",
                Status::Failed => "✗",
                Status::Skipped => "-",
                Status::Warning => "!",
            };
            let label = match &c.target {
                Some(target) => format!("{} {target}", c.stage),
                None => c.stage.clone(),
            };
            // A one-line detail sits on the label's line, which is what the
            // padding is for; anything longer — `validate::check`'s own
            // file:line/message pairs — is indented under it rather than
            // smeared across one unreadable line, and then the label is not
            // padded, because padding before a newline is trailing whitespace.
            if c.detail.contains('\n') {
                out.push_str(&format!("  {mark} {label}\n"));
                for line in c.detail.lines() {
                    out.push_str(&format!("      {line}\n"));
                }
            } else {
                out.push_str(format!("  {mark} {label:<20}  {}", c.detail).trim_end());
                out.push('\n');
            }
        }
        let failed = self.failed();
        out.push_str(&if failed == 0 {
            "\nno problems found".to_string()
        } else {
            format!("\n{failed} problem(s)")
        });
        let warnings = self.count(Status::Warning);
        if warnings > 0 {
            out.push_str(&format!(", {warnings} warning(s)"));
        }
        out.push('\n');
        if !self.live {
            out.push_str("static checks only — pass --live to also probe the resources\n");
        }
        out
    }
}

/// Runs every stage to completion. Someone must not have to invoke the command
/// five times to discover five problems, so a failed stage records its finding
/// and the next one still runs — the single exception being the config itself,
/// which every later stage reads.
///
/// Nothing here is a second implementation of validation: every static stage
/// is a function `run` already calls, in the order `run` calls it in. If
/// `doctor` and `run` ever disagree about whether a config is valid, that is a
/// bug in `doctor`.
// Every input is a flag of the command, and `doctor` is the one place they meet:
// a struct of them would be a second spelling of `DoctorArgs`.
#[allow(clippy::too_many_arguments)]
pub fn check(
    explicit_config: Option<&Path>,
    env: &config::EnvArgs,
    live: bool,
    dir_env: &dirs::Env,
    report_paths: &[&Path],
    events_path: Option<&Path>,
    privacy_flags: &crate::privacy::PrivacyLayer,
    settings: &crate::settings::SettingsArgs,
    telemetry_flag: Option<bool>,
) -> Report {
    let resolved = config::resolve_config_path(explicit_config);
    let mut report = Report {
        config: "(none)".to_string(),
        config_source: None,
        app_env: "unknown".to_string(),
        env_layers: Vec::new(),
        live,
        checks: Vec::new(),
        telemetry: None,
    };
    if let Ok(Some(resolved)) = &resolved {
        report.config = format!("{} ({})", resolved.path.display(), resolved.source.label());
        report.config_source = Some(resolved.source);
        // A broken `.env` costs the header its answer, never the report: the
        // config stage below names the same failure properly.
        report.app_env = config::app_env_for(&resolved.path, env.app_env.as_deref())
            .unwrap_or_else(|_| "unknown".to_string());
        report.env_layers = config::env_layers_for(&resolved.path, env).unwrap_or_default();
    }

    stages::touch_reports(&mut report, report_paths, events_path);

    let config_path = match resolved {
        Ok(Some(resolved)) => resolved.path,
        // No config found, or two of them: nothing to read the key from.
        other => {
            let detail = match other {
                Ok(_) => config::NO_CONFIG_FOUND.to_string(),
                Err(error) => format!("{error:#}"),
            };
            report.push("config", None, Status::Failed, &detail);
            report.note_telemetry(
                telemetry_flag,
                crate::telemetry::ConfigKey::Broken,
                dir_env,
                Path::new("."),
            );
            return report;
        }
    };
    let config_path = config_path.as_path();

    let Some(cfg) = stages::load_config(
        &mut report,
        config_path,
        env,
        settings,
        telemetry_flag,
        dir_env,
    ) else {
        return report;
    };

    // The same resolution `run` makes, so a bad `BDDKIT_PRIVACY_*` value or a
    // list given in both forms is a failed row here and exit 2 there. The row
    // itself is pushed once the features are read, for the hint it carries.
    let privacy = crate::privacy::Privacy::from_sources(
        Some(&cfg.privacy),
        &crate::privacy::process_env,
        privacy_flags.clone(),
    );
    if let Ok(privacy) = &privacy
        && privacy.reveal
    {
        eprintln!("{}", privacy.reveal_notice());
    }

    stages::list_plugin_layers(&mut report, dir_env, config_path);
    let (plugins, plugins_failed) = stages::load_plugins(&mut report, config_path, &cfg, dir_env);

    let registry = match crate::build_registry(&cfg, plugins.as_ref()) {
        Ok(registry) => {
            report.push("macros", None, Status::Ok, "loaded, no conflict or cycle");
            Some(registry)
        }
        Err(error) => {
            report.push("macros", None, Status::Failed, &error);
            None
        }
    };
    stages::warn_quoted_untyped(&mut report, &cfg.macro_paths);

    // A registry built without the plugins that failed to load knows none of
    // their steps, so every plugin step in the suite would come back "unknown
    // step" — a wall of findings whose single cause is already reported above.
    // The two causes get their own message: a plugin failure leaves `macros`
    // green, so "see the stage above" would point the reader at the one row
    // that is fine.
    let vocabulary = match (registry.as_ref(), plugins_failed) {
        (Some(registry), false) => Ok(registry),
        (Some(_), true) => {
            Err("a plugin owning some of these steps did not load — see the plugins stage")
        }
        (None, _) => Err("the step registry did not build — see the macros stage"),
    };
    let words = privacy.as_ref().map_or_else(
        |_| crate::privacy::Privacy::default().words,
        |p| p.words.clone(),
    );
    let unmarked = check_features(&mut report, &cfg, vocabulary, plugins.as_deref(), &words);
    push_privacy(&mut report, &privacy, &unmarked);
    report.note_telemetry(
        telemetry_flag,
        (&cfg.telemetry).into(),
        dir_env,
        crate::config_dir(config_path),
    );
    let stream = events_path.is_some() || crate::privacy::process_env("BDDKIT_EVENTS").is_some();
    push_evidence(&mut report, &cfg, stream);
    if let Some(plugins) = plugins.as_ref() {
        check_plugin_instances(&mut report, plugins, live);
    }
    note_resources(&mut report, &privacy, plugins.as_deref());
    report
}

/// What the telemetry payload of this command carries beyond the checks.
fn note_resources<E>(
    report: &mut Report,
    privacy: &Result<crate::privacy::Privacy, E>,
    plugins: Option<&crate::plugin::Plugins>,
) {
    let Some(note) = &mut report.telemetry else {
        return;
    };
    note.privacy = privacy
        .as_ref()
        .ok()
        .map(crate::telemetry::payload::PrivacyFlags::of);
    let mut resources = crate::telemetry::payload::Resources::default();
    if let Some(plugins) = plugins {
        (resources.plugins, resources.plugins_other) = plugins.instance_counts();
    }
    note.resources = Some(resources);
}

/// One static row and one live row per declared instance, stage the group and
/// target the instance, like an `api` or `db` resource. The static row is what
/// `validate_config` said when the plugins loaded; its `ok` carries the plugin's
/// `detail`, if it gave one.
///
/// A plugin that exports no `bddkit_probe_config` is `Skipped`, never
/// `Failed`: the symbol is optional, and a check that never ran has proved
/// nothing about the configuration. A refused instance is not probed, and its live row says so.
///
/// ponytail: the FFI call blocks this thread. `doctor` runs its stages one at
/// a time and has nothing else in flight, so `spawn_blocking` would buy
/// nothing; it becomes worth it the day the probes run concurrently.
fn check_plugin_instances(report: &mut Report, plugins: &crate::plugin::Plugins, live: bool) {
    let first = report.checks.len();
    push_plugin_instances(report, plugins, live);
    for check in &mut report.checks[first..] {
        check.plugin = true;
    }
}

fn push_plugin_instances(report: &mut Report, plugins: &crate::plugin::Plugins, live: bool) {
    for (group, instance) in plugins.declared_instances() {
        if let Some(refusal) = plugins.refusal(&group, &instance) {
            report.push(&group, Some(&instance), Status::Failed, refusal);
            report.push_live(&group, &instance, Status::Skipped, "not probed: refused");
            continue;
        }
        report.push(
            &group,
            Some(&instance),
            Status::Ok,
            plugins.detail(&group, &instance).unwrap_or(""),
        );
        if !live {
            report.push_live(&group, &instance, Status::Skipped, "live probe skipped");
            continue;
        }
        match plugins.probe_config(&group, &instance) {
            Some(Ok(detail)) => report.push_live(
                &group,
                &instance,
                Status::Ok,
                detail.as_deref().unwrap_or("probed clean"),
            ),
            Some(Err(error)) => report.push_live(&group, &instance, Status::Failed, &error),
            None => report.push_live(
                &group,
                &instance,
                Status::Skipped,
                "the plugin exports no bddkit_probe_config",
            ),
        }
    }
}

/// A hint, never a refusal: `run` accepts a `resources.<group>: {}` that no
/// plugin serves, and says nothing. `doctor` points out the key is redundant.
fn with_empty_group_hints(
    mut detail: String,
    cfg: &config::Config,
    plugins: Option<&crate::plugin::Plugins>,
) -> String {
    let served = plugins
        .map(super::plugin::Plugins::group_names)
        .unwrap_or_default();
    for (group, instances) in &cfg.resources.groups {
        if instances.is_empty() && !served.contains(group) {
            detail.push_str(&format!(
                "; resources.{group} is empty and no installed plugin serves it, the key can be removed"
            ));
        }
    }
    detail
}

/// Hint names and the `file:line` places they were found (spec §7.10).
type Unmarked = std::collections::BTreeMap<String, std::collections::BTreeSet<String>>;

/// Every use of a plugin's deprecated switch word, in the feature files, the
/// scenarios they include (followed through nested includes) and the macro
/// bodies, each with the step that replaces it. A hint under the `steps` row:
/// the step still works and `run` says nothing.
fn deprecated_hints(
    loaded: &[std::sync::Arc<feature::LoadedFeature>],
    registry: &crate::steps::Registry,
) -> Vec<String> {
    let mut out = std::collections::BTreeSet::new();
    let mut seen = std::collections::BTreeSet::new();
    for lf in loaded {
        for sc in lf.scenarios() {
            for expanded in feature::expand_outlines(sc.scenario) {
                let steps = lf
                    .background_steps(sc.rule)
                    .into_iter()
                    .chain(expanded.steps)
                    .collect::<Vec<_>>();
                scan_deprecated(lf, &steps, registry, &mut out, &mut seen);
            }
        }
    }
    for definition in registry.macros() {
        for step in &definition.body {
            if let Some(hint) = deprecated_hint(registry, &step.text) {
                out.insert(format!(
                    "{}:{}: macro {:?}: {hint}",
                    feature::display_path(&definition.source),
                    definition.line,
                    definition.step
                ));
            }
        }
    }
    out.into_iter().collect()
}

fn deprecated_hint(registry: &crate::steps::Registry, text: &str) -> Option<String> {
    let (word, name, group) = registry.deprecated_switch(text)?;
    Some(format!(
        "I use {name:?} {word} is deprecated, write I use {name:?} {group}"
    ))
}

fn scan_deprecated(
    lf: &feature::LoadedFeature,
    steps: &[feature::ExpandedStep],
    registry: &crate::steps::Registry,
    out: &mut std::collections::BTreeSet<String>,
    seen: &mut std::collections::BTreeSet<(std::path::PathBuf, String)>,
) {
    use crate::steps::{StepId, StepTarget};
    let base = lf.path.parent().unwrap_or(std::path::Path::new("."));
    for step in steps {
        if let Some(hint) = deprecated_hint(registry, &step.text) {
            out.insert(format!(
                "{}:{}: {hint}",
                feature::display_path(&lf.path),
                step.line
            ));
        }
        let Ok(Some((StepTarget::Builtin { id, .. }, caps))) = registry.find(&step.text) else {
            continue;
        };
        if !matches!(id, StepId::Include | StepId::IncludeScenario) {
            continue;
        }
        let Some((name, _)) = include_call(id, &caps) else {
            continue;
        };
        let Some(included) = load_included(&caps[0], base) else {
            continue;
        };
        let Ok(chosen) = crate::include::select_scenario(&included, name) else {
            continue;
        };
        // A cycle or a repeat is walked once; `validate::check` refuses cycles.
        if !seen.insert((included.path.clone(), chosen.scenario.name.clone())) {
            continue;
        }
        let inner: Vec<_> = included
            .background_steps(chosen.rule)
            .into_iter()
            .chain(chosen.scenario.steps.iter().map(feature::to_step))
            .collect();
        scan_deprecated(&included, &inner, registry, out, seen);
    }
}

/// The variables a step takes out of the state it reads, each with that state.
/// A family has no one name to export, so it gets no hint.
fn left_behind(
    target: crate::steps::StepTarget,
    caps: &[String],
    registry: &crate::steps::Registry,
) -> Vec<(String, String)> {
    use crate::steps::StepTarget;
    match target {
        StepTarget::Plugin { lib, step, .. } => {
            let Some(effects) = registry
                .plugin_step(lib, step)
                .and_then(|spec| spec.effects(caps))
                .filter(|effects| !effects.reads.is_empty())
            else {
                return Vec::new();
            };
            let state = effects.reads.join(", ");
            effects
                .sets_vars
                .into_iter()
                .filter(|name| !name.ends_with('*'))
                .map(|name| (name, state.clone()))
                .collect()
        }
        _ => Vec::new(),
    }
}

/// Whether `name`, as the caller sees it after `with prefix`, is one of the
/// helper's `exports`, or of a writer list (a trailing `*` matches the rest).
fn exported<'a>(exports: impl IntoIterator<Item = &'a String>, prefix: &str, name: &str) -> bool {
    exports.into_iter().any(|e| match e.strip_suffix('*') {
        Some(stem) => name.starts_with(&prefixed(prefix, stem)),
        None => name == prefixed(prefix, e),
    })
}

fn prefixed(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}_{name}")
    }
}

/// An include step's scenario name and `with prefix` (empty when absent), or
/// `None` for any other step.
fn include_call(id: crate::steps::StepId, caps: &[String]) -> Option<(Option<&str>, &str)> {
    use crate::steps::StepId;
    match id {
        StepId::Include => Some((None, caps.get(1).map_or("", String::as_str))),
        StepId::IncludeScenario => Some((
            Some(caps[1].as_str()),
            caps.get(2).map_or("", String::as_str),
        )),
        _ => None,
    }
}

/// The included file, or `None` for a path `validate::check` already refused.
fn load_included(path_literal: &str, base: &std::path::Path) -> Option<feature::LoadedFeature> {
    let resolved = crate::include::resolve(path_literal, base).ok()?;
    feature::load(&resolved).ok()
}

/// The variable names a scenario's own steps write, or `None` when it has a
/// step whose writes cannot be listed (AES derives a prefix, a name
/// holding a `<<slot>>` or an Outline `<column>`, a macro or include exporting
/// a glob, a plugin step that declares no effects, which can publish what the
/// host cannot see). A plugin step's family stays in the list with its `*`.
fn written_names(
    lf: &feature::LoadedFeature,
    chosen: feature::ScenarioRef<'_>,
    registry: &crate::steps::Registry,
) -> Option<std::collections::BTreeSet<String>> {
    use crate::steps::{StepId, StepTarget};
    // An Outline's columns are seeded by the include's `with:`.
    let mut out: std::collections::BTreeSet<String> = chosen
        .scenario
        .examples
        .iter()
        .filter_map(|e| e.table.as_ref()?.rows.first())
        .flatten()
        .cloned()
        .collect();
    let base = lf.path.parent().unwrap_or(std::path::Path::new("."));
    let steps = lf
        .background_steps(chosen.rule)
        .into_iter()
        .chain(chosen.scenario.steps.iter().map(feature::to_step));
    for step in steps {
        let Ok(Some((target, caps))) = registry.find(&step.text) else {
            return None;
        };
        let names: Vec<String> = match target {
            StepTarget::Macro(index) => registry.macro_def(index).exports.clone(),
            StepTarget::Builtin { id, .. } => match id {
                StepId::SetVariable | StepId::SetVariableGlobal => vec![caps[0].clone()],
                StepId::ExtractJsonFromVariable
                | StepId::ExtractJsonFromVariableGlobal
                | StepId::ExtractRegexFromVariable
                | StepId::ExtractRegexFromVariableGlobal => vec![caps[2].clone()],
                StepId::EncryptWithAes => return None,
                StepId::Include | StepId::IncludeScenario => {
                    let (name, prefix) = include_call(id, &caps)?;
                    let inner = load_included(&caps[0], base)?;
                    let chosen = crate::include::select_scenario(&inner, name).ok()?;
                    feature::exports_of(&inner, chosen.scenario)
                        .ok()?
                        .iter()
                        .map(|e| prefixed(prefix, e))
                        .collect()
                }
                _ => continue,
            },
            StepTarget::Plugin { lib, step, .. } => {
                out.extend(registry.plugin_step(lib, step)?.effects(&caps)?.sets_vars);
                continue;
            }
        };
        if names.iter().any(|n| n.contains('<') || n.ends_with('*')) {
            return None;
        }
        out.extend(names);
    }
    Some(out)
}

/// Slots in the suite's feature files and macro bodies that look like a secret
/// and are not marked as one (spec §7.10).
fn unmarked_names(
    loaded: &[std::sync::Arc<feature::LoadedFeature>],
    registry: Option<&crate::steps::Registry>,
    words: &[String],
) -> Unmarked {
    let mut texts: Vec<crate::privacy::Scanned> = Vec::new();
    for lf in loaded {
        let file = feature::display_path(&lf.path);
        let steps = lf
            .feature
            .background
            .iter()
            .flat_map(|background| background.steps.iter())
            .chain(
                lf.feature
                    .rules
                    .iter()
                    .filter_map(|rule| rule.background.as_ref())
                    .flat_map(|background| background.steps.iter()),
            )
            .chain(lf.scenarios().flat_map(|sc| sc.scenario.steps.iter()));
        for step in steps {
            // A docstring or table cell is reported at its step's line.
            let place = format!("{file}:{}", step.position.line);
            texts.push((place.clone(), &step.value, &[]));
            texts.extend(
                step.docstring
                    .as_deref()
                    .map(|d| (place.clone(), d, &[][..])),
            );
            if let Some(table) = &step.table {
                texts.extend(
                    table
                        .rows
                        .iter()
                        .flatten()
                        .map(|cell| (place.clone(), cell.as_str(), &[][..])),
                );
            }
        }
    }
    // A macro body has no line of its own: it is found at its definition.
    let macros = registry.map_or(&[][..], |r| r.macros());
    // Built first: `texts` borrows these, so they must outlive the loop below.
    let marked: Vec<Vec<String>> = macros
        .iter()
        .map(|definition| {
            let params = definition.compiled.params.iter();
            params
                .filter(|p| p.ty.secret)
                .map(|p| p.name.clone())
                .collect()
        })
        .collect();
    for (definition, marked) in macros.iter().zip(&marked) {
        let place = format!(
            "{}:{}",
            feature::display_path(&definition.source),
            definition.line
        );
        for step in &definition.body {
            texts.push((place.clone(), &step.text, marked));
            texts.extend(
                step.docstring
                    .as_deref()
                    .map(|d| (place.clone(), d, &marked[..])),
            );
        }
    }
    crate::privacy::unmarked_names(texts, words)
}

/// What `run` decides about `--evidence` from the variables and the config,
/// with `stream` standing for its `--events` (or `$BDDKIT_EVENTS`); doctor has
/// no `--evidence` flag, so a run that passes one is not predicted here.
fn push_evidence(report: &mut Report, cfg: &config::Config, stream: bool) {
    let decided = crate::events::sources_from_env().and_then(|env| {
        let (sources, from) = match env {
            Some(_) if !stream => {
                anyhow::bail!("$BDDKIT_EVIDENCE requires --events (or $BDDKIT_EVENTS)")
            }
            Some(sources) => (sources, "$BDDKIT_EVIDENCE"),
            None if stream => (cfg.evidence.unwrap_or_default(), "config"),
            None => (crate::events::Sources::default(), "no stream"),
        };
        if !sources.any() {
            return Ok(format!("off ({from})"));
        }
        let cap = crate::events::buffer_from_env()?;
        Ok(format!(
            "{} ({from}); buffer {cap} bytes",
            sources.names().join(", ")
        ))
    });
    match decided {
        Ok(detail) => report.push("evidence", None, Status::Ok, &detail),
        Err(error) => report.push("evidence", None, Status::Failed, &format!("{error:#}")),
    }
}

fn push_privacy(
    report: &mut Report,
    privacy: &anyhow::Result<crate::privacy::Privacy>,
    unmarked: &Unmarked,
) {
    match privacy {
        Ok(privacy) => {
            let mut detail = format!(
                "masking {} (decided by {}); min_length {}, words [{}], fields [{}]",
                if privacy.reveal { "off" } else { "on" },
                privacy.reveal_source(),
                privacy.min_length,
                privacy.words.join(", "),
                privacy.fields.join(", "),
            );
            if !unmarked.is_empty() {
                detail.push_str(
                    "\nlook sensitive, and nothing marks them as secret (no privacy word in the name, no `secret` parameter of that name):",
                );
                for (name, places) in unmarked {
                    let places: Vec<&str> = places.iter().map(String::as_str).collect();
                    detail.push_str(&format!("\n  {name} — {}", places.join(", ")));
                }
            }
            report.push("privacy", None, Status::Ok, &detail);
        }
        Err(error) => report.push("privacy", None, Status::Failed, &format!("{error:#}")),
    }
}

/// The message for a step whose resource group has nothing to run on, `None`
/// when it has — or when the step reaches for no resource at all (`vars`,
/// `debug`, `general`, and a macro, whose body is checked step by step only
/// when it runs).
fn unserved_group(plugins: Option<&crate::plugin::Plugins>, target: &StepTarget) -> Option<String> {
    let (group, served) = match target {
        StepTarget::Plugin { lib, step, .. } => {
            let plugins = plugins?;
            let group = plugins.group_of_step(*lib, *step);
            (group, plugins.has_instance(group))
        }
        StepTarget::Builtin { .. } | StepTarget::Macro(_) => return None,
    };
    (!served).then(|| {
        format!("this step needs a resource of the \"{group}\" group, and resources.{group} declares none")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(status: Status) -> Check {
        Check {
            stage: "config".into(),
            plugin: false,
            target: None,
            status,
            probe: false,
            detail: "parsed".into(),
        }
    }

    fn report(checks: Vec<Check>) -> Report {
        Report {
            config: "suite.yaml".into(),
            config_source: Some(config::ConfigSource::Flag),
            app_env: "dev".into(),
            env_layers: Vec::new(),
            live: false,
            checks,
            telemetry: None,
        }
    }

    #[test]
    fn a_report_without_a_failed_check_exits_zero() {
        let report = report(vec![check(Status::Ok), check(Status::Skipped)]);
        assert_eq!(report.exit_code(), 0);
    }

    #[test]
    fn any_failed_check_makes_the_report_exit_one() {
        let report = report(vec![check(Status::Ok), check(Status::Failed)]);
        assert_eq!(report.exit_code(), 1);
    }

    #[test]
    fn the_header_names_the_config_and_the_selected_app_env() {
        // The APP_ENV layer is the first thing that is wrong when a suite
        // passes locally and fails in CI, so it is stated, never inferred.
        let mut report = report(vec![check(Status::Ok)]);
        report.app_env = "ci".into();
        let out = report.render();
        assert!(out.contains("suite.yaml"), "{out}");
        assert!(out.contains("APP_ENV: ci"), "{out}");
    }

    #[test]
    fn a_static_report_says_that_live_would_also_probe_the_resources() {
        let out = report(vec![check(Status::Ok)]).render();
        assert!(out.contains("--live"), "{out}");
    }

    #[test]
    fn a_live_report_does_not_repeat_the_live_hint() {
        let mut report = report(vec![check(Status::Ok)]);
        report.live = true;
        let out = report.render();
        assert!(!out.contains("--live"), "{out}");
    }

    #[test]
    fn a_failed_check_renders_its_target_and_every_line_of_its_detail() {
        let report = report(vec![Check {
            stage: "db".into(),
            plugin: false,
            target: Some("primary".into()),
            status: Status::Failed,
            probe: true,
            detail: "connection refused\nis the container running?".into(),
        }]);
        let out = report.render();
        assert!(out.contains("db"), "{out}");
        assert!(out.contains("primary"), "{out}");
        assert!(out.contains("connection refused"), "{out}");
        assert!(out.contains("is the container running?"), "{out}");
        assert!(out.contains("1 problem"), "{out}");
    }
}
