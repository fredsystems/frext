//! Edit actions and applying them to a buffer's text.
//!
//! An [`EditAction`] is one user intent on the buffer: a caret motion,
//! typing, a deletion, an outdent, or undo/redo. [`apply`] carries it out
//! on an [`EditTarget`] (the text, its cursor and its history) and
//! records every text change as an [`Edit`] so it can be undone. Enter
//! and Tab match the GUI exactly: Enter inserts `"\n"` with no indent,
//! Tab inserts a literal `'\t'`, and Shift+Tab is egui's
//! `decrease_indentation`.

use std::time::Instant;

use frext_core::edit_ops::{replace_range, selected_text};

use crate::buffer::cursor::{Cursor, Motion, Selecting, next_grapheme, previous_grapheme};
use crate::buffer::history::{Edit, EditHistory, EditKind};
use crate::buffer::words::{next_word_boundary, previous_word_boundary};
use crate::buffer::wrap::WrapLayout;

/// The most leading spaces one outdent removes.
const OUTDENT_SPACES: usize = 4;

/// One user intent on the buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditAction {
    /// Move the caret, or extend the selection.
    Move(Motion, Selecting),
    /// Type a char, replacing any selection. Control chars are ignored.
    Type(char),
    /// Insert `"\n"`, replacing any selection.
    Newline,
    /// Insert `'\t'`, replacing any selection.
    Tab,
    /// Delete any selection, then remove one indent from the caret's line.
    Outdent,
    /// Delete the selection, or the grapheme before the caret.
    Backspace,
    /// Delete the selection, or the grapheme after the caret.
    Delete,
    /// Delete the selection, or back to the previous word boundary.
    WordBackspace,
    /// Delete the selection, or up to the next word boundary.
    WordDelete,
    /// Revert the last undo group.
    Undo,
    /// Re-apply the last undone group.
    Redo,
}

/// What applying an action did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applied {
    /// Nothing changed.
    Unchanged,
    /// The cursor moved; the text is unchanged.
    Moved,
    /// The text changed.
    Edited,
}

/// The buffer state an action works on.
pub struct EditTarget<'a> {
    /// The buffer text.
    pub text: &'a mut String,
    /// The buffer's caret and selection.
    pub cursor: &'a mut Cursor,
    /// The buffer's undo/redo history.
    pub history: &'a mut EditHistory,
}

/// Applies `action` to `target`. `layout` is the current wrap layout of
/// the text and `page_rows` the visible row count, both for motions.
/// `now` decides whether a typing or deleting edit coalesces with the
/// previous one.
pub fn apply(
    target: EditTarget<'_>,
    layout: &WrapLayout,
    action: EditAction,
    page_rows: u16,
    now: Instant,
) -> Applied {
    let EditTarget {
        text,
        cursor,
        history,
    } = target;
    let before = *cursor;
    cursor.clamp(text);
    match action {
        EditAction::Move(motion, selecting) => {
            cursor.apply(text, layout, motion, selecting, page_rows);
            history.break_group();
            if (cursor.anchor(), cursor.caret()) == (before.anchor(), before.caret()) {
                Applied::Unchanged
            } else {
                Applied::Moved
            }
        }
        EditAction::Type(c) if c.is_control() => Applied::Unchanged,
        EditAction::Type(c) => type_char(text, cursor, history, c, now),
        EditAction::Tab => type_char(text, cursor, history, '\t', now),
        EditAction::Newline => {
            let edit = replace_selection(text, cursor, "\n");
            history.record(edit, EditKind::Other, now);
            Applied::Edited
        }
        EditAction::Backspace => delete(text, cursor, history, now, |t, c| {
            (previous_grapheme(t, c), EditKind::Deleting)
        }),
        EditAction::Delete => delete(text, cursor, history, now, |t, c| {
            (next_grapheme(t, c), EditKind::Deleting)
        }),
        EditAction::WordBackspace => delete(text, cursor, history, now, |t, c| {
            (previous_word_boundary(t, c), EditKind::Other)
        }),
        EditAction::WordDelete => delete(text, cursor, history, now, |t, c| {
            (next_word_boundary(t, c), EditKind::Other)
        }),
        EditAction::Outdent => outdent(text, cursor, history),
        EditAction::Undo => {
            let restored = history.undo(text);
            restore(text, cursor, restored)
        }
        EditAction::Redo => {
            let restored = history.redo(text);
            restore(text, cursor, restored)
        }
    }
}

