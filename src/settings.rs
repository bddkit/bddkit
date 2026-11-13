//! The options a config key, a flag and an environment variable all spell:
//! flag, then environment, then the config key, then the default. `run` and
//! `doctor` flatten the same `SettingsArgs` and call the same `resolve`, so
//! they cannot disagree. An empty variable is unset, a boolean is
//! `true|false|1|0`, a list is comma-separated; anything else is an error.
//! The privacy options have their own, older layer (`privacy.rs`).

use crate::config::Config;
use anyhow::{Context, Result};
use clap::Args;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::str::FromStr;

/// The flag layer shared by `run` and `doctor`. `--fail-fast` and `--tag` are
/// `run`'s alone and are laid over the result by the caller.
#[derive(Args, Default)]
pub struct SettingsArgs {
    /// How many feature files run at once [default: $BDDKIT_CONCURRENCY, else the config's `concurrency`]
    #[arg(long)]
    pub concurrency: Option<usize>,
    /// Directories of macro YAML files, replacing the config's `macro_paths` (comma-separated) [default: $BDDKIT_MACRO_PATHS]
    #[arg(long = "macro-paths", value_delimiter = ',')]
    pub macro_paths: Option<Vec<PathBuf>>,
    /// The API a scenario starts on; an alias of `--default api=<name>` [default: $BDDKIT_DEFAULT_API, else the config's `default_api`]
    #[arg(long = "default-api")]
    pub default_api: Option<String>,
    /// The database connection a scenario starts on; an alias of `--default db=<name>` [default: $BDDKIT_DEFAULT_DB, else the config's `default_db`]
    #[arg(long = "default-db")]
    pub default_db: Option<String>,
    /// The SRP configuration the SRP steps use; an alias of `--default srp=<name>` [default: $BDDKIT_DEFAULT_SRP, else the config's `default_srp`]
    #[arg(long = "default-srp")]
    pub default_srp: Option<String>,
    /// The instance a resource group starts on, as `<group>=<name>`, for any group; repeatable, one group per use [default: $BDDKIT_DEFAULT_<GROUP>, else the config's `default_<group>`]
    #[arg(long = "default", value_name = "GROUP=NAME", value_parser = group_name_pair)]
    pub default: Vec<(String, String)>,
    /// Global assertion polling timeout in seconds [default: $BDDKIT_OPTIONS_POLLING_TIMEOUT_SECS, else the config's `options.polling.timeout_secs`]
    #[arg(long = "options-polling-timeout-secs")]
    pub polling_timeout_secs: Option<u64>,
    /// Global assertion polling interval in milliseconds [default: $BDDKIT_OPTIONS_POLLING_INTERVAL_MS, else the config's `options.polling.interval_ms`]
    #[arg(long = "options-polling-interval-ms")]
    pub polling_interval_ms: Option<u64>,
}

/// What the flags and the environment decided; `None` leaves the config's value.
#[derive(Debug, Default, PartialEq)]
pub struct Overrides {
    pub concurrency: Option<usize>,
    pub paths: Option<Vec<PathBuf>>,
    pub macro_paths: Option<Vec<PathBuf>>,
    /// Group to instance name, from `--default`, its three older spellings and
    /// `BDDKIT_DEFAULT_<GROUP>`.
    pub defaults: BTreeMap<String, String>,
    pub polling_timeout_secs: Option<u64>,
    pub polling_interval_ms: Option<u64>,
    pub fail_fast: Option<bool>,
    pub tags: Option<Vec<String>>,
}

/// `--default <group>=<name>`: both halves are required.
fn group_name_pair(raw: &str) -> Result<(String, String), String> {
    match raw.split_once('=') {
        Some((group, name)) if !group.is_empty() && !name.is_empty() => {
            Ok((group.to_string(), name.to_string()))
        }
        _ => Err(format!("expected <group>=<name>, got {raw:?}")),
    }
}

/// The variable that sets a group's default: the group upper-cased, every
/// character that is not a letter or a digit replaced by `_`.
fn default_variable(group: &str) -> String {
    let group: String = group
        .to_uppercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    format!("BDDKIT_DEFAULT_{group}")
}

