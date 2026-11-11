pub mod debug;
pub mod expression;
pub mod help;
pub mod plugin;
mod suggest;
pub mod vars;

use crate::failure::StepError;
use crate::macros::{MacroCatalog, MacroDef};
use crate::options::{OptionsLayer, PollingOptionsLayer};
use crate::polling::{AttemptError, AttemptResult};
use crate::world::World;
use regex::Regex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StepId {
    UsePluginInstance,
    ExpectEventually,
    ExpectWithin,
    ExpectWithinEvery,
    // variables
    SetVariable,
    SetVariableGlobal,
    VariableEquals,
    VariableNotEquals,
    VariableContains,
    VariableNotContains,
    VariableMatches,
    VariableNotMatches,
    VariableEmpty,
    VariableContainsJson,
    VariableEqualsJson,
    VariableNotContainsJson,
    ExtractJsonFromVariable,
    ExtractJsonFromVariableGlobal,
    ExtractRegexFromVariable,
    ExtractRegexFromVariableGlobal,
    EncryptWithAes,
    // debug
    DebugOn,
    DebugOff,
    EvidenceOn,
    EvidenceOff,
    Sleep,
    ShowAllVariables,
    ShowVariable,
    PrintTable,
    // include
    Include,
    IncludeScenario,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionsSource {
    Global,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepKind {
    Action,
    Assertion(OptionsSource),
}

pub struct StepDef {
    pub id: StepId,
    /// The resource this step belongs to, and the heading `bddkit steps list`
    /// prints it under: `vars`, `debug`, or `general` for
    /// the few steps that belong to no resource at all.
    pub group: &'static str,
    /// A Cucumber Expression with bddkit's parameter extension: `{name}` is a
    /// `text` parameter (`[^"]*`; a bare one is matched inside quotes the host
    /// adds), `{name:type}` picks a type from `expression::TYPES`, which also
    /// decides whether a bare parameter takes quotes. The name is what `bddkit steps
    /// list` prints as `<name>`; captures still reach `dispatch` by position.
    pub expression: &'static str,
    /// One line, in English, stating the step's effect. English is the source
    /// language and it lives here, beside the pattern it describes;
    /// `locales/steps.<code>.yaml` carries translations of it.
    pub description: &'static str,
    pub kind: StepKind,
}

const fn action(
    id: StepId,
    group: &'static str,
    expression: &'static str,
    description: &'static str,
) -> StepDef {
    StepDef {
        id,
        group,
        expression,
        description,
        kind: StepKind::Action,
    }
}

const fn assertion(
    id: StepId,
    group: &'static str,
    expression: &'static str,
    description: &'static str,
    source: OptionsSource,
) -> StepDef {
    StepDef {
        id,
        group,
        expression,
        description,
        kind: StepKind::Assertion(source),
    }
}

/// Every expression is compiled by `expression::compile` at startup; an
/// optional part may not hold a parameter, which is why each `I include` step
/// is declared twice, without and with `with prefix "{prefix}"`.
pub const BUILTIN_STEPS: &[StepDef] = &[
    action(
        StepId::ExpectEventually,
        "general",
        "I expect the next assertion to pass eventually",
        "lets the next assertion retry with the configured polling options",
    ),
    action(
        StepId::ExpectWithinEvery,
        "general",
        r#"I expect the next assertion to pass within {seconds:uint} seconds, checking every {milliseconds:uint} milliseconds"#,
        "lets the next assertion retry for this long, at this interval",
    ),
    action(
        StepId::ExpectWithin,
        "general",
        r#"I expect the next assertion to pass within {seconds:uint} seconds"#,
        "lets the next assertion retry for this long, at the configured interval",
    ),
    action(
        StepId::SetVariableGlobal,
        "vars",
        r#"set variable "{name}" to "{value}" global"#,
        "sets a variable every scenario of this feature file can read",
    ),
    action(
        StepId::SetVariable,
        "vars",
        r#"set variable "{name}" to "{value}""#,
        "sets a variable for the whole feature file",
    ),
    assertion(
        StepId::VariableNotEquals,
        "vars",
        r#"variable "{name}" should not be equal to "{value}""#,
        "asserts a variable differs from this value",
        OptionsSource::Global,
    ),
    assertion(
        StepId::VariableEquals,
        "vars",
        r#"variable "{name}" should be equal to "{value}""#,
        "asserts a variable holds exactly this value",
        OptionsSource::Global,
    ),
    assertion(
        StepId::VariableContains,
        "vars",
        r#"variable "{name}" should contain "{text}""#,
        "asserts a variable's text contains this substring",
        OptionsSource::Global,
    ),
    assertion(
        StepId::VariableNotContains,
        "vars",
        r#"variable "{name}" should not contain "{text}""#,
        "asserts a variable's text does not contain this substring",
        OptionsSource::Global,
    ),
    assertion(
        StepId::VariableMatches,
        "vars",
        r#"variable "{name}" should match "{pattern}""#,
        "asserts a variable's text matches this regular expression, unanchored",
        OptionsSource::Global,
    ),
    assertion(
        StepId::VariableNotMatches,
        "vars",
        r#"variable "{name}" should not match "{pattern}""#,
        "asserts a variable's text does not match this regular expression",
        OptionsSource::Global,
    ),
    assertion(
        StepId::VariableEmpty,
        "vars",
        r#"variable "{name}" should be empty"#,
        "asserts a variable's text is empty or whitespace only",
        OptionsSource::Global,
    ),
    assertion(
        StepId::VariableContainsJson,
        "vars",
        r#"variable "{name}" should contain JSON:"#,
        "asserts a variable's text is JSON containing this JSON — same matching as `the response body contains JSON`",
        OptionsSource::Global,
    ),
    assertion(
        StepId::VariableEqualsJson,
        "vars",
        r#"variable "{name}" should equal JSON:"#,
        "asserts a variable's text is JSON exactly equal to this JSON",
        OptionsSource::Global,
    ),
    assertion(
        StepId::VariableNotContainsJson,
        "vars",
        r#"variable "{name}" should not contain JSON:"#,
        "asserts a variable's text is JSON that does not contain this JSON",
        OptionsSource::Global,
    ),
    action(
        StepId::ExtractJsonFromVariableGlobal,
        "vars",
        r#"extract "{path}" from variable "{variable}" as JSON as "{name}" global"#,
        "reads a JSON path of a variable's text into a variable the whole file can see",
    ),
    action(
        StepId::ExtractJsonFromVariable,
        "vars",
        r#"extract "{path}" from variable "{variable}" as JSON as "{name}""#,
        "reads a JSON path of a variable's text into a variable",
    ),
    action(
        StepId::ExtractRegexFromVariableGlobal,
        "vars",
        r#"extract "{pattern}" from variable "{variable}" as "{name}" global"#,
        "reads the first capture group of a regex over a variable's text into a variable the whole file can see",
    ),
    action(
        StepId::ExtractRegexFromVariable,
        "vars",
        r#"extract "{pattern}" from variable "{variable}" as "{name}""#,
        "reads the first capture group of a regex over a variable's text into a variable",
    ),
    action(
        StepId::EncryptWithAes,
        "vars",
        r#"I encrypt "{value}" with AES using key "{key:secret}" as "{name}""#,
        "encrypts a value with an AES key and stores the result in a variable",
    ),
    action(
        StepId::DebugOn,
        "debug",
        "I am in debug mode",
        "prints the generated SQL, its binds and plugin exchanges to stderr",
    ),
    action(
        StepId::DebugOff,
        "debug",
        "I am not in debug mode",
        "stops the debug output turned on earlier in the scenario",
    ),
    action(
        StepId::EvidenceOn,
        "debug",
        "I am collecting evidence",
        "marks the steps that follow, until the scenario ends, for --evidence=marked",
    ),
    action(
        StepId::EvidenceOff,
        "debug",
        "I am not collecting evidence",
        "ends the mark set earlier in the scenario",
    ),
    action(
        StepId::Sleep,
        "general",
        r#"I sleep {seconds:uint} seconds"#,
        "pauses the scenario — a last resort, an eventual assertion is usually what you want",
    ),
    action(
        StepId::ShowAllVariables,
        "debug",
        "Show all variables",
        "prints every variable in scope to stderr",
    ),
    action(
        StepId::ShowVariable,
        "debug",
        r#"Show "{name}" variable"#,
        "prints one variable to stderr, failing if it is not set",
    ),
    action(
        StepId::PrintTable,
        "debug",
        "Print table:",
        "prints the table below to stderr with variables filled in and columns aligned",
    ),
    action(
        StepId::Include,
        "general",
        r#"I include "{file}"( with:)"#,
        "Runs the only scenario of another feature file inline, sharing HTTP/DB/plugin state.",
    ),
    action(
        StepId::Include,
        "general",
        r#"I include "{file}" with prefix "{prefix}"( with:)"#,
        "Runs the only scenario of another feature file inline, sharing HTTP/DB/plugin state.",
    ),
    action(
        StepId::IncludeScenario,
        "general",
        r#"I include "{file}" scenario "{name}"( with:)"#,
        "Runs one named scenario of another feature file inline, sharing HTTP/DB/plugin state.",
    ),
    action(
        StepId::IncludeScenario,
        "general",
        r#"I include "{file}" scenario "{name}" with prefix "{prefix}"( with:)"#,
        "Runs one named scenario of another feature file inline, sharing HTTP/DB/plugin state.",
    ),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepTarget {
    Builtin {
        id: StepId,
        kind: StepKind,
    },
    Macro(usize),
    /// A step served over FFI. It never becomes a `StepId`, which is what
    /// keeps `dispatch`'s exhaustive match (invariant 4) meaningful.
    Plugin {
        lib: usize,
        step: usize,
        assertion: bool,
    },
}

impl PartialEq<StepId> for StepTarget {
    fn eq(&self, other: &StepId) -> bool {
        matches!(self, Self::Builtin { id, .. } if id == other)
    }
}

/// The group a builtin step is declared under, for telemetry's `usage`.
pub fn group_of(id: StepId) -> &'static str {
    BUILTIN_STEPS
        .iter()
        .find(|def| def.id == id)
        .map_or(crate::telemetry::OTHER, |def| def.group)
}

/// `BUILTIN_STEPS`, compiled once and index-aligned with it. The builtins are
/// constants, so a failure here is a bug in the table, pinned by
/// `all_builtin_expressions_compile`.
pub static COMPILED_BUILTINS: std::sync::LazyLock<Vec<expression::Compiled>> =
    std::sync::LazyLock::new(|| {
        BUILTIN_STEPS
            .iter()
            .map(|def| {
                expression::compile(def.expression, "text")
                    .unwrap_or_else(|e| panic!("builtin step {:?}: {e}", def.expression))
            })
            .collect()
    });

/// A matched step: its target, one argument per parameter (a plugin's trailing
/// groups as they are) and the entry's parameter types.
pub type Found<'a> = (StepTarget, Vec<String>, &'a [expression::Param]);

#[derive(Debug)]
struct Entry {
    target: StepTarget,
    regex: Regex,
    /// One per parameter of an entry declared as an expression (a bare
    /// `optional` one owns two capture groups, see `Param::groups`). A
    /// group-switch regex has none; a plugin's has them only up to its last
    /// secret group (`expression::plugin_params`). Captures of either are
    /// never type-checked; a plugin's `expression` is an expression like any
    /// other.
    params: Vec<expression::Param>,
    /// What `steps list` prints for this entry.
    template: String,
    /// `expression::Compiled::loose`; `None` for a plugin's or a group-switch
    /// regex.
    loose: Option<Regex>,
}

impl Entry {
    fn compiled(target: StepTarget, compiled: &expression::Compiled) -> Self {
        Self {
            target,
            regex: compiled.regex.clone(),
            params: compiled.params.clone(),
            template: compiled.template.clone(),
            loose: compiled.loose.clone(),
        }
    }
}

#[derive(Debug)]
pub struct Registry {
    entries: Vec<Entry>,
    macros: Vec<MacroDef>,
    plugins: Option<std::sync::Arc<crate::plugin::Plugins>>,
}

impl Registry {
    pub fn plugins(&self) -> Option<&std::sync::Arc<crate::plugin::Plugins>> {
        self.plugins.as_ref()
    }

    pub fn plugin_step(&self, lib: usize, step: usize) -> Option<&crate::plugin::abi::StepSpec> {
        self.plugins.as_ref()?.step_spec(lib, step)
    }

    #[cfg(test)]
    pub fn new() -> Result<Self, String> {
        Self::with_macros(MacroCatalog::default())
    }

    /// Only tests build a registry without plugins now: `main` always goes
    /// through `with_macros_and_plugins`, which is a superset.
    #[cfg(test)]
    pub fn with_macros(catalog: MacroCatalog) -> Result<Self, String> {
        Self::with_macros_and_plugins(catalog, &[], &[], None)
    }

    /// Everything is registered before macros are validated, so a macro body may
    /// name a plugin step. Order inside `entries` is irrelevant — `find` collects
    /// every match and reports ambiguity — but a plugin pattern that is not in the
    /// registry yet is indistinguishable from a typo. `plugins` is the handle a
    /// plugin step's spec is asked through at resolve, dispatch and validation
    /// time, macro bodies included.
    pub fn with_macros_and_plugins(
        catalog: MacroCatalog,
        plugin_steps: &[(usize, usize, crate::plugin::Declared, bool)],
        plugin_groups: &[String],
        plugins: Option<std::sync::Arc<crate::plugin::Plugins>>,
    ) -> Result<Self, String> {
        let mut entries = Vec::with_capacity(BUILTIN_STEPS.len());
        for (def, compiled) in BUILTIN_STEPS.iter().zip(COMPILED_BUILTINS.iter()) {
            let target = StepTarget::Builtin {
                id: def.id,
                kind: def.kind,
            };
            entries.push(Entry::compiled(target, compiled));
        }
        for (index, definition) in catalog.definitions.iter().enumerate() {
            entries.push(Entry::compiled(
                StepTarget::Macro(index),
                &definition.compiled,
            ));
        }
        let mut registry = Self {
            entries,
            macros: catalog.definitions,
            plugins,
        };
        let mut expressions = Vec::new();
        for (lib, step, declared, assertion) in plugin_steps {
            match declared {
                crate::plugin::Declared::Pattern {
                    text,
                    secret_groups,
                } => registry.add_plugin_step(*lib, *step, text, *assertion, secret_groups)?,
                crate::plugin::Declared::Expression(compiled) => {
                    let target = StepTarget::Plugin {
                        lib: *lib,
                        step: *step,
                        assertion: *assertion,
                    };
                    registry.entries.push(Entry::compiled(target, compiled));
                    expressions.push(compiled);
                }
            }
        }
        // A deprecated word switches exactly as the group it names does.
        let mut words = plugin_groups.to_vec();
        if let Some(plugins) = &registry.plugins {
            words.extend(plugins.deprecated_words().map(str::to_string));
        }
        registry.add_use_group_step(&words)?;
        registry.check_plugin_conflicts(&expressions)?;
        registry.validate_macros()?;
        Ok(registry)
    }

    /// Returns `Ok(None)` if the step is unknown, and `Err` if it is ambiguous.
    /// Ambiguity is an error, not "first wins": a silently shadowed step
    /// is more expensive to debug than a failed start.
    pub fn find(&self, text: &str) -> Result<Option<(StepTarget, Vec<String>)>, String> {
        Ok(self
            .find_with_params(text)?
            .map(|(target, caps, _)| (target, caps)))
    }

    /// `find`, plus the matched entry's parameter types in declaration order —
    /// what the runner checks interpolated values against.
    pub fn find_with_params(&self, text: &str) -> Result<Option<Found<'_>>, String> {
        let mut hits = Vec::new();
        for entry in &self.entries {
            if let Some(c) = entry.regex.captures(text) {
                let caps = expression::arguments(&entry.params, &c);
                if entry
                    .params
                    .iter()
                    .zip(&caps)
                    .any(|(p, v)| !p.slot_is_whole(v))
                {
                    continue;
                }
                hits.push((entry.target, caps, entry.params.as_slice()));
            }
        }
        match hits.len() {
            0 => Ok(None),
            1 => Ok(Some(hits.remove(0))),
            _ => Err({
                for (_, caps, params) in &hits {
                    record_literal_secrets(params, caps);
                }
                format!(
                    "step {text:?} matches several definitions: {:?}",
                    hits.iter().map(|(target, _, _)| target).collect::<Vec<_>>()
                )
            }),
        }
    }

    /// What follows `unknown step: "…"` wherever that is reported (`validate`,
    /// an include body, a macro body): `\n  <line>` or nothing. A typed
    /// parameter that refused a literal is named first, in the run-time
    /// wording; otherwise the closest templates, as `steps list` prints them.
    pub fn unknown_step_detail(&self, text: &str) -> String {
        let refused = self.entries.iter().find_map(|entry| {
            let caps = entry.loose.as_ref()?.captures(text)?;
            let values = expression::arguments(&entry.params, &caps);
            record_literal_secrets(&entry.params, &values);
            entry
                .params
                .iter()
                .zip(values)
                .filter(|(_, value)| !crate::vars::is_whole_slot(value))
                .find_map(|(param, value)| param.check(&value).err())
        });
        let line = refused.or_else(|| {
            let found = suggest::closest(text, self.entries.iter().map(|e| e.template.as_str()));
            (!found.is_empty()).then(|| format!("did you mean: {}", found.join(" or ")))
        });
        line.map(|line| format!("\n  {line}")).unwrap_or_default()
    }

    /// The step text as a report, the console and the stream show it: a literal
    /// value of a `secret` parameter is written `***`, a value that came through
    /// a `<<…>>` slot stays as written. A text that matches no single entry is
    /// returned as it is — the run reports that failure itself.
    pub fn display_text<'t>(
        &self,
        text: &'t str,
        redactor: &crate::privacy::Redactor,
    ) -> std::borrow::Cow<'t, str> {
        if !redactor.enabled() {
            return std::borrow::Cow::Borrowed(text);
        }
        let mut matches = self.entries.iter().filter_map(|entry| {
            let captures = entry.regex.captures(text)?;
            Some((entry, captures))
        });
        let (Some((entry, captures)), None) = (matches.next(), matches.next()) else {
            return std::borrow::Cow::Borrowed(text);
        };
        let mut spans: Vec<(usize, usize)> = entry
            .params
            .iter()
            .filter(|param| param.ty.secret)
            .filter_map(|param| param.span(&captures))
            .filter(|capture| !crate::vars::is_whole_slot(capture.as_str()))
            .map(|capture| (capture.start(), capture.end()))
            .collect();
        if spans.is_empty() {
            return std::borrow::Cow::Borrowed(text);
        }
        // A plugin's regex can nest its groups, which a builtin's flat
        // `{name:secret}` never does: merge overlaps into one `***`.
        spans.sort_unstable();
        let mut merged: Vec<(usize, usize)> = Vec::with_capacity(spans.len());
        for (start, end) in spans {
            match merged.last_mut() {
                Some(last) if start <= last.1 => last.1 = last.1.max(end),
                _ => merged.push((start, end)),
            }
        }
        let mut shown = String::with_capacity(text.len());
        let mut copied = 0;
        for (start, end) in merged {
            shown.push_str(&text[copied..start]);
            shown.push_str(crate::privacy::MASK);
            copied = end;
        }
        shown.push_str(&text[copied..]);
        std::borrow::Cow::Owned(shown)
    }

    /// The macro definitions, with their bodies, in declaration order.
    pub fn macros(&self) -> &[MacroDef] {
        &self.macros
    }

    /// `(word, name, replacement group)` when `text` is `I use "<name>" <word>`
    /// with a plugin's deprecated word, for `doctor`'s hint. `run` never asks.
    pub fn deprecated_switch(&self, text: &str) -> Option<(String, String, String)> {
        let Ok(Some((
            StepTarget::Builtin {
                id: StepId::UsePluginInstance,
                ..
            },
            caps,
        ))) = self.find(text)
        else {
            return None;
        };
        let group = self.plugins.as_ref()?.resolve_group(&caps[1]).to_string();
        (group != caps[1]).then(|| (caps[1].clone(), caps[0].clone(), group))
    }

    pub fn macro_def(&self, index: usize) -> &MacroDef {
        &self.macros[index]
    }

    /// Registers one pattern a plugin declared. Called at startup, before the
    /// first request, so `validate::check` still sees every step (invariant 1).
    ///
    /// ponytail: no static overlap analysis between two arbitrary plugin
    /// regexes — `find` reports ambiguity per step and `validate::check` runs
    /// it over every selected step, which catches every collision that can
    /// actually fire. Upgrade path if a startup-time check is ever wanted:
    /// extend the token model in steps/expression.rs to raw regexes.
    pub fn add_plugin_step(
        &mut self,
        lib: usize,
        step: usize,
        pattern: &str,
        assertion: bool,
        secret_groups: &[usize],
    ) -> Result<(), String> {
        let re = Regex::new(pattern)
            .map_err(|e| format!("invalid plugin step pattern {pattern:?}: {e}"))?;
        self.entries.push(Entry {
            target: StepTarget::Plugin {
                lib,
                step,
                assertion,
            },
            template: help::template(pattern),
            regex: re,
            params: expression::plugin_params(secret_groups),
            loose: None,
        });
        Ok(())
    }

    /// Registers `I use "<name>" <group>` over the groups the loaded plugins
    /// actually claim. The alternation is built from the real group names, so
    /// the pattern cannot overlap `I use "<name>" api` and no existing
    /// .feature changes meaning.
    pub fn add_use_group_step(&mut self, groups: &[String]) -> Result<(), String> {
        if groups.is_empty() {
            return Ok(());
        }
        let alternation = groups
            .iter()
            .map(|g| regex::escape(g))
            .collect::<Vec<_>>()
            .join("|");
        let pattern = format!(r#"^I use "(?P<name>[^"]*)" (?P<group>{alternation})$"#);
        let re = Regex::new(&pattern)
            .map_err(|e| format!("invalid group-switch pattern {pattern:?}: {e}"))?;
        self.entries.push(Entry {
            target: StepTarget::Builtin {
                id: StepId::UsePluginInstance,
                kind: StepKind::Action,
            },
            template: help::template(&pattern),
            regex: re,
            params: Vec::new(),
            loose: None,
        });
        Ok(())
    }

    /// A plugin's expression meets the check a macro does. A `pattern` is an
    /// arbitrary regex with no token model, so `find` reports its overlaps.
    fn check_plugin_conflicts(&self, expressions: &[&expression::Compiled]) -> Result<(), String> {
        for (index, left) in expressions.iter().enumerate() {
            for (def, builtin) in BUILTIN_STEPS.iter().zip(COMPILED_BUILTINS.iter()) {
                if expression::conflicts(&left.variants, &builtin.variants) {
                    return Err(format!(
                        "plugin step {:?} conflicts with builtin step {:?}",
                        left.template, def.expression
                    ));
                }
            }
            for definition in &self.macros {
                if expression::conflicts(&definition.compiled.variants, &left.variants) {
                    return Err(format!(
                        "macro step {:?} from {}:{} conflicts with plugin step {:?}",
                        definition.step,
                        definition.source.display(),
                        definition.line,
                        left.template
                    ));
                }
            }
            for right in expressions.iter().skip(index + 1) {
                if expression::conflicts(&left.variants, &right.variants) {
                    return Err(format!(
                        "plugin step {:?} conflicts with plugin step {:?}",
                        left.template, right.template
                    ));
                }
            }
        }
        Ok(())
    }

    fn validate_macros(&self) -> Result<(), String> {
        for (left_index, left) in self.macros.iter().enumerate() {
            for (def, builtin) in BUILTIN_STEPS.iter().zip(COMPILED_BUILTINS.iter()) {
                if expression::conflicts(&left.compiled.variants, &builtin.variants) {
                    return Err(format!(
                        "macro step {:?} from {}:{} conflicts with builtin step {:?}",
                        left.step,
                        left.source.display(),
                        left.line,
                        def.expression
                    ));
                }
            }
            for right in self.macros.iter().skip(left_index + 1) {
                if expression::conflicts(&left.compiled.variants, &right.compiled.variants) {
                    return Err(format!(
                        "macro step {:?} from {}:{} conflicts with {:?} from {}:{}",
                        left.step,
                        left.source.display(),
                        left.line,
                        right.step,
                        right.source.display(),
                        right.line,
                    ));
                }
            }
        }

        let mut graph = vec![Vec::new(); self.macros.len()];
        for (index, definition) in self.macros.iter().enumerate() {
            for step in &definition.body {
                match self.find(&step.text)? {
                    Some((StepTarget::Builtin { .. }, _)) => {}
                    Some((target @ StepTarget::Plugin { .. }, caps)) => {
                        // ponytail: a file included from a macro body is not
                        // walked, so its plugin steps are checked only at run time.
                        crate::validate::check_plugin_step(
                            self,
                            &target,
                            &caps,
                            step.docstring.as_deref(),
                            // A macro body step has no table.
                            None,
                            definition
                                .source
                                .parent()
                                .unwrap_or(std::path::Path::new(".")),
                        )
                        .map_err(|e| {
                            format!(
                                "{e}, in macro {:?} from {}:{}",
                                definition.step,
                                definition.source.display(),
                                definition.line,
                            )
                        })?;
                    }
                    Some((StepTarget::Macro(_), _)) if step.docstring.is_some() => {
                        return Err(format!(
                            "macro call {:?} with a doc string inside macro {:?} from {}:{} is not supported",
                            step.text,
                            definition.step,
                            definition.source.display(),
                            definition.line,
                        ));
                    }
                    Some((StepTarget::Macro(target), _)) => graph[index].push(target),
                    None => {
                        return Err(format!(
                            "unknown step {:?} in macro {:?} from {}:{}{}",
                            step.text,
                            definition.step,
                            definition.source.display(),
                            definition.line,
                            self.unknown_step_detail(&step.text),
                        ));
                    }
                }
            }
        }

        let mut visited = vec![false; self.macros.len()];
        let mut visiting = vec![false; self.macros.len()];
        let mut path = Vec::new();
        for index in 0..self.macros.len() {
            self.visit(index, &graph, &mut visited, &mut visiting, &mut path)?;
        }
        let mut depths = vec![None; self.macros.len()];
        for index in 0..self.macros.len() {
            if macro_depth(index, &graph, &mut depths) > 16 {
                return Err(format!(
                    "macro nesting exceeds 16 starting from step {:?}",
                    self.macros[index].step
                ));
            }
        }
        Ok(())
    }

    fn visit(
        &self,
        index: usize,
        graph: &[Vec<usize>],
        visited: &mut [bool],
        visiting: &mut [bool],
        path: &mut Vec<usize>,
    ) -> Result<(), String> {
        if visiting[index] {
            let start = path.iter().position(|item| *item == index).unwrap_or(0);
            let mut cycle: Vec<&str> = path[start..]
                .iter()
                .map(|item| self.macros[*item].step.as_str())
                .collect();
            cycle.push(self.macros[index].step.as_str());
            return Err(format!("cycle in macros: {}", cycle.join(" → ")));
        }
        if visited[index] {
            return Ok(());
        }
        visiting[index] = true;
        path.push(index);
        for child in &graph[index] {
            self.visit(*child, graph, visited, visiting, path)?;
        }
        path.pop();
        visiting[index] = false;
        visited[index] = true;
        Ok(())
    }
}

