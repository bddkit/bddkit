use regex::Regex;
use serde::Deserialize;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

#[derive(Debug, Default)]
pub struct MacroCatalog {
    pub definitions: Vec<MacroDef>,
}

#[derive(Debug)]
pub struct MacroDef {
    pub step: String,
    pub params: Vec<String>,
    pub exports: Vec<String>,
    pub body: Vec<MacroStep>,
    pub source: PathBuf,
    pub line: usize,
    pub regex: Regex,
}

#[derive(Debug, Clone)]
pub struct MacroStep {
    pub text: String,
    pub docstring: Option<String>,
}

#[derive(Deserialize)]
struct RawMacro {
    step: String,
    #[serde(default)]
    exports: Vec<String>,
    #[serde(rename = "do")]
    body: Vec<RawStep>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawStep {
    Scalar(String),
    Docstring(BTreeMap<String, String>),
}

static PARAM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\{([A-Za-z_][A-Za-z0-9_]*)\}").expect("constant macro parameter regex")
});
static MACRO_START: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^([ \t]*)-\s*(?:\{\s*)?(?:step|"step"|'step')\s*:"#)
        .expect("constant macro-start regex")
});

impl MacroCatalog {
    pub fn load(paths: &[PathBuf]) -> Result<Self, String> {
        let mut files = Vec::new();
        for path in paths {
            collect(path, &mut files)?;
        }
        files.sort();

        let mut definitions = Vec::new();
        for path in files {
            let source = std::fs::read_to_string(&path).map_err(|error| {
                format!("failed to read macros {}: {error}", path.display())
            })?;
            let raw: Vec<RawMacro> = serde_yaml_ng::from_str(&source).map_err(|error| {
                format!("failed to parse macros {}: {error}", path.display())
            })?;
            let lines = definition_lines(&source, raw.len(), &path)?;
            for (item, line) in raw.into_iter().zip(lines) {
                definitions.push(compile(item, &path, line)?);
            }
        }
        Ok(Self { definitions })
    }
}

fn definition_lines(source: &str, count: usize, path: &Path) -> Result<Vec<usize>, String> {
    let candidates: Vec<_> = source
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let capture = MACRO_START.captures(line)?;
            Some((capture[1].len(), index + 1))
        })
        .collect();
    let indent = candidates.iter().map(|(indent, _)| *indent).min();
    let lines: Vec<_> = candidates
        .into_iter()
        .filter_map(|(candidate, line)| (Some(candidate) == indent).then_some(line))
        .collect();
    if lines.len() != count {
        return Err(format!(
            "unsupported macro format {}: every definition must start with '- step:' on one line",
            path.display()
        ));
    }
    Ok(lines)
}

fn collect(path: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    if path.is_file() {
        files.push(path.to_path_buf());
        return Ok(());
    }
    if !path.is_dir() {
        return Err(format!("macro path {} does not exist", path.display()));
    }
    let entries = std::fs::read_dir(path).map_err(|error| {
        format!(
            "failed to read macro directory {}: {error}",
            path.display()
        )
    })?;
    for entry in entries {
        let child = entry
            .map_err(|error| {
                format!(
                    "failed to read macro directory {}: {error}",
                    path.display()
                )
            })?
            .path();
        if child.is_dir() {
            collect(&child, files)?;
        } else if child
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| matches!(extension, "yaml" | "yml"))
        {
            files.push(child);
        }
    }
    Ok(())
}

