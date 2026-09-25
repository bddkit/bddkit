use crate::options::{Options, OptionsLayer};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn default_concurrency() -> usize {
    8
}
fn default_timeout() -> u64 {
    20
}

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,
    #[serde(default)]
    pub macro_paths: Vec<PathBuf>,
    pub paths: Vec<PathBuf>,
    pub resources: Resources,
    #[serde(default)]
    pub default_api: Option<String>,
    #[serde(default)]
    pub default_db: Option<String>,
    #[serde(default)]
    pub default_srp: Option<String>,
    #[serde(default)]
    pub options: OptionsLayer,
    #[serde(skip)]
    pub effective_options: Options,
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
    pub api: BTreeMap<String, ApiConfig>,
    #[serde(default)]
    pub db: BTreeMap<String, Connection>,
    #[serde(default)]
    pub srp: BTreeMap<String, SrpConfig>,
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

/// What a `resources.api.<name>` body takes: name, required, description, type.
///
/// Hand-written because Rust has no reflection to derive it from `ApiConfig`,
/// and a `#[serde(skip)]` field is not a config key at all. `bddkit resource
/// fields` prints these beside what a plugin says about its own group, so the
/// three host kinds answer the same question the plugins do.
pub const API_FIELDS: &[(&str, bool, &str, Scalar)] = &[
    (
        "base_url",
        true,
        "root URL every request is resolved against",
        Scalar::Str,
    ),
    (
        "timeout_secs",
        false,
        "per-request timeout in seconds (default 20)",
        Scalar::Num,
    ),
    (
        "default_headers",
        false,
        "headers sent with every request until a scenario replaces them",
        Scalar::NonScalar,
    ),
    (
        "options",
        false,
        "polling.timeout_secs / polling.interval_ms for eventual assertions against this API",
        Scalar::NonScalar,
    ),
];

#[derive(Debug, Deserialize)]
pub struct ApiConfig {
    pub base_url: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub default_headers: BTreeMap<String, String>,
    #[serde(default)]
    pub options: OptionsLayer,
    #[serde(skip)]
    pub effective_options: Options,
}

/// `Clone` so `doctor` can hand `Db::connect` a one-entry map per connection.
/// `Db::connect` already names the failing connection in its error; what it
/// does not do is carry on, so a whole-map call would return at the first dead
/// DSN and never probe the second.
/// What a `resources.db.<name>` body takes. See `API_FIELDS`.
pub const DB_FIELDS: &[(&str, bool, &str, Scalar)] = &[
    (
        "dsn",
        true,
        "connection string; its scheme selects the engine (postgres://, mysql://)",
        Scalar::Str,
    ),
    (
        "search_path",
        false,
        "Postgres schema search path; refused on MySQL and MariaDB, which have no session equivalent",
        Scalar::NonScalar,
    ),
    (
        "options",
        false,
        "polling.timeout_secs / polling.interval_ms for eventual assertions against this connection",
        Scalar::NonScalar,
    ),
];

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Connection {
    pub dsn: String,
    #[serde(default)]
    pub search_path: Vec<String>,
    #[serde(default)]
    pub options: OptionsLayer,
    #[serde(skip)]
    pub effective_options: Options,
}

/// SRP parameters. Everything except `variant` has a sensible default: the
/// RFC 5054 4096-bit group, generator 5, SHA-256.
/// What a `resources.srp.<name>` body takes. See `API_FIELDS`.
pub const SRP_FIELDS: &[(&str, bool, &str, Scalar)] = &[
    ("variant", true, "hex-string or rfc5054", Scalar::Str),
    (
        "prime",
        false,
        "group prime in hexadecimal (default: the RFC 5054 4096-bit group)",
        Scalar::Str,
    ),
    (
        "generator",
        false,
        "generator as a decimal number (default 5)",
        Scalar::Str,
    ),
    (
        "hash",
        false,
        "sha-1, sha-256 or sha-512 (default sha-256)",
        Scalar::Str,
    ),
    (
        "options",
        false,
        "polling.timeout_secs / polling.interval_ms for eventual assertions using this configuration",
        Scalar::NonScalar,
    ),
];

