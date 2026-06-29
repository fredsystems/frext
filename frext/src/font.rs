//! Bundled editor font, embedded at compile time.
//!
//! frext ships the Nerd Fonts-patched **CaskaydiaCove** (Cascadia Code)
//! typeface so the editor has coding ligatures (`->`, `=>`, `!=`, …) and a
//! broad glyph set out of the box, rather than relying on egui's stock
//! Ubuntu-Light fonts which carry no coding ligatures and limited symbol
//! coverage.
//!
//! Ligatures come for free: egui shapes text through its layout pass and
//! applies a font's `calt`/`liga` glyph substitutions when laying out a
//! galley. Because the editor and the line-number gutter both resolve
//! [`egui::TextStyle::Monospace`] from the active style (see
//! [`crate::highlight`] and `app::FrextApp::line_number_gutter`), installing
//! CaskaydiaCove as the monospace family makes ligatures appear with no
//! per-character processing on frext's side — egui caches one galley per text
//! block.
//!
//! Only the regular face is bundled: the editor's highlighter paints with a
//! single [`egui::FontId`] and synthesises slant via the `italics` flag and
//! weight via egui's faux-bold, so dedicated bold/italic faces would be dead
//! weight in the binary today. Keeping to one face also serves frext's
//! small-binary goal.
//!
//! Color emoji is **not** addressed here: egui's rasteriser ignores a font's
//! colour tables (COLR/CPAL, CBDT/sbix), so no bundled font can yield colour
//! emoji through egui's text system. That is tracked as a separate
//! investigation.
//!
//! Licensing: the bundled faces are under the SIL Open Font License 1.1; see
//! `assets/fonts/CaskaydiaCove-NerdFont-LICENSE.md` and `ATTRIBUTIONS.md`.

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};

/// The family name frext registers the bundled regular face under.
const CASKAYDIA: &str = "CaskaydiaCove Nerd Font";

/// The bundled CaskaydiaCove regular face, embedded into the binary.
const REGULAR: &[u8] = include_bytes!("../../assets/fonts/CaskaydiaCoveNerdFont-Regular.ttf");

/// Build the [`FontDefinitions`] that install the bundled regular face as both
/// the monospace primary and a proportional fallback.
///
/// Split out from [`apply`] so it can be unit-tested without an
/// [`egui::Context`].
fn definitions() -> FontDefinitions {
    let mut defs = FontDefinitions::default();

    defs.font_data
        .insert(CASKAYDIA.to_owned(), FontData::from_static(REGULAR).into());

    // Monospace primary: the editor and gutter resolve `TextStyle::Monospace`,
    // so this is the face that carries the editor's ligatures and glyphs.
    if let Some(mono) = defs.families.get_mut(&FontFamily::Monospace) {
        mono.insert(0, CASKAYDIA.to_owned());
    }

    // Also offer it as a proportional fallback so the surrounding chrome (tab
    // bar, sidebar) can borrow its wider glyph coverage when egui's default
    // proportional face lacks a glyph.
    if let Some(prop) = defs.families.get_mut(&FontFamily::Proportional) {
        prop.push(CASKAYDIA.to_owned());
    }

    defs
}

/// Install the bundled CaskaydiaCove font into the given egui context.
///
/// Call once during startup, alongside [`crate::theme::apply`].
pub fn apply(ctx: &egui::Context) {
    ctx.set_fonts(definitions());
}

#[cfg(test)]
mod tests {
    // `expect`/`unwrap` are acceptable in tests: a panic is exactly the
    // failure signal we want when an embedded asset is missing or malformed.
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;
    use eframe::egui::{
        Color32, FontFamily, FontId,
        epaint::text::{Fonts, TextOptions},
    };
    use harfrust::{Feature, ShaperData, Tag, UnicodeBuffer};

    /// The embedded face must be non-empty. A truncated/zero-length copy is
    /// the most likely asset accident; a malformed TTF is caught instead when
    /// [`editor_monospace_renders_ligature_glyph`] constructs epaint's
    /// [`Fonts`] (it panics on a bad face), and when [`shaped_glyph_ids`]
    /// parses it with `harfrust`.
    #[test]
    fn embedded_face_is_non_empty() {
        assert!(!REGULAR.is_empty(), "regular face is empty");
    }

    /// The bundled regular face must be wired in as the monospace primary, so
    /// the editor and gutter pick it up, and offered as a proportional
    /// fallback for the surrounding chrome.
    #[test]
    fn definitions_install_caskaydia_as_monospace_primary() {
        let defs = definitions();

        assert!(
            defs.font_data.contains_key(CASKAYDIA),
            "font data must be registered under {CASKAYDIA}"
        );

        let mono = defs
            .families
            .get(&FontFamily::Monospace)
            .expect("monospace family exists");
        assert_eq!(
            mono.first().map(String::as_str),
            Some(CASKAYDIA),
            "CaskaydiaCove must be the monospace primary"
        );

        let prop = defs
            .families
            .get(&FontFamily::Proportional)
            .expect("proportional family exists");
        assert!(
            prop.iter().any(|f| f == CASKAYDIA),
            "CaskaydiaCove must be a proportional fallback"
        );
    }

