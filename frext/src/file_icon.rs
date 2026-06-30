//! File-type icons for the sidebar file tree.
//!
//! The icon artwork is the Catppuccin Mocha flavour of the
//! [`catppuccin/zed-icons`](https://github.com/catppuccin/zed-icons) set,
//! vendored under `assets/file-icons/mocha/` and embedded at compile time. The
//! lookup tables (`file_icon_table`) and the embedded bytes
//! (`file_icon_bytes`) are generated from the upstream icon-theme manifest; the
//! logic that consumes them lives here so it stays testable.
//!
//! Resolution mirrors the upstream icon theme: a file's full name is matched
//! against the stem table first (so `Cargo.toml` gets the cargo icon rather
//! than the generic TOML one), then its trailing dotted segments are matched
//! against the suffix table longest-first (so `app.component.ts` prefers a
//! `component.ts` rule over a bare `ts` rule). Folders are matched by their
//! lowercased name against the named-directory table, falling back to the
//! generic open/closed folder icons.

use crate::{file_icon_bytes::ICON_BYTES, file_icon_table};

/// The SVG stem of the icon for a file named `name`.
///
/// `name` is the file name only (no directory component). The returned value
/// is a key into [`icon_bytes`]; it is never empty and always resolves to an
/// embedded icon.
#[must_use]
pub fn icon_for_file(name: &str) -> &'static str {
    // Full-name rules win (e.g. `README.md`, `Makefile`, `Dockerfile`).
    if let Some(stem) = lookup(file_icon_table::FILE_STEMS, name) {
        return stem;
    }

    // Then suffix rules, matched longest trailing-dotted-segment first so a
    // multi-part suffix beats a single extension.
    for candidate in suffix_candidates(name) {
        if let Some(stem) = lookup_suffix(candidate) {
            return stem;
        }
    }

    file_icon_table::DEFAULT_FILE
}

/// The SVG stem of the icon for a directory named `name`.
///
/// `expanded` selects the open or closed variant. Folder names are matched
/// case-insensitively against the named-directory table.
#[must_use]
pub fn icon_for_dir(name: &str, expanded: bool) -> &'static str {
    let lower = name.to_lowercase();
    for &(folder, collapsed, opened) in file_icon_table::NAMED_DIRS {
        if folder == lower {
            return if expanded { opened } else { collapsed };
        }
    }

    if expanded {
        file_icon_table::DEFAULT_DIR_EXPANDED
    } else {
        file_icon_table::DEFAULT_DIR_COLLAPSED
    }
}

/// The embedded SVG bytes for an icon `stem`, or `None` if no such icon is
/// bundled (which should not happen for stems returned by [`icon_for_file`] or
/// [`icon_for_dir`], since every referenced icon is vendored).
#[must_use]
pub fn icon_bytes(stem: &str) -> Option<&'static [u8]> {
    ICON_BYTES
        .binary_search_by(|&(key, _)| key.cmp(stem))
        .ok()
        .map(|index| ICON_BYTES[index].1)
}