/// `get` is a variable lookup (`privacy::process_env` in a real run), injected
/// so a test never touches the process environment. `groups` are the groups
/// whose variable is read: `api`, `db` and `srp` always, any other plugin group
/// when the config declares it.
pub fn resolve(
    args: &SettingsArgs,
    groups: &[String],
    get: &dyn Fn(&str) -> Option<String>,
) -> Result<Overrides> {
    let get = |name: &str| get(name).filter(|value| !value.is_empty());
    let mut defaults = BTreeMap::new();
    // `--default-api`, `--default-db` and `--default-srp` are aliases kept from
    // when these were built in; all three are plugin groups now.
    let spelled = [
        ("api", &args.default_api),
        ("db", &args.default_db),
        ("srp", &args.default_srp),
    ]
    .into_iter()
    .filter_map(|(group, name)| Some((group.to_string(), name.clone()?)))
    .chain(args.default.iter().cloned());
    for (group, name) in spelled {
        if defaults.insert(group.clone(), name).is_some() {
            anyhow::bail!(
                "the default of the resource group {group:?} is given twice on the command line"
            );
        }
    }
    for group in groups {
        if !defaults.contains_key(group)
            && let Some(name) = get(&default_variable(group))
        {
            defaults.insert(group.clone(), name);
        }
    }
    // A value of nothing but separators is no list at all: unset, like an empty variable.
    let list = |name: &str| {
        get(name)
            .map(|v| crate::privacy::split_list(&v))
            .filter(|l| !l.is_empty())
    };
    let paths = |l: Vec<String>| l.into_iter().map(PathBuf::from).collect();
    Ok(Overrides {
        concurrency: num(
            args.concurrency,
            "BDDKIT_CONCURRENCY",
            get("BDDKIT_CONCURRENCY"),
        )?,
        paths: list("BDDKIT_PATHS").map(paths),
        macro_paths: args
            .macro_paths
            .clone()
            .or_else(|| list("BDDKIT_MACRO_PATHS").map(paths)),
        defaults,
        polling_timeout_secs: num(
            args.polling_timeout_secs,
            "BDDKIT_OPTIONS_POLLING_TIMEOUT_SECS",
            get("BDDKIT_OPTIONS_POLLING_TIMEOUT_SECS"),
        )?,
        polling_interval_ms: num(
            args.polling_interval_ms,
            "BDDKIT_OPTIONS_POLLING_INTERVAL_MS",
            get("BDDKIT_OPTIONS_POLLING_INTERVAL_MS"),
        )?,
        fail_fast: get("BDDKIT_FAIL_FAST")
            .map(|v| crate::privacy::env_bool("BDDKIT_FAIL_FAST", &v))
            .transpose()?,
        tags: list("BDDKIT_TAG"),
    })
}

/// A flag wins; else the variable's value, which must parse.
fn num<T: FromStr>(flag: Option<T>, name: &str, value: Option<String>) -> Result<Option<T>> {
    match flag {
        Some(n) => Ok(Some(n)),
        None => value.map(|v| parse(name, &v, "a whole number")).transpose(),
    }
}

fn parse<T: FromStr>(name: &str, value: &str, expected: &str) -> Result<T> {
    value
        .trim()
        .parse()
        .map_err(|_| anyhow::anyhow!("{name} is {value:?}, expected {expected}"))
}

