//! The `http` and `sql` diagnostics as data, and the one renderer that turns
//! them into the console dump and into the `evidence` event
//! (`docs/events-design.md` 9.3).
//!
//! A structure carries everything masking needs, so the renderer never asks
//! `HttpState` anything: sensitive header names, the form of the request body,
//! and the text itself. Values under sensitive names are hidden by position,
//! then the secret set is applied, and only then is a body cut — a secret that
//! straddles the cut cannot show its start.

use crate::privacy::{Hit, Redactor, mask_ranges};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fmt::Write;
use std::ops::Range;

/// How the request body of an `http` diagnostic is laid out, which decides
/// where its sensitive fields stand.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BodyForm {
    /// `name=value&…`: fields named by `privacy.fields` are hidden.
    Form,
    /// A description of the parts, never their bytes: nothing to hide by name.
    Multipart,
    /// Tried as JSON, whatever the Content-Type.
    #[default]
    Raw,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpSide {
    /// `[name, value]` pairs, order and repeats kept.
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
}

/// One HTTP exchange. Beside the evidence fields, `sensitive_headers`,
/// `request_form`, `resource` and `in_dump` exist only for the renderer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpDiagnostic {
    pub method: String,
    pub url: String,
    pub request: HttpSide,
    pub status: u16,
    pub response: HttpSide,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_us: Option<u64>,
    /// Header names whose values are `***`, compared without case.
    pub sensitive_headers: Vec<String>,
    pub request_form: BodyForm,
    /// The instance the exchange concerns.
    pub resource: String,
    /// False: evidence only, never in the console dump.
    pub in_dump: bool,
}

/// One statement with its binds; `None` is SQL `NULL`. Masked by value only,
/// never by field name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SqlDiagnostic {
    pub sql: String,
    pub binds: Vec<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_us: Option<u64>,
    pub resource: String,
}

/// The text of a body kept whole, or a reference to the file that holds it:
/// `Trace` decides (the 16 KiB rule) because it owns the artifact root.
pub type Place<'a> = &'a dyn Fn(String, &str) -> Value;

/// The 600-character cut of a body in the console dump.
const DUMP_BODY: usize = 600;

pub fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        format!("{}…", s.chars().take(n).collect::<String>())
    } else {
        s.to_string()
    }
}

pub fn ranges(hits: &[Hit]) -> Vec<Range<usize>> {
    hits.iter().map(|hit| hit.range.clone()).collect()
}

/// A header's value as it may be shown: `***` for a header listed as
/// sensitive, whatever the value's length; otherwise the value, masked.
pub fn shown_header(
    sensitive_headers: &[String],
    name: &str,
    value: &str,
    redactor: &Redactor,
) -> String {
    let sensitive = sensitive_headers
        .iter()
        .any(|listed| listed.eq_ignore_ascii_case(name));
    redactor.show(sensitive, value)
}

impl HttpDiagnostic {
    fn header(&self, name: &str, value: &str, redactor: &Redactor) -> String {
        shown_header(&self.sensitive_headers, name, value, redactor)
    }

    /// The URL, the request body and the response body, each with its values
    /// under sensitive names hidden by position and then the set applied.
    /// All three are plain text while the redactor is disabled.
    fn masked_texts(&self, redactor: &Redactor) -> (String, Option<String>, String) {
        let by_position =
            |text: &str, hits: Vec<Hit>| redactor.mask(&mask_ranges(text, &ranges(&hits)));
        let url = by_position(&self.url, redactor.query_hits(&self.url));
        let request = self.request.body.as_deref().map(|body| {
            let hits = match self.request_form {
                BodyForm::Form => redactor.pair_hits(body),
                BodyForm::Raw => redactor.json_hits(body),
                BodyForm::Multipart => Vec::new(),
            };
            by_position(body, hits)
        });
        let body = self.response.body.as_deref().unwrap_or_default();
        let response = by_position(body, redactor.json_hits(body));
        (url, request, response)
    }

    /// The failure dump; `None` for a diagnostic that is evidence only.
    pub fn dump(&self, redactor: &Redactor) -> Option<String> {
        if !self.in_dump {
            return None;
        }
        let (url, request, response) = self.masked_texts(redactor);
        let mut out = format!("  {} {url}\n", self.method);
        for (k, v) in &self.request.headers {
            let _ = writeln!(out, "    {k}: {}", self.header(k, v, redactor));
        }
        if let Some(body) = request {
            let _ = writeln!(out, "    {}", truncate(&body, DUMP_BODY));
        }
        let _ = writeln!(out, "  ← {}", self.status);
        let _ = write!(out, "    {}", truncate(&response, DUMP_BODY));
        Some(out)
    }

