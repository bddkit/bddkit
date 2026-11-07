//! The only module in the codebase that touches FFI. Everything above it sees
//! `String -> String` calls and typed payloads.

use super::abi::{
    ABI_MIN, ABI_VERSION, Concurrency, ConfigReply, DispatchResult, Envelope, InitFailure,
    InitResponse, Manifest, StepSpec,
};
use anyhow::{Context, Result, bail};
use std::ffi::{CStr, CString, c_char};
use std::mem::ManuallyDrop;
use std::path::Path;

type AbiVersionFn = unsafe extern "C" fn() -> u32;
type NoArgFn = unsafe extern "C" fn() -> *mut c_char;
type JsonFn = unsafe extern "C" fn(*const c_char) -> *mut c_char;
type DispatchFn = unsafe extern "C" fn(u64, u32, *const c_char) -> *mut c_char;
type HandleFn = unsafe extern "C" fn(u64) -> *mut c_char;
type FreeFn = unsafe extern "C" fn(*mut c_char);
type StepValidateFn = unsafe extern "C" fn(u32, *const c_char) -> *mut c_char;
type SelectInstanceFn = unsafe extern "C" fn(u64, *const c_char) -> *mut c_char;

/// One loaded plugin binary.
///
/// Field order is load-bearing: Rust drops fields in declaration order, and
/// every function pointer below points into `_library`'s mapping. `_library`
/// is declared last so it is unloaded last. The leading underscore says the
/// rest: the field is never read, it exists only to be dropped last.
pub struct Library {
    pub name: String,
    pub manifest: Manifest,
    pub steps: Vec<StepSpec>,
    validate_config: JsonFn,
    init_instance: JsonFn,
    dispatch: DispatchFn,
    drop_instance: HandleFn,
    reset_scenario: Option<HandleFn>,
    probe_config: Option<JsonFn>,
    validate_step: Option<StepValidateFn>,
    select_instance: Option<SelectInstanceFn>,
    free_string: FreeFn,
    _library: libloading::Library,
}

impl std::fmt::Debug for Library {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Library")
            .field("name", &self.name)
            .field("groups", &self.manifest.groups)
            .field("steps", &self.steps.len())
            .finish()
    }
}

/// A later task shares one loaded library across worker tasks behind an `Arc`,
/// so losing `Send + Sync` here has to fail at this module, not as a confusing
/// error in the runner. Auto-derived: every field is a function pointer, an
/// owned host value, or `libloading::Library` (which asserts both itself).
const _: fn() = || {
    fn both<T: Send + Sync>() {}
    both::<Library>();
};

