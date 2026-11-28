//! What `--evidence` records, and how much of it a scenario may hold: the
//! values of the flag, the environment variables and the config key.

use anyhow::Result;
use serde::{Deserialize, Deserializer};

/// What `--evidence` records: the `marked` steps, every step of a scenario
/// that failed (`failures`), or `all`. The flag, `$BDDKIT_EVIDENCE` and the config key are
/// read by the one parser here, so all three refuse the same names.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sources {
    pub(super) marked: bool,
    pub(super) failures: bool,
    pub(super) all: bool,
}

impl Sources {
    /// A comma-separated list, as the flag and the variable carry it.
    pub fn parse(text: &str) -> Result<Self, String> {
        text.split(',')
            .try_fold(Self::default(), |sources, name| sources.with(name.trim()))
    }

    fn with(mut self, name: &str) -> Result<Self, String> {
        match name {
            "marked" => self.marked = true,
            "failures" => self.failures = true,
            "all" => self.all = true,
            _ => {
                return Err(format!(
                    "unknown evidence source {name:?}: expected marked, failures or all"
                ));
            }
        }
        Ok(self)
    }

    /// True when anything is recorded at all.
    pub fn any(self) -> bool {
        self.marked || self.failures || self.all
    }

    /// For `run_started.evidence_sources`: canonical order, each once.
    pub fn names(self) -> Vec<&'static str> {
        [
            (self.marked, "marked"),
            (self.failures, "failures"),
            (self.all, "all"),
        ]
        .into_iter()
        .filter_map(|(on, name)| on.then_some(name))
        .collect()
    }
}

/// `$BDDKIT_EVIDENCE`, as `run` reads it and `doctor` checks it.
pub fn sources_from_env() -> Result<Option<Sources>> {
    crate::privacy::process_env("BDDKIT_EVIDENCE")
        .map(|value| Sources::parse(&value))
        .transpose()
        .map_err(|error| anyhow::anyhow!("$BDDKIT_EVIDENCE: {error}"))
}

/// The default of `$BDDKIT_EVIDENCE_BUFFER`: 64 MiB.
pub const EVIDENCE_BUFFER: usize = 64 * 1024 * 1024;

/// `$BDDKIT_EVIDENCE_BUFFER`, in bytes: what a scenario may hold of long
/// deferred text before it goes to disk. Read by `run`, checked by `doctor`.
pub fn buffer_from_env() -> Result<usize> {
    crate::privacy::process_env("BDDKIT_EVIDENCE_BUFFER").map_or(Ok(EVIDENCE_BUFFER), |value| {
        value.parse().map_err(|_| {
            anyhow::anyhow!("$BDDKIT_EVIDENCE_BUFFER: expected a number of bytes, got {value:?}")
        })
    })
}

/// The config key is a YAML list, `evidence: [marked, failures]`.
impl<'de> Deserialize<'de> for Sources {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Vec::<String>::deserialize(deserializer)?
            .iter()
            .try_fold(Self::default(), |sources, name| sources.with(name))
            .map_err(serde::de::Error::custom)
    }
}
