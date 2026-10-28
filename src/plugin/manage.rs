//! `bddkit plugin …`: where an install goes and the commands themselves.
//! Fetching and unpacking is `install.rs`; the lock file format is `lock.rs`.

use crate::dirs::{self, Env, Layer, Os};
use crate::plugin::abi::ABI_VERSION;
use crate::plugin::install::{self, Sources};
use crate::plugin::library::Library;
use crate::plugin::lock;
use crate::telemetry::payload::{Acted, PluginFrom, PluginInfo, PluginResult};
use anyhow::{Context as _, Result, bail};
use std::path::{Path, PathBuf};

/// One lock file an install can write, and where its libraries go.
#[derive(Clone)]
pub(crate) struct Target {
    /// The candidate label `doctor` prints: `user`, `project.local`, …
    pub(crate) label: String,
    pub(crate) lock: PathBuf,
    /// The layer's data directory; libraries go under `<data>/plugins/`.
    pub(crate) data: PathBuf,
    /// Project and override lock files record a path relative to themselves,
    /// so the directory stays relocatable; shared and user record it absolute.
    relative: bool,
}

/// An unpacked release that nothing has loaded or registered yet.
#[derive(Clone)]
pub(crate) struct Fetched {
    pub(crate) lib_path: PathBuf,
    sha256: String,
    host: &'static str,
    /// The copy of this very version that was there before, moved out of the
    /// way so a failed install can put it back (`settle`).
    aside: Option<PathBuf>,
}

impl Fetched {
    /// The new release is registered (or wanted): the old copy goes.
    pub(crate) fn commit(&self) {
        if let Some(aside) = &self.aside {
            let _ = std::fs::remove_dir_all(aside);
        }
    }

    /// The new release is refused: it goes, and the old copy comes back.
    fn rollback(&self) {
        if let Some(dir) = self.lib_path.parent() {
            restore(dir, self.aside.as_deref());
        }
    }
}

/// Drops whatever is in `dir` and, if there was a copy moved aside, puts it back.
/// Windows may refuse both while the new DLL is mapped; the person is told
/// where the old copy is, because nothing else will.
fn restore(dir: &Path, aside: Option<&Path>) {
    let _ = std::fs::remove_dir_all(dir);
    if let Some(aside) = aside
        && let Err(error) = std::fs::rename(aside, dir)
    {
        eprintln!(
            "warning: could not restore {}: {error}; the previous copy is at {}",
            dir.display(),
            aside.display()
        );
    }
}

pub struct Context {
    /// What telemetry may say about the plugins the last command acted on.
    acted: std::sync::Mutex<Vec<PluginInfo>>,
    pub(crate) os: Os,
    pub(crate) env: Env,
    layers: Vec<Layer>,
    config_dir: PathBuf,
    pub(crate) client: reqwest::Client,
    pub(crate) sources: Sources,
}

/// No total timeout: a large library on a slow link is not a hang. A silent
/// server is, and so is an https base that redirects to plain http.
fn http_client(sources: &Sources) -> Result<reqwest::Client> {
    let https = sources.registry.starts_with("https://") && sources.github.starts_with("https://");
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .read_timeout(std::time::Duration::from_secs(30))
        .https_only(https)
        .build()
        .context("failed to build the HTTP client")
}

