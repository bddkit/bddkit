//! `doctor`'s side of telemetry: the row that says what was decided and by
//! what, and the payload of a `doctor` command. The row is `ok` either way and
//! never fails the report.

use super::payload::{Command, DoctorInfo, Payload, PrivacyFlags, Resources};
use super::{Decision, Ending};
use crate::dirs::Layer;
use crate::doctor::{Report, Status};
use std::time::Duration;

/// What `doctor::check` learned that the payload needs, kept off the report's
/// JSON: the report is the tester's, the payload is ours.
#[derive(Debug)]
pub struct Note {
    pub decision: Decision,
    pub layers: Vec<Layer>,
    pub privacy: Option<PrivacyFlags>,
    pub resources: Option<Resources>,
}

/// `on (decided by the user layer); <endpoint>/v1/run`, `off (decided by
/// DO_NOT_TRACK)`, the shape of the `privacy` row. A build with no endpoint
/// says so rather than naming an address.
pub fn row(decision: &Decision) -> String {
    let state = decision.describe();
    if !decision.on {
        return state;
    }
    match super::endpoint(&crate::privacy::process_env) {
        Some(endpoint) => format!("{state}; {endpoint}/v1/run"),
        None => format!("{state}; no endpoint in this build"),
    }
}

fn stages_failed(report: &Report) -> Vec<&'static str> {
    let mut stages: Vec<&'static str> = report
        .checks
        .iter()
        .filter(|check| check.status == Status::Failed)
        .map(|check| check.stage)
        .collect();
    stages.sort_unstable();
    stages.dedup();
    stages
}

pub fn payload(report: &Report, note: &Note, elapsed: Duration) -> Payload {
    let mut payload = Payload::new(Command::Doctor, report.exit_code(), elapsed);
    payload.privacy = note.privacy.clone();
    payload.resources = note.resources.clone();
    payload.doctor = Some(DoctorInfo {
        live: report.live,
        failed: report.failed() as u64,
        stages_failed: stages_failed(report),
    });
    payload
}

/// `doctor` has printed its report; record the payload or ask.
pub async fn finish(report: &Report, elapsed: Duration) {
    let Some(note) = &report.telemetry else {
        return;
    };
    let payload = payload(report, note, elapsed);
    let mut interrupts = crate::Interrupts::install();
    super::end(
        Ending {
            payload: &payload,
            decision: note.decision.clone(),
            layers: &note.layers,
            ask: true,
        },
        &mut interrupts,
    )
    .await;
}
