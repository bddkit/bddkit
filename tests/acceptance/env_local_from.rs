//! `--env-local-from <dir>` / `BDDKIT_ENV_LOCAL_FROM`: the `*.local` layers
//! come from another checkout, as for a run in a git worktree (#312).

use super::{common, path_text, read_stream};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::{Command, Output};

/// Each layer sets its own variable, and the config's `tags` name all four, so
/// the scenarios a command selects say which files were read.
const TAGS: &str = "tags: ['${BASE:-unset}', '${LOCAL:-unset}', '${DEV:-unset}', '${DEVLOCAL:-unset}']\n\
                    paths: [features]\nresources: {}\n";
const LAYERS: [(&str, &str); 4] = [
    (".env", "BASE"),
    (".env.local", "LOCAL"),
    (".env.dev", "DEV"),
    (".env.dev.local", "DEVLOCAL"),
];
const OWNERS: [&str; 3] = ["own", "other", "third"];

/// `<root>/work` is the checkout bddkit is started from; `<root>/other` is the
/// one that holds the credentials. Canonical, so that the absolute paths a test
/// expects are the ones bddkit prints where the temp folder is a symlink.
struct Checkouts(PathBuf);

impl Checkouts {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("bddkit-envlocal-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["work", "other", "no-plugins"] {
            std::fs::create_dir_all(root.join(dir)).expect("mkdir");
        }
        Self(root.canonicalize().expect("real path"))
    }

    fn dir(&self, relative: &str) -> PathBuf {
        self.0.join(relative)
    }

    /// A config in `work/<folder>` with one scenario per layer and owner.
    fn suite(&self, folder: &str) {
        let dir = self.0.join("work").join(folder);
        std::fs::create_dir_all(dir.join("features")).expect("mkdir");
        std::fs::write(dir.join("bddkit.yaml"), TAGS).expect("write config");
        let mut feature = String::from("Feature: f\n");
        for owner in OWNERS {
            for (_, variable) in LAYERS {
                let tag = format!("{owner}-{}", variable.to_lowercase());
                feature.push_str(&format!(
                    "  @{tag}\n  Scenario: {tag}\n    Given set variable \"x\" to \"1\"\n"
                ));
            }
        }
        std::fs::write(dir.join("features/f.feature"), feature).expect("write feature");
    }

    /// The named layer files in `<root>/<folder>`, each naming `owner`.
    fn layers(&self, folder: &str, owner: &str, files: &[&str]) {
        let dir = self.0.join(folder);
        std::fs::create_dir_all(&dir).expect("mkdir");
        for (file, variable) in LAYERS {
            if files.contains(&file) {
                let content = format!("{variable}={owner}-{}\n", variable.to_lowercase());
                std::fs::write(dir.join(file), content).expect("write layer");
            }
        }
    }

    fn all_layers(&self, folder: &str, owner: &str) {
        self.layers(folder, owner, &LAYERS.map(|(file, _)| file));
    }

    /// A command that sees nothing of the developer's own choice.
    fn bddkit(&self, cwd: &str, args: &[&str], envs: &[(&str, &str)]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_bddkit"));
        common::isolate_telemetry(&mut command)
            .env_remove("APP_ENV")
            .env_remove("BDDKIT_ENV_LOCAL_FROM")
            .env_remove("BDDKIT_TAG")
            .env_remove("BDDKIT_PATHS")
            .env("BDDKIT_DIR", self.dir("no-plugins"))
            .envs(envs.iter().copied())
            .current_dir(self.dir(cwd))
            .args(args)
            .output()
            .expect("run bddkit")
    }

    /// The scenarios `list` selects for `work/bddkit.yaml`-like configs.
    fn selected(&self, cwd: &str, args: &[&str], envs: &[(&str, &str)]) -> Vec<String> {
        let out = self.bddkit(cwd, &[&["list", "--json"], args].concat(), envs);
        assert_eq!(out.status.code(), Some(0), "{}", common::combined(&out));
        let list: Value = serde_json::from_slice(&out.stdout).expect("stdout is JSON");
        list["files"][0]["scenarios"]
            .as_array()
            .expect("scenarios")
            .iter()
            .map(|s| s["name"].as_str().expect("name").to_string())
            .collect()
    }
}

const FROM_OTHER: [&str; 2] = ["--env-local-from", "../other"];

#[test]
fn env_local_from_reads_the_local_layers_from_the_other_root() {
    let c = Checkouts::new("reads");
    c.suite("");
    c.layers("work", "own", &[".env", ".env.dev"]);
    c.layers("other", "other", &[".env.local", ".env.dev.local"]);
    // In the order the feature file declares them.
    assert_eq!(
        c.selected("work", &FROM_OTHER, &[]),
        ["own-base", "own-dev", "other-local", "other-devlocal"]
    );
}

