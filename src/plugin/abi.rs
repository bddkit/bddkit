//! Everything that crosses the FFI boundary. Only JSON strings cross it:
//! `rustc` has no stable ABI, so the contract is a documented JSON schema
//! plus the C representation of a pointer, and nothing else.
//!
//! None of these types set `deny_unknown_fields`: a newer plugin may send
//! extra keys an older host doesn't know about yet, and those must be
//! ignored rather than fail the parse. This is the opposite choice from
//! `src/options.rs`, which deliberately rejects unknown keys to catch a typo
//! in hand-written YAML — that asymmetry is intentional, not an oversight to
//! "clean up".

use crate::steps::expression::Compiled;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::LazyLock;

/// The newest ABI generation this host speaks; it loads every plugin reporting
/// a number from `ABI_MIN` to this one, and refuses the rest.
///
/// The rule for a change: a compatible one raises only `ABI_VERSION`, an
/// incompatible one raises `ABI_MIN` too. Compatible means an optional key or
/// symbol with a default, or a request field an older plugin can ignore
/// without harm. Incompatible means removing or renaming a field or symbol,
/// changing its meaning, or adding a request field a plugin must honour. The
/// host never gates an addition on the number a plugin reports: a plugin
/// built for an older generation does not send the new keys or export the new
/// symbols, and gets each addition's default.
///
/// 2: the dispatch request gained `evidence`, the reply `secrets`, a
/// diagnostic `media_type` and a step spec `secret_groups` (#90). Each is
/// optional on its own; the generation moved once, for the lot, so a plugin
/// that sends `secrets` can rely on a host that reads them.
///
/// 3: the request's `vars`, the spec's `wants_vars` and `raw_docstring`, and
/// an optional `bddkit_validate_step` (#55), moved once for the lot.
///
/// 4: the first generation of the range (#205). It adds nothing by itself;
/// every later optional key of the phase builds on it: a step spec's
/// `expression`, `reads`, `writes` and `sets_vars` (#206).
pub const ABI_VERSION: u32 = 4;

/// The oldest ABI generation this host still loads.
pub const ABI_MIN: u32 = 3;

/// A message used whenever a plugin reports failure without giving a reason.
const NO_MESSAGE: &str = "the plugin reported a failure with no message";

/// The loader must never silently treat a manifest declaring `PerWorker` as
/// `Shared`: that would hand an instance the plugin declared is *not*
/// thread-safe to several concurrent workers at once — a data race inside
/// someone else's cdylib, surfacing as a flaky suite.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Concurrency {
    /// One instance serves the whole run; the plugin guarantees its handle is
    /// safe to call from several workers at once.
    #[default]
    Shared,
    /// One instance per feature file: the plugin's state is never touched by
    /// two workers at once, so it may keep per-scenario state at any
    /// concurrency.
    ///
    /// The value set is closed on purpose: a scheduling mode the host does not
    /// implement cannot be degraded into one it does, so a new mode comes with
    /// an `ABI_VERSION` bump rather than with a tolerant parse.
    PerWorker,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub name: String,
    // Printed by `plugin show`; also makes a manifest without a version a
    // load failure.
    pub version: String,
    pub groups: Vec<String>,
    #[serde(default)]
    pub concurrency: Concurrency,
    /// What the `resources.<group>` body of each claimed group takes, keyed by
    /// group because one manifest may claim several. Optional and defaulted:
    /// a plugin that describes nothing stays fully supported, which is why
    /// `ABI_VERSION` did not move for it (it moved later, once, for #90).
    ///
    ///
    /// A key naming a group the manifest does not claim is a load error
    /// (`library::check_field_groups`), for the same reason an unclaimed step
    /// group is.
    #[serde(default)]
    pub fields: BTreeMap<String, Vec<ConfigField>>,
    /// The config body an instance of a group takes when the suite declares
    /// no `resources.<group>` section at all, keyed by group. The host then
    /// synthesizes one instance named `default` from it — validated at load
    /// like a declared one, and the group default by the one-instance rule.
    /// Any declared section, even an empty map, switches this off for that
    /// group. Same claim rule as `fields` (`library::check_implicit_groups`).
    #[serde(default)]
    pub implicit_instance: BTreeMap<String, serde_json::Value>,
}

