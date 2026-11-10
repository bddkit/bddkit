use crate::options::{Options, OptionsLayer};
use crate::privacy;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn default_concurrency() -> usize {
    8
}

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,
    /// `fail_fast: true`: the suite's default for `--fail-fast`.
    #[serde(default)]
    pub fail_fast: bool,
    /// `tags: [smoke]`: the suite's default for `--tag`; a flag replaces it.
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub macro_paths: Vec<PathBuf>,
    pub paths: Vec<PathBuf>,
    pub resources: Resources,
    #[serde(default)]
    pub options: OptionsLayer,
    #[serde(skip)]
    pub effective_options: Options,
    /// The config layer of the privacy vocabulary; `privacy::Privacy::resolve`
    /// folds it with the environment and the flags.
    #[serde(default)]
    pub privacy: crate::privacy::PrivacyLayer,
    /// `evidence: [marked, failures]`: the suite's default sources, used only
    /// when the run has a stream and neither the flag nor the variable is set.
    #[serde(default)]
    pub evidence: Option<crate::events::Sources>,
    /// `telemetry: false`: the suite's decision about anonymous usage counts,
    /// which `bddkit init` writes. See `telemetry::decide` for what outranks it.
    #[serde(default)]
    pub telemetry: Option<bool>,
    /// Everything the host does not name itself: `default_<group>` keys for
    /// plugin groups. Captured rather than rejected — a group's default is
    /// spelled the same way as `default_api`, and the host cannot know the
    /// group names before the plugins are loaded.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_yaml_ng::Value>,
    #[serde(skip)]
    pub plugin_instances: Vec<InstanceSpec>,
}

/// Resources under test. Not tied to a test set: any scenario can reach any
/// of them by name.
#[derive(Debug, Deserialize)]
pub struct Resources {
    /// Groups the host does not serve. The value is kept verbatim: validating
    /// `bucket` is the plugin's job, the host has no schema for it.
    #[serde(flatten)]
    pub groups: BTreeMap<String, BTreeMap<String, serde_yaml_ng::Value>>,
}

/// One declared instance of a plugin group, after the host has taken its
/// reserved `options` key out and resolved it.
#[derive(Debug, Clone)]
pub struct InstanceSpec {
    pub group: String,
    pub name: String,
    pub config: serde_json::Value,
    pub options: Options,
}

/// The YAML type a `resource add` flag's string value has to become for one
/// key. Flag values are always strings; this is the table that converts them,
/// so nothing is ever inferred from the shape of a value.
///
/// No host field is a boolean, but a plugin's may be, and a plugin says so in
/// its manifest — see `Scalar::from_declared`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// `NonScalar` ends in the enum's own name only because "not a scalar" is the
// accurate word for it — renaming it to dodge the lint would make the type
// less clear, not more.
#[allow(clippy::enum_variant_names)]
pub enum Scalar {
    Str,
    Bool,
    Num,
    /// A map or a list: no flag can express it, and `--json` is how it is set.
    NonScalar,
}

impl Scalar {
    /// The names a plugin's manifest may put in a field's `type`, and the only
    /// place they are mapped. `None` is a name the host does not know, which
    /// `plugin::library` refuses at load: degrading it into a string would
    /// reproduce exactly the failure the key exists to prevent — a boolean key
    /// written as `'false'`, refused by the plugin's own `validate_config`, on
    /// every input, forever.
    pub fn from_declared(name: &str) -> Option<Self> {
        match name {
            "string" => Some(Scalar::Str),
            "boolean" => Some(Scalar::Bool),
            "number" => Some(Scalar::Num),
            "nonscalar" => Some(Scalar::NonScalar),
            _ => None,
        }
    }

    /// The same names, for an error message and for `resource fields`.
    pub fn declared_name(&self) -> &'static str {
        match self {
            Scalar::Str => "string",
            Scalar::Bool => "boolean",
            Scalar::Num => "number",
            Scalar::NonScalar => "nonscalar",
        }
    }

    pub const DECLARABLE: &'static str = "string, boolean, number, nonscalar";
}

impl Config {
    pub(crate) fn resolve_options(&mut self) -> Result<()> {
        let global = Options::default()
            .apply(&self.options)
            .map_err(anyhow::Error::msg)?;
        self.effective_options = global.clone();
        self.plugin_instances.clear();
        for (group, instances) in &self.resources.groups {
            for (name, body) in instances {
                let where_ = format!("resources.{group}.{name}");
                let (config, layer) = split_instance_options(body)
                    .map_err(|error| anyhow::anyhow!("{where_}: {error}"))?;
                let options = global
                    .apply(&layer)
                    .map_err(|error| anyhow::anyhow!("{where_}: {error}"))?;
                self.plugin_instances.push(InstanceSpec {
                    group: group.clone(),
                    name: name.clone(),
                    config,
                    options,
                });
            }
        }
        Ok(())
    }

    pub fn group_names(&self) -> impl Iterator<Item = &String> {
        self.resources.groups.keys()
    }

    /// The groups whose `BDDKIT_DEFAULT_<GROUP>` variable is read: `api`, `db`
    /// and `srp` (they had built-in flags and variables, which keep working),
    /// then the plugin groups the config declares.
    pub fn all_group_names(&self) -> Vec<String> {
        let mut names: Vec<String> = ["api", "db", "srp"].map(String::from).into();
        for group in self.group_names() {
            if !names.contains(group) {
                names.push(group.clone());
            }
        }
        names
    }

    /// Sets a group's default as `default_<group>` would, from a flag or a
    /// variable. A group nothing declares is refused, and a plugin group's name
    /// is checked here.
    pub fn set_default(&mut self, group: &str, name: &str) -> Result<()> {
        if !self.resources.groups.contains_key(group) {
            bail!(
                "a default was given for the resource group {group:?}, but this config declares no resources.{group} (groups declared here: {})",
                self.group_names().cloned().collect::<Vec<_>>().join(", ")
            );
        }
        self.extra.insert(
            format!("default_{group}"),
            serde_yaml_ng::Value::String(name.to_string()),
        );
        self.resolve_default_group(group)?;
        Ok(())
    }