impl Library {
    pub fn load(name: &str, path: &Path) -> Result<Self> {
        // SAFETY: dlopen runs the library's initialisers, which is inherently
        // unsafe and cannot be made safe. Installing a plugin is a trust
        // decision, stated as a non-goal in the design.
        // `ManuallyDrop` is the whole point: every `?` below leaves the mapping
        // in place instead of `dlclose`-ing it. `dlopen` has already run the
        // library's initialisers by then, so a plugin that registered an
        // `atexit` handler or a thread-local destructor would have it run
        // against an unmapped page — the same reason `load_plugins` leaks the
        // libraries at exit. A rejected plugin is a "nothing ran" exit anyway,
        // so one leaked mapping costs nothing.
        let library =
            ManuallyDrop::new(unsafe { libloading::Library::new(path) }.with_context(|| {
                format!("failed to load plugin {name:?} from {}", path.display())
            })?);

        // The version check comes before any other symbol is resolved: a
        // binary from another ABI generation may not even have the rest.
        //
        // SAFETY (every `library.get` below): resolving a symbol is unsafe
        // because nothing proves the binary's declared type matches ours —
        // that is exactly what `ABI_VERSION` is checked for. Dereferencing the
        // returned `Symbol` copies out a bare function pointer with no
        // lifetime, so its validity rests on the mapping outliving it, in both
        // of the two cases: on success `_library` is the last field of `Self`
        // and so is dropped after every pointer stored beside it; on any error
        // path below the mapping is leaked by the `ManuallyDrop` above and so
        // outlives every pointer copied out of it.
        let abi: AbiVersionFn = *unsafe { library.get(b"bddkit_abi_version\0") }
            .with_context(|| format!("plugin {name:?} exports no bddkit_abi_version"))?;
        // SAFETY: this one call is an unavoidable act of faith. Every other
        // symbol is called only after the version matched, but the version
        // itself has to be read first, so `bddkit_abi_version` is invoked as
        // `() -> u32` on nothing but the symbol's declared type. A binary that
        // exports that name with another signature is undefined behaviour and
        // no host check can catch it; the bootstrap is inherent to the design,
        // which accepts it as part of the trust decision to install a plugin.
        let reported = unsafe { abi() };
        check_abi_version(name, reported)?;

        let manifest_fn: NoArgFn = *unsafe { library.get(b"bddkit_manifest\0") }
            .with_context(|| format!("plugin {name:?} exports no bddkit_manifest"))?;
        let list_steps_fn: NoArgFn = *unsafe { library.get(b"bddkit_list_steps\0") }
            .with_context(|| format!("plugin {name:?} exports no bddkit_list_steps"))?;
        let free_string: FreeFn = *unsafe { library.get(b"bddkit_free_string\0") }
            .with_context(|| format!("plugin {name:?} exports no bddkit_free_string"))?;
        let validate_config: JsonFn = *unsafe { library.get(b"bddkit_validate_config\0") }
            .with_context(|| format!("plugin {name:?} exports no bddkit_validate_config"))?;
        let init_instance: JsonFn = *unsafe { library.get(b"bddkit_init_instance\0") }
            .with_context(|| format!("plugin {name:?} exports no bddkit_init_instance"))?;
        let dispatch: DispatchFn = *unsafe { library.get(b"bddkit_dispatch\0") }
            .with_context(|| format!("plugin {name:?} exports no bddkit_dispatch"))?;
        let drop_instance: HandleFn = *unsafe { library.get(b"bddkit_drop_instance\0") }
            .with_context(|| format!("plugin {name:?} exports no bddkit_drop_instance"))?;
        // Optional: a plugin with no per-scenario state need not export it.
        let reset_scenario: Option<HandleFn> = unsafe { library.get(b"bddkit_reset_scenario\0") }
            .ok()
            .map(|symbol| *symbol);
        // Optional in the same way: a plugin that cannot check reachability
        // simply has no live check, which is reported as "not available".
        let probe_config: Option<JsonFn> = unsafe { library.get(b"bddkit_probe_config\0") }
            .ok()
            .map(|symbol| *symbol);
        // Optional: a plugin that checks nothing before a run simply accepts
        // every step.
        let validate_step: Option<StepValidateFn> =
            unsafe { library.get(b"bddkit_validate_step\0") }
                .ok()
                .map(|symbol| *symbol);

        // Optional: a plugin that needs no notice of `I use "<name>" <group>`
        // exports nothing.
        let select_instance: Option<SelectInstanceFn> =
            unsafe { library.get(b"bddkit_select_instance\0") }
                .ok()
                .map(|symbol| *symbol);

        // SAFETY: `manifest_fn` takes no arguments, and the pointer it returns
        // is handed straight back to this same plugin's `free_string` inside
        // `take` — the host never frees plugin memory itself.
        let manifest_json = unsafe { take(free_string, manifest_fn()) }
            .with_context(|| format!("plugin {name:?} returned no manifest"))?;
        let manifest: Manifest = serde_json::from_str(&manifest_json)
            .with_context(|| format!("plugin {name:?} returned a malformed manifest"))?;
        if manifest.groups.is_empty() {
            bail!("plugin {name:?} claims no resource group");
        }
        // SAFETY: as for `manifest_fn` above.
        let steps_json = unsafe { take(free_string, list_steps_fn()) }
            .with_context(|| format!("plugin {name:?} returned no step list"))?;
        let mut steps: Vec<StepSpec> = serde_json::from_str(&steps_json)
            .with_context(|| format!("plugin {name:?} returned a malformed step list"))?;
        prepare_steps(name, &mut steps)?;
        check_step_groups(name, &manifest, &steps)?;
        check_field_groups(name, &manifest)?;
        check_field_types(name, &manifest)?;
        check_implicit_groups(name, &manifest)?;
        check_secret_groups(name, &steps)?;
        check_eager_init(name, &manifest)?;
        check_deprecated_targets(name, &manifest)?;

        Ok(Self {
            name: name.to_string(),
            manifest,
            steps,
            validate_config,
            init_instance,
            dispatch,
            drop_instance,
            reset_scenario,
            probe_config,
            validate_step,
            select_instance,
            free_string,
            _library: ManuallyDrop::into_inner(library),
        })
    }

    pub fn has_reset_scenario(&self) -> bool {
        self.reset_scenario.is_some()
    }

    pub fn has_select_instance(&self) -> bool {
        self.select_instance.is_some()
    }

    pub fn has_probe_config(&self) -> bool {
        self.probe_config.is_some()
    }

    pub fn validate_config(&self, request: &str) -> Result<Result<Option<String>, String>> {
        self.config_call(self.validate_config, request, "validate_config")
    }

    /// `None` means the plugin exports no probe at all — "not available",
    /// never a failure. `reset_scenario` can answer `Ok` when it is absent
    /// because "no per-scenario state to clear" genuinely is success; a
    /// reachability check that never ran has proved nothing, so the caller
    /// must be able to tell the two apart.
    ///
    pub fn probe_config(&self, request: &str) -> Option<Result<Result<Option<String>, String>>> {
        let function = self.probe_config?;
        Some(self.config_call(function, request, "probe_config"))
    }

    /// `None` means the plugin exports no hook — every step is accepted.
    /// Otherwise the plugin's verdict on the raw step; a hook that fails is
    /// a refusal, never an accepted step.
    pub fn validate_step(&self, step: u32, request: &str) -> Option<Result<(), String>> {
        let function = self.validate_step?;
        let verdict = self
            .call_with("validate_step", request, |argument| {
                // SAFETY: `function` was resolved from this library with this
                // exact signature, and the plugin only reads from `argument`.
                unsafe { function(step, argument) }
            })
            .and_then(|reply| {
                serde_json::from_str::<Envelope>(&reply)
                    .with_context(|| self.malformed("validate_step", &reply))
            });
        Some(verdict.map_or_else(|e| Err(format!("{e:#}")), Envelope::into_result))
    }

