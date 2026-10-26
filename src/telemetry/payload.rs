//! The payload of `telemetry.md`, section 3: a closed struct of numbers,
//! booleans and enum values from bddkit's own vocabulary. No field can hold
//! text from a suite, so no masking is involved and none can fail. The only
//! strings are the version and platform constants the binary was built with,
//! the names of step groups bddkit declares, and the names and versions of
//! plugins an owner we trust published.
//!
//! A new field is a new schema version.

use super::Ci;
use serde::Serialize;
use std::collections::BTreeMap;

pub const SCHEMA: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Command {
    Run,
    Doctor,
    Report,
    Init,
    PluginInstall,
    PluginUpdate,
    PluginRemove,
    Upgrade,
}

/// Where a run that exited 2 stopped. A refusal before the config was read
/// sends nothing, so there is no `config` here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Plugins,
    Steps,
    Selection,
    Validation,
    Scheduling,
    Resources,
    Reports,
    Events,
}

#[derive(Debug, Serialize)]
pub struct Payload {
    pub schema: u8,
    pub bddkit: &'static str,
    pub os: &'static str,
    pub arch: &'static str,
    pub ci: Option<Ci>,
    pub command: Command,
    pub exit: i32,
    pub stage: Option<Stage>,
    pub duration_ms: u64,
    #[serde(flatten)]
    pub run: Option<RunFields>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub privacy: Option<PrivacyFlags>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<Resources>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doctor: Option<DoctorInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub init: Option<InitInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin: Option<Vec<PluginInfo>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report: Option<ReportInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upgrade: Option<UpgradeInfo>,
}

impl Payload {
    /// What every command has; the command's own object is filled by its caller.
    pub fn new(command: Command, exit: i32, duration: std::time::Duration) -> Self {
        Payload {
            schema: SCHEMA,
            bddkit: env!("CARGO_PKG_VERSION"),
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            ci: super::ci(&crate::privacy::process_env),
            command,
            exit,
            stage: None,
            duration_ms: millis(duration),
            run: None,
            privacy: None,
            resources: None,
            doctor: None,
            init: None,
            plugin: None,
            report: None,
            upgrade: None,
        }
    }
}

pub fn millis(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// What only `run` has.
#[derive(Debug, Serialize)]
pub struct RunFields {
    pub files: u64,
    pub scenarios: u64,
    pub examples: u64,
    pub failed: u64,
    pub concurrency: u64,
    pub fail_fast: bool,
    pub scenario_ms: Option<ScenarioMs>,
    pub features: Features,
    pub usage: BTreeMap<String, Usage>,
}

/// `None` for a run that finished no scenario: there is nothing to take a
/// percentile of.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct ScenarioMs {
    pub p50: u64,
    pub p90: u64,
    pub max: u64,
    /// Scenarios under 100 ms, under 1 s, under 10 s, under 60 s, 60 s and over.
    pub buckets: [u64; 5],
}

impl ScenarioMs {
    /// Computed here from the durations of that run; the durations themselves
    /// are not sent. The nearest-rank percentile: the smallest value that at
    /// least that share of the scenarios do not exceed.
    pub fn of(durations: &[std::time::Duration]) -> Option<Self> {
        let mut ms: Vec<u64> = durations.iter().map(|d| millis(*d)).collect();
        ms.sort_unstable();
        let max = *ms.last()?;
        let rank = |percent: usize| ms[(ms.len() * percent).div_ceil(100).max(1) - 1];
        let mut buckets = [0; 5];
        for value in &ms {
            let bucket = [100, 1_000, 10_000, 60_000]
                .iter()
                .position(|limit| value < limit)
                .unwrap_or(4);
            buckets[bucket] += 1;
        }
        Some(ScenarioMs {
            p50: rank(50),
            p90: rank(90),
            max,
            buckets,
        })
    }
}

/// What was ON for this run, whichever source turned it on; never a value.
#[derive(Debug, Default, Clone, Serialize)]
pub struct Features {
    pub junit: bool,
    pub cucumber_json: bool,
    pub events: bool,
    /// The active evidence sources: `marked`, `failures`, `all`.
    pub evidence: Vec<&'static str>,
    pub artifacts_dir: bool,
    pub tags: bool,
    pub paths: bool,
    pub env: bool,
}

#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub struct Usage {
    pub steps: u64,
    pub ms: u64,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct PrivacyFlags {
    /// Masking was off for this run, by whichever source.
    pub reveal: bool,
    /// Whether the lists differ from the defaults, never the words.
    pub custom_words: bool,
    pub custom_fields: bool,
}

