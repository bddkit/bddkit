//! What the host does with a plugin's reply before it reads the reply's
//! error: registers everything sensitive in it, in a fixed order, and sorts
//! its diagnostics into the shapes the host renders.
//!
//! The order is the point. A `not_yet` message may quote a body that the same
//! reply's `scan` reveals, so the secrets, the `scan` items and the sensitive
//! fields of the `json` and `http` diagnostics all join the set first, on
//! every status, even for a reply whose diagnostics are then dropped.

use super::abi::{Diagnostic, DispatchResult};
use crate::diagnostic::{HttpDiagnostic, SqlDiagnostic};
use crate::events::Trace;
use crate::failure::FailureKind;
use crate::privacy::Redactor;
use serde_json::Value;
use std::path::Path;

use super::Plugins;
use crate::world::World;

/// A diagnostic whose text, title and kind are all plain strings: the shape
/// every plugin of ABI 3 sends.
#[derive(Debug, Clone)]
pub struct Plain {
    pub title: String,
    pub kind: String,
    pub content: Option<String>,
    pub path: Option<String>,
    pub media_type: Option<String>,
    pub in_dump: bool,
    /// The instance the diagnostic concerns; absent: the selected one.
    pub resource: Option<String>,
}

/// One diagnostic of a reply, ready to dump and to emit.
#[derive(Debug, Clone)]
pub enum Shown {
    Http {
        diagnostic: HttpDiagnostic,
        title: Option<String>,
    },
    Sql {
        diagnostic: SqlDiagnostic,
        title: Option<String>,
    },
    Plain(Plain),
}

/// Registers what `result` reveals and returns its diagnostics. `selected` is
/// the instance a diagnostic without a `resource` concerns. A shape that does
/// not fit is an `Err` that quotes nothing, returned only after everything
/// that could be registered was.
pub fn absorb(
    result: &DispatchResult,
    redactor: &Redactor,
    selected: &str,
) -> Result<Vec<Shown>, String> {
    for secret in &result.secrets {
        redactor.secret(secret);
    }
    let mut refused = None;
    for item in &result.scan {
        if let Err(why) = item.register(redactor) {
            refused.get_or_insert(why);
        }
    }
    let mut shown = Vec::with_capacity(result.diagnostics.len());
    for diagnostic in &result.diagnostics {
        match one(diagnostic, redactor, selected) {
            Ok(item) => shown.push(item),
            Err(why) => {
                refused.get_or_insert(why);
            }
        }
    }
    match refused {
        // The plugin's own words come with the refusal: they are what the
        // tester needs, and everything the reply revealed is registered.
        Some(why) => Err(match &result.error {
            Some(error) => {
                format!("the plugin's reply was refused: {why}; the plugin said: {error}")
            }
            None => format!("the plugin's reply was refused: {why}"),
        }),
        None => Ok(shown),
    }
}

fn one(diagnostic: &Diagnostic, redactor: &Redactor, selected: &str) -> Result<Shown, String> {
    let in_dump = diagnostic.in_dump.unwrap_or(true);
    let resource = diagnostic.resource.as_deref().unwrap_or(selected);
    let title = diagnostic.title.clone();
    match (&diagnostic.content, diagnostic.kind.as_str()) {
        (Some(content @ Value::Object(_)), "http") => {
            let diagnostic = HttpDiagnostic::from_plugin(content, resource, in_dump)?;
            diagnostic.register(redactor);
            Ok(Shown::Http { diagnostic, title })
        }
        (Some(content @ Value::Object(_)), "sql") => {
            let diagnostic = SqlDiagnostic::from_plugin(content, resource)?;
            Ok(Shown::Sql { diagnostic, title })
        }
        (content, kind) => {
            let content = match content {
                None => None,
                Some(Value::String(text)) => Some(text.clone()),
                Some(_) => {
                    return Err(
                        "a diagnostic's content is an object only for the kinds http and sql, \
                         and a string for every other"
                            .into(),
                    );
                }
            };
            let title =
                title.ok_or("a diagnostic has no title, which only http and sql may omit")?;
            if kind == "json"
                && let Some(text) = &content
            {
                redactor.register(&redactor.json_hits(text));
            }
            Ok(Shown::Plain(Plain {
                title,
                kind: diagnostic.kind.clone(),
                content,
                path: diagnostic.path.clone(),
                media_type: diagnostic.media_type.clone(),
                in_dump,
                resource: diagnostic.resource.clone(),
            }))
        }
    }
}

impl Shown {
    /// The text this diagnostic adds to the failure dump: masked by position
    /// where its structure allows, then by value. `None` for one that is
    /// evidence only, and for `sql`, which the dump never shows.
    pub fn dump(&self, redactor: &Redactor) -> Option<String> {
        match self {
            Self::Http { diagnostic, .. } => diagnostic.dump(redactor),
            Self::Sql { .. } => None,
            Self::Plain(plain) if plain.in_dump => {
                let mut out = format!("--- {} ({}) ---", plain.title, plain.kind);
                if let Some(content) = &plain.content {
                    out.push('\n');
                    out.push_str(&if plain.kind == "json" {
                        redactor.mask_json(content)
                    } else {
                        redactor.mask(content)
                    });
                }
                if let Some(path) = &plain.path {
                    out.push_str(&format!("\n{path}"));
                }
                Some(out)
            }
            Self::Plain(_) => None,
        }
    }