    /// The `content` of the `http` evidence event: the evidence fields only.
    /// A body goes through `place`, after masking.
    pub fn content(&self, redactor: &Redactor, place: Place) -> Value {
        let (url, request, response) = self.masked_texts(redactor);
        let body_place = |text: String, name: &str| {
            let extension = if text.len() > crate::events::INLINE_LIMIT
                && serde_json::from_str::<Value>(&text).is_ok()
            {
                "json"
            } else {
                "txt"
            };
            place(text, &format!("{name}.{extension}"))
        };
        let headers = |pairs: &[(String, String)]| -> Vec<Value> {
            pairs
                .iter()
                .map(|(name, value)| json!([name, self.header(name, value, redactor)]))
                .collect()
        };
        let mut content = json!({
            "method": self.method,
            "url": url,
            "request": {
                "headers": headers(&self.request.headers),
                "body": request.map(|text| body_place(text, "request-body")),
            },
            "status": self.status,
            "response": {
                "headers": headers(&self.response.headers),
                "body": body_place(response, "response-body"),
            },
        });
        if let Some(duration) = self.duration_us {
            content["duration_us"] = duration.into();
        }
        content
    }
}

/// What a plugin's `http` diagnostic holds in `content`: the evidence fields
/// and the two the renderer needs. `resource` and `in_dump` stand beside it
/// in the diagnostic and are added by `HttpDiagnostic::from_plugin`.
#[derive(Deserialize)]
struct HttpContent {
    method: String,
    url: String,
    #[serde(default)]
    request: HttpSide,
    status: u16,
    #[serde(default)]
    response: HttpSide,
    #[serde(default)]
    duration_us: Option<u64>,
    #[serde(default)]
    sensitive_headers: Vec<String>,
    #[serde(default)]
    request_form: BodyForm,
}

#[derive(Deserialize)]
struct SqlContent {
    sql: String,
    #[serde(default)]
    binds: Vec<Option<String>>,
    #[serde(default)]
    duration_us: Option<u64>,
}

impl HttpDiagnostic {
    /// A plugin's `content` object. A shape that does not fit is refused with
    /// a message that names the kind and quotes nothing: serde's own text
    /// would quote the offending value.
    pub fn from_plugin(content: &Value, resource: &str, in_dump: bool) -> Result<Self, String> {
        let wire: HttpContent = serde_json::from_value(content.clone())
            .map_err(|_| "the content of an http diagnostic does not fit its shape".to_string())?;
        Ok(Self {
            method: wire.method,
            url: wire.url,
            request: wire.request,
            status: wire.status,
            response: wire.response,
            duration_us: wire.duration_us,
            sensitive_headers: wire.sensitive_headers,
            request_form: wire.request_form,
            resource: resource.to_string(),
            in_dump,
        })
    }

    /// Adds to the set what the exchange holds under sensitive names, as
    /// `HttpState::send` does for a built-in one: the listed headers' values
    /// (whole and in parts), the query's and the request body's fields, and
    /// the response body's JSON fields.
    pub fn register(&self, redactor: &Redactor) {
        let listed = |name: &str| {
            self.sensitive_headers
                .iter()
                .any(|listed| listed.eq_ignore_ascii_case(name))
        };
        for (name, value) in self.request.headers.iter().chain(&self.response.headers) {
            if listed(name) {
                register_header(redactor, name, value);
            }
        }
        redactor.register(&redactor.query_hits(&self.url));
        if let Some(body) = &self.request.body {
            redactor.register(&match self.request_form {
                BodyForm::Form => redactor.pair_hits(body),
                BodyForm::Raw => redactor.json_hits(body),
                BodyForm::Multipart => Vec::new(),
            });
        }
        if let Some(body) = &self.response.body {
            redactor.register(&redactor.json_hits(body));
        }
    }
}

impl SqlDiagnostic {
    /// A plugin's `content` object, refused like `HttpDiagnostic::from_plugin`.
    pub fn from_plugin(content: &Value, resource: &str) -> Result<Self, String> {
        let wire: SqlContent = serde_json::from_value(content.clone())
            .map_err(|_| "the content of an sql diagnostic does not fit its shape".to_string())?;
        Ok(Self {
            sql: wire.sql,
            binds: wire.binds,
            duration_us: wire.duration_us,
            resource: resource.to_string(),
        })
    }
}