#[test]
fn env_local_from_ignores_the_local_layers_of_the_config_folder() {
    let c = Checkouts::new("ignores");
    c.suite("");
    c.all_layers("work", "own");
    // The other checkout has no `*.local` file at all: nothing stands in.
    assert_eq!(
        c.selected("work", &FROM_OTHER, &[]),
        ["own-base", "own-dev"]
    );
}

#[test]
fn an_app_env_named_local_takes_every_local_file_from_the_other_root() {
    let c = Checkouts::new("applocal");
    c.suite("");
    c.all_layers("work", "own");
    // Here `.env.<APP_ENV>` is `.env.local` too: not the config folder's.
    let args = [&FROM_OTHER[..], &["--env", "local"]].concat();
    assert_eq!(c.selected("work", &args, &[]), ["own-base"]);
}

#[test]
fn env_local_from_keeps_the_tracked_layers_of_the_config_folder() {
    let c = Checkouts::new("tracked");
    c.suite("");
    c.layers("work", "own", &[".env", ".env.dev"]);
    c.all_layers("other", "other");
    let selected = c.selected("work", &FROM_OTHER, &[]);
    for name in ["own-base", "own-dev"] {
        assert!(selected.iter().any(|s| s == name), "{selected:?}");
    }
    for name in ["other-base", "other-dev"] {
        assert!(!selected.iter().any(|s| s == name), "{selected:?}");
    }
}

#[test]
fn env_local_from_resolves_the_config_folder_relative_to_the_working_directory() {
    let c = Checkouts::new("relative");
    c.suite("services/api");
    c.layers("other/services/api", "other", &[".env.local"]);
    // Neither the other root itself nor the same folder one level up is read.
    c.layers("other", "third", &[".env.local"]);
    c.layers("other/api", "third", &[".env.local"]);
    let other = path_text(&c.dir("other"));
    let absolute = path_text(&c.dir("work/services/api/bddkit.yaml"));
    for config in ["services/api/bddkit.yaml", absolute.as_str()] {
        let args = ["--config", config, "--env-local-from", &other];
        assert_eq!(c.selected("work", &args, &[]), ["other-local"], "{config}");
    }
    // Started inside the config's folder, the relative part is empty: `third`.
    let args = ["--env-local-from", &other];
    assert_eq!(c.selected("work/services/api", &args, &[]), ["third-local"]);
}

/// The system names the working directory with its links resolved, a config
/// path may not: the same folder is still under it.
#[cfg(unix)]
#[test]
fn env_local_from_takes_a_config_named_through_a_link_as_under_the_working_directory() {
    let c = Checkouts::new("linked");
    c.suite("services/api");
    c.layers("other/services/api", "other", &[".env.local"]);
    std::os::unix::fs::symlink(c.dir("work"), c.dir("link")).expect("symlink");
    let config = path_text(&c.dir("link/services/api/bddkit.yaml"));
    let other = path_text(&c.dir("other"));
    let args = ["--config", &config, "--env-local-from", &other];
    assert_eq!(c.selected("link", &args, &[]), ["other-local"]);
}

/// A linked folder inside the checkout keeps the name the config was given
/// by, wherever it points.
#[cfg(unix)]
#[test]
fn env_local_from_takes_a_linked_config_folder_by_the_name_it_is_given() {
    let c = Checkouts::new("linkedfolder");
    c.suite("../elsewhere");
    c.layers("other/api", "other", &[".env.local"]);
    std::os::unix::fs::symlink(c.dir("elsewhere"), c.dir("work/api")).expect("symlink");
    let mut args = FROM_OTHER.to_vec();
    args.extend(["--config", "api/bddkit.yaml"]);
    assert_eq!(c.selected("work", &args, &[]), ["other-local"]);
}

#[test]
fn env_local_from_with_a_config_outside_the_working_directory_is_an_error() {
    let c = Checkouts::new("outside");
    c.suite("");
    let config = path_text(&c.dir("work/bddkit.yaml"));
    for command in ["list", "run"] {
        let out = c.bddkit(
            "other",
            &[command, "--config", &config, "--env-local-from", "."],
            &[],
        );
        let text = common::combined(&out);
        assert_eq!(out.status.code(), Some(2), "{command}: {text}");
        assert!(
            text.contains("--env-local-from needs a config under the working directory"),
            "{command}: {text}"
        );
    }
}

