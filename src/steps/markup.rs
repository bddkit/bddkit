//! Body-kind classification and structured element selection over the raw
//! HTTP response body: XPath for XML (`sxd-document`/`sxd-xpath`), CSS
//! selectors for HTML (`scraper`, html5ever-backed — tolerant of real-world
//! tag soup, unlike a strict XML parser; see Task 2).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BodyKind {
    Json,
    Xml,
    Html,
    Plain,
}

impl BodyKind {
    fn describe(&self) -> &'static str {
        match self {
            Self::Json => "JSON",
            Self::Xml => "XML",
            Self::Html => "HTML",
            Self::Plain => "plain text",
        }
    }
}

/// Substring order matters: `application/xhtml+xml` contains both `html` and
/// `xml` — it must land in `Html`, so `html` is checked before `xml`.
pub(crate) fn classify(content_type: Option<&str>) -> BodyKind {
    let ct = content_type.unwrap_or("").to_ascii_lowercase();
    if ct.contains("json") {
        BodyKind::Json
    } else if ct.contains("html") {
        BodyKind::Html
    } else if ct.contains("xml") {
        BodyKind::Xml
    } else {
        BodyKind::Plain
    }
}

pub(crate) fn content_type(headers: &[(String, String)]) -> Option<&str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v.as_str())
}

/// `Fatal` covers a wrong content-type and a malformed selector/expression —
/// neither gets better by retrying. `NoMatch` is a well-formed selector that
/// simply hasn't matched yet, so an eventual assertion can poll for an
/// element that hasn't appeared in the response yet.
pub(crate) enum MarkupError {
    Fatal(String),
    NoMatch(String),
}

impl MarkupError {
    fn message(&self) -> &str {
        match self {
            Self::Fatal(m) | Self::NoMatch(m) => m,
        }
    }
}

impl std::fmt::Display for MarkupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

fn xpath_select(xml: &str, expr: &str, require_nodeset: bool) -> Result<String, MarkupError> {
    let package = sxd_document::parser::parse(xml)
        .map_err(|e| MarkupError::Fatal(format!("failed to parse XML/HTML: {e}")))?;
    let document = package.as_document();
    let xpath = sxd_xpath::Factory::new()
        .build(expr)
        .map_err(|e| MarkupError::Fatal(format!("invalid XPath {expr:?}: {e}")))?
        .ok_or_else(|| MarkupError::Fatal(format!("empty XPath expression {expr:?}")))?;
    let context = sxd_xpath::Context::new();
    let value = xpath
        .evaluate(&context, document.root())
        .map_err(|e| MarkupError::Fatal(format!("failed to evaluate XPath {expr:?}: {e}")))?;
    match value {
        sxd_xpath::Value::Nodeset(nodes) => {
            let ordered = nodes.document_order();
            if ordered.is_empty() {
                return Err(MarkupError::NoMatch(format!(
                    "XPath {expr:?} found no nodes"
                )));
            }
            Ok(ordered
                .iter()
                .map(|n| n.string_value())
                .collect::<Vec<_>>()
                .join("\n"))
        }
        _other if require_nodeset => Err(MarkupError::Fatal(format!(
            "XPath {expr:?} is not a node-set expression — element steps need one; use `extract` for a computed value"
        ))),
        other => Ok(other.string()),
    }
}

fn css_select(html: &str, selector: &str) -> Result<String, MarkupError> {
    let parsed = scraper::Selector::parse(selector)
        .map_err(|e| MarkupError::Fatal(format!("invalid CSS selector {selector:?}: {e:?}")))?;
    let document = scraper::Html::parse_document(html);
    let matches: Vec<String> = document
        .select(&parsed)
        .map(|el| el.text().collect::<String>())
        .collect();
    if matches.is_empty() {
        return Err(MarkupError::NoMatch(format!(
            "CSS selector {selector:?} matched no elements"
        )));
    }
    Ok(matches.join("\n"))
}

