//! File-tree icons.
//!
//! The sidebar asks an [`IconProvider`] for the icon in front of each
//! row. The providers are:
//!
//! - [`AsciiIcons`]: plain text that works in every terminal;
//! - [`nerd::NerdIcons`]: Nerd Font glyphs in each icon's colour;
//! - [`kitty::KittyIcons`]: the file-type SVGs drawn as kitty images;
//! - [`NoIcons`]: an empty slot for every row.

use frext_core::editor::FolderState;
use frext_core::palette::{self, Rgb};

use crate::term::caps::{IconChoice, Multiplexer};
use crate::term::kitty::ImageId;

pub mod kitty;
pub mod nerd;
pub mod nerd_table;
pub mod raster;

use kitty::KittyIcons;
use nerd::NerdIcons;

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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IconCell {
    /// A text glyph.
    Text {
        /// The glyph; at most [`ICON_WIDTH`] cells wide, possibly empty.
        text: &'static str,
        /// The glyph's foreground colour.
        color: Rgb,
    },
    /// A kitty image, drawn as
    /// [`PLACEHOLDER_CELLS`](crate::term::kitty::PLACEHOLDER_CELLS)
    /// across the slot with the image id in the foreground colour.
    Image {
        /// The uploaded image to show.
        id: ImageId,
    },
}

/// Chooses the icon for a tree row.
///
/// `Debug` is a supertrait so a type that owns a provider (as
/// `FileTree` does) can still derive `Debug`.
pub trait IconProvider: std::fmt::Debug {
    /// The icon for the entry called `name` (its file name, not its
    /// path) of the given `kind`.
    fn icon(&mut self, name: &str, kind: IconKind) -> IconCell;

    /// The bytes queued by [`IconProvider::icon`] since the last call,
    /// to be written to the terminal before the frame that shows them.
    /// The default queues nothing.
    fn take_uploads(&mut self) -> Vec<u8> {
        Vec::new()
    }

    /// The bytes that delete everything uploaded; afterwards the
    /// provider has nothing uploaded. The default uploads nothing.
    fn release(&mut self) -> Vec<u8> {
        Vec::new()
    }
}

/// The provider with no icons: an empty slot for every row.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoIcons;

impl IconProvider for NoIcons {
    fn icon(&mut self, _name: &str, _kind: IconKind) -> IconCell {
        IconCell::Text {
            text: "",
            color: palette::TEXT,
        }
    }
}

/// The plain-text provider: `/` in blue for a folder, nothing for a
/// file. The disclosure marker already shows whether a folder is open.
#[derive(Debug, Clone, Copy, Default)]
pub struct AsciiIcons;

impl IconProvider for AsciiIcons {
    fn icon(&mut self, _name: &str, kind: IconKind) -> IconCell {
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

/// Whether escape output carries colours. crossterm drops every colour
/// when `NO_COLOR` is set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnsiColours {
    /// Colours are written.
    Enabled,
    /// Colours are dropped.
    Disabled,
}

impl AnsiColours {
    /// What crossterm will do with colours, as decided by `NO_COLOR`.
    #[must_use]
    pub fn from_env() -> Self {
        if crossterm::style::Colored::ansi_color_disabled() {
            Self::Disabled
        } else {
            Self::Enabled
        }
    }
}

/// The provider for `choice`: kitty icons wrapped for `multiplexer`,
/// Nerd Font, ASCII or no icons.
///
/// Kitty icons need colour output, because the image id travels in the
/// placeholder's foreground colour; under [`AnsiColours::Disabled`] a
/// kitty choice gives Nerd Font icons instead.
#[must_use]
pub fn provider(
    choice: IconChoice,
    multiplexer: Multiplexer,
    colours: AnsiColours,
) -> Box<dyn IconProvider> {
    match (choice, colours) {
        (IconChoice::Kitty { cell_px }, AnsiColours::Enabled) => {
            Box::new(KittyIcons::new(cell_px, multiplexer))
        }
        (IconChoice::Kitty { .. }, AnsiColours::Disabled) => {
            log::warn!(
                "kitty icons need colour output (the image id travels in the \
                 placeholder's foreground colour); falling back to Nerd Font icons"
            );
            Box::new(NerdIcons)
        }
        (IconChoice::Nerd, _) => Box::new(NerdIcons),
        (IconChoice::Ascii, _) => Box::new(AsciiIcons),
        (IconChoice::None, _) => Box::new(NoIcons),
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
            let IconCell::Text { text, .. } = AsciiIcons.icon("name", kind) else {
                panic!("{kind:?} is not text");
            };
            assert!(text.width() <= usize::from(ICON_WIDTH), "{kind:?}");
        }
    }

    #[test]
    fn works_as_a_trait_object() {
        let mut ascii = AsciiIcons;
        let provider: &mut dyn IconProvider = &mut ascii;
        assert_eq!(
            provider.icon("lib", IconKind::Dir(FolderState::Expanded)),
            FOLDER
        );
        assert_eq!(provider.icon("lib.rs", IconKind::File), FILE);
        assert!(provider.take_uploads().is_empty());
        assert!(provider.release().is_empty());
    }

