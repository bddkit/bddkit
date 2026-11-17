//! One entry of `bddkit plugin update`.

use super::{Context, HOST_ABIS, Target};
use crate::plugin::install::{self, IndexEntry};
use crate::plugin::lock::LockEntry;
use crate::telemetry::payload::{Acted, PluginFrom};

impl Context {
    /// Updates one lock entry and prints its row. Returns true when it failed.
    pub(super) async fn update_entry(
        &self,
        target: &Target,
        entry: &LockEntry,
        index: &[IndexEntry],
        dry_run: bool,
    ) -> bool {
        let row = format!("{} ({})", entry.name, target.label);
        let Some(source) = &entry.source else {
            println!("{row}: skipped: not installed by bddkit");
            return false;
        };
        let Some(current) = &entry.version else {
            println!("{row}: skipped: no version recorded, reinstall it with `plugin install`");
            return false;
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
                HOST_ABIS,
            )
            .await?
            {
                Ok(latest) => latest,
                Err(why) => {
                    return Ok(format!(
                        "up to date for ABI {} ({current}): {why}",
                        install::abi_label(&HOST_ABIS)
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
            self.install_into(target, source, &latest).await?;
            Ok::<_, anyhow::Error>(format!("updated {current} → {latest}"))
        }
        .await;
        if !dry_run {
            self.note(&acted, &outcome);
        }
        match outcome {
            Ok(message) => {
                println!("{row}: {message}");
                false
            }
            Err(error) => {
                println!("{row}: failed: {error:#}");
                true
            }
        }
    }
}