    pub fn init_instance(&self, request: &str) -> Result<Result<u64, InitFailure>> {
        let reply = self.call_json(self.init_instance, request, "init_instance")?;
        let response: InitResponse = serde_json::from_str(&reply)
            .with_context(|| self.malformed("init_instance", &reply))?;
        Ok(response.into_result())
    }

    pub fn dispatch(&self, handle: u64, step: u32, request: &str) -> Result<DispatchResult> {
        let argument = CString::new(request).with_context(|| {
            format!(
                "plugin {:?} dispatch payload contains a NUL byte",
                self.name
            )
        })?;
        // SAFETY: `argument` outlives the call, and the plugin only reads from
        // it. The returned pointer goes straight back to this plugin's own
        // `free_string` inside `take`.
        let reply = unsafe {
            take(
                self.free_string,
                (self.dispatch)(handle, step, argument.as_ptr()),
            )
        }
        .with_context(|| format!("plugin {:?} returned nothing from dispatch", self.name))?;
        serde_json::from_str(&reply).or_else(|error| {
            let message = self.malformed("dispatch", &reply);
            // The message quotes the reply, secrets included: hand them to the
            // host as a failing result rather than lose them in an `Err`.
            DispatchResult::salvage(&reply, message.clone())
                .ok_or_else(|| anyhow::Error::new(error).context(message))
        })
    }

    /// `I use "<instance>" <group>` on a group-scoped handle. Absent symbol:
    /// nothing to tell, so `Ok`.
    pub fn select_instance(&self, handle: u64, request: &str) -> Result<Result<(), String>> {
        let Some(function) = self.select_instance else {
            return Ok(Ok(()));
        };
        let reply = self.call_with("select_instance", request, |argument| {
            // SAFETY: `function` was resolved from this library with this
            // exact signature, and the plugin only reads from `argument`.
            unsafe { function(handle, argument) }
        })?;
        let envelope: Envelope = serde_json::from_str(&reply)
            .with_context(|| self.malformed("select_instance", &reply))?;
        Ok(envelope.into_result())
    }

    pub fn drop_instance(&self, handle: u64) -> Result<Result<(), String>> {
        let reply = self.call_handle(self.drop_instance, handle, "drop_instance")?;
        let envelope: Envelope = serde_json::from_str(&reply)
            .with_context(|| self.malformed("drop_instance", &reply))?;
        Ok(envelope.into_result())
    }

    pub fn reset_scenario(&self, handle: u64) -> Result<Result<(), String>> {
        let Some(function) = self.reset_scenario else {
            return Ok(Ok(()));
        };
        let reply = self.call_handle(function, handle, "reset_scenario")?;
        let envelope: Envelope = serde_json::from_str(&reply)
            .with_context(|| self.malformed("reset_scenario", &reply))?;
        Ok(envelope.into_result())
    }

    /// The two config replies. Their `secrets` are recorded into the process
    /// sink before anything else reads the reply, `ok` or not, and also from a
    /// reply that does not parse as a whole: `malformed` quotes it.
    fn config_call(
        &self,
        function: JsonFn,
        request: &str,
        what: &str,
    ) -> Result<Result<Option<String>, String>> {
        let reply = self.call_json(function, request, what)?;
        if let Some(secrets) = serde_json::from_str::<serde_json::Value>(&reply)
            .ok()
            .as_ref()
            .and_then(|value| value.get("secrets")?.as_array())
        {
            secrets
                .iter()
                .filter_map(|secret| secret.as_str())
                .for_each(crate::privacy::record_value);
        }
        let parsed: ConfigReply =
            serde_json::from_str(&reply).with_context(|| self.malformed(what, &reply))?;
        Ok(parsed.into_result())
    }

    fn call_json(&self, function: JsonFn, request: &str, what: &str) -> Result<String> {
        self.call_with(what, request, |argument| {
            // SAFETY: `function` was resolved from this library with this
            // exact signature, and the plugin only reads from `argument`.
            unsafe { function(argument) }
        })
    }

    /// Hands `request` to `call` as a C string that outlives it. `call` must
    /// return a reply of this plugin's own (or NULL).
    fn call_with(
        &self,
        what: &str,
        request: &str,
        call: impl FnOnce(*const c_char) -> *mut c_char,
    ) -> Result<String> {
        let argument = CString::new(request).with_context(|| {
            format!("plugin {:?} {what} payload contains a NUL byte", self.name)
        })?;
        // SAFETY: the pointer `call` returns came from this plugin, and `take`
        // frees it with the plugin's own `free_string`.
        unsafe { take(self.free_string, call(argument.as_ptr())) }
            .with_context(|| format!("plugin {:?} returned nothing from {what}", self.name))
    }

    fn call_handle(&self, function: HandleFn, handle: u64, what: &str) -> Result<String> {
        // SAFETY: as for `call_json`, with a plain integer argument instead of
        // a borrowed string.
        unsafe { take(self.free_string, function(handle)) }
            .with_context(|| format!("plugin {:?} returned nothing from {what}", self.name))
    }

    fn malformed(&self, what: &str, reply: &str) -> String {
        format!(
            "plugin {:?} returned a malformed {what} reply: {reply}",
            self.name
        )
    }
}

