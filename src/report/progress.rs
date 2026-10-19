//! The one line `bddkit report` draws on stderr while it works.

use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use std::io::{IsTerminal, Read};
use std::sync::atomic::{AtomicUsize, Ordering};

/// A person at a terminal: stderr is one and `CI` is unset or empty. The only
/// definition of it, so every command that draws or asks agrees.
pub fn interactive() -> bool {
    std::io::stderr().is_terminal() && std::env::var_os("CI").is_none_or(|v| v.is_empty())
}

/// At most two stages, numbered over the ones that run. Drawn at most ten times
/// a second; hidden when nobody is looking.
pub struct Progress {
    bar: ProgressBar,
    stages: usize,
    started: AtomicUsize,
}

impl Progress {
    pub fn new(interactive: bool, stages: usize) -> Self {
        let bar = if interactive {
            ProgressBar::with_draw_target(None, ProgressDrawTarget::stderr_with_hz(10))
        } else {
            ProgressBar::hidden()
        };
        Self {
            bar,
            stages,
            started: AtomicUsize::new(0),
        }
    }

    /// Starts the next stage over `total` bytes or files.
    pub fn stage(&self, name: &str, total: u64, bytes: bool) {
        let n = self.started.fetch_add(1, Ordering::Relaxed) + 1;
        let eta = if bytes { "" } else { " ETA {eta}" };
        let counts = if bytes {
            "{bytes}/{total_bytes}"
        } else {
            "{pos}/{len}"
        };
        let template = format!("[{n}/{}] {name} {{bar:30}} {counts}{eta}", self.stages);
        self.bar
            .set_style(ProgressStyle::with_template(&template).expect("a constant template"));
        self.bar.set_length(total);
        self.bar.set_position(0);
    }

    pub fn inc(&self, by: u64) {
        self.bar.inc(by);
    }

    pub fn clear(&self) {
        self.bar.finish_and_clear();
    }
}

/// Counts what passes through, so reading the stream can move the bar.
pub struct Counting<'a, R> {
    pub inner: R,
    pub progress: &'a Progress,
}

impl<R: Read> Read for Counting<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.progress.inc(n as u64);
        Ok(n)
    }
}
