//! The caret and selection of a buffer, and the motions that move them.
//!
//! A [`Cursor`] is an anchor and a caret, both char indices, plus the
//! preferred column (in cells) that vertical motion keeps across shorter
//! rows. Left/Right step by extended grapheme cluster of the whole text,
//! so a caret never rests inside `"\r\n"` or a ZWJ sequence; vertical and
//! Home/End motion work on the visual rows of a [`WrapLayout`].

use std::ops::Range;

use frext_core::offsets::{byte_of_char, char_of_byte};
use unicode_segmentation::GraphemeCursor;

use crate::buffer::words::{next_word_boundary, previous_word_boundary};
use crate::buffer::wrap::WrapLayout;

/// A caret motion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    /// One grapheme left, or collapse a selection to its start.
    Left,
    /// One grapheme right, or collapse a selection to its end.
    Right,
    /// To the previous word boundary.
    WordLeft,
    /// To the next word boundary.
    WordRight,
    /// One visual row up.
    Up,
    /// One visual row down.
    Down,
    /// To the start of the caret's visual row.
    RowStart,
    /// To the end of the caret's visual row.
    RowEnd,
    /// One page of visual rows up.
    PageUp,
    /// One page of visual rows down.
    PageDown,
    /// To the start of the text.
    DocStart,
    /// To the end of the text.
    DocEnd,
}

/// Whether a motion moves the whole cursor or extends the selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selecting {
    /// Move the caret and drop any selection.
    Move,
    /// Move the caret and keep the anchor, extending the selection.
    Extend,
}

/// An anchor and a caret (char indices), and the preferred column kept by
/// vertical motion.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Cursor {
    anchor: usize,
    caret: usize,
    preferred_column: Option<u16>,
}

impl Cursor {
    /// A cursor with no selection at `char_idx`.
    #[must_use]
    pub const fn at(char_idx: usize) -> Self {
        Self {
            anchor: char_idx,
            caret: char_idx,
            preferred_column: None,
        }
    }

    /// A cursor selecting from `anchor` to `caret`.
    #[must_use]
    pub const fn spanning(anchor: usize, caret: usize) -> Self {
        Self {
            anchor,
            caret,
            preferred_column: None,
        }
    }

    /// The caret: the end of the selection that moves.
    #[must_use]
    pub const fn caret(&self) -> usize {
        self.caret
    }

    /// The anchor: the end of the selection that stays put.
    #[must_use]
    pub const fn anchor(&self) -> usize {
        self.anchor
    }

    /// The selected chars, ordered (empty when there is no selection).
    #[must_use]
    pub fn selection(&self) -> Range<usize> {
        self.anchor.min(self.caret)..self.anchor.max(self.caret)
    }

    /// Whether any chars are selected.
    #[must_use]
    pub const fn has_selection(&self) -> bool {
        self.anchor != self.caret
    }

    /// Clamps both ends to the char count of `text` and snaps them down to
    /// grapheme boundaries of the whole text. The preferred column is kept.
    pub fn clamp(&mut self, text: &str) {
        let count = text.chars().count();
        self.anchor = snap_down(text, self.anchor.min(count));
        self.caret = snap_down(text, self.caret.min(count));
    }

