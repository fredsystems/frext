//! Catppuccin Mocha theme for egui.
//!
//! The palette is taken from the official Catppuccin Mocha specification
//! (<https://github.com/catppuccin/catppuccin>). Applying it directly to
//! `egui::Visuals` avoids pulling in a theme crate that lags egui releases,
//! keeping the dependency tree lean.

use eframe::egui::{self, Color32};

/// A subset of the Catppuccin Mocha palette used by frext.
mod mocha {
    use eframe::egui::Color32;

    pub const ROSEWATER: Color32 = Color32::from_rgb(0xf5, 0xe0, 0xdc);
    pub const BLUE: Color32 = Color32::from_rgb(0x89, 0xb4, 0xfa);
    pub const TEXT: Color32 = Color32::from_rgb(0xcd, 0xd6, 0xf4);
    pub const SUBTEXT0: Color32 = Color32::from_rgb(0xa6, 0xad, 0xc8);
    pub const OVERLAY1: Color32 = Color32::from_rgb(0x7f, 0x84, 0x9c);
    pub const OVERLAY0: Color32 = Color32::from_rgb(0x6c, 0x70, 0x86);
    pub const SURFACE2: Color32 = Color32::from_rgb(0x58, 0x5b, 0x70);
    pub const SURFACE1: Color32 = Color32::from_rgb(0x45, 0x47, 0x5a);
    pub const SURFACE0: Color32 = Color32::from_rgb(0x31, 0x32, 0x44);
    pub const BASE: Color32 = Color32::from_rgb(0x1e, 0x1e, 0x2e);
    pub const MANTLE: Color32 = Color32::from_rgb(0x18, 0x18, 0x25);
    pub const RED: Color32 = Color32::from_rgb(0xf3, 0x8b, 0xa8);
}

/// Apply the Catppuccin Mocha theme to the given egui context.
pub fn apply(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();

    visuals.override_text_color = Some(mocha::TEXT);
    visuals.hyperlink_color = mocha::ROSEWATER;
    visuals.faint_bg_color = mocha::SURFACE0;
    // `extreme_bg_color` is what egui paints behind text-input widgets,
    // including the editor `TextEdit`. Use BASE so the editing surface
    // matches the surrounding panels rather than appearing as a darker
    // inset box.
    visuals.extreme_bg_color = mocha::BASE;
    visuals.code_bg_color = mocha::MANTLE;
    visuals.warn_fg_color = mocha::RED;
    visuals.error_fg_color = mocha::RED;

    visuals.window_fill = mocha::BASE;
    visuals.panel_fill = mocha::BASE;
    visuals.window_stroke.color = mocha::SURFACE1;
    visuals.selection.bg_fill = mocha::BLUE.linear_multiply(0.35);
    visuals.selection.stroke.color = mocha::ROSEWATER;

    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_fill = mocha::BASE;
    widgets.noninteractive.weak_bg_fill = mocha::BASE;
    widgets.noninteractive.bg_stroke.color = mocha::SURFACE0;
    widgets.noninteractive.fg_stroke.color = mocha::SUBTEXT0;

    widgets.inactive.bg_fill = mocha::SURFACE0;
    widgets.inactive.weak_bg_fill = mocha::SURFACE0;
    widgets.inactive.bg_stroke.color = mocha::SURFACE1;
    widgets.inactive.fg_stroke.color = mocha::TEXT;

    widgets.hovered.bg_fill = mocha::SURFACE1;
    widgets.hovered.weak_bg_fill = mocha::SURFACE1;
    widgets.hovered.bg_stroke.color = mocha::OVERLAY0;
    widgets.hovered.fg_stroke.color = mocha::TEXT;

    widgets.active.bg_fill = mocha::SURFACE2;
    widgets.active.weak_bg_fill = mocha::SURFACE2;
    widgets.active.bg_stroke.color = mocha::OVERLAY1;
    widgets.active.fg_stroke.color = mocha::TEXT;

    widgets.open.bg_fill = mocha::SURFACE1;
    widgets.open.weak_bg_fill = mocha::SURFACE1;
    widgets.open.bg_stroke.color = mocha::OVERLAY0;
    widgets.open.fg_stroke.color = mocha::TEXT;

    ctx.set_visuals(visuals);
}

/// The accent colour used to mark the active tab.
#[must_use]
pub const fn accent() -> Color32 {
    mocha::BLUE
}

/// The dimmed colour used for the editor's line-number gutter.
#[must_use]
pub const fn gutter() -> Color32 {
    mocha::OVERLAY0
}
