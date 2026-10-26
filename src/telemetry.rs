//! Anonymous usage counts (`telemetry.md`): whether to collect, the question
//! that asks, and where the answer lives. Nothing here sends anything yet.
//!
//! Off until a person says yes. One boolean, decided by the first source that
//! has an opinion — flag, `BDDKIT_TELEMETRY`, `DO_NOT_TRACK`, the config key,
//! the `.bddkit/` layers, then the default.

use crate::dirs::{self, Layer};
use anyhow::{Context as _, Result, bail};
use dialoguer::Select;
use dialoguer::console::Term;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// The question of `telemetry.md`, section 6, word for word.
pub const QUESTION_TEXT: &str =
    "bddkit can send anonymous usage counts to its maintainers after each run:
the bddkit version and OS, how long the run took, how many files, scenarios
and failures it had, and which kinds of resources the suite uses (APIs,
database engines, official plugins).
Never sent: names, step text, URLs, addresses, values, your config.
Your IP address reaches our server with the request and is dropped there;
it is never stored and never passed on.
bddkit is an open-source project; these counts are how we learn which
databases, plugins and suite sizes to work on, and whether a release broke
runs out there. The full list: https://bddkit.dev/telemetry
Change your answer any time: bddkit telemetry on|off
";

pub mod command;
pub mod counters;
pub mod doctor;
mod end;
pub mod payload;
pub mod run;

pub use end::{Ending, end, record};
pub use run::RunProbe;

pub const PROMPT: &str = "Send anonymous usage counts?";

/// The `usage` key, and the plugin bucket, for everything that is not named.
pub const OTHER: &str = "other";

/// A plugin is named in a payload only when its lock entry's `source` is a
/// repository of the `bddkit` owner. A hand-written entry has no source and
/// is never official; neither the name nor the repository of any other
/// plugin leaves the machine.
pub fn is_official(source: Option<&str>) -> bool {
    source
        .and_then(|source| source.split_once('/'))
        .is_some_and(|(owner, repo)| owner.eq_ignore_ascii_case("bddkit") && !repo.is_empty())
}

/// The builtin step groups, as `BUILTIN_STEPS` declares them, once per step.
pub fn builtin_groups<'a>() -> impl Iterator<Item = &'a str> {
    crate::steps::BUILTIN_STEPS.iter().map(|def| def.group)
}

/// The one place the code names the collector's domain.
pub const DEFAULT_ENDPOINT: &str = "https://telemetry.bddkit.dev";

/// Where a payload would go, or `None` for a build or a run that sends
/// nowhere. A debug build never reaches the real collector: only
/// `BDDKIT_TELEMETRY_URL` counts, so a test run cannot report to production.
/// A release build may be given its own collector at compile time through
/// `BDDKIT_TELEMETRY_ENDPOINT` (empty: a binary that never sends); the
/// run-time variable replaces either.
pub fn endpoint(get: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    endpoint_with(
        get,
        option_env!("BDDKIT_TELEMETRY_ENDPOINT"),
        cfg!(debug_assertions),
    )
}

fn endpoint_with(
    get: &dyn Fn(&str) -> Option<String>,
    built_in: Option<&str>,
    debug: bool,
) -> Option<String> {
    if let Some(url) = get("BDDKIT_TELEMETRY_URL") {
        return Some(url);
    }
    if debug {
        return None;
    }
    let url = built_in.unwrap_or(DEFAULT_ENDPOINT);
    (!url.is_empty()).then(|| url.to_string())
}

/// The CI system a run is under, detected from whether a variable is set —
/// never from its value. An empty variable is unset, as everywhere else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Ci {
    Github,
    Gitlab,
    Jenkins,
    Buildkite,
    Circleci,
    Azure,
    Teamcity,
    Bitbucket,
    Other,
}