#[test]
fn env_local_from_that_is_not_a_directory_is_an_error() {
    let c = Checkouts::new("notdir");
    c.suite("");
    std::fs::write(c.dir("other/file"), "").expect("write file");
    for target in ["../missing", "../other/file"] {
        let flag = ["--env-local-from", target];
        for (command, code) in [("list", 2), ("run", 2), ("doctor", 1)] {
            let out = c.bddkit("work", &[&[command], &flag[..]].concat(), &[]);
            let text = common::combined(&out);
            assert_eq!(out.status.code(), Some(code), "{command} {target}: {text}");
            assert!(
                text.contains(&format!("--env-local-from {target} is not a directory")),
                "{command} {target}: {text}"
            );
        }
    }
    let out = c.bddkit(
        "work",
        &["list"],
        &[("BDDKIT_ENV_LOCAL_FROM", "../missing")],
    );
    let text = common::combined(&out);
    assert!(
        text.contains("BDDKIT_ENV_LOCAL_FROM ../missing is not a directory"),
        "{text}"
    );
}

#[test]
fn the_flag_beats_the_variable_and_an_empty_variable_is_unset() {
    let c = Checkouts::new("precedence");
    c.suite("");
    c.layers("work", "own", &[".env.local"]);
    c.layers("other", "other", &[".env.local"]);
    std::fs::create_dir_all(c.dir("third")).expect("mkdir");
    c.layers("third", "third", &[".env.local"]);
    let variable = [("BDDKIT_ENV_LOCAL_FROM", "../third")];
    assert_eq!(c.selected("work", &[], &variable), ["third-local"]);
    assert_eq!(c.selected("work", &FROM_OTHER, &variable), ["other-local"]);
    let empty = [("BDDKIT_ENV_LOCAL_FROM", "")];
    assert_eq!(c.selected("work", &[], &empty), ["own-local"]);
}

#[test]
fn bddkit_env_local_from_in_a_dotenv_layer_is_not_read() {
    let c = Checkouts::new("dotenv");
    c.suite("");
    c.layers("other", "other", &[".env.local"]);
    for file in [".env", ".env.local"] {
        std::fs::write(
            c.dir("work").join(file),
            "BDDKIT_ENV_LOCAL_FROM=../other\nLOCAL=own-local\n",
        )
        .expect("write layer");
    }
    assert_eq!(c.selected("work", &[], &[]), ["own-local"]);
}

#[test]
fn env_layers_show_the_paths_taken_from_the_other_root() {
    let c = Checkouts::new("shown");
    c.suite("api");
    c.layers("work/api", "own", &[".env", ".env.local"]);
    c.layers("other/api", "other", &[".env.local", ".env.dev.local"]);
    c.layers("work/copy/api", "other", &[".env.local"]);
    let shown = |root: &str| -> (Value, Value) {
        let flags = ["--config", "api/bddkit.yaml", "--env-local-from", root];
        let run = [&["run", "--events", "events.ndjson"], &flags[..]].concat();
        let out = c.bddkit("work", &run, &[]);
        assert_eq!(out.status.code(), Some(0), "{}", common::combined(&out));
        let started = read_stream(&c.dir("work/events.ndjson")).remove(0);
        let doctor = [&["doctor", "--json"], &flags[..]].concat();
        let report: Value = serde_json::from_slice(&c.bddkit("work", &doctor, &[]).stdout)
            .expect("--json emits JSON only");
        (started["env_layers"].clone(), report["env_layers"].clone())
    };
    // Outside the working directory the paths are absolute.
    let other = c.dir("other/api");
    let outside = json!([
        "api/.env",
        path_text(&other.join(".env.local")),
        path_text(&other.join(".env.dev.local"))
    ]);
    assert_eq!(shown("../other"), (outside.clone(), outside));
    // A root under it is relative to it, like any other path.
    let inside = json!(["api/.env", "copy/api/.env.local"]);
    assert_eq!(shown("copy"), (inside.clone(), inside));
}

#[test]
fn every_command_with_env_takes_the_flag() {
    let c = Checkouts::new("commands");
    c.suite("");
    let flag = ["--env-local-from", "../missing"];
    let commands: [&[&str]; 5] = [
        &["run"],
        &["list"],
        &["list-all", "bddkit.yaml"],
        &["doctor"],
        &["resource", "add", "s3", "main"],
    ];
    for command in commands {
        let out = c.bddkit("work", &[command, &flag[..]].concat(), &[]);
        let text = common::combined(&out);
        assert!(
            text.contains("--env-local-from ../missing is not a directory"),
            "{command:?}: {text}"
        );
    }
}

#[test]
fn the_variable_reaches_commands_that_have_no_env_flag() {
    let c = Checkouts::new("novar");
    c.suite("");
    let variable = [("BDDKIT_ENV_LOCAL_FROM", "../missing")];
    let commands: [&[&str]; 2] = [&["steps", "list"], &["resource", "fields"]];
    for command in commands {
        let out = c.bddkit("work", command, &variable);
        let text = common::combined(&out);
        assert_eq!(out.status.code(), Some(2), "{command:?}: {text}");
        assert!(
            text.contains("BDDKIT_ENV_LOCAL_FROM ../missing is not a directory"),
            "{command:?}: {text}"
        );
    }
}
