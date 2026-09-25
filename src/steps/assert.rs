use super::markup::{self, MarkupError};
use crate::http::ReplayError;
use crate::json::{matcher, path};
use crate::polling::{AttemptError, AttemptResult};
use crate::world::World;
use regex::Regex;

pub(super) async fn replay_response(w: &mut World, attempt: u64) -> AttemptResult {
    if attempt == 0 {
        return Ok(());
    }
    w.http.replay_last().await.map_err(|error| match error {
        ReplayError::NotYet(message) => AttemptError::NotYet(message),
        ReplayError::Fatal(message) => AttemptError::Fatal(message),
    })
}

pub(crate) fn last(w: &World) -> Result<&crate::http::Exchange, String> {
    w.http
        .last()
        .ok_or_else(|| "no request has been sent yet".to_string())
}

pub fn response_code(w: &World, expected: &str) -> AttemptResult {
    let want: u16 = expected
        .parse()
        .map_err(|_| AttemptError::Fatal(format!("invalid code {expected:?}")))?;
    let got = last(w).map_err(AttemptError::Fatal)?.status;
    if got == want {
        Ok(())
    } else {
        Err(AttemptError::NotYet(format!(
            "    expected: {want}\n    actual:   {got}"
        )))
    }
}

pub fn response_header(w: &World, name: &str, value: &str) -> AttemptResult {
    let ex = last(w).map_err(AttemptError::Fatal)?;
    let got = ex
        .resp_headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str());
    match got {
        Some(v) if v == value => Ok(()),
        Some(v) => Err(AttemptError::NotYet(format!(
            "    expected: {value}\n    actual:   {v}"
        ))),
        None => Err(AttemptError::NotYet(format!(
            "response header {name:?} is missing"
        ))),
    }
}

/// The doc string of a JSON step, parsed. Shared by the body and the variable steps.
pub(super) fn expected_json(docstring: Option<&String>) -> Result<serde_json::Value, AttemptError> {
    let expected_src = docstring
        .ok_or_else(|| AttemptError::Fatal("step requires a doc string with JSON".to_string()))?;
    serde_json::from_str(expected_src)
        .map_err(|e| AttemptError::Fatal(format!("expected JSON is invalid: {e}")))
}

fn body_json(w: &World) -> Result<serde_json::Value, AttemptError> {
    last(w)
        .map_err(AttemptError::Fatal)?
        .json()
        .map_err(AttemptError::NotYet)
}

pub fn body_contains_json(w: &World, docstring: Option<&String>) -> AttemptResult {
    let expected = expected_json(docstring)?;
    check_contains_json(&body_json(w)?, &expected)
}

pub fn body_equals_json(w: &World, docstring: Option<&String>) -> AttemptResult {
    let expected = expected_json(docstring)?;
    check_equals_json(&body_json(w)?, &expected)
}

pub(super) fn check_contains_json(
    actual: &serde_json::Value,
    expected: &serde_json::Value,
) -> AttemptResult {
    matcher::contains(actual, expected).map_err(|m| AttemptError::NotYet(m.to_string()))
}

pub(super) fn check_equals_json(
    actual: &serde_json::Value,
    expected: &serde_json::Value,
) -> AttemptResult {
    matcher::equals(actual, expected).map_err(|m| AttemptError::NotYet(m.to_string()))
}

pub fn array_length(w: &World, expected: &str) -> AttemptResult {
    let want: usize = expected
        .parse()
        .map_err(|_| AttemptError::Fatal(format!("invalid length {expected:?}")))?;
    let v = last(w)
        .map_err(AttemptError::Fatal)?
        .json()
        .map_err(AttemptError::NotYet)?;
    check_array_length(&v, want)
}

/// Issue #22, item 5: "body is not an array at all" and "array has the wrong length"
/// are distinguishable failures already — kept as two separate messages here rather
/// than a new step.
fn check_array_length(v: &serde_json::Value, want: usize) -> AttemptResult {
    let arr = v
        .as_array()
        .ok_or_else(|| AttemptError::NotYet("response body is not an array".to_string()))?;
    if arr.len() == want {
        Ok(())
    } else {
        Err(AttemptError::NotYet(format!(
            "    expected: array of length {want}\n    actual:   length {}",
            arr.len()
        )))
    }
}

