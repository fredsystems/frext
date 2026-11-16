//! Catppuccin Mocha as plain RGB plus semantic roles, frontend-neutral.
//!
//! The palette is taken from the official Catppuccin Mocha specification
//! (<https://github.com/catppuccin/catppuccin>). Colours are stored as plain
//! [`Rgb`] triples so any frontend (egui, a terminal, ...) can convert them
//! into its own colour type.

/// An opaque 24-bit RGB colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgb {
    /// Red channel.
    pub r: u8,
    /// Green channel.
    pub g: u8,
    /// Blue channel.
    pub b: u8,
}

impl Rgb {
    /// Build a colour from its red, green and blue channels.
    #[must_use]
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

/// A translucent colour.
///
/// `opacity` is in `0.0..=1.0`. egui applies it with `linear_multiply`; a
/// terminal frontend blends it over the background to an opaque colour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tint {
    /// The base colour.
    pub color: Rgb,
    /// The opacity, in `0.0..=1.0`.
    pub opacity: f32,
}

/// Mocha Rosewater.
pub const ROSEWATER: Rgb = Rgb::new(0xf5, 0xe0, 0xdc);
/// Mocha Blue.
pub const BLUE: Rgb = Rgb::new(0x89, 0xb4, 0xfa);
/// Mocha Text.
pub const TEXT: Rgb = Rgb::new(0xcd, 0xd6, 0xf4);
/// Mocha Subtext 0.
pub const SUBTEXT0: Rgb = Rgb::new(0xa6, 0xad, 0xc8);
/// Mocha Overlay 1.
pub const OVERLAY1: Rgb = Rgb::new(0x7f, 0x84, 0x9c);
/// Mocha Overlay 0.
pub const OVERLAY0: Rgb = Rgb::new(0x6c, 0x70, 0x86);
/// Mocha Surface 2.
pub const SURFACE2: Rgb = Rgb::new(0x58, 0x5b, 0x70);
/// Mocha Surface 1.
pub const SURFACE1: Rgb = Rgb::new(0x45, 0x47, 0x5a);
/// Mocha Surface 0.
pub const SURFACE0: Rgb = Rgb::new(0x31, 0x32, 0x44);
/// Mocha Base.
pub const BASE: Rgb = Rgb::new(0x1e, 0x1e, 0x2e);
/// Mocha Mantle.
pub const MANTLE: Rgb = Rgb::new(0x18, 0x18, 0x25);
/// Mocha Red.
pub const RED: Rgb = Rgb::new(0xf3, 0x8b, 0xa8);
/// Mocha Green.
pub const GREEN: Rgb = Rgb::new(0xa6, 0xe3, 0xa1);

/// The accent colour used to mark the active tab.
#[must_use]
pub const fn accent() -> Rgb {
    BLUE
}

/// The dimmed colour used for the editor's line-number gutter.
#[must_use]
pub const fn gutter() -> Rgb {
    OVERLAY0
}

/// Background fill for a non-current search match.
#[must_use]
pub const fn search_match() -> Rgb {
    // A muted surface tint so every match is visible without overpowering the
    // syntax-highlighted foreground.
    SURFACE2
}

/// Background fill for the currently-focused search match.
#[must_use]
pub const fn search_current_match() -> Tint {
    // The accent, dimmed so dark foreground glyphs stay legible on top.
    Tint {
        color: BLUE,
        opacity: 0.55,
    }
}

/// The editor's selection fill.
#[must_use]
pub const fn selection() -> Tint {
    Tint {
        color: BLUE,
        opacity: 0.35,
    }
}

/// The squiggle under a misspelled word.
#[must_use]
pub const fn spelling_squiggle() -> Rgb {
    RED
}

/// The squiggle under a grammar (or other non-spelling) issue.
#[must_use]
pub const fn grammar_squiggle() -> Rgb {
    GREEN
}

/// The colour used for error text (e.g. an invalid search regex).
#[must_use]
pub const fn error() -> Rgb {
    RED
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(rgb: Rgb) -> String {
        format!("#{:02x}{:02x}{:02x}", rgb.r, rgb.g, rgb.b)
    }

    #[test]
    fn constants_match_published_mocha_values() {
        assert_eq!(hex(ROSEWATER), "#f5e0dc");
        assert_eq!(hex(BLUE), "#89b4fa");
        assert_eq!(hex(TEXT), "#cdd6f4");
        assert_eq!(hex(SUBTEXT0), "#a6adc8");
        assert_eq!(hex(OVERLAY1), "#7f849c");
        assert_eq!(hex(OVERLAY0), "#6c7086");
        assert_eq!(hex(SURFACE2), "#585b70");
        assert_eq!(hex(SURFACE1), "#45475a");
        assert_eq!(hex(SURFACE0), "#313244");
        assert_eq!(hex(BASE), "#1e1e2e");
        assert_eq!(hex(MANTLE), "#181825");
        assert_eq!(hex(RED), "#f38ba8");
        assert_eq!(hex(GREEN), "#a6e3a1");
    }

    #[test]
    fn opaque_roles_map_to_expected_constants() {
        assert_eq!(accent(), BLUE);
        assert_eq!(gutter(), OVERLAY0);
        assert_eq!(search_match(), SURFACE2);
        assert_eq!(spelling_squiggle(), RED);
        assert_eq!(grammar_squiggle(), GREEN);
        assert_eq!(error(), RED);
    }

    #[test]
    fn search_current_match_is_translucent_blue() {
        let tint = search_current_match();
        assert_eq!(tint.color, BLUE);
        assert_eq!(tint.opacity.to_bits(), 0.55_f32.to_bits());
    }

    #[test]
    fn selection_is_translucent_blue() {
        let tint = selection();
        assert_eq!(tint.color, BLUE);
        assert_eq!(tint.opacity.to_bits(), 0.35_f32.to_bits());
    }
}