/// The field table of a kind the host serves itself, or `None` for anything
/// else — which is either a plugin group or a typo, and telling those two
/// apart needs the loaded plugins the caller has and this module does not.
pub fn host_fields(kind: &str) -> Option<&'static [(&'static str, bool, &'static str, Scalar)]> {
    match kind {
        "api" => Some(API_FIELDS),
        "db" => Some(DB_FIELDS),
        "srp" => Some(SRP_FIELDS),
        _ => None,
    }
}

#[derive(Debug, Deserialize)]
pub struct SrpConfig {
    pub variant: String,
    #[serde(default)]
    pub prime: Option<String>,
    #[serde(default)]
    pub generator: Option<String>,
    #[serde(default)]
    pub hash: Option<String>,
    #[serde(default)]
    pub options: OptionsLayer,
    #[serde(skip)]
    pub effective_options: Options,
}

impl SrpConfig {
    pub fn to_params(&self) -> Result<crate::srp::SrpParams> {
        use crate::srp::{HashAlg, RFC5054_4096_PRIME_HEX, SrpParams, Variant};
        use num_bigint::BigUint;

        let variant = match self.variant.as_str() {
            "hex-string" => Variant::HexString,
            "rfc5054" => Variant::Rfc5054,
            other => bail!("unknown SRP variant {other:?}: expected hex-string or rfc5054"),
        };
        let hash = match self.hash.as_deref().unwrap_or("sha-256") {
            "sha-1" => HashAlg::Sha1,
            "sha-256" => HashAlg::Sha256,
            "sha-512" => HashAlg::Sha512,
            other => {
                bail!("unknown hash algorithm {other:?}: expected sha-1, sha-256, or sha-512")
            }
        };
        let prime_hex = self.prime.as_deref().unwrap_or(RFC5054_4096_PRIME_HEX);
        let prime = BigUint::parse_bytes(prime_hex.as_bytes(), 16)
            .ok_or_else(|| anyhow::anyhow!("prime is not a hexadecimal number"))?;
        let generator_text = self.generator.as_deref().unwrap_or("5");
        let generator = BigUint::parse_bytes(generator_text.as_bytes(), 10).ok_or_else(|| {
            anyhow::anyhow!("generator is not a decimal number: {generator_text:?}")
        })?;

        Ok(SrpParams {
            variant,
            prime,
            generator,
            hash,
        })
    }
}