    /// The plugin groups with at least one instance: an empty
    /// `resources.<group>: {}` asks nothing of a plugin, so nothing is
    /// installed for it.
    pub fn groups_in_use(&self) -> Vec<String> {
        self.resources
            .groups
            .iter()
            .filter(|(_, instances)| !instances.is_empty())
            .map(|(group, _)| group.clone())
            .collect()
    }

    /// Same rule as `default_api`: an explicit name must be declared, a lone
    /// instance is the default on its own, several without an explicit choice
    /// is a startup error.
    pub fn resolve_default_group(&self, group: &str) -> Result<Option<String>> {
        let field = format!("default_{group}");
        let explicit = match self.extra.get(&field) {
            Some(serde_yaml_ng::Value::String(name)) => Some(name.clone()),
            Some(other) => bail!("{field} must be a string, got {other:?}"),
            None => None,
        };
        let empty = BTreeMap::new();
        let names = self.resources.groups.get(group).unwrap_or(&empty).keys();
        resolve_default(&field, &format!("resources.{group}"), &explicit, names)
    }

    /// `extra` keeps every top-level key the host does not name itself, because
    /// a plugin group's default is spelled exactly like `default_api` and the
    /// group names are unknown at parse time. That makes a typo — `default_widgte`
    /// for `default_widget` — a key nothing ever reads, which is the silent no-op
    /// the config layer refuses everywhere else. Checked once the groups are
    /// known, i.e. after the plugins have loaded.
    ///
    /// With no plugin loaded only `default_api`, `default_db` and `default_srp`
    /// are checked: they named built-in kinds, which refused them when nothing
    /// was declared, so a stray one stays an error. Any other `default_*` is
    /// then the unknown key `Config` has always tolerated.
    pub fn check_group_defaults(&self, plugins_loaded: bool) -> Result<()> {
        for key in self.extra.keys() {
            let Some(group) = key.strip_prefix("default_") else {
                continue;
            };
            if !plugins_loaded && !matches!(group, "api" | "db" | "srp") {
                continue;
            }
            if self.resources.groups.contains_key(group) {
                continue;
            }
            let declared: Vec<&str> = self.resources.groups.keys().map(String::as_str).collect();
            bail!(
                "{key} names the resource group {group:?}, but this config declares no resources.{group} (groups declared here: {})",
                if declared.is_empty() {
                    "none".to_string()
                } else {
                    declared.join(", ")
                }
            );
        }
        Ok(())
    }
}

/// Shared logic for every `default_*`: an explicit name is checked for existence,
/// a single resource becomes the default on its own, several without an
/// explicit one is an error.
fn resolve_default<'a>(
    field: &str,
    section: &str,
    explicit: &Option<String>,
    mut names: impl Iterator<Item = &'a String> + Clone,
) -> Result<Option<String>> {
    if let Some(name) = explicit {
        if names.clone().any(|n| n == name) {
            return Ok(Some(name.clone()));
        }
        bail!("{field} references an undeclared resource {name:?} (not in {section})");
    }
    let first = names.next();
    match (first, names.next()) {
        (None, _) => Ok(None),
        (Some(only), None) => Ok(Some(only.clone())),
        (Some(_), Some(_)) => bail!(
            "{field} is not set, and {section} declares several resources — specify which one is the default"
        ),
    }
}

/// Splits a plugin instance body into the opaque config the plugin sees and the
/// host's reserved `options` layer. `deny_unknown_fields` on `OptionsLayer` is
/// what turns `pollng:` into a startup error instead of a silent no-op.
fn split_instance_options(
    body: &serde_yaml_ng::Value,
) -> std::result::Result<(serde_json::Value, OptionsLayer), String> {
    let serde_yaml_ng::Value::Mapping(mapping) = body else {
        return Err("an instance must be a mapping".to_string());
    };
    let mut mapping = mapping.clone();
    let layer = match mapping.remove("options") {
        Some(value) => serde_yaml_ng::from_value::<OptionsLayer>(value)
            .map_err(|error| format!("invalid options: {error}"))?,
        None => OptionsLayer::default(),
    };
    let config = serde_json::to_value(serde_yaml_ng::Value::Mapping(mapping))
        .map_err(|error| format!("instance config is not representable as JSON: {error}"))?;
    Ok((config, layer))
}

/// Reads one `.env` file into `map`; later keys (from later layers)
/// overwrite earlier ones. A missing file is not an error: in the
/// `.env`/`.env.local`/`.env.$APP_ENV`/`.env.$APP_ENV.local` stack, usually
/// not all four exist.
fn load_env_file(path: &Path, map: &mut BTreeMap<String, String>) -> Result<()> {
    match dotenvy::from_path_iter(path) {
        Ok(iter) => {
            for item in iter {
                let (k, v) = item.map_err(|error| match error {
                    // dotenvy's own message quotes the whole line, which is a
                    // `NAME=secret` assignment: name the place and nothing else.
                    dotenvy::Error::LineParse(_, at) => anyhow::anyhow!(
                        "failed to parse {}: a line is malformed (error at character {at} of it)",
                        path.display()
                    ),
                    other => anyhow::Error::new(other)
                        .context(format!("failed to parse {}", path.display())),
                })?;
                map.insert(k, v);
            }
            Ok(())
        }
        Err(dotenvy::Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("failed to read {}", path.display())),
    }
}

/// APP_ENV resolution priority: explicit CLI flag > the process's real
/// environment variable > APP_ENV value from the base `.env` > "dev".
/// The real APP_ENV takes priority over `.env`, but NOT over the CLI flag —
/// `--env` exists specifically to override it.
fn resolve_app_env(cli_env: Option<&str>, base_map: &BTreeMap<String, String>) -> String {
    resolve_app_env_with_process_env(cli_env, std::env::var("APP_ENV").ok(), base_map)
}

fn resolve_app_env_with_process_env(
    cli_env: Option<&str>,
    process_env: Option<String>,
    base_map: &BTreeMap<String, String>,
) -> String {
    cli_env
        .map(str::to_string)
        .or(process_env)
        .or_else(|| base_map.get("APP_ENV").cloned())
        .unwrap_or_else(|| "dev".to_string())
}