/// The most specific system first, then the generic `CI`.
pub fn ci(get: &dyn Fn(&str) -> Option<String>) -> Option<Ci> {
    const SYSTEMS: [(&str, Ci); 8] = [
        ("GITHUB_ACTIONS", Ci::Github),
        ("GITLAB_CI", Ci::Gitlab),
        ("JENKINS_URL", Ci::Jenkins),
        ("BUILDKITE", Ci::Buildkite),
        ("CIRCLECI", Ci::Circleci),
        ("TF_BUILD", Ci::Azure),
        ("TEAMCITY_VERSION", Ci::Teamcity),
        ("BITBUCKET_BUILD_NUMBER", Ci::Bitbucket),
    ];
    SYSTEMS
        .iter()
        .find(|(name, _)| get(name).is_some())
        .map(|(_, ci)| *ci)
        .or_else(|| get("CI").map(|_| Ci::Other))
}

/// What decided, spelled as the person would look for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Flag,
    Env,
    DoNotTrack,
    Config,
    /// A config that could not be read: off, because the team's own `false`
    /// may be the line that did not load.
    BrokenConfig,
    /// A `telemetry.yaml` in a `.bddkit/` layer, by the label `doctor` prints
    /// for it (`user`, `shared.local`, …).
    Layer(String),
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Source::Flag => f.write_str("--telemetry"),
            Source::Env => f.write_str("BDDKIT_TELEMETRY"),
            Source::DoNotTrack => f.write_str("DO_NOT_TRACK"),
            Source::Config => f.write_str("the telemetry key in the config"),
            Source::BrokenConfig => f.write_str("a config that could not be read"),
            Source::Layer(layer) => write!(f, "the {layer} layer"),
        }
    }
}

/// `by` is `None` for the default, which is also what «nobody has said» means:
/// the only state in which the question is asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub on: bool,
    pub by: Option<Source>,
}

impl Decision {
    /// `on (decided by DO_NOT_TRACK)`, the shape of the `privacy` row.
    pub fn describe(&self) -> String {
        let state = if self.on { "on" } else { "off" };
        match &self.by {
            Some(by) => format!("{state} (decided by {by})"),
            None => format!("{state} (default, nobody has answered)"),
        }
    }
}

/// What the config says about telemetry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigKey {
    Absent,
    Set(bool),
    /// Not found, not readable, not YAML, or the key is not a boolean.
    Broken,
}

impl From<&Option<bool>> for ConfigKey {
    fn from(key: &Option<bool>) -> Self {
        key.map_or(ConfigKey::Absent, ConfigKey::Set)
    }
}

/// Reads the one key and nothing else, for the commands that must not load a
/// config (`plugin`, `report`, `upgrade`): an unset `${ENV}` elsewhere in the
/// file is not a reason to call it broken, so it is not expanded.
pub fn config_key(path: Option<&Path>) -> ConfigKey {
    let Some(path) = path else {
        return ConfigKey::Absent;
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return ConfigKey::Broken;
    };
    match serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&text) {
        Ok(serde_yaml_ng::Value::Mapping(map)) => match map.get("telemetry") {
            None | Some(serde_yaml_ng::Value::Null) => ConfigKey::Absent,
            Some(serde_yaml_ng::Value::Bool(on)) => ConfigKey::Set(*on),
            Some(_) => ConfigKey::Broken,
        },
        _ => ConfigKey::Broken,
    }
}

/// Everything the decision reads, so a test can hand it a world.
pub struct Sources<'a> {
    pub flag: Option<bool>,
    pub get: &'a dyn Fn(&str) -> Option<String>,
    pub config: ConfigKey,
    /// `telemetry.yaml` files in the order the chain reads them, lowest
    /// precedence first, as `layer_answers` returns them.
    pub layers: Vec<(String, bool)>,
}

/// The first source with an opinion wins. A value of `BDDKIT_TELEMETRY` that
/// is neither `true|1` nor `false|0` is off: a typo must never turn counting
/// on.
pub fn decide(sources: &Sources) -> Decision {
    let decided = |on, by| Decision { on, by: Some(by) };
    if let Some(on) = sources.flag {
        return decided(on, Source::Flag);
    }
    if let Some(value) = (sources.get)("BDDKIT_TELEMETRY") {
        let on = matches!(value.trim(), "true" | "1");
        return decided(on, Source::Env);
    }
    if matches!(
        (sources.get)("DO_NOT_TRACK").as_deref().map(str::trim),
        Some("1" | "true")
    ) {
        return decided(false, Source::DoNotTrack);
    }
    match sources.config {
        ConfigKey::Set(on) => return decided(on, Source::Config),
        ConfigKey::Broken => return decided(false, Source::BrokenConfig),
        ConfigKey::Absent => {}
    }
    match sources.layers.last() {
        Some((layer, on)) => decided(*on, Source::Layer(layer.clone())),
        None => Decision {
            on: false,
            by: None,
        },
    }
}

