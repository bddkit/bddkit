pub mod abi;
pub mod library;
pub mod lock;

use crate::config::InstanceSpec;
use crate::options::Options;
use abi::{DispatchResult, InitRequest, OptionsJson};
use anyhow::{Context, Result, bail};
use library::Library;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Group names the host serves itself. A plugin claiming one of these would
/// shadow `resources.api` or `resources.db`, so it is refused at startup.
/// `connection` is the built-in step word for the `db` group (`I use "<conn>"
/// connection`) — a plugin claiming it would build a group-switch step that
/// exactly overlaps the built-in one, so it is refused for the same reason.
const HOST_GROUPS: &[&str] = &["api", "db", "srp", "connection"];

/// Everything about plugins that lives for the whole run. Shared through
/// `RunContext`, so every field is `Send + Sync`: handles are `u64`, and the
/// only mutable state sits behind a `Mutex`.
#[derive(Debug)]
pub struct Plugins {
    libs: Vec<Library>,
    /// group -> index into `libs`
    groups: BTreeMap<String, usize>,
    /// (group, instance) -> declared config and resolved options
    declared: BTreeMap<(String, String), InstanceSpec>,
    /// group -> default instance name
    defaults: BTreeMap<String, String>,
    /// (group, instance) -> handle, for `shared` instances only. This is the
    /// lookup that makes a shared instance shared; a `per_worker` instance is
    /// never in here, because it belongs to one file.
    shared: Mutex<BTreeMap<(String, String), u64>>,
    /// (lib index, handle) -> (group, instance) for EVERY live instance,
    /// shared and per-file alike. A handle is unique within one plugin but not
    /// across plugins, so the key must carry the library index. This is what
    /// `shutdown` sweeps, including handles a panicking file never dropped.
    registry: Mutex<BTreeMap<(usize, u64), (String, String)>>,
    artifacts_root: PathBuf,
    artifacts_counter: AtomicUsize,
}

/// Shared across worker tasks behind an `Arc`, so losing either half has to
/// fail here rather than as a confusing trait error in the runner.
const _: fn() = || {
    fn both<T: Send + Sync>() {}
    both::<Plugins>();
};