/// A message about to quote a step that no single entry matched cannot hide a
/// secret by position, as `Registry::display_text` does, so the literal goes to
/// the process sink, which every such message passes through.
// ponytail: masked by value, so `privacy.min_length` applies and a shorter
// secret prints; carry the spans into the message if that ever matters.
fn record_literal_secrets(params: &[expression::Param], values: &[String]) {
    for (_, value) in params
        .iter()
        .zip(values)
        .filter(|(param, value)| param.ty.secret && !crate::vars::is_whole_slot(value))
    {
        crate::privacy::record_value(value);
    }
}

fn macro_depth(index: usize, graph: &[Vec<usize>], memo: &mut [Option<usize>]) -> usize {
    if let Some(depth) = memo[index] {
        return depth;
    }
    let depth = 1 + graph[index]
        .iter()
        .map(|child| macro_depth(*child, graph, memo))
        .max()
        .unwrap_or(0);
    memo[index] = Some(depth);
    depth
}

/// Step arguments after interpolation.
pub struct Args {
    pub caps: Vec<String>,
    pub docstring: Option<String>,
    pub table: Option<Vec<Vec<String>>>,
}

impl Args {
    pub(crate) fn cap(&self, i: usize) -> &str {
        self.caps.get(i).map(String::as_str).unwrap_or("")
    }
}

