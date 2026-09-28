//! `bddkit plugin …`: where an install goes and the commands themselves.
//! Fetching and unpacking is `install.rs`; the lock file format is `lock.rs`.

use crate::dirs::{self, Env, Layer, Os};
use crate::plugin::install::{self, Sources};
use crate::plugin::library::Library;
use crate::plugin::lock;
use anyhow::{Context as _, Result, bail};
use std::path::{Path, PathBuf};

/// One lock file an install can write, and where its libraries go.
struct Target {
    /// The candidate label `doctor` prints: `user`, `project.local`, …
    label: String,
    lock: PathBuf,
    /// The layer's data directory; libraries go under `<data>/plugins/`.
    data: PathBuf,
    /// Project and override lock files record a path relative to themselves,
    /// so the directory stays relocatable; shared and user record it absolute.
    relative: bool,
}

pub struct Context {
    os: Os,
    env: Env,
    layers: Vec<Layer>,
    config_dir: PathBuf,
    client: reqwest::Client,
    sources: Sources,
}

impl Context {
    pub fn new(env: Env, config_dir: &Path) -> Result<Self> {
        let os = Os::current();
        let layers = dirs::layers(os, &env, config_dir)?;
        Ok(Self {
            os,
            env,
            layers,
            config_dir: config_dir.to_path_buf(),
            client: reqwest::Client::new(),
            sources: Sources::from_env(),
        })
    }

    fn target(&self, layer: &Layer, local: bool) -> Target {
        let (label, file) = if local {
            (format!("{}.local", layer.name), "plugins.local.yaml")
        } else {
            (layer.name.to_string(), "plugins.yaml")
        };
        Target {
            label,
            lock: layer.dir.join(file),
            data: dirs::data_dir(self.os, &self.env, layer),
            relative: matches!(layer.name, "project" | "override"),
        }
    }

    /// `--layer` names one of the six candidate files; `--bddkit-dir` /
    /// `BDDKIT_DIR` replaces the whole chain with one directory instead, so
    /// the two are mutually exclusive on every subcommand that takes a
    /// `--layer` (`install`, `update`, `remove`) — not only `install`, which
    /// is where this check used to live alone.
    fn check_layer_flag(&self, layer: Option<&str>) -> Result<()> {
        if layer.is_some() && self.layers.iter().any(|l| l.name == "override") {
            bail!(
                "--layer cannot be combined with --bddkit-dir / BDDKIT_DIR, which replaces every layer"
            );
        }
        Ok(())
    }

    fn install_target(&self, layer: Option<&str>) -> Result<Target> {
        self.check_layer_flag(layer)?;
        let Some(label) = layer else {
            let layer = dirs::install_layer(&self.layers).context(
                "no user directory to install into (HOME / LOCALAPPDATA unset): pass --layer or --bddkit-dir",
            )?;
            return Ok(self.target(layer, false));
        };
        let (name, local) = match label.strip_suffix(".local") {
            Some(name) => (name, true),
            None => (label, false),
        };
        let layer = match self.layers.iter().find(|l| l.name == name) {
            Some(layer) => layer.clone(),
            // A suite with no `.bddkit/` yet: `--layer project` is how it gets one.
            None if name == "project" => Layer {
                name: "project",
                dir: self.config_dir.join(".bddkit"),
            },
            None => bail!("the {name} layer has no directory on this machine"),
        };
        Ok(self.target(&layer, local))
    }

