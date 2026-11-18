//! Nerd Font icons.
//!
//! Each row's icon stem (from [`frext_core::file_icon`]) is looked up in
//! the generated [`NERD_ICONS`] table, giving a glyph and its colour. A
//! stem missing from the table falls back to a generic file or folder
//! glyph.

use frext_core::editor::FolderState;
use frext_core::file_icon::{icon_for_dir, icon_for_file};
use frext_core::palette;

use super::nerd_table::NERD_ICONS;
use super::{IconCell, IconKind, IconProvider};

/// The glyph for a file whose stem is not in the table.
pub const FALLBACK_FILE: &str = "\u{f15b}";

/// The glyph for a closed folder whose stem is not in the table.
pub const FALLBACK_FOLDER: &str = "\u{e5ff}";

/// The glyph for an open folder whose stem is not in the table.
pub const FALLBACK_FOLDER_OPEN: &str = "\u{e5fe}";

/// The icon stem for the entry called `name` of the given `kind`: the
/// file's stem, or the folder's stem for its open or closed state.
#[must_use]
pub fn stem_for(name: &str, kind: IconKind) -> &'static str {
    match kind {
        IconKind::File => icon_for_file(name),
        IconKind::Dir(state) => icon_for_dir(name, state == FolderState::Expanded),
    }
}

/// The Nerd glyph and colour for `stem`, or the fallback for `kind`
/// when the table has no such stem.
#[must_use]
pub fn nerd_icon(stem: &str, kind: IconKind) -> IconCell {
    let found = NERD_ICONS
        .binary_search_by(|&(s, _, _)| s.cmp(stem))
        .ok()
        .and_then(|index| NERD_ICONS.get(index));
    if let Some(&(_, glyph, color)) = found {
        return IconCell::Text { text: glyph, color };
    }
    match kind {
        IconKind::File => IconCell::Text {
            text: FALLBACK_FILE,
            color: palette::TEXT,
        },
        IconKind::Dir(FolderState::Collapsed) => IconCell::Text {
            text: FALLBACK_FOLDER,
            color: palette::BLUE,
        },
        IconKind::Dir(FolderState::Expanded) => IconCell::Text {
            text: FALLBACK_FOLDER_OPEN,
            color: palette::BLUE,
        },
    }
}

/// The Nerd Font provider: each entry's glyph in its icon's colour.
#[derive(Debug, Clone, Copy, Default)]
pub struct NerdIcons;

impl IconProvider for NerdIcons {
    fn icon(&mut self, name: &str, kind: IconKind) -> IconCell {
        nerd_icon(stem_for(name, kind), kind)
    }
}

#[cfg(test)]
mod tests {
    use frext_core::palette::Rgb;
    use unicode_width::UnicodeWidthStr;

    use super::*;
    use crate::icons::ICON_WIDTH;

    const COLLAPSED: IconKind = IconKind::Dir(FolderState::Collapsed);
    const EXPANDED: IconKind = IconKind::Dir(FolderState::Expanded);

    /// The table entry for `stem` as an icon cell.
    fn entry(stem: &str) -> Option<IconCell> {
        NERD_ICONS
            .iter()
            .find(|&&(s, _, _)| s == stem)
            .map(|&(_, text, color)| IconCell::Text { text, color })
    }

    #[test]
    fn rust_file_has_the_rust_glyph() {
        assert_eq!(
            NerdIcons.icon("main.rs", IconKind::File),
            IconCell::Text {
                text: "\u{e68b}",
                color: Rgb::new(0xfa, 0xb3, 0x87),
            }
        );
    }

    #[test]
    fn cargo_toml_matches_its_table_entry() {
        let expected = entry(icon_for_file("Cargo.toml"));
        assert!(expected.is_some());
        assert_eq!(Some(NerdIcons.icon("Cargo.toml", IconKind::File)), expected);
    }

    #[test]
    fn folders_match_their_table_entries() {
        let closed = entry(icon_for_dir("src", false));
        let open = entry(icon_for_dir("src", true));
        assert!(closed.is_some() && open.is_some());
        assert_ne!(closed, open);
        assert_eq!(Some(NerdIcons.icon("src", COLLAPSED)), closed);
        assert_eq!(Some(NerdIcons.icon("src", EXPANDED)), open);
    }

    #[test]
    fn unknown_file_gets_the_generic_file_glyph() {
        let IconCell::Text { text, .. } = NerdIcons.icon("x.zzzunknown", IconKind::File) else {
            panic!("not text");
        };
        assert_eq!(text, "\u{f15b}");
    }

    #[test]
    fn missing_stem_falls_back_by_kind() {
        assert_eq!(
            nerd_icon("no-such-stem", IconKind::File),
            IconCell::Text {
                text: "\u{f15b}",
                color: palette::TEXT,
            }
        );
        assert_eq!(
            nerd_icon("no-such-stem", COLLAPSED),
            IconCell::Text {
                text: "\u{e5ff}",
                color: palette::BLUE,
            }
        );
        assert_eq!(
            nerd_icon("no-such-stem", EXPANDED),
            IconCell::Text {
                text: "\u{e5fe}",
                color: palette::BLUE,
            }
        );
    }

    #[test]
    fn stem_for_examples() {
        assert_eq!(stem_for("main.rs", IconKind::File), "rust");
        assert_eq!(stem_for("x.zzzunknown", IconKind::File), "_file");
        assert_eq!(stem_for("zzz-unnamed", COLLAPSED), "_folder");
        assert_eq!(stem_for("zzz-unnamed", EXPANDED), "_folder_open");
        assert_eq!(stem_for("src", COLLAPSED), icon_for_dir("src", false));
        assert_eq!(stem_for("src", EXPANDED), icon_for_dir("src", true));
    }

    #[test]
    fn every_table_glyph_fits_the_slot() {
        for &(stem, glyph, _) in NERD_ICONS {
            assert!(glyph.width() <= usize::from(ICON_WIDTH), "{stem}");
        }
    }

    #[test]
    fn every_table_stem_is_found_by_the_lookup() {
        for &(stem, glyph, color) in NERD_ICONS {
            assert_eq!(
                nerd_icon(stem, IconKind::File),
                IconCell::Text { text: glyph, color },
                "{stem}"
            );
        }
    }
}
