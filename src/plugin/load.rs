//! The steps of `Plugins::load_collecting`: load the libraries, work out the
//! instances the config and the manifests declare, then ask each plugin to
//! validate its own.

use super::{Plugins, library, library::Library, lock};
use crate::config::InstanceSpec;
use crate::options::Options;
use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;

/// The libraries, whether each is an official one, and the library index
/// serving each group.
pub(super) type Loaded = (Vec<Library>, Vec<bool>, BTreeMap<String, usize>);

/// Loads every lock entry and checks it against the run.
pub(super) fn libraries(entries: Vec<lock::LockEntry>, concurrency: usize) -> Result<Loaded> {
    let mut libs: Vec<Library> = Vec::with_capacity(entries.len());
    let mut official = Vec::with_capacity(entries.len());
    let mut groups: BTreeMap<String, usize> = BTreeMap::new();
    for entry in entries {
        official.push(crate::telemetry::is_official(entry.source.as_deref()));
        let lib = Library::load(&entry.name, &entry.path)?;
        // The lock entry's name is the one every diagnostic and every
        // `plugin remove`/`plugin update` keys on, so it must be the
        // plugin's own. A mismatch is never legitimate: it means the lock
        // file points at the wrong file. Checked here rather than in
        // `Library::load`, which `plugin install owner/repo` calls before
        // any name is known.
        if lib.manifest.name != entry.name {
            bail!(
                "lock entry {:?} points at a plugin whose manifest says {:?}",
                entry.name,
                lib.manifest.name
            );
        }
        library::check_select_instance(
            &lib.name,
            lib.manifest.concurrency,
            lib.has_select_instance(),
            concurrency,
        )?;
        library::check_reset_scenario(
            &lib.name,
            lib.manifest.concurrency,
            lib.has_reset_scenario(),
            concurrency,
        )?;
        library::check_failure_context(
            &lib.name,
            lib.manifest.concurrency,
            lib.has_failure_context(),
            concurrency,
        )?;
        let index = libs.len();
        for group in &lib.manifest.groups {
            if let Some(other) = groups.insert(group.clone(), index) {
                bail!(
                    "plugins {:?} and {:?} both claim the resource group {group:?}",
                    libs[other].name,
                    lib.name
                );
            }
        }
        libs.push(lib);
    }
    Ok((libs, official, groups))
}

/// What the config declared per `(group, instance)`, the default instance of
/// each synthesized group, and the plugin each synthesized group came from.
pub(super) type Declared = (
    BTreeMap<(String, String), InstanceSpec>,
    BTreeMap<String, String>,
    BTreeMap<String, String>,
);

/// A group the config never mentions, whose plugin declares what an
/// instance takes by default, gets one instance named `default`. It is
/// the group's only instance, hence its default — set here because the
/// host's own `default_<group>` resolution walks the config's groups
/// and never sees this one. Any declared section, an empty map
/// included, wins: the suite named its instances, so it gets those.
pub(super) fn declared_instances(
    libs: &[Library],
    instances: &[InstanceSpec],
    groups_in_config: &[String],
    global: &Options,
) -> Declared {
    let mut declared = BTreeMap::new();
    for spec in instances {
        declared.insert((spec.group.clone(), spec.name.clone()), spec.clone());
    }
    let mut defaults = BTreeMap::new();
    let mut implicit: BTreeMap<String, String> = BTreeMap::new();
    for lib in libs {
        for (group, body) in &lib.manifest.implicit_instance {
            if groups_in_config.contains(group) {
                continue;
            }
            declared.insert(
                (group.clone(), "default".to_string()),
                InstanceSpec {
                    group: group.clone(),
                    name: "default".to_string(),
                    config: body.clone(),
                    options: global.clone(),
                },
            );
            defaults.insert(group.clone(), "default".to_string());
            implicit.insert(group.clone(), lib.name.clone());
        }
    }
    (declared, defaults, implicit)
}

/// What `validate_config` refused each instance with, and the `detail` it gave
/// the ones it accepted.
type Verdicts = (
    BTreeMap<(String, String), String>,
    BTreeMap<(String, String), String>,
);

impl Plugins {
    /// Eager, cheap, no connections opened: a typo in the config must exit 2
    /// before the first request, which lazy init alone would not give. A
    /// synthesized instance goes through the same check: a body its own
    /// plugin refuses is a plugin bug, and load time is when to find out.
    ///
    /// The group is looked up, never indexed: `groups_in_config` is the
    /// caller's list of config keys, and an instance whose group is missing
    /// from it must produce this error rather than an index panic.
    pub(super) fn validate_declared(
        &self,
        implicit: &BTreeMap<String, String>,
    ) -> Result<Verdicts> {
        let mut refused = BTreeMap::new();
        let mut details = BTreeMap::new();
        for spec in self.declared.values() {
            // Where the body came from — the config has no `resources.<group>`
            // line to send the reader to for a synthesized one.
            let origin = match implicit.get(&spec.group) {
                Some(plugin) => format!("plugin {plugin:?}: implicit_instance.{}", spec.group),
                None => format!("resources.{}.{}", spec.group, spec.name),
            };
            let lib = self
                .groups
                .get(&spec.group)
                .map(|index| &self.libs[*index])
                .with_context(|| {
                    format!(
                        "{origin}: no installed plugin serves the group {:?}; `bddkit plugin install` installs the plugins this config needs",
                        spec.group
                    )
                })?;
            // An encode failure or a malformed reply is this instance's refusal
            // too: stopping on it would hide the instances after it.
            let verdict = match self
                .init_request(spec)
                .and_then(|request| lib.validate_config(&request))
            {
                Ok(Ok(detail)) => {
                    if let Some(detail) = detail {
                        details.insert((spec.group.clone(), spec.name.clone()), detail);
                    }
                    continue;
                }
                Ok(Err(error)) => error,
                Err(error) => format!("{error:#}"),
            };
            refused.insert(
                (spec.group.clone(), spec.name.clone()),
                format!("{origin}: {verdict}"),
            );
        }
        Ok((refused, details))
    }
}
