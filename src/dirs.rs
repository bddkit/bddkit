//! Where `.bddkit/` files come from. One rule for every file in that directory:
//! the directory is the unit, and the directories form a chain — shared, user,
//! project — each read as `<stem>.yaml` then `<stem>.local.yaml`, later entries
//! overriding earlier ones. `--bddkit-dir` / `BDDKIT_DIR` replaces the whole
//! chain with exactly one directory.
//!
//! Hand-written on environment variables rather than the `directories` crate:
//! that crate puts the macOS user layer under `~/Library/Application Support`,
//! and this is a developer CLI whose files are edited in a terminal — `~/.config`
//! is the convention `gh`, `cargo` and `rustup` follow.

use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

// `MacOs` is the conventional casing and every later task in this plan names it
// verbatim, so it stays as-is rather than being renamed to silence this lint.
#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Linux,
    MacOs,
    Windows,
}

impl Os {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Os::MacOs
        } else if cfg!(windows) {
            Os::Windows
        } else {
            Os::Linux
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct Env {
    pub override_dir: Option<PathBuf>,
    pub home: Option<PathBuf>,
    pub xdg_config_home: Option<PathBuf>,
    pub xdg_data_home: Option<PathBuf>,
    pub local_app_data: Option<PathBuf>,
    pub program_data: Option<PathBuf>,
}

impl Env {
    pub fn from_process(flag: Option<PathBuf>) -> Self {
        let var = |name: &str| {
            std::env::var_os(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        };
        Env {
            override_dir: flag.or_else(|| var("BDDKIT_DIR")),
            home: var("HOME"),
            xdg_config_home: var("XDG_CONFIG_HOME"),
            xdg_data_home: var("XDG_DATA_HOME"),
            local_app_data: var("LOCALAPPDATA"),
            program_data: var("ProgramData"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layer {
    pub name: &'static str,
    pub dir: PathBuf,
}

pub fn layers(os: Os, env: &Env, config_dir: &Path) -> Result<Vec<Layer>> {
    if let Some(dir) = &env.override_dir {
        if !dir.is_dir() {
            bail!("bddkit directory {} does not exist", dir.display());
        }
        return Ok(vec![Layer {
            name: "override",
            dir: dir.clone(),
        }]);
    }
    let mut out = Vec::new();
    if let Some(dir) = shared(os, env) {
        out.push(Layer {
            name: "shared",
            dir,
        });
    }
    if let Some(dir) = user(os, env) {
        out.push(Layer { name: "user", dir });
    }
    if let Some(dir) = project(config_dir)? {
        out.push(Layer {
            name: "project",
            dir,
        });
    }
    Ok(out)
}

fn shared(os: Os, env: &Env) -> Option<PathBuf> {
    match os {
        Os::Linux => Some(PathBuf::from("/etc/bddkit")),
        Os::MacOs => Some(PathBuf::from("/Library/Application Support/bddkit")),
        Os::Windows => env.program_data.as_ref().map(|dir| dir.join("bddkit")),
    }
}

fn user(os: Os, env: &Env) -> Option<PathBuf> {
    match os {
        Os::Windows => env.local_app_data.as_ref().map(|dir| dir.join("bddkit")),
        Os::Linux | Os::MacOs => env
            .xdg_config_home
            .as_ref()
            .map(|dir| dir.join("bddkit"))
            .or_else(|| env.home.as_ref().map(|home| home.join(".config/bddkit"))),
    }
}

/// Nearest `.bddkit/` walking up from `config_dir`; nested ones are not merged.
pub fn project(config_dir: &Path) -> Result<Option<PathBuf>> {
    let start = std::path::absolute(config_dir)?;
    Ok(start
        .ancestors()
        .map(|dir| dir.join(".bddkit"))
        .find(|dir| dir.is_dir()))
}

/// Where the libraries a layer's lock file installs live. Config and data are
/// split the way FHS and XDG split them — `/etc` and `~/.config` hold what a
/// person edits, never a binary. `project` and `override` have one directory
/// to offer, so their libraries sit beside the lock file; so does every layer
/// on Windows and the shared one on macOS, whose directories are data
/// directories already.
pub fn data_dir(os: Os, env: &Env, layer: &Layer) -> PathBuf {
    match (layer.name, os) {
        ("shared", Os::Linux) => PathBuf::from("/usr/local/lib/bddkit"),
        ("user", Os::Linux | Os::MacOs) => env
            .xdg_data_home
            .as_ref()
            .map(|dir| dir.join("bddkit"))
            .or_else(|| {
                env.home
                    .as_ref()
                    .map(|home| home.join(".local/share/bddkit"))
            })
            .unwrap_or_else(|| layer.dir.clone()),
        _ => layer.dir.clone(),
    }
}

/// The layer `plugin install` writes to when no `--layer` names one: the
/// override if there is one; else the user layer if its directory exists;
/// else the shared layer if IT exists — a container image that ships
/// `/etc/bddkit/`; else the user layer, to be created. A project layer is
/// never picked: an install run inside a suite must not drop a binary into
/// the repository tree unasked.
pub fn install_layer(layers: &[Layer]) -> Option<&Layer> {
    let find = |name: &str| layers.iter().find(|layer| layer.name == name);
    if let Some(dir) = find("override") {
        return Some(dir);
    }
    let user = find("user");
    match (user, find("shared")) {
        (Some(user), _) if user.dir.is_dir() => Some(user),
        (_, Some(shared)) if shared.dir.is_dir() => Some(shared),
        _ => user,
    }
}

/// One file the chain reads, and the layer label `doctor` prints for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub layer: String,
    pub path: PathBuf,
}

/// `<stem>.yaml` then `<stem>.local.yaml` in every layer, lowest precedence
/// first — the same base/local pair as `.env` / `.env.local`.
pub fn candidates(layers: &[Layer], stem: &str) -> Vec<Candidate> {
    layers
        .iter()
        .flat_map(|layer| {
            [
                Candidate {
                    layer: layer.name.to_string(),
                    path: layer.dir.join(format!("{stem}.yaml")),
                },
                Candidate {
                    layer: format!("{}.local", layer.name),
                    path: layer.dir.join(format!("{stem}.local.yaml")),
                },
            ]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bddkit-dirs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    fn names(layers: &[Layer]) -> Vec<&'static str> {
        layers.iter().map(|layer| layer.name).collect()
    }

    #[test]
    fn linux_reads_shared_then_user_then_project() {
        let root = temp("linux");
        std::fs::create_dir_all(root.join(".bddkit")).expect("mkdir");
        let env = Env {
            home: Some(PathBuf::from("/home/t")),
            ..Env::default()
        };
        let layers = layers(Os::Linux, &env, &root).expect("resolves");
        assert_eq!(names(&layers), ["shared", "user", "project"]);
        assert_eq!(layers[0].dir, PathBuf::from("/etc/bddkit"));
        assert_eq!(layers[1].dir, PathBuf::from("/home/t/.config/bddkit"));
        assert_eq!(layers[2].dir, root.join(".bddkit"));
    }

    #[test]
    fn xdg_config_home_wins_over_home() {
        let env = Env {
            home: Some(PathBuf::from("/home/t")),
            xdg_config_home: Some(PathBuf::from("/xdg")),
            ..Env::default()
        };
        let layers = layers(Os::Linux, &env, Path::new("/nonexistent")).expect("resolves");
        assert_eq!(layers[1].dir, PathBuf::from("/xdg/bddkit"));
    }

    #[test]
    fn macos_user_layer_is_dot_config_not_library() {
        let env = Env {
            home: Some(PathBuf::from("/Users/t")),
            ..Env::default()
        };
        let layers = layers(Os::MacOs, &env, Path::new("/nonexistent")).expect("resolves");
        assert_eq!(
            layers[0].dir,
            PathBuf::from("/Library/Application Support/bddkit")
        );
        assert_eq!(layers[1].dir, PathBuf::from("/Users/t/.config/bddkit"));
    }

    #[test]
    fn windows_without_home_still_has_a_user_layer() {
        // HOME is normally unset on Windows; the user layer comes from LOCALAPPDATA.
        let env = Env {
            local_app_data: Some(PathBuf::from(r"C:\Users\t\AppData\Local")),
            program_data: Some(PathBuf::from(r"C:\ProgramData")),
            ..Env::default()
        };
        let layers = layers(Os::Windows, &env, Path::new("/nonexistent")).expect("resolves");
        assert_eq!(names(&layers), ["shared", "user"]);
        assert_eq!(
            layers[0].dir,
            PathBuf::from(r"C:\ProgramData").join("bddkit")
        );
        assert_eq!(
            layers[1].dir,
            PathBuf::from(r"C:\Users\t\AppData\Local").join("bddkit")
        );
    }

    #[test]
    fn no_user_variable_at_all_means_no_user_layer() {
        let layers =
            layers(Os::Linux, &Env::default(), Path::new("/nonexistent")).expect("resolves");
        assert_eq!(names(&layers), ["shared"]);
    }

    #[test]
    fn an_override_is_the_only_layer() {
        let root = temp("override");
        std::fs::create_dir_all(root.join(".bddkit")).expect("mkdir");
        let only = temp("override-only");
        let env = Env {
            override_dir: Some(only.clone()),
            home: Some(PathBuf::from("/home/t")),
            ..Env::default()
        };
        let layers = layers(Os::Linux, &env, &root).expect("resolves");
        assert_eq!(
            layers,
            vec![Layer {
                name: "override",
                dir: only
            }]
        );
    }

    #[test]
    fn a_missing_override_directory_is_an_error() {
        let env = Env {
            override_dir: Some(PathBuf::from("/nonexistent/bddkit-dir")),
            ..Env::default()
        };
        let error = layers(Os::Linux, &env, Path::new(".")).expect_err("must exist");
        assert!(
            format!("{error:#}").contains("/nonexistent/bddkit-dir"),
            "{error:#}"
        );
    }

    #[test]
    fn the_project_layer_is_the_nearest_dot_bddkit_walking_up() {
        let root = temp("walkup");
        std::fs::create_dir_all(root.join(".bddkit")).expect("mkdir outer");
        std::fs::create_dir_all(root.join("suites/a/.bddkit")).expect("mkdir inner");
        std::fs::create_dir_all(root.join("suites/b")).expect("mkdir b");
        assert_eq!(
            project(&root.join("suites/a")).expect("ok"),
            Some(root.join("suites/a/.bddkit"))
        );
        assert_eq!(
            project(&root.join("suites/b")).expect("ok"),
            Some(root.join(".bddkit"))
        );
    }

    #[test]
    fn a_relative_config_dir_walks_real_parents() {
        // `--config cfg.yaml` has the parent `.`; `Path::ancestors` on `.` would
        // stop at the empty path, so the walk-up must absolutize first.
        let found = project(Path::new(".")).expect("ok");
        let expected = std::path::absolute(".")
            .expect("abs")
            .ancestors()
            .map(|dir| dir.join(".bddkit"))
            .find(|dir| dir.is_dir());
        assert_eq!(found, expected);
    }

    fn layer(name: &'static str, dir: &Path) -> Layer {
        Layer {
            name,
            dir: dir.to_path_buf(),
        }
    }

    #[test]
    fn shared_libraries_live_outside_etc_on_linux() {
        let env = Env::default();
        let shared = layer("shared", Path::new("/etc/bddkit"));
        assert_eq!(
            data_dir(Os::Linux, &env, &shared),
            PathBuf::from("/usr/local/lib/bddkit")
        );
        let mac = layer("shared", Path::new("/Library/Application Support/bddkit"));
        assert_eq!(data_dir(Os::MacOs, &env, &mac), mac.dir);
        let win = layer("shared", Path::new(r"C:\ProgramData\bddkit"));
        assert_eq!(data_dir(Os::Windows, &env, &win), win.dir);
    }

    #[test]
    fn user_libraries_follow_xdg_data_home_then_local_share() {
        let user = layer("user", Path::new("/home/t/.config/bddkit"));
        let home_only = Env {
            home: Some(PathBuf::from("/home/t")),
            ..Env::default()
        };
        assert_eq!(
            data_dir(Os::Linux, &home_only, &user),
            PathBuf::from("/home/t/.local/share/bddkit")
        );
        assert_eq!(
            data_dir(Os::MacOs, &home_only, &user),
            PathBuf::from("/home/t/.local/share/bddkit")
        );
        let xdg = Env {
            home: Some(PathBuf::from("/home/t")),
            xdg_data_home: Some(PathBuf::from("/data")),
            ..Env::default()
        };
        assert_eq!(
            data_dir(Os::Linux, &xdg, &user),
            PathBuf::from("/data/bddkit")
        );
        let win = layer("user", Path::new(r"C:\Users\t\AppData\Local\bddkit"));
        assert_eq!(data_dir(Os::Windows, &xdg, &win), win.dir);
    }

    #[test]
    fn project_and_override_libraries_sit_beside_the_lock_file() {
        let env = Env::default();
        for name in ["project", "override"] {
            let l = layer(name, Path::new("/suite/.bddkit"));
            assert_eq!(data_dir(Os::Linux, &env, &l), l.dir);
        }
    }

    #[test]
    fn install_prefers_an_existing_user_dir_then_an_existing_shared_dir() {
        let root = temp("install-layer");
        let (user, shared, project) =
            (root.join("user"), root.join("shared"), root.join("project"));
        std::fs::create_dir_all(&project).expect("mkdir");
        let chain = [
            layer("shared", &shared),
            layer("user", &user),
            layer("project", &project),
        ];

        // Neither exists: the user layer, which install will create.
        assert_eq!(install_layer(&chain).map(|l| l.name), Some("user"));
        // Only shared exists — a container image shipping /etc/bddkit.
        std::fs::create_dir_all(&shared).expect("mkdir");
        assert_eq!(install_layer(&chain).map(|l| l.name), Some("shared"));
        // Both exist: user wins — no root needed, and it outranks shared anyway.
        std::fs::create_dir_all(&user).expect("mkdir");
        assert_eq!(install_layer(&chain).map(|l| l.name), Some("user"));
    }

    #[test]
    fn install_takes_the_override_and_never_the_project() {
        let root = temp("install-override");
        let chain = [layer("override", &root)];
        assert_eq!(install_layer(&chain).map(|l| l.name), Some("override"));
        // No user layer at all (no HOME): nothing to pick, the project is not a fallback.
        let chain = [layer("project", &root)];
        assert_eq!(install_layer(&chain), None);
    }

    #[test]
    fn every_layer_yields_base_then_local() {
        let layers = vec![
            Layer {
                name: "user",
                dir: PathBuf::from("/u"),
            },
            Layer {
                name: "project",
                dir: PathBuf::from("/p/.bddkit"),
            },
        ];
        let got: Vec<(String, PathBuf)> = candidates(&layers, "plugins")
            .into_iter()
            .map(|c| (c.layer, c.path))
            .collect();
        assert_eq!(
            got,
            vec![
                ("user".to_string(), PathBuf::from("/u/plugins.yaml")),
                (
                    "user.local".to_string(),
                    PathBuf::from("/u/plugins.local.yaml")
                ),
                (
                    "project".to_string(),
                    PathBuf::from("/p/.bddkit/plugins.yaml")
                ),
                (
                    "project.local".to_string(),
                    PathBuf::from("/p/.bddkit/plugins.local.yaml")
                ),
            ]
        );
    }
}