/// One `scan` item of a plugin's reply: text the host checks for sensitive
/// field names, registering what it finds and printing nothing.
#[derive(Debug, Clone, Deserialize)]
pub struct ScanItem {
    /// `json`, `form` or `query`.
    pub kind: String,
    /// The text of a `json` item.
    #[serde(default)]
    pub content: Option<String>,
    /// The raw, not encoded, `[name, value]` pairs of a `form` or `query`
    /// item: a value holding `&`, `=`, `+` or `%XX` is registered whole.
    #[serde(default)]
    pub pairs: Vec<(String, String)>,
}

impl ScanItem {
    /// An item of another kind is refused; the message quotes nothing.
    pub fn register(&self, redactor: &Redactor) -> Result<(), String> {
        match self.kind.as_str() {
            "json" => {
                let content = self
                    .content
                    .as_deref()
                    .ok_or("a json scan item has no content")?;
                redactor.register(&redactor.json_hits(content));
            }
            "form" | "query" => {
                for (name, value) in &self.pairs {
                    if redactor.field_is_sensitive(name) {
                        redactor.secret(value);
                    }
                }
            }
            _ => return Err("a scan item is neither json, form nor query".to_string()),
        }
        Ok(())
    }
}

impl SqlDiagnostic {
    /// A statement that ran for `elapsed` over `resource`; a bind holding the
    /// variables' `NULL` sentinel is SQL `NULL`.
    #[cfg(not(feature = "dev-unreserve"))]
    pub fn new(
        resource: &str,
        sql: &str,
        binds: &[Option<String>],
        elapsed: std::time::Duration,
    ) -> Self {
        Self {
            sql: sql.to_string(),
            binds: binds
                .iter()
                .map(|bind| {
                    bind.clone()
                        .filter(|value| value != crate::vars::NULL_SENTINEL)
                })
                .collect(),
            duration_us: Some(crate::events::micros(elapsed)),
            resource: resource.to_string(),
        }
    }

    /// The `content` of the `sql` evidence event.
    pub fn content(&self, redactor: &Redactor) -> Value {
        let binds: Vec<Value> = self
            .binds
            .iter()
            .map(|bind| match bind {
                None => Value::Null,
                Some(value) => redactor.mask(value).into(),
            })
            .collect();
        let mut content = json!({"sql": redactor.mask(&self.sql), "binds": binds});
        if let Some(duration) = self.duration_us {
            content["duration_us"] = duration.into();
        }
        content
    }
}

