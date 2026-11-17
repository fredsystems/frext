//! Pure text-buffer edits backing the editor's cut/paste menu actions.
//!
//! GUI text cursors are measured in characters, so these operate on a
//! character range `[start, end)` over `text` and return the new caret
//! position (also a character index). Keeping them free of UI and clipboard
//! I/O makes the splice logic straightforward to unit-test; the app layer
//! wires them to the live `TextEdit` state and the OS clipboard.

use std::ops::Range;

/// Normalise a possibly-reversed selection to `(lo, hi)` character indices,
/// clamped to the buffer's character length.
fn normalize(text: &str, start: usize, end: usize) -> (usize, usize) {
    let len = text.chars().count();
    let lo = start.min(end).min(len);
    let hi = start.max(end).min(len);
    (lo, hi)
}

/// The substring covered by the character range `[start, end)`.
#[must_use]
pub fn selected_text(text: &str, start: usize, end: usize) -> String {
    let (lo, hi) = normalize(text, start, end);
    let lo_byte = crate::offsets::byte_of_char(text, lo);
    let hi_byte = crate::offsets::byte_of_char(text, hi);
    text.get(lo_byte..hi_byte).unwrap_or_default().to_owned()
}

/// Delete the character range `[start, end)` from `text` in place. Returns the
/// new caret character index (the start of what was the selection).
///
/// An empty selection (`start == end`) is a no-op and returns that position.
pub fn delete_range(text: &mut String, start: usize, end: usize) -> usize {
    let (lo, hi) = normalize(text, start, end);
    if lo == hi {
        return lo;
    }
    let lo_byte = crate::offsets::byte_of_char(text, lo);
    let hi_byte = crate::offsets::byte_of_char(text, hi);
    text.replace_range(lo_byte..hi_byte, "");
    lo
}

/// Replace the character range `[start, end)` of `text` with `insert`. Returns
/// the new caret character index (just past the inserted text).
///
/// With an empty selection this inserts `insert` at the caret.
pub fn replace_range(text: &mut String, start: usize, end: usize, insert: &str) -> usize {
    let (lo, hi) = normalize(text, start, end);
    let lo_byte = crate::offsets::byte_of_char(text, lo);
    let hi_byte = crate::offsets::byte_of_char(text, hi);
    text.replace_range(lo_byte..hi_byte, insert);
    lo + insert.chars().count()
}

