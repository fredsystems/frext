//! Soft-wrapping a text into visual rows, and mapping a caret between a
//! char index and a (row, column) position.
//!
//! Each logical line is wrapped greedily to the layout width: a row breaks
//! after the last whitespace grapheme that fits (the whitespace stays on
//! the earlier row), otherwise before the grapheme that would overflow.
//! A row always takes at least one grapheme, so a single grapheme wider
//! than the width sits alone on its row.
//!
//! The char index at a soft-wrap boundary belongs to the start of the next
//! row; the "end" of a soft-wrapped row is the boundary before its last
//! grapheme.

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

use crate::buffer::cells;

/// What ends a visual row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowBreak {
    /// The row is the last of a logical line followed by `'\n'`.
    Newline,
    /// The row is soft-wrapped onto the next row.
    Wrap,
    /// The row is the last row of the text.
    EndOfText,
}

/// One visual row: the drawn content of (part of) one logical line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisualRow {
    /// The row's content as absolute char indices into the text. Never
    /// includes a `'\n'` or the `'\r'` right before one.
    pub chars: Range<usize>,
    /// The row's content as absolute byte offsets into the text.
    pub bytes: Range<usize>,
    /// The 0-based logical line the row belongs to.
    pub line: usize,
    /// What ends the row.
    pub break_after: RowBreak,
}

/// A text laid out into visual rows at a fixed wrap width.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrapLayout {
    rows: Vec<VisualRow>,
    width: u16,
}

/// A position within a text, as both a byte offset and a char index.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Mark {
    byte: usize,
    char: usize,
}

impl Mark {
    /// The position just after `grapheme`, which starts here.
    fn after(self, grapheme: &str) -> Self {
        Self {
            byte: self.byte.saturating_add(grapheme.len()),
            char: self.char.saturating_add(grapheme.chars().count()),
        }
    }
}

/// Whether `grapheme` is whitespace a row may break after. No-break
/// spaces (U+00A0, U+2007, U+202F) are not.
fn is_break_space(grapheme: &str) -> bool {
    !grapheme.is_empty()
        && grapheme
            .chars()
            .all(|c| c.is_whitespace() && !matches!(c, '\u{A0}' | '\u{2007}' | '\u{202F}'))
}

/// Builds the rows of one logical line.
struct LineWrapper<'a> {
    rows: &'a mut Vec<VisualRow>,
    origin: Mark,
    line: usize,
}

impl LineWrapper<'_> {
    /// Pushes the row from `start` to `end` (relative to the line).
    fn push(&mut self, start: Mark, end: Mark, break_after: RowBreak) {
        let char_at = |m: Mark| self.origin.char.saturating_add(m.char);
        let byte_at = |m: Mark| self.origin.byte.saturating_add(m.byte);
        let row = VisualRow {
            chars: char_at(start)..char_at(end),
            bytes: byte_at(start)..byte_at(end),
            line: self.line,
            break_after,
        };
        self.rows.push(row);
    }

    /// Wraps `content` to `width` cells, ending its last row with
    /// `last_break`.
    fn wrap(&mut self, content: &str, width: u16, last_break: RowBreak) {
        let max = u32::from(width);
        let mut start = Mark::default();
        let mut pos = Mark::default();
        let mut row_width: u32 = 0;
        // Just after the last break opportunity in the current row, and
        // the row's width up to it.
        let mut candidate: Option<(Mark, u32)> = None;
        for grapheme in content.graphemes(true) {
            let cells = u32::from(cells::width(grapheme));
            if row_width > 0 && row_width.saturating_add(cells) > max {
                if let Some((cut, cut_width)) = candidate.take() {
                    self.push(start, cut, RowBreak::Wrap);
                    start = cut;
                    row_width = row_width.saturating_sub(cut_width);
                }
                if row_width > 0 && row_width.saturating_add(cells) > max {
                    self.push(start, pos, RowBreak::Wrap);
                    start = pos;
                    row_width = 0;
                }
            }
            pos = pos.after(grapheme);
            row_width = row_width.saturating_add(cells);
            if is_break_space(grapheme) {
                candidate = Some((pos, row_width));
            }
        }
        self.push(start, pos, last_break);
    }
}