/// Registers a sensitive header's value whole, and the part of it that travels
/// on its own: the credential of `Authorization: Bearer <credential>`, each
/// value of `Cookie`, the cookie value of `Set-Cookie` (not its attributes).
pub(crate) fn register_header(redactor: &Redactor, name: &str, value: &str) {
    redactor.secret(value);
    let parts: Vec<&str> = match name.to_ascii_lowercase().as_str() {
        "authorization" | "proxy-authorization" => {
            value.split_once(' ').map(|(_, c)| c).into_iter().collect()
        }
        "cookie" => value
            .split(';')
            .filter_map(|pair| pair.split_once('='))
            .map(|(_, v)| v)
            .collect(),
        "set-cookie" => value
            .split(';')
            .next()
            .and_then(|pair| pair.split_once('='))
            .map(|(_, v)| v)
            .into_iter()
            .collect(),
        _ => Vec::new(),
    };
    for part in parts {
        redactor.secret(part.trim());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::privacy::{Privacy, Secrets};

    fn redactor(secrets: &[&str]) -> Redactor {
        let privacy = Privacy::default();
        let mut set = Secrets::new(privacy.min_length);
        for secret in secrets {
            set.add(secret);
        }
        Redactor::new(&privacy, set)
    }

    fn diagnostic() -> HttpDiagnostic {
        HttpDiagnostic {
            method: "POST".into(),
            url: "http://x/login?token=ab&page=2".into(),
            request: HttpSide {
                headers: vec![
                    ("Authorization".into(), "Bearer abc".into()),
                    ("X-Plain".into(), "ok".into()),
                ],
                body: Some(r#"{"password":"p","name":"Acme"}"#.into()),
            },
            status: 200,
            response: HttpSide {
                headers: vec![("set-cookie".into(), "id=1".into())],
                body: Some(r#"{"token":"t","id":42}"#.into()),
            },
            duration_us: Some(7),
            sensitive_headers: vec!["authorization".into(), "set-cookie".into()],
            request_form: BodyForm::Raw,
            resource: "backend".into(),
            in_dump: true,
        }
    }

    fn dump(d: &HttpDiagnostic, redactor: &Redactor) -> String {
        d.dump(redactor).expect("in the dump")
    }

    #[test]
    fn an_evidence_only_diagnostic_is_not_dumped() {
        let mut d = diagnostic();
        d.in_dump = false;
        assert_eq!(d.dump(&redactor(&[])), None);
    }

    #[test]
    fn a_body_over_the_inline_limit_is_placed_in_a_file_named_by_its_kind() {
        let mut d = diagnostic();
        let json = format!(r#"{{"a":"{}"}}"#, "x".repeat(crate::events::INLINE_LIMIT));
        d.response.body = Some(json);
        d.request.body = Some("y".repeat(crate::events::INLINE_LIMIT + 1));
        let named = |text: String, name: &str| json!([name, text.len()]);
        let content = d.content(&redactor(&[]), &named);
        assert_eq!(content["response"]["body"][0], "response-body.json");
        assert_eq!(content["request"]["body"][0], "request-body.txt");
    }

    fn place(text: String, _: &str) -> Value {
        text.into()
    }

    #[test]
    fn the_dump_hides_by_position_from_the_structure_alone() {
        let dump = dump(&diagnostic(), &redactor(&[]));
        assert!(dump.contains("Authorization: ***"), "{dump}");
        assert!(dump.contains("X-Plain: ok"), "{dump}");
        assert!(dump.contains("?token=***&page=2"), "{dump}");
        assert!(
            dump.contains(r#"{"password":"***","name":"Acme"}"#),
            "{dump}"
        );
        assert!(dump.contains(r#"{"token":"***","id":42}"#), "{dump}");
    }

    #[test]
    fn a_form_body_is_masked_by_field_name_and_a_multipart_one_is_not() {
        let mut d = diagnostic();
        d.request.body = Some("password=p&name=Acme".into());
        d.request_form = BodyForm::Form;
        assert!(dump(&d, &redactor(&[])).contains("password=***&name=Acme"));
        d.request_form = BodyForm::Multipart;
        assert!(dump(&d, &redactor(&[])).contains("password=p&name=Acme"));
    }

    #[test]
    fn a_response_body_is_tried_as_json_whatever_the_content_type() {
        let mut d = diagnostic();
        d.response.headers = vec![("content-type".into(), "text/plain".into())];
        assert!(dump(&d, &redactor(&[])).contains(r#"{"token":"***","id":42}"#));
    }

    #[test]
    fn a_secret_is_masked_before_the_cut_so_it_cannot_straddle_it() {
        let secret = "s3cr3t-value";
        let mut d = diagnostic();
        d.response.body = Some(format!("{}{secret}", "x".repeat(595)));
        let dump = dump(&d, &redactor(&[secret]));
        assert!(!dump.contains("s3cr3"), "{dump}");
    }

    #[test]
    fn a_disabled_redactor_shows_everything() {
        let d = diagnostic();
        let dump = dump(&d, &Redactor::disabled());
        assert!(dump.contains("Authorization: Bearer abc"));
        assert!(dump.contains(r#""password":"p""#));
    }

    #[test]
    fn the_evidence_leaves_out_what_only_the_renderer_uses() {
        let content = diagnostic().content(&redactor(&[]), &place);
        let keys: Vec<_> = content
            .as_object()
            .expect("object")
            .keys()
            .cloned()
            .collect();
        for renderer_only in ["sensitive_headers", "request_form", "resource", "in_dump"] {
            assert!(!keys.iter().any(|key| key == renderer_only), "{keys:?}");
        }
        assert_eq!(
            content["request"]["headers"][0],
            json!(["Authorization", "***"])
        );
        assert_eq!(content["duration_us"], 7);
        let mut unmeasured = diagnostic();
        unmeasured.duration_us = None;
        assert!(
            unmeasured
                .content(&redactor(&[]), &place)
                .get("duration_us")
                .is_none()
        );
    }

    #[test]
    #[cfg(not(feature = "dev-unreserve"))]
    fn the_null_sentinel_bind_is_sql_null() {
        let binds = [
            Some(crate::vars::NULL_SENTINEL.to_string()),
            Some("a".into()),
            None,
        ];
        let sql = SqlDiagnostic::new("main", "select 1", &binds, std::time::Duration::ZERO);
        assert_eq!(sql.binds, [None, Some("a".into()), None]);
        assert_eq!(
            sql.content(&Redactor::disabled())["binds"],
            json!([null, "a", null])
        );
    }

    #[test]
    fn sql_is_masked_by_value_only() {
        let sql = SqlDiagnostic {
            sql: "SELECT 1 WHERE password = $1 AND note = $2".into(),
            binds: vec![Some("hunter2hunter2".into()), None],
            duration_us: None,
            resource: "main".into(),
        };
        let content = sql.content(&redactor(&["hunter2hunter2"]));
        assert_eq!(content["binds"], json!(["***", null]));
        assert_eq!(content["sql"], "SELECT 1 WHERE password = $1 AND note = $2");
    }
}
