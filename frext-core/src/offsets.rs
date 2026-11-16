//! Conversion between char indices and byte offsets within a `&str`.
//!
//! Editor cursors (in both the GUI and the TUI) are measured in characters,
//! while search and proofreading report byte ranges. These two helpers are the
//! single place that translates between the two.

/// The byte offset of char index `char_idx` within `text`.
///
/// If `char_idx` is greater than or equal to the number of chars in `text`,
/// the result is clamped to `text.len()`.
#[must_use]
pub fn byte_of_char(text: &str, char_idx: usize) -> usize {
    text.char_indices()
        .nth(char_idx)
        .map_or(text.len(), |(b, _)| b)
}

/// The char index of byte offset `byte` within `text`; the inverse of
/// [`byte_of_char`].
///
/// A `byte` that falls inside a multi-byte char rounds *up* to the index of
/// the next char. If `byte` is greater than the start of the last char (in
/// particular, `byte >= text.len()`), the result is clamped to the char count
/// of `text`.
#[must_use]
pub fn char_of_byte(text: &str, byte: usize) -> usize {
    text.char_indices()
        .position(|(b, _)| b >= byte)
        .unwrap_or_else(|| text.chars().count())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_round_trips() {
        let text = "hello";
        for i in 0..=text.len() {
            assert_eq!(byte_of_char(text, i), i);
            assert_eq!(char_of_byte(text, i), i);
            assert_eq!(char_of_byte(text, byte_of_char(text, i)), i);
        }
    }

    #[test]
    fn multi_byte_text_converts_both_directions() {
        // 'é' is 2 bytes: "café crème" -> c a f é(3..5) ' '(5) c(6) r(7) è(8..10) m(10) e(11)
        let text = "café crème";
        assert_eq!(byte_of_char(text, 3), 3);
        assert_eq!(byte_of_char(text, 4), 5);
        assert_eq!(byte_of_char(text, 8), 10);
        assert_eq!(char_of_byte(text, 5), 4);
        assert_eq!(char_of_byte(text, 10), 8);
        for i in 0..=text.chars().count() {
            assert_eq!(char_of_byte(text, byte_of_char(text, i)), i);
        }
    }

    #[test]
    fn four_byte_emoji_converts_both_directions() {
        let text = "a😀b";
        assert_eq!(byte_of_char(text, 1), 1);
        assert_eq!(byte_of_char(text, 2), 5);
        assert_eq!(char_of_byte(text, 1), 1);
        assert_eq!(char_of_byte(text, 5), 2);
    }

    #[test]
    fn char_index_at_or_past_end_clamps_to_len() {
        let text = "café";
        assert_eq!(byte_of_char(text, 4), text.len());
        assert_eq!(byte_of_char(text, 100), text.len());
    }

    #[test]
    fn byte_at_or_past_end_clamps_to_char_count() {
        let text = "café";
        assert_eq!(char_of_byte(text, text.len()), 4);
        assert_eq!(char_of_byte(text, 100), 4);
    }

    #[test]
    fn byte_inside_multi_byte_char_rounds_up() {
        let text = "a😀b";
        for byte in 2..5 {
            assert_eq!(char_of_byte(text, byte), 2);
        }
        // Inside the final multi-byte char rounds up to the char count.
        assert_eq!(char_of_byte("café", 4), 4);
    }

    #[test]
    fn empty_string() {
        assert_eq!(byte_of_char("", 0), 0);
        assert_eq!(byte_of_char("", 3), 0);
        assert_eq!(char_of_byte("", 0), 0);
        assert_eq!(char_of_byte("", 3), 0);
    }
}
