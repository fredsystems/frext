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

/// Map the char range `range` through one edit that replaced the `removed`
/// chars starting at char `at` with `inserted` chars.
///
/// A start before `at` is kept, and an end at or before `at` is kept. A
/// boundary at or after `at + removed` shifts by `inserted - removed`. A start
/// inside the removed text moves to `at`, and an end inside it moves to
/// `at + inserted`. So text inserted exactly at either boundary lands outside
/// the range. The result always has `start <= end`.
#[must_use]
pub fn shift_range(
    range: Range<usize>,
    at: usize,
    removed: usize,
    inserted: usize,
) -> Range<usize> {
    let removed_end = at.saturating_add(removed);
    let shift = |x: usize| x.saturating_sub(removed).saturating_add(inserted);
    let start = if range.start < at {
        range.start
    } else if range.start >= removed_end {
        shift(range.start)
    } else {
        at
    };
    let end = if range.end <= at {
        range.end
    } else if range.end >= removed_end {
        shift(range.end)
    } else {
        at.saturating_add(inserted)
    };
    start.min(end)..end
}

/// Map the char range `range` of `before` onto `after`, the same buffer after
/// one edit, so it keeps covering the same text.
///
/// The edit is the minimal one between the two strings: everything outside
/// their common prefix and (non-overlapping) common suffix, in chars. In
/// repeated text the same edit can sit at several positions that all turn
/// `before` into `after` (inserting `"a"` anywhere in `"aaa"`), so `caret`,
/// the char caret the view reported after the edit, picks one: preferably
/// the position whose inserted text ends at the caret, else the one that
/// starts at the caret, else the end of the common prefix. The range is
/// then moved through that edit with [`shift_range`] and clamped to
/// `after`'s char count; identical strings return `range` unchanged apart
/// from that clamp.
#[must_use]
pub fn track_range(
    range: Range<usize>,
    before: &str,
    after: &str,
    caret: Option<usize>,
) -> Range<usize> {
    let before_len = before.chars().count();
    let after_len = after.chars().count();
    let prefix = before
        .chars()
        .zip(after.chars())
        .take_while(|(a, b)| a == b)
        .count();
    let suffix_full = before
        .chars()
        .rev()
        .zip(after.chars().rev())
        .take_while(|(a, b)| a == b)
        .count();
    // The suffix of the minimal edit never overlaps the prefix in either
    // string.
    let suffix = suffix_full.min(before_len.min(after_len).saturating_sub(prefix));
    let removed = before_len.saturating_sub(prefix + suffix);
    let inserted = after_len.saturating_sub(prefix + suffix);

    // The edit can start at any `x` in `lowest..=prefix`: `before` and
    // `after` agree on their first `x` chars, and on everything after the
    // edit when its tail fits in the full common suffix.
    let lowest = before_len.saturating_sub(removed + suffix_full).min(prefix);
    let valid = |x: usize| (lowest..=prefix).contains(&x);
    let at = match caret {
        Some(c) if c >= inserted && valid(c - inserted) => c - inserted,
        Some(c) if valid(c) => c,
        _ => prefix,
    };

    let mapped = shift_range(range, at, removed, inserted);
    let end = mapped.end.min(after_len);
    mapped.start.min(end)..end
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
        assert_eq!(
            track_range(6..11, "hello world", "XXhello world", None),
            8..13
        );
    }

    #[test]
    fn track_range_ignores_an_insert_after_it() {
        assert_eq!(track_range(0..5, "hello world", "hello world!", None), 0..5);
    }

    #[test]
    fn track_range_moves_a_deleted_start_to_the_edit_start() {
        // "bcd" (chars 1..4) is deleted; the range "cde" keeps only "e".
        assert_eq!(track_range(2..5, "abcdef", "aef", None), 1..2);
    }

    #[test]
    fn track_range_extends_over_an_insert_inside_it() {
        // "XY" is inserted at char 2, inside "bcd" (chars 1..4).
        assert_eq!(track_range(1..4, "abcdef", "abXYcdef", None), 1..6);
    }

    #[test]
    fn track_range_collapses_onto_a_replacement_spanning_it() {
        // "bcde" (chars 1..5) becomes "Z", swallowing the range "cd".
        assert_eq!(track_range(2..4, "abcdef", "aZf", None), 1..2);
    }

    #[test]
    fn track_range_leaves_identical_text_alone_but_clamps() {
        assert_eq!(track_range(1..2, "abc", "abc", None), 1..2);
        assert_eq!(track_range(1..10, "abc", "abc", None), 1..3);
    }

    #[test]
    fn track_range_counts_chars_not_bytes() {
        // "本" (char 1) shifts by one char, not by the inserted byte count.
        assert_eq!(track_range(1..2, "日本語", "x日本語", None), 2..3);
        assert_eq!(track_range(0..2, "日本語", "日本語éé", None), 0..2);
    }

    #[test]
    fn track_range_handles_empty_text() {
        assert_eq!(track_range(0..0, "", "abc", None), 0..0);
        assert_eq!(track_range(1..2, "abc", "", None), 0..0);
        assert_eq!(track_range(0..0, "", "", None), 0..0);
    }

    #[test]
    fn shift_range_shifts_past_an_edit_before_it() {
        // "xy" (chars 1..3) becomes "z": the range after it moves back one.
        assert_eq!(shift_range(5..8, 1, 2, 1), 4..7);
    }

    #[test]
    fn shift_range_ignores_an_edit_after_it() {
        assert_eq!(shift_range(1..3, 5, 2, 7), 1..3);
    }

    #[test]
    fn shift_range_moves_an_overlapped_start_to_the_edit_start() {
        // Chars 2..6 are replaced by 1 char; the range 4..8 keeps 6..8.
        assert_eq!(shift_range(4..8, 2, 4, 1), 2..5);
    }

    #[test]
    fn shift_range_moves_an_overlapped_end_to_the_inserted_end() {
        // Chars 3..7 are replaced by 2 chars; the range 1..5 keeps 1..3 plus
        // the inserted text.
        assert_eq!(shift_range(1..5, 3, 4, 2), 1..5);
        assert_eq!(shift_range(1..5, 3, 4, 0), 1..3);
    }

    #[test]
    fn shift_range_puts_an_insertion_at_the_start_outside() {
        assert_eq!(shift_range(4..7, 4, 0, 3), 7..10);
    }

    #[test]
    fn shift_range_puts_an_insertion_at_the_end_outside() {
        assert_eq!(shift_range(4..7, 7, 0, 3), 4..7);
    }

    #[test]
    fn shift_range_collapses_onto_a_replacement_spanning_it() {
        assert_eq!(shift_range(3..5, 1, 6, 2), 1..3);
    }

    #[test]
    fn shift_range_handles_deleting_everything() {
        assert_eq!(shift_range(2..5, 0, 9, 0), 0..0);
        assert_eq!(shift_range(0..9, 0, 9, 0), 0..0);
    }

    #[test]
    fn shift_range_never_overflows() {
        assert_eq!(shift_range(5..usize::MAX, 0, 0, 3), 8..usize::MAX);
        assert_eq!(shift_range(0..1, usize::MAX, usize::MAX, 1), 0..1);
    }

    #[test]
    fn track_range_aligns_an_insertion_in_repeated_text_with_the_caret() {
        // An "a" typed at 0 leaves the caret at 1; "aaa" (2..5) moves on.
        assert_eq!(track_range(2..5, "aaaaZ", "aaaaaZ", Some(1)), 3..6);
        // "foo " typed at 0 leaves the caret at 4; the middle "foo" moves on.
        assert_eq!(
            track_range(4..7, "foo foo foo", "foo foo foo foo", Some(4)),
            8..11
        );
    }

    #[test]
    fn track_range_without_a_caret_uses_the_common_prefix() {
        assert_eq!(track_range(2..5, "aaaaZ", "aaaaaZ", None), 2..6);
        assert_eq!(
            track_range(4..7, "foo foo foo", "foo foo foo foo", None),
            4..7
        );
    }

    #[test]
    fn track_range_aligns_a_deletion_with_a_caret_at_its_start() {
        // The Delete key removes the "a" at 0, leaving the caret at 0.
        assert_eq!(track_range(2..4, "aaaa", "aaa", Some(0)), 1..3);
        // Without the caret the deletion is placed at the end.
        assert_eq!(track_range(2..4, "aaaa", "aaa", None), 2..3);
    }

    #[test]
    fn track_range_ignores_a_caret_no_alignment_allows() {
        // The edit can only be at char 6; a caret elsewhere is ignored.
        assert_eq!(
            track_range(0..5, "hello world", "hello XXworld", Some(2)),
            0..5
        );
        assert_eq!(
            track_range(6..11, "hello world", "hello XXworld", Some(2)),
            8..13
        );
    }

    #[test]
    fn track_range_alignments_reproduce_the_edit() {
        // Every alignment the caret can pick turns `before` into `after`.
        let cases = [
            ("aaaaZ", "aaaaaZ"),
            ("foo foo foo", "foo foo foo foo"),
            ("aaaa", "aaa"),
            ("abab", "ababab"),
            ("xabab", "xab"),
        ];
        for (before, after) in cases {
            let before_len = before.chars().count();
            let after_len = after.chars().count();
            for caret in 0..=after_len {
                // A one-char range at every position, tracked with this
                // caret, must cover the same char in `after`.
                for i in 0..before_len {
                    let mapped = track_range(i..i + 1, before, after, Some(caret));
                    if mapped.len() == 1 {
                        assert_eq!(
                            before.chars().nth(i),
                            after.chars().nth(mapped.start),
                            "{before:?} -> {after:?}, caret {caret}, char {i}"
                        );
                    }
                }
            }
        }
    }
}