impl Config {
    /// Lays the flags and the environment over what the file said, then
    /// resolves again: a defaulted name must exist, a polling pair must make sense.
    pub fn apply(&mut self, o: &Overrides) -> Result<()> {
        if let Some(n) = o.concurrency {
            self.concurrency = n;
        }
        if let Some(v) = &o.paths {
            self.paths.clone_from(v);
        }
        if let Some(v) = &o.macro_paths {
            self.macro_paths.clone_from(v);
        }
        for (group, name) in &o.defaults {
            self.set_default(group, name).with_context(|| {
                format!(
                    "the default of {group:?} came from --default or {}",
                    default_variable(group)
                )
            })?;
        }
        if o.polling_timeout_secs.is_some() || o.polling_interval_ms.is_some() {
            let polling = self.options.polling.get_or_insert_with(Default::default);
            polling.timeout_secs = o.polling_timeout_secs.or(polling.timeout_secs);
            polling.interval_ms = o.polling_interval_ms.or(polling.interval_ms);
        }
        if let Some(v) = o.fail_fast {
            self.fail_fast = v;
        }
        if let Some(v) = &o.tags {
            self.tags.clone_from(v);
        }
        self.resolve_options()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn groups(names: &[&str]) -> Vec<String> {
        names.iter().map(std::string::ToString::to_string).collect()
    }

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn a_flag_beats_the_environment_and_an_empty_variable_is_unset() {
        let args = SettingsArgs {
            concurrency: Some(2),
            ..Default::default()
        };
        let got = resolve(
            &args,
            &groups(&["api"]),
            &env(&[("BDDKIT_CONCURRENCY", "5"), ("BDDKIT_DEFAULT_API", "")]),
        )
        .unwrap();
        assert_eq!(got.concurrency, Some(2));
        assert!(got.defaults.is_empty());
        let got = resolve(
            &SettingsArgs::default(),
            &[],
            &env(&[("BDDKIT_CONCURRENCY", "5")]),
        )
        .unwrap();
        assert_eq!(got.concurrency, Some(5));
    }

    #[test]
    fn lists_split_on_commas_and_booleans_take_four_spellings() {
        let got = resolve(
            &SettingsArgs::default(),
            &[],
            &env(&[
                ("BDDKIT_TAG", "smoke, api"),
                ("BDDKIT_FAIL_FAST", "1"),
                ("BDDKIT_PATHS", "a,b"),
            ]),
        )
        .unwrap();
        assert_eq!(got.tags, Some(vec!["smoke".into(), "api".into()]));
        assert_eq!(got.fail_fast, Some(true));
        assert_eq!(got.paths, Some(vec!["a".into(), "b".into()]));
    }

    #[test]
    fn a_list_of_only_separators_is_unset() {
        let got = resolve(
            &SettingsArgs::default(),
            &[],
            &env(&[("BDDKIT_TAG", " , ")]),
        )
        .unwrap();
        assert_eq!(got.tags, None);
    }

    #[test]
    fn a_malformed_variable_names_itself() {
        for (name, value) in [
            ("BDDKIT_FAIL_FAST", "yes"),
            ("BDDKIT_CONCURRENCY", "many"),
            ("BDDKIT_OPTIONS_POLLING_TIMEOUT_SECS", "-1"),
        ] {
            let error = resolve(&SettingsArgs::default(), &[], &env(&[(name, value)])).unwrap_err();
            assert!(error.to_string().starts_with(name), "{error}");
        }
    }

    const BASE: &str = "paths: [f]\ndefault_api: a\nresources:\n  api:\n    a:\n      base_url: http://a.local\n      options: { polling: { timeout_secs: 9 } }\n    b:\n      base_url: http://b.local\n";

    fn config() -> Config {
        crate::config::load_str(BASE, std::path::Path::new("."), None).unwrap()
    }

    #[test]
    fn apply_replaces_lists_and_keeps_the_half_of_polling_nobody_set() {
        let raw = "paths: [f]\nresources:\n  s3:\n    a:\n      options: { polling: { timeout_secs: 9 } }\n";
        let mut cfg = crate::config::load_str(raw, std::path::Path::new("."), None).unwrap();
        cfg.apply(&Overrides {
            tags: Some(vec!["smoke".into()]),
            polling_interval_ms: Some(50),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(cfg.tags, ["smoke"]);
        // The global timeout is still the default; the resource's own wins over it.
        assert_eq!(cfg.effective_options.polling.timeout.as_secs(), 5);
        assert_eq!(cfg.effective_options.polling.interval.as_millis(), 50);
        assert_eq!(cfg.plugin_instances[0].options.polling.timeout.as_secs(), 9);
    }

    #[test]
    fn apply_refuses_an_undeclared_default_and_an_inverted_polling_pair() {
        let undeclared = Overrides {
            defaults: [("api".to_string(), "nope".to_string())].into(),
            ..Default::default()
        };
        assert!(config().apply(&undeclared).is_err());
        let inverted = Overrides {
            polling_interval_ms: Some(60_000),
            ..Default::default()
        };
        assert!(config().apply(&inverted).is_err());
    }

    #[test]
    fn a_default_comes_from_the_flag_then_the_variable_for_every_group() {
        let args = SettingsArgs {
            default: vec![("my-group".into(), "flag".into())],
            default_db: Some("db-flag".into()),
            default_srp: Some("srp-flag".into()),
            ..Default::default()
        };
        let got = resolve(
            &args,
            &groups(&["api", "db", "srp", "my-group", "other"]),
            &env(&[
                ("BDDKIT_DEFAULT_MY_GROUP", "var"),
                ("BDDKIT_DEFAULT_DB", "db-var"),
                ("BDDKIT_DEFAULT_SRP", "srp-var"),
                ("BDDKIT_DEFAULT_API", "api-var"),
                ("BDDKIT_DEFAULT_OTHER", "other-var"),
                ("BDDKIT_DEFAULT_UNDECLARED", "ignored"),
            ]),
        )
        .unwrap();
        let want = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
            pairs
                .iter()
                .map(|(g, n)| (g.to_string(), n.to_string()))
                .collect()
        };
        assert_eq!(
            got.defaults,
            want(&[
                ("api", "api-var"),
                ("db", "db-flag"),
                ("srp", "srp-flag"),
                ("my-group", "flag"),
                ("other", "other-var"),
            ])
        );
    }

    #[test]
    fn the_variable_of_a_group_is_its_upper_cased_name_with_symbols_as_underscores() {
        for (group, variable) in [
            ("api", "BDDKIT_DEFAULT_API"),
            ("my-group", "BDDKIT_DEFAULT_MY_GROUP"),
            ("s3.archive", "BDDKIT_DEFAULT_S3_ARCHIVE"),
            ("Echo", "BDDKIT_DEFAULT_ECHO"),
        ] {
            assert_eq!(default_variable(group), variable);
        }
    }

    #[test]
    fn one_group_given_twice_on_the_command_line_is_refused() {
        let args = SettingsArgs {
            default: vec![("g".into(), "a".into()), ("g".into(), "b".into())],
            ..Default::default()
        };
        let error = resolve(&args, &[], &env(&[])).unwrap_err().to_string();
        assert!(error.contains("\"g\""), "{error}");
    }

    #[test]
    fn an_old_flag_and_default_for_one_group_are_refused() {
        for (group, args) in [
            (
                "api",
                SettingsArgs {
                    default: vec![("api".into(), "a".into())],
                    default_api: Some("b".into()),
                    ..Default::default()
                },
            ),
            (
                "srp",
                SettingsArgs {
                    default: vec![("srp".into(), "a".into())],
                    default_srp: Some("b".into()),
                    ..Default::default()
                },
            ),
        ] {
            let error = resolve(&args, &[], &env(&[])).unwrap_err().to_string();
            assert!(error.contains(&format!("\"{group}\"")), "{error}");
        }
    }

    #[test]
    fn a_default_needs_a_group_and_a_name() {
        for bad in ["api", "=x", "api="] {
            assert!(group_name_pair(bad).is_err(), "{bad}");
        }
        assert_eq!(group_name_pair("a=b=c"), Ok(("a".into(), "b=c".into())));
    }

    const WIDGETS: &str = "paths: [f]\nresources:\n  api: {}\n  widget:\n    one: { bucket: b }\n    two: { bucket: b }\ndefault_widget: one\n";

    #[test]
    fn apply_sets_a_plugin_groups_default_and_refuses_what_is_not_declared() {
        let load = || crate::config::load_str(WIDGETS, std::path::Path::new("."), None).unwrap();
        let set = |group: &str, name: &str| {
            load().apply(&Overrides {
                defaults: [(group.to_string(), name.to_string())].into(),
                ..Default::default()
            })
        };
        let mut cfg = load();
        cfg.apply(&Overrides {
            defaults: [("widget".to_string(), "two".to_string())].into(),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            cfg.resolve_default_group("widget").unwrap().as_deref(),
            Some("two")
        );
        let error = format!("{:#}", set("widget", "three").unwrap_err());
        assert!(error.contains("three"), "{error}");
        assert!(error.contains("BDDKIT_DEFAULT_WIDGET"), "{error}");
        let error = set("gadget", "x").unwrap_err().to_string();
        assert!(error.contains("gadget"), "{error}");
    }
}