/// One key of a group's instance config, as the plugin describes it.
///
/// Deliberately **not** JSON Schema: no nesting, no enums, no ranges, no
/// defaults, no validation semantics of any kind. The one thing declared
/// beyond the name is `type`, the FORM of a scalar, and it exists because the
/// host is the one turning a `resource add` flag's text into YAML and cannot
/// do that without knowing whether `false` is a boolean or a string. Anything
/// richer would make the host reason about whether a plugin's config is
/// ACCEPTABLE, which is the boundary `bddkit_validate_config` exists to keep —
/// and since the enforcing half already exists, a second, weaker copy in the
/// manifest could only ever disagree with it.
#[derive(Debug, Clone, Deserialize)]
pub struct ConfigField {
    pub name: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub example: Option<String>,
    /// The FORM of the value, never a constraint on it: `string`, `boolean`,
    /// `number` or `nonscalar`, absent meaning `string`. It says how
    /// `resource add` renders a flag's text into YAML — `false` as a boolean
    /// rather than as `'false'` — and nothing about whether the value is
    /// acceptable, which stays `bddkit_validate_config`'s sole business.
    ///
    /// Kept as text here so the mapping and its refusal live in one place
    /// (`config::Scalar::from_declared`, checked at load), and so a manifest
    /// declaring an unknown name fails with the name in the message rather
    /// than as a serde error about an untagged enum.
    #[serde(default, rename = "type")]
    pub value_type: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    Action,
    Assertion,
}

/// A `{param}` in a step effect.
static EFFECT_PLACEHOLDER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{([^{}]*)\}").expect("a constant"));

#[derive(Debug, Clone, Deserialize)]
pub struct StepSpec {
    /// A Rust regex. A step declares this or `expression`, exactly one of the
    /// two (`library::prepare_steps`).
    #[serde(default)]
    pub pattern: Option<String>,
    /// A Cucumber expression with bddkit's `{name:type}`, compiled by the
    /// host's one parser, so the step gets what a built-in gets.
    #[serde(default)]
    pub expression: Option<String>,
    pub group: String,
    pub kind: StepKind,
    /// The author's own one-line help, shown by `bddkit steps list`. Optional
    /// and defaulted, so a plugin published before the field existed still
    /// loads. The host never translates it: localizing a plugin is the
    /// plugin's job, not the host's.
    #[serde(default)]
    pub description: Option<String>,
    /// Capture groups of `pattern` that hold a secret, 1-based like the regex
    /// groups themselves (`[2]` is the second `(...)`). The host hides a
    /// literal one in every step text it shows, registers the value as it
    /// does for `{name:secret}`, and writes `***` for its slot in the stream.
    /// Optional and defaulted; an index no group has is a load error
    /// (`library::check_secret_groups`), and so is the key on an `expression`
    /// step, which writes `{name:secret}` instead.
    #[serde(default)]
    pub secret_groups: Vec<usize>,
    /// The dispatch request carries `vars` for this step, and for no other.
    #[serde(default)]
    pub wants_vars: bool,
    /// The host does not interpolate this step's doc string.
    #[serde(default)]
    pub raw_docstring: bool,
    /// State this step uses, in the plugin's own vocabulary, for `doctor`'s
    /// hints. All three effects are optional, and absent differs from empty: a
    /// step declaring none of them is unknown (`effects`).
    #[serde(default)]
    pub reads: Option<Vec<String>>,
    /// State this step leaves behind, in the same vocabulary as `reads`.
    #[serde(default)]
    pub writes: Option<Vec<String>>,
    /// Variables this step writes: `{name}`, a fixed name, or a trailing `*`
    /// for a family.
    #[serde(default)]
    pub sets_vars: Option<Vec<String>>,
    /// The compiled `expression`, set at load. Never sent by a plugin.
    #[serde(skip)]
    pub compiled: Option<Compiled>,
    /// By argument position, the name an effect may call it: a parameter's or
    /// a named capture group's, `None` for an unnamed or a secret one, and for
    /// every group of a pattern with `secret_groups`. Set at load. Never sent
    /// by a plugin.
    #[serde(skip)]
    pub arg_names: Vec<Option<String>>,
}

/// What a step uses and leaves behind, with its placeholders filled.
#[derive(Debug, PartialEq, Eq)]
pub struct Effects {
    pub reads: Vec<String>,
    pub sets_vars: Vec<String>,
}

impl StepSpec {
    pub fn is_assertion(&self) -> bool {
        matches!(self.kind, StepKind::Assertion)
    }

