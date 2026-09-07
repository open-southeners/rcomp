//! Parsed release notes for the in-app "What's New" viewer.
//!
//! The webview has no back/forward chrome, so a clicked link just navigates
//! it away like a lost browser tab. [`render`] therefore strips the
//! Keep a Changelog / SemVer preamble, the `## [Unreleased]` section (nothing
//! in it has shipped yet), and every link — keeping each link's text but
//! dropping the anchor — before handing the rest to the viewer as HTML.

use pulldown_cmark::{Event, Parser, Tag, TagEnd};

/// The workspace `CHANGELOG.md`, embedded at compile time so the viewer
/// always matches the exact release it ships in.
const CHANGELOG_MARKDOWN: &str = include_str!("../../../../CHANGELOG.md");

/// Render the workspace changelog to HTML for the in-app viewer.
#[tauri::command]
pub fn get_changelog() -> String {
    render(CHANGELOG_MARKDOWN)
}

/// Render `markdown` for the in-app viewer.
///
/// See the module docs for what is stripped and why.
fn render(markdown: &str) -> String {
    let body = strip_unreleased(strip_preamble(markdown));

    let parser = Parser::new(&body).filter(|event| {
        !matches!(
            event,
            Event::Start(Tag::Link { .. }) | Event::End(TagEnd::Link)
        )
    });

    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, parser);
    html
}

/// Drop everything before the first version heading — the `# Changelog`
/// title and the Keep a Changelog / SemVer blurb.
fn strip_preamble(markdown: &str) -> &str {
    match markdown.find("\n## ") {
        Some(i) => &markdown[i + 1..],
        None => markdown,
    }
}

/// Drop the `## [Unreleased]` section: its heading through the next `## `
/// heading (or end of input).
fn strip_unreleased(markdown: &str) -> String {
    let mut out = String::with_capacity(markdown.len());
    let mut skipping = false;
    for line in markdown.lines() {
        if line.starts_with("## ") {
            skipping = line.starts_with("## [Unreleased]");
        }
        if skipping {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_preamble_up_to_first_heading() {
        let md = "# Changelog\n\nSome blurb.\n\n## [1.0.0] - 2026-01-01\n\nStuff.\n";
        assert_eq!(strip_preamble(md), "## [1.0.0] - 2026-01-01\n\nStuff.\n");
    }

    #[test]
    fn strips_unreleased_section_only() {
        let md = "## [Unreleased]\n\n- wip\n\n## [1.0.0] - 2026-01-01\n\n- shipped\n";
        assert_eq!(
            strip_unreleased(md),
            "## [1.0.0] - 2026-01-01\n\n- shipped\n"
        );
    }

    #[test]
    fn render_drops_links_but_keeps_their_text() {
        let md = "## [1.0.0] - 2026-01-01\n\nSee [the repo](https://example.com/repo).\n";
        let html = render(md);
        assert!(
            !html.contains("<a "),
            "rendered changelog must not contain links: {html}"
        );
        assert!(
            html.contains("the repo"),
            "link text should still render: {html}"
        );
    }

    #[test]
    fn render_drops_unreleased_and_preamble_from_real_changelog() {
        let html = render(CHANGELOG_MARKDOWN);
        assert!(!html.contains("Keep a Changelog"), "{html}");
        assert!(!html.contains("Unreleased"), "{html}");
        assert!(!html.contains("<a "), "{html}");
    }
}
