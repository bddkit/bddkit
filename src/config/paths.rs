use super::Config;
use std::path::{Component, Path, PathBuf};

impl Config {
    /// A relative `paths` / `macro_paths` entry written in the file means "next
    /// to the file", wherever bddkit is started from. Under the working
    /// directory the result stays relative to it, outside it the result is
    /// absolute, so every path bddkit prints is one a tester can paste back.
    /// Paths from a flag or the environment replace these later and are not
    /// touched.
    pub(super) fn resolve_paths(&mut self, config_dir: &Path) {
        let dir = shown_dir(config_dir);
        if dir.as_os_str().is_empty() {
            return;
        }
        for path in self.paths.iter_mut().chain(&mut self.macro_paths) {
            if path.is_relative() {
                *path = dir.join(&*path);
            }
        }
    }
}

/// A config's folder as bddkit prints it: relative under the working directory,
/// absolute outside it, empty for the working directory itself.
pub(super) fn shown_dir(config_dir: &Path) -> PathBuf {
    // `--config cfg.yaml` has the parent `""`, which `absolute` refuses.
    if config_dir.as_os_str().is_empty() {
        return PathBuf::new();
    }
    let absolute = std::path::absolute(config_dir).unwrap_or_else(|_| config_dir.to_path_buf());
    // `absolute` keeps `..`, and `cwd/../x` would still start with `cwd`.
    let absolute = absolute.components().fold(PathBuf::new(), |mut acc, part| {
        if part == Component::ParentDir {
            acc.pop();
        } else {
            acc.push(part);
        }
        acc
    });
    std::env::current_dir()
        .ok()
        .and_then(|cwd| absolute.strip_prefix(cwd).ok().map(Path::to_path_buf))
        .unwrap_or(absolute)
}