/// One `match` instead of boxed futures and trait objects.
pub async fn dispatch(w: &mut World, id: StepId, a: &Args) -> AttemptResult {
    let result: Result<(), StepError> = match id {
        StepId::ExpectEventually => {
            w.arm_options(OptionsLayer::default());
            Ok(())
        }
        StepId::ExpectWithin => {
            let timeout_secs = parse_positive(a.cap(0)).map_err(StepError::input)?;
            w.arm_options(OptionsLayer {
                polling: Some(PollingOptionsLayer {
                    timeout_secs: Some(timeout_secs),
                    interval_ms: None,
                }),
            });
            Ok(())
        }
        StepId::ExpectWithinEvery => {
            let timeout_secs = parse_positive(a.cap(0)).map_err(StepError::input)?;
            let interval_ms = parse_positive(a.cap(1)).map_err(StepError::input)?;
            if interval_ms > timeout_secs.saturating_mul(1000) {
                Err(StepError::input(
                    "polling interval_ms must not exceed timeout_secs",
                ))
            } else {
                w.arm_options(OptionsLayer {
                    polling: Some(PollingOptionsLayer {
                        timeout_secs: Some(timeout_secs),
                        interval_ms: Some(interval_ms),
                    }),
                });
                Ok(())
            }
        }
        StepId::SetVariable => vars::set_variable(w, a.cap(0), a.cap(1), false),
        StepId::SetVariableGlobal => vars::set_variable(w, a.cap(0), a.cap(1), true),
        StepId::VariableEquals => {
            return vars::variable_equals(w, a.cap(0), a.cap(1), false);
        }
        StepId::VariableNotEquals => {
            return vars::variable_equals(w, a.cap(0), a.cap(1), true);
        }
        StepId::VariableContains => return vars::variable_contains(w, a.cap(0), a.cap(1), false),
        StepId::VariableNotContains => return vars::variable_contains(w, a.cap(0), a.cap(1), true),
        StepId::VariableMatches => return vars::variable_matches(w, a.cap(0), a.cap(1), false),
        StepId::VariableNotMatches => return vars::variable_matches(w, a.cap(0), a.cap(1), true),
        StepId::VariableEmpty => return vars::variable_empty(w, a.cap(0)),
        StepId::VariableContainsJson => {
            return vars::variable_contains_json(w, a.cap(0), a.docstring.as_ref());
        }
        StepId::VariableEqualsJson => {
            return vars::variable_equals_json(w, a.cap(0), a.docstring.as_ref());
        }
        StepId::VariableNotContainsJson => {
            return vars::variable_not_contains_json(w, a.cap(0), a.docstring.as_ref());
        }
        StepId::ExtractJsonFromVariable => {
            vars::extract_json_from_variable(w, a.cap(0), a.cap(1), a.cap(2), false)
        }
        StepId::ExtractJsonFromVariableGlobal => {
            vars::extract_json_from_variable(w, a.cap(0), a.cap(1), a.cap(2), true)
        }
        StepId::ExtractRegexFromVariable => {
            vars::extract_regex_from_variable(w, a.cap(0), a.cap(1), a.cap(2), false)
        }
        StepId::ExtractRegexFromVariableGlobal => {
            vars::extract_regex_from_variable(w, a.cap(0), a.cap(1), a.cap(2), true)
        }
        StepId::EncryptWithAes => vars::encrypt_with_aes(w, a.cap(0), a.cap(1), a.cap(2)),
        StepId::UsePluginInstance => plugin::use_instance(w, a.cap(1), a.cap(0)).await,
        StepId::DebugOn => debug::debug_on(w),
        StepId::DebugOff => debug::debug_off(w),
        StepId::EvidenceOn => {
            w.trace.mark(true);
            Ok(())
        }
        StepId::EvidenceOff => {
            w.trace.mark(false);
            Ok(())
        }
        StepId::Sleep => sleep(a.cap(0)).await.map_err(StepError::input),
        StepId::ShowAllVariables => show_all_variables(w).map_err(StepError::input),
        StepId::ShowVariable => show_variable(w, a.cap(0)).map_err(StepError::input),
        StepId::PrintTable => debug::print_table(w, a.table.as_ref()).map_err(StepError::input),
        StepId::Include | StepId::IncludeScenario => {
            unreachable!("execute_step intercepts Include/IncludeScenario before calling dispatch")
        }
    };
    result.map_err(AttemptError::Fatal)
}

