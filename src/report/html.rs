//! The model written as one self-contained HTML page.

use super::embed::Embeds;
use super::model::Run;
use serde::Serialize;
use std::path::{Component, Path};

const VIEWER_JS: &str = include_str!("viewer.js");
const VIEWER_CSS: &str = include_str!("viewer.css");

/// No network: inline script and style run, images load from beside the file
/// and from `blob:`, nothing else loads. `file:` is for Firefox, which treats
/// each file on disk as its own origin.
const CSP: &str = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src 'self' file: blob:";

/// What the viewer reads from `<script id="run">`.
#[derive(Serialize)]
struct Page<'a> {
    run: &'a Run,
    /// Prefix that turns a root-relative artifact path into a link from this page.
    artifacts_base: &'a str,
    /// The root is `<temp>/bddkit-<run id>`, which a cleanup will remove.
    artifacts_temporary: bool,
    #[serde(flatten)]
    embeds: &'a Embeds,
}

/// Every `<` becomes `<`: the JSON stays valid and no `</script>` or
/// `<!--` can occur inside the element.
fn embed(page: &Page<'_>) -> String {
    serde_json::to_string(page)
        .expect("the model is plain data")
        .replace('<', "\\u003c")
}

pub fn render(
    run: &Run,
    artifacts_base: &str,
    artifacts_temporary: bool,
    embeds: &Embeds,
) -> String {
    let json = embed(&Page {
        run,
        artifacts_base,
        artifacts_temporary,
        embeds,
    });
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <meta http-equiv=\"Content-Security-Policy\" content=\"{CSP}\">\n\
         <title>bddkit run {id}</title>\n<style>\n{VIEWER_CSS}</style>\n</head>\n<body>\n\
         <div id=\"app\">\n<a class=\"brand\" href=\"{SITE}\" rel=\"noopener noreferrer\">{LOGO}<span>bddkit</span></a>\n</div>\n\
         <footer>\n{footer}</footer>\n\
         <script type=\"application/json\" id=\"run\">{json}</script>\n\
         <script>\n{VIEWER_JS}</script>\n</body>\n</html>\n",
        id = escape_text(&run.run_id),
        footer = footer(run),
    )
}

/// `docs/logo.svg` with its two colours as classes, so the page's theme can
/// pick a teal that reads on a dark background. Inline: `data:` images are
/// not in the CSP.
const LOGO: &str = r#"<svg class="logo" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 128 128" width="28" height="28" role="img" aria-label="bddkit"><g class="logo-a"><path d="M14 14h45v13H27v32H14z"/><path d="M69 14h45v45h-13V27H69z"/><path d="M14 69h13v32h32v13H14z"/></g><rect class="logo-b" x="40" y="40" width="48" height="48"/><path class="logo-a-stroke" d="M74 99l15 15 30-30" fill="none" stroke-width="13" stroke-miterlimit="4"/></svg>"#;

const SITE: &str = "https://bddkit.dev";
const REPO: &str = "https://github.com/bddkit/bddkit";

/// Where the report comes from and where to take a question. Versions only,
/// no clock and no paths, so the page stays the same bytes for the same stream.
fn footer(run: &Run) -> String {
    let renderer = env!("CARGO_PKG_VERSION");
    let versions = if run.bddkit.is_empty() || run.bddkit == renderer {
        format!("bddkit {renderer}")
    } else {
        format!(
            "report by bddkit {renderer}, run by bddkit {}",
            escape_text(&run.bddkit)
        )
    };
    let link =
        |url: &str, text: &str| format!("<a href=\"{url}\" rel=\"noopener noreferrer\">{text}</a>");
    format!(
        "<span>{versions}</span> · {} · {} · {} · {}\n",
        link(SITE, "bddkit.dev"),
        link(REPO, "GitHub"),
        link(&format!("{REPO}/issues"), "Report an issue"),
        link(&format!("{REPO}/blob/main/LICENSE"), "Apache-2.0"),
    )
}

fn escape_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Whether the root is the default `<temp>/bddkit-<run id>`. Decided by name,
/// not by asking this machine for its temporary directory: the report may be
/// rendered somewhere else than the run.
pub fn is_temporary_root(root: &str, run_id: &str) -> bool {
    let name = format!("bddkit-{run_id}");
    Path::new(root.trim_end_matches(['/', '\\']))
        .file_name()
        .is_some_and(|last| last == name.as_str())
        && !run_id.is_empty()
}