/// The host speaks the generations `ABI_MIN` to `ABI_VERSION`. A pure
/// predicate over the reported number, extracted for the same reason as
/// `check_reset_scenario`: it is reachable from a unit test without building a
/// cdylib per rejection case.
fn check_abi_version(name: &str, reported: u32) -> Result<()> {
    if reported > ABI_VERSION {
        bail!(
            "plugin {name:?} was built for ABI version {reported}, newer than the {ABI_VERSION} this bddkit speaks: upgrade bddkit"
        );
    }
    if reported < ABI_MIN {
        bail!(
            "plugin {name:?} was built for ABI version {reported}, older than the {ABI_MIN} this bddkit still loads: the plugin is not updated for this bddkit yet"
        );
    }
    Ok(())
}

/// What a step spec declares is checked and compiled once, here: exactly one of
/// `pattern` and `expression`, `secret_groups` only on a `pattern`, and every
/// effect placeholder naming an argument. A placeholder cannot name a secret
/// one, because `doctor` prints what an effect holds. A pattern that does not
/// compile is left to the registry, which names it.
///
/// Reached from a unit test without building a cdylib per rejection case.
fn prepare_steps(name: &str, steps: &mut [StepSpec]) -> Result<()> {
    for step in steps {
        match (&step.pattern, &step.expression) {
            (Some(_), Some(_)) => bail!(
                "plugin {name:?} declares a step with both pattern and expression: {:?}",
                step.declaration()
            ),
            (None, None) => bail!(
                "plugin {name:?} declares a step with neither pattern nor expression, in group {:?}",
                step.group
            ),
            (Some(pattern), None) => {
                let Ok(regex) = regex::Regex::new(pattern) else {
                    continue;
                };
                // A group can nest inside a secret one and `regex` does not say
                // so, so a pattern holding a secret names no group at all. Such
                // a step can be an `expression`, whose parameters are flat.
                step.arg_names = regex
                    .capture_names()
                    .skip(1)
                    .map(|group| {
                        group
                            .filter(|_| step.secret_groups.is_empty())
                            .map(str::to_string)
                    })
                    .collect();
            }
            (None, Some(expression)) => {
                if !step.secret_groups.is_empty() {
                    bail!(
                        "plugin {name:?} declares secret_groups on the expression {expression:?}; write {{name:secret}} instead"
                    );
                }
                let compiled = crate::steps::expression::compile(expression, "text")
                    .map_err(|error| anyhow::anyhow!("plugin {name:?}: {error}"))?;
                step.arg_names = compiled
                    .params
                    .iter()
                    .map(|param| (!param.ty.secret).then(|| param.name.clone()))
                    .collect();
                step.compiled = Some(compiled);
            }
        }
        for template in step.effect_templates() {
            for placeholder in StepSpec::placeholders(template) {
                if !step.arg_names.iter().flatten().any(|n| n == placeholder) {
                    bail!(
                        "plugin {name:?} step {:?}: the effect {template:?} names {{{placeholder}}}, which is no parameter of the step, or a secret one, or a group of a pattern with secret_groups",
                        step.declaration()
                    );
                }
            }
        }
    }
    Ok(())
}

/// A step may only sit in a group its own manifest claims. A plugin that
/// answered for an unclaimed group would shadow the steps of whichever plugin
/// legitimately owns it, so this is refused at load time rather than surfacing
/// as a mis-routed step much later.
///
/// A pure predicate over already-parsed data, for the same reason as
/// `check_reset_scenario`.
fn check_step_groups(name: &str, manifest: &Manifest, steps: &[StepSpec]) -> Result<()> {
    for step in steps {
        if !manifest.groups.contains(&step.group) {
            bail!(
                "plugin {name:?} declares step {:?} in group {:?}, which it does not claim",
                step.declaration(),
                step.group
            );
        }
    }
    Ok(())
}

/// A group may only be described by the plugin that serves it. Reasoned exactly
/// as `check_step_groups`: a description of a group this plugin does not claim
/// would shadow the description of whichever plugin legitimately owns it, and
/// `bddkit resource fields` would print the wrong keys for it.
///
/// A pure predicate over the already-parsed manifest, so it is reachable from a
/// unit test without building a cdylib per rejection case.
fn check_field_groups(name: &str, manifest: &Manifest) -> Result<()> {
    for group in manifest.fields.keys() {
        if !manifest.groups.contains(group) {
            bail!(
                "plugin {name:?} describes the config of group {group:?}, which it does not claim"
            );
        }
    }
    Ok(())
}

/// The same rule for `implicit_instance`: a body for a group this plugin does
/// not claim would be synthesized in place of whichever plugin serves it.
fn check_implicit_groups(name: &str, manifest: &Manifest) -> Result<()> {
    for group in manifest.implicit_instance.keys() {
        if !manifest.groups.contains(group) {
            bail!(
                "plugin {name:?} declares implicit_instance.{group}, but does not claim the group {group:?}"
            );
        }
    }
    Ok(())
}