    /// Download, verify, unpack, load, and only then write the lock entry —
    /// nothing is registered that `run` would refuse. Returns the plugin's
    /// name, which comes from its manifest and is never guessed.
    ///
    /// `repo` and `version` are validated here, at the one place all three of
    /// their origins converge — the CLI argument (`install`), an index
    /// entry's `repo` (`install` by name), and a lock entry's `source`
    /// (`update`) — before either becomes a directory name under
    /// `<data>/plugins/<repo>/<version>/`, the pre-clean below deletes without
    /// an `owned()` guard.
    async fn install_into(&self, target: &Target, repo: &str, version: &str) -> Result<String> {
        install::check_repo(repo)?;
        install::check_segment("version", version)?;
        let host = install::host_target()?;
        let download =
            install::download(&self.client, &self.sources.github, repo, version, host).await?;
        let dir = target
            .data
            .join("plugins")
            .join(install::repo_basename(repo))
            .join(version);
        // A reinstall of the same version starts clean, not beside a stale file.
        let _ = std::fs::remove_dir_all(&dir);
        let lib_path = install::extract_library(&download.bytes, &download.asset, &dir)?;
        let name = match Library::load(repo, &lib_path) {
            Ok(lib) => {
                let name = lib.manifest.name.clone();
                // Never unloaded, for the reason `load_plugins` gives: a
                // plugin's atexit handler must not run against an unmapped page.
                std::mem::forget(lib);
                name
            }
            Err(error) => {
                // ponytail: Windows cannot delete a DLL it has mapped, so a
                // rejected plugin may leave its directory there; nothing
                // points at it. Sweep it on the next install if it matters.
                let _ = std::fs::remove_dir_all(&dir);
                return Err(error);
            }
        };

        let previous = lock::read_file(&target.lock)?
            .into_iter()
            .find(|entry| entry.name == name);
        let lock_dir = target.lock.parent().unwrap_or(Path::new("."));
        let path = if target.relative {
            lib_path
                .strip_prefix(lock_dir)
                .unwrap_or(&lib_path)
                .to_path_buf()
        } else {
            std::path::absolute(&lib_path)?
        };
        lock::upsert(
            &target.lock,
            &lock::Record {
                name: name.clone(),
                path,
                version: version.to_string(),
                source: repo.to_string(),
                sha256: download.sha256,
                target: host.to_string(),
            },
        )?;
        if let Some(previous) = previous
            && previous.path.parent() != Some(dir.as_path())
            && owned(target, &previous)
            && let Err(error) = remove_version_dir(&previous.path)
        {
            eprintln!(
                "warning: could not remove {}: {error}",
                previous.path.display()
            );
        }
        Ok(name)
    }

    /// The label of a higher-precedence file that declares `name` too, which
    /// makes an install into `label` invisible to `run`.
    fn shadowed_by(&self, name: &str, label: &str) -> Result<Option<String>> {
        let winner = lock::load(&dirs::candidates(&self.layers, "plugins"))?
            .into_iter()
            .find(|entry| entry.name == name);
        Ok(winner
            .map(|entry| entry.layer)
            .filter(|layer| layer != label))
    }

    pub async fn install(&self, arg: &str, layer: Option<&str>) -> Result<i32> {
        let spec = install::Spec::parse(arg)?;
        let repo = if spec.is_repo() {
            spec.target.clone()
        } else {
            install::fetch_index(&self.client, &self.sources)
                .await?
                .into_iter()
                .find(|entry| entry.name == spec.target)
                .map(|entry| entry.repo)
                .with_context(|| {
                    format!(
                        "no plugin {:?} in the index: `bddkit plugin list` shows what is there, and owner/repo installs one that is not",
                        spec.target
                    )
                })?
        };
        let version = match spec.version {
            Some(version) => version,
            None => install::latest_version(&self.client, &self.sources.github, &repo).await?,
        };
        let target = self.install_target(layer)?;
        let name = self.install_into(&target, &repo, &version).await?;
        println!(
            "installed {name} {version} into {} ({})",
            target.label,
            target.lock.display()
        );
        if let Some(winner) = self.shadowed_by(&name, &target.label)? {
            println!(
                "warning: {winner} also declares {name:?} and takes precedence, so this install is not the one that runs"
            );
        }
        Ok(0)
    }

    /// Every lock file of the chain, lowest precedence first — the order and
    /// labels of `dirs::candidates` — narrowed to `layer` when given.
    fn targets(&self, layer: Option<&str>) -> Vec<Target> {
        self.layers
            .iter()
            .flat_map(|layer| [self.target(layer, false), self.target(layer, true)])
            .filter(|target| layer.is_none_or(|label| label == target.label))
            .collect()
    }

    /// Entries of `name` per lock file, narrowed to `layer` when given.
    fn holders(&self, name: &str, layer: Option<&str>) -> Result<Vec<(Target, lock::LockEntry)>> {
        let mut out = Vec::new();
        for target in self.targets(layer) {
            if let Some(entry) = lock::read_file(&target.lock)?
                .into_iter()
                .find(|e| e.name == name)
            {
                out.push((target, entry));
            }
        }
        Ok(out)
    }

