//! Catppuccin Mocha colours as ratatui styles.
//!
//! The palette itself lives in [`frext_core::palette`]; this module only
//! converts it into terminal [`Color`]s and [`Style`]s for each piece of
//! the interface.

use frext_core::palette::{self, BASE, MANTLE, OVERLAY1, Rgb, SUBTEXT0, SURFACE0, SURFACE1, TEXT};
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

/// A text input field inside a popup.
#[must_use]
pub const fn input() -> Style {
    Style::new().fg(color(TEXT)).bg(color(SURFACE1))
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
    fn input_is_text_on_surface1() {
        assert_eq!(colours(input()), (Some(color(TEXT)), Some(color(SURFACE1))));
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
}
