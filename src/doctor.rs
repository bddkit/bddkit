//! `bddkit doctor`: every check a run makes before its first request, reachable
//! without starting a run — plus, under `--live`, the one class of check a run
//! does not have: whether the resources the config names actually answer.

use crate::steps::{BUILTIN_STEPS, StepTarget};
use crate::{config, db, dirs, feature, http, unique, validate};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    Failed,
    Skipped,
}

/// One line of the report. `target` names the resource a stage was checking, so
/// a suite with four connections says which one is unreachable.
///
/// `probe` separates the two rows a resource gets — the static one and the
/// live one — which otherwise share a `(stage, target)` and leave a script
/// with two answers to one question.
#[derive(Debug, Serialize)]
pub struct Check {
    pub stage: &'static str,
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
    pub live: bool,
    pub checks: Vec<Check>,
}

impl Report {
    /// A static check — one that opened no socket.
    fn push(&mut self, stage: &'static str, target: Option<&str>, status: Status, detail: &str) {
        self.row(stage, target, status, false, detail);
    }

    /// The live half of a resource's row: the probe itself, or the note that
    /// `--live` was not passed and so it never ran.
    fn push_live(&mut self, stage: &'static str, target: &str, status: Status, detail: &str) {
        self.row(stage, Some(target), status, true, detail);
    }

    fn row(
        &mut self,
        stage: &'static str,
        target: Option<&str>,
        status: Status,
        probe: bool,
        detail: &str,
    ) {
        self.checks.push(Check {
            stage,
            target: target.map(str::to_string),
            status,
            probe,
            detail: detail.to_string(),
        });
    }

    pub fn failed(&self) -> usize {
        self.checks
            .iter()
            .filter(|c| c.status == Status::Failed)
            .count()
    }

