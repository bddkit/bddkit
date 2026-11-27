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
#[expect(
    clippy::struct_field_names,
    reason = "`_library` is the drop-order anchor the doc above names"
)]
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
    failure_context: Option<HandleFn>,
    free_string: FreeFn,
    _library: libloading::Library,
}

impl std::fmt::Debug for Library {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Library")
            .field("name", &self.name)
            .field("groups", &self.manifest.groups)
            .field("steps", &self.steps.len())
            .finish_non_exhaustive()
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
        // SAFETY: (for every `library.get` in this function) resolving a symbol is unsafe
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

        // SAFETY: as for the first `library.get` above.
        let manifest_fn: NoArgFn = *unsafe { library.get(b"bddkit_manifest\0") }
            .with_context(|| format!("plugin {name:?} exports no bddkit_manifest"))?;
        // SAFETY: as for the first `library.get` above.
        let list_steps_fn: NoArgFn = *unsafe { library.get(b"bddkit_list_steps\0") }
            .with_context(|| format!("plugin {name:?} exports no bddkit_list_steps"))?;
        // SAFETY: as for the first `library.get` above.
        let free_string: FreeFn = *unsafe { library.get(b"bddkit_free_string\0") }
            .with_context(|| format!("plugin {name:?} exports no bddkit_free_string"))?;
        // SAFETY: as for the first `library.get` above.
        let validate_config: JsonFn = *unsafe { library.get(b"bddkit_validate_config\0") }
            .with_context(|| format!("plugin {name:?} exports no bddkit_validate_config"))?;
        // SAFETY: as for the first `library.get` above.
        let init_instance: JsonFn = *unsafe { library.get(b"bddkit_init_instance\0") }
            .with_context(|| format!("plugin {name:?} exports no bddkit_init_instance"))?;
        // SAFETY: as for the first `library.get` above.
        let dispatch: DispatchFn = *unsafe { library.get(b"bddkit_dispatch\0") }
            .with_context(|| format!("plugin {name:?} exports no bddkit_dispatch"))?;
        // SAFETY: as for the first `library.get` above.
        let drop_instance: HandleFn = *unsafe { library.get(b"bddkit_drop_instance\0") }
            .with_context(|| format!("plugin {name:?} exports no bddkit_drop_instance"))?;
        // Optional: a plugin with no per-scenario state need not export it.
        // SAFETY: as for the first `library.get` above.
        let reset_scenario: Option<HandleFn> = unsafe { library.get(b"bddkit_reset_scenario\0") }
            .ok()
            .map(|symbol| *symbol);
        // Optional in the same way: a plugin that cannot check reachability
        // simply has no live check, which is reported as "not available".
        // SAFETY: as for the first `library.get` above.
        let probe_config: Option<JsonFn> = unsafe { library.get(b"bddkit_probe_config\0") }
            .ok()
            .map(|symbol| *symbol);
        // Optional: a plugin that checks nothing before a run simply accepts
        // every step.
        // SAFETY: as for the first `library.get` above.
        let validate_step: Option<StepValidateFn> =
            unsafe { library.get(b"bddkit_validate_step\0") }
                .ok()
                .map(|symbol| *symbol);

        // Optional: a plugin that needs no notice of `I use "<name>" <group>`
        // exports nothing.
        // SAFETY: as for the first `library.get` above.
        let select_instance: Option<SelectInstanceFn> =
            unsafe { library.get(b"bddkit_select_instance\0") }
                .ok()
                .map(|symbol| *symbol);

        // Optional: a plugin that adds nothing to a failure's dump exports
        // nothing.
        // SAFETY: as for the first `library.get` above.
        let failure_context: Option<HandleFn> = unsafe { library.get(b"bddkit_failure_context\0") }
            .ok()
            .map(|symbol| *symbol);

        // SAFETY: `manifest_fn` was resolved from this library with its declared
        // signature after the ABI check, and takes no arguments.
        let manifest_ptr = unsafe { manifest_fn() };
        // SAFETY: the pointer came from this plugin's `manifest_fn` and goes
        // straight back to its `free_string` inside `take` — the host never
        // frees plugin memory itself.
        let manifest_json = unsafe { take(free_string, manifest_ptr) }
            .with_context(|| format!("plugin {name:?} returned no manifest"))?;
        let manifest: Manifest = serde_json::from_str(&manifest_json)
            .with_context(|| format!("plugin {name:?} returned a malformed manifest"))?;
        if manifest.groups.is_empty() {
            bail!("plugin {name:?} claims no resource group");
        }
        // SAFETY: `list_steps_fn` was resolved from this library with its declared
        // signature after the ABI check, and takes no arguments.
        let steps_ptr = unsafe { list_steps_fn() };
        // SAFETY: as for `manifest_ptr` above.
        let steps_json = unsafe { take(free_string, steps_ptr) }
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
            failure_context,
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

    pub fn has_failure_context(&self) -> bool {
        self.failure_context.is_some()
    }

    /// The raw reply of `bddkit_failure_context`; `None` when the plugin
    /// exports no such symbol, which is not a failure. The caller parses it:
    /// a malformed reply is quoted nowhere, and its `secrets` still count.
    pub fn failure_context(&self, handle: u64) -> Option<Result<String>> {
        let function = self.failure_context?;
        Some(self.call_handle(function, handle, "failure_context"))
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
        // it.
        let reply_ptr = unsafe { (self.dispatch)(handle, step, argument.as_ptr()) };
        // SAFETY: the pointer came from this plugin's `dispatch` and goes
        // straight back to its own `free_string` inside `take`.
        let reply = unsafe { take(self.free_string, reply_ptr) }
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
        // SAFETY: `function` was resolved from this library with its declared
        // signature after the ABI check; its one argument is a plain integer.
        let reply_ptr = unsafe { function(handle) };
        // SAFETY: as for `call_with`: the pointer came from this plugin and
        // `take` frees it with the plugin's own `free_string`.
        unsafe { take(self.free_string, reply_ptr) }
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
/// `pattern` and `expression`, `secret_groups` only on a `pattern`, `negative`
/// only on an assertion, and every effect placeholder naming an argument. A
/// placeholder cannot name a secret one, because `doctor` prints what an effect
/// holds. A pattern that does not compile is left to the registry, which names
/// it.
///
/// Reached from a unit test without building a cdylib per rejection case.
fn prepare_steps(name: &str, steps: &mut [StepSpec]) -> Result<()> {
    for step in steps {
        // Before the match, which leaves early on a pattern that does not compile.
        if step.negative && !step.is_assertion() {
            bail!(
                "plugin {name:?} marks the action step {:?} as negative: only an assertion can be",
                step.declaration()
            );
        }
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

pub fn check_failure_context(
    name: &str,
    concurrency_mode: Concurrency,
    exports_context: bool,
    concurrency: usize,
) -> Result<()> {
    check_per_file_symbol(
        name,
        concurrency_mode,
        "bddkit_failure_context",
        exports_context,
        "the scenario's handles are listed per feature file and a shared handle is not",
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
mod tests;