impl WrapLayout {
    /// Lays `text` out at `width` cells per row (0 is treated as 1).
    ///
    /// An empty text, or a text ending in `'\n'`, has a final empty row.
    #[must_use]
    pub fn new(text: &str, width: u16) -> Self {
        let width = width.max(1);
        let mut rows = Vec::new();
        let mut origin = Mark::default();
        let mut segments = text.split('\n').enumerate().peekable();
        while let Some((line, segment)) = segments.next() {
            let has_newline = segments.peek().is_some();
            let (content, break_after) = if has_newline {
                (
                    segment.strip_suffix('\r').unwrap_or(segment),
                    RowBreak::Newline,
                )
            } else {
                (segment, RowBreak::EndOfText)
            };
            let mut wrapper = LineWrapper {
                rows: &mut rows,
                origin,
                line,
            };
            wrapper.wrap(content, width, break_after);
            origin = origin.after(segment);
            if has_newline {
                origin = origin.after("\n");
            }
        }
        Self { rows, width }
    }

    /// The visual rows, in order. Never empty.
    #[must_use]
    pub fn rows(&self) -> &[VisualRow] {
        &self.rows
    }

    /// The wrap width in cells (at least 1).
    #[must_use]
    pub const fn width(&self) -> u16 {
        self.width
    }

    /// The row `row`, or the last row if `row` is past the end.
    fn row_or_last(&self, row: usize) -> Option<&VisualRow> {
        self.rows.get(row).or_else(|| self.rows.last())
    }

    /// The row the caret at `char_idx` is on: the last row whose
    /// `chars.start <= char_idx`. A soft-wrap boundary is on the next row.
    #[must_use]
    pub fn row_of_char(&self, char_idx: usize) -> usize {
        self.rows
            .partition_point(|row| row.chars.start <= char_idx)
            .saturating_sub(1)
    }

    /// The number of cells from the start of the caret's row to `char_idx`.
    #[must_use]
    pub fn column_of_char(&self, text: &str, char_idx: usize) -> u16 {
        let Some(row) = self.rows.get(self.row_of_char(char_idx)) else {
            return 0;
        };
        let mut pos = row.chars.start;
        let mut column: u16 = 0;
        for grapheme in row_content(text, row).graphemes(true) {
            let next = pos.saturating_add(grapheme.chars().count());
            if next > char_idx {
                break;
            }
            column = column.saturating_add(cells::width(grapheme));
            pos = next;
        }
        column
    }

    /// The caret position for a click at `column` on `row`: before the
    /// grapheme whose cells contain `column`, or [`Self::row_end`] past the
    /// row's content. A row past the end is the last row.
    #[must_use]
    pub fn char_at(&self, text: &str, row: usize, column: u16) -> usize {
        let Some(visual) = self.row_or_last(row) else {
            return 0;
        };
        let mut pos = visual.chars.start;
        let mut start_column: u16 = 0;
        for grapheme in row_content(text, visual).graphemes(true) {
            let end_column = start_column.saturating_add(cells::width(grapheme));
            if column < end_column {
                return pos;
            }
            start_column = end_column;
            pos = pos.saturating_add(grapheme.chars().count());
        }
        self.row_end(text, row)
    }

    /// The "end" of `row`: on a soft-wrapped row, the boundary before its
    /// last grapheme; otherwise the end of its content. A row past the end
    /// is the last row.
    #[must_use]
    pub fn row_end(&self, text: &str, row: usize) -> usize {
        let Some(visual) = self.row_or_last(row) else {
            return 0;
        };
        if visual.break_after != RowBreak::Wrap {
            return visual.chars.end;
        }
        let last = row_content(text, visual)
            .graphemes(true)
            .next_back()
            .map_or(0, |g| g.chars().count());
        visual.chars.end.saturating_sub(last)
    }
}

/// The drawn content of `row` within `text`.
fn row_content<'a>(text: &'a str, row: &VisualRow) -> &'a str {
    text.get(row.bytes.clone()).unwrap_or("")
}