    /// 0 when clean, 1 when anything was reported. Unlike `run`, `doctor` never
    /// exits 2: a config it cannot even parse is the most ordinary thing it has
    /// to report, not a reason to answer in a different currency.
    pub fn exit_code(&self) -> i32 {
        if self.failed() == 0 { 0 } else { 1 }
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
            };
            let label = match &c.target {
                Some(target) => format!("{} {target}", c.stage),
                None => c.stage.to_string(),
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
                out.push_str(&format!("  {mark} {label:<20}  {}\n", c.detail));
            }
        }
        let failed = self.failed();
        out.push_str(&if failed == 0 {
            "\nno problems found\n".to_string()
        } else {
            format!("\n{failed} problem(s)\n")
        });
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
pub async fn check(
    explicit_config: Option<&Path>,
    env: Option<&str>,
    live: bool,
    dir_env: &dirs::Env,
    report_paths: &[&Path],
) -> Report {
    let resolved = config::resolve_config_path(explicit_config);
    let mut report = Report {
        config: "(none)".to_string(),
        config_source: None,
        app_env: "unknown".to_string(),
        live,
        checks: Vec::new(),
    };
    if let Ok(Some(resolved)) = &resolved {
        report.config = format!("{} ({})", resolved.path.display(), resolved.source.label());
        report.config_source = Some(resolved.source);
        // A broken `.env` costs the header its answer, never the report: the
        // config stage below names the same failure properly.
        report.app_env =
            config::app_env_for(&resolved.path, env).unwrap_or_else(|_| "unknown".to_string());
    }

    // First, as in `run`: the same `--junit`/`--cucumber-json` a run takes,
    // and the same create-or-truncate it does before resolving or reading the
    // config — so even a config that cannot be found leaves the same empty
    // report files behind in both commands. Without them nothing is touched.
    for path in report_paths {
        let target = path.display().to_string();
        match crate::report::prepare(path) {
            Ok(()) => report.push("reports", Some(&target), Status::Ok, "created, empty"),
            Err(error) => report.push(
                "reports",
                Some(&target),
                Status::Failed,
                &format!("{error:#}"),
            ),
        }
    }

    let config_path = match resolved {
        Ok(Some(resolved)) => resolved.path,
        Ok(None) => {
            report.push("config", None, Status::Failed, config::NO_CONFIG_FOUND);
            return report;
        }
        Err(error) => {
            report.push("config", None, Status::Failed, &format!("{error:#}"));
            return report;
        }
    };
    let config_path = config_path.as_path();

    let cfg = match config::load(config_path, env) {
        Ok(cfg) => {
            report.push(
                "config",
                None,
                Status::Ok,
                "parsed, ${VAR} expanded, defaults resolved",
            );
            cfg
        }
        Err(error) => {
            report.push("config", None, Status::Failed, &format!("{error:#}"));
            return report;
        }
    };

    // This walks the layered `.bddkit` directory chain and reads
    // `plugins.yaml` a second time — `load_plugins` below reads the same
    // chain again for real. That duplication is deliberate: this half is
    // pure listing (no `dlopen`, nothing loaded, nothing run), so it is safe
    // and cheap to redo, and `doctor` is never in a hot loop. It exists to
    // answer a different question than the plugins stage below — what does
    // the chain look like, layer by layer, not did loading actually work —
    // and the two answers must stay legible as two rows, not merged into
    // one.
    //
    // The trade-off: a bad `--bddkit-dir` or a missing `.so` shows up TWICE —
    // once here (`directories`/`plugin_lock`) and once in the real `plugins`
    // summary below, both naming the same root cause. That is accepted, not
    // a bug. Suppressing the second row would mean threading this listing's
    // result into `load_plugins`, entangling the pure-listing path with the
    // real dlopen path for a cosmetic win only.
    match dirs::layers(dirs::Os::current(), dir_env, crate::config_dir(config_path)) {
        Ok(layers) => {
            let candidates = dirs::candidates(&layers, "plugins");
            for c in &candidates {
                let status = if c.path.is_file() {
                    Status::Ok
                } else {
                    Status::Skipped
                };
                report.push(
                    "directories",
                    Some(&c.layer),
                    status,
                    &c.path.display().to_string(),
                );
            }
            if let Ok(entries) = crate::plugin::lock::load(&candidates) {
                for entry in &entries {
                    let status = if entry.path.is_file() {
                        Status::Ok
                    } else {
                        Status::Failed
                    };
                    let detail = format!("{} — {}", entry.layer, entry.path.display());
                    report.push("plugin_lock", Some(&entry.name), status, &detail);
                }
            }
        }
        Err(error) => {
            report.push("directories", None, Status::Failed, &format!("{error:#}"));
        }
    }

    let generator = unique::Generator::new();
    let mut plugins_failed = false;
    let plugins = match crate::load_plugins(config_path, &cfg, &generator, dir_env) {
        Ok(Some(plugins)) => {
            let detail = format!(
                "{} step(s) over {} group(s)",
                plugins.steps().len(),
                plugins.group_names().len()
            );
            report.push("plugins", None, Status::Ok, &detail);
            Some(plugins)
        }
        Ok(None) => {
            report.push("plugins", None, Status::Ok, "none installed");
            None
        }
        Err(error) => {
            report.push("plugins", None, Status::Failed, &format!("{error:#}"));
            plugins_failed = true;
            None
        }
    };

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
    check_features(&mut report, &cfg, vocabulary, plugins.as_deref());
    check_apis(&mut report, &cfg, live).await;
    check_databases(&mut report, &cfg, live).await;
    check_srp(&mut report, &cfg);
    if let Some(plugins) = plugins.as_ref() {
        check_plugin_instances(&mut report, plugins, live);
    }
    report
}

