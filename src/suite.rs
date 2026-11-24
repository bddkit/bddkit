//! The stages `run_suite` runs in order. Each is a step of the sequence in
//! `main.rs`; none reorders it or changes what the run does between them.

use super::{Interrupts, RunArgs, env_var};
use crate::{config, events, feature, privacy, report, runner, select, telemetry, validate};
use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;

/// The event stream asked for, opened once and held (see `events::open`), and
/// the evidence sources a flag or the environment named.
pub(super) type Streams = (
    Option<std::fs::File>,
    Option<(&'static str, events::Sources)>,
);

/// Opened once, here: nothing is written to the stream until the run has
/// started, so every refusal after this leaves it empty.
pub(super) fn open_streams(cli: &RunArgs) -> Result<Streams> {
    let events_path = cli
        .events
        .clone()
        .or_else(|| env_var("BDDKIT_EVENTS").map(PathBuf::from));
    let events_file = events_path
        .as_ref()
        .map(|path| events::open(path))
        .transpose()?;
    let evidence = match cli.evidence {
        Some(sources) => Some(("--evidence", sources)),
        None => events::sources_from_env()?.map(|sources| ("$BDDKIT_EVIDENCE", sources)),
    };
    // `$BDDKIT_EVENTS` counts, so this is not clap's `requires`.
    if let (Some((source, _)), None) = (evidence, &events_file) {
        anyhow::bail!("{source} requires --events (or $BDDKIT_EVENTS)");
    }
    Ok((events_file, evidence))
}

/// The config, with the flags and the environment laid over it.
pub(super) fn load_config(
    cli: &RunArgs,
) -> Result<(config::Config, PathBuf, config::ConfigSource)> {
    let resolved = config::resolve_config_path(cli.config.as_deref())?
        .ok_or_else(|| anyhow::anyhow!("{}", config::NO_CONFIG_FOUND))?;
    let config_path = resolved.path;
    let mut cfg = config::load(&config_path, cli.env.as_deref())?;
    let mut overrides =
        super::settings::resolve(&cli.settings, &cfg.all_group_names(), &privacy::process_env)?;
    // `run`'s own two flags sit on top of the environment.
    overrides.fail_fast = cli.fail_fast.or(overrides.fail_fast);
    if !cli.tags.is_empty() {
        overrides.tags = Some(cli.tags.clone());
    }
    cfg.apply(&overrides)?;
    Ok((cfg, config_path, resolved.source))
}

/// The files the run would start, after the selection and the static check.
/// `None`: a refusal was printed and the run exits 2.
pub(super) fn select_and_validate(
    cli: &RunArgs,
    cfg: &config::Config,
    registry: &crate::steps::Registry,
    filter: &feature::TagFilter,
    probe: &mut telemetry::RunProbe,
) -> Result<Option<Vec<Arc<feature::LoadedFeature>>>> {
    probe.stage = Some(telemetry::payload::Stage::Selection);
    let paths = if cli.paths.is_empty() {
        cfg.paths.as_slice()
    } else {
        cli.paths.as_slice()
    };
    // Only a typed argument carries a line: the config's paths are plain.
    let loaded = match select::select_features(paths, !cli.paths.is_empty(), filter)? {
        Ok(loaded) => loaded,
        Err(message) => {
            eprintln!("error: {message}, run not started");
            return Ok(None);
        }
    };
    if loaded.is_empty() {
        eprintln!("error: no scenario selected, run not started");
        return Ok(None);
    }

    probe.stage = Some(telemetry::payload::Stage::Validation);
    let for_check: Vec<&feature::LoadedFeature> = loaded.iter().map(Arc::as_ref).collect();
    let problems = validate::check(&for_check, registry, filter);
    if !problems.is_empty() {
        eprintln!("error: {} problem(s), run not started\n", problems.len());
        for p in &problems {
            eprintln!("{}", privacy::mask_text(&p.to_string()));
        }
        return Ok(None);
    }
    Ok(Some(loaded))
}

/// A plugin group that asked for `eager_init` is connected here, so an
/// unreachable resource is exit 2 before `run <id>`. Blocking FFI, off the
/// executor; a handle that did come up is swept before the process leaves.
pub(super) async fn eager_init(plugins: Option<&Arc<crate::plugin::Plugins>>) -> Result<()> {
    let Some(plugins) = plugins else {
        return Ok(());
    };
    let eager = Arc::clone(plugins);
    let outcome = tokio::task::spawn_blocking(move || eager.eager_init())
        .await
        .map_err(anyhow::Error::new)?;
    if let Err(error) = outcome {
        let sweep = Arc::clone(plugins);
        let _ = tokio::task::spawn_blocking(move || sweep.shutdown()).await;
        return Err(anyhow::Error::msg(error));
    }
    Ok(())
}

/// What the `run_started` line of the event stream carries.
pub(super) struct Started<'a> {
    pub cli: &'a RunArgs,
    pub cfg: &'a config::Config,
    pub config_path: &'a std::path::Path,
    pub source: config::ConfigSource,
    pub evidence: events::Sources,
    pub privacy: &'a privacy::Privacy,
    pub run_id: &'a str,
    pub artifacts_dir: String,
    pub files: usize,
}

