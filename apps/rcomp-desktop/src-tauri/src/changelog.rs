//! Parsed release notes for the in-app "What's New" viewer.

/// The workspace `CHANGELOG.md`, embedded at compile time so the viewer
/// always matches the exact release it ships in.
const CHANGELOG_MARKDOWN: &str = include_str!("../../../../CHANGELOG.md");

/// Render the workspace changelog to HTML for the in-app viewer.
#[tauri::command]
pub fn get_changelog() -> String {
    let parser = pulldown_cmark::Parser::new(CHANGELOG_MARKDOWN);
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, parser);
    html
}