/// The live half of the plugin config contract, one row per declared instance.
/// Nothing static happens here: `validate_config` already ran for every
/// instance in the plugins stage, and a second row saying the same thing would
/// leave a script with two answers to one question.
///
/// A plugin that exports no `bddkit_probe_config` is `Skipped`, never
/// `Failed`: the symbol is optional, and a check that never ran has proved
/// nothing about the configuration.
///
/// ponytail: the FFI call blocks this thread. `doctor` runs its stages one at
/// a time and has nothing else in flight, so `spawn_blocking` would buy
/// nothing; it becomes worth it the day the probes run concurrently.
fn check_plugin_instances(report: &mut Report, plugins: &crate::plugin::Plugins, live: bool) {
    for (group, instance) in plugins.declared_instances() {
        let target = format!("{group}.{instance}");
        if !live {
            report.push_live("plugin", &target, Status::Skipped, "live probe skipped");
            continue;
        }
        match plugins.probe_config(&group, &instance) {
            Some(Ok(())) => report.push_live("plugin", &target, Status::Ok, "probed clean"),
            Some(Err(error)) => report.push_live("plugin", &target, Status::Failed, &error),
            None => report.push_live(
                "plugin",
                &target,
                Status::Skipped,
                "the plugin exports no bddkit_probe_config",
            ),
        }
    }
}

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
fn check_features(
    report: &mut Report,
    cfg: &config::Config,
    registry: Result<&crate::steps::Registry, &'static str>,
    plugins: Option<&crate::plugin::Plugins>,
) {
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
            return;
        }
    };
    let mut loaded = Vec::new();
    let mut problems = Vec::new();
    for path in paths {
        match feature::load(&path) {
            Ok(lf) => loaded.push(std::sync::Arc::new(lf)),
            Err(error) => problems.push(format!("{}\n  {error:#}", feature::display_path(&path))),
        }
    }
    // No tag filter: `doctor` answers "is this suite whole", not "is this tag
    // green", so every scenario is checked whatever a run would select. The
    // filter is still applied to the file list, because that is how `run`
    // decides whether it has anything to do at all.
    let filter = feature::TagFilter::new(&[]);
    loaded.retain(|lf| lf.has_selected_scenario(&filter));

    let skipped = match registry {
        Ok(registry) => {
            let borrowed: Vec<&feature::LoadedFeature> =
                loaded.iter().map(std::sync::Arc::as_ref).collect();
            let mut found = validate::check(&borrowed, registry, &filter);
            found.extend(validate::unserved_resources(
                &borrowed,
                registry,
                &filter,
                |target| unserved_group(cfg, plugins, target),
            ));
            problems.extend(found.iter().map(|p| {
                format!(
                    "{}:{}\n  {}",
                    feature::display_path(&p.file),
                    p.line,
                    p.message
                )
            }));
            None
        }
        Err(reason) => Some(reason),
    };

    // Parse errors and an empty selection are reported whether or not the
    // vocabulary was usable — neither has anything to do with it, and a
    // finding this stage already computed must not be thrown away.
    if !problems.is_empty() {
        report.push("steps", None, Status::Failed, &problems.join("\n"));
    } else if loaded.is_empty() {
        // `run` exits 2 here. A green "0 file(s), every step matched" would be
        // the most misleading line the command can print: a tick on a suite
        // that cannot run.
        report.push(
            "steps",
            None,
            Status::Failed,
            "no scenario to run: the config's `paths` select no feature file",
        );
    } else if let Some(reason) = skipped {
        report.push("steps", None, Status::Skipped, reason);
    } else {
        let detail = format!("{} file(s), every step matched", loaded.len());
        report.push("steps", None, Status::Ok, &detail);
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
}

/// The message for a step whose resource group has nothing to run on, `None`
/// when it has — or when the step reaches for no resource at all (`vars`,
/// `debug`, `general`, and a macro, whose body is checked step by step only
/// when it runs).
fn unserved_group(
    cfg: &config::Config,
    plugins: Option<&crate::plugin::Plugins>,
    target: &StepTarget,
) -> Option<String> {
    let (group, served) = match target {
        StepTarget::Builtin { id, .. } => {
            let group = BUILTIN_STEPS.iter().find(|def| def.id == *id)?.group;
            let served = match group {
                "api" => !cfg.resources.api.is_empty(),
                "db" => !cfg.resources.db.is_empty(),
                "srp" => !cfg.resources.srp.is_empty(),
                _ => true,
            };
            (group, served)
        }
        StepTarget::Plugin { lib, step, .. } => {
            let plugins = plugins?;
            let group = plugins.group_of_step(*lib, *step);
            (group, plugins.has_instance(group))
        }
        StepTarget::Macro(_) => return None,
    };
    (!served).then(|| {
        format!("this step needs a resource of the \"{group}\" group, and resources.{group} declares none")
    })
}