/// `decide` over this process's environment and the answer files of `layers`:
/// what `run` and `doctor` do, which have a flag, and the commands that have
/// none (`Setting`).
pub fn decide_here(flag: Option<bool>, config: ConfigKey, layers: &[Layer]) -> Decision {
    decide(&Sources {
        flag,
        get: &crate::privacy::process_env,
        config,
        layers: layer_answers(layers),
    })
}

/// The question is asked when nobody has decided and a person can answer.
pub fn should_ask(decision: &Decision, can_prompt: bool) -> bool {
    decision.by.is_none() && can_prompt
}

/// The answers the chain holds, lowest precedence first. A file that cannot be
/// read as `enabled: <bool>` answers no, by its own label: a half-written
/// consent is not one.
pub fn layer_answers(layers: &[Layer]) -> Vec<(String, bool)> {
    dirs::candidates(layers, "telemetry")
        .into_iter()
        .filter_map(|candidate| {
            let text = match std::fs::read_to_string(&candidate.path) {
                Ok(text) => text,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
                Err(_) => return Some((candidate.layer, false)),
            };
            let enabled = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&text)
                .ok()
                .and_then(|value| value.get("enabled")?.as_bool());
            Some((candidate.layer, enabled.unwrap_or(false)))
        })
        .collect()
}

/// The directory the person's own files live in: the override directory when
/// there is one, else the user layer.
fn own_layer(layers: &[Layer]) -> Option<&Layer> {
    layers
        .iter()
        .find(|layer| layer.name == "override")
        .or_else(|| layers.iter().find(|layer| layer.name == "user"))
}

/// `telemetry-last.json`: always the user layer, or the override directory.
pub fn last_payload_path(layers: &[Layer]) -> Option<PathBuf> {
    own_layer(layers).map(|layer| layer.dir.join("telemetry-last.json"))
}

/// The file `on`/`off` and the question write: the user layer's `telemetry.yaml`
/// by default, or the one `--layer` names (`shared`, `project.local`, …).
pub fn answer_path(layers: &[Layer], named: Option<&str>) -> Result<PathBuf> {
    match named {
        None => own_layer(layers)
            .map(|layer| layer.dir.join("telemetry.yaml"))
            .context("there is no user layer to keep the answer in (no home directory)"),
        Some(name) => dirs::candidates(layers, "telemetry")
            .into_iter()
            .find(|candidate| candidate.layer == name)
            .map(|candidate| candidate.path)
            .with_context(|| format!("there is no {name} layer here")),
    }
}

/// Sets `enabled` and leaves any other key of the file as it was.
pub fn store_answer(path: &Path, enabled: bool) -> Result<()> {
    let mut map = match std::fs::read_to_string(path) {
        Ok(text) => match serde_yaml_ng::from_str(&text)? {
            serde_yaml_ng::Value::Mapping(map) => map,
            serde_yaml_ng::Value::Null => Default::default(),
            _ => bail!("{} is not a mapping", path.display()),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Default::default(),
        Err(error) => return Err(error).with_context(|| format!("cannot read {}", path.display())),
    };
    map.insert("enabled".into(), enabled.into());
    write_atomically(path, &serde_yaml_ng::to_string(&map)?)
}

/// A temporary name in the same directory, then a rename: two commands ending
/// at once (parallel CI jobs sharing a home) replace the file whole, one after
/// the other, and never leave half of either.
pub fn write_atomically(path: &Path, text: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&temporary, text)
        .and_then(|()| std::fs::rename(&temporary, path))
        .inspect_err(|_| {
            let _ = std::fs::remove_file(&temporary);
        })
        .with_context(|| format!("cannot write {}", path.display()))
}

/// A line answer: `y`/`yes`, `n`/`no`, any case. Anything else, an empty line
/// included, is not an answer, because there is no default.
pub fn parse_line(line: &str) -> Option<bool> {
    match line.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => Some(true),
        "n" | "no" => Some(false),
        _ => None,
    }
}