    /// Applies `motion` over `text` laid out as `layout`. `page_rows` is
    /// the number of visible rows, for PageUp/PageDown.
    pub fn apply(
        &mut self,
        text: &str,
        layout: &WrapLayout,
        motion: Motion,
        selecting: Selecting,
        page_rows: u16,
    ) {
        self.clamp(text);
        let collapse = selecting == Selecting::Move && self.has_selection();
        let caret = match motion {
            Motion::Left if collapse => self.selection().start,
            Motion::Right if collapse => self.selection().end,
            Motion::Left => previous_grapheme(text, self.caret),
            Motion::Right => next_grapheme(text, self.caret),
            Motion::WordLeft => previous_word_boundary(text, self.caret),
            Motion::WordRight => next_word_boundary(text, self.caret),
            Motion::Up => self.vertical(text, layout, VerticalStep::Up(1)),
            Motion::Down => self.vertical(text, layout, VerticalStep::Down(1)),
            Motion::PageUp => self.vertical(text, layout, VerticalStep::Up(page(page_rows))),
            Motion::PageDown => self.vertical(text, layout, VerticalStep::Down(page(page_rows))),
            Motion::RowStart => layout
                .rows()
                .get(layout.row_of_char(self.caret))
                .map_or(0, |row| row.chars.start),
            Motion::RowEnd => layout.row_end(text, layout.row_of_char(self.caret)),
            Motion::DocStart => 0,
            Motion::DocEnd => text.chars().count(),
        };
        if !matches!(
            motion,
            Motion::Up | Motion::Down | Motion::PageUp | Motion::PageDown
        ) {
            self.preferred_column = None;
        }
        self.caret = caret;
        if selecting == Selecting::Move {
            self.anchor = caret;
        }
    }

    /// The caret after moving `step` visual rows, setting the preferred
    /// column first if there is none.
    fn vertical(&mut self, text: &str, layout: &WrapLayout, step: VerticalStep) -> usize {
        let column = *self
            .preferred_column
            .get_or_insert_with(|| layout.column_of_char(text, self.caret));
        let row = layout.row_of_char(self.caret);
        let last_row = layout.rows().len().saturating_sub(1);
        match step {
            VerticalStep::Up(n) => match row.checked_sub(n) {
                Some(target) => layout.char_at(text, target, column),
                None => 0,
            },
            VerticalStep::Down(n) => match row.checked_add(n) {
                Some(target) if target <= last_row => layout.char_at(text, target, column),
                _ => text.chars().count(),
            },
        }
    }
}

/// A vertical move by a number of visual rows.
#[derive(Debug, Clone, Copy)]
enum VerticalStep {
    Up(usize),
    Down(usize),
}

/// The rows PageUp/PageDown move: `max(page_rows - 1, 1)`.
fn page(page_rows: u16) -> usize {
    usize::from(page_rows.saturating_sub(1).max(1))
}

/// The grapheme boundary of the whole `text` after `char_idx`, or
/// `char_idx` itself at the end.
pub(crate) fn next_grapheme(text: &str, char_idx: usize) -> usize {
    let mut cursor = GraphemeCursor::new(byte_of_char(text, char_idx), text.len(), true);
    match cursor.next_boundary(text, 0) {
        Ok(Some(byte)) => char_of_byte(text, byte),
        _ => char_idx.min(text.chars().count()),
    }
}

/// The grapheme boundary of the whole `text` before `char_idx`, or
/// `char_idx` itself at the start.
pub(crate) fn previous_grapheme(text: &str, char_idx: usize) -> usize {
    let mut cursor = GraphemeCursor::new(byte_of_char(text, char_idx), text.len(), true);
    match cursor.prev_boundary(text, 0) {
        Ok(Some(byte)) => char_of_byte(text, byte),
        _ => char_idx.min(text.chars().count()),
    }
}

/// `char_idx` clamped to the text and snapped down to the nearest grapheme
/// boundary of the whole `text` at or before it.
pub(crate) fn snap_down(text: &str, char_idx: usize) -> usize {
    let char_idx = char_idx.min(text.chars().count());
    let mut cursor = GraphemeCursor::new(byte_of_char(text, char_idx), text.len(), true);
    match cursor.is_boundary(text, 0) {
        Ok(false) => previous_grapheme(text, char_idx),
        _ => char_idx,
    }
}

