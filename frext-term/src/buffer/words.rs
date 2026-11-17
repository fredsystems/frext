//! Word and line boundaries over char indices.
//!
//! A faithful port of egui 0.37.0's `text_cursor_state.rs` rules, so the
//! terminal editor moves and selects exactly like the GUI one: Unicode word
//! bounds, a word char is alphanumeric or `_`, `.` is always a stop, and
//! whitespace runs are skipped. Unlike egui, nothing here panics: every
//! char index is clamped to the text.
//!
//! Selection intentionally differs from egui in two places, while the word
//! motions stay a faithful port:
//!
//! - [`word_at`] finds the word's end first and its start from that end,
//!   where egui's `select_word_at` selects two words when the index is a
//!   word's first char.
//! - [`line_at`] is the logical line without its line break, where egui's
//!   `select_line_at` over-selects at empty and one-char lines.

use std::ops::Range;

use frext_core::offsets::{byte_of_char, char_of_byte};
use unicode_segmentation::UnicodeSegmentation;

/// Whether `c` is part of a word: alphanumeric or `_`.
#[must_use]
pub fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The char index of the next word boundary after `char_idx` (egui's
/// `next_word_boundary_char_index`).
///
/// `.` is always a boundary, and a run of non-word chars (such as
/// whitespace) right after the cursor is skipped. Returns the char count of
/// `text` when there is no later boundary.
#[must_use]
pub fn next_word_boundary(text: &str, char_idx: usize) -> usize {
    let mut current_char_idx = 0;

    for word in text.split_word_bounds() {
        let word_ci = current_char_idx;

        let mut word_char_count = 0;
        for chr in word.chars() {
            let dot_ci = word_ci + word_char_count;
            if chr == '.' && char_idx < dot_ci {
                return dot_ci;
            }
            word_char_count += 1;
        }

        if char_idx < word_ci && !all_word_chars(word) {
            return word_ci;
        }

        current_char_idx += word_char_count;
    }

    current_char_idx
}

/// The char index of the previous word boundary before `char_idx` (egui's
/// `ccursor_previous_word`): the forward rule run over the
/// grapheme-reversed text.
#[must_use]
pub fn previous_word_boundary(text: &str, char_idx: usize) -> usize {
    let num_chars = text.chars().count();
    let char_idx = char_idx.min(num_chars);
    let reversed: String = text.graphemes(true).rev().collect();
    let boundary = next_word_boundary(&reversed, num_chars - char_idx);
    num_chars - boundary.min(num_chars)
}

/// The word around `char_idx`, clipped to its logical line (see
/// [`line_at`]), as an ordered char range.
///
/// The end is the next word boundary after `char_idx` and the start is the
/// previous word boundary before that end. This intentionally differs from
/// egui's `select_word_at`, which takes both boundaries from `char_idx` and
/// so selects two words when `char_idx` is a word's first char.
#[must_use]
pub fn word_at(text: &str, char_idx: usize) -> Range<usize> {
    let num_chars = text.chars().count();
    let char_idx = char_idx.min(num_chars);
    if text.is_empty() {
        return char_idx..char_idx;
    }

    let line = line_at(text, char_idx);
    let current_line_text = slice_char_range(text, line.clone());
    let relative_idx = char_idx
        .saturating_sub(line.start)
        .min(line.end - line.start);

    let end = next_word_boundary(current_line_text, relative_idx);
    let start = previous_word_boundary(current_line_text, end);

    (line.start + start.min(end))..(line.start + start.max(end))
}

/// The logical line containing `char_idx`, as a char range without its
/// line break.
///
/// The line runs from the char after the previous `'\n'` (or 0) up to,
/// but not including, the next `'\n'` (or the end of the text), also
/// excluding a `'\r'` right before that `'\n'`. An index on a `'\n'`
/// belongs to the line that `'\n'` ends; an index past the end is clamped
/// to the char count.
///
/// This intentionally differs from egui's `select_line_at`, which
/// over-selects at empty and one-char lines.
#[must_use]
pub fn line_at(text: &str, char_idx: usize) -> Range<usize> {
    let start = find_line_start(text, char_idx.min(text.chars().count()));
    let start_byte = byte_of_char(text, start);
    let rest = text.get(start_byte..).unwrap_or("");
    let end_byte = rest.find('\n').map_or(text.len(), |newline| {
        let line = rest.get(..newline).unwrap_or("");
        start_byte + line.strip_suffix('\r').unwrap_or(line).len()
    });
    start..char_of_byte(text, end_byte)
}

fn all_word_chars(text: &str) -> bool {
    text.chars().all(is_word_char)
}

fn find_line_start(text: &str, char_idx: usize) -> usize {
    let byte_idx = byte_of_char(text, char_idx);
    let text_before = text.get(..byte_idx).unwrap_or("");
    text_before.rfind('\n').map_or(0, |last_newline_byte| {
        char_of_byte(text, last_newline_byte + 1)
    })
}

fn slice_char_range(text: &str, chars: Range<usize>) -> &str {
    let start = byte_of_char(text, chars.start);
    let end = byte_of_char(text, chars.end.max(chars.start));
    text.get(start..end).unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_word_boundary_egui_vectors() {
        // ASCII only
        let text = "abc d3f g_h i-j";
        assert_eq!(next_word_boundary(text, 1), 3);
        assert_eq!(next_word_boundary(text, 3), 7);
        assert_eq!(next_word_boundary(text, 9), 11);
        assert_eq!(next_word_boundary(text, 12), 13);
        assert_eq!(next_word_boundary(text, 13), 15);
        assert_eq!(next_word_boundary(text, 15), 15);

        assert_eq!(next_word_boundary("", 0), 0);
        assert_eq!(next_word_boundary("", 1), 0);

        // ASCII only
        let text = "abc.def.ghi";
        assert_eq!(next_word_boundary(text, 1), 3);
        assert_eq!(next_word_boundary(text, 3), 7);
        assert_eq!(next_word_boundary(text, 7), 11);

        // Graphemes made of several chars; the values match egui even where
        // they look odd at first glance.
        let text = "❤️👍 skvělá knihovna 👍❤️";
        assert_eq!(next_word_boundary(text, 0), 2);
        // Does not skip the space between thumbs-up and 'skvělá'.
        assert_eq!(next_word_boundary(text, 2), 3);
        assert_eq!(next_word_boundary(text, 6), 10);
        assert_eq!(next_word_boundary(text, 9), 10);
        assert_eq!(next_word_boundary(text, 12), 19);
        assert_eq!(next_word_boundary(text, 15), 19);
        assert_eq!(next_word_boundary(text, 19), 20);
        assert_eq!(next_word_boundary(text, 20), 21);
    }

    #[test]
    fn previous_word_egui_vectors() {
        let text = "abc def ghi";
        assert_eq!(previous_word_boundary(text, 7), 4);
        assert_eq!(previous_word_boundary(text, 5), 4);
        assert_eq!(previous_word_boundary(text, 4), 0);
        assert_eq!(previous_word_boundary(text, 0), 0);
    }

    #[test]
    fn next_word_egui_vectors() {
        let text = "abc def ghi";
        assert_eq!(next_word_boundary(text, 0), 3);
        assert_eq!(next_word_boundary(text, 3), 7);
        assert_eq!(next_word_boundary(text, 7), 11);
        assert_eq!(next_word_boundary(text, 11), 11);
    }

    #[test]
    fn previous_word_graphemes_egui_vectors() {
        let cases = [
            ("", 0, 0),
            ("hello", 0, 0),
            ("hello", "hello".chars().count(), 0),
            ("hello world", 6, 0),
            ("hello world", 8, 6),
            ("hello world", "hello world".chars().count(), 6),
            ("hello world   ", "hello world   ".chars().count(), 6),
            ("hello   world", "hello   world".chars().count(), 8),
            ("   ", "   ".chars().count(), 0),
            ("hello, world", "hello, world".chars().count(), 7),
            ("www.example.com", "www.example.com".chars().count(), 12),
            ("안녕! 😊 세상", 8, 6),
            ("❤️👍 skvělá knihovna 👍❤️", 18, 11),
            (
                "a e\u{301} b",
                "a e\u{301} b".chars().count(),
                "a e\u{301} ".chars().count(),
            ),
            (
                "hi 🙂 world",
                "hi 🙂 world".chars().count(),
                "hi 🙂 ".chars().count(),
            ),
            (
                "hi 👨‍👩‍👧‍👦 world",
                "hi 👨‍👩‍👧‍👦 world".chars().count(),
                "hi 👨‍👩‍👧‍👦 ".chars().count(),
            ),
        ];

        for (text, cursor, expected) in cases {
            assert_eq!(
                previous_word_boundary(text, cursor),
                expected,
                "text={text:?}, cursor={cursor}"
            );
        }
    }

    #[test]
    fn word_at_egui_vectors() {
        let text = "hello world";
        assert_eq!(word_at(text, 2), 0..5);
        assert_eq!(word_at(text, 8), 6..11);
    }

    #[test]
    fn word_at_a_words_first_char_selects_only_that_word() {
        let text = "hello world";
        assert_eq!(word_at(text, 6), 6..11);
        assert_eq!(word_at(text, 0), 0..5);
    }

    #[test]
    fn word_at_is_clipped_to_its_line() {
        let text = "alpha\nbeta";
        assert_eq!(word_at(text, 7), 6..10);
        assert_eq!(word_at(text, 2), 0..5);
    }

    #[test]
    fn word_at_empty_text() {
        assert_eq!(word_at("", 0), 0..0);
    }

    #[test]
    fn is_word_char_rules() {
        assert!(is_word_char('a'));
        assert!(is_word_char('3'));
        assert!(is_word_char('_'));
        assert!(is_word_char('ě'));
        assert!(!is_word_char('.'));
        assert!(!is_word_char('-'));
        assert!(!is_word_char(' '));
    }

    #[test]
    fn line_at_three_lines_excludes_the_line_break() {
        let text = "first\nsecond\nthird";
        // Index 0.
        assert_eq!(line_at(text, 0), 0..5);
        // Inside each line.
        assert_eq!(line_at(text, 2), 0..5);
        assert_eq!(line_at(text, 8), 6..12);
        assert_eq!(line_at(text, 15), 13..18);
        // At the end of a line (just before its '\n').
        assert_eq!(line_at(text, 5), 0..5);
        // At the start of a line (just after the previous '\n').
        assert_eq!(line_at(text, 6), 6..12);
        // At the end of the text.
        assert_eq!(line_at(text, 18), 13..18);
    }

    #[test]
    fn line_at_empty_line_is_empty() {
        assert_eq!(line_at("first\n\nthird", 6), 6..6);
    }

    #[test]
    fn line_at_one_char_line_excludes_the_line_break() {
        assert_eq!(line_at("a\nb", 0), 0..1);
        assert_eq!(line_at("a\nb", 2), 2..3);
    }

    #[test]
    fn line_at_excludes_crlf() {
        assert_eq!(line_at("ab\r\ncd", 1), 0..2);
        assert_eq!(line_at("ab\r\ncd", 5), 4..6);
    }

    #[test]
    fn line_at_index_on_newline_belongs_to_the_line_it_ends() {
        assert_eq!(line_at("ab\ncd", 2), 0..2);
        assert_eq!(line_at("ab\r\ncd", 3), 0..2);
    }

    #[test]
    fn line_at_past_the_end_clamps() {
        assert_eq!(line_at("ab\ncd", 100), 3..5);
    }

    #[test]
    fn word_at_next_to_an_empty_line() {
        let text = "first\n\nthird";
        assert_eq!(word_at(text, 6), 6..6);
        assert_eq!(word_at(text, 5), 0..5);
        assert_eq!(word_at(text, 7), 7..12);
    }

    #[test]
    fn line_at_empty_text() {
        assert_eq!(line_at("", 0), 0..0);
    }

    #[test]
    fn out_of_range_indices_do_not_panic() {
        for text in ["", "abc", "first\nsecond\nthird", "❤️👍 skvělá\n\n"] {
            let len = text.chars().count();
            for idx in [len, len + 1, len + 100, usize::MAX] {
                assert!(next_word_boundary(text, idx) <= len);
                assert!(previous_word_boundary(text, idx) <= len);
                let word = word_at(text, idx);
                assert!(word.start <= word.end && word.end <= len);
                let line = line_at(text, idx);
                assert!(line.start <= line.end && line.end <= len);
            }
        }
    }
}