impl Started<'_> {
    pub(super) fn payload(&self) -> serde_json::Value {
        serde_json::json!({
            "schema": 1,
            "bddkit": env!("CARGO_PKG_VERSION"),
            "run_id": self.run_id,
            "started_at_unix_ms": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(0)),
            "artifacts_dir": self.artifacts_dir,
            "config": feature::display_path(self.config_path),
            "config_source": self.source,
            "selection": {
                "tags": self.cli.tags,
                "paths": self.cli.paths.iter().map(|p| feature::display_path(p)).collect::<Vec<_>>(),
            },
            "evidence": self.evidence.any(),
            "evidence_sources": self.evidence.names(),
            "concurrency": self.cfg.concurrency,
            "privacy": {
                "masking": !self.privacy.reveal,
                "words": self.privacy.words,
                "fields": self.privacy.fields,
                "min_length": self.privacy.min_length,
            },
            "files": self.files,
        })
    }
}

/// The one deliberate exception to "2 = before the first request": a
/// report silently lost behind a green code is the worse outcome.
pub(super) fn write_reports(
    cli: &RunArgs,
    results: &[report::FileResult],
    code: i32,
    probe: &mut telemetry::RunProbe,
) -> i32 {
    if let Err(error) = cli.reports.write(results) {
        eprintln!("error: {}", privacy::mask_text(&format!("{error:#}")));
        probe.stage = Some(telemetry::payload::Stage::Reports);
        return 2;
    }
    code
}

/// Ends the event stream with `code`, after the reports, so `exit` is the code
/// the process really returns — and the same exception covers a stream that
/// could not be written. Returns the code the run ends with.
pub(super) async fn finish_events(
    ctx: &runner::RunContext,
    code: i32,
    tail_interrupts: &mut Interrupts,
    probe: &mut telemetry::RunProbe,
) -> i32 {
    let Some(events) = ctx.events.clone() else {
        return code;
    };
    let mut flush =
        tokio::task::spawn_blocking(move || events.finish(serde_json::json!({"exit": code})));
    // The same two-signal rule as the handler's: the run is over and its
    // code is settled, so a first signal only says «still writing» and the
    // queued lines are not thrown away; a second one exits at once, with
    // the first one's code and the stream cut short. Without the second
    // way out a reader that stopped reading would make the process
    // unkillable but for SIGKILL.
    let stream = tokio::select! {
        stream = &mut flush => stream,
        (name, first) = tail_interrupts.wait() => {
            eprintln!("\n{name}: still writing the event stream; signal again to exit now");
            tokio::select! {
                stream = flush => stream,
                _ = tail_interrupts.wait() => std::process::exit(first),
            }
        }
    };
    if let Ok(Err(error)) = stream {
        eprintln!(
            "error: cannot write the events file: {}",
            privacy::mask_text(&error.to_string())
        );
        probe.stage = Some(telemetry::payload::Stage::Events);
        return 2;
    }
    code
}

