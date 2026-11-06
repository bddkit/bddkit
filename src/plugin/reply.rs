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
use crate::privacy::Redactor;
use serde_json::Value;
use std::path::Path;

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