    /// The text the plugin declared the step with, for a message.
    pub fn declaration(&self) -> &str {
        self.expression
            .as_deref()
            .or(self.pattern.as_deref())
            .unwrap_or_default()
    }

    /// Every template of `reads`, `writes` and `sets_vars`.
    pub fn effect_templates(&self) -> impl Iterator<Item = &String> {
        [&self.reads, &self.writes, &self.sets_vars]
            .into_iter()
            .flatten()
            .flatten()
    }

    /// The effects of one use of this step, from its RAW arguments: `doctor`
    /// knows no variable's value (invariant 1). `None` when the step is
    /// unknown: it declares no effect at all, or a placeholder's value holds a
    /// `<<slot>>` or an Outline `<column>`, or is absent. A step that declares
    /// only `reads` or `writes` is known, and sets nothing.
    pub fn effects(&self, args: &[String]) -> Option<Effects> {
        if self.reads.is_none() && self.writes.is_none() && self.sets_vars.is_none() {
            return None;
        }
        let fill = |list: &Option<Vec<String>>| -> Option<Vec<String>> {
            list.iter()
                .flatten()
                .map(|template| self.fill(template, args))
                .collect()
        };
        fill(&self.writes)?;
        Some(Effects {
            reads: fill(&self.reads)?,
            sets_vars: fill(&self.sets_vars)?,
        })
    }

    fn fill(&self, template: &str, args: &[String]) -> Option<String> {
        let mut known = true;
        let filled = EFFECT_PLACEHOLDER.replace_all(template, |captures: &regex::Captures<'_>| {
            let value = self
                .arg_names
                .iter()
                .position(|name| name.as_deref() == Some(&captures[1]))
                .and_then(|index| args.get(index))
                .filter(|value| !value.is_empty() && !value.contains('<'));
            known &= value.is_some();
            value.cloned().unwrap_or_default()
        });
        known.then(|| filled.into_owned())
    }

    /// The `{param}` names one effect template holds.
    pub fn placeholders(template: &str) -> impl Iterator<Item = &str> {
        EFFECT_PLACEHOLDER
            .captures_iter(template)
            .filter_map(|captures| captures.get(1))
            .map(|name| name.as_str())
    }
}

