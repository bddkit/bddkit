use crate::world::World;

use super::markup::{self, BodyKind};

fn headers_table(headers: &[(String, String)]) -> String {
    let width = headers
        .iter()
        .map(|(k, _)| k.chars().count())
        .max()
        .unwrap_or(0);
    headers
        .iter()
        .map(|(k, v)| format!("{k:width$} : {v}"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn print_headers(w: &World) -> Result<(), String> {
    let ex = super::assert::last(w)?;
    eprintln!(
        "=== Response headers ===\n{}",
        headers_table(&ex.resp_headers)
    );
    Ok(())
}

/// Naive tag-by-tag line breaking for XML/HTML with nesting-based indentation.
/// It does not really parse the syntax (a text node containing `<` would
/// distort the result) — good enough for debug printing; a real
/// parse is only needed for structured selection (see `markup::select`).
fn beautify_markup(raw: &str) -> String {
    let mut out = String::new();
    let mut depth: usize = 0;
    for chunk in raw.split('<').filter(|s| !s.is_empty()) {
        let Some(gt) = chunk.find('>') else {
            out.push_str(&"  ".repeat(depth));
            out.push('<');
            out.push_str(chunk);
            out.push('\n');
            continue;
        };
        let (tag_body, text) = chunk.split_at(gt + 1);
        let tag = format!("<{tag_body}");
        let is_closing = tag.starts_with("</");
        let is_self_closing = tag.ends_with("/>") || tag.starts_with("<?") || tag.starts_with("<!");
        if is_closing && depth > 0 {
            depth -= 1;
        }
        out.push_str(&"  ".repeat(depth));
        out.push_str(&tag);
        out.push('\n');
        if !is_closing && !is_self_closing {
            depth += 1;
        }
        let text = text.trim();
        if !text.is_empty() {
            out.push_str(&"  ".repeat(depth));
            out.push_str(text);
            out.push('\n');
        }
    }
    out
}

static SYNTAX_SET: std::sync::LazyLock<syntect::parsing::SyntaxSet> =
    std::sync::LazyLock::new(syntect::parsing::SyntaxSet::load_defaults_newlines);
static THEME_SET: std::sync::LazyLock<syntect::highlighting::ThemeSet> =
    std::sync::LazyLock::new(syntect::highlighting::ThemeSet::load_defaults);

/// Syntax highlighting via ANSI codes. Only enabled when stderr is a terminal;
/// when redirected to a file/log (CI), escape codes are unwanted — they'd corrupt the output.
fn highlight(code: &str, extension: &str) -> String {
    use std::io::IsTerminal;
    highlight_inner(code, extension, std::io::stderr().is_terminal())
}

fn highlight_inner(code: &str, extension: &str, colorize: bool) -> String {
    if !colorize {
        return code.to_string();
    }
    let syntax = SYNTAX_SET
        .find_syntax_by_extension(extension)
        .unwrap_or_else(|| SYNTAX_SET.find_syntax_plain_text());
    let theme = &THEME_SET.themes["base16-ocean.dark"];
    let mut h = syntect::easy::HighlightLines::new(syntax, theme);
    let mut out = String::new();
    for line in syntect::util::LinesWithEndings::from(code) {
        match h.highlight_line(line, &SYNTAX_SET) {
            Ok(ranges) => out.push_str(&syntect::util::as_24_bit_terminal_escaped(
                &ranges[..],
                false,
            )),
            Err(_) => out.push_str(line),
        }
    }
    out.push_str("\x1b[0m");
    out
}

pub fn print_body(w: &World) -> Result<(), String> {
    let ex = super::assert::last(w)?;
    match markup::classify(markup::content_type(&ex.resp_headers)) {
        BodyKind::Json => {
            let v = ex.json()?;
            let pretty = serde_json::to_string_pretty(&v)
                .map_err(|e| format!("failed to serialize JSON: {e}"))?;
            eprintln!("{}", highlight(&pretty, "json"));
        }
        BodyKind::Xml => eprintln!("{}", highlight(&beautify_markup(&ex.body), "xml")),
        BodyKind::Html => eprintln!("{}", highlight(&beautify_markup(&ex.body), "html")),
        BodyKind::Plain => eprintln!("{}", ex.body),
    }
    Ok(())
}

pub fn print_body_as(w: &World, path: &str) -> Result<(), String> {
    let ex = super::assert::last(w)?;
    let kind = markup::classify(markup::content_type(&ex.resp_headers));
    match kind {
        BodyKind::Json => {
            let root = ex.json()?;
            let sub = crate::json::path::read(&root, path)?;
            let pretty = serde_json::to_string_pretty(sub)
                .map_err(|e| format!("failed to serialize JSON: {e}"))?;
            eprintln!("{}", highlight(&pretty, "json"));
        }
        BodyKind::Xml | BodyKind::Html => {
            eprintln!(
                "{}",
                markup::select(kind, &ex.body, path, false).map_err(|e| e.to_string())?
            );
        }
        BodyKind::Plain => {
            return Err(
                "path selection is not supported: content-type is not JSON/XML/HTML".to_string(),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers_table_aligns_names_to_the_longest_one() {
        let headers = vec![
            ("x-trace".to_string(), "abc".to_string()),
            ("content-type".to_string(), "application/json".to_string()),
        ];
        let table = headers_table(&headers);
        assert_eq!(table, "x-trace      : abc\ncontent-type : application/json");
    }

    #[test]
    fn headers_table_of_no_headers_is_empty() {
        assert_eq!(headers_table(&[]), "");
    }

    #[test]
    fn beautify_markup_indents_nested_elements_and_text() {
        let out = beautify_markup("<root><name>Acme</name></root>");
        assert_eq!(out, "<root>\n  <name>\n    Acme\n  </name>\n</root>\n");
    }

    #[test]
    fn beautify_markup_handles_self_closing_and_attributes() {
        let out = beautify_markup(r#"<root><br/><img src="x.png"/></root>"#);
        assert_eq!(out, "<root>\n  <br/>\n  <img src=\"x.png\"/>\n</root>\n");
    }

    #[test]
    fn highlight_wraps_code_in_ansi_escapes_when_colorize_is_on() {
        let out = highlight_inner("{\"a\": 1}", "json", true);
        assert!(
            out.contains('\u{1b}'),
            "highlighted output should contain an ANSI escape: {out:?}"
        );
        assert!(
            out.len() > "{\"a\": 1}".len(),
            "highlighting should add escape bytes"
        );
    }

    #[test]
    fn highlight_falls_back_to_plain_text_for_an_unknown_extension() {
        // Does not panic or fail — simply no highlighting.
        let out = highlight_inner("hello", "not-a-real-extension", true);
        assert!(out.contains("hello"));
    }

    #[test]
    fn highlight_returns_code_unmodified_when_colorize_is_off() {
        let out = highlight_inner("{\"a\": 1}", "json", false);
        assert_eq!(out, "{\"a\": 1}");
    }
}
