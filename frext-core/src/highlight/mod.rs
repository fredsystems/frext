//! Frontend-neutral syntax highlighting into styled byte spans.
//!
//! [`highlight`] runs syntect (with the syntaxes and theme from
//! [`crate::syntax`]) over a buffer and produces [`StyledSpan`]s: byte ranges
//! with a plain [`Rgb`] foreground and font-style flags. [`overlay_matches`]
//! then splits those spans so search matches carry a [`MatchBackground`]
//! role. Each front end converts the spans and roles into its own text and
//! colour types.
//!
//! [`incremental::IncrementalHighlighter`] produces the same spans for a
//! window of lines, resuming from cached checkpoints instead of
//! re-highlighting the whole buffer.

pub mod incremental;

use std::ops::Range;

use two_face::re_exports::syntect::{
    easy::HighlightLines, highlighting::FontStyle, util::LinesWithEndings,
};

use crate::palette::Rgb;

/// Foreground of the unhighlighted plain-text fallback: a neutral light grey.
pub(crate) const PLAIN_FG: Rgb = Rgb::new(220, 220, 220);

/// The background role of a search match.
///
/// This is a role, not a colour: each front end maps it to its own colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchBackground {
    /// A match that is not the currently-focused one.
    Other,
    /// The currently-focused match.
    Current,
}

/// A run of text sharing one style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyledSpan {
    /// Byte range of the run within the highlighted text.
    pub bytes: Range<usize>,
    /// Foreground colour.
    pub fg: Rgb,
    /// Search-match background role, if the run is part of a match.
    pub background: Option<MatchBackground>,
    /// Whether the run is italic.
    pub italic: bool,
    /// Whether the run is underlined.
    pub underline: bool,
}

/// Search matches to paint over the syntax highlighting: every match range
/// gets a background fill, and the `current` match (if any) gets a brighter
/// one.
#[derive(Debug, Clone, Default)]
pub struct MatchHighlights {
    /// Byte ranges of all matches in the buffer.
    pub ranges: Vec<Range<usize>>,
    /// Index into `ranges` of the currently-focused match, if any.
    pub current: Option<usize>,
}

/// Highlight `text` for the syntax identified by `language` (a file-extension
/// token, see [`crate::syntax::language_from_path`]).
///
/// Spans are emitted in order and tile the text. Falls back to a single
/// unstyled span covering the whole text when no syntax matches or
/// highlighting fails part-way through.
#[must_use]
pub fn highlight(text: &str, language: &str) -> Vec<StyledSpan> {
    highlight_impl(text, language).unwrap_or_else(|| vec![plain_span(0..text.len())])
}

/// The unstyled plain-text fallback span over `bytes`.
pub(crate) fn plain_span(bytes: Range<usize>) -> StyledSpan {
    StyledSpan {
        bytes,
        fg: PLAIN_FG,
        background: None,
        italic: false,
        underline: false,
    }
}

/// The fallible highlighting core: `None` signals "no syntax / parse error",
/// at which point the caller renders plain text.
fn highlight_impl(text: &str, language: &str) -> Option<Vec<StyledSpan>> {
    let syntaxes = crate::syntax::syntax_set();
    let syntax = syntaxes.find_syntax_by_extension(language)?;

    let mut lines = HighlightLines::new(syntax, crate::syntax::theme());
    let mut spans = Vec::new();

    for line in LinesWithEndings::from(text) {
        for (style, range) in lines.highlight_line(line, syntaxes).ok()? {
            let fg = style.foreground;
            spans.push(StyledSpan {
                bytes: as_byte_range(text, range),
                fg: Rgb {
                    r: fg.r,
                    g: fg.g,
                    b: fg.b,
                },
                background: None,
                italic: style.font_style.contains(FontStyle::ITALIC),
                underline: style.font_style.contains(FontStyle::UNDERLINE),
            });
        }
    }

    Some(spans)
}

/// Translate a syntect string slice back into a byte range within `whole`.
///
/// syntect yields sub-`&str`s that point into the line being highlighted,
/// which itself points into `whole`; pointer arithmetic recovers the offset.
/// The casts here are pointer-to-address comparisons, not lossy numeric
/// conversions, and the offset is provably in-bounds because `range` is a
/// slice of `whole`.
fn as_byte_range(whole: &str, range: &str) -> Range<usize> {
    let whole_start = whole.as_ptr() as usize;
    let range_start = range.as_ptr() as usize;
    let offset = range_start - whole_start;
    offset..offset + range.len()
}

/// Paint search-match background roles onto already-highlighted `spans`.
///
/// Each span that overlaps a match is split so the matched bytes carry a
/// [`MatchBackground`] role; the current match uses
/// [`MatchBackground::Current`]. Spans stay sorted and non-overlapping. With
/// no matches the spans are returned unchanged.
#[must_use]
pub fn overlay_matches(spans: Vec<StyledSpan>, matches: &MatchHighlights) -> Vec<StyledSpan> {
    if matches.ranges.is_empty() {
        return spans;
    }

    let current = matches.current.and_then(|i| matches.ranges.get(i)).cloned();

    // Background role for byte `b`, if it falls inside any match.
    let bg_at = |b: usize| -> Option<MatchBackground> {
        if let Some(cur) = &current
            && cur.contains(&b)
        {
            return Some(MatchBackground::Current);
        }
        if matches.ranges.iter().any(|r| r.contains(&b)) {
            Some(MatchBackground::Other)
        } else {
            None
        }
    };

    let mut out = Vec::new();
    for span in spans {
        split_span_by_background(&mut out, span, &bg_at);
    }
    out
}