/// The polling options the host resolved for this instance. Serialised for the
/// plugin's information only: the sleep loop itself always stays on the host.
#[derive(Debug, Clone, Serialize)]
pub struct PollingJson {
    pub timeout_secs: u64,
    pub interval_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct OptionsJson {
    pub polling: PollingJson,
}

impl From<&crate::options::Options> for OptionsJson {
    fn from(options: &crate::options::Options) -> Self {
        Self {
            polling: PollingJson {
                timeout_secs: options.polling.timeout.as_secs(),
                interval_ms: options.polling.interval.as_millis() as u64,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct InitRequest<'a> {
    pub group: &'a str,
    pub instance: &'a str,
    pub config: &'a serde_json::Value,
    pub options: OptionsJson,
}

#[derive(Debug, Clone, Serialize)]
pub struct DispatchRequest<'a> {
    pub args: &'a [String],
    pub docstring: Option<&'a String>,
    pub table: Option<&'a Vec<Vec<String>>>,
    pub artifacts_dir: String,
    /// The working directory of the feature file this call belongs to. Unlike
    /// `artifacts_dir` the host has already created it: it is one per file and
    /// shared with host steps, not fresh per dispatch.
    pub workspace_dir: String,
    pub debug: bool,
    pub options: OptionsJson,
    /// The run writes the evidence of a PASSING step too (`--evidence`), so a
    /// plugin may attach a screenshot nobody would read otherwise. Sent on
    /// every attempt of a polled assertion.
    pub evidence: bool,
    /// The file's variables, for a step whose spec says `wants_vars`; `{}`
    /// for every other step.
    pub vars: BTreeMap<String, VarJson>,
}

/// One variable as a plugin that asked for them sees it.
#[derive(Debug, Clone, Serialize)]
pub struct VarJson {
    /// `None` for a variable holding the `<<null>>` sentinel: a plugin never
    /// receives the sentinel's NUL bytes.
    pub value: Option<String>,
    /// Whether the host would mask the value (`Redactor::reveals`).
    pub secret: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Passed,
    NotYet,
    Fatal,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Diagnostic {
    pub title: String,
    /// `text | json | http | image`. Not an enum: an unknown kind from a newer
    /// plugin must degrade to "print it as text", never fail the parse.
    pub kind: String,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    /// The media type of the file at `path`. When absent the host guesses it
    /// from the file's extension.
    #[serde(default)]
    pub media_type: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DispatchResult {
    pub status: Status,
    #[serde(default)]
    pub vars: BTreeMap<String, String>,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
    /// Values the host adds to its filter, on every status: a failing reply
    /// is where a secret is most likely to be quoted. Subject to the same
    /// `privacy.min_length` as any other value.
    #[serde(default)]
    pub secrets: Vec<String>,
    #[serde(default)]
    pub error: Option<String>,
}

impl DispatchResult {
    /// A reply that does not parse as a whole may still have declared
    /// `secrets`, and the message that reports it quotes the raw reply. Read
    /// just that key so the failure can be masked like any other; `None` when
    /// it declared none, and the caller reports the parse error as it always
    /// did.
    pub fn salvage(reply: &str, message: String) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_str(reply).ok()?;
        let secrets: Vec<String> = value
            .get("secrets")?
            .as_array()?
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        (!secrets.is_empty()).then(|| Self {
            status: Status::Fatal,
            vars: BTreeMap::new(),
            diagnostics: Vec::new(),
            secrets,
            error: Some(message),
        })
    }

    /// Diagnostics are evidence on the failure path, so they have to end up
    /// inside the one string `report::render_file` writes. Printing them
    /// directly would land in the middle of another worker's dump.
    ///
    /// A `json` diagnostic is masked by name BEFORE it is joined in, while its
    /// positions still exist, and what it holds under a sensitive name joins the
    /// file's set. The rest of the text (the error, a text diagnostic) is masked
    /// by value by whoever prints the failure.
    pub fn render_failure(&self, redactor: &crate::privacy::Redactor) -> String {
        let mut out = self.error.clone().unwrap_or_else(|| NO_MESSAGE.to_string());
        for d in &self.diagnostics {
            out.push_str(&format!("\n\n--- {} ({}) ---", d.title, d.kind));
            if let Some(content) = &d.content {
                out.push('\n');
                if d.kind == "json" {
                    redactor.register(&redactor.json_hits(content));
                    out.push_str(&redactor.mask_json(content));
                } else {
                    out.push_str(content);
                }
            }
            if let Some(path) = &d.path {
                out.push_str(&format!("\n{path}"));
            }
        }
        out
    }
}

/// The reply shape for everything that is not a step: configuration validation
/// is not an assertion and must not be able to answer `not_yet`.
#[derive(Debug, Clone, Deserialize)]
pub struct Envelope {
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
}

impl Envelope {
    pub fn into_result(self) -> Result<(), String> {
        if self.ok {
            Ok(())
        } else {
            Err(self.error.unwrap_or_else(|| NO_MESSAGE.to_string()))
        }
    }
}

/// The reply of `bddkit_validate_config` and `bddkit_probe_config`: the
/// envelope plus the optional ABI 4 `detail`. `secrets` is read by
/// `Library::config_call` straight from the JSON, whatever `ok` says; `detail` is the one-line text of an `ok` instance's `doctor` row.
#[derive(Debug, Clone, Deserialize)]
pub struct ConfigReply {
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
}

impl ConfigReply {
    /// `Ok` carries the plugin's `detail`, if it gave one (an empty one is none).
    pub fn into_result(self) -> Result<Option<String>, String> {
        if self.ok {
            Ok(self.detail.filter(|detail| !detail.is_empty()))
        } else {
            Err(self.error.unwrap_or_else(|| NO_MESSAGE.to_string()))
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct InitResponse {
    pub ok: bool,
    #[serde(default)]
    pub handle: Option<u64>,
    #[serde(default)]
    pub error: Option<String>,
}

impl InitResponse {
    pub fn into_result(self) -> Result<u64, String> {
        if self.ok {
            self.handle
                .ok_or_else(|| "the plugin reported ok with no handle".to_string())
        } else {
            Err(self
                .error
                .unwrap_or_else(|| "the plugin refused to create the instance".to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_step_spec_defaults_both_new_flags_to_false() {
        let spec: StepSpec =
            serde_json::from_str(r#"{"pattern":"^x$","group":"g","kind":"action"}"#).unwrap();
        assert!(!spec.wants_vars);
        assert!(!spec.raw_docstring);
    }

    fn spec_with_args(json: &str, names: &[Option<&str>]) -> StepSpec {
        let mut spec: StepSpec = serde_json::from_str(json).expect("step parses");
        spec.arg_names = names.iter().map(|name| name.map(str::to_string)).collect();
        spec
    }

    #[test]
    fn a_step_with_no_effect_is_unknown_and_one_with_any_effect_is_known() {
        let bare = spec_with_args(r#"{"pattern":"^x$","group":"g","kind":"action"}"#, &[]);
        assert_eq!(bare.effects(&[]), None);
        for key in ["reads", "writes", "sets_vars"] {
            let spec = spec_with_args(
                &format!(r#"{{"pattern":"^x$","group":"g","kind":"action","{key}":[]}}"#),
                &[],
            );
            let effects = spec.effects(&[]).expect("declared, so known");
            assert!(effects.sets_vars.is_empty(), "{key}");
        }
    }

    #[test]
    fn effects_are_filled_from_the_raw_arguments() {
        let spec = spec_with_args(
            r#"{"pattern":"^x$","group":"g","kind":"action","reads":["process:{p}"],
                "writes":["w"],"sets_vars":["fixed","{name}","{name}_*"]}"#,
            &[Some("p"), None, Some("name")],
        );
        let args = |p: &str, name: &str| vec![p.to_string(), "other".to_string(), name.to_string()];
        assert_eq!(
            spec.effects(&args("a", "n")),
            Some(Effects {
                reads: vec!["process:a".to_string()],
                sets_vars: vec!["fixed".to_string(), "n".to_string(), "n_*".to_string()],
            })
        );
        // A value not known statically, wherever it sits, makes the step unknown.
        assert_eq!(spec.effects(&args("<<slot>>", "n")), None);
        assert_eq!(spec.effects(&args("a", "<column>")), None);
        assert_eq!(spec.effects(&args("a", "")), None);
        assert_eq!(spec.effects(&["a".to_string()]), None);

        let writes = spec_with_args(
            r#"{"pattern":"^x$","group":"g","kind":"action","writes":["w:{p}"]}"#,
            &[Some("p")],
        );
        assert!(writes.effects(&["a".to_string()]).is_some());
        assert_eq!(writes.effects(&["<<slot>>".to_string()]), None);
    }

    #[test]
    fn a_manifest_parses() {
        let m: Manifest = serde_json::from_str(
            r#"{"name":"widget","version":"1.2.0","groups":["widget"],"concurrency":"shared"}"#,
        )
        .expect("manifest parses");
        assert_eq!(m.name, "widget");
        assert_eq!(m.groups, vec!["widget".to_string()]);
        assert_eq!(m.concurrency, Concurrency::Shared);
    }

    #[test]
    fn concurrency_defaults_to_shared() {
        // The field enters the ABI now so adding it later cannot break every
        // published plugin; a plugin that omits it means "shared".
        let m: Manifest = serde_json::from_str(r#"{"name":"x","version":"0.1.0","groups":["x"]}"#)
            .expect("manifest parses");
        assert_eq!(m.concurrency, Concurrency::Shared);
    }

    #[test]
    fn a_step_spec_parses_both_kinds() {
        let steps: Vec<StepSpec> = serde_json::from_str(
            r#"[{"pattern":"^a$","group":"widget","kind":"action"},
                {"pattern":"^b$","group":"widget","kind":"assertion"}]"#,
        )
        .expect("steps parse");
        assert!(!steps[0].is_assertion());
        assert!(steps[1].is_assertion());
        assert_eq!(steps[0].group, "widget");
    }

    #[test]
    fn a_step_description_is_optional_in_both_directions() {
        // Plugins published before the field existed must keep loading — they
        // simply list without a description — which is why this is additive
        // and `ABI_VERSION` did not move for it (generation 2 moved once, for #90).
        let plain: Vec<StepSpec> =
            serde_json::from_str(r#"[{"pattern":"^a$","group":"widget","kind":"action"}]"#)
                .expect("steps parse");
        assert!(plain[0].description.is_none());

        let described: Vec<StepSpec> = serde_json::from_str(
            r#"[{"pattern":"^a$","group":"widget","kind":"action","description":"does a"}]"#,
        )
        .expect("steps parse");
        assert_eq!(described[0].description.as_deref(), Some("does a"));
    }

    #[test]
    fn a_dispatch_result_parses_with_defaults() {
        let r: DispatchResult =
            serde_json::from_str(r#"{"status":"passed"}"#).expect("result parses");
        assert_eq!(r.status, Status::Passed);
        assert!(r.vars.is_empty());
        assert!(r.diagnostics.is_empty());
        assert!(r.error.is_none());
    }

    #[test]
    fn diagnostics_render_into_the_failure_text() {
        // The report layer composes a file's whole output into ONE string, so
        // plugin evidence has to arrive as text on the error path, not as a
        // separate print (invariant 6 + the atomic per-file dump).
        let r = DispatchResult {
            status: Status::Fatal,
            vars: Default::default(),
            diagnostics: vec![
                Diagnostic {
                    title: "PUT /b/o".into(),
                    kind: "http".into(),
                    content: Some("403 Forbidden".into()),
                    path: None,
                    media_type: None,
                },
                Diagnostic {
                    title: "Screenshot".into(),
                    kind: "image".into(),
                    content: None,
                    path: Some("/run/artifacts/7/fail.png".into()),
                    media_type: None,
                },
            ],
            secrets: Vec::new(),
            error: Some("access denied".into()),
        };
        let text = r.render_failure(&crate::privacy::Redactor::disabled());
        assert!(text.contains("access denied"), "{text}");
        assert!(text.contains("PUT /b/o"), "{text}");
        assert!(text.contains("403 Forbidden"), "{text}");
        assert!(text.contains("/run/artifacts/7/fail.png"), "{text}");
    }

    #[test]
    fn a_malformed_reply_still_yields_the_secrets_it_declared() {
        let reply = r#"{"status":"fatal","secrets":["tok-123456"],"diagnostics":[{"kind":"x"}]}"#;
        assert!(serde_json::from_str::<DispatchResult>(reply).is_err());
        let salvaged = DispatchResult::salvage(reply, "malformed".into()).expect("secrets kept");
        assert_eq!(salvaged.secrets, vec!["tok-123456".to_string()]);
        assert_eq!(salvaged.status, Status::Fatal);
        assert!(DispatchResult::salvage(r#"{"status":1}"#, "m".into()).is_none());
        assert!(DispatchResult::salvage("not json", "m".into()).is_none());
    }

    #[test]
    fn an_envelope_carries_the_error() {
        let e: Envelope = serde_json::from_str(r#"{"ok":false,"error":"bucket is required"}"#)
            .expect("envelope parses");
        assert_eq!(e.into_result().unwrap_err(), "bucket is required");
    }

    #[test]
    fn an_envelope_without_a_message_still_fails() {
        // A plugin that returns ok:false with no message must not be reported
        // as a success, and must not panic the host either.
        let e: Envelope = serde_json::from_str(r#"{"ok":false}"#).expect("envelope parses");
        assert!(e.into_result().is_err());
    }

    #[test]
    fn an_init_response_yields_a_handle() {
        let r: InitResponse =
            serde_json::from_str(r#"{"ok":true,"handle":7}"#).expect("init response parses");
        assert_eq!(r.into_result().expect("ok"), 7);
    }

    #[test]
    fn an_init_response_ok_with_no_handle_is_an_error() {
        // A plugin that forgets `handle` must fail loudly at init, not hand
        // back handle 0 and let a later step fail confusingly instead.
        let r: InitResponse = serde_json::from_str(r#"{"ok":true}"#).expect("init response parses");
        let err = r.into_result().unwrap_err();
        assert!(err.contains("handle"), "{err}");
    }

    #[test]
    fn a_dispatch_request_serialises_the_documented_keys() {
        // These key names are the half of the contract plugin authors parse:
        // renaming one is a breaking change no deserialise-side test would catch.
        let args = vec!["report.pdf".to_string()];
        let request = DispatchRequest {
            args: &args,
            docstring: None,
            table: None,
            artifacts_dir: "/tmp/run/0007".into(),
            workspace_dir: "/tmp/run/workspace/000000".into(),
            debug: false,
            options: OptionsJson::from(&crate::options::Options::default()),
            evidence: true,
            vars: BTreeMap::new(),
        };
        let value = serde_json::to_value(&request).expect("serialises");
        // `serde_json::Value::Object` is a `BTreeMap` (no `preserve_order`
        // feature here), so keys come back alphabetical, not in field order —
        // sort both sides to pin the key SET without depending on that.
        let mut keys: Vec<&String> = value.as_object().expect("an object").keys().collect();
        keys.sort();
        let mut expected = [
            "args",
            "docstring",
            "table",
            "artifacts_dir",
            "workspace_dir",
            "debug",
            "options",
            "evidence",
            "vars",
        ];
        expected.sort();
        assert_eq!(keys, expected);
        assert!(
            value["docstring"].is_null(),
            "an absent docstring stays present as null, as the documented payload shows"
        );
        assert_eq!(value["options"]["polling"]["interval_ms"], 100);
        assert_eq!(value["evidence"], true);
    }

    #[test]
    fn the_evidence_additions_are_optional_in_what_a_plugin_sends() {
        let step: StepSpec = serde_json::from_str(
            r#"{"pattern":"^a \"(.*)\" \"(.*)\"$","group":"w","kind":"action","secret_groups":[2]}"#,
        )
        .expect("step parses");
        assert_eq!(step.secret_groups, vec![2]);
        let plain: StepSpec =
            serde_json::from_str(r#"{"pattern":"^a$","group":"w","kind":"action"}"#).unwrap();
        assert!(plain.secret_groups.is_empty());

        let reply: DispatchResult = serde_json::from_str(
            r#"{"status":"fatal","secrets":["s3cr3t-value"],
                "diagnostics":[{"title":"t","kind":"image","path":"a.bin","media_type":"image/png"}]}"#,
        )
        .expect("reply parses");
        assert_eq!(reply.secrets, vec!["s3cr3t-value".to_string()]);
        assert_eq!(
            reply.diagnostics[0].media_type.as_deref(),
            Some("image/png")
        );
        let bare: DispatchResult = serde_json::from_str(r#"{"status":"passed"}"#).unwrap();
        assert!(bare.secrets.is_empty());
    }

    #[test]
    fn an_init_request_serialises_the_documented_keys_and_config_verbatim() {
        let config = serde_json::json!({"bucket": "acme-uploads", "region": "eu-west-1"});
        let request = InitRequest {
            group: "widget",
            instance: "primary",
            config: &config,
            options: OptionsJson::from(&crate::options::Options::default()),
        };
        let value = serde_json::to_value(&request).expect("serialises");
        let mut keys: Vec<&String> = value.as_object().expect("an object").keys().collect();
        keys.sort();
        let mut expected = ["group", "instance", "config", "options"];
        expected.sort();
        assert_eq!(keys, expected);
        assert_eq!(value["config"], config);
    }

    #[test]
    fn an_unknown_field_from_a_newer_plugin_is_ignored() {
        let manifest: Manifest = serde_json::from_str(
            r#"{"name":"widget","version":"1.0.0","groups":["widget"],"capabilities":["streaming"]}"#,
        )
        .expect("a newer plugin's extra keys do not break an older host");
        assert_eq!(manifest.name, "widget");
    }

    #[test]
    fn a_manifest_declares_the_config_fields_of_each_group() {
        // The host prints these and interprets none of them: `fields` is
        // documentation the plugin owns, never a second, weaker copy of the
        // check `validate_config` already enforces.
        let m: Manifest = serde_json::from_str(
            r#"{"name":"s3","version":"1.2.0","groups":["s3"],"fields":{
                "s3":[{"name":"bucket","required":true,
                       "description":"bucket the steps read and write","example":"acceptance"},
                      {"name":"endpoint","description":"S3-compatible endpoint; omit for AWS"}]}}"#,
        )
        .expect("manifest parses");
        let s3 = &m.fields["s3"];
        assert_eq!(s3[0].name, "bucket");
        assert!(s3[0].required);
        assert_eq!(
            s3[0].description.as_deref(),
            Some("bucket the steps read and write")
        );
        assert_eq!(s3[0].example.as_deref(), Some("acceptance"));
        assert_eq!(s3[1].name, "endpoint");
        assert!(!s3[1].required, "required defaults to false");
        assert!(s3[1].example.is_none());
    }

    #[test]
    fn a_manifest_without_fields_parses_to_an_empty_map() {
        // Additive: `ABI_VERSION` did not move for it. A plugin published before
        // the key existed keeps loading and simply describes nothing.
        let m: Manifest = serde_json::from_str(r#"{"name":"x","version":"0.1.0","groups":["x"]}"#)
            .expect("manifest parses");
        assert!(m.fields.is_empty());
    }
}
