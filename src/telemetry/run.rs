//! What `run` learns about itself as it goes, kept apart from `run` so the
//! payload is assembled in one place and every way out of `run` — a refusal
//! with a stage, the tail's report and stream failures, the normal end — hands
//! over the same thing.

use super::counters::Counters;
use super::payload::{
    Command, Failures, Features, Payload, PrivacyFlags, Resources, RunFields, ScenarioMs, Stage,
};
use super::{Decision, Ending};
use crate::dirs::Layer;
use crate::report::FileResult;
use std::sync::Arc;
use std::time::Duration;

#[derive(Default)]
pub struct RunProbe {
    /// The config was read. Before that a run reports nothing, so a config the
    /// team's own `telemetry: false` lives in is never reported on blind.
    pub read: bool,
    pub decision: Option<Decision>,
    pub layers: Vec<Layer>,
    /// The phase a refusal happened in; `None` once the run has started.
    pub stage: Option<Stage>,
    /// The first request is about to be made. Not `stage.is_none()`: that is
    /// also true before the first phase, while an option can still be refused.
    pub started: bool,
    pub concurrency: u64,
    pub fail_fast: bool,
    pub features: Features,
    pub privacy: Option<PrivacyFlags>,
    pub resources: Resources,
    pub counters: Option<Arc<Counters>>,
    files: u64,
    failed: u64,
    failures: Failures,
    durations: Vec<Duration>,
}

impl RunProbe {
    /// The scenario figures, from what the run produced.
    pub fn tally(&mut self, results: &[FileResult]) {
        self.files = results.len() as u64;
        self.failed = results.iter().map(|file| file.failed() as u64).sum();
        self.failures = Failures::of(results);
        self.durations = results
            .iter()
            .flat_map(|file| file.scenarios.iter().map(|s| s.duration))
            .collect();
    }

    pub fn payload(&self, exit: i32, elapsed: Duration) -> Payload {
        let mut payload = Payload::new(Command::Run, exit, elapsed);
        payload.stage = self.stage;
        payload.run = Some(RunFields {
            files: self.files,
            scenarios: self.durations.len() as u64,
            examples: self.counters.as_ref().map_or(0, |c| c.examples()),
            failed: self.failed,
            failures: self.failures.clone(),
            concurrency: self.concurrency,
            fail_fast: self.fail_fast,
            scenario_ms: ScenarioMs::of(&self.durations),
            features: self.features.clone(),
            usage: self
                .counters
                .as_ref()
                .map(|c| c.usage())
                .unwrap_or_default(),
        });
        payload.privacy = self.privacy.clone();
        payload.resources = Some(self.resources.clone());
        payload
    }

    /// The run is over, whichever way. A run that never read its config, or
    /// whose ending a signal took, reports nothing. The question is asked only
    /// after a run that started and returned normally: a refused one has an
    /// error to read first, and `main` prints it after `run` returns.
    pub async fn finish(self, exit: i32, elapsed: Duration, completed: bool) {
        let (true, Some(decision)) = (self.read, &self.decision) else {
            return;
        };
        let payload = self.payload(exit, elapsed);
        let mut interrupts = crate::Interrupts::install();
        super::end(
            Ending {
                payload: &payload,
                decision: decision.clone(),
                layers: &self.layers,
                ask: completed && self.started && self.stage.is_none(),
            },
            &mut interrupts,
        )
        .await;
    }
}

/// The counts of what a config declares: APIs, SRP configurations and database
/// engines. Plugin instances are added once the plugins are loaded. An engine
/// is named by its DSN scheme, so MariaDB reads as `mysql` until a connection
/// has told them apart (`Db::engines`).
pub fn resources_of(cfg: &crate::config::Config) -> Resources {
    let mut resources = Resources {
        api: cfg.resources.api.len() as u64,
        srp: cfg.resources.srp.len() as u64,
        ..Default::default()
    };
    for connection in cfg.resources.db.values() {
        if let Some(engine) = crate::db::family_name(&connection.dsn) {
            resources.db.add(engine);
        }
    }
    resources
}
