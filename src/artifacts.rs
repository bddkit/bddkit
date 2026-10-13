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

/// What a consumer needs before it has read a file; from the extension alone.
pub fn media_type(name: &str) -> &'static str {
    let extension = name.rsplit_once('.').map_or("", |(_, e)| e);
    match extension.to_ascii_lowercase().as_str() {
        "json" => "application/json",
        "txt" | "log" => "text/plain",
        "html" | "htm" => "text/html",
        "xml" => "application/xml",
        "csv" => "text/csv",
        "yaml" | "yml" => "application/yaml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

impl Artifacts {
    /// Writes `bytes` as `<root>/<counter>/<name>` and returns the path the
    /// stream carries: relative to the root, forward slashes. The file is
    /// complete when this returns, which is what lets an event point at it.
    pub fn write(&self, name: &str, bytes: &[u8]) -> std::io::Result<String> {
        let dir = self.next_dir();
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join(name), bytes)?;
        Ok(self.relative(&dir.join(name)).unwrap_or_default())
    }

    /// `path` as the stream writes it, when it lies under the root.
    pub fn relative(&self, path: &Path) -> Option<String> {
        let inside = path.strip_prefix(&self.root).ok()?;
        // Lexical only: a `..` would name a file outside the root.
        if !inside
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)))
        {
            return None;
        }
        Some(
            inside
                .components()
                .map(|part| part.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/"),
        )
    }

    /// Copies a file from outside the root in, under its own file name.
    pub fn copy_in(&self, source: &Path) -> std::io::Result<String> {
        let name = source
            .file_name()
            .map_or_else(|| "file".into(), |n| n.to_string_lossy());
        let dir = self.next_dir();
        std::fs::create_dir_all(&dir)?;
        std::fs::copy(source, dir.join(&*name))?;
        Ok(self.relative(&dir.join(&*name)).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_that_climbs_out_of_the_root_is_not_inside_it() {
        let artifacts = Artifacts::new(Some("/evidence".into()), "r1").expect("resolves");
        assert_eq!(
            artifacts.relative(Path::new("/evidence/000003/a.png")),
            Some("000003/a.png".into())
        );
        assert_eq!(
            artifacts.relative(Path::new("/evidence/000003/../../etc/x")),
            None
        );
        assert_eq!(artifacts.relative(Path::new("/elsewhere/a.png")), None);
    }

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
