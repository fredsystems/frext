//! egui text layout for the editor surface.
//!
//! Converts the frontend-neutral [`StyledSpan`]s produced by
//! [`crate::highlight`] into an egui [`LayoutJob`], mapping palette colours
//! and search-match background roles through [`crate::theme`], and provides
//! the [`layouter`] closure handed to [`egui::TextEdit::layouter`].

use eframe::egui::{
    self, Color32, FontId, Stroke, TextStyle,
    text::{ByteIndex, LayoutJob, LayoutSection, TextFormat},
};

use crate::highlight::{MatchBackground, MatchHighlights, StyledSpan, highlight, overlay_matches};

/// Build a `layouter` closure suitable for [`egui::TextEdit::layouter`].
///
/// The closure highlights the buffer for `language` (a file-extension token,
/// see [`crate::syntax::language_from_path`]) and lays the result out into a
/// [`Galley`]. When the extension does not resolve to a known syntax the text
/// is laid out unhighlighted, so the editor always renders. Any `matches` are
/// painted on top with a background fill so search hits stand out.
///
/// [`Galley`]: egui::Galley
pub fn layouter<'a>(
    language: &'a str,
    matches: &'a MatchHighlights,
) -> impl FnMut(&egui::Ui, &dyn egui::TextBuffer, f32) -> std::sync::Arc<egui::Galley> + 'a {
    move |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
        let font_id = ui
            .style()
            .override_font_id
            .clone()
            .unwrap_or_else(|| TextStyle::Monospace.resolve(ui.style()));
        let mut job = layout_job(buf.as_str(), language, font_id, matches);
        job.wrap.max_width = wrap_width;
        ui.fonts_mut(|f| f.layout_job(job))
    }
}

/// Build the styled [`LayoutJob`] for `text` without laying it out.
///
/// Highlights `text` for `language` (a file-extension token, see
/// [`crate::syntax::language_from_path`]) in `font_id`, then paints any search
/// `matches` on top as background fills. This is exactly what [`layouter`]
/// feeds to egui, minus the wrap width and galley layout, so it can be tested
/// and benchmarked without a UI context.
#[must_use]
pub fn layout_job(
    text: &str,
    language: &str,
    font_id: FontId,
    matches: &MatchHighlights,
) -> LayoutJob {
    let sections = overlay_matches(highlight(text, language), matches)
        .into_iter()
        .map(|span| section(span, &font_id))
        .collect();
    LayoutJob {
        text: text.into(),
        sections,
        ..Default::default()
    }
}

/// Convert one styled span into an egui layout section in `font_id`.
fn section(span: StyledSpan, font_id: &FontId) -> LayoutSection {
    let color = crate::theme::color(span.fg);
    LayoutSection {
        leading_space: 0.0,
        byte_range: ByteIndex(span.bytes.start)..ByteIndex(span.bytes.end),
        format: TextFormat {
            font_id: font_id.clone(),
            color,
            italics: span.italic,
            underline: if span.underline {
                Stroke::new(1.0, color)
            } else {
                Stroke::NONE
            },
            background: match span.background {
                None => Color32::TRANSPARENT,
                Some(MatchBackground::Other) => crate::theme::search_match(),
                Some(MatchBackground::Current) => crate::theme::search_current_match(),
            },
            ..Default::default()
        },
    }
}

#[cfg(test)]
mod tests {
    use std::ops::Range;

    use super::*;

    /// The plain fallback must be exactly the job `LayoutJob::simple` built
    /// before highlighting was split from egui layout.
    #[test]
    fn plain_fallback_matches_layout_job_simple() {
        for text in ["just some text\nsecond line", ""] {
            let job = layout_job(
                text,
                "definitelynotalang",
                FontId::monospace(12.0),
                &MatchHighlights::default(),
            );
            let simple = LayoutJob::simple(
                text.into(),
                FontId::monospace(12.0),
                Color32::LIGHT_GRAY,
                f32::INFINITY,
            );
            assert_eq!(job, simple, "fallback job differs for {text:?}");
        }
    }

    const GOLDEN_SNIPPET: &str = concat!(
        "/// Adds two numbers.\n",
        "fn add(a: i32, b: i32) -> i32 {\n",
        "    // sum them\n",
        "    let label = \"total\";\n",
        "    a + b + 42\n",
        "}\n",
    );

