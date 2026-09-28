//! Where plugins come from. Provisioning is machine state and never appears in
//! the test config: the test config is committed and describes the system
//! under test, while a `.so` path describes one machine.

use crate::dirs::Candidate;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_yaml_ng::Value;
use std::path::{Path, PathBuf};

/// One installed plugin. `plugin install` writes `version`, `source`,
/// `sha256` and `target` beside `name`/`path`; the host reads `version` and
/// `source` (for `plugin show`/`plugin update`) and ignores the rest, because
/// serde ignores unknown keys unless a struct asks for `deny_unknown_fields`
/// and declaring a field just to discard it would suggest the host validates
/// something it does not. The manifest inside the binary is authoritative,
/// not the lock file's copy of its version.
#[derive(Debug, Clone, Deserialize)]
pub struct LockEntry {
    pub name: String,
    /// Absolute, or relative to the lock file's own directory (`.bddkit/`).
    /// `~` is NOT expanded — a `~/…` path reaches `dlopen` verbatim and fails
    /// with a confusing "no such file", so say so where the author will read it.
    pub path: PathBuf,
    /// The release tag, without its `v`. `None` on a hand-written entry.
    pub version: Option<String>,
    /// `owner/repo` the entry was installed from. Its presence is what makes an
    /// entry "installed by bddkit" to `plugin update` and to `manage::owned`.
    pub source: Option<String>,
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

pub fn read_file(path: &Path) -> Result<Vec<LockEntry>> {
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

/// What `plugin install` writes for one plugin. `path` is stored as given —
/// absolute, or relative to the lock file's directory.
#[derive(Debug, Serialize)]
pub struct Record {
    pub name: String,
    pub path: PathBuf,
    pub version: String,
    pub source: String,
    pub sha256: String,
    pub target: String,
}

/// Replaces the entry named `record.name` in place, or appends it. The file
/// round-trips through `Value`: keys this host does not know survive,
/// comments and formatting do not — a lock file `plugin install` writes is
/// tool-managed, which the authoring guide says.
pub fn upsert(file: &Path, record: &Record) -> Result<()> {
    let entry = serde_yaml_ng::to_value(record)?;
    let mut doc = read_value(file)?;
    let list = plugin_list(&mut doc, file)?;
    match list
        .iter_mut()
        .find(|e| entry_name(e) == Some(&record.name))
    {
        Some(existing) => *existing = entry,
        None => list.push(entry),
    }
    write_value(file, &doc)
}

/// Drops the entry named `name`; `false` when there was none, in which case
/// nothing is written — not even a missing file.
pub fn remove(file: &Path, name: &str) -> Result<bool> {
    let mut doc = read_value(file)?;
    let list = plugin_list(&mut doc, file)?;
    let before = list.len();
    list.retain(|e| entry_name(e) != Some(name));
    if list.len() == before {
        return Ok(false);
    }
    write_value(file, &doc)?;
    Ok(true)
}

fn entry_name(entry: &Value) -> Option<&str> {
    entry.get("name").and_then(Value::as_str)
}

fn read_value(file: &Path) -> Result<Value> {
    match std::fs::read_to_string(file) {
        Ok(raw) => serde_yaml_ng::from_str(&raw)
            .with_context(|| format!("failed to parse {}", file.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Value::Null),
        Err(e) => Err(e).with_context(|| format!("failed to read {}", file.display())),
    }
}

/// The `plugin` list, created when the document (or the key) is absent — an
/// empty file parses as `Null`.
fn plugin_list<'a>(doc: &'a mut Value, file: &Path) -> Result<&'a mut Vec<Value>> {
    if doc.is_null() {
        *doc = Value::Mapping(Default::default());
    }
    let Value::Mapping(map) = doc else {
        bail!("{} is not a mapping", file.display());
    };
    let list = map
        .entry(Value::from("plugin"))
        .or_insert_with(|| Value::Sequence(Vec::new()));
    match list {
        Value::Sequence(list) => Ok(list),
        _ => bail!("`plugin` in {} is not a list", file.display()),
    }
}

fn write_value(file: &Path, doc: &Value) -> Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("failed to create {}", dir.display()))?;
    }
    std::fs::write(file, serde_yaml_ng::to_string(doc)?)
        .with_context(|| format!("failed to write {}", file.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dirs::Candidate;

    fn write(dir: &Path, file: &str, body: &str) -> Candidate {
        std::fs::create_dir_all(dir).expect("mkdir");
        let path = dir.join(file);
        std::fs::write(&path, body).expect("write lock");
        Candidate {
            layer: "test".to_string(),
            path,
        }
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
        let c = write(
            &dir,
            "plugins.yaml",
            "plugin:\n  - name: widget\n    path: /opt/libwidget.so\n",
        );
        let entries = load(&[c]).expect("loads");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "widget");
        assert_eq!(entries[0].path, PathBuf::from("/opt/libwidget.so"));
        assert_eq!(entries[0].layer, "test");
    }

    #[test]
    fn a_lock_file_carrying_p2_provisioning_fields_still_parses() {
        // `sha256`/`target` are written by `plugin install` and ignored on
        // read; `version`/`source` are read.
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
        let mut u = write(
            &user,
            "plugins.yaml",
            "plugin:\n  - name: widget\n    path: /user/libwidget.so\n  - name: mail\n    path: /user/libmail.so\n",
        );
        u.layer = "user".to_string();
        let mut p = write(
            &project,
            "plugins.yaml",
            "plugin:\n  - name: widget\n    path: /project/libwidget.so\n",
        );
        p.layer = "project".to_string();
        let entries = load(&[u, p]).expect("loads");
        let widget = entries
            .iter()
            .find(|e| e.name == "widget")
            .expect("widget present");
        assert_eq!(widget.path, PathBuf::from("/project/libwidget.so"));
        assert_eq!(
            widget.layer, "project",
            "the entry remembers the layer that won"
        );
        let mail = entries
            .iter()
            .find(|e| e.name == "mail")
            .expect("a user entry the project does not override survives");
        assert_eq!(mail.layer, "user");
    }

    #[test]
    fn local_overrides_base_in_the_same_directory() {
        let dir = temp("local");
        let base = write(
            &dir,
            "plugins.yaml",
            "plugin:\n  - name: widget\n    path: vendor/libwidget.so\n",
        );
        let mut local = write(
            &dir,
            "plugins.local.yaml",
            "plugin:\n  - name: widget\n    path: /home/dev/target/debug/libwidget.so\n",
        );
        local.layer = "test.local".to_string();
        let entries = load(&[base, local]).expect("loads");
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].path,
            PathBuf::from("/home/dev/target/debug/libwidget.so")
        );
        assert_eq!(entries[0].layer, "test.local");
    }

    #[test]
    fn a_missing_lock_file_is_not_an_error() {
        // Most runs have no plugins at all; a missing file means "none".
        let dir = temp("absent");
        let c = Candidate {
            layer: "test".to_string(),
            path: dir.join("plugins.yaml"),
        };
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

    fn record(name: &str, version: &str) -> Record {
        Record {
            name: name.to_string(),
            path: PathBuf::from(format!("plugins/bddkit-{name}/{version}/lib{name}.so")),
            version: version.to_string(),
            source: format!("bddkit/bddkit-{name}"),
            sha256: "e3b0c442".to_string(),
            target: "x86_64-unknown-linux-gnu".to_string(),
        }
    }

    #[test]
    fn upsert_creates_a_missing_file_and_reads_back() {
        let dir = temp("upsert-new");
        let file = dir.join("plugins.yaml");
        upsert(&file, &record("exec", "0.1.0")).expect("writes");
        let entries = read_file(&file).expect("reads");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "exec");
        assert_eq!(entries[0].version.as_deref(), Some("0.1.0"));
        assert_eq!(entries[0].source.as_deref(), Some("bddkit/bddkit-exec"));
        assert_eq!(
            entries[0].path,
            dir.join("plugins/bddkit-exec/0.1.0/libexec.so")
        );
    }

    #[test]
    fn upsert_replaces_by_name_and_keeps_everything_else() {
        let dir = temp("upsert-replace");
        let c = write(
            &dir,
            "plugins.yaml",
            "keep_me: 1\nplugin:\n  - name: mail\n    path: /opt/libmail.so\n    custom: x\n  - name: exec\n    path: /old/libexec.so\n",
        );
        upsert(&c.path, &record("exec", "0.2.0")).expect("writes");
        let raw = std::fs::read_to_string(&c.path).expect("read");
        assert!(raw.contains("keep_me: 1"), "{raw}");
        assert!(
            raw.contains("custom: x"),
            "an unknown key of another entry survives:\n{raw}"
        );
        let entries = read_file(&c.path).expect("reads");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "mail", "order is kept");
        assert_eq!(entries[1].version.as_deref(), Some("0.2.0"));
    }

    #[test]
    fn remove_drops_the_named_entry_only() {
        let dir = temp("remove");
        let c = write(
            &dir,
            "plugins.yaml",
            "plugin:\n  - name: mail\n    path: /opt/libmail.so\n  - name: exec\n    path: /opt/libexec.so\n",
        );
        assert!(remove(&c.path, "exec").expect("writes"));
        let entries = read_file(&c.path).expect("reads");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "mail");
        assert!(!remove(&c.path, "exec").expect("no-op"), "absent now");
    }

    #[test]
    fn remove_from_a_missing_file_creates_nothing() {
        let dir = temp("remove-missing");
        let file = dir.join("plugins.yaml");
        assert!(!remove(&file, "exec").expect("no-op"));
        assert!(!file.exists());
    }

    #[test]
    fn an_empty_file_is_an_empty_lock() {
        let dir = temp("empty-file");
        let c = write(&dir, "plugins.yaml", "");
        upsert(&c.path, &record("exec", "0.1.0")).expect("writes");
        assert_eq!(read_file(&c.path).expect("reads").len(), 1);
    }

    #[test]
    fn a_relative_path_resolves_against_the_lock_file_directory() {
        // A committed project lock referring to ./vendor/libwidget.so must work
        // regardless of the working directory the run was started from.
        let dir = temp("relative");
        let c = write(
            &dir,
            "plugins.yaml",
            "plugin:\n  - name: widget\n    path: vendor/libwidget.so\n",
        );
        let entries = load(&[c]).expect("loads");
        assert_eq!(entries[0].path, dir.join("vendor/libwidget.so"));
    }
}
