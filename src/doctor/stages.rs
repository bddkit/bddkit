//! The stages of `doctor::check` kept out of its own file: the report files,
//! the config, the layer listing, the plugin load and the macro warnings.

use super::{Report, Status, with_empty_group_hints};
use crate::{config, dirs};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// First, as in `run`: the same `--junit`/`--cucumber-json` a run takes,
/// and the same create-or-truncate it does before resolving or reading the
/// config — so even a config that cannot be found leaves the same empty
/// report files behind in both commands. Without them nothing is touched.
pub(super) fn touch_reports(
    report: &mut Report,
    report_paths: &[&Path],
    events_path: Option<&Path>,
) {
    for (path, stream) in report_paths
        .iter()
        .map(|path| (*path, false))
        .chain(events_path.map(|path| (path, true)))
    {
        let target = path.display().to_string();
        // Opening a FIFO would hand its reader an end-of-file, and `run` opens
        // it exactly once; `metadata` follows the symlink a /dev/fd/N is. A
        // directory is not a stream either: `run` refuses it, so it falls through.
        if stream && std::fs::metadata(path).is_ok_and(|meta| !meta.is_file() && !meta.is_dir()) {
            report.push(
                "reports",
                Some(&target),
                Status::Ok,
                "exists and is not a regular file, left unopened",
            );
            continue;
        }
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
}

/// Loads the config and lays the flags and the environment over it. `None`: a
/// failed `config` row was pushed and nothing later can run.
pub(super) fn load_config(
    report: &mut Report,
    config_path: &Path,
    env: &config::EnvArgs,
    settings: &crate::settings::SettingsArgs,
    telemetry_flag: Option<bool>,
    dir_env: &dirs::Env,
) -> Option<config::Config> {
    let mut cfg = match config::load(config_path, env) {
        Ok(cfg) => cfg,
        Err(error) => {
            report.push("config", None, Status::Failed, &format!("{error:#}"));
            report.note_telemetry(
                telemetry_flag,
                crate::telemetry::ConfigKey::Broken,
                dir_env,
                crate::config_dir(config_path),
            );
            return None;
        }
    };
    // The flags and the environment on top of the file, as `run` lays them. A
    // bad one is a failed row, but the file itself was readable, so its own
    // telemetry key still decides.
    let applied = crate::settings::resolve(
        settings,
        &cfg.all_group_names(),
        &crate::privacy::process_env,
    )
    .and_then(|overrides| cfg.apply(&overrides));
    if let Err(error) = applied {
        report.push("config", None, Status::Failed, &format!("{error:#}"));
        report.note_telemetry(
            telemetry_flag,
            (&cfg.telemetry).into(),
            dir_env,
            crate::config_dir(config_path),
        );
        return None;
    }
    report.push(
        "config",
        None,
        Status::Ok,
        "parsed, ${VAR} expanded, defaults resolved",
    );
    Some(cfg)
}

/// This walks the layered `.bddkit` directory chain and reads
/// `plugins.yaml` a second time — `load_plugins` below reads the same
/// chain again for real. That duplication is deliberate: this half is
/// pure listing (no `dlopen`, nothing loaded, nothing run), so it is safe
/// and cheap to redo, and `doctor` is never in a hot loop. It exists to
/// answer a different question than the plugins stage below — what does
/// the chain look like, layer by layer, not did loading actually work —
/// and the two answers must stay legible as two rows, not merged into
/// one.
///
/// The trade-off: a bad `--bddkit-dir` or a missing `.so` shows up TWICE —
/// once here (`directories`/`plugin_lock`) and once in the real `plugins`
/// summary below, both naming the same root cause. That is accepted, not
/// a bug. Suppressing the second row would mean threading this listing's
/// result into `load_plugins`, entangling the pure-listing path with the
/// real dlopen path for a cosmetic win only.
pub(super) fn list_plugin_layers(report: &mut Report, dir_env: &dirs::Env, config_path: &Path) {
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
}

/// The `plugins` stage. Returns the loaded plugins and whether loading failed.
pub(super) fn load_plugins(
    report: &mut Report,
    config_path: &Path,
    cfg: &config::Config,
    dir_env: &dirs::Env,
) -> (Option<Arc<crate::plugin::Plugins>>, bool) {
    match crate::load_plugins_collecting(config_path, cfg, dir_env, false) {
        Ok(Some(plugins)) => {
            let detail = format!(
                "{} step(s) over {} group(s)",
                plugins.steps().len(),
                plugins.group_names().len()
            );
            report.push(
                "plugins",
                None,
                Status::Ok,
                &with_empty_group_hints(detail, cfg, Some(&plugins)),
            );
            (Some(plugins), false)
        }
        Ok(None) => {
            let detail = with_empty_group_hints("none installed".into(), cfg, None);
            report.push("plugins", None, Status::Ok, &detail);
            (None, false)
        }
        Err(error) => {
            report.push("plugins", None, Status::Failed, &format!("{error:#}"));
            (None, true)
        }
    }
}

/// A warning row per untyped macro parameter written directly between quotes:
/// it is `any`, so it matches across the quotes that look like its limits.
///
/// Reads the catalog itself rather than the registry's, because there is no
/// registry when two macros conflict, the case the warning is for. A catalog
/// that does not load is already the failed `macros` row.
pub(super) fn warn_quoted_untyped(report: &mut Report, macro_paths: &[PathBuf]) {
    let Ok(catalog) = crate::macros::MacroCatalog::load(macro_paths) else {
        return;
    };
    for definition in &catalog.definitions {
        let template = &definition.step;
        let target = format!(
            "{}:{}",
            crate::feature::display_path(&definition.source),
            definition.line
        );
        for param in &definition.compiled.params {
            let name = &param.name;
            // Only the untyped spelling is `{name}`. An even number of quotes
            // before it means the first quote opens a literal, not closes one.
            let quoted = template
                .match_indices(&format!("\"{{{name}}}\""))
                .any(|(at, _)| template[..at].matches('"').count().is_multiple_of(2));
            if quoted {
                let detail = format!(
                    "{template}\nparameter {{{name}}} has no type and can match across quotes: declare it \"{{{name}:text}}\""
                );
                report.push("macros", Some(&target), Status::Warning, &detail);
            }
        }
    }
}