/// Dispatches to the engine matching the response's content type. JSON and
/// unclassified bodies have no element structure to query — refused rather
/// than degraded into a guess.
pub(crate) fn select(
    kind: BodyKind,
    body: &str,
    selector: &str,
    require_nodeset: bool,
) -> Result<String, MarkupError> {
    match kind {
        BodyKind::Xml => xpath_select(body, selector, require_nodeset),
        BodyKind::Html => css_select(body, selector),
        BodyKind::Json | BodyKind::Plain => Err(MarkupError::Fatal(format!(
            "element selectors need HTML or XML content, got {}",
            kind.describe()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_json() {
        assert_eq!(
            classify(Some("application/json; charset=utf-8")),
            BodyKind::Json
        );
    }

    #[test]
    fn classifies_xml() {
        assert_eq!(classify(Some("application/xml")), BodyKind::Xml);
    }

    #[test]
    fn classifies_html() {
        assert_eq!(classify(Some("text/html; charset=utf-8")), BodyKind::Html);
    }

    #[test]
    fn xhtml_xml_is_classified_as_html_not_xml() {
        assert_eq!(classify(Some("application/xhtml+xml")), BodyKind::Html);
    }

    #[test]
    fn unrecognized_content_type_is_plain() {
        assert_eq!(classify(Some("application/octet-stream")), BodyKind::Plain);
    }

    #[test]
    fn missing_content_type_is_plain() {
        assert_eq!(classify(None), BodyKind::Plain);
    }

    #[test]
    fn content_type_reads_the_header_case_insensitively() {
        let headers = vec![("Content-Type".to_string(), "application/json".to_string())];
        assert_eq!(content_type(&headers), Some("application/json"));
    }

    #[test]
    fn content_type_is_none_when_absent() {
        let headers = vec![("x-trace".to_string(), "1".to_string())];
        assert_eq!(content_type(&headers), None);
    }

    #[test]
    fn describe_reads_naturally_not_as_a_rust_variant_name() {
        assert_eq!(BodyKind::Json.describe(), "JSON");
        assert_eq!(BodyKind::Plain.describe(), "plain text");
    }

    fn users_xml() -> &'static str {
        r#"<users><user id="1"><email>a@b.net</email></user><user id="2"><email>c@d.net</email></user></users>"#
    }

    fn xpath(input: &str, expr: &str) -> Result<String, String> {
        xpath_select(input, expr, true).map_err(|e| e.to_string())
    }

    #[test]
    fn xpath_select_reads_element_text() {
        assert_eq!(
            xpath(users_xml(), "//user[@id='2']/email").unwrap(),
            "c@d.net"
        );
    }

    #[test]
    fn xpath_select_reads_an_attribute() {
        assert_eq!(xpath(users_xml(), "//user[1]/@id").unwrap(), "1");
    }

    #[test]
    fn xpath_select_joins_multiple_matches_with_newlines() {
        assert_eq!(
            xpath(users_xml(), "//user/email").unwrap(),
            "a@b.net\nc@d.net"
        );
    }

    #[test]
    fn xpath_select_with_no_matches_is_a_retryable_no_match() {
        assert!(matches!(
            xpath_select(users_xml(), "//missing", true),
            Err(MarkupError::NoMatch(_))
        ));
    }

    #[test]
    fn xpath_select_on_malformed_xml_is_fatal() {
        let err = xpath_select("<not-closed>", "/x", true).unwrap_err();
        assert!(
            matches!(err, MarkupError::Fatal(ref m) if m.contains("failed to parse")),
            "{err}"
        );
    }

    #[test]
    fn xpath_select_with_an_invalid_expression_is_fatal() {
        assert!(matches!(
            xpath_select(users_xml(), "///[[[", true),
            Err(MarkupError::Fatal(_))
        ));
    }

    #[test]
    fn xpath_select_permits_a_boolean_result_when_nodeset_not_required() {
        assert_eq!(
            xpath_select(users_xml(), "//user/@id = '99'", false)
                .map_err(|e| e.to_string())
                .unwrap(),
            "false"
        );
    }

    #[test]
    fn xpath_select_refuses_a_boolean_result_when_nodeset_required() {
        assert!(matches!(
            xpath_select(users_xml(), "//user/@id = '99'", true),
            Err(MarkupError::Fatal(_))
        ));
    }

    #[test]
    fn xpath_select_refuses_a_count_result_when_nodeset_required() {
        assert!(matches!(
            xpath_select(users_xml(), "count(//missing)", true),
            Err(MarkupError::Fatal(_))
        ));
    }

    fn css(input: &str, selector: &str) -> Result<String, String> {
        css_select(input, selector).map_err(|e| e.to_string())
    }

    #[test]
    fn css_select_reads_element_text() {
        let html = r#"<html><body><h1 id="title">Hello</h1></body></html>"#;
        assert_eq!(css(html, "h1#title").unwrap(), "Hello");
    }

    #[test]
    fn css_select_joins_multiple_matches_with_newlines() {
        let html = r#"<ul><li>one</li><li>two</li></ul>"#;
        assert_eq!(css(html, "li").unwrap(), "one\ntwo");
    }

    #[test]
    fn css_select_with_no_matches_is_a_retryable_no_match() {
        let html = r#"<html><body></body></html>"#;
        assert!(matches!(
            css_select(html, ".missing"),
            Err(MarkupError::NoMatch(_))
        ));
    }

    #[test]
    fn css_select_with_an_invalid_selector_is_fatal() {
        let html = r#"<html><body></body></html>"#;
        assert!(matches!(
            css_select(html, ":::"),
            Err(MarkupError::Fatal(_))
        ));
    }

    /// Pins the finding from the design spike: `sxd-document` (the XML
    /// engine) fails on this exact shape, `scraper` must not.
    #[test]
    fn css_select_tolerates_real_world_tag_soup() {
        let messy = r#"<html><head><meta charset="utf-8"><link rel="stylesheet" href="a.css"></head>
<body><img src="a.png"><br><input type="text" value="x"><div class=box id=main>hi <b>there</b></div></body></html>"#;
        assert_eq!(css(messy, "div.box").unwrap(), "hi there");
    }

    #[test]
    fn css_select_tolerates_doctype_and_no_self_closing_void_elements() {
        let real = "<!doctype html>\n<html lang=\"en\">\n  <head><title>Post 1</title></head>\n  <body><h1 id=\"title\">sunt aut facere repellat provident</h1></body>\n</html>\n";
        assert_eq!(
            css(real, "h1#title").unwrap(),
            "sunt aut facere repellat provident"
        );
    }
}