#[cfg(test)]
mod tests {
    // `unwrap` is acceptable in test code: a panic on an unexpected `None`
    // is exactly the failure signal we want from a test.
    #![allow(clippy::unwrap_used)]

    use super::*;

    /// The drawn content of every row.
    fn contents(text: &str, width: u16) -> Vec<String> {
        let layout = WrapLayout::new(text, width);
        layout
            .rows()
            .iter()
            .map(|row| row_content(text, row).to_owned())
            .collect()
    }

    /// The cell width of `content`.
    fn cells_of(content: &str) -> u32 {
        content
            .graphemes(true)
            .map(|g| u32::from(cells::width(g)))
            .sum()
    }

    #[test]
    fn short_line_is_one_row() {
        let layout = WrapLayout::new("hello", 10);
        assert_eq!(
            layout.rows(),
            &[VisualRow {
                chars: 0..5,
                bytes: 0..5,
                line: 0,
                break_after: RowBreak::EndOfText,
            }]
        );
        assert_eq!(layout.width(), 10);
    }

    #[test]
    fn empty_text_is_one_empty_row_and_trailing_newline_adds_a_row() {
        let empty = WrapLayout::new("", 10);
        assert_eq!(
            empty.rows(),
            &[VisualRow {
                chars: 0..0,
                bytes: 0..0,
                line: 0,
                break_after: RowBreak::EndOfText,
            }]
        );
        let layout = WrapLayout::new("a\n", 10);
        assert_eq!(
            layout.rows(),
            &[
                VisualRow {
                    chars: 0..1,
                    bytes: 0..1,
                    line: 0,
                    break_after: RowBreak::Newline,
                },
                VisualRow {
                    chars: 2..2,
                    bytes: 2..2,
                    line: 1,
                    break_after: RowBreak::EndOfText,
                },
            ]
        );
    }

    #[test]
    fn break_after_space_keeps_space_on_first_row() {
        assert_eq!(contents("hello world", 8), vec!["hello ", "world"]);
        let layout = WrapLayout::new("hello world", 8);
        assert_eq!(layout.rows()[0].break_after, RowBreak::Wrap);
        assert_eq!(layout.rows()[1].break_after, RowBreak::EndOfText);
    }

