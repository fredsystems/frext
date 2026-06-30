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
//! Emoji are handled by discovering the host's emoji font at startup (via
//! [`fontdb`]) and slotting it into the family chains right after
//! CaskaydiaCove. egui's per-codepoint fallback walks each family's font list
//! in order and uses the first font that has a glyph for the codepoint;
//! CaskaydiaCove does **not** cover the emoji codepoints (it declines
//! `U+1F600` and friends), so they fall through to the system emoji font while
//! CaskaydiaCove still wins every glyph it does carry (text, ligatures, Nerd
//! Font icons). When no usable system emoji font is found, egui's bundled
//! `NotoEmoji` (already last in each chain) remains as a backstop.
//!
//! **egui can only draw monochrome (outline) emoji.** Its rasteriser ignores a
//! font's colour tables (COLR/CPAL, CBDT/CBLC, sbix). A colour-bitmap font such
//! as *Noto Color Emoji* therefore rasterises to an empty glyph that still
//! occupies advance width — i.e. emoji that "take up space but show nothing".
//! Discovery guards against exactly this by **rasterising a probe emoji through
//! each candidate and only accepting a font whose glyph is actually drawable**,
//! so a colour-only font installed ahead of a monochrome one is skipped rather
//! than silently producing blank glyphs. True colour emoji would require
//! rendering text outside egui's text system — a separate, much larger
//! investigation.
//!
//! No emoji font is bundled — only the host's is used — so there is nothing
//! extra to attribute for it.
//!
//! Licensing: the bundled CaskaydiaCove face is under the SIL Open Font
//! License 1.1; see `assets/fonts/CaskaydiaCove-NerdFont-LICENSE.md` and
//! `ATTRIBUTIONS.md`.

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};

/// The family name frext registers the bundled regular face under.
const CASKAYDIA: &str = "CaskaydiaCove Nerd Font";

/// The font-data key the discovered system emoji face is registered under.
const SYSTEM_EMOJI: &str = "frext-system-emoji";

/// The bundled CaskaydiaCove regular face, embedded into the binary.
const REGULAR: &[u8] = include_bytes!("../../assets/fonts/CaskaydiaCoveNerdFont-Regular.ttf");

/// A probe emoji (grinning face) whose glyph is checked for a drawable outline
/// when validating a candidate emoji font.
const PROBE_EMOJI: char = '\u{1F600}';

/// System emoji font family names to try, in order of preference. Matched as a
/// substring against each face's reported families.
///
/// Monochrome/outline fonts are listed first because egui can only draw those;
/// colour-bitmap fonts (e.g. *Noto Color Emoji*) are still listed — a host may
/// only have one — but are accepted only if the outline check passes, which
/// for a pure colour-bitmap font it will not. The check, not this order, is
/// what ultimately guarantees a usable font; the order just expresses
/// preference when several usable fonts exist.
const EMOJI_CANDIDATES: &[&str] = &[
    "Noto Emoji",
    "OpenMoji",
    "Twemoji Mozilla",
    "Symbola",
    "Apple Color Emoji",
    "Segoe UI Emoji",
    "Noto Color Emoji",
    "Twemoji",
    "Emoji One",
    "Emoji",
];

/// Build the [`FontDefinitions`] that install the bundled regular face as the
/// monospace primary and a proportional fallback, then slot a discovered
/// system emoji font in just after it.
///
/// Split out from [`apply`] so the bundled-font wiring can be unit-tested
/// without an [`egui::Context`].
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

    add_system_emoji(&mut defs);

    defs
}

/// Discover the host's emoji font and slot it into the family chains right
/// after CaskaydiaCove, so emoji codepoints (which CaskaydiaCove declines)
/// resolve to it ahead of egui's smaller bundled emoji font.
///
/// Best-effort: if no candidate font is found or its bytes cannot be read,
/// this logs and leaves the definitions unchanged (egui's bundled `NotoEmoji`
/// then remains the emoji backstop).
fn add_system_emoji(defs: &mut FontDefinitions) {
    let Some(bytes) = discover_system_emoji_bytes() else {
        log::info!("no system emoji font found; using egui's bundled emoji fallback");
        return;
    };

    defs.font_data
        .insert(SYSTEM_EMOJI.to_owned(), FontData::from_owned(bytes).into());

    for family in [FontFamily::Monospace, FontFamily::Proportional] {
        if let Some(list) = defs.families.get_mut(&family) {
            insert_after(list, CASKAYDIA, SYSTEM_EMOJI);
        }
    }
}