impl Config {
    fn resolve_options(&mut self) -> Result<()> {
        let global = Options::default()
            .apply(&self.options)
            .map_err(anyhow::Error::msg)?;
        self.effective_options = global.clone();
        for (name, resource) in &mut self.resources.api {
            resource.effective_options = global
                .apply(&resource.options)
                .map_err(|error| anyhow::anyhow!("resources.api.{name}: {error}"))?;
        }
        for (name, resource) in &mut self.resources.db {
            resource.effective_options = global
                .apply(&resource.options)
                .map_err(|error| anyhow::anyhow!("resources.db.{name}: {error}"))?;
        }
        for (name, resource) in &mut self.resources.srp {
            resource.effective_options = global
                .apply(&resource.options)
                .map_err(|error| anyhow::anyhow!("resources.srp.{name}: {error}"))?;
        }
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

    /// The default API name. `None` if no APIs are declared at all — that's
    /// legal, HTTP steps then fail on first use (see `resolve_default_db`).
    pub fn resolve_default_api(&self) -> Result<Option<String>> {
        resolve_default(
            "default_api",
            "resources.api",
            &self.default_api,
            self.resources.api.keys(),
        )
    }

    /// The default connection name. `None` if no DBs are declared at all —
    /// that's legal, DB steps then fail on first use.
    pub fn resolve_default_db(&self) -> Result<Option<String>> {
        resolve_default(
            "default_db",
            "resources.db",
            &self.default_db,
            self.resources.db.keys(),
        )
    }

    /// The default SRP resource name. `None` if no SRP is declared at all —
    /// that's legal, SRP steps then fail on first use.
    pub fn resolve_default_srp(&self) -> Result<Option<String>> {
        resolve_default(
            "default_srp",
            "resources.srp",
            &self.default_srp,
            self.resources.srp.keys(),
        )
    }

    pub fn group_names(&self) -> impl Iterator<Item = &String> {
        self.resources.groups.keys()
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
    pub fn check_group_defaults(&self) -> Result<()> {
        for key in self.extra.keys() {
            let Some(group) = key.strip_prefix("default_") else {
                continue;
            };
            // api/db/srp are typed fields and never reach `extra`; listed so a
            // future untyped one cannot be reported as a typo.
            if matches!(group, "api" | "db" | "srp") || self.resources.groups.contains_key(group) {
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
                let (k, v) = item.with_context(|| format!("failed to parse {}", path.display()))?;
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
    let mut cfg: Config = serde_yaml_ng::from_str(&expanded).context("failed to parse config")?;
    // Resolve the defaults right away: an ambiguous config should fail at
    // startup, not at the first step that reaches for them.
    cfg.resolve_default_api()?;
    cfg.resolve_default_db()?;
    cfg.resolve_default_srp()?;
    cfg.resolve_options()?;
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
concurrency: 8
paths: [features]
default_api: review
resources:
  api:
    review:
      base_url: http://review.local
    billing:
      base_url: http://billing.local
      timeout_secs: 5
";

    /// A hand-written field table can drift from the struct it describes, and
    /// this checks the drift that matters: a renamed field, and a `required`
    /// flag flipped the wrong way. `doc` spells one value per table entry, in
    /// the table's own order, so a renamed or added entry fails here until the
    /// document follows it.
    ///
    /// What it deliberately does not catch is a field added to the struct and
    /// forgotten in the table — the benign direction: the key still works, it
    /// is only undocumented.
    fn check_table<T: serde::de::DeserializeOwned + std::fmt::Debug>(
        table: &[(&str, bool, &str, Scalar)],
        doc: &[(&str, &str)],
    ) {
        let names: Vec<&str> = table.iter().map(|(name, ..)| *name).collect();
        let keys: Vec<&str> = doc.iter().map(|(key, _)| *key).collect();
        assert_eq!(names, keys, "the table and this test's document disagree");
        let render = |omit: &str| -> String {
            doc.iter()
                .filter(|(key, _)| *key != omit)
                .map(|(key, value)| format!("{key}: {value}\n"))
                .collect()
        };
        serde_yaml_ng::from_str::<T>(&render("")).expect("every field together deserializes");
        for (name, required, ..) in table {
            if !required {
                continue;
            }
            serde_yaml_ng::from_str::<T>(&render(name))
                .expect_err("a field the table calls required cannot be omitted");
        }
    }

    #[test]
    fn the_api_field_table_matches_the_api_struct() {
        check_table::<ApiConfig>(
            API_FIELDS,
            &[
                ("base_url", "http://localhost:1"),
                ("timeout_secs", "5"),
                ("default_headers", "{ Accept: application/json }"),
                ("options", "{ polling: { timeout_secs: 3 } }"),
            ],
        );
    }

    #[test]
    fn the_db_field_table_matches_the_connection_struct() {
        check_table::<Connection>(
            DB_FIELDS,
            &[
                ("dsn", "postgres://localhost/app"),
                ("search_path", "[public]"),
                ("options", "{ polling: { timeout_secs: 3 } }"),
            ],
        );
    }

    #[test]
    fn the_srp_field_table_matches_the_srp_struct() {
        check_table::<SrpConfig>(
            SRP_FIELDS,
            &[
                ("variant", "hex-string"),
                ("prime", "\"ff\""),
                ("generator", "\"5\""),
                ("hash", "sha-256"),
                ("options", "{ polling: { timeout_secs: 3 } }"),
            ],
        );
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
    fn a_default_for_a_group_nothing_declares_is_rejected() {
        let c = parse(
            "paths: [features]\nresources:\n  api: {}\n  widget:\n    main:\n      bucket: b\ndefault_widgte: main\n",
        )
        .expect("config parses");
        let error = c
            .check_group_defaults()
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
        c.check_group_defaults()
            .expect("default_widget names its group");
    }

    #[test]
    fn reads_api_resources() {
        let c = parse(SAMPLE).expect("config parses");
        assert_eq!(c.resources.api["review"].base_url, "http://review.local");
    }

    #[test]
    fn timeout_defaults_to_twenty_seconds() {
        let c = parse(SAMPLE).expect("config parses");
        assert_eq!(c.resources.api["review"].timeout_secs, 20);
    }

    #[test]
    fn timeout_can_be_set_per_api_resource() {
        let c = parse(SAMPLE).expect("config parses");
        assert_eq!(c.resources.api["billing"].timeout_secs, 5);
    }

    #[test]
    fn options_cascade_from_root_to_each_resource_kind() {
        let c = parse(
            "options:\n  polling:\n    timeout_secs: 10\n    interval_ms: 200\nresources:\n  api:\n    jobs:\n      base_url: http://jobs.local\n      options:\n        polling:\n          timeout_secs: 30\n  db:\n    reporting:\n      dsn: postgres://u:p@db/reporting\n      options:\n        polling:\n          interval_ms: 500\n  srp:\n    login:\n      variant: hex-string\n      options: {}\npaths: [features]\n",
        )
        .expect("config parses");
        assert_eq!(
            c.effective_options.polling.timeout,
            std::time::Duration::from_secs(10)
        );
        assert_eq!(
            c.resources.api["jobs"].effective_options.polling.timeout,
            std::time::Duration::from_secs(30)
        );
        assert_eq!(
            c.resources.api["jobs"].effective_options.polling.interval,
            std::time::Duration::from_millis(200)
        );
        assert_eq!(
            c.resources.db["reporting"]
                .effective_options
                .polling
                .timeout,
            std::time::Duration::from_secs(10)
        );
        assert_eq!(
            c.resources.db["reporting"]
                .effective_options
                .polling
                .interval,
            std::time::Duration::from_millis(500)
        );
        assert_eq!(
            c.resources.srp["login"].effective_options.polling.timeout,
            std::time::Duration::from_secs(10)
        );
        assert_eq!(
            c.resources.srp["login"].effective_options.polling.interval,
            std::time::Duration::from_millis(200)
        );
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
            "paths: [f]\nresources:\n  api:\n    a:\n      base_url: http://${BDDKIT_TEST_HOST}\n",
        )
        .expect("config parses");
        assert_eq!(c.resources.api["a"].base_url, "http://example.test");
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

    mod resolve_default_api {
        use super::*;

        #[test]
        fn a_single_resource_needs_no_explicit_default() {
            let c = parse(
                "paths: [f]\nresources:\n  api:\n    only:\n      base_url: http://a.local\n",
            )
            .expect("config parses");
            assert_eq!(
                c.resolve_default_api().expect("default is inferred"),
                Some("only".to_string())
            );
        }

        #[test]
        fn an_explicit_default_wins() {
            let c = parse(SAMPLE).expect("config parses");
            assert_eq!(
                c.resolve_default_api().expect("default is set"),
                Some("review".to_string())
            );
        }

        #[test]
        fn several_resources_without_a_default_is_an_error() {
            let src = "paths: [f]\nresources:\n  api:\n    a:\n      base_url: http://a.local\n    \
                       b:\n      base_url: http://b.local\n";
            let c = parse(src).expect("config parses");
            let err = c.resolve_default_api().expect_err("default is ambiguous");
            assert!(err.to_string().contains("default_api"), "{err}");
        }

        #[test]
        fn a_default_naming_an_undeclared_resource_is_an_error() {
            let src = "paths: [f]\ndefault_api: missing\nresources:\n  api:\n    a:\n      base_url: http://a.local\n";
            let c = parse(src).expect("config parses");
            let err = c
                .resolve_default_api()
                .expect_err("resource is not declared");
            assert!(err.to_string().contains("missing"), "{err}");
        }

        #[test]
        fn no_apis_resolves_to_none() {
            // Legal: a config may describe only DBs. HTTP steps then fail on
            // first use — symmetric to resolve_default_db.
            let c = parse("paths: [f]\nresources:\n  api: {}\n").expect("config parses");
            assert_eq!(c.resolve_default_api().expect("no APIs are declared"), None);
        }
    }

    mod resolve_default_db {
        use super::*;

        #[test]
        fn no_databases_resolves_to_none() {
            let c = parse(SAMPLE).expect("config parses");
            assert_eq!(c.resolve_default_db().expect("no DBs are declared"), None);
        }

        #[test]
        fn a_single_connection_needs_no_explicit_default() {
            let src = "paths: [f]\nresources:\n  api:\n    a:\n      base_url: http://a.local\n  \
                       db:\n    only:\n      dsn: postgres://u:p@db/x\n";
            let c = parse(src).expect("config parses");
            assert_eq!(
                c.resolve_default_db().expect("default is inferred"),
                Some("only".to_string())
            );
        }

        #[test]
        fn several_connections_without_a_default_is_an_error() {
            let src = "paths: [f]\nresources:\n  api:\n    a:\n      base_url: http://a.local\n  \
                       db:\n    one:\n      dsn: postgres://u:p@db/x\n    two:\n      dsn: postgres://u:p@db/y\n";
            let c = parse(src).expect("config parses");
            let err = c.resolve_default_db().expect_err("default is ambiguous");
            assert!(err.to_string().contains("default_db"), "{err}");
        }

        #[test]
        fn reads_search_path() {
            let src = "paths: [f]\nresources:\n  api:\n    a:\n      base_url: http://a.local\n  \
                       db:\n    main:\n      dsn: postgres://u:p@db/x\n      search_path: [app, public]\n";
            let c = parse(src).expect("config parses");
            assert_eq!(c.resources.db["main"].search_path, vec!["app", "public"]);
        }

        #[test]
        fn search_path_defaults_to_empty() {
            let src = "paths: [f]\nresources:\n  api:\n    a:\n      base_url: http://a.local\n  \
                       db:\n    main:\n      dsn: postgres://u:p@db/x\n";
            let c = parse(src).expect("config parses");
            assert!(c.resources.db["main"].search_path.is_empty());
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

    #[test]
    fn load_rejects_an_ambiguous_default_api() {
        let path = std::env::temp_dir().join("bddkit_cfg_load_rejects_ambiguous.yaml");
        std::fs::write(
            &path,
            "paths: [f]\nresources:\n  api:\n    a:\n      base_url: http://a.local\n    b:\n      base_url: http://b.local\n",
        )
        .expect("write config");
        let err = load(&path, None).expect_err("default is ambiguous");
        // `load` now wraps the error with `config <path>:` (`load_str`'s own
        // context), so the ambiguity reason is in the chain, not in the
        // top-level `Display` — `{:#}` renders the whole chain.
        assert!(format!("{err:#}").contains("default_api"), "{err:#}");
    }

    /// The point of `load_str`: the text being validated need never have been a
    /// file. This is what lets `resource add` check the exact bytes it is about to
    /// write, with the `.env` layers of the real config's directory.
    #[test]
    fn load_str_validates_text_that_was_never_a_file() {
        let raw = "paths: [features]\nresources:\n  api:\n    a:\n      base_url: http://a.local\n";
        load_str(raw, Path::new("."), None).expect("one API needs no default_api");

        let two = "paths: [features]\nresources:\n  api:\n    a:\n      base_url: http://a.local\n    b:\n      base_url: http://b.local\n";
        let error = load_str(two, Path::new("."), None)
            .expect_err("two APIs and no default_api is ambiguous")
            .to_string();
        assert!(error.contains("default_api"), "{error}");
    }

    /// `resource add` converts a flag's string through this table, so a kind the
    /// host serves must be reachable by name and must say which keys no flag can
    /// express.
    #[test]
    fn host_fields_names_the_type_each_key_takes() {
        let api = host_fields("api").expect("api is a host kind");
        let by_name = |key: &str| {
            api.iter()
                .find(|(name, ..)| *name == key)
                .expect("declared")
                .3
        };
        assert_eq!(by_name("base_url"), Scalar::Str);
        assert_eq!(by_name("timeout_secs"), Scalar::Num);
        assert_eq!(by_name("default_headers"), Scalar::NonScalar);
        assert_eq!(by_name("options"), Scalar::NonScalar);
        assert!(
            host_fields("s3").is_none(),
            "a plugin group is not a host kind"
        );
    }

    #[test]
    fn srp_defaults_to_the_4096_bit_group_with_sha256() {
        let cfg = SrpConfig {
            variant: "hex-string".into(),
            prime: None,
            generator: None,
            hash: None,
            options: OptionsLayer::default(),
            effective_options: Options::default(),
        };
        let params = cfg.to_params().expect("defaults are valid");
        assert_eq!(params.variant, crate::srp::Variant::HexString);
        assert_eq!(params.hash, crate::srp::HashAlg::Sha256);
        assert_eq!(params.generator, num_bigint::BigUint::from(5u32));
        assert_eq!(
            crate::srp::hex(&params.k()),
            "3509477ea9fca66eadb7cf7b1bd0eb508f54d3989a9c988006a7d0b338374dd2"
        );
    }

    #[test]
    fn srp_accepts_an_explicit_group_and_hash() {
        let cfg = SrpConfig {
            variant: "rfc5054".into(),
            prime: Some(crate::srp::RFC5054_1024_PRIME_HEX.to_string()),
            generator: Some("2".into()),
            hash: Some("sha-1".into()),
            options: OptionsLayer::default(),
            effective_options: Options::default(),
        };
        let params = cfg.to_params().expect("explicit values are valid");
        assert_eq!(
            crate::srp::hex(&params.k()),
            "7556aa045aef2cdd07abaf0f665c3e818913186f"
        );
    }

    #[test]
    fn an_unknown_variant_is_rejected_by_name() {
        let cfg = SrpConfig {
            variant: "srp7".into(),
            prime: None,
            generator: None,
            hash: None,
            options: OptionsLayer::default(),
            effective_options: Options::default(),
        };
        let err = cfg.to_params().unwrap_err().to_string();
        assert!(err.contains("srp7"), "error must name the value: {err}");
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
                "paths: [f]\nresources:\n  api:\n    a:\n      base_url: http://a.local\n  \
                 db:\n    main:\n      dsn: postgres://u:${DB_PASS}@db/x\n",
            );
            let c = load(&path, None).expect("config loads with .env");
            assert_eq!(c.resources.db["main"].dsn, "postgres://u:secret@db/x");
        }

        #[test]
        fn cli_env_flag_selects_the_matching_env_specific_file() {
            let dir = unique_dir("cli_env_flag");
            std::fs::write(dir.join(".env"), "DB_PASS=base_secret\n").expect("write .env");
            std::fs::write(dir.join(".env.prod"), "DB_PASS=prod_secret\n")
                .expect("write .env.prod");
            let path = write_config(
                &dir,
                "paths: [f]\nresources:\n  api:\n    a:\n      base_url: http://a.local\n  \
                 db:\n    main:\n      dsn: postgres://u:${DB_PASS}@db/x\n",
            );
            let c = load(&path, Some("prod")).expect("config loads with --env=prod");
            assert_eq!(c.resources.db["main"].dsn, "postgres://u:prod_secret@db/x");
        }

        #[test]
        fn without_cli_env_flag_defaults_to_dev_file() {
            let dir = unique_dir("default_dev");
            std::fs::write(dir.join(".env"), "DB_PASS=base_secret\n").expect("write .env");
            std::fs::write(dir.join(".env.dev"), "DB_PASS=dev_secret\n").expect("write .env.dev");
            let path = write_config(
                &dir,
                "paths: [f]\nresources:\n  api:\n    a:\n      base_url: http://a.local\n  \
                 db:\n    main:\n      dsn: postgres://u:${DB_PASS}@db/x\n",
            );
            if std::env::var("APP_ENV").is_err() {
                let c = load(&path, None).expect("config loads without --env");
                assert_eq!(c.resources.db["main"].dsn, "postgres://u:dev_secret@db/x");
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
                "paths: [f]\nresources:\n  api:\n    a:\n      base_url: http://a.local\n  \
                 db:\n    main:\n      dsn: postgres://u:${DB_PASS}@db/x\n",
            );
            let c = load(&path, None).expect("config loads");
            assert_eq!(c.resources.db["main"].dsn, "postgres://u:local_secret@db/x");
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
        assert_eq!(names, vec!["widget"]);
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
