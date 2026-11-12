//! `bddkit list`: what a run would run, without running it (issue #170).
//!
//! The same config, paths, `--tag` and `file:line` arguments as `run`, and the
//! same `select_features` / `selects_row` behind them, so the two cannot
//! disagree. It stops where `run` would load plugins: no registry, no step
//! check, no socket. That is `doctor`'s job.

use crate::{config, events, feature, privacy, settings};
use anyhow::Result;
use clap::Args;
use serde_json::{Value, json};
use std::path::PathBuf;

#[derive(Args)]
pub struct ListArgs {
    /// Path to the YAML config [default: $BDDKIT_CONFIG, else ./bddkit.yaml or ./bddkit.yml]
    #[arg(long)]
    config: Option<PathBuf>,
    /// List only these directories or .feature files instead of the config's `paths`;
    /// a file may end in `:LINE` or `:LINE#ROW`, as for `run`
    paths: Vec<PathBuf>,
    /// List only scenarios with one of these tags (repeatable); replaces the
    /// config's `tags` [default: $BDDKIT_TAG, comma-separated]
    #[arg(long = "tag")]
    tags: Vec<String>,
    /// Override APP_ENV: selects .env.<name> / .env.<name>.local
    #[arg(long = "env")]
    env: Option<String>,
    /// Machine-readable output, with the field names of the event stream
    #[arg(long)]
    json: bool,
    #[command(flatten)]
    privacy: crate::PrivacyArgs,
}

/// Exit 0 with the list, 2 with the reason on stderr and, under `--json`, an
/// `error` object on stdout. Nothing here is a plugin, a socket or telemetry.
pub fn command(args: ListArgs) -> i32 {
    match build(&args) {
        Ok(list) => match print(&list, args.json) {
            Ok(()) => 0,
            // `bddkit list | head`: the reader left, which is not a failure.
            Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => 0,
            Err(error) => {
                eprintln!("error: {error}, nothing listed");
                2
            }
        },
        Err(error) => {
            let message = privacy::mask_text(&format!("{error:#}"));
            eprintln!("error: {message}, nothing listed");
            if args.json {
                println!("{}", json!({"error": {"message": message}}));
            }
            2
        }
    }
}

fn build(args: &ListArgs) -> Result<Value> {
    privacy::set_flags(args.privacy.layer());
    let resolved = config::resolve_config_path(args.config.as_deref())?
        .ok_or_else(|| anyhow::anyhow!("{}", config::NO_CONFIG_FOUND))?;
    let mut cfg = config::load(&resolved.path, args.env.as_deref())?;
    let mut overrides = settings::resolve(
        &settings::SettingsArgs::default(),
        &cfg.all_group_names(),
        &privacy::process_env,
    )?;
    if !args.tags.is_empty() {
        overrides.tags = Some(args.tags.clone());
    }
    cfg.apply(&overrides)?;
    let privacy = privacy::Privacy::from_sources(
        Some(&cfg.privacy),
        &privacy::process_env,
        args.privacy.layer(),
    )?;
    if privacy.reveal {
        eprintln!("{}", privacy.reveal_notice());
    }
    let redactor = privacy::Redactor::new(&privacy, privacy::recorded());
    let mut trace = events::Trace::off();
    trace.set_redactor(redactor);

    let filter = feature::TagFilter::new(&cfg.tags);
    let paths = if args.paths.is_empty() {
        cfg.paths.as_slice()
    } else {
        args.paths.as_slice()
    };
    let loaded = crate::select_features(paths, !args.paths.is_empty(), &filter)?
        .map_err(anyhow::Error::msg)?;

    let files: Vec<Value> = loaded
        .iter()
        .map(|lf| {
            let mut scenarios = Vec::new();
            for sc in lf.scenarios() {
                for ex in lf.expand(sc.scenario) {
                    if lf.selects_row(&filter, sc.scenario, &ex) {
                        scenarios.push(trace.scenario_fields(lf, sc, &ex));
                    }
                }
            }
            let mut file = trace.file_fields(lf);
            file["file"] = feature::display_path(&lf.path).into();
            file["scenarios"] = scenarios.into();
            file
        })
        .collect();
    Ok(json!({
        "bddkit": env!("CARGO_PKG_VERSION"),
        "config": feature::display_path(&resolved.path),
        "config_source": resolved.source,
        "selection": {
            "tags": args.tags,
            "paths": args.paths.iter().map(|p| feature::display_path(p)).collect::<Vec<_>>(),
        },
        "files": files,
    }))
}

/// `--json`: one line, like the error object. Text: one line per scenario,
/// `file:LINE  name`; the line is the example row's when it has one, so it can
/// be pasted back into `run`.
fn print(list: &Value, json: bool) -> std::io::Result<()> {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    if json {
        return writeln!(out, "{list}");
    }
    for file in list["files"].as_array().into_iter().flatten() {
        let path = file["file"].as_str().unwrap_or_default();
        for scenario in file["scenarios"].as_array().into_iter().flatten() {
            let line = scenario["row_line"].as_u64().or(scenario["line"].as_u64());
            writeln!(
                out,
                "{path}:{}  {}",
                line.unwrap_or_default(),
                scenario["name"].as_str().unwrap_or_default()
            )?;
        }
    }
    Ok(())
}
