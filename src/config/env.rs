//! The `.env` layers next to a config: which name selects them, which of them
//! are there, and what they hold.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::Path;

/// Reads one `.env` file into `map`; later keys (from later layers)
/// overwrite earlier ones. A missing file is not an error: in the
/// `.env`/`.env.local`/`.env.$APP_ENV`/`.env.$APP_ENV.local` stack, usually
/// not all four exist. `true` when the file was there.
fn load_env_file(path: &Path, map: &mut BTreeMap<String, String>) -> Result<bool> {
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
            Ok(true)
        }
        Err(dotenvy::Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
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

/// What a config's folder says about the environment.
pub(super) struct Layers {
    /// The values, a later layer over an earlier one.
    pub values: BTreeMap<String, String>,
    /// The files that were there, in load order, as `Config::env_layers` shows them.
    pub read: Vec<String>,
}

/// The full `.env` layer stack, in increasing priority order: a value from a
/// later layer overwrites an earlier one. A skipped file simply contributes
/// nothing — see `load_env_file`.
pub(super) fn load_env_layers(config_dir: &Path, app_env: &str) -> Result<Layers> {
    let shown = super::paths::shown_dir(config_dir);
    let mut values = BTreeMap::new();
    let mut read = Vec::new();
    for name in [
        ".env".to_string(),
        ".env.local".to_string(),
        format!(".env.{app_env}"),
        format!(".env.{app_env}.local"),
    ] {
        if load_env_file(&config_dir.join(&name), &mut values)? {
            read.push(crate::feature::display_path(&shown.join(name)));
        }
    }
    Ok(Layers { values, read })
}

/// Which `.env` layer a load from this path would select. `doctor` states it
/// in its header — it is the first thing that is wrong when a suite passes
/// locally and fails in CI — and `config::load` settles it the same way, so the
/// two can never name different layers.
///
/// path.parent() returns Some("") for a bare file name (not None) — join()
/// with "" gives a path relative to cwd, which is what the normal case wants;
/// unwrap_or here guards the case of a path with no parent at all
/// (root/prefix), not the main mechanism.
pub fn app_env_for(path: &Path, cli_env: Option<&str>) -> Result<String> {
    app_env_in(path.parent().unwrap_or_else(|| Path::new(".")), cli_env)
}

/// The `.env` files a load from this path would read, for `doctor`, which
/// states them even when the config itself does not load.
pub fn env_layers_for(path: &Path, cli_env: Option<&str>) -> Result<Vec<String>> {
    let config_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let app_env = app_env_in(config_dir, cli_env)?;
    Ok(load_env_layers(config_dir, &app_env)?.read)
}

/// The same answer from the directory alone, for a caller holding text rather
/// than a path.
pub(super) fn app_env_in(config_dir: &Path, cli_env: Option<&str>) -> Result<String> {
    let mut base_map = BTreeMap::new();
    load_env_file(&config_dir.join(".env"), &mut base_map)?;
    Ok(resolve_app_env(cli_env, &base_map))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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
        let read =
            load_env_file(&dir.join(".env"), &mut map).expect("a missing file is not an error");
        assert!(!read);
        assert!(map.is_empty());
    }

    #[test]
    fn load_env_file_reads_key_value_pairs() {
        let dir = unique_dir("basic_read");
        std::fs::write(dir.join(".env"), "FOO=bar\nBAZ=qux\n").expect("write .env");
        let mut map = BTreeMap::new();
        assert!(load_env_file(&dir.join(".env"), &mut map).expect(".env reads"));
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
        std::fs::write(dir.join(".env"), "A=base\nB=base\nC=base\nD=base\n").expect("write .env");
        std::fs::write(dir.join(".env.local"), "B=local\nC=local\nD=local\n")
            .expect("write .env.local");
        std::fs::write(dir.join(".env.dev"), "C=dev\nD=dev\n").expect("write .env.dev");
        std::fs::write(dir.join(".env.dev.local"), "D=dev_local\n").expect("write .env.dev.local");

        let map = load_env_layers(&dir, "dev").expect("layers read").values;
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
            .expect("missing .env.local/.env.prod* is not an error")
            .values;
        assert_eq!(map.get("A"), Some(&"base".to_string()));
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn resolve_app_env_cli_flag_wins() {
        let base = BTreeMap::new();
        let resolved =
            resolve_app_env_with_process_env(Some("from_cli"), Some("from_process".into()), &base);
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
        let resolved = resolve_app_env_with_process_env(None, Some("from_real_env".into()), &base);
        assert_eq!(resolved, "from_real_env");
    }
}