/// The full `.env` layer stack, in increasing priority order: a value from a
/// later layer overwrites an earlier one. A skipped file simply contributes
/// nothing — see `load_env_file`.
fn load_env_layers(config_dir: &Path, app_env: &str) -> Result<BTreeMap<String, String>> {
    let mut map = BTreeMap::new();
    load_env_file(&config_dir.join(".env"), &mut map)?;
    load_env_file(&config_dir.join(".env.local"), &mut map)?;
    load_env_file(&config_dir.join(format!(".env.{app_env}")), &mut map)?;
    load_env_file(&config_dir.join(format!(".env.{app_env}.local")), &mut map)?;
    Ok(map)
}

/// Expands environment variables docker-compose style:
/// `$VAR`/`${VAR}` (error if unset), `${VAR:-def}` (value, or def if
/// unset/empty), `${VAR-def}` (def only if unset), `${VAR:?msg}`/`${VAR?msg}`
/// (error with message msg, symmetric to `-`), `${VAR:+alt}`/`${VAR+alt}`
/// (alt if set, symmetric to `-`), `$$` as a literal `$`. Value lookup: the
/// process's real environment variable, then `env_map` (the `.env` layers) —
/// the real one always wins.
///
/// ponytail: `arg` (the default/alt/msg text) is taken as-is, without
/// recursively expanding `${...}` inside it (docker compose can do this,
/// this project's configs never needed it). Upgrade if needed: recursively
/// run `expand_env` over the extracted `arg` before use.
fn expand_env(src: &str, env_map: &BTreeMap<String, String>) -> Result<String> {
    let re = regex::Regex::new(
        r"\$\$|\$\{(?P<name>[A-Za-z_][A-Za-z0-9_]*)(?:(?P<op>:-|-|:\?|\?|:\+|\+)(?P<arg>[^}]*))?\}|\$(?P<bare>[A-Za-z_][A-Za-z0-9_]*)",
    )
    .expect("constant regex");

    let lookup = |name: &str| -> Option<String> {
        std::env::var(name)
            .ok()
            .or_else(|| env_map.get(name).cloned())
    };

    let mut out = String::with_capacity(src.len());
    let mut last = 0usize;
    for c in re.captures_iter(src) {
        let m = c.get(0).expect("group 0 always exists");
        out.push_str(&src[last..m.start()]);
        last = m.end();

        if m.as_str() == "$$" {
            out.push('$');
            continue;
        }

        let name = c
            .name("name")
            .or_else(|| c.name("bare"))
            .expect("either name or bare — otherwise the regex would not have matched")
            .as_str();
        let op = c.name("op").map(|o| o.as_str());
        let arg = c.name("arg").map(|a| a.as_str()).unwrap_or("");
        let value = lookup(name);

        let resolved = match op {
            None => match value {
                Some(v) => v,
                None => {
                    bail!("environment variable {name} is not set but is referenced in the config")
                }
            },
            Some(":-") => match value {
                Some(v) if !v.is_empty() => v,
                _ => arg.to_string(),
            },
            Some("-") => value.unwrap_or_else(|| arg.to_string()),
            Some(":?") => match value {
                Some(v) if !v.is_empty() => v,
                _ => bail!("{}", default_missing_message(name, arg)),
            },
            Some("?") => match value {
                Some(v) => v,
                None => bail!("{}", default_missing_message(name, arg)),
            },
            Some(":+") => match value {
                Some(v) if !v.is_empty() => arg.to_string(),
                _ => String::new(),
            },
            Some("+") => match value {
                Some(_) => arg.to_string(),
                None => String::new(),
            },
            Some(other) => {
                unreachable!("the regex only captures known operators, got {other:?}")
            }
        };
        out.push_str(&resolved);
    }
    out.push_str(&src[last..]);
    Ok(out)
}

/// Error message for `:?`/`?`: the custom text if given, otherwise the
/// standard wording (symmetric to the message for bare `${VAR}`).
fn default_missing_message(name: &str, custom: &str) -> String {
    if custom.is_empty() {
        format!("environment variable {name} is not set but is referenced in the config")
    } else {
        custom.to_string()
    }
}

/// Which `.env` layer a load from this path would select. `doctor` states it
/// in its header — it is the first thing that is wrong when a suite passes
/// locally and fails in CI — and `load` below settles it the same way, so the
/// two can never name different layers.
///
/// path.parent() returns Some("") for a bare file name (not None) — join()
/// with "" gives a path relative to cwd, which is what the normal case wants;
/// unwrap_or here guards the case of a path with no parent at all
/// (root/prefix), not the main mechanism.
pub fn app_env_for(path: &Path, cli_env: Option<&str>) -> Result<String> {
    app_env_in(path.parent().unwrap_or_else(|| Path::new(".")), cli_env)
}

/// The same answer from the directory alone, for a caller holding text rather
/// than a path.
fn app_env_in(config_dir: &Path, cli_env: Option<&str>) -> Result<String> {
    let mut base_map = BTreeMap::new();
    load_env_file(&config_dir.join(".env"), &mut base_map)?;
    Ok(resolve_app_env(cli_env, &base_map))
}

/// Where a resolved config path came from — `doctor` states it next to the
/// path, the same way it states the `.env` layer, and as its own field in
/// `--json` rather than folded into the path string a consumer would have to
/// parse back apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigSource {
    Flag,
    Env,
    Discovered,
}

