//! Incremental syntax highlighting of a window of lines.
//!
//! [`IncrementalHighlighter`] produces the same [`StyledSpan`]s as
//! [`highlight`](super::highlight), but only for a requested range of lines.
//! It keeps a copy of the text it last highlighted and the syntect parser and
//! highlighter state every [`CHECKPOINT_LINES`] lines, so a request resumes
//! from the nearest checkpoint instead of re-highlighting from the top. When
//! the text changes, only the checkpoints after the first changed line are
//! dropped.

use std::ops::Range;

use two_face::re_exports::syntect::{
    highlighting::{FontStyle, HighlightState, Highlighter, RangedHighlightIterator, Style},
    parsing::{ParseState, ScopeStack},
};

use super::{StyledSpan, plain_span};
use crate::palette::Rgb;

/// How many lines apart the cached highlighter checkpoints are.
pub const CHECKPOINT_LINES: usize = 64;

/// The parser and highlighter state before a line.
#[derive(Debug, Clone)]
struct Checkpoint {
    /// syntect parser state.
    parse: ParseState,
    /// syntect highlighter (style stack) state.
    highlight: HighlightState,
}

/// Highlights windows of lines, caching state so that a window near the
/// last one, or a window after an edit, does not re-highlight the whole text.
#[derive(Debug, Default)]
pub struct IncrementalHighlighter {
    /// The language token the checkpoints were computed for.
    language: String,
    /// A copy of the text last highlighted.
    text: String,
    /// The byte start of each line (`split_inclusive('\n')` piece) of `text`.
    line_starts: Vec<usize>,
    /// `checkpoints[k]` is the state before line `k * CHECKPOINT_LINES`;
    /// always a valid prefix for `text`.
    checkpoints: Vec<Checkpoint>,
}

impl IncrementalHighlighter {
    /// An empty highlighter with no cached state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The spans for `lines` (0-based line indices, end exclusive) of `text`,
    /// highlighted as `language` (a file-extension token, see
    /// [`crate::syntax::language_from_path`]).
    ///
    /// A line is a `split_inclusive('\n')` piece of `text`; `lines` is
    /// clamped to the line count. The returned spans have absolute byte
    /// ranges within `text` and tile exactly the bytes of the clamped lines,
    /// equal to the matching spans of [`highlight`](super::highlight). With
    /// no syntax for `language`, or on a parse error, the result is one plain
    /// span over those bytes.
    pub fn spans(&mut self, text: &str, language: &str, lines: Range<usize>) -> Vec<StyledSpan> {
        if self.language != language {
            self.reset(language);
        }
        if self.text != text {
            self.update_text(text);
        }

        let end = lines.end.min(self.line_starts.len());
        let start = lines.start.min(end);
        if start == end {
            return Vec::new();
        }
        let bytes = self.window_bytes(start..end);

        let syntaxes = crate::syntax::syntax_set();
        let Some(syntax) = syntaxes.find_syntax_by_extension(language) else {
            return vec![plain_span(bytes)];
        };

        let highlighter = Highlighter::new(crate::syntax::theme());
        let Self {
            text: stored,
            line_starts,
            checkpoints,
            ..
        } = self;

        let (mut parse, mut highlight, resume) = match checkpoints
            .len()
            .checked_sub(1)
            .map(|last| (start / CHECKPOINT_LINES).min(last))
            .and_then(|k| checkpoints.get(k).map(|c| (k, c)))
        {
            Some((k, checkpoint)) => (
                checkpoint.parse.clone(),
                checkpoint.highlight.clone(),
                k.saturating_mul(CHECKPOINT_LINES),
            ),
            None => (
                ParseState::new(syntax),
                HighlightState::new(&highlighter, ScopeStack::new()),
                0,
            ),
        };

        let mut spans = Vec::new();
        for index in resume..end {
            if index == checkpoints.len().saturating_mul(CHECKPOINT_LINES) {
                checkpoints.push(Checkpoint {
                    parse: parse.clone(),
                    highlight: highlight.clone(),
                });
            }
            let line_bytes = line_range(line_starts, stored.len(), index);
            let Some(line) = stored.get(line_bytes.clone()) else {
                return vec![plain_span(bytes)];
            };
            let Ok(ops) = parse.parse_line(line, syntaxes) else {
                checkpoints.truncate(index.div_ceil(CHECKPOINT_LINES));
                return vec![plain_span(bytes)];
            };
            let pieces = RangedHighlightIterator::new(&mut highlight, &ops, line, &highlighter);
            if index < start {
                pieces.for_each(drop);
            } else {
                spans.extend(pieces.map(|(style, _, range)| {
                    styled_span(
                        style,
                        line_bytes.start.saturating_add(range.start)
                            ..line_bytes.start.saturating_add(range.end),
                    )
                }));
            }
        }
        spans
    }

