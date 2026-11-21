//! `bddkit ui` on a machine without `bddkit-ui` (issue #287): ask, then fetch
//! the local UI from its public releases and put it next to this binary.
//! The core stores no consent and shows no licence: the UI shows its own EULA
//! on its first start, so the only question here is whether to install it.

use crate::plugin::install::{self, Sources};
use crate::plugin::manage;
use crate::report::progress::can_prompt;
use crate::telemetry::payload::PluginResult;
use crate::upgrade;
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

const REPO: &str = "bddkit/bddkit-ui-releases";
/// The executable's name and the prefix of its release assets; the repository
/// is called otherwise, so `asset_name` is given this, not `REPO`.
const NAME: &str = "bddkit-ui";

/// The installed UI, or an error that ends the command with exit 2. Nothing
/// is requested from the network before the user has said yes.
pub async fn offer(exe_dir: &Path) -> Result<PathBuf> {
    // No network and no question yet: a host with no build hears it first.
    let target = install::host_target().map_err(|_| {
        anyhow::anyhow!(
            "{NAME} has no build for {}-{}",
            std::env::consts::ARCH,
            std::env::consts::OS
        )
    })?;
    if !can_prompt() {
        bail!(
            "{NAME} is not installed, and installing it asks a question: run `bddkit ui` in an interactive terminal (stdin and stderr a terminal, no CI variable set)"
        );
    }
    println!(
        "{NAME} is not installed. It is a closed-source product, free to use under a licence agreement (EULA) that it shows on its first start."
    );
    if !upgrade::confirm("Install it?")? {
        bail!("{NAME} was not installed");
    }
    let sources = Sources::from_env();
    let client = manage::http_client(&sources)?;
    let version = install::latest_version(&client, &sources.github, REPO).await?;
    let asset = install::asset_name(NAME, &version, target);
    let download = install::download_asset(&client, &sources.github, REPO, &version, asset)
        .await
        .map_err(|error| {
            let no_build = matches!(
                error.downcast_ref::<install::Classed>().map(|c| c.class),
                Some(PluginResult::NoBuild)
            );
            if no_build {
                anyhow::anyhow!("{NAME} {version} has no build for {target}")
            } else {
                error
            }
        })?;
    place(&download.bytes, &download.asset, exe_dir)
}

/// Unpacks beside the target and renames into place, so an interrupted write
/// never leaves a half-written `bddkit-ui` for `find` to run.
fn place(archive: &[u8], asset: &str, exe_dir: &Path) -> Result<PathBuf> {
    let staging = exe_dir.join(format!(".{NAME}-staging-{}", std::process::id()));
    let result = std::fs::create_dir_all(&staging)
        .with_context(|| {
            format!(
                "cannot write to {}: install {NAME} there by hand, or run `bddkit ui` with the rights to write it",
                exe_dir.display()
            )
        })
        .and_then(|()| install::extract_executable(archive, asset, &staging, NAME))
        .and_then(|staged| {
            let target = exe_dir.join(format!("{NAME}{}", std::env::consts::EXE_SUFFIX));
            std::fs::rename(&staged, &target)
                .with_context(|| format!("failed to place {}", target.display()))?;
            Ok(target)
        });
    let _ = std::fs::remove_dir_all(&staging);
    result
}