    pub async fn update(
        &self,
        names: &[String],
        layer: Option<&str>,
        dry_run: bool,
    ) -> Result<i32> {
        self.check_layer_flag(layer)?;
        let mut failed = false;
        let mut matched: std::collections::HashSet<String> = std::collections::HashSet::new();
        for target in self.targets(layer) {
            for entry in lock::read_file(&target.lock)? {
                if !names.is_empty() && !names.contains(&entry.name) {
                    continue;
                }
                matched.insert(entry.name.clone());
                let row = format!("{} ({})", entry.name, target.label);
                let (Some(source), Some(current)) = (&entry.source, &entry.version) else {
                    println!("{row}: skipped: not installed by bddkit");
                    continue;
                };
                let outcome = async {
                    let latest =
                        install::latest_version(&self.client, &self.sources.github, source).await?;
                    if &latest == current {
                        return Ok(format!("up to date ({current})"));
                    }
                    if dry_run {
                        return Ok(format!("{current} → {latest}"));
                    }
                    self.install_into(&target, source, &latest).await?;
                    Ok::<_, anyhow::Error>(format!("updated {current} → {latest}"))
                }
                .await;
                match outcome {
                    Ok(message) => println!("{row}: {message}"),
                    Err(error) => {
                        failed = true;
                        println!("{row}: failed: {error:#}");
                    }
                }
            }
        }
        for name in names {
            if !matched.contains(name) {
                println!("{name}: failed: not installed in any lock file");
                failed = true;
            }
        }
        Ok(i32::from(failed))
    }

