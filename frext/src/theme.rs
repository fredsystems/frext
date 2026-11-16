//! Catppuccin Mocha theme for egui.
//!
//! The palette values and semantic roles live in [`crate::palette`] as
//! frontend-neutral RGB; this module converts them to egui colours. Applying
//! them directly to `egui::Visuals` avoids pulling in a theme crate that lags
//! egui releases, keeping the dependency tree lean.

use eframe::egui::{self, Color32};

use crate::palette::{self, Rgb, Tint};

/// Convert a neutral palette colour to an egui colour.
#[must_use]
pub const fn color(rgb: Rgb) -> Color32 {
    Color32::from_rgb(rgb.r, rgb.g, rgb.b)
}

/// Convert a neutral translucent colour to an egui colour.
#[must_use]
pub fn tint(tint: Tint) -> Color32 {
    color(tint.color).linear_multiply(tint.opacity)
}

/// Apply the Catppuccin Mocha theme to the given egui context.
pub fn apply(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();

    visuals.override_text_color = Some(color(palette::TEXT));
    visuals.hyperlink_color = color(palette::ROSEWATER);
    visuals.faint_bg_color = color(palette::SURFACE0);
    // `extreme_bg_color` is what egui paints behind text-input widgets,
    // including the editor `TextEdit`. Use BASE so the editing surface
    // matches the surrounding panels rather than appearing as a darker
    // inset box.
    visuals.extreme_bg_color = color(palette::BASE);
    visuals.code_bg_color = color(palette::MANTLE);
    visuals.warn_fg_color = color(palette::RED);
    visuals.error_fg_color = color(palette::RED);

    visuals.window_fill = color(palette::BASE);
    visuals.panel_fill = color(palette::BASE);
    visuals.window_stroke.color = color(palette::SURFACE1);
    visuals.selection.bg_fill = tint(palette::selection());
    visuals.selection.stroke.color = color(palette::ROSEWATER);

    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_fill = color(palette::BASE);
    widgets.noninteractive.weak_bg_fill = color(palette::BASE);
    widgets.noninteractive.bg_stroke.color = color(palette::SURFACE0);
    widgets.noninteractive.fg_stroke.color = color(palette::SUBTEXT0);

    widgets.inactive.bg_fill = color(palette::SURFACE0);
    widgets.inactive.weak_bg_fill = color(palette::SURFACE0);
    widgets.inactive.bg_stroke.color = color(palette::SURFACE1);
    widgets.inactive.fg_stroke.color = color(palette::TEXT);

    widgets.hovered.bg_fill = color(palette::SURFACE1);
    widgets.hovered.weak_bg_fill = color(palette::SURFACE1);
    widgets.hovered.bg_stroke.color = color(palette::OVERLAY0);
    widgets.hovered.fg_stroke.color = color(palette::TEXT);

    widgets.active.bg_fill = color(palette::SURFACE2);
    widgets.active.weak_bg_fill = color(palette::SURFACE2);
    widgets.active.bg_stroke.color = color(palette::OVERLAY1);
    widgets.active.fg_stroke.color = color(palette::TEXT);

    widgets.open.bg_fill = color(palette::SURFACE1);
    widgets.open.weak_bg_fill = color(palette::SURFACE1);
    widgets.open.bg_stroke.color = color(palette::OVERLAY0);
    widgets.open.fg_stroke.color = color(palette::TEXT);

    ctx.set_visuals(visuals);
}

/// The accent colour used to mark the active tab.
#[must_use]
pub const fn accent() -> Color32 {
    color(palette::accent())
}

/// The dimmed colour used for the editor's line-number gutter.
#[must_use]
pub const fn gutter() -> Color32 {
    color(palette::gutter())
}

/// Background fill for a non-current search match.
#[must_use]
pub const fn search_match() -> Color32 {
    color(palette::search_match())
}

/// Background fill for the currently-focused search match.
#[must_use]
pub fn search_current_match() -> Color32 {
    tint(palette::search_current_match())
}

/// The squiggle under a misspelled word.
#[must_use]
pub const fn spelling_squiggle() -> Color32 {
    color(palette::spelling_squiggle())
}

/// The squiggle under a grammar (or other non-spelling) issue.
#[must_use]
pub const fn grammar_squiggle() -> Color32 {
    color(palette::grammar_squiggle())
}

/// The colour used for error text (e.g. an invalid search regex).
#[must_use]
pub const fn error() -> Color32 {
    color(palette::error())
}

#[cfg(test)]
mod tests {
    use super::*;

    const OLD_BLUE: Color32 = Color32::from_rgb(0x89, 0xb4, 0xfa);

    #[test]
    fn opaque_roles_are_pixel_identical_to_previous_values() {
        assert_eq!(accent(), OLD_BLUE);
        assert_eq!(gutter(), Color32::from_rgb(0x6c, 0x70, 0x86));
        assert_eq!(search_match(), Color32::from_rgb(0x58, 0x5b, 0x70));
        assert_eq!(spelling_squiggle(), Color32::from_rgb(0xf3, 0x8b, 0xa8));
        assert_eq!(grammar_squiggle(), Color32::from_rgb(0xa6, 0xe3, 0xa1));
        assert_eq!(error(), Color32::from_rgb(0xf3, 0x8b, 0xa8));
    }

    #[test]
    fn translucent_roles_are_pixel_identical_to_previous_values() {
        assert_eq!(search_current_match(), OLD_BLUE.linear_multiply(0.55));
        assert_eq!(tint(palette::selection()), OLD_BLUE.linear_multiply(0.35));
    }

    #[test]
    fn color_converts_channels_directly() {
        assert_eq!(
            color(Rgb::new(0x12, 0x34, 0x56)),
            Color32::from_rgb(0x12, 0x34, 0x56)
        );
    }
}
