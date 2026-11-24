//! `bddkit list`: what a run would run, without running it (issue #170).
//!
//! The same config, paths, `--tag` and `file:line` arguments as `run`, and the
//! same selection behind them (`select::select_listed` is `select_features` that
//! also returns a file that does not parse, which `run` refuses), so the two
//! cannot disagree about a file that does. It stops where `run` would load plugins: no registry, no step
//! check, no socket. That is `doctor`'s job.

use crate::select::{Selected, Unparsed};
use crate::{config, events, feature, privacy, settings};
use anyhow::Result;
use clap::Args;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

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
pub fn command(args: &ListArgs) -> i32 {
    match build(args) {
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
    let files = list_files(
        &resolved.path,
        &args.tags,
        args.env.as_deref(),
        &args.paths,
        &args.privacy,
        &mut Vec::new(),
    )?;
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

/// What one config lists: the shared core of `list` and `list-all`. `paths`
/// are the typed ones, empty for the config's own.
fn list_files(
    config_path: &Path,
    tags: &[String],
    env: Option<&str>,
    paths: &[PathBuf],
    privacy_args: &crate::PrivacyArgs,
    notices: &mut Vec<String>,
) -> Result<Vec<Value>> {
    let mut cfg = config::load(config_path, env)?;
    let mut overrides = settings::resolve(
        &settings::SettingsArgs::default(),
        &cfg.all_group_names(),
        &privacy::process_env,
    )?;
    if !tags.is_empty() {
        overrides.tags = Some(tags.to_vec());
    }
    cfg.apply(&overrides)?;
    let privacy = privacy::Privacy::from_sources(
        Some(&cfg.privacy),
        &privacy::process_env,
        privacy_args.layer(),
    )?;
    // Once per distinct text: `list-all` over many configs says it once, and
    // says it again only when a config reveals for another reason.
    if privacy.reveal && !notices.contains(&privacy.reveal_notice()) {
        eprintln!("{}", privacy.reveal_notice());
        notices.push(privacy.reveal_notice());
    }
    let redactor = privacy::Redactor::new(&privacy, privacy::recorded());
    let mut trace = events::Trace::off();
    trace.set_redactor(redactor);

    let filter = feature::TagFilter::new(&cfg.tags);
    let selected = if paths.is_empty() {
        cfg.paths.as_slice()
    } else {
        paths
    };
    let loaded = crate::select::select_listed(selected, !paths.is_empty(), &filter)?
        .map_err(anyhow::Error::msg)?;

    Ok(loaded
        .iter()
        .map(|selected| {
            let lf = match selected {
                Selected::File(lf) => lf,
                Selected::Unparsed(broken) => return unparsed_file(broken),
            };
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
        .collect())
}

#[derive(Args)]
pub struct ListAllArgs {
    /// Every config to list, each as `list --config <file>` would
    #[arg(required = true, num_args = 1..)]
    configs: Vec<PathBuf>,
    /// List only scenarios with one of these tags (repeatable); applies to every config
    #[arg(long = "tag")]
    tags: Vec<String>,
    /// Override APP_ENV for every config: selects .env.<name> / .env.<name>.local
    #[arg(long = "env")]
    env: Option<String>,
    /// Machine-readable output: `{"bddkit": …, "configs": […]}`, always an array
    #[arg(long)]
    json: bool,
    #[command(flatten)]
    privacy: crate::PrivacyArgs,
}

/// Exit 0 once every config has an entry, listed or with an `error`; 2 only for
/// stdout that cannot be written (a usage error is clap's own 2).
pub fn command_all(args: &ListAllArgs) -> i32 {
    privacy::set_flags(args.privacy.layer());
    let mut seen = std::collections::HashSet::new();
    let mut notices = Vec::new();
    let entries: Vec<Value> = args
        .configs
        .iter()
        .filter(|path| seen.insert(std::fs::canonicalize(path).unwrap_or_else(|_| (*path).clone())))
        .map(|path| {
            // One config at a time, each as if alone: nothing the previous one
            // recorded may mask, or be masked in, this one.
            privacy::forget();
            list_one(path, args, &mut notices)
        })
        .collect();
    let list = json!({"bddkit": env!("CARGO_PKG_VERSION"), "configs": entries});
    match print_all(&list, args.json) {
        Ok(()) => 0,
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => 0,
        Err(error) => {
            eprintln!("error: {error}, nothing listed");
            2
        }
    }
}

fn list_one(path: &Path, args: &ListAllArgs, notices: &mut Vec<String>) -> Value {
    let name = feature::display_path(path);
    let selection = json!({"tags": args.tags});
    if let Some(message) = not_a_suite(path) {
        return json!({"config": name, "error": {"kind": "not_a_suite", "message": message}});
    }
    match list_files(
        path,
        &args.tags,
        args.env.as_deref(),
        &[],
        &args.privacy,
        notices,
    ) {
        Ok(files) => json!({"config": name, "selection": selection, "files": files}),
        Err(error) => {
            let message = privacy::mask_text(&format!("{error:#}"));
            json!({"config": name, "error": {"kind": "invalid", "message": message}})
        }
    }
}

/// Why a file cannot be a bddkit config at all, read from the raw text before
/// any `${VAR}` expansion (a foreign file has its own `$`s). A mapping with
/// `paths` and `resources` is a suite whatever else is wrong with it. Text that
/// does not parse yet (`paths: [${ROOT}/x]` is no YAML until expanded) is a
/// suite when it has both keys at the start of a line.
/// ponytail: a one-line flow document (`{paths: …}`) that does not parse raw is
/// taken for a foreign file; expand `${…}` first if that bites.
fn not_a_suite(path: &Path) -> Option<String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
            return Some("not UTF-8 text".into());
        }
        // Unreadable is a config somebody named: the ordinary error follows.
        Err(_) => return None,
    };
    let missing = |has: &dyn Fn(&str) -> bool| {
        ["paths", "resources"]
            .into_iter()
            .find(|key| !has(key))
            .map(|key| format!("missing field `{key}`"))
    };
    match serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&raw) {
        Ok(serde_yaml_ng::Value::Mapping(map)) => missing(&|key| map.contains_key(key)),
        Ok(_) => Some("not a YAML mapping".into()),
        Err(_) => {
            missing(&|key| starts_a_line(&raw, key)).map(|_| "not a single YAML document".into())
        }
    }
}