impl Context {
    pub fn new(env: Env, config_dir: &Path) -> Result<Self> {
        let os = Os::current();
        let layers = dirs::layers(os, &env, config_dir)?;
        let sources = Sources::from_env();
        Ok(Self {
            acted: Default::default(),
            os,
            env,
            layers,
            config_dir: config_dir.to_path_buf(),
            client: http_client(&sources)?,
            sources,
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

    /// Download, verify and unpack a release into
    /// `<data>/plugins/<repo>/<version>/`, and stop there: not loaded, not in
    /// any lock file. `plugin install` goes on to load and register it;
    /// `bddkit upgrade` hands it to the staged binary instead, because this
    /// process may speak another ABI than the plugin does.
    ///
    /// `repo` and `version` are validated here, at the one place all three of
    /// their origins converge — the CLI argument (`install`), an index
    /// entry's `repo` (`install` by name), and a lock entry's `source`
    /// (`update`) — before either becomes a directory name under
    /// `<data>/plugins/<repo>/<version>/`, the pre-clean below deletes without
    /// an `owned()` guard.
    pub(crate) async fn fetch_into(
        &self,
        target: &Target,
        repo: &str,
        version: &str,
    ) -> Result<Fetched> {
        install::check_repo(repo)?;
        install::check_segment("version", version)?;
        let host = install::host_target()?;
        let download =
            install::download(&self.client, &self.sources.github, repo, version, host).await?;
        let dir = version_dir(target, repo, version);
        // A reinstall of the same version starts clean, not beside a stale
        // file — but the working copy is moved aside, not deleted, until the
        // new one has loaded. It cannot be extracted elsewhere and renamed in:
        // Windows will not rename a directory holding a mapped DLL.
        let aside = dir.exists().then(|| {
            let aside = dir.with_file_name(format!("{version}.replaced-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&aside);
            aside
        });
        if let Some(aside) = &aside {
            std::fs::rename(&dir, aside)
                .with_context(|| format!("failed to move {} aside", dir.display()))?;
        }
        match install::extract_library(&download.bytes, &download.asset, &dir) {
            Ok(lib_path) => Ok(Fetched {
                lib_path,
                sha256: download.sha256,
                host,
                aside,
            }),
            Err(error) => {
                restore(&dir, aside.as_deref());
                Err(error)
            }
        }
    }

    /// The lock entry for a fetched release of the plugin the manifest calls
    /// `name`. A path is stored as the target's file wants it.
    pub(crate) fn record(
        target: &Target,
        name: &str,
        repo: &str,
        version: &str,
        fetched: &Fetched,
    ) -> Result<lock::Record> {
        let lock_dir = target.lock.parent().unwrap_or(Path::new("."));
        let path = if target.relative {
            fetched
                .lib_path
                .strip_prefix(lock_dir)
                .unwrap_or(&fetched.lib_path)
                .to_path_buf()
        } else {
            std::path::absolute(&fetched.lib_path)?
        };
        Ok(lock::Record {
            name: name.to_string(),
            path,
            version: version.to_string(),
            source: repo.to_string(),
            sha256: fetched.sha256.clone(),
            target: fetched.host.to_string(),
        })
    }

    /// Fetch, load, and only then write the lock entry — nothing is
    /// registered that `run` would refuse. Returns the plugin's name, which
    /// comes from its manifest and is never guessed.
    async fn install_into(&self, target: &Target, repo: &str, version: &str) -> Result<String> {
        let fetched = self.fetch_into(target, repo, version).await?;
        match self.register(target, repo, version, &fetched) {
            Ok((name, previous)) => {
                fetched.commit();
                if let Some(previous) = previous
                    && owned(target, &previous)
                    && let Err(error) = self.remove_unreferenced_version(&previous.path)
                {
                    eprintln!(
                        "warning: could not remove {}: {error:#}",
                        previous.path.display()
                    );
                }
                Ok(name)
            }
            // Nothing points at the new copy, and the one it replaced is back.
            Err(error) => {
                fetched.rollback();
                Err(error)
            }
        }
    }

    /// Load the fetched library and only then write the lock entry — nothing
    /// is registered that `run` would refuse. The plugin's name comes from its
    /// manifest and is never guessed; the entry it replaced is returned.
    fn register(
        &self,
        target: &Target,
        repo: &str,
        version: &str,
        fetched: &Fetched,
    ) -> Result<(String, Option<lock::LockEntry>)> {
        // A path already loaded in this process (`update` over two layers that
        // share a release) may resolve to the old mapping, not the new file:
        // dlopen matches by name first. No regression — the old code loaded
        // from the same path — and a separate `install` is unaffected.
        let lib = Library::load(repo, &fetched.lib_path)
            .map_err(|error| install::classed(error, PluginResult::Refused))?;
        let name = lib.manifest.name.clone();
        // Never unloaded, for the reason `load_plugins` gives: a plugin's
        // atexit handler must not run against an unmapped page.
        std::mem::forget(lib);
        let previous = lock::read_file(&target.lock)?
            .into_iter()
            .find(|entry| entry.name == name);
        lock::upsert(
            &target.lock,
            &Self::record(target, &name, repo, version, fetched)?,
        )?;
        Ok((name, previous))
    }

    /// The label of a higher-precedence file that declares `name` too, which
    /// makes an install into `label` invisible to `run`.
    fn shadowed_by(&self, name: &str, label: &str) -> Result<Option<String>> {
        // A project layer `install_target` made up for a suite with no
        // `.bddkit/` yet is not in the chain, and it would be the top of it.
        let layer = label.strip_suffix(".local").unwrap_or(label);
        if !self.layers.iter().any(|l| l.name == layer) {
            return Ok(None);
        }
        let winner = lock::load(&dirs::candidates(&self.layers, "plugins"))?
            .into_iter()
            .find(|entry| entry.name == name);
        Ok(winner
            .map(|entry| entry.layer)
            .filter(|layer| layer != label))
    }

    /// What the last `install`, `update` or `remove` acted on, for telemetry.
    pub fn take_acted(&self) -> Vec<PluginInfo> {
        std::mem::take(&mut *self.acted.lock().expect("acted is never poisoned"))
    }

    fn note<T>(&self, acted: &Acted, outcome: &Result<T>) {
        let info = PluginInfo::of(acted, install::result_of(outcome));
        self.acted
            .lock()
            .expect("acted is never poisoned")
            .push(info);
    }

    pub async fn install(&self, arg: &str, layer: Option<&str>) -> Result<i32> {
        let mut acted = Acted::default();
        let outcome = self.install_one(arg, layer, &mut acted).await;
        self.note(&acted, &outcome);
        outcome
    }

    async fn install_one(&self, arg: &str, layer: Option<&str>, acted: &mut Acted) -> Result<i32> {
        let spec = install::Spec::parse(arg)?;
        // Everything that can be refused without the network is refused first.
        let target = self.install_target(layer)?;
        lock::check_writable(&target.lock)?;
        acted.layer = Some(target.label.clone());
        let indexed = if spec.is_repo() {
            acted.from = Some(PluginFrom::Repository);
            acted.repo = Some(spec.target.clone());
            None
        } else {
            acted.from = Some(PluginFrom::Registry);
            let entry = install::fetch_index(&self.client, &self.sources)
                .await?
                .into_iter()
                .find(|entry| entry.name == spec.target)
                .with_context(|| {
                    format!(
                        "no plugin {:?} in the index: `bddkit plugin list` shows what is there, and owner/repo installs one that is not",
                        spec.target
                    )
                })
                .map_err(|error| install::classed(error, PluginResult::NotFound))?;
            Some(entry)
        };
        let repo = indexed
            .as_ref()
            .map_or_else(|| spec.target.clone(), |entry| entry.repo.clone());
        acted.repo = Some(repo.clone());
        let pinned = spec.version.is_some();
        let version = match spec.version {
            Some(version) => {
                let github = &self.sources.github;
                match install::release_abi(&self.client, github, &repo, &version).await? {
                    Some(abi) if abi != ABI_VERSION => {
                        return Err(install::classed(
                            anyhow::anyhow!(
                                "{repo} {version} targets plugin ABI {abi}, this bddkit speaks {ABI_VERSION}"
                            ),
                            PluginResult::NoBuild,
                        ));
                    }
                    _ => version,
                }
            }
            None => install::host_release(
                &self.client,
                &self.sources.github,
                &repo,
                indexed.as_ref(),
                ABI_VERSION,
            )
            .await?
            .map_err(|why| install::classed(anyhow::Error::msg(why), PluginResult::NoBuild))?,
        };
        // A version the person typed is only text until it has been installed.
        if !pinned {
            acted.version = Some(version.clone());
        }
        let name = self.install_into(&target, &repo, &version).await?;
        acted.name = Some(name.clone());
        acted.version = Some(version.clone());
        println!(
            "installed {name} {version} into {} ({})",
            target.label,
            target.lock.display()
        );
        // The install is done: a lock file that cannot be re-read now is a
        // warning, not a failure of what was already printed.
        match self.shadowed_by(&name, &target.label) {
            Ok(Some(winner)) => eprintln!(
                "warning: {winner} also declares {name:?} and takes precedence, so this install is not the one that runs"
            ),
            Ok(None) => {}
            Err(error) => eprintln!("warning: could not check for a shadowing entry: {error:#}"),
        }
        Ok(0)
    }

    /// Every lock file of the chain, lowest precedence first — the order and
    /// labels of `dirs::candidates` — narrowed to `layer` when given.
    pub(crate) fn targets(&self, layer: Option<&str>) -> Vec<Target> {
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
        // A dead index only costs the shortcut: every entry then falls back to
        // asking its own release, which is the same answer one request slower.
        let index = install::fetch_index(&self.client, &self.sources)
            .await
            .unwrap_or_default();
        let mut matched: std::collections::HashSet<String> = std::collections::HashSet::new();
        for target in self.targets(layer) {
            for entry in lock::read_file(&target.lock)? {
                if !names.is_empty() && !names.contains(&entry.name) {
                    continue;
                }
                matched.insert(entry.name.clone());
                let row = format!("{} ({})", entry.name, target.label);
                let Some(source) = &entry.source else {
                    println!("{row}: skipped: not installed by bddkit");
                    continue;
                };
                let Some(current) = &entry.version else {
                    println!(
                        "{row}: skipped: no version recorded, reinstall it with `plugin install`"
                    );
                    continue;
                };
                let mut acted = Acted {
                    repo: Some(source.clone()),
                    name: Some(entry.name.clone()),
                    version: Some(current.clone()),
                    from: None,
                    layer: Some(target.label.clone()),
                };
                let outcome = async {
                    let indexed = index.iter().find(|e| &e.repo == source);
                    acted.from = Some(if indexed.is_some() {
                        PluginFrom::Registry
                    } else {
                        PluginFrom::Repository
                    });
                    let latest = match install::host_release(
                        &self.client,
                        &self.sources.github,
                        source,
                        indexed,
                        ABI_VERSION,
                    )
                    .await?
                    {
                        Ok(latest) => latest,
                        Err(why) => {
                            return Ok(format!(
                                "up to date for ABI {ABI_VERSION} ({current}): {why}"
                            ));
                        }
                    };
                    if &latest == current {
                        return Ok(format!("up to date ({current})"));
                    }
                    if dry_run {
                        return Ok(format!("{current} → {latest}"));
                    }
                    acted.version = Some(latest.clone());
                    self.install_into(&target, source, &latest).await?;
                    Ok::<_, anyhow::Error>(format!("updated {current} → {latest}"))
                }
                .await;
                if !dry_run {
                    self.note(&acted, &outcome);
                }
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
                // Not a plugin of ours at all, as far as anyone can tell.
                self.note(
                    &Acted::default(),
                    &Err::<(), _>(install::classed(
                        anyhow::anyhow!("not installed"),
                        PluginResult::NotFound,
                    )),
                );
            }
        }
        Ok(i32::from(failed))
    }

    pub async fn remove(&self, name: &str, layer: Option<&str>) -> Result<i32> {
        let mut acted = Acted::default();
        let outcome = self.remove_one(name, layer, &mut acted);
        self.note(&acted, &outcome);
        outcome
    }

    fn remove_one(&self, name: &str, layer: Option<&str>, acted: &mut Acted) -> Result<i32> {
        self.check_layer_flag(layer)?;
        let mut holders = self.holders(name, layer)?;
        let (target, entry) = match holders.len() {
            0 => {
                return Err(install::classed(
                    anyhow::anyhow!("{name:?} is not installed in any lock file of the chain"),
                    PluginResult::NotFound,
                ));
            }
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
        acted.repo.clone_from(&entry.source);
        acted.name = Some(entry.name.clone());
        acted.version.clone_from(&entry.version);
        acted.layer = Some(target.label.clone());
        lock::remove(&target.lock, name)?;
        if owned(&target, &entry) {
            self.remove_unreferenced_version(&entry.path)
                .with_context(|| {
                    format!(
                        "unregistered, but could not finish removing {}",
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

    /// Every library any lock file of the chain points at, and any of their
    /// `.before-upgrade` backups, which is what `upgrade --rollback` restores.
    /// One definition of «still in use» for `prune`, `remove` and `update`.
    fn referenced_paths(&self) -> Result<Vec<PathBuf>> {
        let mut used = Vec::new();
        for target in self.targets(None) {
            let backup = crate::upgrade::sibling_of_backup(&target.lock);
            for file in [&target.lock, &backup] {
                used.extend(lock::read_file(file)?.iter().map(|e| real(&e.path)));
            }
        }
        Ok(used)
    }

    /// `remove_version_dir`, unless some lock file still points into it. Called
    /// AFTER the entry that named it is gone from its own file: `user` and
    /// `user.local` share one data directory, so another file may well be
    /// running the same release. That order is load-bearing: before the write,
    /// the file being changed would still name the directory it is about to
    /// lose, and a same-version reinstall would delete its own new copy.
    fn remove_unreferenced_version(&self, lib: &Path) -> Result<()> {
        if let Some(dir) = lib.parent()
            && is_referenced(&self.referenced_paths()?, dir)
        {
            return Ok(());
        }
        Ok(remove_version_dir(lib)?)
    }

    /// Version directories under the data directory of every layer `plugin
    /// install` may write (not project) that nothing references. A reference
    /// is a path inside it in any lock file of the chain or in that file's
    /// `.before-upgrade` backup, which is what `upgrade --rollback` restores.
    /// Only the shape `install` makes qualifies: a real directory holding
    /// exactly one regular file, a dynamic library. A symlink, a dev build
    /// with more files, anything else is not bddkit's to delete.
    pub(crate) fn unreferenced(&self) -> Result<Vec<PathBuf>> {
        let used = self.referenced_paths()?;
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for target in self.targets(None) {
            if target.label.starts_with("project") || !seen.insert(target.data.clone()) {
                continue;
            }
            for repo in subdirs(&target.data.join("plugins")) {
                for dir in subdirs(&repo) {
                    if is_install_shape(&dir) && !is_referenced(&used, &dir) {
                        out.push(dir);
                    }
                }
            }
        }
        out.sort();
        Ok(out)
    }

    pub fn prune(&self, dry_run: bool, yes: bool) -> Result<i32> {
        use std::io::IsTerminal;
        if !dry_run && !yes && !std::io::stdin().is_terminal() {
            bail!(
                "there is no terminal to confirm on: pass --yes, or --dry-run to see the list only"
            );
        }
        let stale = self.unreferenced()?;
        if stale.is_empty() {
            println!("nothing to prune");
            return Ok(0);
        }
        let sizes: Vec<u64> = stale.iter().map(|d| size(d)).collect();
        for (dir, bytes) in stale.iter().zip(&sizes) {
            println!("{} ({})", dir.display(), human(*bytes));
        }
        println!("{} version(s), {}", stale.len(), human(sizes.iter().sum()));
        if dry_run {
            return Ok(0);
        }
        if !yes && !crate::upgrade::confirm("remove them?")? {
            println!("nothing removed");
            return Ok(0);
        }
        // One directory the user cannot write (the shared layer) must not
        // keep the others from going: report it and carry on.
        let mut failed = 0;
        for dir in &stale {
            if let Err(error) = std::fs::remove_dir_all(dir) {
                eprintln!("error: could not delete {}: {error}", dir.display());
                failed += 1;
            } else if let Some(repo) = dir.parent() {
                // Like `remove_version_dir`: the repo directory goes once empty.
                let _ = std::fs::remove_dir(repo);
            }
        }
        println!(
            "removed {} of {} version(s)",
            stale.len() - failed,
            stale.len()
        );
        Ok(i32::from(failed > 0))
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

/// What `install` leaves in a version directory: one regular file, the library.
fn is_install_shape(dir: &Path) -> bool {
    let Ok(read) = std::fs::read_dir(dir) else {
        return false;
    };
    let files: Vec<_> = read.flatten().collect();
    matches!(files.as_slice(), [e]
        if e.file_type().is_ok_and(|t| t.is_file())
            && e.file_name().to_string_lossy().ends_with(std::env::consts::DLL_SUFFIX))
}

/// Real (non-symlink) subdirectories of `dir`; a missing `dir` has none.
fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    read.flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect()
}

fn size(dir: &Path) -> u64 {
    let Ok(read) = std::fs::read_dir(dir) else {
        return 0;
    };
    read.flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => size(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map_or(0, |m| m.len()),
            _ => 0,
        })
        .sum()
}

fn human(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 20 => format!("{:.1} MiB", b as f64 / (1u64 << 20) as f64),
        b if b >= 1 << 10 => format!("{:.1} KiB", b as f64 / 1024.0),
        b => format!("{b} B"),
    }
}

fn real(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Whether any of `used` (from `referenced_paths`) lies inside `dir`.
fn is_referenced(used: &[PathBuf], dir: &Path) -> bool {
    let dir = real(dir);
    used.iter().any(|path| path.starts_with(&dir))
}

/// `<data>/plugins/<repo>/<version>/`: where one release of one plugin lives.
pub(crate) fn version_dir(target: &Target, repo: &str, version: &str) -> PathBuf {
    target
        .data
        .join("plugins")
        .join(install::repo_basename(repo))
        .join(version)
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
pub(crate) fn remove_version_dir(lib: &Path) -> std::io::Result<()> {
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