/// The static check of one API resource: the resource it builds, and the line
/// describing it.
///
/// This and its three neighbours below are the per-resource checks `doctor` and
/// `resource add` share. They live in this module because its whole subject is
/// the checks a run makes before its first request, and `add` makes the same
/// ones over the single resource it is about to write. A second copy is how the
/// two commands come to disagree about whether a config is valid — which for
/// `doctor` is by definition a bug in `doctor`.
pub fn check_api(api: &config::ApiConfig) -> Result<(http::ApiResource, String), String> {
    let headers = api
        .default_headers
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let resource = http::ApiResource::new(
        &api.base_url,
        api.timeout_secs,
        headers,
        api.effective_options.clone(),
    )
    .map_err(|error| format!("{error:#}"))?;
    Ok((
        resource,
        format!("{} ({}s timeout)", api.base_url, api.timeout_secs),
    ))
}

/// The live half: whether the base URL answers at all. Any status is a pass —
/// reachability is the question, the application's own routing is not.
pub async fn probe_api(resource: &http::ApiResource, base_url: &str) -> Result<String, String> {
    let status = resource.probe().await?;
    Ok(format!("GET {base_url} → {status}"))
}

/// One connection at a time. `Db::connect` already names the failing connection
/// in its error — what it does not do is carry on: the whole-map call returns
/// at the first failure, so a second dead DSN would never be probed and a stage
/// would not run to completion.
///
/// ponytail: a refused connection surfaces as sqlx's pool acquire timeout —
/// thirty seconds, and the message says "pool timed out" rather than
/// "connection refused". That is `Db::connect`'s own behaviour, which `run`
/// shares, so fixing it belongs there rather than here: an `acquire_timeout` on
/// `AnyPoolOptions` in `Db::connect` would make every caller fail fast at once.
pub async fn probe_db(name: &str, connection: &config::Connection) -> Result<String, String> {
    let one = std::collections::BTreeMap::from([(name.to_string(), connection.clone())]);
    let pool = db::Db::connect(&one, 1).await?;
    // The vendor the connection settled on, which on the shared `mysql://`
    // scheme is only known after this round-trip.
    let vendor = pool.platform(name).map(|p| p.name()).unwrap_or("?");
    Ok(format!("connected, {vendor}"))
}

/// The SRP parameters a run would derive, and the variant that named them.
pub fn check_srp_resource(srp: &config::SrpConfig) -> Result<String, String> {
    srp.to_params()
        .map(|_| srp.variant.clone())
        .map_err(|error| format!("{error:#}"))
}

async fn check_apis(report: &mut Report, cfg: &config::Config, live: bool) {
    for (name, api) in &cfg.resources.api {
        let resource = match check_api(api) {
            Ok((resource, detail)) => {
                report.push("api", Some(name), Status::Ok, &detail);
                resource
            }
            Err(error) => {
                report.push("api", Some(name), Status::Failed, &error);
                continue;
            }
        };
        if !live {
            report.push_live("api", name, Status::Skipped, "live probe skipped");
            continue;
        }
        match probe_api(&resource, &api.base_url).await {
            Ok(detail) => report.push_live("api", name, Status::Ok, &detail),
            Err(error) => report.push_live("api", name, Status::Failed, &error),
        }
    }
}

async fn check_databases(report: &mut Report, cfg: &config::Config, live: bool) {
    for (name, connection) in &cfg.resources.db {
        match db::check_dsn(connection) {
            Ok(platform) => report.push("db", Some(name), Status::Ok, platform),
            Err(error) => {
                report.push("db", Some(name), Status::Failed, &error);
                continue;
            }
        }
        if !live {
            report.push_live("db", name, Status::Skipped, "live probe skipped");
            continue;
        }
        match probe_db(name, connection).await {
            Ok(detail) => report.push_live("db", name, Status::Ok, &detail),
            Err(error) => report.push_live("db", name, Status::Failed, &error),
        }
    }
}

fn check_srp(report: &mut Report, cfg: &config::Config) {
    for (name, srp) in &cfg.resources.srp {
        match check_srp_resource(srp) {
            Ok(variant) => report.push("srp", Some(name), Status::Ok, &variant),
            Err(error) => report.push("srp", Some(name), Status::Failed, &error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(status: Status) -> Check {
        Check {
            stage: "config",
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
            live: false,
            checks,
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
            stage: "db",
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
