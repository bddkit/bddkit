//! `bddkit upgrade`: replace the running binary with another release and move
//! every plugin bddkit installed to a release for the new plugin ABI, or
//! change nothing. The sequence is the one in the specification attached to #121:
//! resolve, stage and verify, plan, install the new plugin versions beside the
//! old ones, have the STAGED binary vouch for them, and only then switch.
//!
//! Two rules carry the design. The receipt `install.sh` / `install.ps1` write
//! is the only proof a binary is bddkit's to replace. And nothing the running
//! process loads is trusted for the new release: this process speaks the OLD
//! plugin ABI, so a new-ABI plugin is checked by `<staged> plugin check`, never
//! by `Library::load` here.

use crate::dirs;
use crate::plugin::install;
use crate::plugin::lock;
use crate::plugin::manage::{self, Context, Fetched, Target};
use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

const REPO: &str = "bddkit/bddkit";
const CURRENT: &str = env!("CARGO_PKG_VERSION");
const BINARY: &str = if cfg!(windows) {
    "bddkit.exe"
} else {
    "bddkit"
};

pub struct Options {
    pub check: bool,
    pub version: Option<String>,
    pub rollback: bool,
    pub force: bool,
    pub yes: bool,
}

/// `install.json`, written by the install scripts in the user layer's data
/// directory: the one binary bddkit put there.
#[derive(Serialize, Deserialize)]
struct Receipt {
    path: PathBuf,
    version: String,
    target: String,
}

/// `<data>/previous/pending.json`: what the last switch backed up, so
/// `--rollback` restores exactly that and not a `.before-upgrade` file left
/// by an older upgrade.
#[derive(Serialize, Deserialize)]
struct Pending {
    from: String,
    to: String,
    /// Lock files that have a `.before-upgrade` copy.
    locks: Vec<PathBuf>,
}

/// What the staged binary says about itself (`version --json`).
#[derive(Deserialize)]
struct Info {
    version: String,
    target: String,
    plugin_abi: u32,
}

/// One plugin that moves with the binary.
struct Move {
    target: Target,
    name: String,
    repo: String,
    from: String,
    to: String,
}

#[derive(Default)]
struct Plan {
    moves: Vec<Move>,
    /// Writable entries with no release for the new ABI.
    blocked: Vec<String>,
    /// Hand-written and project-layer entries: listed, never changed.
    left: Vec<String>,
}

/// Removes the staging directory always, and the plugin directories this run
/// created unless the switch began — until then nothing may outlive a failure.
struct Cleanup {
    staging: PathBuf,
    created: Vec<PathBuf>,
    keep_created: bool,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.staging);
        if !self.keep_created {
            for lib in &self.created {
                let _ = manage::remove_version_dir(lib);
            }
        }
    }
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn sibling_of_backup(lock: &Path) -> PathBuf {
    let mut name = lock.as_os_str().to_os_string();
    name.push(".before-upgrade");
    PathBuf::from(name)
}

fn refusal(exe: &Path) -> anyhow::Error {
    anyhow::anyhow!(
        "{} was not installed by install.sh / install.ps1, so `bddkit upgrade` will not replace it:\n  \
         - built with cargo: cargo install --git https://github.com/{REPO} --force\n  \
         - from a package manager or a container image: update it with that manager or image\n  \
         - copied by hand: run install.sh (install.ps1 on Windows) once, which records the install",
        exe.display()
    )
}

fn read_receipt(path: &Path) -> Result<Option<Receipt>> {
    match std::fs::read_to_string(path) {
        Ok(raw) => {
            Ok(Some(serde_json::from_str(&raw).with_context(|| {
                format!("failed to parse {}", path.display())
            })?))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("failed to read {}", path.display())),
    }
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    std::fs::write(path, serde_json::to_string_pretty(value)?)
        .with_context(|| format!("failed to write {}", path.display()))
}