    /// Forget all cached state and switch to `language`.
    fn reset(&mut self, language: &str) {
        language.clone_into(&mut self.language);
        self.text.clear();
        self.line_starts.clear();
        self.checkpoints.clear();
    }

    /// Store `text`, dropping every checkpoint after the first line that
    /// differs from the stored copy.
    fn update_text(&mut self, text: &str) {
        let first_diff = self
            .text
            .bytes()
            .zip(text.bytes())
            .take_while(|(old, new)| old == new)
            .count();
        let changed_line = self
            .line_starts
            .partition_point(|&s| s <= first_diff)
            .saturating_sub(1);
        // Checkpoint k depends only on the lines before k * CHECKPOINT_LINES,
        // so it survives while that line is at or before the changed one.
        self.checkpoints
            .truncate(changed_line / CHECKPOINT_LINES + 1);

        self.text.clear();
        self.text.push_str(text);
        self.line_starts.clear();
        let mut offset = 0;
        for piece in text.split_inclusive('\n') {
            self.line_starts.push(offset);
            offset += piece.len();
        }
    }

    /// The byte range of the non-empty line range `lines` of the stored text.
    fn window_bytes(&self, lines: Range<usize>) -> Range<usize> {
        let first = line_range(&self.line_starts, self.text.len(), lines.start);
        let last = line_range(
            &self.line_starts,
            self.text.len(),
            lines.end.saturating_sub(1),
        );
        first.start..last.end
    }
}

/// The byte range of line `index`, given the line starts and the text length.
fn line_range(line_starts: &[usize], text_len: usize, index: usize) -> Range<usize> {
    let start = line_starts.get(index).copied().unwrap_or(text_len);
    let end = line_starts
        .get(index.saturating_add(1))
        .copied()
        .unwrap_or(text_len);
    start..end
}

/// A syntect style over `bytes`, mapped exactly as `highlight` maps it.
fn styled_span(style: Style, bytes: Range<usize>) -> StyledSpan {
    let fg = style.foreground;
    StyledSpan {
        bytes,
        fg: Rgb {
            r: fg.r,
            g: fg.g,
            b: fg.b,
        },
        background: None,
        italic: style.font_style.contains(FontStyle::ITALIC),
        underline: style.font_style.contains(FontStyle::UNDERLINE),
    }
}

#[cfg(test)]
mod tests {
    // `unwrap` is acceptable in test code: a panic on an unexpected `None`
    // is exactly the failure signal we want from a test.
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::highlight::highlight;

    /// The 50-line Rust template of `benches/highlight.rs`.
    const TEMPLATE: &str = r#"//! A synthetic module used for highlighting benchmarks.

use std::collections::HashMap;
use std::fmt;

/// A widget with a name and a weight.
#[derive(Debug, Clone, PartialEq)]
pub struct Widget {
    /// The display name.
    pub name: String,
    /// The weight in grams.
    pub weight: u32,
}

impl Widget {
    /// Create a new widget.
    pub fn new(name: &str, weight: u32) -> Self {
        Self {
            name: name.to_owned(),
            weight,
        }
    }

    /// Frobnicate the widget, doubling its weight.
    pub fn frobnicate(&mut self) {
        self.weight = self.weight.saturating_mul(2);
    }
}

impl fmt::Display for Widget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({}g)", self.name, self.weight)
    }
}

/// Count widgets by name.
pub fn tally(widgets: &[Widget]) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for w in widgets {
        // Bump the count for this name.
        *counts.entry(w.name.clone()).or_insert(0) += 1;
    }
    counts
}

