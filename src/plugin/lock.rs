//! Where plugins come from. Provisioning is machine state and never appears in
//! the test config: the test config is committed and describes the system
//! under test, while a `.so` path describes one machine.

use crate::dirs::Candidate;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// One installed plugin. `bddkit plugin install` (P2) also writes `version`,
/// `source`, `sha256`, `target`, `groups` and `abi`; P1 neither reads nor
/// declares them, because serde ignores unknown keys unless a struct asks for
/// `deny_unknown_fields` — and declaring a field just to discard it would
/// suggest the host validates something it does not. The manifest inside the
/// binary is authoritative here, not the lock file's cached copy of it.
#[derive(Debug, Clone, Deserialize)]
pub struct LockEntry {
    pub name: String,
    /// Absolute, or relative to the lock file's own directory (`.bddkit/`).
    /// `~` is NOT expanded — a `~/…` path reaches `dlopen` verbatim and fails
    /// with a confusing "no such file", so say so where the author will read it.
    pub path: PathBuf,
    /// The layer whose file this entry was read from (`user`, `project.local`,
    /// …). Set by `load`, never parsed — it is what `bddkit doctor` prints.
    #[serde(skip)]
    pub layer: String,
}

#[derive(Debug, Deserialize)]
struct LockFile {
    #[serde(default)]
    plugin: Vec<LockEntry>,
}

/// Reads the candidates in order; a later entry overrides an earlier one of
/// the same name, so a project can pin the version its CI uses without the
/// developer losing the plugins installed globally, and `plugins.local.yaml`
/// can point a committed entry at a local build. Two entries with one name in
/// the same file collapse to the last through the same loop.
pub fn load(candidates: &[Candidate]) -> Result<Vec<LockEntry>> {
    let mut entries: Vec<LockEntry> = Vec::new();
    for candidate in candidates {
        for mut entry in read_file(&candidate.path)? {
            entry.layer = candidate.layer.clone();
            match entries.iter_mut().find(|e| e.name == entry.name) {
                Some(existing) => *existing = entry,
                None => entries.push(entry),
            }
        }
    }
    Ok(entries)
}

fn read_file(path: &Path) -> Result<Vec<LockEntry>> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        // No lock file means no plugins, which is the normal case.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("failed to read {}", path.display())),
    };
    let parsed: LockFile = serde_yaml_ng::from_str(&raw)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    Ok(parsed
        .plugin
        .into_iter()
        .map(|mut entry| {
            if entry.path.is_relative() {
                entry.path = dir.join(&entry.path);
            }
            entry
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dirs::Candidate;

    fn write(dir: &Path, file: &str, body: &str) -> Candidate {
        std::fs::create_dir_all(dir).expect("mkdir");
        let path = dir.join(file);
        std::fs::write(&path, body).expect("write lock");
        Candidate { layer: "test".to_string(), path }
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bddkit-lock-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir.join(".bddkit")
    }

    #[test]
    fn parses_the_minimal_entry() {
        let dir = temp("minimal");
        let c = write(&dir, "plugins.yaml", "plugin:\n  - name: widget\n    path: /opt/libwidget.so\n");
        let entries = load(&[c]).expect("loads");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "widget");
        assert_eq!(entries[0].path, PathBuf::from("/opt/libwidget.so"));
        assert_eq!(entries[0].layer, "test");
    }

    #[test]
    fn a_lock_file_carrying_p2_provisioning_fields_still_parses() {
        // sha256/target/source/version are written by `plugin install` in P2.
        // P1 declares none of them and must not reject a file that has them.
        let dir = temp("extra-fields");
        let c = write(
            &dir,
            "plugins.yaml",
            concat!(
                "plugin:\n  - name: widget\n    path: /opt/libwidget.so\n    version: 1.2.0\n",
                "    source: https://github.com/example/bddkit-widget\n    sha256: e3b0c442\n",
                "    target: x86_64-unknown-linux-gnu\n",
            ),
        );
        assert_eq!(load(&[c]).expect("loads").len(), 1);
    }

    #[test]
    fn a_later_candidate_overrides_an_earlier_entry_of_the_same_name() {
        let user = temp("user");
        let project = temp("project");
        let mut u = write(&user, "plugins.yaml", "plugin:\n  - name: widget\n    path: /user/libwidget.so\n  - name: mail\n    path: /user/libmail.so\n");
        u.layer = "user".to_string();
        let mut p = write(&project, "plugins.yaml", "plugin:\n  - name: widget\n    path: /project/libwidget.so\n");
        p.layer = "project".to_string();
        let entries = load(&[u, p]).expect("loads");
        let widget = entries.iter().find(|e| e.name == "widget").expect("widget present");
        assert_eq!(widget.path, PathBuf::from("/project/libwidget.so"));
        assert_eq!(widget.layer, "project", "the entry remembers the layer that won");
        let mail = entries.iter().find(|e| e.name == "mail").expect("a user entry the project does not override survives");
        assert_eq!(mail.layer, "user");
    }

    #[test]
    fn local_overrides_base_in_the_same_directory() {
        let dir = temp("local");
        let base = write(&dir, "plugins.yaml", "plugin:\n  - name: widget\n    path: vendor/libwidget.so\n");
        let mut local = write(&dir, "plugins.local.yaml", "plugin:\n  - name: widget\n    path: /home/dev/target/debug/libwidget.so\n");
        local.layer = "test.local".to_string();
        let entries = load(&[base, local]).expect("loads");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, PathBuf::from("/home/dev/target/debug/libwidget.so"));
        assert_eq!(entries[0].layer, "test.local");
    }

    #[test]
    fn a_missing_lock_file_is_not_an_error() {
        // Most runs have no plugins at all; a missing file means "none".
        let dir = temp("absent");
        let c = Candidate { layer: "test".to_string(), path: dir.join("plugins.yaml") };
        assert!(load(&[c]).expect("loads").is_empty());
    }

    #[test]
    fn a_malformed_lock_file_is_an_error() {
        let dir = temp("malformed");
        let c = write(&dir, "plugins.yaml", "plugin:\n  - name: widget\n");
        let error = load(&[c]).expect_err("path is required");
        assert!(format!("{error:#}").contains("plugins.yaml"), "{error:#}");
    }

    #[test]
    fn a_lock_file_with_no_plugin_key_yields_no_entries() {
        let dir = temp("empty");
        let c = write(&dir, "plugins.yaml", "{}\n");
        assert!(load(&[c]).expect("loads").is_empty());
    }

    #[test]
    fn a_relative_path_resolves_against_the_lock_file_directory() {
        // A committed project lock referring to ./vendor/libwidget.so must work
        // regardless of the working directory the run was started from.
        let dir = temp("relative");
        let c = write(&dir, "plugins.yaml", "plugin:\n  - name: widget\n    path: vendor/libwidget.so\n");
        let entries = load(&[c]).expect("loads");
        assert_eq!(entries[0].path, dir.join("vendor/libwidget.so"));
    }
}