impl ConfigSource {
    pub fn label(&self) -> &'static str {
        match self {
            ConfigSource::Flag => "--config",
            ConfigSource::Env => "BDDKIT_CONFIG",
            ConfigSource::Discovered => "found in working directory",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedConfig {
    pub path: PathBuf,
    pub source: ConfigSource,
}

/// Resolution order shared by every command that takes `--config`: the flag
/// itself, then `$BDDKIT_CONFIG`, then `bddkit.yaml`/`bddkit.yml` in the
/// working directory. `None` means nothing was found anywhere — the caller
/// decides what that means for its own exit code. An explicit flag or
/// environment variable is never checked for existence here: a missing file
/// is `load`'s own "failed to read config" error, not a silent fallback to
/// the default.
pub fn resolve_config_path(explicit: Option<&Path>) -> Result<Option<ResolvedConfig>> {
    let env_path = std::env::var_os("BDDKIT_CONFIG").map(PathBuf::from);
    resolve_config_path_in(explicit, env_path.as_deref(), Path::new("."))
}

/// `resolve_config_path` step 4: nothing was named and nothing was found.
/// Shared verbatim by every caller (`run`, `doctor`, `resource add`) so they
/// can never name the candidate files differently from one another.
pub(crate) const NO_CONFIG_FOUND: &str = "no config found: looked for ./bddkit.yaml and \
./bddkit.yml in the working directory; pass --config or set BDDKIT_CONFIG";

fn resolve_config_path_in(
    explicit: Option<&Path>,
    env_path: Option<&Path>,
    cwd: &Path,
) -> Result<Option<ResolvedConfig>> {
    if let Some(path) = explicit {
        return Ok(Some(ResolvedConfig {
            path: path.to_path_buf(),
            source: ConfigSource::Flag,
        }));
    }
    if let Some(path) = env_path {
        return Ok(Some(ResolvedConfig {
            path: path.to_path_buf(),
            source: ConfigSource::Env,
        }));
    }
    let yaml = cwd.join("bddkit.yaml");
    let yml = cwd.join("bddkit.yml");
    match (yaml.is_file(), yml.is_file()) {
        (true, true) => bail!(
            "both {} and {} exist; keep one, or pass --config to pick",
            yaml.display(),
            yml.display()
        ),
        (true, false) => Ok(Some(ResolvedConfig {
            path: yaml,
            source: ConfigSource::Discovered,
        })),
        (false, true) => Ok(Some(ResolvedConfig {
            path: yml,
            source: ConfigSource::Discovered,
        })),
        (false, false) => Ok(None),
    }
}

pub fn load(path: &Path, cli_env: Option<&str>) -> Result<Config> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read config {}", path.display()))?;
    load_str(
        &raw,
        path.parent().unwrap_or_else(|| Path::new(".")),
        cli_env,
    )
    // The migration hint belongs to a file somebody wrote, not to text a
    // command just assembled: `resource add` validates its prospective config
    // through `load_str`, and telling whoever adds a resource that "the format
    // changed" would be answering a question they did not ask, about a file
    // that is very likely fine.
    .with_context(|| {
        format!(
            "config {}. If this suite predates the format change, suites were replaced by \
             paths + resources — see docs/writing-tests.md",
            path.display()
        )
    })
}

/// Everything `load` does except reading the file. `config_dir` is what selects
/// the `.env` layers and is the config's own directory, not the working
/// directory — so `resource add` can validate the exact text it is about to
/// write with the layers a real run would see.
pub fn load_str(raw: &str, config_dir: &Path, cli_env: Option<&str>) -> Result<Config> {
    let app_env = app_env_in(config_dir, cli_env)?;
    let env_map = load_env_layers(config_dir, &app_env)?;
    let expanded = expand_env(raw, &env_map)?;
    // Before the typed parse, whose errors can quote a value: a secret the
    // document holds is recorded the moment it is known.
    // A syntax error names a position and quotes nothing; a type error
    // (`invalid type: string "…"`) does quote, which is why this comes first.
    if let Ok(document) = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&expanded) {
        record_secrets(&document);
    }
    let mut cfg: Config = serde_yaml_ng::from_str(&expanded).context("failed to parse config")?;
    cfg.resolve_options()?;
    Ok(cfg)
}

/// The vocabulary a config selects, under the flags and the environment this
/// process has; the defaults when the section or the sources are unusable —
/// the typed parse and `run` report those properly.
fn resolved_privacy(layer: &privacy::PrivacyLayer) -> privacy::Privacy {
    privacy::Privacy::from_sources(Some(layer), &privacy::process_env, privacy::flags())
        .unwrap_or_default()
}