/// `secret_groups` counts capture groups from 1, as a regex does. An index no
/// group has would silently protect nothing, so it is refused at load. A
/// pattern that does not compile is left to the registry, which names it.
///
/// A pure predicate over the already-parsed step list, like its neighbours.
fn check_secret_groups(name: &str, steps: &[StepSpec]) -> Result<()> {
    for step in steps.iter().filter(|step| !step.secret_groups.is_empty()) {
        let Some(Ok(pattern)) = step.pattern.as_deref().map(regex::Regex::new) else {
            continue;
        };
        let groups = pattern.captures_len() - 1;
        if let Some(bad) = step
            .secret_groups
            .iter()
            .find(|g| !(1..=groups).contains(*g))
        {
            bail!(
                "plugin {name:?} marks capture group {bad} of {:?} as secret, but the pattern has {groups} (groups count from 1)",
                step.declaration()
            );
        }
    }
    Ok(())
}

/// A declared `type` the host does not know is refused here rather than
/// silently read as a string, for the reason the key exists at all: a string
/// is what a boolean field already got, and what its `validate_config` already
/// refused. A typo that degrades into the old behaviour would be reported to
/// the plugin author as their own bug, from the far side of the FFI boundary.
///
/// A pure predicate over the already-parsed manifest, like its two neighbours.
fn check_field_types(name: &str, manifest: &Manifest) -> Result<()> {
    for (group, fields) in &manifest.fields {
        for field in fields {
            let Some(declared) = &field.value_type else {
                continue;
            };
            if crate::config::Scalar::from_declared(declared).is_none() {
                bail!(
                    "plugin {name:?} declares field {:?} of group {group:?} as type {declared:?}, which is not one of: {}",
                    field.name,
                    crate::config::Scalar::DECLARABLE
                );
            }
        }
    }
    Ok(())
}

/// A `shared` handle serves every worker at once, so a per-worker notion
/// cannot be scoped to one: a per-scenario reset would clear state another
/// worker is mid-scenario in (the reviewer saw both an assertion that never
/// passed because its counter kept being zeroed, and a file with no plugin
/// steps failing on another file's instance), and an instance selection is
/// per file, which a shared handle is not. `per_worker` has no such problem,
/// so it is allowed at any concurrency, and declaring it is the remedy the
/// message offers.
///
/// `symbol` is the export that cannot be kept (`bddkit_reset_scenario`,
/// `bddkit_select_instance`), `what` says why. A free function over
/// already-parsed data, so it is reachable from a unit test without building a
/// cdylib per rejection case.
pub fn check_per_file_symbol(
    name: &str,
    concurrency_mode: Concurrency,
    symbol: &str,
    exported: bool,
    why: &str,
    concurrency: usize,
) -> Result<()> {
    if exported && concurrency_mode == Concurrency::Shared && concurrency > 1 {
        bail!(
            "plugin {name:?} declares concurrency \"shared\" and exports \
             {symbol}, but this run has concurrency {concurrency}: {why}. Declare \
             \"per_worker\" in the manifest, or set concurrency: 1"
        );
    }
    Ok(())
}

pub fn check_reset_scenario(
    name: &str,
    concurrency_mode: Concurrency,
    exports_reset: bool,
    concurrency: usize,
) -> Result<()> {
    check_per_file_symbol(
        name,
        concurrency_mode,
        "bddkit_reset_scenario",
        exports_reset,
        "a shared instance cannot have its per-scenario reset scoped to one worker",
        concurrency,
    )
}

pub fn check_select_instance(
    name: &str,
    concurrency_mode: Concurrency,
    exports_select: bool,
    concurrency: usize,
) -> Result<()> {
    check_per_file_symbol(
        name,
        concurrency_mode,
        "bddkit_select_instance",
        exports_select,
        "the selection is per feature file and a shared handle is not",
        concurrency,
    )
}

/// `eager_init` names groups whose `shared` handle the host builds at startup:
/// a group the manifest does not claim has no handle, and a `per_worker` one
/// belongs to a file that does not exist yet.
fn check_eager_init(name: &str, manifest: &Manifest) -> Result<()> {
    if manifest.eager_init.is_empty() {
        return Ok(());
    }
    if manifest.concurrency != Concurrency::Shared {
        bail!(
            "plugin {name:?} declares eager_init but not concurrency \"shared\": only a shared \
             handle exists before the first file"
        );
    }
    for group in &manifest.eager_init {
        if !manifest.groups.contains(group) {
            bail!(
                "plugin {name:?} declares eager_init for the group {group:?}, which it does not claim"
            );
        }
    }
    Ok(())
}

/// A deprecated word maps to a group this plugin claims; clashes with other
/// plugins and the host are checked once every plugin is loaded
/// (`Plugins::check_deprecated_words`).
fn check_deprecated_targets(name: &str, manifest: &Manifest) -> Result<()> {
    for (word, target) in &manifest.deprecated {
        if !manifest.groups.contains(target) {
            bail!(
                "plugin {name:?} maps the deprecated word {word:?} to the group {target:?}, which it does not claim"
            );
        }
    }
    Ok(())
}