/// A yes/no question, `items` being its two answers, yes first: drawn on
/// stderr with nothing highlighted, so a yes takes a deliberate move. `None`: Esc, `q`, Ctrl-C or end of input — nothing is
/// stored. Where the terminal cannot run the prompt — `TERM` unset or `dumb`,
/// which `dialoguer` would still draw with cursor movements, or a console
/// that makes it fail — the same question is asked as a `[y/n]` line.
pub fn ask_yes_no(term: &Term, prompt: &str, items: [&str; 2]) -> Result<Option<bool>> {
    if dialoguer::console::is_dumb() {
        return ask_line(&mut std::io::stdin().lock(), term, prompt);
    }
    let selected = Select::new()
        .with_prompt(prompt)
        .items(items)
        .interact_on_opt(term);
    match selected {
        Ok(choice) => Ok(choice.map(|index| index == 0)),
        Err(dialoguer::Error::IO(error)) if error.kind() == std::io::ErrorKind::Interrupted => {
            Ok(None)
        }
        Err(dialoguer::Error::IO(_)) => ask_line(&mut std::io::stdin().lock(), term, prompt),
    }
}

fn ask_line(input: &mut dyn std::io::BufRead, term: &Term, prompt: &str) -> Result<Option<bool>> {
    loop {
        let _ = term.write_str(&format!("{prompt} [y/n] "));
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        if let Some(answer) = parse_line(&line) {
            return Ok(Some(answer));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            vars.iter()
                .find(|(key, value)| *key == name && !value.is_empty())
                .map(|(_, value)| value.to_string())
        }
    }

    fn decision(
        flag: Option<bool>,
        vars: &[(&str, &str)],
        config: ConfigKey,
        layers: &[(&str, bool)],
    ) -> Decision {
        decide(&Sources {
            flag,
            get: &world(vars),
            config,
            layers: layers.iter().map(|(l, on)| (l.to_string(), *on)).collect(),
        })
    }

    #[test]
    fn nothing_decided_is_off_by_default_and_the_question_is_due() {
        let d = decision(None, &[], ConfigKey::Absent, &[]);
        assert_eq!(
            d,
            Decision {
                on: false,
                by: None
            }
        );
        assert!(should_ask(&d, true));
        assert!(!should_ask(&d, false));
    }

    #[test]
    fn every_source_can_turn_it_off_and_beats_a_yes_below_it() {
        let yes = [("user", true)];
        for (flag, vars, config, by) in [
            (Some(false), vec![], ConfigKey::Absent, Source::Flag),
            (
                None,
                vec![("BDDKIT_TELEMETRY", "0")],
                ConfigKey::Absent,
                Source::Env,
            ),
            (
                None,
                vec![("BDDKIT_TELEMETRY", "false")],
                ConfigKey::Absent,
                Source::Env,
            ),
            (
                None,
                vec![("DO_NOT_TRACK", "1")],
                ConfigKey::Absent,
                Source::DoNotTrack,
            ),
            (None, vec![], ConfigKey::Set(false), Source::Config),
        ] {
            let d = decision(flag, &vars, config, &yes);
            assert_eq!(
                d,
                Decision {
                    on: false,
                    by: Some(by)
                }
            );
            assert!(!should_ask(&d, true), "a decided run is never asked");
        }
        let layer_off = decision(
            None,
            &[],
            ConfigKey::Absent,
            &[("shared", false), ("user", true)],
        );
        assert!(layer_off.on, "the later layer wins");
        let layer_off = decision(
            None,
            &[],
            ConfigKey::Absent,
            &[("shared", true), ("user", false)],
        );
        assert_eq!(layer_off.by, Some(Source::Layer("user".into())));
        assert!(!layer_off.on);
    }

    #[test]
    fn bddkit_telemetry_wins_over_do_not_track() {
        let vars = [("BDDKIT_TELEMETRY", "1"), ("DO_NOT_TRACK", "1")];
        let d = decision(None, &vars, ConfigKey::Absent, &[]);
        assert_eq!(
            d,
            Decision {
                on: true,
                by: Some(Source::Env)
            }
        );
    }

    #[test]
    fn the_order_is_flag_env_do_not_track_config_layers() {
        let vars = [("BDDKIT_TELEMETRY", "0"), ("DO_NOT_TRACK", "1")];
        assert!(decision(Some(true), &vars, ConfigKey::Set(false), &[("user", false)]).on);
        assert!(!decision(None, &[("DO_NOT_TRACK", "1")], ConfigKey::Set(true), &[]).on);
        assert!(decision(None, &[], ConfigKey::Set(true), &[("user", false)]).on);
    }

    #[test]
    fn a_broken_config_is_off_unless_the_flag_or_the_variable_says_on() {
        let broken = ConfigKey::Broken;
        let d = decision(None, &[], broken, &[("user", true)]);
        assert_eq!(
            d,
            Decision {
                on: false,
                by: Some(Source::BrokenConfig)
            }
        );
        assert!(decision(Some(true), &[], broken, &[]).on);
        assert!(decision(None, &[("BDDKIT_TELEMETRY", "true")], broken, &[]).on);
        assert!(!decision(None, &[("DO_NOT_TRACK", "1")], broken, &[("user", true)]).on);
    }

    #[test]
    fn a_misspelled_variable_never_turns_counting_on() {
        let d = decision(
            None,
            &[("BDDKIT_TELEMETRY", "yes please")],
            ConfigKey::Absent,
            &[],
        );
        assert_eq!(
            d,
            Decision {
                on: false,
                by: Some(Source::Env)
            }
        );
        // An empty one is unset.
        let d = decision(None, &[("BDDKIT_TELEMETRY", "")], ConfigKey::Absent, &[]);
        assert_eq!(d.by, None);
    }

    #[test]
    fn the_decision_reads_the_way_the_doctor_row_prints_it() {
        let d = decision(None, &[("DO_NOT_TRACK", "1")], ConfigKey::Absent, &[]);
        assert_eq!(d.describe(), "off (decided by DO_NOT_TRACK)");
        let d = decision(None, &[], ConfigKey::Absent, &[("user", true)]);
        assert_eq!(d.describe(), "on (decided by the user layer)");
    }

    #[test]
    fn ci_is_found_by_presence_and_an_empty_variable_is_unset() {
        assert_eq!(ci(&world(&[("GITLAB_CI", "true")])), Some(Ci::Gitlab));
        assert_eq!(
            ci(&world(&[("JENKINS_URL", "http://x")])),
            Some(Ci::Jenkins)
        );
        assert_eq!(ci(&world(&[("TF_BUILD", "False")])), Some(Ci::Azure));
        assert_eq!(ci(&world(&[("CI", "1")])), Some(Ci::Other));
        assert_eq!(ci(&world(&[("CI", "")])), None);
        assert_eq!(ci(&world(&[])), None);
        // The specific system wins over the generic variable it also sets.
        assert_eq!(
            ci(&world(&[("CI", "true"), ("CIRCLECI", "true")])),
            Some(Ci::Circleci)
        );
    }

    #[test]
    fn the_endpoint_follows_build_and_environment() {
        let none = world(&[]);
        let url = world(&[("BDDKIT_TELEMETRY_URL", "http://127.0.0.1:1")]);
        // A debug build sends only where it is told to.
        assert_eq!(endpoint_with(&none, None, true), None);
        assert_eq!(
            endpoint_with(&url, None, true).as_deref(),
            Some("http://127.0.0.1:1")
        );
        // A release build has the constant, a compile-time replacement, or none.
        assert_eq!(
            endpoint_with(&none, None, false).as_deref(),
            Some(DEFAULT_ENDPOINT)
        );
        assert_eq!(
            endpoint_with(&none, Some("https://c.example"), false).as_deref(),
            Some("https://c.example")
        );
        assert_eq!(endpoint_with(&none, Some(""), false), None);
        assert_eq!(
            endpoint_with(&url, Some(""), false).as_deref(),
            Some("http://127.0.0.1:1")
        );
    }

    #[test]
    fn only_a_repository_of_the_bddkit_owner_is_official() {
        assert!(is_official(Some("bddkit/bddkit-browser")));
        assert!(is_official(Some("BDDKit/bddkit-exec")));
        assert!(!is_official(Some("bddkit-evil/bddkit-browser")));
        assert!(!is_official(Some("someone/bddkit")));
        assert!(!is_official(Some("bddkit")));
        assert!(!is_official(Some("bddkit/")));
        assert!(!is_official(None));
    }

    #[test]
    fn the_builtin_groups_are_the_ones_the_table_declares() {
        let groups: std::collections::BTreeSet<_> = builtin_groups().collect();
        assert_eq!(
            groups.into_iter().collect::<Vec<_>>(),
            ["api", "db", "debug", "general", "srp", "vars"]
        );
    }

    #[test]
    fn a_line_answer_has_no_default() {
        assert_eq!(parse_line("y\n"), Some(true));
        assert_eq!(parse_line(" YES "), Some(true));
        assert_eq!(parse_line("No"), Some(false));
        assert_eq!(parse_line("\n"), None);
        assert_eq!(parse_line("maybe"), None);
    }

    #[test]
    fn the_line_fallback_asks_again_and_end_of_input_answers_nothing() {
        let term = Term::buffered_stderr();
        let mut input = std::io::Cursor::new("\nmaybe\ny\n");
        assert_eq!(ask_line(&mut input, &term, PROMPT).unwrap(), Some(true));
        let mut input = std::io::Cursor::new("\n\n");
        assert_eq!(ask_line(&mut input, &term, PROMPT).unwrap(), None);
    }

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("bddkit-telemetry-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    #[test]
    fn an_answer_is_stored_in_the_user_layer_and_read_back_from_the_chain() {
        let root = temp("answer");
        let layers = vec![
            Layer {
                name: "shared",
                dir: root.join("etc"),
            },
            Layer {
                name: "user",
                dir: root.join("home/.config/bddkit"),
            },
        ];
        let path = answer_path(&layers, None).unwrap();
        assert_eq!(path, root.join("home/.config/bddkit/telemetry.yaml"));
        store_answer(&path, true).unwrap();
        assert_eq!(layer_answers(&layers), vec![("user".to_string(), true)]);
        store_answer(&answer_path(&layers, Some("shared")).unwrap(), false).unwrap();
        // The user layer is later in the chain, so it still wins.
        assert_eq!(
            layer_answers(&layers),
            vec![("shared".to_string(), false), ("user".to_string(), true)]
        );
        assert_eq!(
            last_payload_path(&layers),
            Some(root.join("home/.config/bddkit/telemetry-last.json"))
        );
        assert!(answer_path(&layers, Some("project")).is_err());
    }

    #[test]
    fn storing_an_answer_keeps_the_other_keys_and_a_garbled_file_answers_no() {
        let root = temp("keep");
        let path = root.join("telemetry.yaml");
        std::fs::write(&path, "note: kept\nenabled: true\n").unwrap();
        store_answer(&path, false).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("note: kept") && text.contains("enabled: false"),
            "{text}"
        );
        std::fs::write(&path, "enabled: [oops\n").unwrap();
        let layers = vec![Layer {
            name: "user",
            dir: root.clone(),
        }];
        assert_eq!(layer_answers(&layers), vec![("user".to_string(), false)]);
    }

    #[test]
    fn the_config_key_is_read_without_loading_the_config() {
        let root = temp("key");
        let write = |text: &str| {
            let path = root.join("bddkit.yaml");
            std::fs::write(&path, text).unwrap();
            config_key(Some(&path))
        };
        // `${UNSET}` would fail a real load; here it is just text.
        assert_eq!(
            write("paths: [\"${UNSET_VARIABLE}\"]\ntelemetry: false\n"),
            ConfigKey::Set(false)
        );
        assert_eq!(write("telemetry: true\n"), ConfigKey::Set(true));
        assert_eq!(write("paths: []\n"), ConfigKey::Absent);
        assert_eq!(write("telemetry: sure\n"), ConfigKey::Broken);
        assert_eq!(write("telemetry: [\n"), ConfigKey::Broken);
        assert_eq!(
            config_key(Some(&root.join("missing.yaml"))),
            ConfigKey::Broken
        );
        assert_eq!(config_key(None), ConfigKey::Absent);
    }
}