pub fn json_node_exists(w: &World, p: &str) -> AttemptResult {
    path::validate(p).map_err(AttemptError::Fatal)?;
    let v = last(w)
        .map_err(AttemptError::Fatal)?
        .json()
        .map_err(AttemptError::NotYet)?;
    path::read(&v, p).map(|_| ()).map_err(AttemptError::NotYet)
}

pub fn json_node_not_exists(w: &World, p: &str) -> AttemptResult {
    path::validate(p).map_err(AttemptError::Fatal)?;
    let v = last(w)
        .map_err(AttemptError::Fatal)?
        .json()
        .map_err(AttemptError::NotYet)?;
    check_not_exists(&v, p)
}

/// A `null` node exists — this must not treat `null` as absent (issue #22, item 1).
fn check_not_exists(v: &serde_json::Value, p: &str) -> AttemptResult {
    match path::read(v, p) {
        Err(_) => Ok(()),
        Ok(node) => Err(AttemptError::NotYet(format!(
            "    expected: node {p:?} to not exist\n    actual:   {node}"
        ))),
    }
}

pub fn json_node_is_null(w: &World, p: &str) -> AttemptResult {
    path::validate(p).map_err(AttemptError::Fatal)?;
    let v = last(w)
        .map_err(AttemptError::Fatal)?
        .json()
        .map_err(AttemptError::NotYet)?;
    check_is_null(&v, p)
}

pub fn json_node_not_null(w: &World, p: &str) -> AttemptResult {
    path::validate(p).map_err(AttemptError::Fatal)?;
    let v = last(w)
        .map_err(AttemptError::Fatal)?
        .json()
        .map_err(AttemptError::NotYet)?;
    check_not_null(&v, p)
}

/// "does not exist" and "exists but not null" are different bugs (issue #22, item 2) —
/// a missing node is always its own NotYet message, never folded into "not null".
fn check_is_null(v: &serde_json::Value, p: &str) -> AttemptResult {
    match path::read(v, p) {
        Err(_) => Err(AttemptError::NotYet(format!("node {p:?} does not exist"))),
        Ok(node) if node.is_null() => Ok(()),
        Ok(node) => Err(AttemptError::NotYet(format!(
            "    expected: null\n    actual:   {node}"
        ))),
    }
}

fn check_not_null(v: &serde_json::Value, p: &str) -> AttemptResult {
    match path::read(v, p) {
        Err(_) => Err(AttemptError::NotYet(format!("node {p:?} does not exist"))),
        Ok(node) if node.is_null() => Err(AttemptError::NotYet(
            "    expected: not null\n    actual:   null".to_string(),
        )),
        Ok(_) => Ok(()),
    }
}

pub fn body_not_contains_json(w: &World, docstring: Option<&String>) -> AttemptResult {
    let expected = expected_json(docstring)?;
    check_not_contains_json("body", &body_json(w)?, &expected)
}

pub(super) fn check_not_contains_json(
    subject: &str,
    actual: &serde_json::Value,
    expected: &serde_json::Value,
) -> AttemptResult {
    match matcher::contains(actual, expected) {
        Ok(()) => Err(AttemptError::NotYet(format!(
            "    expected: {subject} to NOT contain JSON matching:\n{}\n    actual:   it does",
            serde_json::to_string_pretty(expected).unwrap_or_default()
        ))),
        Err(_) => Ok(()),
    }
}

pub fn json_node_not_contains(w: &World, p: &str, needle: &str) -> AttemptResult {
    path::validate(p).map_err(AttemptError::Fatal)?;
    let v = last(w)
        .map_err(AttemptError::Fatal)?
        .json()
        .map_err(AttemptError::NotYet)?;
    check_not_contains_substring(&v, p, needle)
}

