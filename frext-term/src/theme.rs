//! Catppuccin Mocha colours as ratatui styles.
//!
//! The palette itself lives in [`frext_core::palette`]; this module only
//! converts it into terminal [`Color`]s and [`Style`]s for each piece of
//! the interface.

use frext_core::highlight::{MatchBackground, StyledSpan};
use frext_core::palette::{
    self, BASE, MANTLE, OVERLAY0, OVERLAY1, Rgb, SUBTEXT0, SURFACE0, SURFACE1, TEXT,
};
use frext_core::proofread::DiagnosticKind;
use ratatui::style::{Color, Modifier, Style};

/// Converts a palette colour into a 24-bit terminal colour.
#[must_use]
pub const fn color(rgb: Rgb) -> Color {
    Color::Rgb(rgb.r, rgb.g, rgb.b)
}

/// The editor body: text on the base background.
#[must_use]
pub const fn base() -> Style {
    Style::new().fg(color(TEXT)).bg(color(BASE))
}

/// The tab bar and status bar background.
#[must_use]
pub const fn chrome() -> Style {
    Style::new().fg(color(TEXT)).bg(color(MANTLE))
}

/// The title of the active tab.
#[must_use]
pub const fn active_tab() -> Style {
    chrome()
        .fg(color(palette::accent()))
        .add_modifier(Modifier::BOLD)
}

/// The close glyph on a tab.
#[must_use]
pub const fn close_glyph() -> Style {
    chrome().fg(color(OVERLAY1))
}

/// The line-number gutter.
#[must_use]
pub const fn gutter() -> Style {
    Style::new().fg(color(palette::gutter())).bg(color(BASE))
}

/// An ordinary status-line message.
#[must_use]
pub const fn status() -> Style {
    Style::new().fg(color(SUBTEXT0)).bg(color(MANTLE))
}

/// A status-line error message.
#[must_use]
pub const fn status_error() -> Style {
    Style::new().fg(color(palette::error())).bg(color(MANTLE))
}

/// The body of a popup.
#[must_use]
pub const fn popup() -> Style {
    Style::new().fg(color(TEXT)).bg(color(SURFACE0))
}

/// The border of a popup.
#[must_use]
pub const fn popup_border() -> Style {
    Style::new()
        .fg(color(palette::accent()))
        .bg(color(SURFACE0))
}

/// A disabled entry inside a popup.
#[must_use]
pub const fn disabled() -> Style {
    Style::new().fg(color(OVERLAY0)).bg(color(SURFACE0))
}

/// A text input field inside a popup.
#[must_use]
pub const fn input() -> Style {
    Style::new().fg(color(TEXT)).bg(color(SURFACE1))
}

/// Placeholder text drawn over an empty input field.
#[must_use]
pub const fn input_hint() -> Style {
    Style::new().fg(color(OVERLAY0)).bg(color(SURFACE1))
}

/// A find-bar toggle that is on: distinct from [`hover_bg`], so an off
/// toggle under the mouse does not look on.
#[must_use]
pub fn toggle_on() -> Style {
    Style::new()
        .fg(color(palette::accent()))
        .bg(color(palette::selection().over(SURFACE0)))
        .add_modifier(Modifier::BOLD)
}

/// The focused button in a popup.
#[must_use]
pub fn button_focused() -> Style {
    Style::new()
        .fg(color(TEXT))
        .bg(color(palette::selection().over(SURFACE0)))
}

/// The background of a hovered element on the chrome.
#[must_use]
pub fn hover_bg() -> Color {
    color(palette::selection().over(MANTLE))
}

/// The background of selected text in the editor.
#[must_use]
pub fn selection_bg() -> Color {
    color(palette::selection().over(BASE))
}

/// The background of a search match in the editor.
#[must_use]
pub fn match_bg(role: MatchBackground) -> Color {
    match role {
        MatchBackground::Other => color(palette::search_match()),
        MatchBackground::Current => color(palette::search_current_match().over(BASE)),
    }
}