fn parse_positive(value: &str) -> Result<u64, String> {
    let parsed = value
        .parse::<u64>()
        .map_err(|_| format!("polling value must be a positive integer: {value:?}"))?;
    if parsed == 0 {
        Err("polling value must be positive".to_string())
    } else {
        Ok(parsed)
    }
}

async fn sleep(secs: &str) -> Result<(), String> {
    let n = secs
        .parse::<u64>()
        .map_err(|_| format!("not a number: {secs}"))?;
    tokio::time::sleep(std::time::Duration::from_secs(n)).await;
    Ok(())
}

fn show_all_variables(w: &World) -> Result<(), String> {
    debug_eprintln!(w, "=== All variables ===");
    for (name, value) in w.vars.all_vars() {
        debug_eprintln!(w, "{}: {}", name, w.redactor.shown(&name, &value));
    }
    Ok(())
}

fn show_variable(w: &World, name: &str) -> Result<(), String> {
    match w.vars.get(name) {
        Some(v) => {
            debug_eprintln!(w, "{}: {}", name, w.redactor.shown(name, v));
            Ok(())
        }
        None => Err(format!("variable not found: {name}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reg() -> Registry {
        Registry::new().unwrap()
    }

    #[test]
    fn a_malformed_slot_in_a_typed_position_matches_no_step() {
        let r = reg();
        for text in ["I sleep <<x>> seconds", r#"I sleep "<<x>>" seconds"#] {
            assert!(r.find(text).unwrap().is_some(), "{text}");
        }
        for text in [
            "I sleep <<user-id>> seconds",
            "I sleep <<>> seconds",
            "I sleep <<1x>> seconds",
        ] {
            assert!(r.find(text).unwrap().is_none(), "{text}");
        }
    }

    #[test]
    fn all_builtin_expressions_compile() {
        assert_eq!(COMPILED_BUILTINS.len(), BUILTIN_STEPS.len());
        assert!(Registry::new().is_ok());
    }

    #[test]
    fn no_two_distinct_builtin_steps_conflict() {
        // The include twins share a StepId: one step, declared twice.
        for (i, (a, ca)) in BUILTIN_STEPS
            .iter()
            .zip(COMPILED_BUILTINS.iter())
            .enumerate()
        {
            for (b, cb) in BUILTIN_STEPS
                .iter()
                .zip(COMPILED_BUILTINS.iter())
                .skip(i + 1)
            {
                if a.id != b.id {
                    assert!(
                        !expression::conflicts(&ca.variants, &cb.variants),
                        "{:?} conflicts with {:?}",
                        a.expression,
                        b.expression
                    );
                }
            }
        }
    }

    #[test]
    fn a_typed_position_accepts_a_whole_placeholder() {
        let reg = reg();
        let (target, caps, params) = reg
            .find_with_params("I sleep <<n>> seconds")
            .unwrap()
            .expect("matches");
        assert!(target == StepId::Sleep);
        assert_eq!(caps, ["<<n>>"]);
        assert_eq!(params[0].name, "seconds");
        assert!(params[0].check("abc").is_err());
        assert!(reg.find(r#"I sleep "<<n>>" seconds"#).unwrap().is_some());
        assert!(reg.find("I sleep abc seconds").unwrap().is_none());
    }

    #[test]
    fn a_plugin_step_carries_no_parameter_types() {
        let mut reg = Registry::new().expect("registry");
        reg.add_plugin_step(0, 1, r#"^I upload file "([^"]*)"$"#, false, &[])
            .expect("valid pattern");
        let (_, _, params) = reg
            .find_with_params(r#"I upload file "a""#)
            .unwrap()
            .expect("matches");
        assert!(params.is_empty());
    }

    fn expression_registry(expressions: &[&str]) -> Result<Registry, String> {
        let steps: Vec<_> = expressions
            .iter()
            .enumerate()
            .map(|(index, text)| {
                let compiled = expression::compile(text, "text").expect("compiles");
                (
                    0,
                    index,
                    crate::plugin::Declared::Expression(compiled),
                    false,
                )
            })
            .collect();
        Registry::with_macros_and_plugins(MacroCatalog::default(), &steps, &[], None)
    }

    #[test]
    fn a_plugin_expression_is_found_with_one_argument_per_parameter() {
        let reg = expression_registry(&[r#"I ship {n:uint} to "{city}""#]).expect("builds");
        for text in [r#"I ship 5 to "Riga""#, r#"I ship "5" to "Riga""#] {
            let (target, caps) = reg.find(text).expect("no ambiguity").expect("matched");
            assert!(matches!(target, StepTarget::Plugin { step: 0, .. }));
            assert_eq!(caps, ["5", "Riga"], "{text}");
        }
        assert!(
            reg.unknown_step_detail(r#"I ship abc to "Riga""#)
                .contains(r#"parameter <n> expects uint, got "abc""#)
        );
    }

    #[test]
    fn a_plugin_expression_conflict_is_refused_as_a_macros_is() {
        let error = expression_registry(&[r#"set variable "{a}" to "{b}""#])
            .expect_err("overlaps a builtin");
        assert!(error.contains("conflicts with builtin step"), "{error}");
        let error = expression_registry(&["I ship {n:uint}", "I ship {n:word}"])
            .expect_err("overlaps the other plugin step");
        assert!(error.contains("conflicts with plugin step"), "{error}");
        expression_registry(&["I ship {n:uint}", "I ship all"]).expect("no overlap");
    }

    #[test]
    fn a_plugin_secret_group_is_hidden_like_a_secret_parameter() {
        let mut reg = Registry::new().expect("registry");
        reg.add_plugin_step(
            0,
            1,
            r#"^I sign in as "([^"]*)" with "([^"]*)"$"#,
            false,
            &[2],
        )
        .expect("valid pattern");
        let redactor = crate::privacy::Redactor::new(
            &crate::privacy::Privacy::default(),
            crate::privacy::Secrets::new(7),
        );
        let shown = |text: &str| reg.display_text(text, &redactor).into_owned();
        assert_eq!(
            shown(r#"I sign in as "ann" with "hunter2""#),
            r#"I sign in as "ann" with "***""#
        );
        assert_eq!(
            shown(r#"I sign in as "ann" with "<<pw>>""#),
            r#"I sign in as "ann" with "<<pw>>""#
        );
        let (_, _, params) = reg
            .find_with_params(r#"I sign in as "ann" with "x""#)
            .unwrap()
            .expect("matches");
        assert_eq!(
            params.iter().map(|p| p.ty.secret).collect::<Vec<_>>(),
            [false, true]
        );
    }

    #[test]
    fn nested_plugin_secret_groups_are_masked_once_and_do_not_panic() {
        let mut reg = Registry::new().expect("registry");
        reg.add_plugin_step(0, 1, r#"^I use "((\w+):(\w+))"$"#, false, &[1, 2])
            .expect("valid pattern");
        let redactor = crate::privacy::Redactor::new(
            &crate::privacy::Privacy::default(),
            crate::privacy::Secrets::new(7),
        );
        assert_eq!(
            reg.display_text(r#"I use "ann:pw""#, &redactor),
            r#"I use "***""#
        );
    }

    #[test]
    fn a_plugin_step_is_found_like_any_other() {
        let mut reg = Registry::new().expect("registry");
        reg.add_plugin_step(
            0,
            1,
            r#"^I upload file "([^"]*)" to "([^"]*)"$"#,
            false,
            &[],
        )
        .expect("valid pattern");
        let (target, caps) = reg
            .find(r#"I upload file "report.pdf" to "backups""#)
            .expect("no ambiguity")
            .expect("matched");
        assert_eq!(
            target,
            StepTarget::Plugin {
                lib: 0,
                step: 1,
                assertion: false
            }
        );
        assert_eq!(caps, vec!["report.pdf".to_string(), "backups".to_string()]);
    }

    #[test]
    fn an_invalid_plugin_pattern_is_rejected_at_registration() {
        let mut reg = Registry::new().expect("registry");
        let error = reg
            .add_plugin_step(0, 0, "^I upload file \"([^\"]*$", false, &[])
            .expect_err("unbalanced group");
        assert!(error.contains("plugin"), "{error}");
    }

    #[test]
    fn a_plugin_pattern_colliding_with_a_builtin_is_an_ambiguity_error() {
        // Cross-layer ambiguity needs no separate checker: `find` already reports
        // every match, and validate::check runs `find` over every selected step
        // before the first request.
        let mut reg = Registry::new().expect("registry");
        reg.add_plugin_step(0, 0, r#"^I sleep (\d+) seconds$"#, true, &[])
            .expect("valid pattern");
        let error = reg
            .find("I sleep 2 seconds")
            .expect_err("two definitions match");
        assert!(error.contains("several definitions"), "{error}");
    }

    #[test]
    fn a_group_name_with_regex_metacharacters_is_escaped_not_interpreted() {
        let reg = Registry::with_macros_and_plugins(
            MacroCatalog::default(),
            &[],
            &["widget.beta".to_string()],
            None,
        )
        .expect("registry");

        // If the dot were left as a regex metachar it would match any single
        // character here too, not just a literal dot.
        assert!(
            reg.find(r#"I use "x" widgetXbeta"#)
                .expect("no ambiguity")
                .is_none(),
            "an unescaped '.' would wrongly match any character"
        );
        assert!(
            reg.find(r#"I use "x" widget.beta"#)
                .expect("no ambiguity")
                .is_some(),
            "the literal group name must still match"
        );
    }

    #[test]
    fn every_builtin_declares_a_known_group_and_a_description() {
        // The group is what `bddkit steps list` prints and filters on: a typo
        // there hides a whole step from discovery without failing anything else.
        const GROUPS: &[&str] = &["vars", "debug", "general"];
        for def in BUILTIN_STEPS {
            assert!(
                GROUPS.contains(&def.group),
                "{:?}: unknown group {:?}",
                def.id,
                def.group
            );
            assert!(
                !def.description.is_empty(),
                "{:?} has no description",
                def.id
            );
        }
    }

    #[test]
    fn a_macro_conflicting_with_a_named_builtin_is_still_rejected() {
        // A parameterless macro against a builtin whose capture must expand:
        // the builtin side of the conflict check is the one under test.
        let path = std::env::temp_dir().join(format!(
            "bddkit-steps-named-group-conflict-{}.yaml",
            std::process::id()
        ));
        std::fs::write(
            &path,
            "- step: variable \"a\" should be empty\n  do: [Show all variables]\n",
        )
        .expect("write macro file");
        let catalog = MacroCatalog::load(std::slice::from_ref(&path)).expect("macro loads");
        std::fs::remove_file(&path).ok();

        let error = Registry::with_macros(catalog).expect_err("the macro shadows a builtin");
        assert!(error.contains("conflicts with builtin"), "{error}");
    }

    #[test]
    fn a_macro_conflicting_with_include_is_still_rejected() {
        // Regression (#50): the include steps end in an optional "( with:)";
        // a macro equal to the bare step must still be seen as shadowing it.
        let path = std::env::temp_dir().join(format!(
            "bddkit-steps-include-conflict-{}.yaml",
            std::process::id()
        ));
        std::fs::write(
            &path,
            "- step: I include \"setup.feature\"\n  do: [Show all variables]\n",
        )
        .expect("write macro file");
        let catalog = MacroCatalog::load(std::slice::from_ref(&path)).expect("macro loads");
        std::fs::remove_file(&path).ok();

        let error =
            Registry::with_macros(catalog).expect_err("the macro shadows the Include builtin");
        assert!(error.contains("conflicts with builtin"), "{error}");

        std::fs::write(
            &path,
            "- step: \"I include \\\"setup.feature\\\" with:\"\n  do: [Show all variables]\n",
        )
        .expect("write macro file");
        let catalog = MacroCatalog::load(std::slice::from_ref(&path)).expect("macro loads");
        std::fs::remove_file(&path).ok();

        let error = Registry::with_macros(catalog)
            .expect_err("the macro shadows the Include builtin with a table");
        assert!(error.contains("conflicts with builtin"), "{error}");
    }

    #[test]
    fn a_macro_body_may_call_a_plugin_step() {
        // Macros exist to compose steps and plugins exist to add steps.
        // Validating macros before the plugin patterns are registered made a
        // plugin step in a macro body indistinguishable from a typo.
        let path = std::env::temp_dir().join(format!(
            "bddkit-steps-macro-calls-plugin-{}.yaml",
            std::process::id()
        ));
        std::fs::write(
            &path,
            "- step: I do the plugin thing\n  do: [I upload file \"a\" to \"b\"]\n",
        )
        .expect("write macro file");
        let catalog = MacroCatalog::load(std::slice::from_ref(&path)).expect("macro loads");
        std::fs::remove_file(&path).ok();

        let reg = Registry::with_macros_and_plugins(
            catalog,
            &[(
                0,
                1,
                crate::plugin::Declared::Pattern {
                    text: r#"^I upload file "([^"]*)" to "([^"]*)"$"#.to_string(),
                    secret_groups: Vec::new(),
                },
                false,
            )],
            &[],
            None,
        )
        .expect("a plugin step in a macro body is not a typo");

        let (target, _) = reg
            .find("I do the plugin thing")
            .expect("no ambiguity")
            .expect("matched");
        assert_eq!(target, StepTarget::Macro(0));
    }

    #[test]
    fn an_unknown_step_names_the_typed_parameter_that_refused_its_literal() {
        let reg = Registry::new().unwrap();
        assert_eq!(
            reg.unknown_step_detail("I sleep abc seconds"),
            "\n  parameter <seconds> expects uint, got \"abc\""
        );
        assert_eq!(
            reg.unknown_step_detail(r#"I sleep "abc" seconds"#),
            "\n  parameter <seconds> expects uint, got \"abc\""
        );
    }

    #[test]
    fn an_unknown_step_suggests_the_closest_templates() {
        let reg = Registry::new().unwrap();
        let cases = [(
            r#"variable "a" should be empt"#,
            r#"variable "<name>" should be empty"#,
        )];
        for (text, suggestion) in cases {
            assert_eq!(
                reg.unknown_step_detail(text),
                format!("\n  did you mean: {suggestion}"),
                "{text}"
            );
        }
        assert_eq!(reg.unknown_step_detail("please make me a sandwich"), "");
        // Too far from every template at a third of the phrase: no guess.
        assert_eq!(
            reg.unknown_step_detail(r#"I send a GET request to "/x""#),
            ""
        );
        // An unterminated quote swallows the rest as one token, not drops it.
        let mut grouped = Registry::new().unwrap();
        grouped.add_use_group_step(&["echo".to_string()]).unwrap();
        assert_eq!(
            grouped.unknown_step_detail(r#"I use "x" ech"#),
            "\n  did you mean: I use \"<name>\" <group>"
        );
    }

    #[test]
    fn tied_plugin_templates_are_all_suggested() {
        let mut reg = Registry::new().unwrap();
        reg.add_plugin_step(0, 0, r"^the echo counter is (\d+)$", true, &[])
            .unwrap();
        reg.add_plugin_step(0, 1, r"^the echo counter was (\d+)$", true, &[])
            .unwrap();
        assert_eq!(
            reg.unknown_step_detail("the echo counter be 3"),
            "\n  did you mean: the echo counter is <value1> or the echo counter was <value1>"
        );
    }

    #[test]
    fn a_macro_body_with_a_near_miss_step_gets_a_suggestion() {
        let path = std::env::temp_dir().join(format!(
            "bddkit-steps-macro-near-miss-{}.yaml",
            std::process::id()
        ));
        std::fs::write(
            &path,
            "- step: I do the mystery thing\n  do: [Show all variable]\n",
        )
        .expect("write macro file");
        let catalog = MacroCatalog::load(std::slice::from_ref(&path)).expect("macro loads");
        std::fs::remove_file(&path).ok();

        let error = Registry::with_macros_and_plugins(catalog, &[], &[], None)
            .expect_err("no step matches the body");
        assert!(
            error.contains("did you mean: Show all variables"),
            "{error}"
        );
    }

    #[test]
    fn a_macro_body_calling_a_truly_unknown_step_still_fails() {
        let path = std::env::temp_dir().join(format!(
            "bddkit-steps-macro-calls-unknown-{}.yaml",
            std::process::id()
        ));
        std::fs::write(
            &path,
            "- step: I do the mystery thing\n  do: [I have never heard of this step]\n",
        )
        .expect("write macro file");
        let catalog = MacroCatalog::load(std::slice::from_ref(&path)).expect("macro loads");
        std::fs::remove_file(&path).ok();

        let error = Registry::with_macros_and_plugins(catalog, &[], &[], None)
            .expect_err("no builtin, macro, or plugin step matches");
        assert!(error.contains("unknown step"), "{error}");
    }

    #[test]
    fn eventual_modifiers_are_registered_as_actions() {
        let cases = [
            (
                "I expect the next assertion to pass eventually",
                StepId::ExpectEventually,
                vec![],
            ),
            (
                r#"I expect the next assertion to pass within "10" seconds"#,
                StepId::ExpectWithin,
                vec!["10".to_string()],
            ),
            (
                r#"I expect the next assertion to pass within "10" seconds, checking every "100" milliseconds"#,
                StepId::ExpectWithinEvery,
                vec!["10".to_string(), "100".to_string()],
            ),
        ];

        for (text, expected_id, expected_caps) in cases {
            let (target, caps) = reg().find(text).unwrap().expect("step is registered");
            let StepTarget::Builtin { id, kind } = target else {
                panic!("modifier resolved to a macro");
            };
            assert_eq!(id, expected_id);
            assert_eq!(kind, StepKind::Action);
            assert_eq!(caps, expected_caps);
        }
    }

    #[test]
    fn assertions_keep_their_option_source_in_registry_metadata() {
        let cases = [(
            r#"variable "state" should be equal to "ready""#,
            OptionsSource::Global,
        )];

        for (text, source) in cases {
            let (target, _) = reg().find(text).unwrap().expect("assertion is registered");
            let StepTarget::Builtin { kind, .. } = target else {
                panic!("assertion resolved to a macro");
            };
            assert_eq!(kind, StepKind::Assertion(source));
        }
    }

    #[test]
    fn no_two_builtin_steps_are_ambiguous() {
        // Every pattern is checked against all the others on concrete examples.
        let samples = [
            r#"set variable "a" to "1""#,
            r#"set variable "a" to "1" global"#,
            r#"variable "a" should be equal to "1""#,
            r#"variable "a" should not be equal to "1""#,
            r#"variable "a" should contain "1""#,
            r#"variable "a" should not contain "1""#,
            r#"variable "a" should match "^\d+$""#,
            r#"variable "a" should not match "^\d+$""#,
            r#"variable "a" should be empty"#,
            "variable \"a\" should contain JSON:",
            "variable \"a\" should equal JSON:",
            "variable \"a\" should not contain JSON:",
            r#"extract "id" from variable "a" as JSON as "b""#,
            r#"extract "id" from variable "a" as JSON as "b" global"#,
            r#"extract "(\d+)" from variable "a" as "b""#,
            r#"extract "(\d+)" from variable "a" as "b" global"#,
            r#"I encrypt "555555" with AES using key "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f" as "otp""#,
            "I am in debug mode",
            "I am not in debug mode",
            "I am collecting evidence",
            "I am not collecting evidence",
            r#"I sleep "5" seconds"#,
            "Show all variables",
            r#"Show "userId" variable"#,
            r#"I include "a.feature""#,
            r#"I include "a.feature" with:"#,
            r#"I include "a.feature" with prefix "p""#,
            r#"I include "a.feature" scenario "s" with prefix "p" with:"#,
        ];
        let r = reg();
        for s in samples {
            match r.find(s) {
                Ok(Some(_)) => {}
                Ok(None) => panic!("step not recognized: {s}"),
                Err(e) => panic!("ambiguity: {e}"),
            }
        }
    }

    #[test]
    fn display_text_hides_a_literal_secret_and_keeps_a_slot() {
        let r = reg();
        let redactor = crate::privacy::Redactor::new(
            &crate::privacy::Privacy::default(),
            crate::privacy::Secrets::new(7),
        );
        let shown = |text: &str| r.display_text(text, &redactor).into_owned();
        assert_eq!(
            shown(r#"I encrypt "x" with AES using key "abc" as "y""#),
            r#"I encrypt "x" with AES using key "***" as "y""#
        );
        assert_eq!(
            shown(r#"I encrypt "x" with AES using key "<<key>>" as "y""#),
            r#"I encrypt "x" with AES using key "<<key>>" as "y""#
        );
        // A literal glued to a slot is still a literal.
        assert_eq!(
            shown(r#"I encrypt "x" with AES using key "ab<<key>>" as "y""#),
            r#"I encrypt "x" with AES using key "***" as "y""#
        );
        assert_eq!(
            shown(r#"I include "a.feature""#),
            r#"I include "a.feature""#
        );
        assert_eq!(shown("no such step"), "no such step");
    }

    #[test]
    fn the_aes_encryption_step_resolves_to_its_builtin() {
        let (target, caps) = reg()
            .find(
                r#"I encrypt "555555" with AES using key "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f" as "otp""#,
            )
            .expect("pattern is unambiguous")
            .expect("step is declared");

        assert!(target == StepId::EncryptWithAes);
        assert_eq!(
            caps,
            vec![
                "555555",
                "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
                "otp",
            ]
        );
    }

    #[test]
    fn global_suffix_picks_the_global_variant() {
        let (id, _) = reg()
            .find(r#"set variable "a" to "1" global"#)
            .unwrap()
            .unwrap();
        assert_eq!(id, StepId::SetVariableGlobal);
        let (id, _) = reg().find(r#"set variable "a" to "1""#).unwrap().unwrap();
        assert_eq!(id, StepId::SetVariable);
    }

    #[test]
    fn unknown_method_is_not_a_known_step() {
        assert!(
            reg()
                .find(r#"I request "/ping" using HTTP POSTT"#)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn unknown_step_returns_none() {
        assert!(reg().find("I refund the order").unwrap().is_none());
    }

    #[test]
    fn include_step_matches_a_bare_file() {
        let reg = Registry::new().unwrap();
        let (target, caps) = reg
            .find(r#"I include "flows/register.feature""#)
            .unwrap()
            .unwrap();
        assert!(matches!(
            target,
            StepTarget::Builtin {
                id: StepId::Include,
                ..
            }
        ));
        assert_eq!(caps, vec!["flows/register.feature".to_string()]);
    }

    #[test]
    fn include_step_matches_file_and_scenario_name() {
        let reg = Registry::new().unwrap();
        let (target, caps) = reg
            .find(r#"I include "flows/admin.feature" scenario "Activate a user""#)
            .unwrap()
            .unwrap();
        assert!(matches!(
            target,
            StepTarget::Builtin {
                id: StepId::IncludeScenario,
                ..
            }
        ));
        assert_eq!(
            caps,
            vec![
                "flows/admin.feature".to_string(),
                "Activate a user".to_string(),
            ]
        );
    }

    #[test]
    fn include_step_matches_with_prefix() {
        let reg = Registry::new().unwrap();
        let (target, caps) = reg
            .find(r#"I include "flows/register.feature" with prefix "buyer""#)
            .unwrap()
            .unwrap();
        assert!(matches!(
            target,
            StepTarget::Builtin {
                id: StepId::Include,
                ..
            }
        ));
        assert_eq!(
            caps,
            vec!["flows/register.feature".to_string(), "buyer".to_string()]
        );
    }

    #[test]
    fn include_scenario_step_matches_with_prefix_and_table() {
        let reg = Registry::new().unwrap();
        let (target, caps) = reg
            .find(
                r#"I include "flows/admin.feature" scenario "Activate a user" with prefix "buyer" with:"#,
            )
            .unwrap()
            .unwrap();
        assert!(matches!(
            target,
            StepTarget::Builtin {
                id: StepId::IncludeScenario,
                ..
            }
        ));
        assert_eq!(
            caps,
            vec![
                "flows/admin.feature".to_string(),
                "Activate a user".to_string(),
                "buyer".to_string(),
            ]
        );
    }

    #[test]
    fn include_step_matches_the_with_suffixed_form() {
        let reg = Registry::new().unwrap();
        let (target, caps) = reg
            .find(r#"I include "flows/register.feature" with:"#)
            .unwrap()
            .unwrap();
        assert!(matches!(
            target,
            StepTarget::Builtin {
                id: StepId::Include,
                ..
            }
        ));
        assert_eq!(caps, vec!["flows/register.feature".to_string()]);
    }
}