/// An [`egui::ImageSource`] for the icon `stem`, ready to hand to
/// [`egui::Image::new`].
///
/// The `uri` is stable per stem so egui's image cache reuses a single rendered
/// texture across frames and rows. Returns `None` when the stem is not bundled.
#[must_use]
pub fn image_source(stem: &'static str) -> Option<egui::ImageSource<'static>> {
    let bytes = icon_bytes(stem)?;
    Some(egui::ImageSource::Bytes {
        // The `.svg` extension routes the bytes to egui_extras' SVG loader,
        // and the `frext-file-icon://` scheme keeps these URIs from colliding
        // with any other image source in the app.
        uri: stem_uri(stem).into(),
        bytes: egui::load::Bytes::Static(bytes),
    })
}

/// The cache URI for an icon `stem`. Pure and deterministic so it can be tested
/// without an egui context.
#[must_use]
fn stem_uri(stem: &str) -> String {
    format!("frext-file-icon://{stem}.svg")
}

/// Look up `key` in a sorted `(key, value)` table by binary search.
fn lookup(table: &[(&str, &'static str)], key: &str) -> Option<&'static str> {
    table
        .binary_search_by(|&(table_key, _)| table_key.cmp(key))
        .ok()
        .map(|index| table[index].1)
}

/// Look up a suffix candidate, trying an exact (case-sensitive) match first —
/// the upstream table carries a handful of case-sensitive keys such as
/// `Dockerfile` and `Rproj` — then a lowercased match for the common case.
fn lookup_suffix(candidate: &str) -> Option<&'static str> {
    if let Some(stem) = lookup(file_icon_table::FILE_SUFFIXES, candidate) {
        return Some(stem);
    }
    let lower = candidate.to_lowercase();
    if lower != candidate {
        return lookup(file_icon_table::FILE_SUFFIXES, &lower);
    }
    None
}

/// The trailing dotted-segment candidates of `name`, longest first.
///
/// `app.component.ts` yields `app.component.ts`, `component.ts`, then `ts`. A
/// leading-dot dotfile such as `.gitignore` yields `gitignore` (the upstream
/// suffix keys are stored without the leading dot for that style of match,
/// while full dotfile names like `.alexrc` are caught by the stem table).
fn suffix_candidates(name: &str) -> impl Iterator<Item = &str> {
    // Byte offsets of each '.' in the name, ascending.
    let dots: Vec<usize> = name
        .char_indices()
        .filter(|&(_, c)| c == '.')
        .map(|(i, _)| i)
        .collect();

    dots.into_iter().map(move |dot| {
        // Skip the dot itself so `foo.ts` yields the candidate `ts`.
        &name[dot + 1..]
    })
}

#[cfg(test)]
mod tests {
    // `unwrap` is acceptable in test code: a panic on an unexpected `None`
    // is exactly the failure signal we want from a test.
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn every_referenced_icon_is_embedded() {
        // The default stems, plus every stem named by the lookup tables, must
        // resolve to embedded bytes — otherwise a real file would render with
        // a missing icon.
        for stem in [
            file_icon_table::DEFAULT_FILE,
            file_icon_table::DEFAULT_DIR_COLLAPSED,
            file_icon_table::DEFAULT_DIR_EXPANDED,
        ] {
            assert!(icon_bytes(stem).is_some(), "missing default icon: {stem}");
        }
        for &(_, stem) in file_icon_table::FILE_STEMS {
            assert!(icon_bytes(stem).is_some(), "missing stem icon: {stem}");
        }
        for &(_, stem) in file_icon_table::FILE_SUFFIXES {
            assert!(icon_bytes(stem).is_some(), "missing suffix icon: {stem}");
        }
        for &(_, collapsed, expanded) in file_icon_table::NAMED_DIRS {
            assert!(
                icon_bytes(collapsed).is_some(),
                "missing dir icon: {collapsed}"
            );
            assert!(
                icon_bytes(expanded).is_some(),
                "missing dir icon: {expanded}"
            );
        }
    }

    #[test]
    fn embed_table_is_sorted_for_binary_search() {
        assert!(
            ICON_BYTES.windows(2).all(|w| w[0].0 < w[1].0),
            "ICON_BYTES must be sorted by stem"
        );
    }

    #[test]
    fn lookup_tables_are_sorted_for_binary_search() {
        assert!(
            file_icon_table::FILE_STEMS
                .windows(2)
                .all(|w| w[0].0 < w[1].0),
            "FILE_STEMS must be sorted by key"
        );
        assert!(
            file_icon_table::FILE_SUFFIXES
                .windows(2)
                .all(|w| w[0].0 < w[1].0),
            "FILE_SUFFIXES must be sorted by key"
        );
    }

    #[test]
    fn unknown_file_falls_back_to_the_generic_icon() {
        assert_eq!(
            icon_for_file("mystery.unheardofextension"),
            file_icon_table::DEFAULT_FILE
        );
        assert_eq!(
            icon_for_file("no_extension_at_all"),
            file_icon_table::DEFAULT_FILE
        );
    }

    #[test]
    fn rust_source_gets_the_rust_icon() {
        // `rs` is a stable suffix rule in the upstream set.
        assert_eq!(icon_for_file("main.rs"), "rust");
    }

    #[test]
    fn full_name_rules_beat_suffix_rules() {
        // `Cargo.toml` resolves to the cargo icon via the stem table, not the
        // generic TOML icon its `toml` suffix would otherwise select.
        let by_name = icon_for_file("Cargo.toml");
        let by_suffix = lookup_suffix("toml").unwrap();
        assert_ne!(
            by_name, by_suffix,
            "Cargo.toml should not resolve to the generic toml icon"
        );
    }

    #[test]
    fn longer_suffixes_win_over_shorter_ones() {
        // `app.component.ts` should not silently resolve to the same icon as a
        // bare `.ts` file when a `component.ts` rule exists. We assert the
        // candidate ordering directly so the test does not depend on which
        // specific framework rules upstream ships.
        let candidates: Vec<&str> = suffix_candidates("app.component.ts").collect();
        assert_eq!(candidates, vec!["component.ts", "ts"]);
    }

    #[test]
    fn suffix_candidates_handles_dotfiles_and_plain_names() {
        assert_eq!(
            suffix_candidates(".gitignore").collect::<Vec<_>>(),
            vec!["gitignore"]
        );
        assert!(suffix_candidates("plain").next().is_none());
    }

    #[test]
    fn directories_use_open_and_closed_variants() {
        let closed = icon_for_dir("some_unknown_folder", false);
        let open = icon_for_dir("some_unknown_folder", true);
        assert_eq!(closed, file_icon_table::DEFAULT_DIR_COLLAPSED);
        assert_eq!(open, file_icon_table::DEFAULT_DIR_EXPANDED);
        assert_ne!(closed, open);
    }

    #[test]
    fn named_directories_are_matched_case_insensitively() {
        // `.github` is a named directory in the upstream set; the lookup must
        // not depend on the exact case the user has on disk.
        let lower = icon_for_dir(".github", false);
        let upper = icon_for_dir(".GITHUB", false);
        assert_eq!(lower, upper);
    }

    #[test]
    fn stem_uri_is_stable_and_svg_suffixed() {
        assert_eq!(stem_uri("rust"), "frext-file-icon://rust.svg");
    }
}
