//! The process-wide sink: what this process knows about privacy outside any file's own state.

use super::{DEFAULT_MIN_LENGTH, Privacy, PrivacyLayer, Secrets, process_env};
use std::sync::Mutex;

/// What this process knows about privacy before and outside any file's own
/// state: the flags `main` parsed, and the candidate secrets the configuration
/// holds. Text printed with no `World` around it — `error:` lines, `doctor`,
/// a driver's connection error — is masked from here.
struct Process {
    flags: PrivacyLayer,
    min_length: usize,
    reveal: bool,
    candidates: Vec<String>,
}

static PROCESS: Mutex<Process> = Mutex::new(Process {
    flags: PrivacyLayer {
        words: None,
        words_only: None,
        fields: None,
        fields_only: None,
        min_length: None,
        reveal: None,
    },
    min_length: DEFAULT_MIN_LENGTH,
    reveal: false,
    candidates: Vec::new(),
});

fn process() -> std::sync::MutexGuard<'static, Process> {
    // A panic elsewhere must not stop an error message from being masked.
    PROCESS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `main` hands over the flag layer once, before any config is read. The
/// vocabulary the flags and the environment alone select applies from here, so
/// an error raised before the config is parsed is masked the way they say.
pub fn set_flags(flags: PrivacyLayer) {
    let early = Privacy::from_sources(None, &process_env, flags.clone());
    process().flags = flags;
    if let Ok(privacy) = early {
        record(&privacy, []);
    }
}

pub fn flags() -> PrivacyLayer {
    process().flags.clone()
}

/// Records what a loaded configuration decided: its resolved vocabulary, and
/// the values it holds that must never be printed.
pub fn record(privacy: &Privacy, values: impl IntoIterator<Item = String>) {
    let mut process = process();
    process.min_length = privacy.min_length;
    process.reveal = privacy.reveal;
    process.candidates.extend(values);
}

/// Records one value a message is about to quote.
pub fn record_value(value: &str) {
    process().candidates.push(value.to_string());
}

/// The secrets recorded so far, filtered by the `min_length` in force.
pub fn recorded() -> Secrets {
    let process = process();
    let mut secrets = Secrets::new(process.min_length);
    process.candidates.iter().for_each(|value| {
        secrets.add(value);
    });
    secrets
}

/// `text` with the recorded secrets masked, or as it is under `--reveal`.
pub fn mask_text(text: &str) -> String {
    if process().reveal {
        return text.to_string();
    }
    recorded().mask(text).into_owned()
}