/// Re-emit `span`, split at every byte where the search-match background role
/// changes, so matched runs carry a `background` and unmatched runs do not.
fn split_span_by_background(
    out: &mut Vec<StyledSpan>,
    span: StyledSpan,
    bg_at: &impl Fn(usize) -> Option<MatchBackground>,
) {
    let start = span.bytes.start;
    let end = span.bytes.end;
    if start >= end {
        out.push(span);
        return;
    }

    let mut run_start = start;
    let mut run_bg = bg_at(start);
    for b in (start + 1)..end {
        let bg = bg_at(b);
        if bg != run_bg {
            out.push(sub_span(&span, run_start, b, run_bg));
            run_start = b;
            run_bg = bg;
        }
    }
    out.push(sub_span(&span, run_start, end, run_bg));
}

/// Clone `span`'s style over the byte range `start..end`, setting (or
/// clearing) the background role.
fn sub_span(
    span: &StyledSpan,
    start: usize,
    end: usize,
    background: Option<MatchBackground>,
) -> StyledSpan {
    StyledSpan {
        bytes: start..end,
        fg: span.fg,
        background,
        italic: span.italic,
        underline: span.underline,
    }
}

#[cfg(test)]
mod tests {
    // `expect` is acceptable in test code: a panic on an unexpected `None`
    // is exactly the failure signal we want from a test.
    #![allow(clippy::expect_used)]

    use super::*;

    /// A TOML buffer must produce highlighted spans rather than a single
    /// plain fallback span.
    #[test]
    fn toml_is_highlighted_into_multiple_spans() {
        let spans = highlight("[package]\nname = \"frext\"\n", "toml");
        assert!(
            spans.len() > 1,
            "TOML should highlight into multiple styled spans, got {}",
            spans.len()
        );
    }

    #[test]
    fn unknown_extension_falls_back_to_plain_text() {
        let spans = highlight("just some text", "definitelynotalang");
        // The plain fallback is a single span covering the whole buffer.
        assert_eq!(
            spans,
            vec![StyledSpan {
                bytes: 0.."just some text".len(),
                fg: Rgb::new(220, 220, 220),
                background: None,
                italic: false,
                underline: false,
            }]
        );
    }

    #[test]
    fn empty_text_with_a_known_language_yields_no_spans() {
        assert!(highlight("", "rs").is_empty());
    }

    #[test]
    fn empty_text_with_an_unknown_language_yields_one_empty_span() {
        let spans = highlight("", "definitelynotalang");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].bytes, 0..0);
    }

    #[test]
    fn highlighted_spans_tile_the_text_contiguously() {
        let text = "fn main() {\n    let x = \"hi\"; // greet\n    println!(\"{x}\");\n}\n";
        let spans = highlight(text, "rs");
        assert!(spans.len() > 1);
        let mut next = 0;
        for span in &spans {
            assert_eq!(span.bytes.start, next, "gap or overlap at {span:?}");
            next = span.bytes.end;
        }
        assert_eq!(next, text.len());
    }

    /// Build the plain (single-span) highlighting for `text`, used by the
    /// match-highlight tests below.
    fn plain_spans(text: &str) -> Vec<StyledSpan> {
        highlight(text, "definitelynotalang")
    }

    /// Spans that carry a background role, as `(start, end)` byte pairs.
    fn highlighted_ranges(spans: &[StyledSpan]) -> Vec<(usize, usize)> {
        spans
            .iter()
            .filter(|s| s.background.is_some())
            .map(|s| (s.bytes.start, s.bytes.end))
            .collect()
    }

    #[test]
    fn no_matches_leaves_the_spans_untouched() {
        let spans = plain_spans("hello world");
        let after = overlay_matches(spans.clone(), &MatchHighlights::default());
        assert_eq!(after, spans);
        assert!(highlighted_ranges(&after).is_empty());
    }

    #[test]
    fn matches_get_background_runs_covering_exactly_the_match_bytes() {
        let matches = MatchHighlights {
            ranges: vec![0..3, 8..11],
            current: None,
        };
        let spans = overlay_matches(plain_spans("foo bar foo"), &matches);

        // Every original byte is still covered (spans remain contiguous).
        let total: usize = spans.iter().map(|s| s.bytes.end - s.bytes.start).sum();
        assert_eq!(total, "foo bar foo".len());

        // The highlighted runs line up with the match byte ranges.
        assert_eq!(highlighted_ranges(&spans), vec![(0, 3), (8, 11)]);
    }

    #[test]
    fn current_match_uses_a_distinct_background() {
        let matches = MatchHighlights {
            ranges: vec![0..3, 4..7],
            current: Some(1),
        };
        let spans = overlay_matches(plain_spans("foo foo"), &matches);

        let current_span = spans
            .iter()
            .find(|s| s.bytes.start == 4)
            .expect("a span starting at the current match");
        let other_span = spans
            .iter()
            .find(|s| s.bytes.start == 0)
            .expect("a span starting at the other match");

        assert_eq!(current_span.background, Some(MatchBackground::Current));
        assert_eq!(other_span.background, Some(MatchBackground::Other));
    }
}