fn encode(segment: &str) -> String {
    let mut out = String::new();
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The prefix of a link from a page in `page_dir` to a file under `root`:
/// relative where one exists, `file://` and absolute where it does not
/// (another drive on Windows). Segments are percent-encoded, and a non-empty
/// result ends in `/`. Lexical on purpose: it resolves no symlink, so it
/// agrees with how the run printed the root.
pub fn artifacts_base(root: &Path, page_dir: &Path) -> String {
    let root = std::path::absolute(root).unwrap_or_else(|_| root.to_path_buf());
    let page = std::path::absolute(page_dir).unwrap_or_else(|_| page_dir.to_path_buf());
    let root_parts: Vec<Component<'_>> = root.components().collect();
    let page_parts: Vec<Component<'_>> = page.components().collect();
    let common = root_parts
        .iter()
        .zip(&page_parts)
        .take_while(|(a, b)| a == b)
        .count();
    let anchored = |parts: &[Component<'_>]| {
        parts
            .first()
            .is_some_and(|c| matches!(c, Component::Prefix(_) | Component::RootDir))
    };
    if common == 0 && anchored(&root_parts) {
        let mut out = String::from("file://");
        for part in &root_parts {
            match part {
                Component::Prefix(prefix) => {
                    out.push('/');
                    out.push_str(&prefix.as_os_str().to_string_lossy());
                }
                Component::Normal(name) => {
                    out.push('/');
                    out.push_str(&encode(&name.to_string_lossy()));
                }
                _ => {}
            }
        }
        out.push('/');
        return out;
    }
    let mut out = String::new();
    for part in &page_parts[common..] {
        if matches!(part, Component::Normal(_)) {
            out.push_str("../");
        }
    }
    for part in &root_parts[common..] {
        if let Component::Normal(name) = part {
            out.push_str(&encode(&name.to_string_lossy()));
            out.push('/');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::model;

    fn run_with(text: &str) -> Run {
        let started =
            r#"{"type":"run_started","run_id":"r1","bddkit":"0.3.1","artifacts_dir":"/a/b"}"#;
        let events = format!(
            "{started}\n{}\n{}\n{}\n",
            r#"{"type":"file_started","file":"a.feature","name":"A","tags":[]}"#,
            r#"{"type":"scenario_started","file":"a.feature","scenario":0,"name":"s","line":1,"tags":[]}"#,
            serde_json::json!({"type":"step_started","file":"a.feature","scenario":0,"step":0,
                "keyword":"Given ","text":text,"line":2}),
        );
        model::build(events.as_bytes()).expect("a stream")
    }

    fn embedded(html: &str) -> serde_json::Value {
        let open = "<script type=\"application/json\" id=\"run\">";
        let start = html.find(open).expect("element") + open.len();
        let end = start + html[start..].find("</script>").expect("closed");
        serde_json::from_str(&html[start..end]).expect("valid JSON")
    }

    #[test]
    fn a_closing_script_tag_in_a_step_cannot_end_the_element() {
        let html = render(
            &run_with("</script><script>alert(1)</script><!--"),
            "",
            false,
            &Embeds::default(),
        );
        let data = embedded(&html);
        let step = &data["run"]["files"][0]["scenarios"][0]["items"][0];
        assert_eq!(step["text"], "</script><script>alert(1)</script><!--");
        assert_eq!(html.matches("</script>").count(), 2);
    }

    #[test]
    fn the_embedded_json_is_the_model_and_the_base() {
        let run = run_with("x");
        let data = embedded(&render(&run, "../art/", true, &Embeds::default()));
        assert_eq!(data["run"], serde_json::to_value(&run).expect("json"));
        assert_eq!(data["artifacts_base"], "../art/");
        assert_eq!(data["artifacts_temporary"], true);
    }

    #[test]
    fn the_same_run_gives_the_same_bytes() {
        let run = run_with("x");
        assert_eq!(
            render(&run, "../art/", false, &Embeds::default()),
            render(&run, "../art/", false, &Embeds::default())
        );
    }

    #[test]
    fn the_footer_names_the_project_and_the_logo_is_inline() {
        let html = render(&run_with("x"), "", false, &Embeds::default());
        for needle in [
            "https://bddkit.dev",
            "https://github.com/bddkit/bddkit",
            "Apache-2.0",
            "<svg class=\"logo\"",
        ] {
            assert!(html.contains(needle), "{needle}");
        }
    }

    #[test]
    fn the_page_forbids_the_network() {
        assert!(
            render(&run_with("x"), "", false, &Embeds::default()).contains("default-src 'none'")
        );
    }

    #[test]
    fn links_are_relative_to_the_pages_directory() {
        let base = |root, page| artifacts_base(Path::new(root), Path::new(page));
        assert_eq!(base("/out/artifacts", "/out"), "artifacts/");
        assert_eq!(base("/out/artifacts", "/out/reports"), "../artifacts/");
        assert_eq!(base("/out", "/out"), "");
        assert_eq!(base("/x/my dir#1", "/x"), "my%20dir%231/");
    }

    #[test]
    fn the_default_temporary_root_is_recognised_by_name() {
        assert!(is_temporary_root("/tmp/bddkit-r1", "r1"));
        assert!(!is_temporary_root("/tmp/bddkit-r1", "r2"));
        assert!(!is_temporary_root("/home/x/artifacts", "r1"));
    }
}