    /// Characterisation test pinning the exact output of [`layout_job`] for a
    /// small Rust snippet with two search matches -- `numbers` (13..20) and
    /// the current match `total";` (87..94, excluding the opening quote) --
    /// so any change to highlighting or match painting is caught
    /// pixel-for-pixel.
    #[test]
    fn layout_job_matches_golden_sections() {
        let matches = MatchHighlights {
            // `numbers` inside the doc comment (13..20), and `total";`
            // (87..94, current; the opening quote at 86 is outside the
            // match), which spans several syntax sections.
            ranges: vec![13..20, 87..94],
            current: Some(1),
        };
        let job = layout_job(GOLDEN_SNIPPET, "rs", FontId::monospace(12.0), &matches);

        assert_eq!(job.text, GOLDEN_SNIPPET);

        let actual: Vec<(Range<usize>, Color32, bool, Stroke, Color32)> = job
            .sections
            .iter()
            .map(|s| {
                (
                    s.byte_range.start.0..s.byte_range.end.0,
                    s.format.color,
                    s.format.italics,
                    s.format.underline,
                    s.format.background,
                )
            })
            .collect();
        let match_bg = Color32::from_rgba_premultiplied(88, 91, 112, 255);
        let current_bg = Color32::from_rgba_premultiplied(75, 99, 138, 140);
        let expected: Vec<(Range<usize>, Color32, bool, Stroke, Color32)> = vec![
            (
                0..3,
                Color32::from_rgb(147, 153, 178),
                true,
                Stroke::NONE,
                Color32::TRANSPARENT,
            ), // "///"
            (
                3..13,
                Color32::from_rgb(147, 153, 178),
                true,
                Stroke::NONE,
                Color32::TRANSPARENT,
            ), // " Adds two "
            (
                13..20,
                Color32::from_rgb(147, 153, 178),
                true,
                Stroke::NONE,
                match_bg,
            ), // "numbers"
            (
                20..22,
                Color32::from_rgb(147, 153, 178),
                true,
                Stroke::NONE,
                Color32::TRANSPARENT,
            ), // ".\n"
            (
                22..24,
                Color32::from_rgb(203, 166, 247),
                false,
                Stroke::NONE,
                Color32::TRANSPARENT,
            ), // "fn"
            (
                24..25,
                Color32::from_rgb(205, 214, 244),
                false,
                Stroke::NONE,
                Color32::TRANSPARENT,
            ), // " "
            (
                25..28,
                Color32::from_rgb(137, 180, 250),
                true,
                Stroke::NONE,
                Color32::TRANSPARENT,
            ), // "add"
            (
                28..29,
                Color32::from_rgb(147, 153, 178),
                false,
                Stroke::NONE,
                Color32::TRANSPARENT,
            ), // "("
            (
                29..30,
                Color32::from_rgb(235, 160, 172),
                true,
                Stroke::NONE,
                Color32::TRANSPARENT,
            ), // "a"
            (
                30..31,
                Color32::from_rgb(147, 153, 178),
                false,
                Stroke::NONE,
                Color32::TRANSPARENT,
            ), // ":"
            (
                31..32,
                Color32::from_rgb(235, 160, 172),
                true,
                Stroke::NONE,
                Color32::TRANSPARENT,
            ), // " "
            (
                32..35,
                Color32::from_rgb(203, 166, 247),
                false,
                Stroke::NONE,
                Color32::TRANSPARENT,
            ), // "i32"
            (
                35..37,
                Color32::from_rgb(235, 160, 172),
                true,
                Stroke::NONE,
                Color32::TRANSPARENT,
            ), // ", "
            (
                37..38,
                Color32::from_rgb(235, 160, 172),
                true,
                Stroke::NONE,
                Color32::TRANSPARENT,
            ), // "b"
            (
                38..39,
                Color32::from_rgb(147, 153, 178),
                false,
                Stroke::NONE,
                Color32::TRANSPARENT,
            ), // ":"
            (
                39..40,
                Color32::from_rgb(235, 160, 172),
                true,
                Stroke::NONE,
                Color32::TRANSPARENT,
            ), // " "
            (
                40..43,
                Color32::from_rgb(203, 166, 247),
                false,
                Stroke::NONE,
                Color32::TRANSPARENT,
            ), // "i32"
            (
                43..44,
                Color32::from_rgb(147, 153, 178),
                false,
                Stroke::NONE,
                Color32::TRANSPARENT,
            ), // ")"
            (
                44..45,
                Color32::from_rgb(205, 214, 244),
                false,
                Stroke::NONE,
                Color32::TRANSPARENT,
            ), // " "
            (
                45..47,
                Color32::from_rgb(147, 153, 178),
                false,
                Stroke::NONE,
                Color32::TRANSPARENT,
            ), // "->"
            (
                47..48,
                Color32::from_rgb(205, 214, 244),
                false,
                Stroke::NONE,
                Color32::TRANSPARENT,
            ), // " "
            (
                48..51,
                Color32::from_rgb(203, 166, 247),
                false,
                Stroke::NONE,
                Color32::TRANSPARENT,
            ), // "i32"
            (
                51..52,
                Color32::from_rgb(205, 214, 244),
                false,
                Stroke::NONE,
                Color32::TRANSPARENT,
            ), // " "
            (
                52..53,
                Color32::from_rgb(147, 153, 178),
                false,
                Stroke::NONE,
                Color32::TRANSPARENT,
            ), // "{"
            (
                53..54,
                Color32::from_rgb(205, 214, 244),
                false,
                Stroke::NONE,
                Color32::TRANSPARENT,
            ), // "\n"
            (
                54..58,
                Color32::from_rgb(205, 214, 244),
                false,
                Stroke::NONE,
                Color32::TRANSPARENT,
            ), // "    "
            (
                58..60,
                Color32::from_rgb(147, 153, 178),
                true,
                Stroke::NONE,
                Color32::TRANSPARENT,
            ), // "//"
            (
                60..70,
                Color32::from_rgb(147, 153, 178),
                true,
                Stroke::NONE,
                Color32::TRANSPARENT,
            ), // " sum them\n"
            (
                70..74,
                Color32::from_rgb(205, 214, 244),
                false,
                Stroke::NONE,
                Color32::TRANSPARENT,
            ), // "    "
            (
                74..77,
                Color32::from_rgb(203, 166, 247),
                false,
                Stroke::NONE,
                Color32::TRANSPARENT,
            ), // "let"
            (
                77..84,
                Color32::from_rgb(205, 214, 244),
                false,
                Stroke::NONE,
                Color32::TRANSPARENT,
            ), // " label "
            (
                84..85,
                Color32::from_rgb(148, 226, 213),
                false,
                Stroke::NONE,
                Color32::TRANSPARENT,
            ), // "="
            (
                85..86,
                Color32::from_rgb(205, 214, 244),
                false,
                Stroke::NONE,
                Color32::TRANSPARENT,
            ), // " "
            (
                86..87,
                Color32::from_rgb(166, 227, 161),
                false,
                Stroke::NONE,
                Color32::TRANSPARENT,
            ), // "\""
            (
                87..92,
                Color32::from_rgb(166, 227, 161),
                false,
                Stroke::NONE,
                current_bg,
            ), // "total"
            (
                92..93,
                Color32::from_rgb(166, 227, 161),
                false,
                Stroke::NONE,
                current_bg,
            ), // "\""
            (
                93..94,
                Color32::from_rgb(147, 153, 178),
                false,
                Stroke::NONE,
                current_bg,
            ), // ";"
            (
                94..95,
                Color32::from_rgb(205, 214, 244),
                false,
                Stroke::NONE,
                Color32::TRANSPARENT,
            ), // "\n"
            (
                95..101,
                Color32::from_rgb(205, 214, 244),
                false,
                Stroke::NONE,
                Color32::TRANSPARENT,
            ), // "    a "
            (
                101..102,
                Color32::from_rgb(148, 226, 213),
                false,
                Stroke::NONE,
                Color32::TRANSPARENT,
            ), // "+"
            (
                102..105,
                Color32::from_rgb(205, 214, 244),
                false,
                Stroke::NONE,
                Color32::TRANSPARENT,
            ), // " b "
            (
                105..106,
                Color32::from_rgb(148, 226, 213),
                false,
                Stroke::NONE,
                Color32::TRANSPARENT,
            ), // "+"
            (
                106..107,
                Color32::from_rgb(205, 214, 244),
                false,
                Stroke::NONE,
                Color32::TRANSPARENT,
            ), // " "
            (
                107..109,
                Color32::from_rgb(250, 179, 135),
                false,
                Stroke::NONE,
                Color32::TRANSPARENT,
            ), // "42"
            (
                109..110,
                Color32::from_rgb(205, 214, 244),
                false,
                Stroke::NONE,
                Color32::TRANSPARENT,
            ), // "\n"
            (
                110..111,
                Color32::from_rgb(147, 153, 178),
                false,
                Stroke::NONE,
                Color32::TRANSPARENT,
            ), // "}"
            (
                111..112,
                Color32::from_rgb(205, 214, 244),
                false,
                Stroke::NONE,
                Color32::TRANSPARENT,
            ), // "\n"
        ];
        assert_eq!(actual, expected);
    }
}