impl PrivacyFlags {
    pub fn of(privacy: &crate::privacy::Privacy) -> Self {
        let defaults = crate::privacy::Privacy::default();
        PrivacyFlags {
            reveal: privacy.reveal,
            custom_words: privacy.words != defaults.words,
            custom_fields: privacy.fields != defaults.fields,
        }
    }
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Resources {
    pub api: u64,
    pub srp: u64,
    pub db: Engines,
    /// Instances per official plugin name.
    pub plugins: BTreeMap<String, u64>,
    pub plugins_other: u64,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Engines {
    pub postgres: u64,
    pub mysql: u64,
    pub mariadb: u64,
}

impl Engines {
    /// By the platform's own name; a name this binary does not know is not
    /// counted, rather than invented.
    pub fn add(&mut self, platform: &str) {
        match platform {
            "postgres" => self.postgres += 1,
            "mysql" => self.mysql += 1,
            "mariadb" => self.mariadb += 1,
            _ => {}
        }
    }
}

#[derive(Debug, Serialize)]
pub struct DoctorInfo {
    pub live: bool,
    pub failed: u64,
    /// From doctor's own fixed set of stage names.
    pub stages_failed: Vec<&'static str>,
}

#[derive(Debug, Default, Serialize)]
pub struct InitInfo {
    pub interactive: bool,
    pub plugins: BTreeMap<String, u64>,
    pub plugins_other: u64,
    pub plugins_failed: u64,
    pub resources: Resources,
    pub settings: BTreeMap<&'static str, bool>,
}

/// One plugin a plugin command acted on. Name and version for an official
/// plugin; `other: true` for any other, whose name and repository are never sent.
#[derive(Debug, Serialize)]
pub struct PluginInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub other: bool,
    /// Absent for `remove`, which looks nothing up.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<PluginFrom>,
    pub layer: Option<&'static str>,
    pub result: PluginResult,
}

/// What a plugin command knew about one plugin when it ended, whatever it had
/// got to: a command that failed on the index knows no repository at all.
#[derive(Debug, Default, Clone)]
pub struct Acted {
    /// `owner/repo`, once known.
    pub repo: Option<String>,
    /// The plugin's own name, once known.
    pub name: Option<String>,
    pub version: Option<String>,
    pub from: Option<PluginFrom>,
    /// The layer label (`user`, `project.local`, …).
    pub layer: Option<String>,
}

/// The fixed names a layer label can have; anything else is not sent.
pub fn layer_name(label: &str) -> Option<&'static str> {
    [
        "shared",
        "shared.local",
        "user",
        "user.local",
        "project",
        "project.local",
        "override",
        "override.local",
    ]
    .into_iter()
    .find(|name| *name == label)
}

