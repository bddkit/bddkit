use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// The run's evidence root and the allocator of per-dispatch directories under
/// it. One per run, owned by `RunContext`, so the host allocates a path with no
/// plugin loaded and a plugin gets its path from the same counter.
pub struct Artifacts {
    root: PathBuf,
    counter: AtomicUsize,
}

impl Artifacts {
    /// `--artifacts-dir` if given, else `<temp>/bddkit-<run_id>`; made absolute
    /// against the working directory, because the root is reported to
    /// consumers that do not share it. Not created and not checked: nothing
    /// exists under it until a plugin writes.
    pub fn new(explicit: Option<PathBuf>, run_id: &str) -> std::io::Result<Self> {
        let root =
            explicit.unwrap_or_else(|| std::env::temp_dir().join(format!("bddkit-{run_id}")));
        Ok(Self {
            root: std::path::absolute(root)?,
            counter: AtomicUsize::new(0),
        })
    }

    #[cfg(test)]
    pub fn for_test() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self::new(None, "test").expect("resolves"))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// A fresh `<root>/<six-digit counter>` per dispatch: two workers handed
    /// the same path would overwrite each other's evidence. The host does not
    /// create it — a plugin that writes calls `create_dir_all` first, and most
    /// steps never write anything.
    pub fn next_dir(&self) -> PathBuf {
        let index = self.counter.fetch_add(1, Ordering::Relaxed);
        self.root.join(format!("{index:06}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_given_root_is_used_and_made_absolute() {
        let artifacts = Artifacts::new(Some("out/evidence".into()), "r1").expect("resolves");
        assert!(artifacts.root().is_absolute());
        assert!(artifacts.root().ends_with("out/evidence"));
        assert_eq!(
            artifacts.next_dir(),
            artifacts.root().join("000000"),
            "the layout is <root>/<six-digit counter>"
        );
    }
}
