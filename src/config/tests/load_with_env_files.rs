//! `config::load`: the `.env` layers and the migration hint.
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
    std::fs::write(dir.join(".env.prod"), "DB_PASS=prod_secret\n").expect("write .env.prod");
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
    std::fs::write(dir.join(".env.local"), "DB_PASS=local_secret\n").expect("write .env.local");
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

fn load_error(name: &str, yaml: Option<&str>) -> String {
    let path = std::env::temp_dir().join(format!("bddkit_cfg_hint_{name}.yaml"));
    match yaml {
        Some(yaml) => std::fs::write(&path, yaml).expect("write config"),
        None => {
            let _ = std::fs::remove_file(&path);
        }
    }
    format!("{:#}", load(&path, None).expect_err("config must not load"))
}

const FORMAT_HINT: &str = "predates the format change";

#[test]
fn an_old_format_config_gets_the_migration_hint() {
    // `suites` only lacks `paths`; `paths` only lacks `resources`.
    for (name, yaml) in [
        ("old_suites", "suites:\n  main:\n    paths: [f]\n"),
        ("old_no_resources", "paths: [f]\n"),
    ] {
        let error = load_error(name, Some(yaml));
        assert!(error.contains(FORMAT_HINT), "{name}: {error}");
    }
}

#[test]
fn other_config_errors_do_not_get_the_migration_hint() {
    // Each case also names the text of its own failure, so it proves it hit that path.
    for (name, yaml, cause) in [
        (
            "unset_var",
            Some("paths: [f]\nresources:\n  api: {}\nx: ${BDDKIT_CFG_HINT_UNSET_VAR}\n"),
            "BDDKIT_CFG_HINT_UNSET_VAR",
        ),
        ("unreadable", None, "failed to read config"),
        ("syntax", Some("paths: [f\n"), "failed to parse config"),
    ] {
        let error = load_error(name, yaml);
        assert!(error.contains(cause), "{name}: {error}");
        assert!(!error.contains(FORMAT_HINT), "{name}: {error}");
    }
}
