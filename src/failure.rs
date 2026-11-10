//! Why a scenario failed, as data. The class is set where the error is born
//! and travels with it; it is never parsed back out of the message, and there
//! is deliberately no `From<String>`: a silent `Unclassified` is how the
//! scheme would rot.

use std::fmt;

/// The class of the failure that ended a scenario. Every `match` on it is
/// exhaustive, with no catch-all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FailureKind {
    /// A check did not hold, including after polling ran out.
    Assertion,
    /// The service under test was not reachable.
    Transport,
    /// A database connection could not be opened or was lost.
    DbConnection,
    /// The database refused a statement.
    DbStatement,
    /// The scenario itself is wrong: an undefined variable, a mistyped value.
    Input,
    /// A plugin answered `fatal`, or did not answer.
    Plugin,
    /// A plugin's per-scenario reset failed.
    Reset,
    /// The file panicked: a bug in bddkit.
    Panic,
    /// A failure no class fits: only a host fault (the clock, the entropy) is
    /// built as this, through `StepError::host`.
    Unclassified,
}

impl FailureKind {
    /// The name the stream, the JUnit `type` and the console summary use.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Assertion => "assertion",
            Self::Transport => "transport",
            Self::DbConnection => "db_connection",
            Self::DbStatement => "db_statement",
            Self::Input => "input",
            Self::Plugin => "plugin",
            Self::Reset => "reset",
            Self::Panic => "panic",
            Self::Unclassified => "unclassified",
        }
    }
}

impl fmt::Display for FailureKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A step's failure: its class and the text a person reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepError {
    pub kind: FailureKind,
    pub message: String,
    /// What a plugin's failing reply showed besides its message, already
    /// rendered: the console dump appends it once. It is kept apart from
    /// `message` so a polled assertion's `attempt` event and the timeout text
    /// quote the mismatch alone, and it survives `wrap`.
    pub diagnostics: Vec<String>,
    /// The plugin handle (library index, handle) whose reply caused this
    /// failure, so the failure dump asks every OTHER handle of the scenario
    /// for its context, and this one too when the reply showed nothing. Kept
    /// through `wrap` and a polling timeout like `diagnostics`.
    pub handle: Option<(usize, u64)>,
}

impl StepError {
    pub fn new(kind: FailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            diagnostics: Vec::new(),
            handle: None,
        }
    }

    /// The same error, blamed on the plugin handle that replied.
    pub fn with_handle(mut self, handle: Option<(usize, u64)>) -> Self {
        self.handle = handle;
        self
    }

    /// The same error with the diagnostics of the reply that caused it.
    pub fn with_diagnostics(mut self, diagnostics: Vec<String>) -> Self {
        self.diagnostics = diagnostics;
        self
    }

    pub fn assertion(message: impl Into<String>) -> Self {
        Self::new(FailureKind::Assertion, message)
    }

    pub fn input(message: impl Into<String>) -> Self {
        Self::new(FailureKind::Input, message)
    }

    /// The host failed (the clock, the system entropy): neither the scenario
    /// nor the service is at fault, so no class fits.
    pub fn host(message: impl Into<String>) -> Self {
        Self::new(FailureKind::Unclassified, message)
    }

    pub fn plugin(message: impl Into<String>) -> Self {
        Self::new(FailureKind::Plugin, message)
    }

    /// The same class, a message built from the old one: what a macro or an
    /// include does when it adds its own context lines.
    pub fn wrap(self, f: impl FnOnce(&str) -> String) -> Self {
        Self {
            message: f(&self.message),
            ..self
        }
    }
}

impl fmt::Display for StepError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