/// `char_idx` clamped to the text and snapped up to the nearest grapheme
/// boundary of the whole `text` at or after it.
pub(crate) fn snap_up(text: &str, char_idx: usize) -> usize {
    let char_idx = char_idx.min(text.chars().count());
    let mut cursor = GraphemeCursor::new(byte_of_char(text, char_idx), text.len(), true);
    match cursor.is_boundary(text, 0) {
        Ok(false) => next_grapheme(text, char_idx),
        _ => char_idx,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Applies `motion` to `cursor` over `text` wrapped at `width`.
    fn moved(
        text: &str,
        width: u16,
        mut cursor: Cursor,
        motion: Motion,
        selecting: Selecting,
    ) -> Cursor {
        let layout = WrapLayout::new(text, width);
        cursor.apply(text, &layout, motion, selecting, 10);
        cursor
    }

    const TEXT: &str = "alpha beta\ngamma\ndelta epsilon";

    #[test]
    fn left_moves_and_extends() {
        let c = moved(TEXT, 80, Cursor::at(3), Motion::Left, Selecting::Move);
        assert_eq!(c, Cursor::at(2));
        let c = moved(TEXT, 80, Cursor::at(3), Motion::Left, Selecting::Extend);
        assert_eq!(c, Cursor::spanning(3, 2));
        let c = moved(TEXT, 80, Cursor::at(0), Motion::Left, Selecting::Move);
        assert_eq!(c, Cursor::at(0));
    }

    #[test]
    fn right_moves_and_extends() {
        let c = moved(TEXT, 80, Cursor::at(3), Motion::Right, Selecting::Move);
        assert_eq!(c, Cursor::at(4));
        let c = moved(TEXT, 80, Cursor::at(3), Motion::Right, Selecting::Extend);
        assert_eq!(c, Cursor::spanning(3, 4));
        let end = TEXT.chars().count();
        let c = moved(TEXT, 80, Cursor::at(end), Motion::Right, Selecting::Move);
        assert_eq!(c, Cursor::at(end));
    }

    #[test]
    fn plain_left_right_collapse_a_selection() {
        let sel = Cursor::spanning(6, 2);
        assert_eq!(
            moved(TEXT, 80, sel, Motion::Left, Selecting::Move),
            Cursor::at(2)
        );
        assert_eq!(
            moved(TEXT, 80, sel, Motion::Right, Selecting::Move),
            Cursor::at(6)
        );
        assert_eq!(
            moved(TEXT, 80, sel, Motion::Left, Selecting::Extend),
            Cursor::spanning(6, 1)
        );
        assert_eq!(
            moved(TEXT, 80, sel, Motion::Right, Selecting::Extend),
            Cursor::spanning(6, 3)
        );
    }

    #[test]
    fn word_left_moves_and_extends() {
        let c = moved(TEXT, 80, Cursor::at(8), Motion::WordLeft, Selecting::Move);
        assert_eq!(c, Cursor::at(6));
        let c = moved(TEXT, 80, Cursor::at(8), Motion::WordLeft, Selecting::Extend);
        assert_eq!(c, Cursor::spanning(8, 6));
    }

    #[test]
    fn word_right_moves_and_extends() {
        let c = moved(TEXT, 80, Cursor::at(1), Motion::WordRight, Selecting::Move);
        assert_eq!(c, Cursor::at(5));
        let c = moved(
            TEXT,
            80,
            Cursor::at(1),
            Motion::WordRight,
            Selecting::Extend,
        );
        assert_eq!(c, Cursor::spanning(1, 5));
    }

    #[test]
    fn word_motion_moves_from_the_caret_with_a_selection() {
        let c = moved(
            TEXT,
            80,
            Cursor::spanning(8, 1),
            Motion::WordRight,
            Selecting::Move,
        );
        assert_eq!(c, Cursor::at(5));
    }

    #[test]
    fn up_moves_and_extends() {
        // Caret at "gam|ma" (line 1, column 3).
        let c = moved(TEXT, 80, Cursor::at(14), Motion::Up, Selecting::Move);
        assert_eq!(c.caret(), 3);
        assert_eq!(c.anchor(), 3);
        let c = moved(TEXT, 80, Cursor::at(14), Motion::Up, Selecting::Extend);
        assert_eq!((c.anchor(), c.caret()), (14, 3));
    }

    #[test]
    fn down_moves_and_extends() {
        let c = moved(TEXT, 80, Cursor::at(3), Motion::Down, Selecting::Move);
        assert_eq!(c.caret(), 14);
        assert_eq!(c.anchor(), 14);
        let c = moved(TEXT, 80, Cursor::at(3), Motion::Down, Selecting::Extend);
        assert_eq!((c.anchor(), c.caret()), (3, 14));
    }

    #[test]
    fn up_on_first_row_goes_to_start_and_down_on_last_row_to_end() {
        let end = TEXT.chars().count();
        let c = moved(TEXT, 80, Cursor::at(4), Motion::Up, Selecting::Move);
        assert_eq!(c.caret(), 0);
        let c = moved(TEXT, 80, Cursor::at(20), Motion::Down, Selecting::Move);
        assert_eq!(c.caret(), end);
    }

    #[test]
    fn vertical_motion_keeps_preferred_column_across_a_shorter_row() {
        let layout = WrapLayout::new(TEXT, 80);
        // "alpha beta" column 9 -> "gamma" (5 cols) -> "delta epsilon".
        let mut c = Cursor::at(9);
        c.apply(TEXT, &layout, Motion::Down, Selecting::Move, 10);
        assert_eq!(c.caret(), 16);
        c.apply(TEXT, &layout, Motion::Down, Selecting::Move, 10);
        assert_eq!(c.caret(), 17 + 9);
        c.apply(TEXT, &layout, Motion::Up, Selecting::Move, 10);
        c.apply(TEXT, &layout, Motion::Up, Selecting::Move, 10);
        assert_eq!(c.caret(), 9);
    }

    #[test]
    fn left_drops_the_preferred_column() {
        let layout = WrapLayout::new(TEXT, 80);
        let mut c = Cursor::at(9);
        c.apply(TEXT, &layout, Motion::Down, Selecting::Move, 10);
        assert_eq!(c.caret(), 16);
        c.apply(TEXT, &layout, Motion::Left, Selecting::Move, 10);
        assert_eq!(c.caret(), 15);
        c.apply(TEXT, &layout, Motion::Down, Selecting::Move, 10);
        // Column 4 of "delta epsilon", not the old column 9.
        assert_eq!(c.caret(), 17 + 4);
    }

    #[test]
    fn row_start_moves_and_extends() {
        let c = moved(TEXT, 80, Cursor::at(14), Motion::RowStart, Selecting::Move);
        assert_eq!(c, Cursor::at(11));
        let c = moved(
            TEXT,
            80,
            Cursor::at(14),
            Motion::RowStart,
            Selecting::Extend,
        );
        assert_eq!(c, Cursor::spanning(14, 11));
    }

    #[test]
    fn row_start_on_a_soft_wrapped_row() {
        // Width 6: "alpha " | "beta" | "gamma" | ...
        let c = moved(TEXT, 6, Cursor::at(8), Motion::RowStart, Selecting::Move);
        assert_eq!(c, Cursor::at(6));
    }

    #[test]
    fn row_end_moves_and_extends() {
        let c = moved(TEXT, 80, Cursor::at(12), Motion::RowEnd, Selecting::Move);
        assert_eq!(c, Cursor::at(16));
        let c = moved(TEXT, 80, Cursor::at(12), Motion::RowEnd, Selecting::Extend);
        assert_eq!(c, Cursor::spanning(12, 16));
    }

    #[test]
    fn row_end_on_a_soft_wrapped_row() {
        // Width 6: "alpha " wraps; its end is before the space.
        let c = moved(TEXT, 6, Cursor::at(1), Motion::RowEnd, Selecting::Move);
        assert_eq!(c, Cursor::at(5));
        // The last row of the line ends at the content end.
        let c = moved(TEXT, 6, Cursor::at(6), Motion::RowEnd, Selecting::Move);
        assert_eq!(c, Cursor::at(10));
    }

    #[test]
    fn down_onto_a_soft_wrapped_row() {
        // Width 9: "abcdefghi" | "j" | "hello " | "world".
        let text = "abcdefghij\nhello world";
        let layout = WrapLayout::new(text, 9);
        let mut c = Cursor::at(8);
        c.apply(text, &layout, Motion::Down, Selecting::Move, 10);
        assert_eq!(c.caret(), 10);
        c.apply(text, &layout, Motion::Down, Selecting::Move, 10);
        assert_eq!(c.caret(), 16);
        c.apply(text, &layout, Motion::Down, Selecting::Move, 10);
        assert_eq!(c.caret(), 22);
    }

    fn numbered_lines(n: usize) -> String {
        (0..n)
            .map(|i| format!("{i:02}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn page_up_moves_and_extends() {
        let text = numbered_lines(30);
        // Line 20, column 1 is char 20 * 3 + 1; page_rows 10 moves 9 rows.
        let c = moved(&text, 80, Cursor::at(61), Motion::PageUp, Selecting::Move);
        assert_eq!(c.caret(), 11 * 3 + 1);
        assert_eq!(c.anchor(), c.caret());
        let c = moved(&text, 80, Cursor::at(61), Motion::PageUp, Selecting::Extend);
        assert_eq!((c.anchor(), c.caret()), (61, 34));
        let c = moved(&text, 80, Cursor::at(7), Motion::PageUp, Selecting::Move);
        assert_eq!(c.caret(), 0);
    }

    #[test]
    fn page_down_moves_and_extends() {
        let text = numbered_lines(30);
        let c = moved(&text, 80, Cursor::at(4), Motion::PageDown, Selecting::Move);
        assert_eq!(c.caret(), 10 * 3 + 1);
        assert_eq!(c.anchor(), c.caret());
        let c = moved(
            &text,
            80,
            Cursor::at(4),
            Motion::PageDown,
            Selecting::Extend,
        );
        assert_eq!((c.anchor(), c.caret()), (4, 31));
        let c = moved(&text, 80, Cursor::at(70), Motion::PageDown, Selecting::Move);
        assert_eq!(c.caret(), text.chars().count());
    }

    #[test]
    fn page_moves_at_least_one_row() {
        let text = numbered_lines(5);
        let layout = WrapLayout::new(&text, 80);
        for page_rows in [0, 1, 2] {
            let mut c = Cursor::at(4);
            c.apply(&text, &layout, Motion::PageDown, Selecting::Move, page_rows);
            assert_eq!(c.caret(), 7, "page_rows {page_rows}");
        }
    }

    #[test]
    fn page_motion_keeps_the_preferred_column() {
        let text = format!("abcdef\n{}\nabcdef", numbered_lines(2));
        let layout = WrapLayout::new(&text, 80);
        let mut c = Cursor::at(5);
        c.apply(&text, &layout, Motion::PageDown, Selecting::Move, 2);
        assert_eq!(c.caret(), 9);
        c.apply(&text, &layout, Motion::PageDown, Selecting::Move, 2);
        assert_eq!(c.caret(), 12);
        c.apply(&text, &layout, Motion::PageDown, Selecting::Move, 2);
        assert_eq!(c.caret(), 13 + 5);
    }

    #[test]
    fn doc_start_moves_and_extends() {
        let c = moved(TEXT, 80, Cursor::at(14), Motion::DocStart, Selecting::Move);
        assert_eq!(c, Cursor::at(0));
        let c = moved(
            TEXT,
            80,
            Cursor::at(14),
            Motion::DocStart,
            Selecting::Extend,
        );
        assert_eq!(c, Cursor::spanning(14, 0));
    }

    #[test]
    fn doc_end_moves_and_extends() {
        let end = TEXT.chars().count();
        let c = moved(TEXT, 80, Cursor::at(14), Motion::DocEnd, Selecting::Move);
        assert_eq!(c, Cursor::at(end));
        let c = moved(TEXT, 80, Cursor::at(14), Motion::DocEnd, Selecting::Extend);
        assert_eq!(c, Cursor::spanning(14, end));
    }

    #[test]
    fn left_right_step_over_crlf() {
        let text = "a\r\nb";
        let c = moved(text, 80, Cursor::at(1), Motion::Right, Selecting::Move);
        assert_eq!(c.caret(), 3);
        let c = moved(text, 80, Cursor::at(3), Motion::Left, Selecting::Move);
        assert_eq!(c.caret(), 1);
    }

    #[test]
    fn left_right_step_over_a_combining_mark() {
        let text = "xe\u{301}y";
        let c = moved(text, 80, Cursor::at(1), Motion::Right, Selecting::Move);
        assert_eq!(c.caret(), 3);
        let c = moved(text, 80, Cursor::at(3), Motion::Left, Selecting::Move);
        assert_eq!(c.caret(), 1);
    }

    #[test]
    fn left_right_step_over_a_zwj_emoji() {
        // Woman, ZWJ, laptop: three chars, one grapheme.
        let text = "a\u{1F469}\u{200D}\u{1F4BB}b";
        let c = moved(text, 80, Cursor::at(1), Motion::Right, Selecting::Move);
        assert_eq!(c.caret(), 4);
        let c = moved(text, 80, Cursor::at(4), Motion::Left, Selecting::Move);
        assert_eq!(c.caret(), 1);
    }

    #[test]
    fn clamp_after_the_text_shrinks() {
        let mut c = Cursor::spanning(40, 12);
        c.clamp("short");
        assert_eq!(c, Cursor::spanning(5, 5));
    }

    #[test]
    fn clamp_inside_crlf_snaps_down() {
        let mut c = Cursor::spanning(2, 2);
        c.clamp("a\r\nb");
        assert_eq!(c, Cursor::at(1));
    }

    #[test]
    fn clamp_keeps_the_preferred_column() {
        let layout = WrapLayout::new(TEXT, 80);
        let mut c = Cursor::at(9);
        c.apply(TEXT, &layout, Motion::Down, Selecting::Move, 10);
        let before = c.preferred_column;
        c.clamp(TEXT);
        assert_eq!(c.preferred_column, before);
        assert!(before.is_some());
    }

    #[test]
    fn apply_clamps_first() {
        let c = moved("abc", 80, Cursor::at(99), Motion::Left, Selecting::Move);
        assert_eq!(c, Cursor::at(2));
    }

    #[test]
    fn snap_up_moves_to_the_next_boundary() {
        // Inside "\r\n": after it.
        assert_eq!(snap_up("a\r\nb", 2), 3);
        // On a boundary: unchanged.
        assert_eq!(snap_up("a\r\nb", 1), 1);
        assert_eq!(snap_up("a\r\nb", 3), 3);
        assert_eq!(snap_up("e\u{301}x", 0), 0);
        // Inside a combining sequence: after it.
        assert_eq!(snap_up("e\u{301}x", 1), 2);
        // Past the end: the char count.
        assert_eq!(snap_up("a\r\nb", 100), 4);
        assert_eq!(snap_up("", 5), 0);
    }

    #[test]
    fn grapheme_helpers_at_the_ends() {
        assert_eq!(next_grapheme("ab", 2), 2);
        assert_eq!(previous_grapheme("ab", 0), 0);
        assert_eq!(next_grapheme("", 0), 0);
        assert_eq!(previous_grapheme("", 0), 0);
        assert_eq!(snap_down("", 3), 0);
        assert_eq!(snap_down("ab", 1), 1);
    }

    #[test]
    fn selection_is_ordered() {
        let c = Cursor::spanning(7, 3);
        assert_eq!(c.selection(), 3..7);
        assert!(c.has_selection());
        assert!(!Cursor::at(4).has_selection());
        assert_eq!(Cursor::at(4).selection(), 4..4);
    }
}