    /// Emits the `evidence` event of this diagnostic (nothing without
    /// evidence); returns what could not be done, as warnings.
    pub fn emit(
        &self,
        trace: &Trace,
        group: &str,
        instance: &str,
        dispatch_dir: &Path,
    ) -> Vec<String> {
        match self {
            Self::Http { diagnostic, title } => {
                trace.http_in(group, diagnostic, title.as_deref());
                Vec::new()
            }
            Self::Sql { diagnostic, title } => {
                trace.sql_in(group, diagnostic, title.as_deref());
                Vec::new()
            }
            Self::Plain(plain) => trace.diagnostic(
                group,
                plain.resource.as_deref().unwrap_or(instance),
                dispatch_dir,
                plain,
            ),
        }
    }
}

/// The dump texts of a failing reply's diagnostics, in order.
pub fn dumps(shown: &[Shown], redactor: &Redactor) -> Vec<String> {
    shown
        .iter()
        .filter_map(|item| item.dump(redactor))
        .collect()
}

/// The class a failing reply names, and the warning to show when it names one
/// the host does not know (once per run and value): that counts as absent, so
/// the caller falls back to today's class. `plugin`, `reset` and `panic` are
/// the host's own and cannot be named.
pub fn class(result: &DispatchResult, plugins: &Plugins) -> (Option<FailureKind>, Option<String>) {
    let Some(value) = &result.class else {
        return (None, None);
    };
    let known = match value.as_str() {
        Some("assertion") => Some(FailureKind::Assertion),
        Some("transport") => Some(FailureKind::Transport),
        Some("db_connection") => Some(FailureKind::DbConnection),
        Some("db_statement") => Some(FailureKind::DbStatement),
        Some("input") => Some(FailureKind::Input),
        Some("unclassified") => Some(FailureKind::Unclassified),
        _ => None,
    };
    if known.is_some() {
        return (known, None);
    }
    let named: String = value
        .as_str()
        .map_or_else(|| value.to_string(), str::to_string)
        .chars()
        .take(40)
        .collect();
    let note = plugins.first_unknown_class(&named).then(|| {
        format!("a plugin replied with the failure class {named:?}, which is not one it may name; it counts as absent")
    });
    (None, note)
}

/// One item of a reply's `output`.
#[derive(serde::Deserialize)]
#[serde(untagged)]
pub enum Out {
    Headers {
        title: String,
        headers: Vec<(String, String)>,
        #[serde(default)]
        sensitive_headers: Vec<String>,
    },
    Text {
        text: String,
        #[serde(default)]
        syntax: Option<String>,
    },
}

/// The `output` items of a reply. An item that fits neither shape is refused
/// with a message that quotes nothing.
pub fn outputs(result: &DispatchResult) -> Result<Vec<Out>, String> {
    result
        .output
        .iter()
        .map(|item| {
            serde_json::from_value(item.clone())
                .map_err(|_| "an output item fits neither shape of text or headers".to_string())
        })
        .collect()
}

/// Prints the items to stderr in one write, rendered as the debug prints are
/// and masked once more over the whole text, as `debug_eprintln!` does: a
/// title or a header name is no more exempt than a body. A `json` text is
/// pretty-printed first when it parses.
pub fn print_output(items: &[Out], redactor: &Redactor) {
    if items.is_empty() {
        return;
    }
    let lines: Vec<String> = items
        .iter()
        .map(|item| match item {
            Out::Text { text, syntax } => {
                let syntax = syntax
                    .as_deref()
                    .filter(|s| matches!(*s, "json" | "xml" | "html"));
                let pretty;
                let text = match (syntax, serde_json::from_str::<Value>(text)) {
                    (Some("json"), Ok(value)) => {
                        pretty =
                            serde_json::to_string_pretty(&value).unwrap_or_else(|_| text.clone());
                        &pretty
                    }
                    _ => text,
                };
                crate::steps::debug::render_text(redactor, syntax, text)
            }
            Out::Headers {
                title,
                headers,
                sensitive_headers,
            } => crate::steps::debug::render_headers(redactor, title, headers, sensitive_headers),
        })
        .collect();
    eprintln!("{}", redactor.mask(&lines.join("\n")));
}

/// Publishes a passing reply's variables: `vars` into the current frame, then
/// `global_vars` like `set variable … global`. A name in both ends as
/// `global_vars` says, so `vars` skips it: a frame value would shadow the
/// global one, and the shadowing warning would fire on what this reply made.
pub fn publish_vars(world: &mut World, result: &mut DispatchResult) {
    let globals = std::mem::take(&mut result.global_vars);
    for (name, value) in std::mem::take(&mut result.vars) {
        if globals.contains_key(&name) {
            // Never stored, but a value under a sensitive name is still one
            // the reply revealed.
            world.redactor.variable(&name, &value);
        } else {
            world.vars.set(&name, value);
        }
    }
    for (name, value) in globals {
        // Infallible: `set_variable` returns a `Result` for the step table.
        let _ = crate::steps::vars::set_variable(world, &name, &value, true);
    }
}
