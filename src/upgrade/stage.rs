//! The parts of `upgrade` that resolve a version, stage its binary and fetch
//! the plugin versions that go with it.

use super::{
    CURRENT, Cleanup, Info, Move, Options, Plan, REPO, check_with_staged, install, manage,
};
use crate::plugin::manage::{Context, Fetched};
use anyhow::{Context as _, Result, bail};
use std::path::{Path, PathBuf};

/// The release to upgrade to. `None`: there is nothing to do, and it said so.
pub(super) async fn target_version(ctx: &Context, opts: &Options) -> Result<Option<String>> {
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
        return Ok(None);
    }
    if opts.version.is_none() && super::older(&version, CURRENT) {
        println!(
            "the latest release ({version}) is older than the installed {CURRENT}: --version {version} downgrades"
        );
        return Ok(None);
    }
    Ok(Some(version))
}

/// Beside the binary rather than in the temp directory: the final rename
/// cannot cross filesystems, and a directory we cannot create here is the
/// "cannot write where bddkit lives" answer, before anything is downloaded.
/// The staged binary is run, so it is staged where the real run would stage
/// it. `--check` switches nothing and may fall back to the temp directory
/// when this one is not writable, but then says so: a plan the real run
/// would refuse must not look fine. Returns the directory and, for that
/// fallback, the bin directory that is not writable.
pub(super) fn staging_dir<'a>(
    opts: &Options,
    bin_dir: &'a Path,
) -> Result<(PathBuf, Option<&'a Path>)> {
    let mut staging = bin_dir.join(format!(".bddkit-upgrade-{}", std::process::id()));
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
        return Ok((staging, Some(bin_dir)));
    }
    Ok((staging, None))
}

/// A release must be the version asked for, built for this machine.
pub(super) fn verify_release(info: &Info, version: &str, host: &str) -> Result<()> {
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
    Ok(())
}

/// New plugin versions land beside the old ones, and the staged binary
/// must accept each before anything is replaced. Two lock files moving the
/// same release share one directory: it is fetched and checked once. Returns
/// what was fetched and the rows of what `--force` left as it is.
pub(super) async fn fetch_moves<'a>(
    ctx: &Context,
    opts: &Options,
    plan: &'a Plan,
    staged: &Path,
    cleanup: &mut Cleanup,
) -> Result<(Vec<(&'a Move, Fetched)>, Vec<String>)> {
    let mut fetched: Vec<(&Move, Fetched)> = Vec::new();
    let mut refused: Vec<String> = Vec::new();
    let mut seen: std::collections::HashMap<PathBuf, Option<Fetched>> =
        std::collections::HashMap::default();
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
        // `upgrade` never kept a copy of the version it is about to replace.
        got.commit();
        cleanup.created.push(got.lib_path.clone());
        if let Err(error) = check_with_staged(staged, &planned.name, &got.lib_path) {
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
    Ok((fetched, refused))
}