/// Copies a plugin-allocated string and hands the original straight back to the
/// plugin's own `free_string`. Freeing it with the host allocator would be
/// undefined behaviour, and the copy is why every later step can ignore FFI.
///
/// # Safety
/// `pointer` must be NULL, or a NUL-terminated string this plugin allocated and
/// has not yet freed; `free` must be the same plugin's `bddkit_free_string`.
/// The pointer is consumed: the caller must not use it again.
unsafe fn take(free: FreeFn, pointer: *mut c_char) -> Option<String> {
    if pointer.is_null() {
        return None;
    }
    // SAFETY: the caller guarantees a live NUL-terminated string; the copy is
    // finished before the pointer is released, so nothing borrows freed memory.
    let owned = unsafe { CStr::from_ptr(pointer) }
        .to_string_lossy()
        .into_owned();
    // SAFETY: the pointer came from this plugin and is freed exactly once,
    // here, by the plugin's own allocator.
    unsafe { free(pointer) };
    Some(owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Same build as tests/common::build_fixture_plugin; a unit test cannot
    /// reach the integration test helpers, and six lines beat a shared crate.
    fn fixture() -> std::path::PathBuf {
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
        target
            .join("debug")
            .join(crate::plugin::install::dll_name("echo_plugin"))
    }

    fn prepared(step: &str) -> Result<Vec<StepSpec>> {
        let mut steps: Vec<StepSpec> =
            serde_json::from_str(&format!(r#"[{{"group":"w","kind":"action",{step}}}]"#))
                .expect("steps parse");
        prepare_steps("widget", &mut steps)?;
        Ok(steps)
    }

    #[test]
    fn a_step_declares_exactly_one_of_pattern_and_expression() {
        prepared(r#""pattern":"^a$""#).expect("a pattern alone");
        let steps = prepared(r#""expression":"a {n:uint}""#).expect("an expression alone");
        assert!(steps[0].compiled.is_some());
        for (step, needle) in [
            (r#""pattern":"^a$","expression":"a""#, "both"),
            (r#""description":"d""#, "neither"),
            (r#""expression":"a {n:nope}""#, "unknown type"),
            (
                r#""expression":"a {n}","secret_groups":[1]"#,
                "secret_groups",
            ),
        ] {
            let error = prepared(step).expect_err("refused").to_string();
            assert!(error.contains(needle), "{step}: {error}");
        }
    }

    #[test]
    fn a_builtins_expression_compiles_as_it_does_for_the_builtin() {
        // The API and DB plugins declare built-in expressions verbatim.
        for (def, builtin) in crate::steps::BUILTIN_STEPS
            .iter()
            .zip(crate::steps::COMPILED_BUILTINS.iter())
        {
            let expression = serde_json::to_string(def.expression).expect("encodes");
            let steps = prepared(&format!(r#""expression":{expression}"#)).expect("compiles");
            let compiled = steps[0].compiled.as_ref().expect("compiled");
            assert_eq!(compiled.regex.as_str(), builtin.regex.as_str());
            assert_eq!(compiled.template, builtin.template);
        }
    }

    #[test]
    fn an_effect_may_name_only_an_argument_the_step_has_and_shows() {
        prepared(r#""expression":"a {n}","sets_vars":["{n}_*"],"reads":["s:{n}"],"writes":["w"]"#)
            .expect("n is a parameter");
        prepared(r#""pattern":"^a (?P<n>x)$","writes":["{n}"]"#).expect("n is a named group");
        for step in [
            r#""expression":"a {n}","sets_vars":["{m}"]"#,
            r#""expression":"a {n}","reads":["{m}"]"#,
            r#""expression":"a {n}","writes":["{m}"]"#,
            r#""pattern":"^a (x)$","sets_vars":["{m}"]"#,
            r#""expression":"a {key:secret}","reads":["{key}"]"#,
            r#""pattern":"^a (?P<key>x)$","secret_groups":[1],"reads":["{key}"]"#,
            r#""pattern":"^a (?P<key>x(?P<inner>y))$","secret_groups":[1],"reads":["{inner}"]"#,
            r#""pattern":"^a (?P<key>x) (?P<n>y)$","secret_groups":[1],"reads":["{n}"]"#,
        ] {
            let error = prepared(step).expect_err("refused").to_string();
            assert!(error.contains("no parameter"), "{step}: {error}");
        }
    }

    #[test]
    fn a_secret_group_no_pattern_group_has_is_refused() {
        let step = |groups: &str| -> Vec<StepSpec> {
            serde_json::from_str(&format!(
                r#"[{{"pattern":"^a \"(.*)\" \"(.*)\"$","group":"w","kind":"action","secret_groups":{groups}}}]"#
            ))
            .expect("steps parse")
        };
        check_secret_groups("widget", &step("[1,2]")).expect("both groups exist");
        for bad in ["[0]", "[3]"] {
            let error = check_secret_groups("widget", &step(bad)).expect_err("refused");
            assert!(
                error.to_string().contains("groups count from 1"),
                "{bad}: {error}"
            );
        }
    }

    #[test]
    fn loads_the_fixture_and_reads_its_manifest() {
        let lib = Library::load("echo", &fixture()).expect("loads");
        assert_eq!(lib.manifest.name, "echo");
        assert_eq!(lib.manifest.groups, vec!["echo".to_string()]);
    }

    #[test]
    fn loads_under_any_label_and_keeps_the_manifest_name() {
        // `plugin install owner/repo` loads before the name is known; the
        // lock-driven path checks the name itself (`Plugins::load`).
        let lib = Library::load("someone/echo-plugin", &fixture()).expect("loads");
        assert_eq!(lib.manifest.name, "echo");
    }

    #[test]
    fn lists_the_steps() {
        let lib = Library::load("echo", &fixture()).expect("loads");
        assert_eq!(lib.steps.len(), 27);
        assert!(!lib.steps[0].is_assertion());
        assert!(lib.steps[1].is_assertion());
        assert_eq!(lib.steps[0].group, "echo");
    }

    #[test]
    fn a_missing_file_names_the_path() {
        let error = Library::load("ghost", std::path::Path::new("/nope/libghost.so"))
            .expect_err("no such file");
        assert!(
            format!("{error:#}").contains("/nope/libghost.so"),
            "{error:#}"
        );
    }

    #[test]
    fn validate_config_reaches_the_plugin() {
        let lib = Library::load("echo", &fixture()).expect("loads");
        let ok = lib
            .validate_config(r#"{"group":"echo","instance":"a","config":{"prefix":"p-"}}"#)
            .expect("call succeeds");
        assert!(ok.is_ok(), "{ok:?}");

        let bad = lib
            .validate_config(r#"{"group":"echo","instance":"a","config":{}}"#)
            .expect("call succeeds");
        assert!(bad.unwrap_err().contains("prefix"));
    }

    #[test]
    fn an_instance_round_trips_through_init_dispatch_and_drop() {
        let lib = Library::load("echo", &fixture()).expect("loads");
        let handle = lib
            .init_instance(r#"{"group":"echo","instance":"a","config":{"prefix":"p-"}}"#)
            .expect("call succeeds")
            .expect("instance created");
        let result = lib
            .dispatch(handle, 0, r#"{"args":["x","name"],"debug":false}"#)
            .expect("call succeeds");
        assert_eq!(result.status, crate::plugin::abi::Status::Passed);
        assert_eq!(result.vars.get("name").map(String::as_str), Some("p-x"));
        lib.drop_instance(handle)
            .expect("call succeeds")
            .expect("dropped");
    }

    #[test]
    fn an_optional_symbol_that_is_absent_is_not_an_error() {
        // The fixture exports reset_scenario, so the absent case has to be
        // constructed: a plugin without it must still answer Ok, not fail.
        let mut lib = Library::load("echo", &fixture()).expect("loads");
        assert!(lib.has_reset_scenario());
        lib.reset_scenario = None;
        assert!(!lib.has_reset_scenario());
        // Resolves to the method; the field would need parentheses.
        assert!(lib.reset_scenario(1).expect("call succeeds").is_ok());
    }

    #[test]
    fn an_abi_mismatch_names_both_versions() {
        let text = |reported| {
            format!(
                "{:#}",
                check_abi_version("odd", reported).expect_err("refused")
            )
        };
        let newer = text(ABI_VERSION + 1);
        assert!(
            newer.contains("odd") && newer.contains(&(ABI_VERSION + 1).to_string()),
            "{newer}"
        );
        assert!(newer.contains("upgrade bddkit"), "{newer}");
        let older = text(ABI_MIN - 1);
        assert!(older.contains(&(ABI_MIN - 1).to_string()), "{older}");
        assert!(older.contains("not updated"), "{older}");
        for accepted in ABI_MIN..=ABI_VERSION {
            check_abi_version("current", accepted).expect("inside the range");
        }
    }

    #[test]
    fn a_step_in_an_unclaimed_group_is_refused() {
        // Answering for a group it never claimed would shadow the steps of the
        // plugin that legitimately owns it.
        let manifest: Manifest =
            serde_json::from_str(r#"{"name":"widget","version":"1.0.0","groups":["widget"]}"#)
                .expect("manifest parses");
        let steps: Vec<StepSpec> = serde_json::from_str(
            r#"[{"pattern":"^I upload$","group":"widget","kind":"action"},
                {"pattern":"^I click$","group":"browser","kind":"action"}]"#,
        )
        .expect("steps parse");
        check_step_groups("widget", &manifest, &steps[..1]).expect("a claimed group is fine");
        let error = check_step_groups("widget", &manifest, &steps).expect_err("refused");
        let text = format!("{error:#}");
        assert!(text.contains("browser"), "{text}");
        assert!(text.contains("I click"), "{text}");
    }

    #[test]
    fn a_shared_plugin_with_a_reset_is_still_refused_in_parallel() {
        let error =
            check_reset_scenario("echo", Concurrency::Shared, true, 8).expect_err("refused");
        let text = format!("{error:#}");
        assert!(text.contains("echo"), "{text}");
        assert!(text.contains("bddkit_reset_scenario"), "{text}");
        assert!(text.contains("concurrency: 1"), "{text}");
        assert!(text.contains("per_worker"), "the remedy is named: {text}");
        // At the boundary too: the rule is "more than one worker", not "many".
        check_reset_scenario("echo", Concurrency::Shared, true, 2).expect_err("refused at 2");
    }

    #[test]
    fn a_per_worker_plugin_with_a_reset_loads_in_parallel() {
        // This is the whole point of the milestone.
        check_reset_scenario("browser", Concurrency::PerWorker, true, 8).expect("allowed");
    }

    #[test]
    fn a_shared_plugin_with_a_reset_loads_sequentially() {
        check_reset_scenario("echo", Concurrency::Shared, true, 1).expect("allowed");
    }

    #[test]
    fn a_plugin_without_a_reset_loads_at_any_concurrency() {
        check_reset_scenario("echo", Concurrency::Shared, false, 8).expect("allowed");
    }

    #[test]
    fn probe_config_reaches_the_plugin() {
        let lib = Library::load("echo", &fixture()).expect("loads");
        let ok = lib
            .probe_config(r#"{"group":"echo","instance":"a","config":{"prefix":"p-"}}"#)
            .expect("the fixture exports a probe")
            .expect("call succeeds");
        assert!(ok.is_ok(), "{ok:?}");

        let bad = lib
            .probe_config(
                r#"{"group":"echo","instance":"a","config":{"prefix":"p-","probe_error":"endpoint refused the connection"}}"#,
            )
            .expect("the fixture exports a probe")
            .expect("call succeeds");
        assert_eq!(bad.unwrap_err(), "endpoint refused the connection");
    }

    #[test]
    fn a_plugin_without_a_probe_reports_no_probe_rather_than_a_failure() {
        // The distinction is the whole point of the outer Option: an absent
        // live check is "not available", never "the configuration is broken".
        // `reset_scenario` can answer Ok when absent because "nothing to
        // reset" really is success; a probe that never ran proved nothing.
        let mut lib = Library::load("echo", &fixture()).expect("loads");
        assert!(lib.has_probe_config());
        lib.probe_config = None;
        assert!(!lib.has_probe_config());
        assert!(
            lib.probe_config(r#"{"group":"echo","instance":"a","config":{}}"#)
                .is_none()
        );
    }

    #[test]
    fn describing_a_group_it_does_not_claim_is_refused() {
        // Same reason as `check_step_groups`: a plugin describing a group it
        // does not serve would shadow the description of whichever plugin
        // legitimately owns it.
        let manifest: Manifest = serde_json::from_str(
            r#"{"name":"widget","version":"1.0.0","groups":["widget"],
                "fields":{"widget":[{"name":"selector"}],
                          "browser":[{"name":"headless"}]}}"#,
        )
        .expect("manifest parses");
        let error = check_field_groups("widget", &manifest).expect_err("refused");
        let text = format!("{error:#}");
        assert!(text.contains("widget"), "{text}");
        assert!(text.contains("browser"), "{text}");
    }

    /// A typo in a declared type must not degrade into "string": that is the
    /// exact failure the key exists to prevent, and a silent fallback would
    /// write `'false'` for a boolean key and leave the plugin's own
    /// `validate_config` to take the blame for refusing it.
    #[test]
    fn declaring_a_type_the_host_does_not_know_is_refused() {
        let manifest: Manifest = serde_json::from_str(
            r#"{"name":"widget","version":"1.0.0","groups":["widget"],
                "fields":{"widget":[{"name":"headless","type":"boolena"}]}}"#,
        )
        .expect("manifest parses");
        let error = check_field_types("widget", &manifest).expect_err("refused");
        let text = format!("{error:#}");
        assert!(text.contains("headless"), "{text}");
        assert!(text.contains("boolena"), "{text}");
        assert!(
            text.contains("boolean"),
            "the accepted values are named: {text}"
        );
    }

    /// The backward-compatibility half: a manifest written before `type`
    /// existed declares none, and that is not a finding.
    #[test]
    fn declaring_no_type_at_all_is_fine() {
        let manifest: Manifest = serde_json::from_str(
            r#"{"name":"widget","version":"1.0.0","groups":["widget"],
                "fields":{"widget":[{"name":"selector"},{"name":"headless","type":"boolean"}]}}"#,
        )
        .expect("manifest parses");
        check_field_types("widget", &manifest).expect("string is the default, boolean is known");
    }

    #[test]
    fn an_implicit_instance_of_a_group_it_does_not_claim_is_refused() {
        // Same shadowing hazard as `fields`: the body would stand in for one
        // belonging to whichever plugin serves that group.
        let manifest: Manifest = serde_json::from_str(
            r#"{"name":"widget","version":"1.0.0","groups":["widget"],
                "implicit_instance":{"browser":{}}}"#,
        )
        .expect("manifest parses");
        let error = check_implicit_groups("widget", &manifest).expect_err("refused");
        let text = format!("{error:#}");
        assert!(text.contains("widget"), "{text}");
        assert!(text.contains("browser"), "{text}");
        assert!(text.contains("implicit_instance"), "{text}");
    }

    #[test]
    fn describing_only_claimed_groups_is_fine() {
        let manifest: Manifest = serde_json::from_str(
            r#"{"name":"widget","version":"1.0.0","groups":["widget"],
                "fields":{"widget":[{"name":"selector"}]}}"#,
        )
        .expect("manifest parses");
        check_field_groups("widget", &manifest).expect("its own group is fine");
    }

    #[test]
    fn a_shared_plugin_with_a_select_instance_is_refused_in_parallel_only() {
        let error = check_select_instance("db", Concurrency::Shared, true, 2).expect_err("refused");
        assert!(
            format!("{error}").contains("bddkit_select_instance"),
            "{error}"
        );
        check_select_instance("db", Concurrency::Shared, true, 1).expect("sequential");
        check_select_instance("api", Concurrency::PerWorker, true, 8).expect("per_worker");
        check_select_instance("db", Concurrency::Shared, false, 8).expect("no export");
    }
}
