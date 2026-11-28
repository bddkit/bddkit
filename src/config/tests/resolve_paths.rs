//! `paths` and `macro_paths` resolve against the config's folder (issue #301).
use super::*;

fn resolved(config_dir: &Path, body: &str) -> Config {
    load_str(
        &format!("{body}resources: {{}}\n"),
        config_dir,
        &EnvArgs::default(),
    )
    .expect("loads")
}

#[test]
fn config_paths_resolve_against_the_config_folder() {
    let c = resolved(Path::new("module"), "paths: [features]\n");
    assert_eq!(c.paths, vec![PathBuf::from("module/features")]);
}

#[test]
fn config_macro_paths_resolve_against_the_config_folder() {
    let c = resolved(Path::new("module"), "paths: [f]\nmacro_paths: [macros]\n");
    assert_eq!(c.macro_paths, vec![PathBuf::from("module/macros")]);
}

#[test]
fn an_absolute_config_path_is_kept() {
    let abs = std::env::temp_dir().join("abs");
    let body = format!("paths: ['{}']\n", abs.display());
    assert_eq!(resolved(Path::new("module"), &body).paths, vec![abs]);
}

#[test]
fn a_config_in_the_working_directory_keeps_its_paths_as_written() {
    for dir in ["", "."] {
        assert_eq!(
            resolved(Path::new(dir), "paths: [features]\n").paths,
            vec![PathBuf::from("features")]
        );
    }
    let cwd = std::env::current_dir().expect("cwd");
    assert_eq!(
        resolved(&cwd.join("module"), "paths: [features]\n").paths,
        vec![PathBuf::from("module/features")],
        "a config named by an absolute path under the working directory prints relative"
    );
}

#[test]
fn load_reads_a_config_file() {
    let path = std::env::temp_dir().join("bddkit_cfg_load_reads_a_config_file.yaml");
    std::fs::write(
        &path,
        "paths: [features]\nresources:\n  api:\n    review:\n      base_url: http://review.local\n",
    )
    .expect("write config");
    let c = load(&path, &EnvArgs::default()).expect("config loads");
    // Relative to the config's folder, however that folder is spelled.
    let folder = std::path::absolute(std::env::temp_dir()).expect("absolute");
    let cwd = std::env::current_dir().expect("cwd");
    let folder = folder.strip_prefix(&cwd).unwrap_or(&folder);
    assert_eq!(c.paths, vec![folder.join("features")]);
}

#[test]
fn a_config_outside_the_working_directory_resolves_to_an_absolute_path() {
    let outside = std::path::absolute(Path::new(if cfg!(windows) { "C:\\" } else { "/" }))
        .expect("absolute")
        .join("bddkit-nowhere");
    let c = resolved(&outside, "paths: [features]\n");
    assert_eq!(c.paths, vec![outside.join("features")]);
}