/// Inserts `inserted` (the paste path), replacing the selection, as one
/// undo group of its own. `"\r\n"` and lone `'\r'` become `"\n"`. An
/// empty `inserted` with no selection is [`Applied::Unchanged`]. A paste
/// never coalesces, so `_now` does not affect grouping.
pub fn insert_text(target: EditTarget<'_>, inserted: &str, _now: Instant) -> Applied {
    let EditTarget {
        text,
        cursor,
        history,
    } = target;
    cursor.clamp(text);
    let normalised = inserted.replace("\r\n", "\n").replace('\r', "\n");
    if normalised.is_empty() && !cursor.has_selection() {
        return Applied::Unchanged;
    }
    let edit = replace_selection(text, cursor, &normalised);
    history.record_group(vec![edit]);
    Applied::Edited
}

/// Replaces chars `at..end` of `text` with `inserted`, leaving the
/// cursor after the insertion, and returns the recorded edit.
fn splice(text: &mut String, cursor: &mut Cursor, at: usize, end: usize, inserted: &str) -> Edit {
    let before = *cursor;
    let removed = selected_text(text, at, end);
    replace_range(text, at, end, inserted);
    *cursor = Cursor::at(at + inserted.chars().count());
    Edit {
        at,
        removed,
        inserted: inserted.to_owned(),
        before,
        after: *cursor,
    }
}

/// Replaces the selection (or inserts at the caret) with `inserted`.
fn replace_selection(text: &mut String, cursor: &mut Cursor, inserted: &str) -> Edit {
    let range = cursor.selection();
    splice(text, cursor, range.start, range.end, inserted)
}

/// Types `c`: `Typing` with no selection, `Other` over a selection.
fn type_char(
    text: &mut String,
    cursor: &mut Cursor,
    history: &mut EditHistory,
    c: char,
    now: Instant,
) -> Applied {
    let kind = if cursor.has_selection() {
        EditKind::Other
    } else {
        EditKind::Typing
    };
    let mut buf = [0; 4];
    let edit = replace_selection(text, cursor, c.encode_utf8(&mut buf));
    history.record(edit, kind, now);
    Applied::Edited
}

/// Deletes the selection (`Other`), or else the chars between the caret
/// and the position `target` gives, recorded with the kind it gives.
fn delete(
    text: &mut String,
    cursor: &mut Cursor,
    history: &mut EditHistory,
    now: Instant,
    target: impl Fn(&str, usize) -> (usize, EditKind),
) -> Applied {
    let (range, kind) = if cursor.has_selection() {
        (cursor.selection(), EditKind::Other)
    } else {
        let caret = cursor.caret();
        let (to, kind) = target(text, caret);
        (caret.min(to)..caret.max(to), kind)
    };
    if range.is_empty() {
        return Applied::Unchanged;
    }
    let edit = splice(text, cursor, range.start, range.end, "");
    history.record(edit, kind, now);
    Applied::Edited
}

/// egui's Shift+Tab: delete any selection, then remove one leading
/// `'\t'`, or up to 4 leading spaces, from the caret's logical line. Both
/// steps are one undo group.
fn outdent(text: &mut String, cursor: &mut Cursor, history: &mut EditHistory) -> Applied {
    let mut edits = Vec::new();
    if cursor.has_selection() {
        edits.push(replace_selection(text, cursor, ""));
    }
    let caret = cursor.caret();
    let line_start = text
        .chars()
        .take(caret)
        .enumerate()
        .filter(|&(_, c)| c == '\n')
        .last()
        .map_or(0, |(i, _)| i + 1);
    let mut rest = text.chars().skip(line_start);
    let removed = if rest.clone().next() == Some('\t') {
        1
    } else {
        let spaces: Vec<char> = rest.by_ref().take(OUTDENT_SPACES).collect();
        if spaces.iter().all(|&c| c == ' ') {
            spaces.len()
        } else {
            0
        }
    };
    if removed > 0 {
        let new_caret = if caret == line_start {
            caret
        } else {
            caret.saturating_sub(removed).max(line_start)
        };
        let before = *cursor;
        let removed_text = selected_text(text, line_start, line_start + removed);
        replace_range(text, line_start, line_start + removed, "");
        *cursor = Cursor::at(new_caret);
        edits.push(Edit {
            at: line_start,
            removed: removed_text,
            inserted: String::new(),
            before,
            after: *cursor,
        });
    }
    if edits.is_empty() {
        return Applied::Unchanged;
    }
    history.record_group(edits);
    Applied::Edited
}