/// A top-level `key:` at column 0, the key optionally quoted.
fn starts_a_line(raw: &str, key: &str) -> bool {
    raw.lines().any(|line| {
        let line = line.strip_prefix(['"', '\'']).unwrap_or(line);
        line.strip_prefix(key).is_some_and(|rest| {
            rest.trim_start_matches(['"', '\''])
                .trim_start()
                .starts_with(':')
        })
    })
}

/// `--json`: one line. Text: a `# <config>` heading per config, then the
/// lines `list` prints; an error is told inline.
fn print_all(list: &Value, json: bool) -> std::io::Result<()> {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    if json {
        return writeln!(out, "{list}");
    }
    for entry in list["configs"].as_array().into_iter().flatten() {
        writeln!(out, "# {}", entry["config"].as_str().unwrap_or_default())?;
        if let Some(error) = entry.get("error") {
            writeln!(
                out,
                "error ({}): {}",
                error["kind"].as_str().unwrap_or_default(),
                error["message"].as_str().unwrap_or_default()
            )?;
            continue;
        }
        write_files(&mut out, entry, false)?;
    }
    Ok(())
}

/// A file that did not parse: its path and the `error`, no `scenarios`. The
/// message is masked like every other text, though gherkin's never quotes the
/// file.
fn unparsed_file(broken: &Unparsed) -> Value {
    let mut error = json!({"message": privacy::mask_text(&broken.message)});
    if let Some(line) = broken.line {
        error["line"] = line.into();
    }
    json!({"file": feature::display_path(&broken.path), "error": error})
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
    write_files(&mut out, list, true)
}

/// The `file:LINE  name` lines of one listing. A broken file is told on stderr
/// by `list`, whose stdout is what `run` takes back, and inline by `list-all`.
fn write_files(
    out: &mut impl std::io::Write,
    list: &Value,
    errors_on_stderr: bool,
) -> std::io::Result<()> {
    for file in list["files"].as_array().into_iter().flatten() {
        let path = file["file"].as_str().unwrap_or_default();
        if let Some(error) = file.get("error") {
            let message = error["message"].as_str().unwrap_or_default();
            if errors_on_stderr {
                eprintln!("error: {message}");
            } else {
                writeln!(out, "error: {path}: {message}")?;
            }
            continue;
        }
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