fn record_secrets(document: &serde_yaml_ng::Value) {
    let layer = document
        .get("privacy")
        .and_then(|section| serde_yaml_ng::from_value(section.clone()).ok())
        .unwrap_or_default();
    let privacy = resolved_privacy(&layer);
    privacy::record(&privacy, privacy::secrets_in(document, &privacy.fields));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sink is process-wide and only ever grows, so each test uses values
    /// no other test writes. Never load a config here that sets `reveal` or a
    /// large `min_length`: `record` overwrites both for every test in the process.
    fn load_api_with(body: &str) {
        let src =
            format!("paths: [f]\nresources:\n  api:\n    a:\n      base_url: http://x\n{body}");
        load_str(&src, Path::new("."), None).expect("config loads");
    }

    #[test]
    fn a_url_password_and_a_field_word_value_are_recorded_after_expansion() {
        unsafe { std::env::set_var("BDDKIT_TEST_DB_PW", "from-env-pw-3") };
        load_api_with(
            "      password: literal-pw-4\ndb_url: postgres://app:${BDDKIT_TEST_DB_PW}@db/app\n",
        );
        assert_eq!(privacy::mask_text("x from-env-pw-3 y"), "x *** y");
        assert_eq!(privacy::mask_text("x literal-pw-4 y"), "x *** y");
    }

    fn parse(src: &str) -> Result<Config> {
        let expanded = expand_env(src, &BTreeMap::new())?;
        let mut cfg: Config = serde_yaml_ng::from_str(&expanded)?;
        cfg.resolve_options()?;
        Ok(cfg)
    }

    /// The whole point of `check_group_defaults`: `extra` swallows the typo
    /// silently, so nothing else in the config layer can catch it.
    #[test]
    fn a_misspelt_key_in_the_privacy_section_fails_the_config_parse() {
        let error = parse("paths: [f]\nresources:\n  api: {}\nprivacy:\n  wrods: [x]\n")
            .unwrap_err()
            .to_string();
        assert!(error.contains("wrods"), "{error}");
    }

    #[test]
    fn the_privacy_section_parses_through_the_flattened_config() {
        let cfg = parse("paths: [f]\nresources:\n  api: {}\nprivacy:\n  fields: [otp]\n  min_length: 5\n  reveal: true\n")
            .unwrap();
        assert_eq!(cfg.privacy.fields, Some(vec!["otp".to_string()]));
        assert_eq!(
            (cfg.privacy.min_length, cfg.privacy.reveal),
            (Some(5), Some(true))
        );
    }

    #[test]
    fn a_default_for_a_group_nothing_declares_is_rejected() {
        let c = parse(
            "paths: [features]\nresources:\n  api: {}\n  widget:\n    main:\n      bucket: b\ndefault_widgte: main\n",
        )
        .expect("config parses");
        let error = c
            .check_group_defaults(true)
            .expect_err("default_widgte names nothing")
            .to_string();
        assert!(error.contains("default_widgte"), "{error}");
        assert!(error.contains("widget"), "{error}");
    }

    #[test]
    fn a_default_for_a_declared_group_is_accepted() {
        let c = parse(
            "paths: [features]\nresources:\n  api: {}\n  widget:\n    main:\n      bucket: b\ndefault_widget: main\n",
        )
        .expect("config parses");
        c.check_group_defaults(true)
            .expect("default_widget names its group");
    }

    #[test]
    fn unknown_nested_option_fields_are_rejected() {
        let error = parse(
            "paths: [features]\nresources:\n  api:\n    jobs:\n      base_url: http://jobs.local\n      options:\n        polling:\n          retries: 3\n",
        )
        .expect_err("unknown option field is rejected");
        assert!(error.to_string().contains("retries"), "{error}");
    }

    #[test]
    fn concurrency_defaults_to_eight() {
        let c = parse("paths: [f]\nresources:\n  api:\n    a:\n      base_url: http://a.local\n")
            .expect("config parses");
        assert_eq!(c.concurrency, 8);
    }

    #[test]
    fn expands_environment_variables() {
        unsafe { std::env::set_var("BDDKIT_TEST_HOST", "example.test") };
        let c = parse(
            "paths: [f]\nresources:\n  s3:\n    a:\n      endpoint: http://${BDDKIT_TEST_HOST}\n",
        )
        .expect("config parses");
        assert_eq!(
            c.resources.groups["s3"]["a"]["endpoint"],
            "http://example.test"
        );
    }

    #[test]
    fn missing_environment_variable_is_an_error() {
        let err =
            parse("paths: [f]\nresources:\n  api:\n    a:\n      base_url: ${BDDKIT_ABSENT_VAR}\n")
                .expect_err("variable is missing");
        assert!(err.to_string().contains("BDDKIT_ABSENT_VAR"), "{err}");
    }

    mod expand_env_operators {
        use super::*;

        fn expand(src: &str, pairs: &[(&str, &str)]) -> Result<String> {
            let map: BTreeMap<String, String> = pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            expand_env(src, &map)
        }

        #[test]
        fn dollar_dollar_is_a_literal_dollar_sign() {
            assert_eq!(
                expand("$${NOT_A_VAR}", &[]).expect("parses"),
                "${NOT_A_VAR}"
            );
        }

        #[test]
        fn bare_dollar_var_without_braces_behaves_like_braced_form() {
            let out = expand("$FOO", &[("FOO", "bar")]).expect("FOO is set");
            assert_eq!(out, "bar");
        }

        #[test]
        fn bare_dollar_var_without_braces_errors_when_unset() {
            let err = expand("$FOO", &[]).expect_err("FOO is not set");
            assert!(err.to_string().contains("FOO"), "{err}");
        }

        #[test]
        fn colon_dash_uses_default_when_unset() {
            assert_eq!(expand("${FOO:-fallback}", &[]).expect("parses"), "fallback");
        }

        #[test]
        fn colon_dash_uses_default_when_set_but_empty() {
            assert_eq!(
                expand("${FOO:-fallback}", &[("FOO", "")]).expect("parses"),
                "fallback"
            );
        }

        #[test]
        fn colon_dash_uses_value_when_set_and_nonempty() {
            assert_eq!(
                expand("${FOO:-fallback}", &[("FOO", "real")]).expect("parses"),
                "real"
            );
        }

        #[test]
        fn dash_uses_default_only_when_unset() {
            assert_eq!(expand("${FOO-fallback}", &[]).expect("parses"), "fallback");
        }

        #[test]
        fn dash_passes_through_empty_value_unchanged() {
            assert_eq!(
                expand("${FOO-fallback}", &[("FOO", "")]).expect("parses"),
                ""
            );
        }

        #[test]
        fn colon_question_errors_when_unset_with_custom_message() {
            let err = expand("${FOO:?custom message}", &[]).expect_err("FOO is not set");
            assert_eq!(err.to_string(), "custom message");
        }

        #[test]
        fn colon_question_errors_when_set_but_empty() {
            let err = expand("${FOO:?custom message}", &[("FOO", "")]).expect_err("FOO is empty");
            assert_eq!(err.to_string(), "custom message");
        }

        #[test]
        fn colon_question_with_empty_message_falls_back_to_default_message() {
            let err = expand("${FOO:?}", &[]).expect_err("FOO is not set");
            assert_eq!(
                err.to_string(),
                "environment variable FOO is not set but is referenced in the config"
            );
        }

        #[test]
        fn colon_question_passes_through_value_when_set_and_nonempty() {
            assert_eq!(
                expand("${FOO:?msg}", &[("FOO", "real")]).expect("parses"),
                "real"
            );
        }

        #[test]
        fn question_errors_only_when_unset() {
            let err = expand("${FOO?custom message}", &[]).expect_err("FOO is not set");
            assert_eq!(err.to_string(), "custom message");
        }

        #[test]
        fn question_passes_through_empty_value_without_error() {
            assert_eq!(
                expand("${FOO?custom message}", &[("FOO", "")]).expect("FOO is set to empty"),
                ""
            );
        }

        #[test]
        fn colon_plus_uses_alt_when_set_and_nonempty() {
            assert_eq!(
                expand("${FOO:+alt}", &[("FOO", "real")]).expect("parses"),
                "alt"
            );
        }

        #[test]
        fn colon_plus_is_empty_when_unset() {
            assert_eq!(expand("${FOO:+alt}", &[]).expect("parses"), "");
        }

        #[test]
        fn colon_plus_is_empty_when_set_but_empty() {
            assert_eq!(expand("${FOO:+alt}", &[("FOO", "")]).expect("parses"), "");
        }

        #[test]
        fn plus_uses_alt_when_set_even_if_empty() {
            assert_eq!(expand("${FOO+alt}", &[("FOO", "")]).expect("parses"), "alt");
        }

        #[test]
        fn plus_is_empty_when_unset() {
            assert_eq!(expand("${FOO+alt}", &[]).expect("parses"), "");
        }

        #[test]
        fn real_process_env_wins_over_env_map() {
            // PATH is guaranteed to be set in any process — no need to mutate
            // the real environment to prove precedence: if the map won, the
            // result would match the fake value below.
            let out = expand("${PATH}", &[("PATH", "definitely_not_the_real_path")])
                .expect("PATH is set in the real environment");
            assert_ne!(out, "definitely_not_the_real_path");
        }
    }

    /// `db` is a plugin group: `default_db` keeps its meaning through the rule
    /// every plugin group's `default_<group>` follows.
    mod default_db {
        use super::*;

        #[test]
        fn the_config_key_names_the_default() {
            let src = "paths: [f]\ndefault_db: two\nresources:\n  db:\n    one:\n      dsn: postgres://u:p@db/x\n    \
                       two:\n      dsn: postgres://u:p@db/y\n";
            let c = parse(src).expect("config parses");
            c.check_group_defaults(true)
                .expect("default_db names a declared group");
            assert_eq!(
                c.resolve_default_group("db").expect("default is set"),
                Some("two".to_string())
            );
        }

        /// The built-in refused a `default_db` with nothing declared; with no
        /// plugin loaded it still is, while other unknown keys stay tolerated.
        #[test]
        fn a_stray_default_db_is_refused_with_no_plugin_loaded() {
            let c = parse("paths: [f]\ndefault_db: main\ndefault_timeout: 30\nresources: {}\n")
                .expect("config parses");
            let error = c
                .check_group_defaults(false)
                .expect_err("default_db names nothing")
                .to_string();
            assert!(error.contains("default_db"), "{error}");
            let other = parse("paths: [f]\ndefault_timeout: 30\nresources: {}\n").expect("parses");
            other
                .check_group_defaults(false)
                .expect("an unknown key is tolerated");
        }
    }

    mod env_layers {
        use super::*;

        fn unique_dir(name: &str) -> PathBuf {
            let dir = std::env::temp_dir().join(format!("bddkit_env_layers_{name}"));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("create temp directory");
            dir
        }

        #[test]
        fn load_env_file_is_a_noop_when_file_is_missing() {
            let dir = unique_dir("missing_file");
            let mut map = BTreeMap::new();
            load_env_file(&dir.join(".env"), &mut map).expect("a missing file is not an error");
            assert!(map.is_empty());
        }

        #[test]
        fn load_env_file_reads_key_value_pairs() {
            let dir = unique_dir("basic_read");
            std::fs::write(dir.join(".env"), "FOO=bar\nBAZ=qux\n").expect("write .env");
            let mut map = BTreeMap::new();
            load_env_file(&dir.join(".env"), &mut map).expect(".env reads");
            assert_eq!(map.get("FOO"), Some(&"bar".to_string()));
            assert_eq!(map.get("BAZ"), Some(&"qux".to_string()));
        }

        #[test]
        fn load_env_file_reports_the_path_on_a_malformed_file() {
            let dir = unique_dir("malformed_file");
            let path = dir.join(".env");
            std::fs::write(&path, "FOO=\"unterminated\n").expect("write invalid .env");
            let mut map = BTreeMap::new();
            let err = load_env_file(&path, &mut map).expect_err("invalid .env is an error");
            assert!(
                err.to_string().contains(&path.display().to_string()),
                "{err}"
            );
        }

        #[test]
        fn a_malformed_line_is_never_quoted_back() {
            let dir = unique_dir("malformed_quote");
            let path = dir.join(".env");
            std::fs::write(&path, "DB_PASSWORD=hunter2222 \"unterminated\n").expect("write");
            let err = load_env_file(&path, &mut BTreeMap::new()).expect_err("malformed");
            let text = format!("{err:#}");
            assert!(!text.contains("hunter2222"), "{text}");
            assert!(text.contains(&path.display().to_string()), "{text}");
        }

        #[test]
        fn later_file_overrides_earlier_for_same_key() {
            let dir = unique_dir("override");
            let mut map = BTreeMap::new();
            std::fs::write(dir.join("a"), "FOO=from_a\n").expect("write a");
            std::fs::write(dir.join("b"), "FOO=from_b\n").expect("write b");
            load_env_file(&dir.join("a"), &mut map).expect("a reads");
            load_env_file(&dir.join("b"), &mut map).expect("b reads");
            assert_eq!(map.get("FOO"), Some(&"from_b".to_string()));
        }

        #[test]
        fn load_env_layers_applies_full_precedence_order() {
            let dir = unique_dir("full_precedence");
            std::fs::write(dir.join(".env"), "A=base\nB=base\nC=base\nD=base\n")
                .expect("write .env");
            std::fs::write(dir.join(".env.local"), "B=local\nC=local\nD=local\n")
                .expect("write .env.local");
            std::fs::write(dir.join(".env.dev"), "C=dev\nD=dev\n").expect("write .env.dev");
            std::fs::write(dir.join(".env.dev.local"), "D=dev_local\n")
                .expect("write .env.dev.local");

            let map = load_env_layers(&dir, "dev").expect("layers read");
            assert_eq!(map.get("A"), Some(&"base".to_string()), "only in .env");
            assert_eq!(
                map.get("B"),
                Some(&"local".to_string()),
                ".env.local overrides .env"
            );
            assert_eq!(
                map.get("C"),
                Some(&"dev".to_string()),
                ".env.dev overrides .env.local"
            );
            assert_eq!(
                map.get("D"),
                Some(&"dev_local".to_string()),
                ".env.dev.local is the most specific, it wins"
            );
        }

        #[test]
        fn load_env_layers_ignores_missing_optional_files() {
            let dir = unique_dir("only_base");
            std::fs::write(dir.join(".env"), "A=base\n").expect("write .env");
            let map = load_env_layers(&dir, "prod")
                .expect("missing .env.local/.env.prod* is not an error");
            assert_eq!(map.get("A"), Some(&"base".to_string()));
            assert_eq!(map.len(), 1);
        }

        #[test]
        fn resolve_app_env_cli_flag_wins() {
            let base = BTreeMap::new();
            let resolved = resolve_app_env_with_process_env(
                Some("from_cli"),
                Some("from_process".into()),
                &base,
            );
            assert_eq!(resolved, "from_cli");
        }

        #[test]
        fn resolve_app_env_falls_back_to_base_map_value() {
            let mut base = BTreeMap::new();
            base.insert("APP_ENV".to_string(), "from_dotenv".to_string());
            let resolved = resolve_app_env_with_process_env(None, None, &base);
            assert_eq!(resolved, "from_dotenv");
        }

        #[test]
        fn resolve_app_env_defaults_to_dev() {
            let base = BTreeMap::new();
            let resolved = resolve_app_env_with_process_env(None, None, &base);
            assert_eq!(resolved, "dev");
        }

        #[test]
        fn resolve_app_env_real_env_var_wins_over_base_map() {
            let mut base = BTreeMap::new();
            base.insert("APP_ENV".to_string(), "from_base_map".to_string());
            let resolved =
                resolve_app_env_with_process_env(None, Some("from_real_env".into()), &base);
            assert_eq!(resolved, "from_real_env");
        }
    }

    mod resolve_config_path {
        use super::*;

        fn scratch_dir(name: &str) -> PathBuf {
            let dir = std::env::temp_dir().join(format!("bddkit_resolve_config_path_{name}"));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("create scratch dir");
            dir
        }

        #[test]
        fn flag_wins_over_everything() {
            let dir = scratch_dir("flag_wins");
            std::fs::write(dir.join("bddkit.yaml"), "").expect("write");
            let flag = PathBuf::from("explicit.yaml");
            let resolved =
                resolve_config_path_in(Some(&flag), Some(Path::new("from_env.yaml")), &dir)
                    .expect("resolves")
                    .expect("some");
            assert_eq!(resolved.path, flag);
            assert_eq!(resolved.source, ConfigSource::Flag);
        }

        #[test]
        fn env_wins_over_the_default_file() {
            let dir = scratch_dir("env_wins");
            std::fs::write(dir.join("bddkit.yaml"), "").expect("write");
            let env_path = PathBuf::from("from_env.yaml");
            let resolved = resolve_config_path_in(None, Some(&env_path), &dir)
                .expect("resolves")
                .expect("some");
            assert_eq!(resolved.path, env_path);
            assert_eq!(resolved.source, ConfigSource::Env);
        }

        #[test]
        fn discovers_bddkit_yaml_in_the_working_directory() {
            let dir = scratch_dir("discovers_yaml");
            std::fs::write(dir.join("bddkit.yaml"), "").expect("write");
            let resolved = resolve_config_path_in(None, None, &dir)
                .expect("resolves")
                .expect("some");
            assert_eq!(resolved.path, dir.join("bddkit.yaml"));
            assert_eq!(resolved.source, ConfigSource::Discovered);
        }

        #[test]
        fn discovers_the_yml_spelling_too() {
            let dir = scratch_dir("discovers_yml");
            std::fs::write(dir.join("bddkit.yml"), "").expect("write");
            let resolved = resolve_config_path_in(None, None, &dir)
                .expect("resolves")
                .expect("some");
            assert_eq!(resolved.path, dir.join("bddkit.yml"));
            assert_eq!(resolved.source, ConfigSource::Discovered);
        }

        #[test]
        fn refuses_when_both_spellings_exist() {
            let dir = scratch_dir("both_spellings");
            std::fs::write(dir.join("bddkit.yaml"), "").expect("write");
            std::fs::write(dir.join("bddkit.yml"), "").expect("write");
            let err = resolve_config_path_in(None, None, &dir).expect_err("ambiguous");
            assert!(format!("{err:#}").contains("bddkit.yaml"), "{err:#}");
            assert!(format!("{err:#}").contains("bddkit.yml"), "{err:#}");
        }

        #[test]
        fn nothing_found_is_none() {
            let dir = scratch_dir("nothing_found");
            let resolved = resolve_config_path_in(None, None, &dir).expect("resolves");
            assert!(resolved.is_none());
        }

        #[test]
        fn a_bddkit_yaml_in_a_parent_directory_is_not_picked_up() {
            let dir = scratch_dir("parent_not_walked");
            std::fs::write(dir.join("bddkit.yaml"), "").expect("write");
            let child = dir.join("child");
            std::fs::create_dir_all(&child).expect("create child dir");
            let resolved = resolve_config_path_in(None, None, &child).expect("resolves");
            assert!(resolved.is_none());
        }
    }

    #[test]
    fn load_reads_a_config_file() {
        let path = std::env::temp_dir().join("bddkit_cfg_load_reads_a_config_file.yaml");
        std::fs::write(
            &path,
            "paths: [features]\nresources:\n  api:\n    review:\n      base_url: http://review.local\n",
        )
        .expect("write config");
        let c = load(&path, None).expect("config loads");
        assert_eq!(c.paths, vec![PathBuf::from("features")]);
    }

    mod load_with_env_files {
        use super::*;

        fn write_config(dir: &Path, yaml: &str) -> PathBuf {
            let path = dir.join("config.yaml");
            std::fs::write(&path, yaml).expect("write config");
            path
        }

        fn unique_dir(name: &str) -> PathBuf {
            let dir = std::env::temp_dir().join(format!("bddkit_load_with_env_{name}"));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("create temp directory");
            dir
        }

        #[test]
        fn substitutes_value_from_base_dotenv_file() {
            let dir = unique_dir("base_dotenv");
            std::fs::write(dir.join(".env"), "DB_PASS=secret\n").expect("write .env");
            let path = write_config(
                &dir,
                "paths: [f]\nresources:\n  s3:\n    main:\n      dsn: postgres://u:${DB_PASS}@db/x\n",
            );
            let c = load(&path, None).expect("config loads with .env");
            assert_eq!(
                c.resources.groups["s3"]["main"]["dsn"],
                "postgres://u:secret@db/x"
            );
        }

        #[test]
        fn cli_env_flag_selects_the_matching_env_specific_file() {
            let dir = unique_dir("cli_env_flag");
            std::fs::write(dir.join(".env"), "DB_PASS=base_secret\n").expect("write .env");
            std::fs::write(dir.join(".env.prod"), "DB_PASS=prod_secret\n")
                .expect("write .env.prod");
            let path = write_config(
                &dir,
                "paths: [f]\nresources:\n  s3:\n    main:\n      dsn: postgres://u:${DB_PASS}@db/x\n",
            );
            let c = load(&path, Some("prod")).expect("config loads with --env=prod");
            assert_eq!(
                c.resources.groups["s3"]["main"]["dsn"],
                "postgres://u:prod_secret@db/x"
            );
        }

        #[test]
        fn without_cli_env_flag_defaults_to_dev_file() {
            let dir = unique_dir("default_dev");
            std::fs::write(dir.join(".env"), "DB_PASS=base_secret\n").expect("write .env");
            std::fs::write(dir.join(".env.dev"), "DB_PASS=dev_secret\n").expect("write .env.dev");
            let path = write_config(
                &dir,
                "paths: [f]\nresources:\n  s3:\n    main:\n      dsn: postgres://u:${DB_PASS}@db/x\n",
            );
            if std::env::var("APP_ENV").is_err() {
                let c = load(&path, None).expect("config loads without --env");
                assert_eq!(
                    c.resources.groups["s3"]["main"]["dsn"],
                    "postgres://u:dev_secret@db/x"
                );
            }
        }

        #[test]
        fn local_file_overrides_base_env_for_same_key() {
            let dir = unique_dir("local_override");
            std::fs::write(dir.join(".env"), "DB_PASS=base_secret\n").expect("write .env");
            std::fs::write(dir.join(".env.local"), "DB_PASS=local_secret\n")
                .expect("write .env.local");
            let path = write_config(
                &dir,
                "paths: [f]\nresources:\n  s3:\n    main:\n      dsn: postgres://u:${DB_PASS}@db/x\n",
            );
            let c = load(&path, None).expect("config loads");
            assert_eq!(
                c.resources.groups["s3"]["main"]["dsn"],
                "postgres://u:local_secret@db/x"
            );
        }
    }

    const WITH_GROUP: &str = "\
paths: [features]
options:
  polling:
    timeout_secs: 10
resources:
  api:
    review:
      base_url: http://review.local
  widget:
    backups:
      endpoint: http://storage.internal:9000
      bucket: backups
      options:
        polling:
          timeout_secs: 30
    archive:
      endpoint: http://storage.internal:9000
      bucket: archive
default_widget: backups
";

    #[test]
    fn an_unknown_resource_group_is_kept_verbatim() {
        // The host has no schema for a group it did not write: it must carry the
        // instance body through untouched and let the plugin judge it.
        let cfg = parse(WITH_GROUP).expect("parses");
        let names: Vec<&String> = cfg.group_names().collect();
        // No typed `api` field here: the flattened map takes it.
        assert_eq!(names, vec!["api", "widget"]);
        let instance = cfg
            .plugin_instances
            .iter()
            .find(|i| i.name == "backups")
            .expect("backups declared");
        assert_eq!(instance.config["bucket"], serde_json::json!("backups"));
        assert_eq!(
            instance.config["endpoint"],
            serde_json::json!("http://storage.internal:9000")
        );
        // A flattened catch-all is one typo away from cannibalising the typed
        // fields beside it, and the failure would be silent: the field keeps
        // its default and the value lands in `extra` instead.
        assert_eq!(cfg.paths, vec![std::path::PathBuf::from("features")]);
        assert_eq!(cfg.concurrency, default_concurrency());
        assert!(
            cfg.extra.keys().eq(["default_widget".to_string()].iter()),
            "only the group default is unclaimed, got {:?}",
            cfg.extra.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn the_reserved_options_key_never_reaches_the_plugin() {
        let cfg = parse(WITH_GROUP).expect("parses");
        let instance = cfg
            .plugin_instances
            .iter()
            .find(|i| i.name == "backups")
            .expect("backups declared");
        assert!(
            instance.config.get("options").is_none(),
            "options is host-managed and must be stripped: {:?}",
            instance.config
        );
    }

    #[test]
    fn instance_options_inherit_from_the_global_layer() {
        let cfg = parse(WITH_GROUP).expect("parses");
        let backups = cfg
            .plugin_instances
            .iter()
            .find(|i| i.name == "backups")
            .unwrap();
        let archive = cfg
            .plugin_instances
            .iter()
            .find(|i| i.name == "archive")
            .unwrap();
        assert_eq!(
            backups.options.polling.timeout,
            std::time::Duration::from_secs(30)
        );
        assert_eq!(
            archive.options.polling.timeout,
            std::time::Duration::from_secs(10),
            "an instance with no options layer inherits the global one"
        );
    }

    #[test]
    fn an_invalid_host_option_inside_a_group_is_rejected_before_the_plugin_loads() {
        let src = "\
paths: [features]
resources:
  api: {}
  widget:
    backups:
      options:
        polling:
          timeout_secs: 0
";
        let error = parse(src).expect_err("zero timeout is invalid");
        assert!(
            format!("{error:#}").contains("resources.widget.backups"),
            "{error:#}"
        );
    }

    #[test]
    fn an_unknown_option_inside_a_group_is_rejected() {
        let src = "\
paths: [features]
resources:
  api: {}
  widget:
    backups:
      options:
        pollng:
          timeout_secs: 3
";
        let error = parse(src).expect_err("typo in a host option");
        assert!(
            format!("{error:#}").contains("resources.widget.backups"),
            "{error:#}"
        );
    }

    #[test]
    fn a_group_default_resolves_like_every_other_default() {
        let cfg = parse(WITH_GROUP).expect("parses");
        assert_eq!(
            cfg.resolve_default_group("widget").unwrap().as_deref(),
            Some("backups")
        );
    }

    #[test]
    fn a_single_instance_becomes_the_group_default_on_its_own() {
        let src = "\
paths: [features]
resources:
  api: {}
  widget:
    only:
      bucket: b
";
        let cfg = parse(src).expect("parses");
        assert_eq!(
            cfg.resolve_default_group("widget").unwrap().as_deref(),
            Some("only")
        );
    }

    #[test]
    fn several_instances_without_a_default_is_an_error() {
        let src = "\
paths: [features]
resources:
  api: {}
  widget:
    a: {bucket: a}
    b: {bucket: b}
";
        let cfg = parse(src).expect("parses");
        let error = cfg.resolve_default_group("widget").expect_err("ambiguous");
        assert!(format!("{error:#}").contains("default_widget"), "{error:#}");
    }

    #[test]
    fn a_group_default_naming_an_undeclared_instance_is_an_error() {
        let src = "\
paths: [features]
resources:
  api: {}
  widget:
    a: {bucket: a}
default_widget: nope
";
        let cfg = parse(src).expect("parses");
        let error = cfg.resolve_default_group("widget").expect_err("undeclared");
        assert!(format!("{error:#}").contains("nope"), "{error:#}");
    }
}