    #[test]
    fn long_word_breaks_at_width() {
        assert_eq!(contents("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn wide_char_that_does_not_fit_moves_to_next_row() {
        assert_eq!(contents("abc漢", 4), vec!["abc", "漢"]);
        // A grapheme wider than the width sits alone on its row.
        assert_eq!(contents("a漢b", 1), vec!["a", "漢", "b"]);
    }

    #[test]
    fn tab_counts_as_four_cells() {
        assert_eq!(contents("a\tbcd", 5), vec!["a\t", "bcd"]);
        assert_eq!(contents("ab\tc", 5), vec!["ab", "\tc"]);
    }

    #[test]
    fn crlf_carriage_return_is_not_in_row_content() {
        let text = "ab\r\ncd";
        let layout = WrapLayout::new(text, 10);
        assert_eq!(contents(text, 10), vec!["ab", "cd"]);
        assert_eq!(layout.rows()[0].chars, 0..2);
        assert_eq!(layout.rows()[1].chars, 4..6);
        assert_eq!(layout.rows()[1].bytes, 4..6);
        assert_eq!(layout.rows()[1].line, 1);
    }

    #[test]
    fn nbsp_is_not_a_break_opportunity() {
        assert_eq!(contents("ab\u{A0}cd ef", 5), vec!["ab\u{A0}cd", " ef"]);
        assert_eq!(contents("ab\u{202F}cdef", 5), vec!["ab\u{202F}cd", "ef"]);
    }

    #[test]
    fn every_row_is_within_width_over_template() {
        let text = template();
        for width in [1, 7, 39, 79] {
            let layout = WrapLayout::new(&text, width);
            let mut next_char = 0;
            for row in layout.rows() {
                let content = row_content(&text, row);
                assert!(
                    cells_of(content) <= u32::from(width),
                    "{content:?} > {width}"
                );
                assert!(!content.contains('\n'));
                assert!(row.chars.start >= next_char);
                assert_eq!(
                    row.chars.end - row.chars.start,
                    content.chars().count(),
                    "char and byte ranges disagree"
                );
                next_char = row.chars.end;
            }
            assert_eq!(
                layout
                    .rows()
                    .iter()
                    .filter(|r| r.break_after != RowBreak::Wrap)
                    .count(),
                frext_core::lines::line_count(&text)
            );
        }
    }

    #[test]
    fn first_row_of_line_agrees_with_core_line_numbers() {
        let text = template();
        for width in [7, 39] {
            let layout = WrapLayout::new(&text, width);
            let rows = layout.rows();
            let expected: Vec<(usize, usize)> = rows
                .iter()
                .enumerate()
                .filter(|(i, row)| *i == 0 || rows[i - 1].line != row.line)
                .map(|(i, row)| (i, row.line + 1))
                .collect();
            let numbered = frext_core::lines::line_numbers_for_rows(
                rows.iter().map(|r| r.break_after == RowBreak::Newline),
            );
            assert_eq!(numbered, expected);
        }
    }

    #[test]
    fn row_of_char_at_soft_wrap_boundary_is_next_row() {
        let layout = WrapLayout::new("hello world", 8);
        assert_eq!(layout.row_of_char(5), 0);
        assert_eq!(layout.row_of_char(6), 1);
        assert_eq!(layout.column_of_char("hello world", 6), 0);
        assert_eq!(layout.row_of_char(11), 1);
    }

    #[test]
    fn column_and_char_at_round_trip_on_every_boundary() {
        let text = "a漢b\tc \u{1F468}\u{200D}\u{1F469} e\u{301}f\nxy 漢漢漢 z\n\nend";
        for width in [1, 3, 4, 7, 40] {
            let layout = WrapLayout::new(text, width);
            let mut boundaries: Vec<usize> = text
                .grapheme_indices(true)
                .map(|(b, _)| frext_core::offsets::char_of_byte(text, b))
                .collect();
            boundaries.push(text.chars().count());
            for char_idx in boundaries {
                let row = layout.row_of_char(char_idx);
                let column = layout.column_of_char(text, char_idx);
                assert_eq!(
                    layout.char_at(text, row, column),
                    char_idx,
                    "width {width}, char {char_idx}, row {row}, column {column}"
                );
            }
        }
    }

    #[test]
    fn char_at_past_content() {
        let text = "hello world\nab";
        let layout = WrapLayout::new(text, 8);
        // Row 0 "hello " is soft-wrapped: its end is before the space.
        assert_eq!(layout.char_at(text, 0, 50), 5);
        assert_eq!(layout.row_end(text, 0), 5);
        // Row 1 "world" ends with a newline: its end is the content end.
        assert_eq!(layout.char_at(text, 1, 50), 11);
        assert_eq!(layout.row_end(text, 1), 11);
        // Row 2 is the last row; a row past the end is the last row.
        assert_eq!(layout.char_at(text, 2, 50), 14);
        assert_eq!(layout.char_at(text, 99, 0), 12);
        assert_eq!(layout.char_at(text, 99, 50), 14);
        // A click inside a wide char lands before it.
        let wide = "a漢";
        let layout = WrapLayout::new(wide, 10);
        assert_eq!(layout.char_at(wide, 0, 2), 1);
        assert_eq!(layout.char_at(wide, 0, 3), 2);
    }

    #[test]
    fn width_zero_does_not_panic() {
        let text = "ab 漢\tc\n";
        let layout = WrapLayout::new(text, 0);
        assert_eq!(layout.width(), 1);
        assert!(!layout.rows().is_empty());
        let row = layout.row_of_char(3);
        let column = layout.column_of_char(text, 3);
        assert_eq!(layout.char_at(text, row, column), 3);
        let _ = layout.row_end(text, 100);
    }

    /// The 50-line Rust template from `benches/wrap.rs`.
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

    /// [`TEMPLATE`] repeated 4 times.
    fn template() -> String {
        TEMPLATE.repeat(4)
    }
}