/// Discover a usable system emoji font: the first candidate (in
/// [`EMOJI_CANDIDATES`] preference order) that has a **drawable outline** for
/// [`PROBE_EMOJI`]. Returns its bytes, or `None` when nothing usable is
/// installed.
///
/// The outline check is the crucial step. A colour-bitmap font such as *Noto
/// Color Emoji* maps the codepoint but stores it as a CBDT/sbix bitmap with no
/// outline; egui cannot decode that and would draw a blank glyph that still
/// occupies advance width. Requiring an outline skips such fonts so emoji are
/// never invisible-but-spaced.
fn discover_system_emoji_bytes() -> Option<Vec<u8>> {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();

    for candidate in EMOJI_CANDIDATES {
        for face in db.faces() {
            let matches = face
                .families
                .iter()
                .any(|(name, _)| name.contains(candidate));
            if !matches {
                continue;
            }
            let fontdb::Source::File(path) = &face.source else {
                continue;
            };
            let bytes = match std::fs::read(path) {
                Ok(bytes) => bytes,
                Err(err) => {
                    log::warn!("failed to read emoji font {}: {err}", path.display());
                    continue;
                }
            };
            if font_has_drawable_emoji(&bytes, face.index) {
                log::info!("using system emoji font: {candidate} ({})", path.display());
                return Some(bytes);
            }
            log::debug!(
                "skipping emoji font {candidate} ({}): no drawable outline for the probe \
                 emoji (likely a colour-bitmap font egui cannot render)",
                path.display()
            );
        }
    }
    None
}

/// Whether the face in `font_bytes` at `face_index` has a glyph for
/// [`PROBE_EMOJI`] with a non-empty **outline** — the only thing egui can
/// rasterise.
///
/// `glyph_index` returning `None` means the codepoint is unmapped (tofu);
/// `glyph_bounding_box` returning `None` means the glyph has no outline extent
/// (e.g. a colour-bitmap-only glyph). Both are rejected; only a glyph with a
/// real outline bounding box counts as drawable.
fn font_has_drawable_emoji(font_bytes: &[u8], face_index: u32) -> bool {
    let Ok(face) = ttf_parser::Face::parse(font_bytes, face_index) else {
        return false;
    };
    let Some(glyph_id) = face.glyph_index(PROBE_EMOJI) else {
        return false;
    };
    face.glyph_bounding_box(glyph_id).is_some()
}

/// Insert `name` immediately after the first occurrence of `anchor` in `list`.
/// If `anchor` is absent, append `name` to the end. A no-op when `name` is
/// already present, so repeated calls stay idempotent.
///
/// Split out as a pure function so the fallback ordering — the part that
/// decides whether emoji resolve to the system font before egui's bundled one
/// — is unit-testable without touching the filesystem or an egui context.
fn insert_after(list: &mut Vec<String>, anchor: &str, name: &str) {
    if list.iter().any(|f| f == name) {
        return;
    }
    match list.iter().position(|f| f == anchor) {
        Some(i) => list.insert(i + 1, name.to_owned()),
        None => list.push(name.to_owned()),
    }
}