fn confirm(question: &str) -> Result<bool> {
    print!("{question} [y/N] ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(matches!(line.trim().to_lowercase().as_str(), "y" | "yes"))
}

/// Puts `new` where `target` is. A rename on Unix, which the running process
/// does not mind; on Windows the running executable cannot be renamed over,
/// and `self-replace` moves it aside instead.
fn swap(new: &Path, target: &Path) -> Result<()> {
    #[cfg(windows)]
    if std::env::current_exe().map(|exe| canonical(&exe)).ok() == Some(canonical(target)) {
        return self_replace::self_replace(new)
            .with_context(|| format!("failed to replace {}", target.display()));
    }
    std::fs::rename(new, target).with_context(|| format!("failed to replace {}", target.display()))
}

/// `swap` for a file that may sit on another filesystem than `target`: a
/// rename across filesystems fails, so copy beside the target first.
fn place(src: &Path, target: &Path) -> Result<()> {
    let tmp = target.with_file_name(format!(".bddkit-restore-{}", std::process::id()));
    std::fs::copy(src, &tmp).with_context(|| format!("failed to copy {}", src.display()))?;
    swap(&tmp, target).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

pub async fn run(ctx: &Context, opts: &Options) -> Result<i32> {
    let data = dirs::user_data(ctx.os, &ctx.env)
        .context("no user data directory (HOME / LOCALAPPDATA unset)")?;
    let receipt_path = data.join("install.json");
    let exe = canonical(&std::env::current_exe().context("cannot tell which binary is running")?);
    let Some(receipt) = read_receipt(&receipt_path)? else {
        return Err(refusal(&exe));
    };
    let managed = canonical(&receipt.path);
    let previous = data.join("previous");
    // `--rollback` is also how a new binary that will not start is undone, so
    // it may run as the previous binary this tool kept.
    let is_previous = exe == canonical(&previous.join(BINARY));
    if exe != managed && !(opts.rollback && is_previous) {
        return Err(refusal(&exe));
    }
    if opts.rollback {
        return rollback(&receipt_path, receipt, &managed, &previous);
    }
    if !opts.check && !opts.yes && !std::io::stdin().is_terminal() {
        bail!("there is no terminal to confirm on: pass --yes, or --check to see the plan only");
    }

    let github = &ctx.sources.github;
    let version = match &opts.version {
        Some(version) => {
            let version = version.strip_prefix('v').unwrap_or(version);
            install::check_segment("version", version)?;
            version.to_string()
        }
        None => install::latest_version(&ctx.client, github, REPO).await?,
    };
    if version == CURRENT {
        println!("bddkit {CURRENT} is already the version installed");
        return Ok(0);
    }
    if opts.version.is_none() && older(&version, CURRENT) {
        println!(
            "the latest release ({version}) is older than the installed {CURRENT}: --version {version} downgrades"
        );
        return Ok(0);
    }

    // Beside the binary rather than in the temp directory: the final rename
    // cannot cross filesystems, and a directory we cannot create here is the
    // "cannot write where bddkit lives" answer, before anything is downloaded.
    let bin_dir = managed
        .parent()
        .context("the installed binary has no directory")?;
    // The staged binary is run, so it is staged where the real run would stage
    // it. `--check` switches nothing and may fall back to the temp directory
    // when this one is not writable, but then says so: a plan the real run
    // would refuse must not look fine.
    let mut staging = bin_dir.join(format!(".bddkit-upgrade-{}", std::process::id()));
    let mut not_writable = None;
    if let Err(error) = std::fs::create_dir_all(&staging) {
        if !opts.check {
            return Err(error).with_context(|| {
                format!(
                    "cannot write to {}, where bddkit is installed; upgrade it as the user who installed it",
                    bin_dir.display()
                )
            });
        }
        staging = std::env::temp_dir().join(format!(".bddkit-upgrade-{}", std::process::id()));
        std::fs::create_dir_all(&staging)
            .with_context(|| format!("cannot write to {}", staging.display()))?;
        not_writable = Some(bin_dir);
    }
    let mut cleanup = Cleanup {
        staging: staging.clone(),
        created: Vec::new(),
        keep_created: false,
    };

    let host = install::host_target()?;
    let download = install::download(&ctx.client, github, REPO, &version, host).await?;
    let staged = install::extract_binary(&download.bytes, &download.asset, &staging)?;
    let info = staged_info(&staged)?;
    if info.version != version {
        bail!(
            "the release v{version} holds a bddkit that reports itself as {}",
            info.version
        );
    }
    if info.target != host {
        bail!(
            "the release holds a bddkit for {}, this machine is {host}",
            info.target
        );
    }

    let abi_changes = info.plugin_abi != crate::plugin::abi::ABI_VERSION;
    let plan = if abi_changes {
        plan_plugins(ctx, info.plugin_abi).await?
    } else {
        Plan::default()
    };
    print_plan(&version, &info, abi_changes, &plan);
    if !plan.blocked.is_empty() && !opts.force {
        bail!(
            "nothing was changed: {} plugin(s) have no release for plugin ABI {}; --force upgrades anyway and leaves them",
            plan.blocked.len(),
            info.plugin_abi
        );
    }
    if let Some(dir) = not_writable {
        println!(
            "the upgrade itself would be refused: {} is not writable by this user",
            dir.display()
        );
    }
    if opts.check {
        return Ok(0);
    }
    if !opts.yes && !confirm("Upgrade?")? {
        println!("nothing was changed");
        return Ok(0);
    }

    preflight(ctx, &plan, &previous, &receipt_path)?;

    // New plugin versions land beside the old ones, and the staged binary
    // must accept each before anything is replaced. Two lock files moving the
    // same release share one directory: it is fetched and checked once.
    let mut fetched: Vec<(&Move, Fetched)> = Vec::new();
    let mut refused: Vec<String> = Vec::new();
    let mut seen: std::collections::HashMap<PathBuf, Option<Fetched>> = Default::default();
    for planned in &plan.moves {
        let dir = manage::version_dir(&planned.target, &planned.repo, &planned.to);
        if let Some(known) = seen.get(&dir) {
            match known {
                Some(got) => fetched.push((planned, got.clone())),
                None => refused.push(format!(
                    "{} ({}): refused above",
                    planned.name, planned.target.label
                )),
            }
            continue;
        }
        let got = ctx
            .fetch_into(&planned.target, &planned.repo, &planned.to)
            .await
            .with_context(|| format!("{} {}", planned.name, planned.to))?;
        cleanup.created.push(got.lib_path.clone());
        if let Err(error) = check_with_staged(&staged, &planned.name, &got.lib_path) {
            // The same condition as a missing release, found later: --force
            // leaves the plugin as it is, otherwise nothing changes.
            if !opts.force {
                return Err(error.context(format!("{} {}", planned.name, planned.to)));
            }
            cleanup.created.pop();
            let _ = manage::remove_version_dir(&got.lib_path);
            refused.push(format!(
                "{} ({}): {error:#}",
                planned.name, planned.target.label
            ));
            seen.insert(dir, None);
            continue;
        }
        seen.insert(dir, Some(got.clone()));
        fetched.push((planned, got));
    }

    let mut locks: Vec<PathBuf> = Vec::new();
    for (planned, _) in &fetched {
        if !locks.contains(&planned.target.lock) {
            locks.push(planned.target.lock.clone());
        }
    }
    backup(&exe, &previous, &locks, &version)?;
    cleanup.keep_created = true;
    let switched = switch(
        &staged,
        &managed,
        &receipt_path,
        receipt,
        &version,
        &fetched,
    );
    if let Err(error) = switched {
        bail!(
            "{error:#}\nthe switch stopped halfway; `bddkit upgrade --rollback` restores the previous binary and lock files ({} backed up)",
            locks.len()
        );
    }

    println!("bddkit {CURRENT} → {version}");
    for row in &refused {
        println!("left as it is: {row}");
    }
    for (planned, _) in &fetched {
        println!(
            "{} {} → {} ({}, ABI {} → {})",
            planned.name,
            planned.from,
            planned.to,
            planned.target.label,
            crate::plugin::abi::ABI_VERSION,
            info.plugin_abi
        );
    }
    Ok(0)
}

/// Everything the switch will write, refused now rather than halfway (the bin
/// directory is proven by the staging directory): the receipt, the kept-binary directory, each lock file and data directory a
/// plugin moves through, and a version directory that something else uses.
fn preflight(ctx: &Context, plan: &Plan, previous: &Path, receipt_path: &Path) -> Result<()> {
    let writable = |path: &Path| -> Result<()> {
        std::fs::OpenOptions::new()
            .append(true)
            .open(path)
            .map(drop)
            .with_context(|| format!("cannot write {}", path.display()))
    };
    writable(receipt_path)?;
    std::fs::create_dir_all(previous)
        .with_context(|| format!("cannot write {}", previous.display()))?;
    let mut in_use: Vec<(String, PathBuf)> = Vec::new();
    for target in ctx.targets(None) {
        for entry in lock::read_file(&target.lock)? {
            in_use.push((format!("{} ({})", entry.name, target.label), entry.path));
        }
    }
    for planned in &plan.moves {
        writable(&planned.target.lock)?;
        let plugins = planned.target.data.join("plugins");
        std::fs::create_dir_all(&plugins)
            .with_context(|| format!("cannot write {}", plugins.display()))?;
        let dir = manage::version_dir(&planned.target, &planned.repo, &planned.to);
        if let Some((who, _)) = in_use.iter().find(|(_, path)| path.starts_with(&dir)) {
            bail!(
                "{} is already used by {who}: nothing was changed; point it elsewhere or run `bddkit plugin update`",
                dir.display()
            );
        }
    }
    Ok(())
}

/// Whether release `a` is older than `b`, comparing the leading numeric
/// components. Anything that does not parse is "not older": the guard is a
/// courtesy against a stale `latest`, never a reason to refuse.
fn older(a: &str, b: &str) -> bool {
    let parts = |v: &str| -> Option<Vec<u64>> {
        v.split(['-', '+'])
            .next()?
            .split('.')
            .map(|n| n.parse().ok())
            .collect()
    };
    matches!((parts(a), parts(b)), (Some(a), Some(b)) if a < b)
}

fn staged_info(staged: &Path) -> Result<Info> {
    let out = Command::new(staged)
        .args(["version", "--json"])
        .output()
        .with_context(|| {
            format!(
                "the downloaded bddkit does not run here ({})",
                staged.display()
            )
        })?;
    if !out.status.success() {
        bail!(
            "the downloaded bddkit failed `version --json` (a release older than `bddkit upgrade` has no such flag: install.sh --version <x> installs it): {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    serde_json::from_slice(&out.stdout)
        .context("the downloaded bddkit's `version --json` is malformed")
}

/// Asks the staged binary to load the plugin: the host that will run it is
/// the only one whose answer counts.
fn check_with_staged(staged: &Path, name: &str, lib: &Path) -> Result<()> {
    let out = Command::new(staged)
        .args(["plugin", "check"])
        .arg(lib)
        .output()
        .context("failed to run the downloaded bddkit")?;
    if !out.status.success() {
        bail!(
            "the new bddkit refuses it: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let reported = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if reported != name {
        bail!("the new bddkit loads it as {reported:?}, the lock file calls it {name:?}");
    }
    Ok(())
}

async fn plan_plugins(ctx: &Context, abi: u32) -> Result<Plan> {
    // A dead index costs only the shortcut: each plugin is then asked directly.
    let index = install::fetch_index(&ctx.client, &ctx.sources)
        .await
        .unwrap_or_default();
    let mut plan = Plan::default();
    for target in ctx.targets(None) {
        let project = target.label.starts_with("project");
        for entry in lock::read_file(&target.lock)? {
            let row = format!("{} ({})", entry.name, target.label);
            let (false, Some(repo), Some(from)) = (project, &entry.source, &entry.version) else {
                let why = if project {
                    "project layer"
                } else if entry.source.is_some() {
                    "installed by bddkit but records no version"
                } else {
                    "hand-written"
                };
                plan.left.push(format!("{row}: {why}"));
                continue;
            };
            let indexed = index.iter().find(|e| &e.repo == repo);
            let release =
                install::host_release(&ctx.client, &ctx.sources.github, repo, indexed, abi).await;
            match release {
                // The newest release for this ABI is the one installed: fine
                // when the index or the release itself says it was built for
                // it, otherwise the plugin has not published one, and the new
                // binary would refuse it at load.
                Ok(Ok(to)) if &to == from => {
                    let indexed_for =
                        indexed.is_some_and(|e| e.abi.get(&abi.to_string()) == Some(&to));
                    let declared_for = !indexed_for
                        && matches!(
                            install::release_abi(&ctx.client, &ctx.sources.github, repo, &to).await,
                            Ok(Some(declared)) if declared == abi
                        );
                    if !(indexed_for || declared_for) {
                        plan.blocked.push(format!(
                            "{row}: {from} is its newest release and was not built for ABI {abi}"
                        ));
                    }
                }
                Ok(Ok(to)) => plan.moves.push(Move {
                    target: target.clone(),
                    name: entry.name.clone(),
                    repo: repo.clone(),
                    from: from.clone(),
                    to,
                }),
                Ok(Err(why)) => plan.blocked.push(format!("{row}: {why}")),
                Err(error) => plan.blocked.push(format!("{row}: {error:#}")),
            }
        }
    }
    Ok(plan)
}

fn print_plan(version: &str, info: &Info, abi_changes: bool, plan: &Plan) {
    println!(
        "bddkit {CURRENT} → {version} (plugin ABI {})",
        info.plugin_abi
    );
    if !abi_changes {
        println!(
            "the plugin ABI is unchanged: plugins are left alone (`bddkit plugin update` moves them)"
        );
        return;
    }
    for planned in &plan.moves {
        println!(
            "  {} {} → {} ({})",
            planned.name, planned.from, planned.to, planned.target.label
        );
    }
    for row in &plan.blocked {
        println!("  no release for ABI {}: {row}", info.plugin_abi);
    }
    for row in &plan.left {
        println!("  left as it is: {row}");
    }
}

/// Keeps what the switch is about to change: the running binary under
/// `<data>/previous/`, and each lock file as `<file>.before-upgrade`. The
/// new copies are made under `.new` names first and moved into place only
/// when all of them exist, with the record written last: a failure while
/// copying leaves the previous upgrade's backups, and so a rollback of it,
/// exactly as they were. Backups of an older upgrade are dropped at the
/// commit, not before.
fn backup(exe: &Path, previous: &Path, locks: &[PathBuf], to: &str) -> Result<()> {
    std::fs::create_dir_all(previous)
        .with_context(|| format!("failed to create {}", previous.display()))?;
    let staged_name = |path: &Path| {
        let mut name = path.as_os_str().to_os_string();
        name.push(".new");
        PathBuf::from(name)
    };
    let kept = previous.join(BINARY);
    let mut copies: Vec<(PathBuf, PathBuf)> = vec![(exe.to_path_buf(), kept)];
    copies.extend(locks.iter().map(|l| (l.clone(), sibling_of_backup(l))));
    let mut made: Vec<PathBuf> = Vec::new();
    let prepared = copies.iter().try_for_each(|(from, to)| {
        let tmp = staged_name(to);
        let _ = std::fs::remove_file(&tmp);
        made.push(tmp.clone());
        std::fs::copy(from, &tmp)
            .map(drop)
            .with_context(|| format!("failed to back up {}", from.display()))
    });
    if let Err(error) = prepared {
        made.iter().for_each(|tmp| {
            let _ = std::fs::remove_file(tmp);
        });
        return Err(error);
    }

    let record = previous.join("pending.json");
    let old: Option<Pending> = std::fs::read_to_string(&record)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok());
    for (_, to) in &copies {
        std::fs::rename(staged_name(to), to)
            .with_context(|| format!("failed to keep {}", to.display()))?;
    }
    // Only after the new copies are in place, and never one of them: a lock
    // that two upgrades both backed up would lose the fresh backup here.
    for lock in old.into_iter().flat_map(|old| old.locks) {
        if !locks.contains(&lock) {
            let _ = std::fs::remove_file(sibling_of_backup(&lock));
        }
    }
    write_json(
        &record,
        &Pending {
            from: CURRENT.to_string(),
            to: to.to_string(),
            locks: locks.to_vec(),
        },
    )
}

fn switch(
    staged: &Path,
    managed: &Path,
    receipt_path: &Path,
    receipt: Receipt,
    version: &str,
    fetched: &[(&Move, Fetched)],
) -> Result<()> {
    swap(staged, managed)?;
    for (planned, got) in fetched {
        let record = Context::record(
            &planned.target,
            &planned.name,
            &planned.repo,
            &planned.to,
            got,
        )?;
        lock::upsert(&planned.target.lock, &record)?;
    }
    write_json(
        receipt_path,
        &Receipt {
            version: version.to_string(),
            ..receipt
        },
    )
}

fn rollback(receipt_path: &Path, receipt: Receipt, managed: &Path, previous: &Path) -> Result<i32> {
    let record = previous.join("pending.json");
    let kept = previous.join(BINARY);
    let Ok(raw) = std::fs::read_to_string(&record) else {
        bail!("nothing to roll back: no upgrade has been made, or it was already rolled back");
    };
    let pending: Pending = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse {}", record.display()))?;
    place(&kept, managed)?;
    let mut report: Vec<String> = Vec::new();
    for lock in &pending.locks {
        match std::fs::rename(sibling_of_backup(lock), lock) {
            Ok(()) => report.push(format!("restored {}", lock.display())),
            // Restored by an earlier attempt that stopped later on, or lost:
            // a retry must be able to finish, and the output must not claim
            // a restore that did not happen.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                report.push(format!("{}: no backup left, unchanged", lock.display()));
            }
            Err(e) => {
                return Err(e).with_context(|| format!("failed to restore {}", lock.display()));
            }
        }
    }
    write_json(
        receipt_path,
        &Receipt {
            version: pending.from.clone(),
            ..receipt
        },
    )?;
    let _ = std::fs::remove_dir_all(previous);
    println!("bddkit {} → {}", pending.to, pending.from);
    for line in report {
        println!("{line}");
    }
    if !pending.locks.is_empty() {
        println!(
            "lock files are back as they were before the upgrade, including any change made to them since"
        );
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("bddkit-upgrade-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    #[test]
    fn releases_are_compared_by_their_numeric_parts() {
        assert!(older("0.3.9", "0.3.10"));
        assert!(older("0.9.0", "1.0.0"));
        assert!(!older("1.0.0", "1.0.0"));
        assert!(!older("1.0.1", "1.0.0"));
        assert!(
            !older("1.0.0-rc.1", "1.0.0"),
            "a pre-release suffix is ignored"
        );
        assert!(
            !older("nightly", "1.0.0"),
            "what does not parse is not older"
        );
    }

    #[test]
    fn a_failed_backup_leaves_the_previous_upgrade_as_it_was() {
        let dir = temp("backup");
        let previous = dir.join("previous");
        std::fs::create_dir_all(&previous).expect("mkdir");
        let exe = dir.join("bddkit");
        std::fs::write(&exe, "new").expect("exe");
        // The record and binary of an earlier upgrade, and one lock backup.
        let lock = dir.join("plugins.yaml");
        std::fs::write(sibling_of_backup(&lock), "old lock").expect("backup");
        std::fs::write(previous.join(BINARY), "old binary").expect("kept");
        write_json(
            &previous.join("pending.json"),
            &Pending {
                from: "1".into(),
                to: "2".into(),
                locks: vec![lock.clone()],
            },
        )
        .expect("record");
        // A directory where the new copy must go makes the copy fail.
        std::fs::create_dir(previous.join(format!("{BINARY}.new"))).expect("blocker");

        let error = backup(&exe, &previous, &[], "3").expect_err("the copy fails");
        assert!(
            format!("{error:#}").contains("failed to back up"),
            "{error:#}"
        );
        assert_eq!(
            std::fs::read_to_string(previous.join(BINARY)).unwrap(),
            "old binary"
        );
        assert_eq!(
            std::fs::read_to_string(sibling_of_backup(&lock)).unwrap(),
            "old lock"
        );
        let record: Pending =
            serde_json::from_str(&std::fs::read_to_string(previous.join("pending.json")).unwrap())
                .unwrap();
        assert_eq!(record.to, "2");
    }

    #[test]
    fn a_lock_backed_up_twice_keeps_the_newer_backup() {
        let dir = temp("backup-twice");
        let previous = dir.join("previous");
        let exe = dir.join("bddkit");
        std::fs::write(&exe, "bin").expect("exe");
        let lock = dir.join("plugins.yaml");

        std::fs::write(&lock, "first").expect("lock");
        backup(&exe, &previous, std::slice::from_ref(&lock), "2").expect("first backup");
        std::fs::write(&lock, "second").expect("lock");
        backup(&exe, &previous, std::slice::from_ref(&lock), "3").expect("second backup");

        assert_eq!(
            std::fs::read_to_string(sibling_of_backup(&lock)).unwrap(),
            "second"
        );
        assert!(!previous.join(format!("{BINARY}.new")).exists());
    }
}