fn compile(raw: RawMacro, source: &Path, line: usize) -> Result<MacroDef, String> {
    let mut params = Vec::new();
    let mut seen = HashSet::new();
    let mut pattern = String::from("^");
    let mut last = 0;
    for capture in PARAM.captures_iter(&raw.step) {
        let whole = capture.get(0).expect("group 0 always exists");
        let name = capture.get(1).expect("group 1 is required").as_str();
        if !seen.insert(name.to_string()) {
            return Err(format!(
                "macro parameter {name:?} is declared more than once in {}:{line}",
                source.display(),
            ));
        }
        pattern.push_str(&regex::escape(&raw.step[last..whole.start()]));
        pattern.push_str("(.*?)");
        params.push(name.to_string());
        last = whole.end();
    }
    pattern.push_str(&regex::escape(&raw.step[last..]));
    pattern.push('$');

    let body = raw
        .body
        .into_iter()
        .map(|step| match step {
            RawStep::Scalar(text) => Ok(MacroStep {
                text,
                docstring: None,
            }),
            RawStep::Docstring(values) => {
                let mut values = values.into_iter();
                let Some((mut text, docstring)) = values.next() else {
                    return Err(format!("empty macro step in {}:{line}", source.display()));
                };
                if values.next().is_some() {
                    return Err(format!(
                        "a step with a docstring in {}:{line} must contain exactly one name",
                        source.display(),
                    ));
                }
                if !text.ends_with(':') {
                    text.push(':');
                }
                Ok(MacroStep {
                    text,
                    docstring: Some(docstring),
                })
            }
        })
        .collect::<Result<Vec<_>, String>>()?;

    Ok(MacroDef {
        regex: Regex::new(&pattern).map_err(|error| {
            format!(
                "invalid macro template {:?} in {}:{line}: {error}",
                raw.step,
                source.display()
            )
        })?,
        step: raw.step,
        params,
        exports: raw.exports,
        body,
        source: source.to_path_buf(),
        line,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::steps::{Registry, StepTarget};
    use std::path::PathBuf;

    fn fixture(name: &str, source: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("bddkit-macros-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("macros.yaml");
        std::fs::write(&path, source).unwrap();
        path
    }

    #[test]
    fn load_reads_scalar_and_docstring_steps() {
        let path = fixture(
            "body",
            r#"
- step: 'I login as "{email}"'
  exports: [token]
  do:
    - the request body is: |
        {"email": "<<email>>"}
    - I request "/login" using HTTP POST
"#,
        );

        let catalog = MacroCatalog::load(&[path]).unwrap();
        let definition = &catalog.definitions[0];

        assert_eq!(definition.step, r#"I login as "{email}""#);
        assert_eq!(definition.exports, ["token"]);
        assert_eq!(definition.body[0].text, "the request body is:");
        assert_eq!(
            definition.body[0].docstring.as_deref(),
            Some("{\"email\": \"<<email>>\"}\n")
        );
        assert_eq!(
            definition.body[1].text,
            r#"I request "/login" using HTTP POST"#
        );
    }

    #[test]
    fn load_compiles_parameters_in_declaration_order() {
        let path = fixture(
            "parameters",
            r#"
- step: 'I login as "{email}" with password "{password}"'
  do: [the response code is 200]
"#,
        );

        let catalog = MacroCatalog::load(&[path]).unwrap();
        let definition = &catalog.definitions[0];
        let captures = definition
            .regex
            .captures(r#"I login as "a@b.net" with password "secret""#)
            .unwrap();

        assert_eq!(definition.params, ["email", "password"]);
        assert_eq!(captures.get(1).unwrap().as_str(), "a@b.net");
        assert_eq!(captures.get(2).unwrap().as_str(), "secret");
    }

    #[test]
    fn load_discovers_yaml_files_recursively() {
        let root =
            std::env::temp_dir().join(format!("bddkit-macros-{}-recursive", std::process::id()));
        let nested = root.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(
            root.join("one.yaml"),
            "- step: I do one\n  do: [the response code is 200]\n",
        )
        .unwrap();
        std::fs::write(
            nested.join("two.yml"),
            "- step: I do two\n  do: [the response code is 201]\n",
        )
        .unwrap();

        let catalog = MacroCatalog::load(&[root]).unwrap();

        assert_eq!(catalog.definitions.len(), 2);
    }

    #[test]
    fn load_rejects_duplicate_parameter_names() {
        let path = fixture(
            "duplicate-param",
            "- step: 'I compare {value} with {value}'\n  do: [the response code is 200]\n",
        );

        let error = MacroCatalog::load(&[path]).unwrap_err();

        assert!(error.contains("value"), "{error}");
    }

    #[test]
    fn load_error_names_malformed_file() {
        let path = fixture("malformed", "- step: [\n");

        let error = MacroCatalog::load(std::slice::from_ref(&path)).unwrap_err();

        assert!(error.contains(&path.display().to_string()), "{error}");
    }

    #[test]
    fn registry_matches_macro_and_captures_arguments() {
        let path = fixture(
            "registry-match",
            "- step: 'I login as \"{email}\"'\n  do: [the response code is 200]\n",
        );
        let registry = Registry::with_macros(MacroCatalog::load(&[path]).unwrap()).unwrap();

        let (target, captures) = registry.find(r#"I login as "a@b.net""#).unwrap().unwrap();

        assert_eq!(target, StepTarget::Macro(0));
        assert_eq!(captures, ["a@b.net"]);
    }

    #[test]
    fn registry_rejects_duplicate_macro_steps() {
        let path = fixture(
            "duplicate-step",
            "- step: I do business\n  do: [the response code is 200]\n- step: I do business\n  do: [the response code is 201]\n",
        );

        let error = Registry::with_macros(MacroCatalog::load(std::slice::from_ref(&path)).unwrap())
            .unwrap_err();

        assert!(error.contains("I do business"), "{error}");
        assert!(error.contains(&format!("{}:1", path.display())), "{error}");
        assert!(error.contains(&format!("{}:3", path.display())), "{error}");
    }

    #[test]
    fn registry_reports_lines_for_noncanonical_yaml() {
        let path = fixture(
            "duplicate-step-lines",
            "  - step : I do business\n    do: [the response code is 200]\n  - { 'step': I do business, do: [the response code is 201] }\n",
        );

        let error = Registry::with_macros(MacroCatalog::load(std::slice::from_ref(&path)).unwrap())
            .unwrap_err();

        assert!(error.contains(&format!("{}:1", path.display())), "{error}");
        assert!(error.contains(&format!("{}:3", path.display())), "{error}");
    }

    #[test]
    fn load_rejects_layout_without_traceable_definition_lines() {
        let path = fixture(
            "untraceable-lines",
            "-\n  step: I do business\n  do: [the response code is 200]\n",
        );

        let error = MacroCatalog::load(std::slice::from_ref(&path)).unwrap_err();

        assert!(
            error.contains("format") && error.contains(&path.display().to_string()),
            "{error}"
        );
    }

    #[test]
    fn registry_rejects_macro_conflicting_with_builtin() {
        let path = fixture(
            "builtin-conflict",
            "- step: 'the response code is {code}'\n  do: [Show all variables]\n",
        );

        let error = Registry::with_macros(MacroCatalog::load(&[path]).unwrap()).unwrap_err();

        assert!(error.contains("the response code is"), "{error}");
    }

    #[test]
    fn registry_rejects_partial_overlap_with_builtin() {
        let path = fixture(
            "partial-builtin-conflict",
            "- step: 'the response code is 2{tail}'\n  do: [Show all variables]\n",
        );

        let error = Registry::with_macros(MacroCatalog::load(&[path]).unwrap()).unwrap_err();

        assert!(error.contains("the response code is"), "{error}");
    }

    #[test]
    fn registry_rejects_unicode_digit_overlap_with_builtin() {
        let path = fixture(
            "unicode-digit-conflict",
            "- step: 'the response code is ٢{tail}'\n  do: [Show all variables]\n",
        );

        let error = Registry::with_macros(MacroCatalog::load(&[path]).unwrap()).unwrap_err();

        assert!(error.contains("the response code is"), "{error}");
    }

    #[test]
    fn registry_rejects_crossing_macro_patterns() {
        let path = fixture(
            "crossing-macros",
            "- step: 'I {x} foo'\n  do: [Show all variables]\n- step: 'I bar {y}'\n  do: [Show all variables]\n",
        );

        let error = Registry::with_macros(MacroCatalog::load(&[path]).unwrap()).unwrap_err();

        assert!(
            error.contains("I {x} foo") && error.contains("I bar {y}"),
            "{error}"
        );
    }

    #[test]
    fn registry_rejects_unknown_macro_body_step() {
        let path = fixture(
            "unknown-body",
            "- step: I do business\n  do: [I invoke something absent]\n",
        );

        let error = Registry::with_macros(MacroCatalog::load(&[path]).unwrap()).unwrap_err();

        assert!(error.contains("I invoke something absent"), "{error}");
    }

    #[test]
    fn registry_rejects_nested_macro_call_with_docstring() {
        let path = fixture(
            "nested-docstring",
            r#"
- step: 'I do inner:'
  do: [Show all variables]
- step: I do outer
  do:
    - I do inner: |
        unsupported
"#,
        );

        let error = Registry::with_macros(MacroCatalog::load(&[path]).unwrap()).unwrap_err();

        assert!(
            error.contains("docstring") && error.contains("I do outer"),
            "{error}"
        );
    }

    #[test]
    fn registry_accepts_nested_macros() {
        let path = fixture(
            "nested",
            "- step: I do inner\n  do: [the response code is 200]\n- step: I do outer\n  do: [I do inner]\n",
        );

        let registry = Registry::with_macros(MacroCatalog::load(&[path]).unwrap());

        assert!(registry.is_ok(), "{:?}", registry.unwrap_err());
    }

    #[test]
    fn registry_rejects_indirect_macro_cycle_with_path() {
        let path = fixture(
            "cycle",
            "- step: I do first\n  do: [I do second]\n- step: I do second\n  do: [I do first]\n",
        );

        let error = Registry::with_macros(MacroCatalog::load(&[path]).unwrap()).unwrap_err();

        assert!(
            error.contains("I do first") && error.contains("I do second"),
            "{error}"
        );
    }

    #[test]
    fn registry_rejects_nesting_deeper_than_sixteen() {
        let mut source = String::new();
        for index in 0..17 {
            let next = if index == 16 {
                "the response code is 200".to_string()
            } else {
                format!("I do level {}", index + 1)
            };
            source.push_str(&format!("- step: I do level {index}\n  do: [{next}]\n"));
        }
        let path = fixture("depth", &source);

        let error = Registry::with_macros(MacroCatalog::load(&[path]).unwrap()).unwrap_err();

        assert!(error.contains("16"), "{error}");
    }

    #[test]
    fn registry_rejects_long_path_through_previsited_suffix() {
        let mut source = String::new();
        for index in 0..8 {
            let next = if index == 7 {
                "Show all variables".to_string()
            } else {
                format!("I do suffix {}", index + 1)
            };
            source.push_str(&format!("- step: I do suffix {index}\n  do: [{next}]\n"));
        }
        for index in 0..9 {
            let next = if index == 8 {
                "I do suffix 0".to_string()
            } else {
                format!("I do prefix {}", index + 1)
            };
            source.push_str(&format!("- step: I do prefix {index}\n  do: [{next}]\n"));
        }
        let path = fixture("shared-suffix-depth", &source);

        let error = Registry::with_macros(MacroCatalog::load(&[path]).unwrap()).unwrap_err();

        assert!(error.contains("16"), "{error}");
    }
}