fn main() {
    let mut w = Widget::new("sprocket", 0x2A);
    w.frobnicate();
    println!("{w} weighs {:.2} kg", f64::from(w.weight) / 1000.0);
}
"#;

    /// A `lines`-line Rust source built from [`TEMPLATE`].
    fn source(lines: usize) -> String {
        assert_eq!(TEMPLATE.lines().count(), 50);
        TEMPLATE.split_inclusive('\n').cycle().take(lines).collect()
    }

    /// The byte offset of the start of line `line` of `text`.
    fn line_start(text: &str, line: usize) -> usize {
        text.split_inclusive('\n').take(line).map(str::len).sum()
    }

    /// The spans of the full highlight of `text` that lie inside `lines`.
    fn expected(text: &str, language: &str, lines: Range<usize>) -> Vec<StyledSpan> {
        let start = line_start(text, lines.start);
        let end = line_start(text, lines.end);
        highlight(text, language)
            .into_iter()
            .filter(|s| s.bytes.start >= start && s.bytes.end <= end)
            .collect()
    }

    /// Assert `spans` tile exactly `lines` of `text`.
    fn assert_tiles(spans: &[StyledSpan], text: &str, lines: Range<usize>) {
        let mut next = line_start(text, lines.start);
        for span in spans {
            assert_eq!(span.bytes.start, next, "gap or overlap at {span:?}");
            next = span.bytes.end;
        }
        assert_eq!(next, line_start(text, lines.end));
    }

    #[test]
    fn matches_full_highlight_on_every_window() {
        let text = source(5_000);
        let mut hl = IncrementalHighlighter::new();
        for window in [0..50, 1_000..1_050, 4_950..5_000] {
            let spans = hl.spans(&text, "rs", window.clone());
            assert!(!spans.is_empty());
            assert_tiles(&spans, &text, window.clone());
            assert_eq!(spans, expected(&text, "rs", window));
        }
    }

    #[test]
    fn edit_invalidates_from_the_changed_line() {
        let text = source(5_000);
        let mut hl = IncrementalHighlighter::new();
        let before = hl.spans(&text, "rs", 3_000..3_001);
        assert_eq!(before, expected(&text, "rs", 3_000..3_001));

        let mut edited = text.clone();
        edited.insert_str(line_start(&text, 10), "/*");
        let after = hl.spans(&edited, "rs", 3_000..3_001);
        assert_eq!(after, expected(&edited, "rs", 3_000..3_001));
        assert_ne!(after, before, "line 3000 is now inside a comment");
    }

    #[test]
    fn edit_after_the_window_keeps_earlier_checkpoints() {
        let text = source(5_000);
        let mut hl = IncrementalHighlighter::new();
        let window = 1_000..1_050;
        let before = hl.spans(&text, "rs", window.clone());
        let count = hl.checkpoints.len();
        assert_eq!(count, 1_050_usize.div_ceil(CHECKPOINT_LINES));

        let mut edited = text.clone();
        edited.insert_str(line_start(&text, 4_000), "/*");
        let probe = hl.spans(&edited, "rs", window.clone());
        assert_eq!(hl.checkpoints.len(), count);
        assert_eq!(probe, before);

        // Requesting past the edit extends the checkpoints from the kept ones.
        let tail = hl.spans(&edited, "rs", 4_990..5_000);
        assert_eq!(tail, expected(&edited, "rs", 4_990..5_000));
    }

    #[test]
    fn language_change_resets() {
        let text = "[package]\nname = \"frext\"\n";
        let mut hl = IncrementalHighlighter::new();
        let rust = hl.spans(text, "rs", 0..2);
        assert_eq!(rust, expected(text, "rs", 0..2));

        let toml = hl.spans(text, "toml", 0..2);
        assert_eq!(hl.language, "toml");
        assert_eq!(toml, expected(text, "toml", 0..2));
        assert_ne!(toml, rust);
    }

    #[test]
    fn unknown_language_is_one_plain_span() {
        let text = "one\ntwo\nthree\n";
        let mut hl = IncrementalHighlighter::new();
        let spans = hl.spans(text, "definitelynotalang", 1..2);
        assert_eq!(spans, vec![plain_span(4..8)]);
        assert_eq!(spans[0].fg, Rgb::new(220, 220, 220));
    }

    #[test]
    fn empty_text_has_no_spans() {
        let mut hl = IncrementalHighlighter::new();
        assert!(hl.spans("", "rs", 0..10).is_empty());
        assert!(hl.spans("", "definitelynotalang", 0..10).is_empty());
    }

    #[test]
    fn window_past_the_end_is_clamped() {
        let text = source(100);
        let mut hl = IncrementalHighlighter::new();
        let spans = hl.spans(&text, "rs", 90..1_000);
        assert_tiles(&spans, &text, 90..100);
        assert_eq!(spans, expected(&text, "rs", 90..100));
        assert!(hl.spans(&text, "rs", 200..300).is_empty());

        // A final line without a newline is still a line.
        let text = "fn a() {}\nfn b() {}";
        let spans = hl.spans(text, "rs", 1..5);
        assert_tiles(&spans, text, 1..2);
        assert_eq!(spans, expected(text, "rs", 1..2));
    }

    #[test]
    fn crlf_text_matches_full_highlight() {
        let text = source(300).replace('\n', "\r\n");
        let mut hl = IncrementalHighlighter::new();
        for window in [0..50, 200..300] {
            let spans = hl.spans(&text, "rs", window.clone());
            assert_tiles(&spans, &text, window.clone());
            assert_eq!(spans, expected(&text, "rs", window));
        }
    }
}