impl Plugins {
    /// `groups_in_config` is every `resources.<group>` key the host does not
    /// serve itself. A group nothing claims is a startup error; a loaded plugin
    /// whose group is absent from the config is not — it is simply never used.
    ///
    /// `concurrency` is the run's worker count, needed here because a `shared`
    /// plugin that keeps per-scenario state is only loadable sequentially —
    /// see `library::check_reset_scenario`.
    ///
    /// `global` is the run's resolved options: a group with no config section
    /// gets its instance synthesized HERE, from the manifest's
    /// `implicit_instance`, and that instance inherits the global options the
    /// same way a declared one with no `options` key of its own does.
    pub fn load(
        entries: Vec<lock::LockEntry>,
        instances: &[InstanceSpec],
        groups_in_config: &[String],
        concurrency: usize,
        global: &Options,
    ) -> Result<Self> {
        let mut libs: Vec<Library> = Vec::with_capacity(entries.len());
        let mut groups: BTreeMap<String, usize> = BTreeMap::new();
        for entry in entries {
            let lib = Library::load(&entry.name, &entry.path)?;
            library::check_reset_scenario(
                &lib.name,
                lib.manifest.concurrency,
                lib.has_reset_scenario(),
                concurrency,
            )?;
            let index = libs.len();
            check_host_groups(&lib.name, &lib.manifest.groups)?;
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

        for group in groups_in_config {
            if !groups.contains_key(group) {
                bail!(
                    "the config declares resources.{group}, but no installed plugin serves the group {group:?}"
                );
            }
        }

        let mut declared = BTreeMap::new();
        for spec in instances {
            declared.insert((spec.group.clone(), spec.name.clone()), spec.clone());
        }

        // A group the config never mentions, whose plugin declares what an
        // instance takes by default, gets one instance named `default`. It is
        // the group's only instance, hence its default — set here because the
        // host's own `default_<group>` resolution walks the config's groups
        // and never sees this one. Any declared section, an empty map
        // included, wins: the suite named its instances, so it gets those.
        let mut defaults = BTreeMap::new();
        let mut implicit: BTreeMap<String, String> = BTreeMap::new();
        for lib in &libs {
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

        let plugins = Self {
            libs,
            groups,
            declared,
            defaults,
            shared: Mutex::new(BTreeMap::new()),
            registry: Mutex::new(BTreeMap::new()),
            artifacts_root: std::env::temp_dir(),
            artifacts_counter: AtomicUsize::new(0),
        };

        // Eager, cheap, no connections opened: a typo in the config must exit 2
        // before the first request, which lazy init alone would not give. A
        // synthesized instance goes through the same check: a body its own
        // plugin refuses is a plugin bug, and load time is when to find out.
        //
        // The group is looked up, never indexed: `groups_in_config` is the
        // caller's list of config keys, and an instance whose group is missing
        // from it must produce this error rather than an index panic.
        for spec in plugins.declared.values() {
            // Where the body came from — the config has no `resources.<group>`
            // line to send the reader to for a synthesized one.
            let origin = match implicit.get(&spec.group) {
                Some(plugin) => format!("plugin {plugin:?}: implicit_instance.{}", spec.group),
                None => format!("resources.{}.{}", spec.group, spec.name),
            };
            let lib = plugins
                .groups
                .get(&spec.group)
                .map(|index| &plugins.libs[*index])
                .with_context(|| {
                    format!(
                        "{origin}: no installed plugin serves the group {:?}",
                        spec.group
                    )
                })?;
            let request = plugins.init_request(spec)?;
            lib.validate_config(&request)?
                .map_err(|error| anyhow::anyhow!("{origin}: {error}"))?;
        }
        Ok(plugins)
    }

    /// Where per-dispatch artifact directories are rooted. Set once from the
    /// run id so two runs never collide.
    pub fn set_artifacts_root(&mut self, root: PathBuf) {
        self.artifacts_root = root;
    }

    /// Adds the defaults the config resolved for its own groups. Extends
    /// rather than replaces: a group with no section already has its default
    /// from `load`, and the config, which never saw that group, must not
    /// clear it.
    pub fn add_defaults(&mut self, defaults: BTreeMap<String, String>) {
        self.defaults.extend(defaults);
    }

    pub fn defaults(&self) -> &BTreeMap<String, String> {
        &self.defaults
    }

    pub fn is_empty(&self) -> bool {
        self.libs.is_empty()
    }

    pub fn group_names(&self) -> Vec<String> {
        self.groups.keys().cloned().collect()
    }

    /// What a group's `resources.<group>.<instance>` body takes, as the plugin
    /// that serves it describes it. `None` for a group nothing serves and for
    /// one whose manifest describes nothing — the two are told apart by
    /// `group_names`, because "no plugin serves this" is the caller's error and
    /// "this plugin describes nothing" is not an error at all.
    pub fn fields_for(&self, group: &str) -> Option<&[abi::ConfigField]> {
        let lib = &self.libs[*self.groups.get(group)?];
        lib.manifest.fields.get(group).map(Vec::as_slice)
    }

    /// Every `(group, instance)` the config declared.
    pub fn declared_instances(&self) -> Vec<(String, String)> {
        self.declared.keys().cloned().collect()
    }

    /// The live half of the config contract: does this instance's config reach
    /// anything. `None` means the plugin exports no `bddkit_probe_config`,
    /// which is "not available" and never a failure — a check that never ran
    /// has proved nothing either way. So is a group nothing serves, which
    /// `load` has already refused for a declared instance. Everything else
    /// that goes wrong (an undeclared instance, an encode failure, a malformed
    /// reply) comes back as `Err`, because to the caller they are all "this
    /// instance could not be probed".
    ///
    /// Blocking, like `call_step`: FFI happens here.
    pub fn probe_config(&self, group: &str, instance: &str) -> Option<Result<(), String>> {
        let lib = &self.libs[*self.groups.get(group)?];
        if !lib.has_probe_config() {
            return None;
        }
        let spec = match self.declared.get(&(group.to_string(), instance.to_string())) {
            Some(spec) => spec,
            None => return Some(Err(undeclared(group, instance))),
        };
        let request = match self.init_request(spec) {
            Ok(request) => request,
            Err(error) => return Some(Err(format!("{error:#}"))),
        };
        Some(match lib.probe_config(&request)? {
            Ok(result) => result,
            Err(error) => Err(format!("{error:#}")),
        })
    }

    /// `(lib index, step index, pattern, is assertion)` for registry loading.
    pub fn steps(&self) -> Vec<(usize, usize, String, bool)> {
        let mut out = Vec::new();
        for (lib_index, lib) in self.libs.iter().enumerate() {
            for (step_index, step) in lib.steps.iter().enumerate() {
                out.push((
                    lib_index,
                    step_index,
                    step.pattern.clone(),
                    step.is_assertion(),
                ));
            }
        }
        out
    }

    /// `(group, pattern, is assertion, description)` for `bddkit steps list`.
    /// Separate from `steps()`, which feeds the registry and needs indices
    /// instead of the help text.
    pub fn described_steps(&self) -> Vec<(String, String, bool, Option<String>)> {
        self.libs
            .iter()
            .flat_map(|lib| {
                lib.steps.iter().map(|step| {
                    (
                        step.group.clone(),
                        step.pattern.clone(),
                        step.is_assertion(),
                        step.description.clone(),
                    )
                })
            })
            .collect()
    }

    #[cfg(test)]
    pub fn step_count(&self) -> usize {
        self.libs.iter().map(|l| l.steps.len()).sum()
    }

    pub fn group_of_step(&self, lib: usize, step: usize) -> &str {
        &self.libs[lib].steps[step].group
    }

    pub fn is_declared(&self, group: &str, instance: &str) -> bool {
        self.declared
            .contains_key(&(group.to_string(), instance.to_string()))
    }

    pub fn options_for(&self, group: &str, instance: &str) -> Result<&Options, String> {
        self.declared
            .get(&(group.to_string(), instance.to_string()))
            .map(|spec| &spec.options)
            .ok_or_else(|| undeclared(group, instance))
    }

    /// A fresh directory path per dispatch, from a process-global counter:
    /// two workers handed the same path would overwrite each other's evidence.
    /// The host does not create it — a plugin that writes calls `create_dir_all`
    /// first, and most steps never write anything.
    pub fn next_artifacts_dir(&self) -> String {
        let index = self.artifacts_counter.fetch_add(1, Ordering::Relaxed);
        self.artifacts_root
            .join(format!("{index:06}"))
            .display()
            .to_string()
    }

    /// The one blocking entry point. Everything FFI happens here, so the caller
    /// wraps exactly this in `spawn_blocking`.
    ///
    /// `existing` is the handle the caller already holds for this instance, if
    /// any. Passing it keeps a dispatch to ONE blocking hop: resolving the
    /// handle in the async context and then dispatching would cost two on the
    /// first plugin step of every file. The handle comes back so the caller can
    /// record it.
    pub fn call_step(
        &self,
        group: &str,
        instance: &str,
        lib: usize,
        step: usize,
        request: &str,
        existing: Option<u64>,
    ) -> Result<(u64, DispatchResult), String> {
        // Computed BEFORE the handle is resolved: afterwards `existing.is_none()`
        // no longer says whether THIS call is the one that created the instance.
        let created = existing.is_none() && self.is_per_worker(lib);
        let handle = match existing {
            Some(handle) => handle,
            None if self.is_per_worker(lib) => self.create_instance(group, instance, lib)?,
            None => self.shared_handle(group, instance, lib)?,
        };
        match self.libs[lib].dispatch(handle, step as u32, request) {
            Ok(result) => Ok((handle, result)),
            Err(error) => {
                // A handle the caller never received is a handle nothing drops
                // until shutdown — and the next step in this file, finding no
                // cached handle, would create a second one. A `shared` handle
                // is already in `shared` by now, so it survives the failure and
                // is reused; only a per-file one is this call's to undo.
                if created {
                    self.drop_instances(&[(lib, handle)]);
                }
                Err(format!("{error:#}"))
            }
        }
    }

    /// Whether this plugin's instances belong to one file rather than to the
    /// whole run. The manifest declares what the plugin cannot tolerate — "do
    /// not call one handle from two workers at once" — and a per-file instance
    /// satisfies that strictly, since a file runs on one worker and its
    /// scenarios are sequential.
    pub fn is_per_worker(&self, lib: usize) -> bool {
        self.libs[lib].manifest.concurrency == abi::Concurrency::PerWorker
    }

    /// Creates an instance unconditionally. Used for `per_worker`, where every
    /// file gets its own and there is nothing to look up.
    fn create_instance(&self, group: &str, instance: &str, lib: usize) -> Result<u64, String> {
        let key = (group.to_string(), instance.to_string());
        let spec = self
            .declared
            .get(&key)
            .ok_or_else(|| undeclared(group, instance))?;
        let request = self.init_request(spec).map_err(|error| format!("{error:#}"))?;
        let created = self.libs[lib]
            .init_instance(&request)
            .map_err(|error| format!("{error:#}"))?
            .map_err(|error| format!("resources.{group}.{instance}: {error}"))?;
        self.register(lib, created, group, instance);
        Ok(created)
    }

    /// Lazily creates the one instance a `shared` plugin has for the whole run.
    /// The `Mutex` is never held across `init_instance`: two workers may both
    /// build one, and the loser drops its own rather than serialising every
    /// worker behind a lock held across a network round trip.
    fn shared_handle(&self, group: &str, instance: &str, lib: usize) -> Result<u64, String> {
        let key = (group.to_string(), instance.to_string());
        {
            let shared = self.shared.lock().expect("plugin instances");
            if let Some(handle) = shared.get(&key) {
                return Ok(*handle);
            }
        }
        let created = self.create_instance(group, instance, lib)?;

        let mut shared = self.shared.lock().expect("plugin instances");
        // Another worker got there first: drop the loser's own instance, off
        // the lock, and answer with the winner's handle either way.
        if let Some(winner) = shared.get(&key).copied() {
            drop(shared);
            self.drop_instances(&[(lib, created)]);
            return Ok(winner);
        }
        shared.insert(key, created);
        Ok(created)
    }

    /// Records a live instance. Called on every successful `init_instance`,
    /// whatever its ownership.
    fn register(&self, lib: usize, handle: u64, group: &str, instance: &str) {
        self.registry
            .lock()
            .expect("plugin registry")
            .insert((lib, handle), (group.to_string(), instance.to_string()));
    }

    /// Drops instances a file owned. Unknown handles are ignored: `run_file`
    /// calls this on every exit path and must not care whether something else
    /// already swept them.
    pub fn drop_instances(&self, handles: &[(usize, u64)]) {
        for (lib, handle) in handles {
            let named = self
                .registry
                .lock()
                .expect("plugin registry")
                .remove(&(*lib, *handle));
            let Some((group, instance)) = named else {
                continue;
            };
            // A cleanup failure is a warning, never a scenario failure: the
            // file's result is already decided, and turning a failed drop into
            // a test failure would misreport the system under test.
            match self.libs[*lib].drop_instance(*handle) {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    eprintln!("warning: dropping {group}.{instance} failed: {error}")
                }
                Err(error) => {
                    eprintln!("warning: dropping {group}.{instance} failed: {error:#}")
                }
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn registered_count(&self) -> usize {
        self.registry.lock().expect("plugin registry").len()
    }

    /// Resets exactly the instances the caller names, and nothing else.
    ///
    /// This is what stops the reset being a broadcast. It used to walk every
    /// live instance, so one worker's scenario boundary cleared state another
    /// worker was mid-scenario in, and a failed reset was attributed to
    /// whichever file reached a boundary first — including a file with no
    /// plugin steps at all. The caller now passes only what its own file
    /// touched, so neither can happen.
    ///
    /// The first failure wins and every remaining instance is still attempted:
    /// leaving some unreset because an earlier one refused would be worse.
    pub fn reset_instances(&self, handles: &[(usize, u64)]) -> Result<(), String> {
        let mut failure = None;
        for (lib, handle) in handles {
            let named = self
                .registry
                .lock()
                .expect("plugin registry")
                .get(&(*lib, *handle))
                .cloned();
            let Some((group, instance)) = named else {
                continue;
            };
            let error = match self.libs[*lib].reset_scenario(*handle) {
                Ok(Ok(())) => continue,
                // The plugin answered, and answered that it failed.
                Ok(Err(error)) => error,
                // The call itself failed: a malformed reply, a NUL byte.
                Err(error) => format!("{error:#}"),
            };
            failure.get_or_insert(format!("resetting {group}.{instance} failed: {error}"));
        }
        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Every instance that was created is dropped, including after a failed or
    /// `--fail-fast` run: a plugin's external process must not outlive the run.
    /// That includes handles a file owned but never dropped, because it
    /// panicked and lost its `World` — which is what makes a panicking file
    /// safe rather than a leak.
    ///
    /// Call it once, after every worker has finished. It takes `&self`, so the
    /// registry stays usable afterwards, but a `call_step` after this point
    /// creates an instance nothing will ever drop. The single call site — after
    /// the pool drains — is what prevents that; a flag would mean a check
    /// inside the lock region for a hazard that cannot arise.
    pub fn shutdown(&self) {
        // The keys are copied out, not taken: `drop_instances` removes each one
        // itself and needs the names still there to say what it failed to drop.
        // That makes the sweep a sequence of per-handle removals rather than one
        // atomic take — not observable, because the worker pool has drained by
        // the time this runs and nothing else touches the registry.
        let live: Vec<(usize, u64)> = {
            let registry = self.registry.lock().expect("plugin registry");
            registry.keys().copied().collect()
        };
        self.shared.lock().expect("plugin instances").clear();
        self.drop_instances(&live);
    }

    fn init_request(&self, spec: &InstanceSpec) -> Result<String> {
        serde_json::to_string(&InitRequest {
            group: &spec.group,
            instance: &spec.name,
            config: &spec.config,
            options: OptionsJson::from(&spec.options),
        })
        .with_context(|| format!("failed to encode resources.{}.{}", spec.group, spec.name))
    }
}

/// A plugin claiming a group the host serves itself would shadow
/// `resources.api` or `resources.db`. A free function over the parsed manifest's
/// group list, for the same reason as `library`'s own checks: reachable from a
/// unit test without building a second cdylib.
fn check_host_groups(name: &str, groups: &[String]) -> Result<()> {
    for group in groups {
        if HOST_GROUPS.contains(&group.as_str()) {
            bail!("plugin {name:?} claims the group {group:?}, which bddkit serves itself");
        }
    }
    Ok(())
}

fn undeclared(group: &str, instance: &str) -> String {
    format!("instance {instance:?} is not declared in resources.{group}")
}

/// The scenario's current instance per group. Selection is per group on
/// purpose: switching the browser must not disturb the selected widget instance.
pub struct PluginState {
    plugins: Option<Arc<Plugins>>,
    defaults: BTreeMap<String, String>,
    current: BTreeMap<String, String>,
    /// (group, instance) -> (lib index, handle) for every instance this file
    /// has touched. Doubles as the per-file handle cache for `per_worker`
    /// lookups and as the set to reset at a scenario boundary.
    used: BTreeMap<(String, String), (usize, u64)>,
    /// The subset of `used` this file created and must drop when it ends. A
    /// `shared` handle appears in `used` but never here — it outlives the file
    /// and is swept by `Plugins::shutdown`.
    owned: Vec<(usize, u64)>,
}

impl PluginState {
    pub fn new(plugins: Option<Arc<Plugins>>) -> Self {
        let defaults = plugins
            .as_ref()
            .map(|p| p.defaults().clone())
            .unwrap_or_default();
        Self {
            plugins,
            current: defaults.clone(),
            defaults,
            used: BTreeMap::new(),
            owned: Vec::new(),
        }
    }

    /// `PluginState::new` already takes its defaults from `Plugins::defaults()`;
    /// nothing in production sets them again. Kept for tests that need a
    /// `PluginState` without loading a real `Plugins`.
    #[cfg(test)]
    pub fn set_defaults(&mut self, defaults: BTreeMap<String, String>) {
        self.current = defaults.clone();
        self.defaults = defaults;
    }

    pub fn plugins(&self) -> Option<&Arc<Plugins>> {
        self.plugins.as_ref()
    }

    pub fn current(&self, group: &str) -> Result<&str, String> {
        self.current.get(group).map(String::as_str).ok_or_else(|| {
            format!(
                "no instance of the resource group {group:?} is selected, and no default_{group} is set"
            )
        })
    }

    pub fn use_instance(&mut self, group: &str, name: &str) -> Result<(), String> {
        let declared = self
            .plugins
            .as_ref()
            .is_some_and(|p| p.is_declared(group, name));
        if !declared {
            return Err(undeclared(group, name));
        }
        self.current.insert(group.to_string(), name.to_string());
        Ok(())
    }

    #[cfg(test)]
    pub fn use_instance_unchecked(&mut self, group: &str, name: &str) {
        self.current.insert(group.to_string(), name.to_string());
    }

    /// The handle this file already holds for an instance, if any.
    pub fn handle_for(&self, group: &str, instance: &str) -> Option<(usize, u64)> {
        self.used
            .get(&(group.to_string(), instance.to_string()))
            .copied()
    }

    /// Records an instance this file has used. `owned` is true when the file
    /// created it and must therefore drop it — that is, when the plugin
    /// declares `per_worker`.
    pub fn record(&mut self, group: &str, instance: &str, lib: usize, handle: u64, owned: bool) {
        let key = (group.to_string(), instance.to_string());
        // A second record for one instance is always the same handle: the cache
        // is consulted before every dispatch, and a dispatch that fails after
        // creating a handle drops it rather than leaving a second one behind.
        if self.used.insert(key, (lib, handle)).is_some() {
            return;
        }
        if owned {
            self.owned.push((lib, handle));
        }
    }

    /// Every instance this file has touched, for the scenario boundary.
    pub fn to_reset(&self) -> Vec<(usize, u64)> {
        self.used.values().copied().collect()
    }

    /// Only what this file created, for the end of the file.
    pub fn to_drop(&self) -> Vec<(usize, u64)> {
        self.owned.clone()
    }

    /// Per invariant 2 the selection returns to `default_<group>`. The handles
    /// are file state, not scenario state, so `used`/`owned` survive.
    pub fn reset(&mut self) {
        self.current = self.defaults.clone();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::config::InstanceSpec;
    use crate::options::Options;

    /// Same build as `library`'s own fixture helper; a unit test cannot reach
    /// the integration test helpers, and six lines beat a shared crate.
    pub fn fixture() -> std::path::PathBuf {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let target = root.join("target/fixture-plugin");
        let out = std::process::Command::new(env!("CARGO"))
            .args(["build", "--manifest-path"])
            .arg(root.join("tests/fixtures/echo-plugin/Cargo.toml"))
            .arg("--target-dir")
            .arg(&target)
            .output()
            .expect("cargo runs");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        target.join("debug").join(format!(
            "{}echo_plugin{}",
            std::env::consts::DLL_PREFIX,
            std::env::consts::DLL_SUFFIX
        ))
    }

    pub fn entry() -> crate::plugin::lock::LockEntry {
        serde_yaml_ng::from_str(&format!("name: echo\npath: {}\n", fixture().display()))
            .expect("entry parses")
    }

    pub fn instance(name: &str, prefix: Option<&str>) -> InstanceSpec {
        InstanceSpec {
            group: "echo".to_string(),
            name: name.to_string(),
            config: match prefix {
                Some(p) => serde_json::json!({ "prefix": p }),
                None => serde_json::json!({}),
            },
            options: Options::default(),
        }
    }

    #[test]
    fn a_group_serves_the_fields_its_manifest_declares() {
        let plugins = Plugins::load(vec![entry()], &[instance("a", Some("p-"))], &["echo".into()], 1, &Options::default())
            .expect("loads");
        let fields = plugins.fields_for("echo").expect("the fixture describes echo");
        assert_eq!(fields[0].name, "prefix");
        assert!(fields[0].required, "the fixture declares prefix required");
        assert_eq!(fields[0].example.as_deref(), Some("p-"));
        // A group nothing serves has no description, and that is not an error:
        // the caller asks `group_names` whether the group exists at all.
        assert!(plugins.fields_for("browser").is_none());
    }

    #[test]
    fn a_declared_instance_can_be_probed_live() {
        let plugins = Plugins::load(vec![entry()], &[instance("a", Some("p-"))], &["echo".into()], 1, &Options::default())
            .expect("loads");
        assert_eq!(
            plugins.declared_instances(),
            vec![("echo".to_string(), "a".to_string())]
        );
        assert_eq!(plugins.probe_config("echo", "a"), Some(Ok(())));

        // The fixture's probe answers with whatever `probe_error` names, which
        // is how a real plugin reports an endpoint that refused it.
        let mut spec = instance("b", Some("p-"));
        spec.config = serde_json::json!({"prefix": "p-", "probe_error": "bucket not found"});
        let plugins = Plugins::load(vec![entry()], &[spec], &["echo".into()], 1, &Options::default()).expect("loads");
        assert_eq!(
            plugins.probe_config("echo", "b"),
            Some(Err("bucket not found".to_string()))
        );
        // An instance nothing declared cannot be probed, and says so.
        let error = plugins
            .probe_config("echo", "ghost")
            .expect("the plugin has a probe");
        assert!(error.expect_err("undeclared").contains("ghost"));
    }

    #[test]
    fn an_implicit_instance_is_synthesized_only_when_the_group_has_no_section() {
        // The fixture's manifest declares `implicit_instance.echo`, so a config
        // with no `resources.echo` gets one instance named `default`, which is
        // the group default by the one-instance rule — before `set_defaults`
        // has run, because the host's own default resolution never sees the
        // group.
        let plugins = Plugins::load(vec![entry()], &[], &[], 1, &Options::default()).expect("loads");
        assert_eq!(
            plugins.declared_instances(),
            vec![("echo".to_string(), "default".to_string())]
        );
        assert_eq!(plugins.defaults().get("echo").map(String::as_str), Some("default"));
        assert!(plugins.is_declared("echo", "default"));
        assert_eq!(plugins.probe_config("echo", "default"), Some(Ok(())));

        // A declared section — this one non-empty — is the only source then.
        let plugins = Plugins::load(
            vec![entry()],
            &[instance("a", Some("p-"))],
            &["echo".into()],
            1,
            &Options::default(),
        )
        .expect("loads");
        assert_eq!(
            plugins.declared_instances(),
            vec![("echo".to_string(), "a".to_string())]
        );
        assert!(plugins.defaults().is_empty(), "a declared group's default is the config's to resolve");
    }

    #[test]
    fn loads_a_plugin_and_maps_its_group() {
        let plugins = Plugins::load(vec![entry()], &[instance("a", Some("p-"))], &["echo".into()], 1, &Options::default())
            .expect("loads");
        assert_eq!(plugins.step_count(), 3);
        assert_eq!(plugins.group_of_step(0, 0), "echo");
    }

    #[test]
    fn a_config_group_with_no_plugin_is_a_startup_error() {
        let error = Plugins::load(Vec::new(), &[instance("a", Some("p-"))], &["echo".into()], 1, &Options::default())
            .expect_err("nothing claims the group");
        assert!(format!("{error:#}").contains("echo"), "{error:#}");
    }

    #[test]
    fn an_instance_in_an_unserved_group_is_an_error_not_a_panic() {
        // groups_in_config is the caller's list of config keys; the eager
        // validation loop must not index its way into a panic when an instance
        // names a group that list left out.
        let error = Plugins::load(Vec::new(), &[instance("a", Some("p-"))], &[], 1, &Options::default())
            .expect_err("nothing serves the group");
        assert!(format!("{error:#}").contains("echo"), "{error:#}");
    }

    #[test]
    fn a_declared_instance_is_validated_eagerly() {
        // A typo must exit before the first request, without opening anything:
        // that is what validate_config buys over lazy init alone.
        let error = Plugins::load(vec![entry()], &[instance("a", None)], &["echo".into()], 1, &Options::default())
            .expect_err("prefix missing");
        let text = format!("{error:#}");
        assert!(text.contains("prefix"), "{text}");
        assert!(text.contains("resources.echo.a"), "{text}");
    }

    #[test]
    fn an_instance_is_created_only_on_first_use() {
        let plugins = Plugins::load(vec![entry()], &[instance("a", Some("p-"))], &["echo".into()], 1, &Options::default())
            .expect("loads");
        assert_eq!(
            plugins.registered_count(),
            0,
            "loading must not create instances"
        );
        let (_, result) = plugins
            .call_step("echo", "a", 0, 0, r#"{"args":["x","name"],"debug":false}"#, None)
            .expect("dispatch");
        assert_eq!(result.status, abi::Status::Passed);
        assert_eq!(plugins.registered_count(), 1);
    }

    #[test]
    fn a_second_call_reuses_the_same_instance() {
        let plugins = Plugins::load(vec![entry()], &[instance("a", Some("p-"))], &["echo".into()], 1, &Options::default())
            .expect("loads");
        let request = r#"{"args":["3"],"debug":false}"#;
        let (_, first) = plugins
            .call_step("echo", "a", 0, 1, request, None)
            .expect("dispatch");
        let (_, second) = plugins
            .call_step("echo", "a", 0, 1, request, None)
            .expect("dispatch");
        assert_eq!(first.status, abi::Status::NotYet);
        assert_eq!(second.status, abi::Status::NotYet);
        assert!(
            second.error.unwrap_or_default().contains("2 of 3"),
            "the counter must survive between dispatches, i.e. one instance"
        );
        assert_eq!(plugins.registered_count(), 1);
    }

    #[test]
    fn an_undeclared_instance_is_an_error_naming_the_group() {
        let plugins = Plugins::load(vec![entry()], &[instance("a", Some("p-"))], &["echo".into()], 1, &Options::default())
            .expect("loads");
        let error = plugins
            .call_step("echo", "ghost", 0, 0, r#"{"args":[],"debug":false}"#, None)
            .expect_err("not declared");
        assert!(error.contains("ghost") && error.contains("echo"), "{error}");
    }

    #[test]
    fn shutdown_sweeps_what_a_file_never_dropped() {
        // A panicking file loses its World and with it the handles it owned.
        // The registry is what keeps the invariant "no instance outlives the
        // run" resting on one mechanism instead of on discipline.
        let plugins = Plugins::load(vec![entry()], &[instance("a", Some("p-"))], &["echo".into()], 1, &Options::default())
            .expect("loads");
        plugins
            .call_step("echo", "a", 0, 0, r#"{"args":["x","n"],"debug":false}"#, None)
            .expect("dispatch");
        assert_eq!(plugins.registered_count(), 1);
        plugins.shutdown();
        assert_eq!(plugins.registered_count(), 0);
    }

    #[test]
    fn a_shared_instance_survives_a_failed_dispatch() {
        // The other half of "only a handle this call created is undone": a NUL
        // byte in the payload fails the dispatch before the FFI call, and the
        // shared instance behind it must still be there for the next step.
        let plugins = Plugins::load(vec![entry()], &[instance("a", Some("p-"))], &["echo".into()], 1, &Options::default())
            .expect("loads");
        // No handle passed in, so this call is the one that creates the
        // instance — the case where the undo applies at all.
        let error = plugins
            .call_step("echo", "a", 0, 1, "\u{0}", None)
            .expect_err("a NUL byte cannot cross the boundary");
        assert!(error.contains("NUL byte"), "{error}");
        assert_eq!(
            plugins.registered_count(),
            1,
            "a shared instance is not this call's to drop"
        );
        let request = r#"{"args":["2"],"debug":false}"#;
        plugins
            .call_step("echo", "a", 0, 1, request, None)
            .expect("dispatch");
        let (_, after) = plugins
            .call_step("echo", "a", 0, 1, request, None)
            .expect("dispatch");
        assert_eq!(
            after.status,
            abi::Status::Passed,
            "the counter reached 2, so both calls found the same instance"
        );
    }

    #[test]
    fn a_reset_reaches_a_live_instance_and_an_unknown_handle_is_ignored() {
        let plugins = Plugins::load(vec![entry()], &[instance("a", Some("p-"))], &["echo".into()], 1, &Options::default())
            .expect("loads");
        // Nothing initialised yet: this must be a no-op, not a failure.
        plugins.reset_instances(&[(0, 1)]).expect("nothing to reset");
        let request = r#"{"args":["2"],"debug":false}"#;
        let (handle, _) = plugins
            .call_step("echo", "a", 0, 1, request, None)
            .expect("dispatch");
        plugins
            .reset_instances(&[(0, handle)])
            .expect("the fixture resets");
        let (_, after) = plugins
            .call_step("echo", "a", 0, 1, request, Some(handle))
            .expect("dispatch");
        assert_eq!(
            after.status,
            abi::Status::NotYet,
            "the counter restarted, so attempt 1 of 2 is not there yet"
        );
    }

    /// The fixture refuses its reset when its config says so — the same switch
    /// `tests/plugin.rs` uses to reach the reset-failure path.
    fn instance_failing_reset(name: &str) -> InstanceSpec {
        InstanceSpec {
            group: "echo".to_string(),
            name: name.to_string(),
            config: serde_json::json!({ "prefix": "p-", "fail_reset": true }),
            options: Options::default(),
        }
    }

    #[test]
    fn resetting_an_empty_list_touches_nothing() {
        // A file with no plugin steps holds no handles, so its scenario
        // boundaries must not reach an instance ANOTHER file is using. The live
        // instance below is what makes this fail if the reset ever goes back to
        // broadcasting over everything and ignoring its argument.
        let plugins = Plugins::load(vec![entry()], &[instance("a", Some("p-"))], &["echo".into()], 1, &Options::default())
            .expect("loads");
        let request = r#"{"args":["2"],"debug":false}"#;
        let (handle, _) = plugins
            .call_step("echo", "a", 0, 1, request, None)
            .expect("dispatch");
        plugins.reset_instances(&[]).expect("nothing to reset");
        let (_, after) = plugins
            .call_step("echo", "a", 0, 1, request, Some(handle))
            .expect("dispatch");
        assert_eq!(
            after.status,
            abi::Status::Passed,
            "the counter reached 2, so nothing reset it"
        );
    }

    #[test]
    fn resetting_reaches_only_the_listed_instances() {
        let plugins = Plugins::load(
            vec![entry()],
            &[instance("a", Some("p-")), instance("b", Some("o-"))],
            &["echo".into()],
            1,
            &Options::default(),
        )
        .expect("loads");
        let request = r#"{"args":["2"],"debug":false}"#;
        let (a, _) = plugins
            .call_step("echo", "a", 0, 1, request, None)
            .expect("dispatch");
        let (b, _) = plugins
            .call_step("echo", "b", 0, 1, request, None)
            .expect("dispatch");

        // Reset only `a`. `b`'s counter must survive, which is the whole point:
        // one file's scenario boundary must not clear another's instance.
        plugins.reset_instances(&[(0, a)]).expect("reset a");

        let (_, after_a) = plugins
            .call_step("echo", "a", 0, 1, request, Some(a))
            .expect("dispatch");
        assert_eq!(
            after_a.status,
            abi::Status::NotYet,
            "a restarted at attempt 1 of 2"
        );
        let (_, after_b) = plugins
            .call_step("echo", "b", 0, 1, request, Some(b))
            .expect("dispatch");
        assert_eq!(
            after_b.status,
            abi::Status::Passed,
            "b kept its count and reached 2"
        );
    }

    #[test]
    fn a_reset_the_plugin_refuses_names_the_instance() {
        let plugins = Plugins::load(
            vec![entry()],
            &[instance_failing_reset("bad")],
            &["echo".into()],
            1,
            &Options::default(),
        )
        .expect("loads");
        let (handle, _) = plugins
            .call_step("echo", "bad", 0, 0, r#"{"args":["x","n"],"debug":false}"#, None)
            .expect("dispatch");
        let error = plugins.reset_instances(&[(0, handle)]).expect_err("refused");
        assert!(error.contains("echo.bad"), "{error}");
        assert!(error.contains("refuses to reset"), "{error}");
    }

    #[test]
    fn every_created_handle_is_registered_for_sweeping() {
        let plugins = Plugins::load(vec![entry()], &[instance("a", Some("p-"))], &["echo".into()], 1, &Options::default())
            .expect("loads");
        assert_eq!(plugins.registered_count(), 0, "loading creates nothing");
        let (handle, _) = plugins
            .call_step("echo", "a", 0, 0, r#"{"args":["x","name"],"debug":false}"#, None)
            .expect("dispatch");
        assert_eq!(plugins.registered_count(), 1);
        plugins.drop_instances(&[(0, handle)]);
        assert_eq!(plugins.registered_count(), 0, "dropping deregisters");
    }

    #[test]
    fn dropping_an_unregistered_handle_is_a_no_op() {
        // `run_file` drops its owned handles on every exit path, including
        // after `shutdown` has already run in a torn-down test.
        let plugins = Plugins::load(vec![entry()], &[instance("a", Some("p-"))], &["echo".into()], 1, &Options::default())
            .expect("loads");
        plugins.drop_instances(&[(0, 999)]);
        assert_eq!(plugins.registered_count(), 0);
    }

    #[test]
    fn a_shared_instance_is_reused_across_calls() {
        let plugins = Plugins::load(vec![entry()], &[instance("a", Some("p-"))], &["echo".into()], 1, &Options::default())
            .expect("loads");
        let request = r#"{"args":["3"],"debug":false}"#;
        let (first, _) = plugins
            .call_step("echo", "a", 0, 1, request, None)
            .expect("dispatch");
        let (second, result) = plugins
            .call_step("echo", "a", 0, 1, request, None)
            .expect("dispatch");
        assert_eq!(first, second, "a shared plugin answers with one instance");
        assert!(
            result.error.unwrap_or_default().contains("2 of 3"),
            "the counter survived, so it is the same instance"
        );
        assert_eq!(plugins.registered_count(), 1);
    }

    #[test]
    fn a_supplied_handle_is_used_without_creating_another() {
        // Partial: the echo fixture is `shared`, so a `call_step` that ignored
        // `existing` entirely would still find the one shared handle and pass
        // this. What closes it is the `per_worker` fixture, out of process, in
        // `tests/plugin.rs::two_files_get_different_per_worker_instances`.
        let plugins = Plugins::load(vec![entry()], &[instance("a", Some("p-"))], &["echo".into()], 1, &Options::default())
            .expect("loads");
        let request = r#"{"args":["3"],"debug":false}"#;
        let (handle, _) = plugins
            .call_step("echo", "a", 0, 1, request, None)
            .expect("dispatch");
        let (same, result) = plugins
            .call_step("echo", "a", 0, 1, request, Some(handle))
            .expect("dispatch");
        assert_eq!(same, handle);
        assert!(result.error.unwrap_or_default().contains("2 of 3"));
        assert_eq!(plugins.registered_count(), 1, "no second instance");
    }

    #[test]
    fn the_fixture_is_not_per_worker() {
        let plugins = Plugins::load(vec![entry()], &[instance("a", Some("p-"))], &["echo".into()], 1, &Options::default())
            .expect("loads");
        assert!(!plugins.is_per_worker(0));
    }

    #[test]
    fn a_recorded_handle_comes_back_for_the_same_instance() {
        let mut state = PluginState::new(None);
        assert_eq!(state.handle_for("echo", "a"), None);
        state.record("echo", "a", 0, 7, true);
        assert_eq!(state.handle_for("echo", "a"), Some((0, 7)));
        assert_eq!(
            state.handle_for("echo", "b"),
            None,
            "another instance is another handle"
        );
    }

    #[test]
    fn every_touched_instance_is_reset_but_only_owned_ones_are_dropped() {
        // A shared instance is borrowed: the file resets it (at concurrency 1,
        // where a shared plugin may export a reset at all) but must not drop
        // it, because it outlives the file.
        let mut state = PluginState::new(None);
        state.record("echo", "owned", 0, 1, true);
        state.record("echo", "borrowed", 0, 2, false);
        // Sorted: `used` is keyed by (group, instance), so its iteration order
        // is the instance names', which is not what this test is about.
        let mut reset = state.to_reset();
        reset.sort();
        assert_eq!(reset, vec![(0, 1), (0, 2)]);
        assert_eq!(state.to_drop(), vec![(0, 1)]);
    }

    #[test]
    fn recording_the_same_instance_twice_does_not_double_it() {
        // Every dispatch records; the second one must not queue a second drop
        // of a handle that is already owned.
        let mut state = PluginState::new(None);
        state.record("echo", "a", 0, 1, true);
        state.record("echo", "a", 0, 1, true);
        assert_eq!(state.to_drop(), vec![(0, 1)]);
        assert_eq!(state.to_reset(), vec![(0, 1)]);
    }

    #[test]
    fn the_selection_resets_per_scenario_but_the_handles_do_not() {
        // Invariant 2 applies to which instance is SELECTED. The instance
        // itself lives for the file: recreating a browser at every scenario
        // boundary is exactly what per-file ownership exists to avoid.
        let mut state = PluginState::new(None);
        state.set_defaults([("echo".to_string(), "a".to_string())].into_iter().collect());
        state.use_instance_unchecked("echo", "b");
        state.record("echo", "b", 0, 1, true);
        state.reset();
        assert_eq!(state.current("echo").expect("selected"), "a");
        assert_eq!(
            state.to_drop(),
            vec![(0, 1)],
            "the handle survives the scenario boundary"
        );
    }

    #[test]
    fn a_plugin_claiming_a_host_group_is_refused() {
        // docs/plugin-authoring.md promises these four names are reserved;
        // nothing pinned that promise before this test.
        for group in ["api", "db", "srp", "connection"] {
            let error = check_host_groups("intruder", &[group.to_string()])
                .unwrap_err()
                .to_string();
            assert!(error.contains(group), "{error}");
            assert!(error.contains("intruder"), "{error}");
        }
        check_host_groups("echo", &["echo".to_string()]).expect("its own group is fine");
    }

    #[test]
    fn the_fixture_plugin_is_refused_under_parallelism() {
        // It exports bddkit_reset_scenario, and a `shared` instance cannot hold
        // per-scenario state while every worker shares it.
        let error =
            Plugins::load(vec![entry()], &[instance("a", Some("p-"))], &["echo".into()], 2, &Options::default())
                .expect_err("refused");
        let text = format!("{error:#}");
        assert!(text.contains("echo"), "{text}");
        assert!(text.contains("concurrency: 1"), "{text}");
    }

    #[test]
    fn each_dispatch_gets_its_own_artifacts_dir() {
        let plugins = Plugins::load(vec![entry()], &[instance("a", Some("p-"))], &["echo".into()], 1, &Options::default())
            .expect("loads");
        let first = plugins.next_artifacts_dir();
        let second = plugins.next_artifacts_dir();
        assert_ne!(first, second, "two workers must never share an artifact path");
    }

    #[test]
    fn switching_to_a_declared_instance_selects_it_and_an_undeclared_one_is_an_error() {
        let plugins = Plugins::load(vec![entry()], &[instance("a", Some("p-"))], &["echo".into()], 1, &Options::default())
            .expect("loads");
        let mut state = PluginState::new(Some(Arc::new(plugins)));
        state.use_instance("echo", "a").expect("declared instance");
        assert_eq!(state.current("echo").expect("selected"), "a");
        let error = state.use_instance("echo", "ghost").expect_err("undeclared");
        assert!(error.contains("ghost") && error.contains("echo"), "{error}");
        assert_eq!(
            state.current("echo").expect("selected"),
            "a",
            "a failed switch must not change the selection"
        );
    }
}