/// Restores the cursor an undo or redo returned.
fn restore(text: &str, cursor: &mut Cursor, restored: Option<Cursor>) -> Applied {
    match restored {
        Some(c) => {
            *cursor = c;
            cursor.clamp(text);
            Applied::Edited
        }
        None => Applied::Unchanged,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A buffer under test: text, cursor and history.
    struct Buf {
        text: String,
        cursor: Cursor,
        history: EditHistory,
        now: Instant,
    }

    impl Buf {
        fn new(text: &str, cursor: Cursor) -> Self {
            Self {
                text: text.to_owned(),
                cursor,
                history: EditHistory::default(),
                now: Instant::now(),
            }
        }

        fn act(&mut self, action: EditAction) -> Applied {
            let layout = WrapLayout::new(&self.text, 80);
            apply(
                EditTarget {
                    text: &mut self.text,
                    cursor: &mut self.cursor,
                    history: &mut self.history,
                },
                &layout,
                action,
                10,
                self.now,
            )
        }

        fn paste(&mut self, s: &str) -> Applied {
            insert_text(
                EditTarget {
                    text: &mut self.text,
                    cursor: &mut self.cursor,
                    history: &mut self.history,
                },
                s,
                self.now,
            )
        }

        fn assert(&self, text: &str, cursor: Cursor) {
            assert_eq!(self.text, text);
            assert_eq!(self.cursor, cursor);
        }
    }

    #[test]
    fn move_moves_and_breaks_the_group() {
        let mut b = Buf::new("abc", Cursor::at(0));
        assert_eq!(b.act(EditAction::Type('x')), Applied::Edited);
        assert_eq!(
            b.act(EditAction::Move(Motion::Left, Selecting::Move)),
            Applied::Moved
        );
        assert_eq!(
            b.act(EditAction::Move(Motion::Right, Selecting::Move)),
            Applied::Moved
        );
        b.act(EditAction::Type('y'));
        b.assert("xyabc", Cursor::at(2));
        b.act(EditAction::Undo);
        b.assert("xabc", Cursor::at(1));
    }

    #[test]
    fn move_with_no_change_is_unchanged() {
        let mut b = Buf::new("abc", Cursor::at(0));
        assert_eq!(
            b.act(EditAction::Move(Motion::Left, Selecting::Move)),
            Applied::Unchanged
        );
        // Up on the first row sets only the preferred column.
        assert_eq!(
            b.act(EditAction::Move(Motion::Up, Selecting::Move)),
            Applied::Unchanged
        );
        assert_eq!(b.text, "abc");
        assert_eq!((b.cursor.anchor(), b.cursor.caret()), (0, 0));
        assert_ne!(b.cursor, Cursor::at(0));
    }

    #[test]
    fn type_inserts_and_coalesces() {
        let mut b = Buf::new("ab", Cursor::at(1));
        assert_eq!(b.act(EditAction::Type('x')), Applied::Edited);
        assert_eq!(b.act(EditAction::Type('y')), Applied::Edited);
        b.assert("axyb", Cursor::at(3));
        assert_eq!(b.act(EditAction::Undo), Applied::Edited);
        b.assert("ab", Cursor::at(1));
    }

    #[test]
    fn type_a_control_char_is_unchanged() {
        let mut b = Buf::new("ab", Cursor::at(1));
        assert_eq!(b.act(EditAction::Type('\u{7}')), Applied::Unchanged);
        b.assert("ab", Cursor::at(1));
        assert!(!b.history.can_undo());
    }

    #[test]
    fn typing_over_a_selection_then_undo_restores_it() {
        let mut b = Buf::new("hello world", Cursor::spanning(6, 11));
        assert_eq!(b.act(EditAction::Type('X')), Applied::Edited);
        b.assert("hello X", Cursor::at(7));
        // Replacing a selection does not coalesce with the next char.
        b.act(EditAction::Type('Y'));
        b.act(EditAction::Undo);
        b.act(EditAction::Undo);
        b.assert("hello world", Cursor::spanning(6, 11));
    }

    #[test]
    fn newline_inserts_with_no_indent() {
        let mut b = Buf::new("    ab", Cursor::at(5));
        assert_eq!(b.act(EditAction::Newline), Applied::Edited);
        b.assert("    a\nb", Cursor::at(6));
        b.act(EditAction::Undo);
        b.assert("    ab", Cursor::at(5));
    }

    #[test]
    fn newline_replaces_a_selection() {
        let mut b = Buf::new("abcd", Cursor::spanning(3, 1));
        b.act(EditAction::Newline);
        b.assert("a\nd", Cursor::at(2));
    }

    #[test]
    fn tab_inserts_a_literal_tab() {
        let mut b = Buf::new("ab", Cursor::at(1));
        assert_eq!(b.act(EditAction::Tab), Applied::Edited);
        b.assert("a\tb", Cursor::at(2));
        b.act(EditAction::Undo);
        b.assert("ab", Cursor::at(1));
    }

    #[test]
    fn tab_with_a_selection_replaces_it() {
        let mut b = Buf::new("one\ntwo", Cursor::spanning(0, 7));
        assert_eq!(b.act(EditAction::Tab), Applied::Edited);
        b.assert("\t", Cursor::at(1));
        b.act(EditAction::Undo);
        b.assert("one\ntwo", Cursor::spanning(0, 7));
    }

    #[test]
    fn backspace_deletes_and_coalesces() {
        let mut b = Buf::new("abc", Cursor::at(3));
        assert_eq!(b.act(EditAction::Backspace), Applied::Edited);
        assert_eq!(b.act(EditAction::Backspace), Applied::Edited);
        b.assert("a", Cursor::at(1));
        b.act(EditAction::Undo);
        b.assert("abc", Cursor::at(3));
    }

    #[test]
    fn backspace_at_the_start_is_unchanged() {
        let mut b = Buf::new("abc", Cursor::at(0));
        assert_eq!(b.act(EditAction::Backspace), Applied::Unchanged);
        b.assert("abc", Cursor::at(0));
    }

    #[test]
    fn backspace_deletes_a_selection() {
        let mut b = Buf::new("abcd", Cursor::spanning(1, 3));
        b.act(EditAction::Backspace);
        b.assert("ad", Cursor::at(1));
        b.act(EditAction::Undo);
        b.assert("abcd", Cursor::spanning(1, 3));
    }

    #[test]
    fn backspace_removes_a_zwj_emoji_whole() {
        let mut b = Buf::new("a\u{1F469}\u{200D}\u{1F4BB}", Cursor::at(4));
        b.act(EditAction::Backspace);
        b.assert("a", Cursor::at(1));
        b.act(EditAction::Undo);
        b.assert("a\u{1F469}\u{200D}\u{1F4BB}", Cursor::at(4));
    }

    #[test]
    fn delete_deletes_forward_and_coalesces() {
        let mut b = Buf::new("abc", Cursor::at(0));
        assert_eq!(b.act(EditAction::Delete), Applied::Edited);
        assert_eq!(b.act(EditAction::Delete), Applied::Edited);
        b.assert("c", Cursor::at(0));
        b.act(EditAction::Undo);
        b.assert("abc", Cursor::at(0));
    }

    #[test]
    fn delete_steps_over_crlf_and_is_unchanged_at_the_end() {
        let mut b = Buf::new("a\r\nb", Cursor::at(1));
        b.act(EditAction::Delete);
        b.assert("ab", Cursor::at(1));
        let mut b = Buf::new("ab", Cursor::at(2));
        assert_eq!(b.act(EditAction::Delete), Applied::Unchanged);
    }

    #[test]
    fn word_backspace_removes_the_last_word() {
        let mut b = Buf::new("foo.bar baz", Cursor::at(11));
        assert_eq!(b.act(EditAction::WordBackspace), Applied::Edited);
        b.assert("foo.bar ", Cursor::at(8));
        b.act(EditAction::Undo);
        b.assert("foo.bar baz", Cursor::at(11));
    }

    #[test]
    fn word_backspace_deletes_a_selection_and_is_unchanged_at_the_start() {
        let mut b = Buf::new("abc def", Cursor::spanning(1, 2));
        b.act(EditAction::WordBackspace);
        b.assert("ac def", Cursor::at(1));
        let mut b = Buf::new("foo", Cursor::at(0));
        assert_eq!(b.act(EditAction::WordBackspace), Applied::Unchanged);
    }

    #[test]
    fn word_delete_removes_the_next_word() {
        let mut b = Buf::new("foo bar", Cursor::at(0));
        assert_eq!(b.act(EditAction::WordDelete), Applied::Edited);
        b.assert(" bar", Cursor::at(0));
        b.act(EditAction::Undo);
        b.assert("foo bar", Cursor::at(0));
        let mut b = Buf::new("foo", Cursor::at(3));
        assert_eq!(b.act(EditAction::WordDelete), Applied::Unchanged);
    }

    #[test]
    fn outdent_removes_a_tab() {
        let mut b = Buf::new("x\n\tab", Cursor::at(4));
        assert_eq!(b.act(EditAction::Outdent), Applied::Edited);
        b.assert("x\nab", Cursor::at(3));
        b.act(EditAction::Undo);
        b.assert("x\n\tab", Cursor::at(4));
    }

    #[test]
    fn outdent_removes_four_spaces() {
        let mut b = Buf::new("      ab", Cursor::at(7));
        b.act(EditAction::Outdent);
        b.assert("  ab", Cursor::at(3));
        b.act(EditAction::Undo);
        b.assert("      ab", Cursor::at(7));
    }

    #[test]
    fn outdent_removes_two_trailing_spaces_the_egui_quirk() {
        let mut b = Buf::new("x\n  ", Cursor::at(4));
        assert_eq!(b.act(EditAction::Outdent), Applied::Edited);
        b.assert("x\n", Cursor::at(2));
    }

    #[test]
    fn outdent_on_a_line_starting_with_text_does_nothing() {
        let mut b = Buf::new("a\nx  y", Cursor::at(4));
        assert_eq!(b.act(EditAction::Outdent), Applied::Unchanged);
        b.assert("a\nx  y", Cursor::at(4));
        assert!(!b.history.can_undo());
        // Two spaces then text is not an indent either.
        let mut b = Buf::new("  ab", Cursor::at(3));
        assert_eq!(b.act(EditAction::Outdent), Applied::Unchanged);
    }

    #[test]
    fn outdent_with_the_caret_at_the_line_start_keeps_it() {
        let mut b = Buf::new("a\n\tb", Cursor::at(2));
        b.act(EditAction::Outdent);
        b.assert("a\nb", Cursor::at(2));
    }

    #[test]
    fn outdent_on_an_empty_buffer_is_unchanged() {
        let mut b = Buf::new("", Cursor::at(0));
        assert_eq!(b.act(EditAction::Outdent), Applied::Unchanged);
    }

    #[test]
    fn outdent_deletes_the_selection_then_outdents_in_one_step() {
        let mut b = Buf::new("\tabc", Cursor::spanning(2, 4));
        assert_eq!(b.act(EditAction::Outdent), Applied::Edited);
        b.assert("a", Cursor::at(1));
        b.act(EditAction::Undo);
        b.assert("\tabc", Cursor::spanning(2, 4));
        assert!(!b.history.can_undo());
    }

    #[test]
    fn undo_with_nothing_is_unchanged() {
        let mut b = Buf::new("ab", Cursor::at(1));
        assert_eq!(b.act(EditAction::Undo), Applied::Unchanged);
        assert_eq!(b.act(EditAction::Redo), Applied::Unchanged);
        b.assert("ab", Cursor::at(1));
    }

    #[test]
    fn redo_after_undo() {
        let mut b = Buf::new("ab", Cursor::spanning(0, 1));
        b.act(EditAction::Type('z'));
        b.assert("zb", Cursor::at(1));
        b.act(EditAction::Undo);
        b.assert("ab", Cursor::spanning(0, 1));
        assert_eq!(b.act(EditAction::Redo), Applied::Edited);
        b.assert("zb", Cursor::at(1));
        assert_eq!(b.act(EditAction::Redo), Applied::Unchanged);
    }

    #[test]
    fn insert_text_normalises_line_endings_in_one_undo() {
        let mut b = Buf::new("ab", Cursor::at(1));
        b.act(EditAction::Type('x'));
        assert_eq!(b.paste("1\r\n2\r3\n"), Applied::Edited);
        b.assert("ax1\n2\n3\nb", Cursor::at(8));
        b.act(EditAction::Undo);
        b.assert("axb", Cursor::at(2));
        b.act(EditAction::Undo);
        b.assert("ab", Cursor::at(1));
    }

    #[test]
    fn insert_text_replaces_a_selection_and_empty_is_unchanged() {
        let mut b = Buf::new("abcd", Cursor::spanning(1, 3));
        assert_eq!(b.paste(""), Applied::Edited);
        b.assert("ad", Cursor::at(1));
        assert_eq!(b.paste(""), Applied::Unchanged);
        b.assert("ad", Cursor::at(1));
    }

    #[test]
    fn edits_drop_the_preferred_column() {
        // Down sets a preferred column of 4; after typing it is gone.
        let mut b = Buf::new("abcdef\nab\nabcdef", Cursor::at(4));
        b.act(EditAction::Move(Motion::Down, Selecting::Move));
        assert_eq!(b.cursor.caret(), 9);
        b.act(EditAction::Type('x'));
        b.act(EditAction::Move(Motion::Down, Selecting::Move));
        // Column 3 of the last line, not column 4.
        assert_eq!(b.cursor.caret(), 11 + 3);
    }
}