    pub async fn remove(&self, name: &str, layer: Option<&str>) -> Result<i32> {
        self.check_layer_flag(layer)?;
        let mut holders = self.holders(name, layer)?;
        let (target, entry) = match holders.len() {
            0 => bail!("{name:?} is not installed in any lock file of the chain"),
            1 => holders.remove(0),
            _ => bail!(
                "{name:?} is declared in {}: pass --layer to pick one",
                holders
                    .iter()
                    .map(|(t, _)| t.label.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        };
        lock::remove(&target.lock, name)?;
        if owned(&target, &entry) {
            remove_version_dir(&entry.path).with_context(|| {
                format!(
                    "unregistered, but could not delete {}",
                    entry.path.display()
                )
            })?;
        }
        println!(
            "removed {name} from {} ({})",
            target.label,
            target.lock.display()
        );
        Ok(0)
    }

    pub async fn show(&self, name: Option<&str>) -> Result<i32> {
        let effective = lock::load(&dirs::candidates(&self.layers, "plugins"))?;
        let Some(name) = name else {
            if effective.is_empty() {
                println!("no plugin installed");
            }
            for entry in &effective {
                println!(
                    "{:<12} {:<10} {:<14} {:<28} {}",
                    entry.name,
                    entry.version.as_deref().unwrap_or("-"),
                    entry.layer,
                    entry.source.as_deref().unwrap_or("-"),
                    entry.path.display()
                );
            }
            return Ok(0);
        };
        let entry = effective
            .iter()
            .find(|entry| entry.name == name)
            .with_context(|| format!("{name:?} is not installed"))?;
        // Wrapped the instant it is loaded, not `mem::forget`ten at the end:
        // the loop below runs `self.holders(...)?` and `println!`, either of
        // which can return early (a `?`) or panic (an EPIPE on a closed
        // stdout unwinds through here) before a trailing `forget` is ever
        // reached — and an unwind still runs `Drop` on a plain value, against
        // the "libraries are never unloaded" invariant. `ManuallyDrop` makes
        // that true unconditionally, because there is no `Drop` impl left to
        // skip.
        let lib = std::mem::ManuallyDrop::new(Library::load(name, &entry.path)?);
        println!(
            "{name} {} ({})",
            entry.version.as_deref().unwrap_or("-"),
            entry.layer
        );
        println!("  source:      {}", entry.source.as_deref().unwrap_or("-"));
        println!("  path:        {}", entry.path.display());
        println!("  manifest:    {}", lib.manifest.version);
        println!("  groups:      {}", lib.manifest.groups.join(", "));
        let concurrency = match lib.manifest.concurrency {
            crate::plugin::abi::Concurrency::Shared => "shared",
            crate::plugin::abi::Concurrency::PerWorker => "per_worker",
        };
        println!("  concurrency: {concurrency}");
        println!("  steps:       {}", lib.steps.len());
        for (target, shadowed) in self.holders(name, None)? {
            if target.label != entry.layer {
                println!(
                    "  shadowed:    {} ({})",
                    target.label,
                    shadowed.path.display()
                );
            }
        }
        Ok(0)
    }

    pub async fn list(&self, query: Option<&str>) -> Result<i32> {
        let index = install::fetch_index(&self.client, &self.sources).await?;
        let installed = lock::load(&dirs::candidates(&self.layers, "plugins"))?;
        let found = install::search(&index, query);
        if found.is_empty() {
            println!(
                "no plugin in the index matches {:?}",
                query.unwrap_or_default()
            );
        }
        for entry in found {
            let mark = installed
                .iter()
                .find(|e| e.name == entry.name)
                .map(|e| {
                    format!(
                        "  installed {} ({})",
                        e.version.as_deref().unwrap_or("?"),
                        e.layer
                    )
                })
                .unwrap_or_default();
            println!(
                "{:<12} {:<28} {}{mark}",
                entry.name, entry.repo, entry.description
            );
        }
        Ok(0)
    }
}

/// Whether `entry` is a library `plugin install` itself wrote into this
/// target's data directory — the only files `remove`/`update` ever delete.
/// True only when BOTH hold: `entry.source` is set (a hand-written entry
/// never carries one, so it is never "installed by bddkit") AND the path is
/// shaped exactly `<data>/plugins/<repo>/<version>/<lib>` — three `Normal`
/// path components under `<data>/plugins`, no more, no fewer, and none of
/// them `.`/`..`. That shape is what `remove_version_dir` assumes when it
/// deletes the version directory two levels up from the library; a
/// shallower hand-edit (`plugins/lib.so`) or one carrying `..` must fail this
/// check rather than let a delete reach outside what this install wrote.
fn owned(target: &Target, entry: &lock::LockEntry) -> bool {
    if entry.source.is_none() {
        return false;
    }
    let Ok(rest) = entry.path.strip_prefix(target.data.join("plugins")) else {
        return false;
    };
    let components: Vec<_> = rest.components().collect();
    components.len() == 3
        && components
            .iter()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}

/// Deletes `<…>/plugins/<repo>/<version>/` and then `<repo>/` if that left it
/// empty (`remove_dir` refuses a non-empty directory, which is the point).
fn remove_version_dir(lib: &Path) -> std::io::Result<()> {
    let Some(version_dir) = lib.parent() else {
        return Ok(());
    };
    std::fs::remove_dir_all(version_dir)?;
    if let Some(repo_dir) = version_dir.parent() {
        let _ = std::fs::remove_dir(repo_dir);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(data: &Path) -> Target {
        Target {
            label: "user".to_string(),
            lock: data.join("plugins.yaml"),
            data: data.to_path_buf(),
            relative: false,
        }
    }

    fn entry(path: PathBuf, source: Option<&str>) -> lock::LockEntry {
        lock::LockEntry {
            name: "echo".to_string(),
            path,
            version: Some("0.1.0".to_string()),
            source: source.map(str::to_string),
            layer: "user".to_string(),
        }
    }

    #[test]
    fn owned_is_true_only_for_a_proper_installed_entry() {
        let data = PathBuf::from("/data");
        let t = target(&data);

        let lib_name = format!(
            "{}echo_plugin{}",
            std::env::consts::DLL_PREFIX,
            std::env::consts::DLL_SUFFIX
        );

        // The shape `plugin install` itself writes: source set, three
        // Normal components under `<data>/plugins`.
        let proper = entry(
            data.join("plugins/echo-plugin/0.1.0").join(&lib_name),
            Some("bddkit/echo-plugin"),
        );
        assert!(owned(&t, &proper), "a proper installed entry is owned");

        // No `source`: a hand-written entry, never bddkit's to delete, even
        // if its path happens to have the right shape.
        let hand_written = entry(data.join("plugins/echo-plugin/0.1.0").join(&lib_name), None);
        assert!(
            !owned(&t, &hand_written),
            "an entry with no source is never owned"
        );

        // Too shallow: `plugins/<lib>` — one component, not the
        // `<repo>/<version>/<lib>` triple `remove_version_dir` assumes.
        let shallow = entry(data.join("plugins").join(&lib_name), Some("bddkit/echo"));
        assert!(!owned(&t, &shallow), "a shallow path is not owned");

        // A `..` component in the tail must never be owned, even with a
        // `source` set and exactly three components after `plugins/`.
        let traversal = entry(
            data.join("plugins/../0.1.0").join(&lib_name),
            Some("bddkit/echo-plugin"),
        );
        assert!(!owned(&t, &traversal), "a path carrying .. is never owned");

        // Outside `<data>/plugins` entirely.
        let outside = entry(
            PathBuf::from("/opt/other/lib.so"),
            Some("bddkit/echo-plugin"),
        );
        assert!(!owned(&t, &outside), "a path outside plugins/ is not owned");
    }
}
