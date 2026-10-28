//! The options a config key, a flag and an environment variable all spell:
//! flag, then environment, then the config key, then the default. `run` and
//! `doctor` flatten the same `SettingsArgs` and call the same `resolve`, so
//! they cannot disagree. An empty variable is unset, a boolean is
//! `true|false|1|0`, a list is comma-separated; anything else is an error.
//! The privacy options have their own, older layer (`privacy.rs`).

use crate::config::Config;
use anyhow::Result;
use clap::Args;
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
    /// The API a scenario starts on [default: $BDDKIT_DEFAULT_API, else the config's `default_api`]
    #[arg(long = "default-api")]
    pub default_api: Option<String>,
    /// The database connection a scenario starts on [default: $BDDKIT_DEFAULT_DB, else the config's `default_db`]
    #[arg(long = "default-db")]
    pub default_db: Option<String>,
    /// The SRP configuration the SRP steps use [default: $BDDKIT_DEFAULT_SRP, else the config's `default_srp`]
    #[arg(long = "default-srp")]
    pub default_srp: Option<String>,
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
    pub default_api: Option<String>,
    pub default_db: Option<String>,
    pub default_srp: Option<String>,
    pub polling_timeout_secs: Option<u64>,
    pub polling_interval_ms: Option<u64>,
    pub fail_fast: Option<bool>,
    pub tags: Option<Vec<String>>,
}

/// `get` is a variable lookup (`privacy::process_env` in a real run), injected
/// so a test never touches the process environment.
pub fn resolve(args: &SettingsArgs, get: &dyn Fn(&str) -> Option<String>) -> Result<Overrides> {
    let get = |name: &str| get(name).filter(|value| !value.is_empty());
    let text = |flag: &Option<String>, name: &str| flag.clone().or_else(|| get(name));
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
        default_api: text(&args.default_api, "BDDKIT_DEFAULT_API"),
        default_db: text(&args.default_db, "BDDKIT_DEFAULT_DB"),
        default_srp: text(&args.default_srp, "BDDKIT_DEFAULT_SRP"),
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
            self.paths = v.clone();
        }
        if let Some(v) = &o.macro_paths {
            self.macro_paths = v.clone();
        }
        for (slot, value) in [
            (&mut self.default_api, &o.default_api),
            (&mut self.default_db, &o.default_db),
            (&mut self.default_srp, &o.default_srp),
        ] {
            if value.is_some() {
                *slot = value.clone();
            }
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
            self.tags = v.clone();
        }
        self.resolve_default_api()?;
        self.resolve_default_db()?;
        self.resolve_default_srp()?;
        self.resolve_options()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            &env(&[("BDDKIT_CONCURRENCY", "5"), ("BDDKIT_DEFAULT_API", "")]),
        )
        .unwrap();
        assert_eq!(got.concurrency, Some(2));
        assert_eq!(got.default_api, None);
        let got = resolve(
            &SettingsArgs::default(),
            &env(&[("BDDKIT_CONCURRENCY", "5")]),
        )
        .unwrap();
        assert_eq!(got.concurrency, Some(5));
    }

    #[test]
    fn lists_split_on_commas_and_booleans_take_four_spellings() {
        let got = resolve(
            &SettingsArgs::default(),
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
        let got = resolve(&SettingsArgs::default(), &env(&[("BDDKIT_TAG", " , ")])).unwrap();
        assert_eq!(got.tags, None);
    }

    #[test]
    fn a_malformed_variable_names_itself() {
        for (name, value) in [
            ("BDDKIT_FAIL_FAST", "yes"),
            ("BDDKIT_CONCURRENCY", "many"),
            ("BDDKIT_OPTIONS_POLLING_TIMEOUT_SECS", "-1"),
        ] {
            let error = resolve(&SettingsArgs::default(), &env(&[(name, value)])).unwrap_err();
            assert!(error.to_string().starts_with(name), "{error}");
        }
    }

    const BASE: &str = "paths: [f]\ndefault_api: a\nresources:\n  api:\n    a:\n      base_url: http://a.local\n      options: { polling: { timeout_secs: 9 } }\n    b:\n      base_url: http://b.local\n";

    fn config() -> Config {
        crate::config::load_str(BASE, std::path::Path::new("."), None).unwrap()
    }

    #[test]
    fn apply_replaces_lists_and_keeps_the_half_of_polling_nobody_set() {
        let mut cfg = config();
        cfg.apply(&Overrides {
            tags: Some(vec!["smoke".into()]),
            polling_interval_ms: Some(50),
            default_api: Some("b".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(cfg.tags, ["smoke"]);
        assert_eq!(cfg.default_api.as_deref(), Some("b"));
        // The global timeout is still the default; the resource's own wins over it.
        assert_eq!(cfg.effective_options.polling.timeout.as_secs(), 5);
        assert_eq!(cfg.effective_options.polling.interval.as_millis(), 50);
        assert_eq!(
            cfg.resources.api["a"]
                .effective_options
                .polling
                .timeout
                .as_secs(),
            9
        );
    }

    #[test]
    fn apply_refuses_an_undeclared_default_and_an_inverted_polling_pair() {
        let undeclared = Overrides {
            default_api: Some("nope".into()),
            ..Default::default()
        };
        assert!(config().apply(&undeclared).is_err());
        let inverted = Overrides {
            polling_interval_ms: Some(60_000),
            ..Default::default()
        };
        assert!(config().apply(&inverted).is_err());
    }
}