    /// Shape `text` against the embedded regular face with `harfrust` (the
    /// same shaper egui/epaint 0.35 uses internally) and return the resulting
    /// glyph ids. `enable_ligatures` toggles the `calt`/`liga` features.
    fn shaped_glyph_ids(text: &str, enable_ligatures: bool) -> Vec<u32> {
        let font = harfrust::FontRef::from_index(REGULAR, 0).expect("regular face parses");
        let data = ShaperData::new(&font);
        let shaper = data.shaper(&font).build();

        let mut buffer = UnicodeBuffer::new();
        buffer.push_str(text);
        buffer.guess_segment_properties();

        // With ligatures disabled, explicitly turn off every contextual/standard
        // ligature feature so the baseline is the unligated glyph run. With them
        // enabled, pass no overrides: harfrust turns `calt`/`liga` on by default
        // (the same defaults egui shapes with), so this mirrors real rendering.
        let value = u32::from(enable_ligatures);
        let off = [
            Feature::new(Tag::new(b"calt"), value, ..),
            Feature::new(Tag::new(b"liga"), value, ..),
            Feature::new(Tag::new(b"clig"), value, ..),
            Feature::new(Tag::new(b"rclt"), value, ..),
        ];
        let glyphs = shaper.shape(buffer, &off);
        glyphs
            .glyph_infos()
            .iter()
            .map(|info| info.glyph_id)
            .collect()
    }

    /// The decisive ligature regression test, mirroring freminal's
    /// `bundled_font_forms_ligatures`.
    ///
    /// Cascadia Code (and thus CaskaydiaCove) implements coding ligatures via
    /// `calt` *contextual substitution into ligature-piece glyphs*: the glyph
    /// **count and advances stay the same** (two equal-width cells for `->`),
    /// but the glyph **ids change** to the joined-looking pieces. So the
    /// reliable signal is that the shaped glyph ids differ with ligatures on
    /// versus off. If a future swap drops to a non-ligating variant (e.g.
    /// CaskaydiaMono), the ids stop changing and this fails.
    #[test]
    fn bundled_font_forms_ligatures() {
        for ligature in ["->", "=>", "!=", "==", ">=", "<="] {
            let on = shaped_glyph_ids(ligature, true);
            let off = shaped_glyph_ids(ligature, false);
            assert_ne!(
                on, off,
                "{ligature:?}: ligatures must substitute different glyph ids \
                 (on={on:?}, off={off:?}) — bundled font is not ligating"
            );
        }
    }

    /// A plain character must not be altered by ligature shaping — the control
    /// that proves [`bundled_font_forms_ligatures`] is detecting a real
    /// substitution and not noise.
    #[test]
    fn plain_text_is_unaffected_by_ligatures() {
        for text in ["x", "ab", "let"] {
            assert_eq!(
                shaped_glyph_ids(text, true),
                shaped_glyph_ids(text, false),
                "{text:?}: non-ligating text must shape identically on and off"
            );
        }
    }

    /// End-to-end: laying the ligature out through egui's *monospace family*
    /// (the family the editor and gutter resolve) must produce a different
    /// rasterised glyph than the unligated baseline. This proves the bundled
    /// font is actually installed as the monospace face and that egui draws
    /// the ligature glyph — not just that the font *could* ligate in isolation.
    ///
    /// epaint preserves `glyphs.len() == char_count`, so the observable change
    /// is the rasterised glyph size ([`UvRect::size`]) of the second cell: the
    /// ligature-piece `>` is drawn differently from a standalone `>`.
    #[test]
    fn editor_monospace_renders_ligature_glyph() {
        let mut fonts = Fonts::new(TextOptions::default(), definitions());

        let glyph_sizes = |fonts: &mut Fonts, text: &str| -> Vec<[f32; 2]> {
            let galley = fonts.with_pixels_per_point(1.0).layout_no_wrap(
                text.to_owned(),
                FontId::monospace(14.0),
                Color32::WHITE,
            );
            galley
                .rows
                .iter()
                .flat_map(|row| {
                    row.row
                        .glyphs
                        .iter()
                        .map(|g| [g.uv_rect.size.x, g.uv_rect.size.y])
                })
                .collect()
        };

        // `x>` shares its second character with `->` but cannot ligate, so its
        // `>` is the standalone glyph. `->` ligates, so its `>` cell is the
        // ligature piece and must rasterise to a different size.
        let plain = glyph_sizes(&mut fonts, "x>");
        let ligated = glyph_sizes(&mut fonts, "->");

        assert_eq!(plain.len(), 2);
        assert_eq!(ligated.len(), 2);
        assert_ne!(
            plain[1], ligated[1],
            "the `>` in `->` must rasterise as the ligature piece, \
             differing from a standalone `>` (plain={plain:?}, ligated={ligated:?})"
        );
    }
}
