//! File-tree icons.
//!
//! The sidebar asks an [`IconProvider`] for the icon in front of each
//! row. [`AsciiIcons`] is the plain-text provider that works in every
//! terminal; the provider is the seam plan 08 extends with Nerd Font and
//! kitty-image providers.

use frext_core::editor::FolderState;
use frext_core::palette::{self, Rgb};

/// The width of the icon slot in front of a tree row, in cells.
pub const ICON_WIDTH: u16 = 2;

/// What a tree row names: a file, or a folder in a given state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconKind {
    /// A regular file.
    File,
    /// A folder, open or closed.
    Dir(FolderState),
}

/// The icon drawn in a row's icon slot.
///
/// Plan 08 adds an image variant for terminals that can draw pictures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IconCell {
    /// A text glyph.
    Text {
        /// The glyph; at most [`ICON_WIDTH`] cells wide, possibly empty.
        text: &'static str,
        /// The glyph's foreground colour.
        color: Rgb,
    },
}

/// Chooses the icon for a tree row.
///
/// `Debug` is a supertrait so a type that owns a provider (as
/// `FileTree` does) can still derive `Debug`.
pub trait IconProvider: std::fmt::Debug {
    /// The icon for the entry called `name` (its file name, not its
    /// path) of the given `kind`.
    fn icon(&self, name: &str, kind: IconKind) -> IconCell;
}

/// The plain-text provider: `/` in blue for a folder, nothing for a
/// file. The disclosure marker already shows whether a folder is open.
#[derive(Debug, Clone, Copy, Default)]
pub struct AsciiIcons;

impl IconProvider for AsciiIcons {
    fn icon(&self, _name: &str, kind: IconKind) -> IconCell {
        match kind {
            IconKind::Dir(_) => IconCell::Text {
                text: "/",
                color: palette::BLUE,
            },
            IconKind::File => IconCell::Text {
                text: "",
                color: palette::TEXT,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    const FOLDER: IconCell = IconCell::Text {
        text: "/",
        color: palette::BLUE,
    };
    const FILE: IconCell = IconCell::Text {
        text: "",
        color: palette::TEXT,
    };

    #[test]
    fn collapsed_folder_is_a_blue_slash() {
        assert_eq!(
            AsciiIcons.icon("src", IconKind::Dir(FolderState::Collapsed)),
            FOLDER
        );
    }

    #[test]
    fn expanded_folder_is_a_blue_slash() {
        assert_eq!(
            AsciiIcons.icon("src", IconKind::Dir(FolderState::Expanded)),
            FOLDER
        );
    }

    #[test]
    fn files_have_no_glyph() {
        assert_eq!(AsciiIcons.icon("main.rs", IconKind::File), FILE);
        assert_eq!(AsciiIcons.icon("README", IconKind::File), FILE);
    }

    #[test]
    fn every_icon_fits_the_slot() {
        let kinds = [
            IconKind::File,
            IconKind::Dir(FolderState::Collapsed),
            IconKind::Dir(FolderState::Expanded),
        ];
        for kind in kinds {
            let IconCell::Text { text, .. } = AsciiIcons.icon("name", kind);
            assert!(text.width() <= usize::from(ICON_WIDTH), "{kind:?}");
        }
    }

    #[test]
    fn works_as_a_trait_object() {
        let provider: &dyn IconProvider = &AsciiIcons;
        assert_eq!(
            provider.icon("lib", IconKind::Dir(FolderState::Expanded)),
            FOLDER
        );
        assert_eq!(provider.icon("lib.rs", IconKind::File), FILE);
    }
}