/// The evidence sources the run uses and the size of the buffer that defers
/// them. The config key is the suite's default: a run without a stream
/// ignores it. Notes the config for telemetry on the way.
pub(super) fn resolve_evidence(
    cli: &RunArgs,
    cfg: &config::Config,
    config_path: &std::path::Path,
    streams: &Streams,
    probe: &mut telemetry::RunProbe,
) -> Result<(events::Sources, usize)> {
    let (events_file, chosen) = streams;
    let evidence = match chosen {
        Some((_, sources)) => *sources,
        None if events_file.is_some() => cfg.evidence.unwrap_or_default(),
        None => events::Sources::default(),
    };
    super::note_config(
        probe,
        cli,
        cfg,
        config_path,
        events_file.is_some(),
        evidence,
    );
    // Only a run that defers anything has a use for it.
    let buffer = if evidence.any() {
        events::buffer_from_env()?
    } else {
        events::EVIDENCE_BUFFER
    };
    Ok((evidence, buffer))
}

/// The privacy settings of the run, noted for telemetry. A revealed run says so.
pub(super) fn resolve_privacy(
    cli: &RunArgs,
    cfg: &config::Config,
    probe: &mut telemetry::RunProbe,
) -> Result<privacy::Privacy> {
    let privacy = privacy::Privacy::from_sources(
        Some(&cfg.privacy),
        &privacy::process_env,
        cli.privacy.layer(),
    )?;
    probe.privacy = Some(telemetry::payload::PrivacyFlags::of(&privacy));
    if privacy.reveal {
        eprintln!("{}", privacy.reveal_notice());
    }
    Ok(privacy)
}

/// Loads the plugins and refuses an instance a plugin did not accept. Non-strict
/// load, so the refusal of an instance is told apart from a plugin that never
/// loaded: the first is a resource the host cannot build.
pub(super) fn load_plugins(
    cli: &RunArgs,
    cfg: &config::Config,
    config_path: &std::path::Path,
    probe: &mut telemetry::RunProbe,
) -> Result<Option<Arc<crate::plugin::Plugins>>> {
    let env = crate::dirs::Env::from_process(cli.dir.bddkit_dir.clone());
    probe.stage = Some(telemetry::payload::Stage::Plugins);
    let plugins = super::load_plugins_collecting(config_path, cfg, &env, false)?;
    probe.stage = Some(telemetry::payload::Stage::Resources);
    if let Some(plugins) = &plugins {
        plugins.ensure_valid()?;
        (probe.resources.plugins, probe.resources.plugins_other) = plugins.instance_counts();
    }
    Ok(plugins)
}

/// The step registry, or the refusal printed. `None`: the run exits 2.
pub(super) fn registry_or_refusal(
    cfg: &config::Config,
    plugins: Option<&Arc<crate::plugin::Plugins>>,
    probe: &mut telemetry::RunProbe,
) -> Option<crate::steps::Registry> {
    probe.stage = Some(telemetry::payload::Stage::Steps);
    match super::build_registry(cfg, plugins) {
        Ok(registry) => Some(registry),
        Err(error) => {
            eprintln!(
                "error: 1 problem, run not started\n\n  {}",
                privacy::mask_text(&error)
            );
            None
        }
    }
}

/// The refusal a command ends in. `run` and `report` print it themselves,
/// before telemetry's request, so the person is not left waiting on the
/// network for an error that is already known.
pub(crate) fn print_refusal(error: &anyhow::Error, nothing_happened: &str) {
    eprintln!(
        "error: {}\n\n{nothing_happened}",
        privacy::mask_text(&format!("{error:#}"))
    );
}
