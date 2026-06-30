//! Pure text-buffer edits backing the editor's cut/paste menu actions.
//!
//! egui text cursors are measured in characters, so these operate on a
//! character range `[start, end)` over `text` and return the new caret
//! position (also a character index). Keeping them free of egui and clipboard
//! I/O makes the splice logic straightforward to unit-test; the app layer
//! wires them to the live `TextEdit` state and the OS clipboard.

/// The byte offset of character index `char_idx` within `text`, clamped to the
/// end of the string. Mirrors the cursor model egui uses (character indices).
fn byte_offset(text: &str, char_idx: usize) -> usize {
    text.char_indices()
        .nth(char_idx)
        .map_or(text.len(), |(byte, _)| byte)
}

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
    let lo_byte = byte_offset(text, lo);
    let hi_byte = byte_offset(text, hi);
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
    let lo_byte = byte_offset(text, lo);
    let hi_byte = byte_offset(text, hi);
    text.replace_range(lo_byte..hi_byte, "");
    lo
}

/// Replace the character range `[start, end)` of `text` with `insert`. Returns
/// the new caret character index (just past the inserted text).
///
/// With an empty selection this inserts `insert` at the caret.
pub fn replace_range(text: &mut String, start: usize, end: usize, insert: &str) -> usize {
    let (lo, hi) = normalize(text, start, end);
    let lo_byte = byte_offset(text, lo);
    let hi_byte = byte_offset(text, hi);
    text.replace_range(lo_byte..hi_byte, insert);
    lo + insert.chars().count()
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
}