/// A node that isn't a string is a type bug, not a pass — never silently stringified
/// (issue #22, item 4). A missing node has nothing to contain the substring, so it passes.
fn check_not_contains_substring(v: &serde_json::Value, p: &str, needle: &str) -> AttemptResult {
    match path::read(v, p) {
        Err(_) => Ok(()),
        Ok(serde_json::Value::String(s)) if s.contains(needle) => Err(AttemptError::NotYet(
            format!("    node {p:?} should not contain {needle:?}\n    actual:   {s:?}"),
        )),
        Ok(serde_json::Value::String(_)) => Ok(()),
        Ok(other) => Err(AttemptError::Fatal(format!(
            "node {p:?} is not a string, got: {other}"
        ))),
    }
}

pub fn body_empty(w: &World) -> AttemptResult {
    check_empty("body", &last(w).map_err(AttemptError::Fatal)?.body)
}

pub fn body_contains_text(w: &World, needle: &str) -> AttemptResult {
    check_contains(
        "body",
        &last(w).map_err(AttemptError::Fatal)?.body,
        needle,
        false,
    )
}

pub fn body_not_contains_text(w: &World, needle: &str) -> AttemptResult {
    check_contains(
        "body",
        &last(w).map_err(AttemptError::Fatal)?.body,
        needle,
        true,
    )
}

pub fn body_matches(w: &World, pattern: &str) -> AttemptResult {
    check_matches(
        "body",
        &last(w).map_err(AttemptError::Fatal)?.body,
        pattern,
        false,
    )
}

pub fn body_not_matches(w: &World, pattern: &str) -> AttemptResult {
    check_matches(
        "body",
        &last(w).map_err(AttemptError::Fatal)?.body,
        pattern,
        true,
    )
}

/// The raw-text checks, over whichever text `subject` names: the response
/// body or a variable (issue #39).
pub(super) fn check_empty(subject: &str, text: &str) -> AttemptResult {
    if text.trim().is_empty() {
        Ok(())
    } else {
        Err(AttemptError::NotYet(format!(
            "    expected: empty {subject}\n    actual:   {text:?}"
        )))
    }
}

pub(super) fn check_contains(
    subject: &str,
    text: &str,
    needle: &str,
    negate: bool,
) -> AttemptResult {
    if text.contains(needle) != negate {
        return Ok(());
    }
    let not = if negate { "NOT " } else { "" };
    Err(AttemptError::NotYet(format!(
        "    expected {subject} to {not}contain: {needle:?}\n    actual {subject}:\n{text}"
    )))
}

pub(super) fn check_matches(
    subject: &str,
    text: &str,
    pattern: &str,
    negate: bool,
) -> AttemptResult {
    let re = Regex::new(pattern)
        .map_err(|e| AttemptError::Fatal(format!("invalid regex {pattern:?}: {e}")))?;
    if re.is_match(text) != negate {
        return Ok(());
    }
    let not = if negate { "NOT " } else { "" };
    Err(AttemptError::NotYet(format!(
        "    expected {subject} to {not}match: {pattern}\n    actual {subject}:\n{text}"
    )))
}

fn selected_element(w: &World, selector: &str) -> Result<String, AttemptError> {
    let ex = last(w).map_err(AttemptError::Fatal)?;
    let kind = markup::classify(markup::content_type(&ex.resp_headers));
    markup::select(kind, &ex.body, selector, true).map_err(|e| match e {
        MarkupError::NoMatch(msg) => AttemptError::NotYet(msg),
        MarkupError::Fatal(msg) => AttemptError::Fatal(msg),
    })
}

pub fn body_has_element(w: &World, selector: &str) -> AttemptResult {
    selected_element(w, selector)?;
    Ok(())
}

pub fn body_has_element_with_text(w: &World, selector: &str, text: &str) -> AttemptResult {
    let got = selected_element(w, selector)?;
    if got == text {
        Ok(())
    } else {
        Err(AttemptError::NotYet(format!(
            "    expected: {text:?}\n    actual:   {got:?}"
        )))
    }
}