/// Map the char range `range` of `before` onto `after`, the same buffer after
/// one edit, so it keeps covering the same text.
///
/// The edit is located from the common prefix and suffix (in chars) of the two
/// strings. A position before the edit is unchanged, and one after it shifts
/// by the change in length. A start inside the edit moves to the edit start,
/// and an end inside it moves to the end of the inserted text. The result is
/// clamped so `start <= end <= after`'s char count; identical strings return
/// `range` unchanged apart from that clamp.
#[must_use]
pub fn track_range(range: Range<usize>, before: &str, after: &str) -> Range<usize> {
    let before_len = before.chars().count();
    let after_len = after.chars().count();
    let prefix = before
        .chars()
        .zip(after.chars())
        .take_while(|(a, b)| a == b)
        .count();
    // The suffix never overlaps the prefix in either string.
    let suffix = before
        .chars()
        .rev()
        .zip(after.chars().rev())
        .take(before_len.min(after_len).saturating_sub(prefix))
        .take_while(|(a, b)| a == b)
        .count();
    let old_end = before_len.saturating_sub(suffix);
    let new_end = after_len.saturating_sub(suffix);

    let map = |x: usize, inside: usize| {
        if x <= prefix {
            x
        } else if x >= old_end {
            // `x >= before_len - suffix` and `after_len >= suffix`, so this
            // never underflows before saturating.
            (x + after_len).saturating_sub(before_len)
        } else {
            inside
        }
    };
    let end = map(range.end, new_end).min(after_len);
    let start = map(range.start, prefix).min(end);
    start..end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_text_extracts_the_range() {
        assert_eq!(selected_text("hello world", 0, 5), "hello");
        assert_eq!(selected_text("hello world", 6, 11), "world");
        // Reversed range is normalised.
        assert_eq!(selected_text("hello world", 11, 6), "world");
        // Empty selection.
        assert_eq!(selected_text("hello", 2, 2), "");
    }

    #[test]
    fn selected_text_is_char_aware() {
        // Multi-byte characters: indices are characters, not bytes.
        let text = "café au lait";
        assert_eq!(selected_text(text, 0, 4), "café");
        assert_eq!(selected_text(text, 5, 7), "au");
    }

    #[test]
    fn delete_range_removes_and_returns_caret() {
        let mut text = "hello world".to_owned();
        let caret = delete_range(&mut text, 5, 11);
        assert_eq!(text, "hello");
        assert_eq!(caret, 5);
    }

    #[test]
    fn delete_empty_range_is_a_noop() {
        let mut text = "stable".to_owned();
        let caret = delete_range(&mut text, 3, 3);
        assert_eq!(text, "stable");
        assert_eq!(caret, 3);
    }

    #[test]
    fn delete_range_is_char_aware() {
        let mut text = "café crème".to_owned();
        // Delete "café " (chars 0..5), leaving "crème".
        let caret = delete_range(&mut text, 0, 5);
        assert_eq!(text, "crème");
        assert_eq!(caret, 0);
    }

    #[test]
    fn replace_range_inserts_over_selection() {
        let mut text = "hello world".to_owned();
        let caret = replace_range(&mut text, 6, 11, "there");
        assert_eq!(text, "hello there");
        // Caret sits just past the inserted "there".
        assert_eq!(caret, 11);
    }

    #[test]
    fn replace_empty_selection_inserts_at_caret() {
        let mut text = "ac".to_owned();
        let caret = replace_range(&mut text, 1, 1, "b");
        assert_eq!(text, "abc");
        assert_eq!(caret, 2);
    }

    #[test]
    fn replace_range_is_char_aware() {
        let mut text = "café".to_owned();
        // Replace the "é" (char index 3..4) with "e".
        let caret = replace_range(&mut text, 3, 4, "e");
        assert_eq!(text, "cafe");
        assert_eq!(caret, 4);
    }

    #[test]
    fn track_range_shifts_past_an_insert_before_it() {
        assert_eq!(track_range(6..11, "hello world", "XXhello world"), 8..13);
    }

    #[test]
    fn track_range_ignores_an_insert_after_it() {
        assert_eq!(track_range(0..5, "hello world", "hello world!"), 0..5);
    }

    #[test]
    fn track_range_moves_a_deleted_start_to_the_edit_start() {
        // "bcd" (chars 1..4) is deleted; the range "cde" keeps only "e".
        assert_eq!(track_range(2..5, "abcdef", "aef"), 1..2);
    }

    #[test]
    fn track_range_extends_over_an_insert_inside_it() {
        // "XY" is inserted at char 2, inside "bcd" (chars 1..4).
        assert_eq!(track_range(1..4, "abcdef", "abXYcdef"), 1..6);
    }

    #[test]
    fn track_range_collapses_onto_a_replacement_spanning_it() {
        // "bcde" (chars 1..5) becomes "Z", swallowing the range "cd".
        assert_eq!(track_range(2..4, "abcdef", "aZf"), 1..2);
    }

    #[test]
    fn track_range_leaves_identical_text_alone_but_clamps() {
        assert_eq!(track_range(1..2, "abc", "abc"), 1..2);
        assert_eq!(track_range(1..10, "abc", "abc"), 1..3);
    }

    #[test]
    fn track_range_counts_chars_not_bytes() {
        // "本" (char 1) shifts by one char, not by the inserted byte count.
        assert_eq!(track_range(1..2, "日本語", "x日本語"), 2..3);
        assert_eq!(track_range(0..2, "日本語", "日本語éé"), 0..2);
    }

    #[test]
    fn track_range_handles_empty_text() {
        assert_eq!(track_range(0..0, "", "abc"), 0..0);
        assert_eq!(track_range(1..2, "abc", ""), 0..0);
        assert_eq!(track_range(0..0, "", ""), 0..0);
    }
}
