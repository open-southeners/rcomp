//! Partial-extraction entry selection.
//!
//! A [`Selection`] decides which archive entries an extraction writes and
//! where each one lands.  It is built from the entry paths a caller picked
//! (as reported by [`crate::list`]); each picked entry is extracted directly
//! into the destination under its own file name, and a picked directory brings
//! its whole subtree along.  Picking `docs/guide/` therefore writes
//! `dest/guide/…`, not `dest/docs/guide/…` — the same result as dragging that
//! folder out of a file manager.
//!
//! The remapped path is still a raw, attacker-controlled archive path: every
//! backend must pass it through [`super::sanitize::sanitize_entry_path`]
//! exactly as it does for a full extraction.

use std::{
    borrow::Cow,
    ffi::OsString,
    path::{Component, Path, PathBuf},
};

/// Which entries an extraction writes, and under which relative path.
///
/// The default selection extracts every entry at its stored path.
#[derive(Debug, Clone, Default)]
pub(crate) struct Selection {
    /// Selected roots as plain path components, with descendants of another
    /// selected root removed.  Empty means "everything".
    roots: Vec<Vec<OsString>>,
}

impl Selection {
    /// Build a selection from caller-supplied entry paths.
    ///
    /// Leading `./` components and trailing separators are ignored, so the
    /// paths [`crate::list`] reports match regardless of how the archive
    /// stored them.  An empty slice selects everything.  Paths that normalise
    /// to nothing (e.g. `"./"`) select the whole archive at its stored layout.
    pub(crate) fn new(paths: &[PathBuf]) -> Self {
        let mut roots: Vec<Vec<OsString>> = Vec::with_capacity(paths.len());
        for path in paths {
            let comps = normal_components(path);
            if comps.is_empty() {
                return Self::default();
            }
            roots.push(comps);
        }
        // Shortest first, so a parent is kept before any of its descendants.
        roots.sort_by_key(Vec::len);
        let mut kept: Vec<Vec<OsString>> = Vec::with_capacity(roots.len());
        for root in roots {
            if !kept.iter().any(|k| root.starts_with(k)) {
                kept.push(root);
            }
        }
        Self { roots: kept }
    }

    /// Whether every entry is extracted at its stored path.
    pub(crate) fn is_all(&self) -> bool {
        self.roots.is_empty()
    }

    /// Map a raw archive entry path to the relative path it is extracted at,
    /// or `None` when the entry is not selected.
    ///
    /// For a selected root `a/b`, the entry `a/b/c/d` maps to `b/c/d`.  Any
    /// non-normal trailing components (e.g. `..`) are carried over verbatim so
    /// the sanitizer still sees — and rejects — them.
    pub(crate) fn map<'p>(&self, raw: &'p Path) -> Option<Cow<'p, Path>> {
        if self.is_all() {
            return Some(Cow::Borrowed(raw));
        }
        let comps: Vec<Component<'_>> = raw
            .components()
            .skip_while(|c| matches!(c, Component::CurDir))
            .collect();
        for root in &self.roots {
            if comps.len() < root.len() {
                continue;
            }
            let matches = root
                .iter()
                .zip(&comps)
                .all(|(r, c)| matches!(c, Component::Normal(n) if *n == r.as_os_str()));
            if matches {
                let mut out = PathBuf::from(root.last()?);
                for c in &comps[root.len()..] {
                    out.push(c.as_os_str());
                }
                return Some(Cow::Owned(out));
            }
        }
        None
    }
}

/// The `Normal` components of `path`, ignoring `.` and any root/prefix.
fn normal_components(path: &Path) -> Vec<OsString> {
    path.components()
        .filter_map(|c| match c {
            Component::Normal(n) => Some(n.to_os_string()),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sel(paths: &[&str]) -> Selection {
        Selection::new(&paths.iter().map(PathBuf::from).collect::<Vec<_>>())
    }

    fn mapped(s: &Selection, raw: &str) -> Option<PathBuf> {
        s.map(Path::new(raw)).map(Cow::into_owned)
    }

    #[test]
    fn empty_selects_everything_unchanged() {
        let s = sel(&[]);
        assert!(s.is_all());
        assert_eq!(mapped(&s, "a/b.txt"), Some(PathBuf::from("a/b.txt")));
    }

    #[test]
    fn selected_file_lands_under_its_own_name() {
        let s = sel(&["docs/readme.md"]);
        assert_eq!(
            mapped(&s, "docs/readme.md"),
            Some(PathBuf::from("readme.md"))
        );
        assert_eq!(mapped(&s, "docs/other.md"), None);
        assert_eq!(mapped(&s, "docs/"), None);
    }

    #[test]
    fn selected_directory_brings_its_subtree() {
        let s = sel(&["docs/guide/"]);
        assert_eq!(mapped(&s, "docs/guide/"), Some(PathBuf::from("guide")));
        assert_eq!(
            mapped(&s, "docs/guide/intro/a.md"),
            Some(PathBuf::from("guide/intro/a.md"))
        );
        assert_eq!(mapped(&s, "docs/guidebook.md"), None);
    }

    #[test]
    fn leading_dot_components_are_ignored_on_both_sides() {
        let s = sel(&["./a"]);
        assert_eq!(mapped(&s, "./a/b"), Some(PathBuf::from("a/b")));
        assert_eq!(mapped(&s, "a/b"), Some(PathBuf::from("a/b")));
    }

    #[test]
    fn descendants_of_a_selected_root_keep_the_outer_layout() {
        let s = sel(&["a/b", "a"]);
        assert_eq!(mapped(&s, "a/b/c"), Some(PathBuf::from("a/b/c")));
    }

    #[test]
    fn traversal_after_a_selected_root_is_preserved_for_the_sanitizer() {
        let s = sel(&["a"]);
        assert_eq!(mapped(&s, "a/../../x"), Some(PathBuf::from("a/../../x")));
    }

    #[test]
    fn whole_archive_marker_selects_everything() {
        assert!(sel(&["./"]).is_all());
    }
}