pub fn body_not_has_element(w: &World, selector: &str) -> AttemptResult {
    match selected_element(w, selector) {
        Ok(got) => Err(AttemptError::NotYet(format!(
            "    expected: no element to match {selector:?}\n    actual:   matched {got:?}"
        ))),
        Err(AttemptError::NotYet(_)) => Ok(()),
        Err(fatal) => Err(fatal),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn world_with_response(body: &str, headers: Vec<(String, String)>) -> World {
        let resources = std::collections::HashMap::from([(
            "main".to_string(),
            crate::http::ApiResource::new(
                "http://x.local",
                5,
                Vec::new(),
                crate::options::Options::default(),
            )
            .expect("valid base URL"),
        )]);
        let apis = std::sync::Arc::new(
            crate::http::Apis::new(resources, Some("main".to_string()))
                .expect("default API exists"),
        );
        let mut w = World::new(
            apis,
            std::sync::Arc::new(crate::unique::Generator::new()),
            crate::db::DbHandle::new(None, String::new()),
            None,
            None,
            crate::options::Options::default(),
        );
        w.http.store_test_exchange(crate::http::Exchange {
            method: "GET".to_string(),
            url: "http://x.local/".to_string(),
            req_headers: Vec::new(),
            req_body: None,
            status: 200,
            resp_headers: headers,
            body: body.to_string(),
        });
        w
    }

    #[test]
    fn contains_text_passes_on_a_substring() {
        let w = world_with_response("<h1>Hello</h1>", Vec::new());
        assert_eq!(body_contains_text(&w, "<h1>"), Ok(()));
    }

    #[test]
    fn contains_text_fails_when_absent() {
        let w = world_with_response("<h1>Hello</h1>", Vec::new());
        assert!(matches!(
            body_contains_text(&w, "<h2>"),
            Err(AttemptError::NotYet(_))
        ));
    }

    #[test]
    fn not_contains_text_passes_when_absent() {
        let w = world_with_response("<h1>Hello</h1>", Vec::new());
        assert_eq!(body_not_contains_text(&w, "<h2>"), Ok(()));
    }

    #[test]
    fn not_contains_text_fails_when_present() {
        let w = world_with_response("<h1>Hello</h1>", Vec::new());
        assert!(matches!(
            body_not_contains_text(&w, "<h1>"),
            Err(AttemptError::NotYet(_))
        ));
    }

    #[test]
    fn matches_passes_on_a_regex_hit() {
        let w = world_with_response("<h1>Hello</h1>", Vec::new());
        assert_eq!(body_matches(&w, "<h[0-9]>"), Ok(()));
    }

    #[test]
    fn matches_fails_on_a_regex_miss() {
        let w = world_with_response("<h1>Hello</h1>", Vec::new());
        assert!(matches!(
            body_matches(&w, "<h9>"),
            Err(AttemptError::NotYet(_))
        ));
    }

    #[test]
    fn matches_is_fatal_on_an_invalid_pattern() {
        let w = world_with_response("<h1>Hello</h1>", Vec::new());
        assert!(matches!(body_matches(&w, "["), Err(AttemptError::Fatal(_))));
    }

    #[test]
    fn not_matches_passes_on_a_regex_miss() {
        let w = world_with_response("<h1>Hello</h1>", Vec::new());
        assert_eq!(body_not_matches(&w, "<h9>"), Ok(()));
    }

    #[test]
    fn not_matches_fails_on_a_regex_hit() {
        let w = world_with_response("<h1>Hello</h1>", Vec::new());
        assert!(matches!(
            body_not_matches(&w, "<h[0-9]>"),
            Err(AttemptError::NotYet(_))
        ));
    }

    #[test]
    fn not_matches_is_fatal_on_an_invalid_pattern_not_swallowed_as_a_pass() {
        let w = world_with_response("<h1>Hello</h1>", Vec::new());
        assert!(matches!(
            body_not_matches(&w, "["),
            Err(AttemptError::Fatal(_))
        ));
    }

    #[test]
    fn not_exists_passes_when_the_path_is_missing() {
        assert_eq!(check_not_exists(&json!({"a": 1}), "b"), Ok(()));
    }

    #[test]
    fn not_exists_fails_when_the_path_is_present() {
        assert!(matches!(
            check_not_exists(&json!({"a": 1}), "a"),
            Err(AttemptError::NotYet(_))
        ));
    }

    #[test]
    fn not_exists_treats_a_null_value_as_present() {
        // Issue #22, item 1: null is not absence.
        assert!(matches!(
            check_not_exists(&json!({"a": null}), "a"),
            Err(AttemptError::NotYet(_))
        ));
    }

    #[test]
    fn is_null_passes_on_a_null_node() {
        assert_eq!(check_is_null(&json!({"a": null}), "a"), Ok(()));
    }

    #[test]
    fn is_null_fails_on_a_non_null_node() {
        assert!(matches!(
            check_is_null(&json!({"a": 1}), "a"),
            Err(AttemptError::NotYet(_))
        ));
    }

    #[test]
    fn is_null_fails_when_the_node_is_missing() {
        assert!(matches!(
            check_is_null(&json!({"a": 1}), "b"),
            Err(AttemptError::NotYet(_))
        ));
    }

    #[test]
    fn not_null_passes_on_a_non_null_node() {
        assert_eq!(check_not_null(&json!({"a": 1}), "a"), Ok(()));
    }

    #[test]
    fn not_null_fails_on_a_null_node() {
        assert!(matches!(
            check_not_null(&json!({"a": null}), "a"),
            Err(AttemptError::NotYet(_))
        ));
    }

    #[test]
    fn not_null_fails_when_the_node_is_missing() {
        // Existence and non-null are different assertions (issue #22, item 2).
        assert!(matches!(
            check_not_null(&json!({"a": 1}), "b"),
            Err(AttemptError::NotYet(_))
        ));
    }

    #[test]
    fn not_contains_json_passes_when_the_shape_is_absent() {
        assert_eq!(
            check_not_contains_json("body", &json!({"a": 1}), &json!({"b": 2})),
            Ok(())
        );
    }

    #[test]
    fn not_contains_json_fails_when_the_shape_matches() {
        assert!(matches!(
            check_not_contains_json("body", &json!({"a": 1, "b": 2}), &json!({"a": 1})),
            Err(AttemptError::NotYet(_))
        ));
    }

    #[test]
    fn not_contains_substring_passes_when_the_node_is_missing() {
        assert_eq!(
            check_not_contains_substring(&json!({"a": "hello"}), "b", "ell"),
            Ok(())
        );
    }

    #[test]
    fn not_contains_substring_passes_when_absent() {
        assert_eq!(
            check_not_contains_substring(&json!({"a": "hello"}), "a", "xyz"),
            Ok(())
        );
    }

    #[test]
    fn not_contains_substring_fails_when_present() {
        assert!(matches!(
            check_not_contains_substring(&json!({"a": "hello"}), "a", "ell"),
            Err(AttemptError::NotYet(_))
        ));
    }

    #[test]
    fn array_length_on_a_non_array_body_reports_missing_rather_than_a_length_mismatch() {
        // Issue #22, item 5: "not an array" and "wrong length" must stay distinguishable.
        assert_eq!(
            check_array_length(&json!({"a": 1}), 2),
            Err(AttemptError::NotYet(
                "response body is not an array".to_string()
            ))
        );
    }

    #[test]
    fn array_length_on_an_array_of_the_wrong_length_reports_the_mismatch() {
        let err = check_array_length(&json!([1, 2]), 3).unwrap_err();
        assert_ne!(
            err,
            AttemptError::NotYet("response body is not an array".to_string())
        );
    }

    #[test]
    fn not_contains_substring_errors_on_a_non_string_node() {
        assert!(matches!(
            check_not_contains_substring(&json!({"a": 1}), "a", "1"),
            Err(AttemptError::Fatal(_))
        ));
    }

    #[test]
    fn has_element_passes_on_html() {
        let headers = vec![("content-type".to_string(), "text/html".to_string())];
        let w = world_with_response(
            r#"<html><body><h1 id="title">Hi</h1></body></html>"#,
            headers,
        );
        assert_eq!(body_has_element(&w, "h1#title"), Ok(()));
    }

    #[test]
    fn has_element_passes_on_xml() {
        let headers = vec![("content-type".to_string(), "application/xml".to_string())];
        let w = world_with_response(r#"<root><name>Acme</name></root>"#, headers);
        assert_eq!(body_has_element(&w, "//name"), Ok(()));
    }

    #[test]
    fn has_element_fails_when_no_match() {
        let headers = vec![("content-type".to_string(), "text/html".to_string())];
        let w = world_with_response(r#"<html><body></body></html>"#, headers);
        assert!(matches!(
            body_has_element(&w, ".missing"),
            Err(AttemptError::NotYet(_))
        ));
    }

    #[test]
    fn has_element_is_fatal_on_json_content_type() {
        let headers = vec![("content-type".to_string(), "application/json".to_string())];
        let w = world_with_response(r#"{"a": 1}"#, headers);
        assert!(matches!(
            body_has_element(&w, "a"),
            Err(AttemptError::Fatal(_))
        ));
    }

    #[test]
    fn has_element_with_text_passes_on_exact_match() {
        let headers = vec![("content-type".to_string(), "text/html".to_string())];
        let w = world_with_response(
            r#"<html><body><h1 id="title">Hi</h1></body></html>"#,
            headers,
        );
        assert_eq!(body_has_element_with_text(&w, "h1#title", "Hi"), Ok(()));
    }

    #[test]
    fn has_element_with_text_fails_on_mismatch() {
        let headers = vec![("content-type".to_string(), "text/html".to_string())];
        let w = world_with_response(
            r#"<html><body><h1 id="title">Hi</h1></body></html>"#,
            headers,
        );
        assert!(matches!(
            body_has_element_with_text(&w, "h1#title", "Bye"),
            Err(AttemptError::NotYet(_))
        ));
    }

    #[test]
    fn not_has_element_passes_when_absent() {
        let headers = vec![("content-type".to_string(), "text/html".to_string())];
        let w = world_with_response(r#"<html><body></body></html>"#, headers);
        assert_eq!(body_not_has_element(&w, ".missing"), Ok(()));
    }

    #[test]
    fn not_has_element_fails_when_present() {
        let headers = vec![("content-type".to_string(), "text/html".to_string())];
        let w = world_with_response(
            r#"<html><body><h1 id="title">Hi</h1></body></html>"#,
            headers,
        );
        assert!(matches!(
            body_not_has_element(&w, "h1#title"),
            Err(AttemptError::NotYet(_))
        ));
    }

    /// Negation must not swallow a config-level error: "wrong content-type"
    /// is a failure to evaluate, not a successful absence.
    #[test]
    fn not_has_element_stays_fatal_on_json_content_type() {
        let headers = vec![("content-type".to_string(), "application/json".to_string())];
        let w = world_with_response(r#"{"a": 1}"#, headers);
        assert!(matches!(
            body_not_has_element(&w, "a"),
            Err(AttemptError::Fatal(_))
        ));
    }

    #[test]
    fn has_element_does_not_silently_pass_on_a_false_boolean_xpath() {
        let headers = vec![("content-type".to_string(), "application/xml".to_string())];
        let w = world_with_response(r#"<root><name>Acme</name></root>"#, headers);
        assert!(matches!(
            body_has_element(&w, "//missing = 'x'"),
            Err(AttemptError::Fatal(_))
        ));
    }

    #[test]
    fn has_element_with_text_fails_on_whitespace_only_difference_and_the_message_shows_it() {
        let headers = vec![("content-type".to_string(), "text/html".to_string())];
        let indented = "<html><body>\n  <div class=\"box\">\n    Loose\n  </div>\n</body></html>";
        let w = world_with_response(indented, headers);
        let err = body_has_element_with_text(&w, "div.box", "Loose").unwrap_err();
        let AttemptError::NotYet(msg) = err else {
            panic!("expected NotYet")
        };
        // Debug-quoted so the whitespace difference is visible, not silently swallowed.
        assert!(
            msg.contains("\\n"),
            "message should show escaped whitespace: {msg}"
        );
    }
}
