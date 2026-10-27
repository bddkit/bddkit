//! `bddkit telemetry status | show | on | off`. These commands send nothing,
//! whatever is decided, and none of them asks the question.

use super::payload::Payload;
use super::{ConfigKey, Decision, Sources, answer_path, decide, last_payload_path};
use crate::dirs::Layer;
use anyhow::{Context as _, Result};
use std::path::{Path, PathBuf};

/// The end of a command that never asks: the payload is recorded when the
/// decision is on, and nothing else happens. A setting that could not be
/// worked out (a layer chain that does not resolve) records nothing; none of
/// it may change the command's exit code.
pub fn record_if_on(setting: Option<&Setting>, payload: &Payload) {
    if let Some(setting) = setting
        && setting.decision().on
    {
        super::record(payload, &setting.layers);
    }
}

/// Everything the decision reads that is not the command line: no flag exists
/// for these commands, the config is located and not loaded, and the layers
/// are the chain from the config's directory.
pub struct Setting {
    pub layers: Vec<Layer>,
    pub key: ConfigKey,
    /// The environment, so a test is not at the mercy of this machine's own
    /// `DO_NOT_TRACK`.
    pub get: fn(&str) -> Option<String>,
}

impl Setting {
    /// Located, never loaded: the key is read as text, so a `${VAR}` that is
    /// unset elsewhere in the file cannot make a command that needs no config
    /// refuse. No config found is `Absent`; one that is there and cannot be
    /// read is `Broken`.
    pub fn discover(config: Option<&Path>, bddkit_dir: Option<PathBuf>) -> Result<Setting> {
        let resolved = crate::config::resolve_config_path(config)?;
        let config_dir = resolved.as_ref().map_or_else(
            || PathBuf::from("."),
            |r| crate::config_dir(&r.path).to_path_buf(),
        );
        let env = crate::dirs::Env::from_process(bddkit_dir);
        Ok(Setting {
            layers: crate::dirs::layers(crate::dirs::Os::current(), &env, &config_dir)?,
            key: super::config_key(resolved.as_ref().map(|r| r.path.as_path())),
            get: crate::privacy::process_env,
        })
    }

    pub fn decision(&self) -> Decision {
        decide(&Sources {
            flag: None,
            get: &self.get,
            config: self.key,
            layers: super::layer_answers(&self.layers),
        })
    }
}

/// What is decided, by what, and where a payload would go.
pub fn status(setting: &Setting) -> String {
    let decision = setting.decision();
    let endpoint = match super::endpoint(&crate::privacy::process_env) {
        Some(endpoint) => format!("{endpoint}/v1/run"),
        None => "none (this build sends nowhere)".to_string(),
    };
    format!(
        "telemetry: {}\nendpoint: {endpoint}\nnothing is sent yet: while telemetry is on, each payload is only recorded on this machine (bddkit telemetry show)\n",
        decision.describe()
    )
}

/// The last recorded payload, pretty-printed; or a line saying there is none.
pub fn show(setting: &Setting) -> Result<String> {
    let Some(path) = last_payload_path(&setting.layers) else {
        return Ok("nothing recorded yet: there is no user layer to keep it in\n".to_string());
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let value: serde_json::Value = serde_json::from_str(&text)
                .with_context(|| format!("{} is not a payload", path.display()))?;
            Ok(format!("{}\n", serde_json::to_string_pretty(&value)?))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(
            "nothing recorded yet: a payload is recorded when telemetry is on and a command ends\n"
                .to_string(),
        ),
        Err(error) => Err(error).with_context(|| format!("cannot read {}", path.display())),
    }
}

/// Writes the answer to the user layer, or to the one `--layer` names, and says
/// so — and, when a source above that file still decides otherwise, says that
/// too: an `on` that changes nothing must not look like it worked.
pub fn set(setting: &Setting, enabled: bool, layer: Option<&str>) -> Result<String> {
    let path = answer_path(&setting.layers, layer)?;
    super::store_answer(&path, enabled)?;
    Ok(report_set(setting, enabled, &path))
}

fn report_set(setting: &Setting, enabled: bool, path: &Path) -> String {
    let word = if enabled { "on" } else { "off" };
    let mut out = format!("telemetry {word}: written to {}\n", path.display());
    let decision = setting.decision();
    if decision.on != enabled || decision.by.is_none() {
        out.push_str(&format!(
            "note: it is {} for now: {}\n",
            if decision.on { "on" } else { "off" },
            decision.describe()
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bddkit-telemetry-cmd-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    fn setting(dir: &Path, key: ConfigKey) -> Setting {
        Setting {
            layers: vec![Layer {
                name: "user",
                dir: dir.to_path_buf(),
            }],
            key,
            get: |_| None,
        }
    }

    #[test]
    fn show_says_so_when_nothing_was_recorded_and_prints_what_was() {
        let dir = temp("show");
        let setting = setting(&dir, ConfigKey::Absent);
        assert!(show(&setting).unwrap().starts_with("nothing recorded yet"));
        std::fs::write(dir.join("telemetry-last.json"), "{\"schema\":1}\n").unwrap();
        assert_eq!(show(&setting).unwrap(), "{\n  \"schema\": 1\n}\n");
        std::fs::write(dir.join("telemetry-last.json"), "not json").unwrap();
        assert!(show(&setting).is_err());
    }

    #[test]
    fn a_set_that_a_higher_source_overrides_says_so() {
        let dir = temp("set");
        // The suite's own `false` is above the layer files.
        let overridden = setting(&dir, ConfigKey::Set(false));
        let said = set(&overridden, true, None).unwrap();
        assert!(said.contains("telemetry on: written to"), "{said}");
        assert!(said.contains("note: it is off for now"), "{said}");

        let free = setting(&dir, ConfigKey::Absent);
        let said = set(&free, true, None).unwrap();
        assert!(!said.contains("note:"), "{said}");
        assert!(free.decision().on);
        let said = set(&free, false, None).unwrap();
        assert!(!said.contains("note:") && !free.decision().on, "{said}");
    }
}