    #[test]
    fn no_icons_is_empty_for_every_kind() {
        for kind in [
            IconKind::File,
            IconKind::Dir(FolderState::Collapsed),
            IconKind::Dir(FolderState::Expanded),
        ] {
            assert_eq!(NoIcons.icon("src", kind), FILE, "{kind:?}");
        }
        assert!(NoIcons.take_uploads().is_empty());
        assert!(NoIcons.release().is_empty());
    }

    const KITTY: IconChoice = IconChoice::Kitty {
        cell_px: crate::term::caps::CellPx {
            width: 10,
            height: 20,
        },
    };

    fn provider_name(choice: IconChoice, colours: AnsiColours) -> String {
        format!("{:?}", provider(choice, Multiplexer::None, colours))
    }

    #[test]
    fn kitty_choice_with_colours_gives_kitty_icons() {
        assert!(provider_name(KITTY, AnsiColours::Enabled).starts_with("KittyIcons"));
    }

    #[test]
    fn kitty_choice_without_colours_falls_back_to_nerd_icons() {
        assert!(provider_name(KITTY, AnsiColours::Disabled).starts_with("NerdIcons"));
    }

    #[test]
    fn other_choices_give_their_providers() {
        for colours in [AnsiColours::Enabled, AnsiColours::Disabled] {
            assert!(provider_name(IconChoice::Nerd, colours).starts_with("NerdIcons"));
            assert!(provider_name(IconChoice::Ascii, colours).starts_with("AsciiIcons"));
            assert!(provider_name(IconChoice::None, colours).starts_with("NoIcons"));
        }
    }

    #[test]
    fn ansi_colours_from_env_is_callable() {
        let colours = AnsiColours::from_env();
        assert!(matches!(
            colours,
            AnsiColours::Enabled | AnsiColours::Disabled
        ));
    }

    /// The `(glyph, colour)` the Nerd table has for `stem`.
    fn nerd(stem: &str) -> Option<(&'static str, Rgb)> {
        nerd_table::NERD_ICONS
            .iter()
            .find(|&&(s, _, _)| s == stem)
            .map(|&(_, glyph, color)| (glyph, color))
    }

    #[test]
    fn nerd_table_stems_match_the_file_icon_stems_in_order() {
        let nerd: Vec<&str> = nerd_table::NERD_ICONS
            .iter()
            .map(|&(stem, _, _)| stem)
            .collect();
        let core: Vec<&str> = frext_core::file_icon::stems().collect();
        assert_eq!(nerd, core);
    }

    #[test]
    fn every_nerd_glyph_is_one_char() {
        for &(stem, glyph, _) in nerd_table::NERD_ICONS {
            assert_eq!(glyph.chars().count(), 1, "{stem}");
        }
    }

    #[test]
    fn nerd_folder_and_file_glyphs() {
        assert_eq!(nerd("_folder").map(|(g, _)| g), Some("\u{e5ff}"));
        assert_eq!(nerd("_folder_open").map(|(g, _)| g), Some("\u{e5fe}"));
        assert_eq!(nerd("_file").map(|(g, _)| g), Some("\u{f15b}"));
    }

    #[test]
    fn nerd_rust_has_the_devicons_glyph_and_svg_colour() {
        assert_eq!(nerd("rust"), Some(("\u{e68b}", Rgb::new(0xfa, 0xb3, 0x87))));
    }

    /// The Nerd table's glyph for `stem`, as a code point.
    fn nerd_glyph(stem: &str) -> Option<u32> {
        nerd(stem).and_then(|(glyph, _)| glyph.chars().next().map(u32::from))
    }

    #[test]
    fn nerd_generic_stems_keep_their_extension_glyph() {
        // Tool-config filename entries must not outvote the language's own
        // extension glyph, and ties pick the entry named like the stem.
        for (stem, glyph) in [
            ("javascript", 0xe60c),
            ("yaml", 0xe8eb),
            ("ruby", 0xe791),
            ("text", 0xf0219),
            ("properties", 0xe615),
            ("haskell", 0xe61f),
            ("html", 0xe736),
            ("coffeescript", 0xe61b),
        ] {
            assert_eq!(nerd_glyph(stem), Some(glyph), "{stem}");
        }
    }

    #[test]
    fn nerd_languages_match_devicons_extension_entries() {
        // The glyphs of devicons' own `ts`, `json`, `toml`, `nix` and `py`
        // entries at the pinned commit. `markdown` takes devicons' `markdown`
        // entry, which wins its tie with `md` because its key equals the stem.
        for (stem, glyph) in [
            ("typescript", 0xe628),
            ("json", 0xe60b),
            ("toml", 0xe6b2),
            ("markdown", 0xe609),
            ("nix", 0xf313),
            ("python", 0xe606),
        ] {
            assert_eq!(nerd_glyph(stem), Some(glyph), "{stem}");
        }
    }

    #[test]
    fn nerd_folder_controllers_takes_its_accent_colour() {
        let colour = nerd("folder_controllers").map(|(_, colour)| colour);
        assert!(colour.is_some());
        assert_ne!(colour, Some(palette::TEXT));
    }
}