/// A syntax-highlighted run on the base background.
#[must_use]
pub fn span(span: &StyledSpan) -> Style {
    let mut style = Style::new().fg(color(span.fg)).bg(color(BASE));
    if span.italic {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if span.underline {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    style
}

/// The colour of the underline under a proofreading diagnostic of
/// `kind`.
#[must_use]
pub const fn squiggle(kind: DiagnosticKind) -> Color {
    match kind {
        DiagnosticKind::Spelling => color(palette::spelling_squiggle()),
        DiagnosticKind::Grammar => color(palette::grammar_squiggle()),
    }
}

/// The glyph drawn in place of a control char.
#[must_use]
pub const fn replacement() -> Style {
    Style::new().fg(color(palette::gutter())).bg(color(BASE))
}

/// The file-tree row of the active tab's file.
#[must_use]
pub const fn tree_active() -> Style {
    Style::new()
        .fg(color(palette::accent()))
        .bg(color(MANTLE))
        .add_modifier(Modifier::BOLD)
}

/// The background of the file-tree row under the keyboard cursor.
#[must_use]
pub fn tree_cursor_bg() -> Color {
    color(palette::selection().over(SURFACE0))
}

/// The file tree's resizable border.
#[must_use]
pub const fn tree_border() -> Style {
    Style::new().bg(color(SURFACE0))
}

/// The file tree's border while hovered or being dragged.
#[must_use]
pub const fn tree_border_active() -> Style {
    Style::new().bg(color(palette::accent()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn colours(style: Style) -> (Option<Color>, Option<Color>) {
        (style.fg, style.bg)
    }

    #[test]
    fn color_maps_rgb_exactly() {
        assert_eq!(color(Rgb::new(1, 2, 3)), Color::Rgb(1, 2, 3));
        assert_eq!(color(TEXT), Color::Rgb(0xcd, 0xd6, 0xf4));
    }

    #[test]
    fn base_is_text_on_base() {
        assert_eq!(colours(base()), (Some(color(TEXT)), Some(color(BASE))));
    }

    #[test]
    fn chrome_is_text_on_mantle() {
        assert_eq!(colours(chrome()), (Some(color(TEXT)), Some(color(MANTLE))));
    }

    #[test]
    fn active_tab_is_bold_accent_on_mantle() {
        let style = active_tab();
        assert_eq!(
            colours(style),
            (Some(color(palette::accent())), Some(color(MANTLE)))
        );
        assert!(style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn close_glyph_is_overlay1_on_mantle() {
        assert_eq!(
            colours(close_glyph()),
            (Some(color(OVERLAY1)), Some(color(MANTLE)))
        );
    }

    #[test]
    fn gutter_is_gutter_on_base() {
        assert_eq!(
            colours(gutter()),
            (Some(color(palette::gutter())), Some(color(BASE)))
        );
    }

    #[test]
    fn status_is_subtext0_on_mantle() {
        assert_eq!(
            colours(status()),
            (Some(color(SUBTEXT0)), Some(color(MANTLE)))
        );
    }

    #[test]
    fn status_error_is_error_on_mantle() {
        assert_eq!(
            colours(status_error()),
            (Some(color(palette::error())), Some(color(MANTLE)))
        );
    }

    #[test]
    fn popup_is_text_on_surface0() {
        assert_eq!(colours(popup()), (Some(color(TEXT)), Some(color(SURFACE0))));
    }

    #[test]
    fn popup_border_is_accent_on_surface0() {
        assert_eq!(
            colours(popup_border()),
            (Some(color(palette::accent())), Some(color(SURFACE0)))
        );
    }

    #[test]
    fn disabled_is_overlay0_on_surface0() {
        assert_eq!(
            colours(disabled()),
            (Some(color(OVERLAY0)), Some(color(SURFACE0)))
        );
    }

    #[test]
    fn input_is_text_on_surface1() {
        assert_eq!(colours(input()), (Some(color(TEXT)), Some(color(SURFACE1))));
    }

    #[test]
    fn input_hint_is_overlay0_on_surface1() {
        assert_eq!(
            colours(input_hint()),
            (Some(color(OVERLAY0)), Some(color(SURFACE1)))
        );
    }

    #[test]
    fn toggle_on_is_bold_accent_on_selection_over_surface0() {
        let style = toggle_on();
        assert_eq!(
            colours(style),
            (
                Some(color(palette::accent())),
                Some(color(palette::selection().over(SURFACE0)))
            )
        );
        assert!(style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn toggle_on_background_differs_from_hover() {
        assert_ne!(toggle_on().bg, Some(hover_bg()));
    }

    #[test]
    fn button_focused_is_text_on_selection_over_surface0() {
        assert_eq!(
            colours(button_focused()),
            (
                Some(color(TEXT)),
                Some(color(palette::selection().over(SURFACE0)))
            )
        );
    }

    #[test]
    fn hover_bg_is_selection_over_mantle() {
        assert_eq!(hover_bg(), color(palette::selection().over(MANTLE)));
        assert_ne!(hover_bg(), color(MANTLE));
    }

    #[test]
    fn selection_bg_is_selection_over_base() {
        assert_eq!(selection_bg(), color(palette::selection().over(BASE)));
        assert_ne!(selection_bg(), color(BASE));
    }

    #[test]
    fn match_bg_maps_both_roles() {
        assert_eq!(
            match_bg(MatchBackground::Other),
            color(palette::search_match())
        );
        assert_eq!(
            match_bg(MatchBackground::Current),
            color(palette::search_current_match().over(BASE))
        );
        assert_ne!(
            match_bg(MatchBackground::Other),
            match_bg(MatchBackground::Current)
        );
    }

    #[test]
    fn span_uses_fg_on_base_with_flags() {
        let plain = StyledSpan {
            bytes: 0..1,
            fg: Rgb::new(1, 2, 3),
            background: Some(MatchBackground::Current),
            italic: false,
            underline: false,
        };
        let style = span(&plain);
        assert_eq!(
            colours(style),
            (Some(Color::Rgb(1, 2, 3)), Some(color(BASE)))
        );
        assert!(style.add_modifier.is_empty());
        let styled = StyledSpan {
            italic: true,
            underline: true,
            ..plain
        };
        let style = span(&styled);
        assert!(style.add_modifier.contains(Modifier::ITALIC));
        assert!(style.add_modifier.contains(Modifier::UNDERLINED));
    }

    #[test]
    fn squiggle_maps_both_kinds() {
        assert_eq!(
            squiggle(DiagnosticKind::Spelling),
            color(palette::spelling_squiggle())
        );
        assert_eq!(
            squiggle(DiagnosticKind::Grammar),
            color(palette::grammar_squiggle())
        );
        assert_ne!(
            squiggle(DiagnosticKind::Spelling),
            squiggle(DiagnosticKind::Grammar)
        );
    }

    #[test]
    fn replacement_is_gutter_on_base() {
        assert_eq!(
            colours(replacement()),
            (Some(color(palette::gutter())), Some(color(BASE)))
        );
    }

    #[test]
    fn tree_active_is_bold_accent_on_mantle() {
        let style = tree_active();
        assert_eq!(
            colours(style),
            (Some(color(palette::accent())), Some(color(MANTLE)))
        );
        assert!(style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn tree_cursor_bg_is_selection_over_surface0() {
        assert_eq!(tree_cursor_bg(), color(palette::selection().over(SURFACE0)));
        assert_ne!(tree_cursor_bg(), color(SURFACE0));
    }

    #[test]
    fn tree_border_is_surface0_bg() {
        assert_eq!(colours(tree_border()), (None, Some(color(SURFACE0))));
    }

    #[test]
    fn tree_border_active_is_accent_bg() {
        assert_eq!(
            colours(tree_border_active()),
            (None, Some(color(palette::accent())))
        );
        assert_ne!(tree_border_active().bg, tree_border().bg);
    }
}