impl PluginInfo {
    /// The name and version are sent only when the repository is one of the
    /// `bddkit` owner's. For one that failed before a name was known, the
    /// repository's own name without its `bddkit-` prefix stands in.
    pub fn of(acted: &Acted, result: PluginResult) -> Self {
        let official = super::is_official(acted.repo.as_deref());
        // The repository's own name stands in only when the index named it:
        // `bddkit/anything` typed on the command line is text, not a plugin.
        let name = official
            .then(|| {
                acted.name.clone().or_else(|| {
                    (acted.from == Some(PluginFrom::Registry)).then_some(())?;
                    let repo = acted.repo.as_deref()?;
                    let base = repo.rsplit('/').next()?;
                    Some(base.strip_prefix("bddkit-").unwrap_or(base).to_string())
                })
            })
            .flatten();
        PluginInfo {
            name,
            version: official.then(|| acted.version.clone()).flatten(),
            other: !official,
            from: acted.from,
            layer: acted.layer.as_deref().and_then(layer_name),
            result,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginFrom {
    Registry,
    Repository,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginResult {
    Ok,
    NotFound,
    NoBuild,
    Checksum,
    Network,
    Refused,
    Other,
}

#[derive(Debug, Default, Serialize)]
pub struct ReportInfo {
    pub formats: Vec<&'static str>,
    pub detail: Option<&'static str>,
    pub embed: Option<&'static str>,
    pub events: u64,
    pub evidence: bool,
    pub complete: bool,
    pub artifacts: bool,
    pub result: &'static str,
}

#[derive(Debug, Default, Serialize)]
pub struct UpgradeInfo {
    pub from: &'static str,
    pub to: Option<String>,
    pub abi_changed: bool,
    pub plugins_switched: u64,
    pub plugins_refused: u64,
    pub plugins_left: u64,
    pub result: &'static str,
    pub check: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn ms(values: &[u64]) -> Vec<Duration> {
        values.iter().map(|v| Duration::from_millis(*v)).collect()
    }

    #[test]
    fn percentiles_are_nearest_rank_and_buckets_cover_every_scenario() {
        let spread = ScenarioMs::of(&ms(&[10, 20, 30, 40, 50, 60, 70, 80, 90, 5_000])).unwrap();
        assert_eq!((spread.p50, spread.p90, spread.max), (50, 90, 5_000));
        assert_eq!(spread.buckets, [9, 0, 1, 0, 0]);
        let edges = ScenarioMs::of(&ms(&[99, 100, 999, 1_000, 59_999, 60_000])).unwrap();
        assert_eq!(edges.buckets, [1, 2, 1, 1, 1]);
        assert_eq!(edges.buckets.iter().sum::<u64>(), 6);
        let one = ScenarioMs::of(&ms(&[7])).unwrap();
        assert_eq!((one.p50, one.p90, one.max), (7, 7, 7));
        assert_eq!(ScenarioMs::of(&[]), None);
    }

    #[test]
    fn a_run_payload_flattens_its_fields_and_other_commands_have_none_of_them() {
        let mut run = Payload::new(Command::Run, 1, Duration::from_millis(83));
        run.ci = Some(Ci::Gitlab);
        run.run = Some(RunFields {
            files: 1,
            scenarios: 2,
            examples: 1,
            failed: 1,
            concurrency: 8,
            fail_fast: false,
            scenario_ms: None,
            features: Features {
                junit: true,
                cucumber_json: false,
                events: false,
                evidence: vec!["failures"],
                artifacts_dir: false,
                tags: false,
                paths: false,
                env: false,
            },
            usage: BTreeMap::new(),
        });
        let json = serde_json::to_value(&run).unwrap();
        assert_eq!(json["command"], "run");
        assert_eq!(json["ci"], "gitlab");
        assert_eq!(json["scenarios"], 2);
        assert!(json["scenario_ms"].is_null());

        let mut install = Payload::new(Command::PluginInstall, 0, Duration::ZERO);
        install.ci = None;
        let json = serde_json::to_value(&install).unwrap();
        assert_eq!(json["command"], "plugin_install");
        assert!(json["ci"].is_null() && json["stage"].is_null());
        for absent in [
            "files",
            "scenarios",
            "usage",
            "features",
            "scenario_ms",
            "privacy",
        ] {
            assert!(json.get(absent).is_none(), "{absent} is run-only");
        }
    }

    #[test]
    fn only_a_bddkit_repository_gives_its_name_and_version() {
        let mut acted = Acted {
            repo: Some("bddkit/bddkit-exec".into()),
            name: Some("exec".into()),
            version: Some("0.2.0".into()),
            from: Some(PluginFrom::Registry),
            layer: Some("user".into()),
        };
        let official = serde_json::to_value(PluginInfo::of(&acted, PluginResult::Ok)).unwrap();
        assert_eq!(
            official,
            serde_json::json!({"name": "exec", "version": "0.2.0", "from": "registry", "layer": "user", "result": "ok"})
        );
        // Failed before the manifest was read: the index's repository name.
        acted.name = None;
        let early = serde_json::to_value(PluginInfo::of(&acted, PluginResult::Network)).unwrap();
        assert_eq!(early["name"], "exec");
        // A repository typed on the command line is not a name.
        acted.from = Some(PluginFrom::Repository);
        assert!(PluginInfo::of(&acted, PluginResult::Network).name.is_none());
        acted.from = Some(PluginFrom::Registry);

        acted.repo = Some("someone/their-secret-plugin".into());
        acted.name = Some("their-secret-plugin".into());
        let other = serde_json::to_string(&PluginInfo::of(&acted, PluginResult::Ok)).unwrap();
        assert!(
            !other.contains("secret") && other.contains("\"other\":true"),
            "{other}"
        );
        // A label that is not one of ours is not sent either.
        acted.layer = Some("wherever-the-user-put-it".into());
        assert!(PluginInfo::of(&acted, PluginResult::Ok).layer.is_none());
    }

    #[test]
    fn an_unofficial_plugin_has_no_name_in_its_entry() {
        let info = PluginInfo {
            name: None,
            version: None,
            other: true,
            from: Some(PluginFrom::Repository),
            layer: Some("user"),
            result: PluginResult::Checksum,
        };
        assert_eq!(
            serde_json::to_value(&info).unwrap(),
            serde_json::json!({"other": true, "from": "repository", "layer": "user", "result": "checksum"})
        );
    }
}