/// Install the bundled CaskaydiaCove font (and a discovered system emoji
/// fallback) into the given egui context.
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
    use harfrust::{Feature, ShapeOptions, ShaperData, Tag, UnicodeBuffer};

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
        let glyphs = shaper.shape(buffer, ShapeOptions::new().features(&off));
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

    #[test]
    fn insert_after_places_name_directly_after_the_anchor() {
        let mut list = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
        insert_after(&mut list, "a", "emoji");
        assert_eq!(list, vec!["a", "emoji", "b", "c"]);
    }

    #[test]
    fn insert_after_appends_when_anchor_is_absent() {
        let mut list = vec!["x".to_owned(), "y".to_owned()];
        insert_after(&mut list, "missing", "emoji");
        assert_eq!(list, vec!["x", "y", "emoji"]);
    }

    #[test]
    fn insert_after_is_idempotent() {
        let mut list = vec!["a".to_owned(), "emoji".to_owned(), "b".to_owned()];
        insert_after(&mut list, "a", "emoji");
        assert_eq!(
            list,
            vec!["a", "emoji", "b"],
            "an already-present name must not be inserted twice"
        );
    }

    /// The emoji fallback must sit immediately after CaskaydiaCove in both
    /// families: ahead of egui's Ubuntu/Hack/NotoEmoji so emoji codepoints
    /// (which CaskaydiaCove declines) resolve to the system font, but behind
    /// CaskaydiaCove so it keeps winning every glyph it covers.
    #[test]
    fn add_system_emoji_orders_fallback_right_after_caskaydia() {
        let mut defs = FontDefinitions::default();
        // Mirror the primary wiring `definitions` does before the emoji step.
        defs.font_data
            .insert(CASKAYDIA.to_owned(), FontData::from_static(REGULAR).into());
        if let Some(mono) = defs.families.get_mut(&FontFamily::Monospace) {
            mono.insert(0, CASKAYDIA.to_owned());
        }
        if let Some(prop) = defs.families.get_mut(&FontFamily::Proportional) {
            prop.push(CASKAYDIA.to_owned());
        }

        // Drive the ordering directly rather than through filesystem discovery
        // (which is host-dependent): register a stand-in emoji face and place
        // it with the same helper `add_system_emoji` uses.
        defs.font_data.insert(
            SYSTEM_EMOJI.to_owned(),
            FontData::from_static(REGULAR).into(),
        );
        for family in [FontFamily::Monospace, FontFamily::Proportional] {
            if let Some(list) = defs.families.get_mut(&family) {
                insert_after(list, CASKAYDIA, SYSTEM_EMOJI);
            }
        }

        for family in [FontFamily::Monospace, FontFamily::Proportional] {
            let list = defs.families.get(&family).expect("family exists");
            let caskaydia = list
                .iter()
                .position(|f| f == CASKAYDIA)
                .expect("CaskaydiaCove present");
            let emoji = list
                .iter()
                .position(|f| f == SYSTEM_EMOJI)
                .expect("system emoji present");
            assert_eq!(
                emoji,
                caskaydia + 1,
                "emoji fallback must sit immediately after CaskaydiaCove in {family:?}: {list:?}"
            );
        }
    }

    /// Negative control: CaskaydiaCove has no emoji glyph at all, so the
    /// outline check must reject it. This proves the check actually inspects
    /// the glyph rather than always returning true.
    #[test]
    fn outline_check_rejects_a_font_without_the_emoji() {
        assert!(
            !font_has_drawable_emoji(REGULAR, 0),
            "CaskaydiaCove has no grinning-face glyph, so the check must reject it"
        );
    }

    /// Host-tolerant integration check. Discovery is best-effort and depends on
    /// the machine's installed fonts, so this asserts a conditional invariant:
    /// *if* a system emoji font is discovered, it must have a drawable outline
    /// for the probe emoji, be wired into the monospace family right after
    /// CaskaydiaCove, and render a non-empty glyph through the assembled
    /// definitions. On a machine with no usable emoji font discovery returns
    /// `None` and the test trivially passes.
    #[test]
    fn discovered_system_emoji_is_drawable_and_wired_in() {
        if discover_system_emoji_bytes().is_none() {
            // No usable emoji font on this host; nothing to assert.
            return;
        }

        // The discovered font must end up in the monospace chain just after
        // CaskaydiaCove.
        let defs = definitions();
        let mono = defs
            .families
            .get(&FontFamily::Monospace)
            .expect("monospace family exists");
        let caskaydia = mono
            .iter()
            .position(|f| f == CASKAYDIA)
            .expect("CaskaydiaCove present");
        assert_eq!(
            mono.get(caskaydia + 1).map(String::as_str),
            Some(SYSTEM_EMOJI),
            "the discovered emoji font must sit right after CaskaydiaCove: {mono:?}"
        );

        // Sanity: the whole assembled definition renders the probe emoji.
        let mut fonts = Fonts::new(TextOptions::default(), definitions());
        let galley = fonts.with_pixels_per_point(1.0).layout_no_wrap(
            PROBE_EMOJI.to_string(),
            FontId::monospace(16.0),
            Color32::WHITE,
        );
        let drawable = galley.rows.iter().any(|row| {
            row.row
                .glyphs
                .iter()
                .any(|g| g.chr == PROBE_EMOJI && g.uv_rect.size.x > 0.0 && g.uv_rect.size.y > 0.0)
        });
        assert!(
            drawable,
            "the assembled font definition must render the probe emoji"
        );
    }
}
