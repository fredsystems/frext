//! Undo/redo history for one buffer.
//!
//! Edits are stored as char-indexed splices with the cursor before and
//! after them, grouped so that one undo step reverts a burst of typing or
//! deleting. Consecutive edits coalesce into the open group only when
//! they are the same kind (`Typing` or `Deleting`), arrive within
//! [`COALESCE`] of each other, and are contiguous. An undo or redo whose
//! recorded text no longer matches the buffer clears the history and
//! leaves the text untouched.

use std::time::{Duration, Instant};

use frext_core::edit_ops::{replace_range, selected_text};

use crate::buffer::cursor::Cursor;

/// The most undo groups kept; older ones are dropped.
pub const MAX_GROUPS: usize = 100;

/// The longest pause between two edits that still coalesce.
pub const COALESCE: Duration = Duration::from_secs(1);

/// One splice: at char index `at`, `removed` was replaced by `inserted`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    /// The char index the splice starts at.
    pub at: usize,
    /// The text the splice removed.
    pub removed: String,
    /// The text the splice inserted.
    pub inserted: String,
    /// The cursor before the edit.
    pub before: Cursor,
    /// The cursor after the edit.
    pub after: Cursor,
}

/// What kind of edit was recorded, deciding whether it may coalesce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKind {
    /// Inserting typed text; coalesces with contiguous typing.
    Typing,
    /// Backspace or Delete; coalesces with a contiguous deletion run.
    Deleting,
    /// Anything else; never coalesces.
    Other,
}

/// The undo and redo stacks of one buffer.
#[derive(Debug, Default)]
pub struct EditHistory {
    undo: Vec<Vec<Edit>>,
    redo: Vec<Vec<Edit>>,
    open: Option<(EditKind, Instant)>,
}

impl EditHistory {
    /// Records `edit`, joining the open group when the coalescing rule
    /// allows it, and clears redo.
    pub fn record(&mut self, edit: Edit, kind: EditKind, now: Instant) {
        self.redo.clear();
        if self.joins_open_group(&edit, kind, now)
            && let Some(group) = self.undo.last_mut()
        {
            group.push(edit);
        } else {
            self.undo.push(vec![edit]);
            self.trim();
        }
        self.open = match kind {
            EditKind::Typing | EditKind::Deleting => Some((kind, now)),
            EditKind::Other => None,
        };
    }

    /// Records `edits` as one new group that nothing coalesces into. An
    /// empty `edits` does nothing.
    pub fn record_group(&mut self, edits: Vec<Edit>) {
        if edits.is_empty() {
            return;
        }
        self.redo.clear();
        self.undo.push(edits);
        self.open = None;
        self.trim();
    }

    /// Closes the open group so the next edit starts a new one.
    pub const fn break_group(&mut self) {
        self.open = None;
    }

    /// Reverts the last group in `text` and returns the cursor before
    /// it. Returns `None` when there is nothing to undo, or when the
    /// text no longer matches (the history is then cleared).
    pub fn undo(&mut self, text: &mut String) -> Option<Cursor> {
        let group = self.undo.pop()?;
        let mut scratch = text.clone();
        for edit in group.iter().rev() {
            if !splice(&mut scratch, edit.at, &edit.inserted, &edit.removed) {
                self.clear();
                return None;
            }
        }
        let cursor = group.first().map(|edit| edit.before);
        *text = scratch;
        self.redo.push(group);
        self.open = None;
        cursor
    }

    /// Re-applies the last undone group in `text` and returns the cursor
    /// after it. Returns `None` when there is nothing to redo, or when
    /// the text no longer matches (the history is then cleared).
    pub fn redo(&mut self, text: &mut String) -> Option<Cursor> {
        let group = self.redo.pop()?;
        let mut scratch = text.clone();
        for edit in &group {
            if !splice(&mut scratch, edit.at, &edit.removed, &edit.inserted) {
                self.clear();
                return None;
            }
        }
        let cursor = group.last().map(|edit| edit.after);
        *text = scratch;
        self.undo.push(group);
        self.open = None;
        self.trim();
        cursor
    }

    /// Forgets all history.
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.open = None;
    }

    /// Whether there is a group to undo.
    #[must_use]
    pub const fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// Whether there is a group to redo.
    #[must_use]
    pub const fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    fn joins_open_group(&self, edit: &Edit, kind: EditKind, now: Instant) -> bool {
        let Some((open_kind, last_time)) = self.open else {
            return false;
        };
        if open_kind != kind || now.saturating_duration_since(last_time) > COALESCE {
            return false;
        }
        let Some(previous) = self.undo.last().and_then(|group| group.last()) else {
            return false;
        };
        match kind {
            EditKind::Typing => edit.at == previous.at + previous.inserted.chars().count(),
            EditKind::Deleting => {
                edit.inserted.is_empty()
                    && (edit.at + edit.removed.chars().count() == previous.at
                        || edit.at == previous.at)
            }
            EditKind::Other => false,
        }
    }

    fn trim(&mut self) {
        if self.undo.len() > MAX_GROUPS {
            let excess = self.undo.len() - MAX_GROUPS;
            self.undo.drain(..excess);
        }
    }
}

/// Replaces `expected` at char index `at` of `text` with `replacement`.
/// Returns `false`, leaving `text` unchanged, when `expected` is not
/// there.
fn splice(text: &mut String, at: usize, expected: &str, replacement: &str) -> bool {
    if at > text.chars().count() {
        return false;
    }
    let end = at + expected.chars().count();
    if selected_text(text, at, end) != expected {
        return false;
    }
    replace_range(text, at, end, replacement);
    true
}

#[cfg(test)]
mod tests {
    // Tests may unwrap: a failure is a test failure, not a production panic.
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn insert(at: usize, s: &str) -> Edit {
        let len = s.chars().count();
        Edit {
            at,
            removed: String::new(),
            inserted: s.to_owned(),
            before: Cursor::at(at),
            after: Cursor::at(at + len),
        }
    }

    fn remove(at: usize, s: &str, caret_before: usize) -> Edit {
        Edit {
            at,
            removed: s.to_owned(),
            inserted: String::new(),
            before: Cursor::at(caret_before),
            after: Cursor::at(at),
        }
    }

    /// Applies `edit` to `text` and records it.
    fn apply(
        history: &mut EditHistory,
        text: &mut String,
        edit: Edit,
        kind: EditKind,
        now: Instant,
    ) {
        let end = edit.at + edit.removed.chars().count();
        assert_eq!(selected_text(text, edit.at, end), edit.removed);
        replace_range(text, edit.at, end, &edit.inserted);
        history.record(edit, kind, now);
    }

    fn ms(base: Instant, millis: u64) -> Instant {
        base + Duration::from_millis(millis)
    }

    fn type_str(
        history: &mut EditHistory,
        text: &mut String,
        at: usize,
        s: &str,
        base: Instant,
        start_ms: u64,
    ) {
        for (i, c) in s.chars().enumerate() {
            let mut buf = [0; 4];
            let edit = insert(at + i, c.encode_utf8(&mut buf));
            apply(
                history,
                text,
                edit,
                EditKind::Typing,
                ms(base, start_ms + i as u64 * 100),
            );
        }
    }

    #[test]
    fn typing_within_a_second_is_one_group() {
        let base = Instant::now();
        let mut h = EditHistory::default();
        let mut text = String::new();
        type_str(&mut h, &mut text, 0, "abc", base, 0);
        assert_eq!(text, "abc");
        assert_eq!(h.undo(&mut text), Some(Cursor::at(0)));
        assert_eq!(text, "");
        assert!(!h.can_undo());
    }

    #[test]
    fn a_pause_of_more_than_a_second_splits() {
        let base = Instant::now();
        let mut h = EditHistory::default();
        let mut text = String::new();
        apply(&mut h, &mut text, insert(0, "a"), EditKind::Typing, base);
        apply(
            &mut h,
            &mut text,
            insert(1, "b"),
            EditKind::Typing,
            ms(base, 1000),
        );
        apply(
            &mut h,
            &mut text,
            insert(2, "c"),
            EditKind::Typing,
            ms(base, 2001),
        );
        assert_eq!(h.undo(&mut text), Some(Cursor::at(2)));
        assert_eq!(text, "ab");
        assert_eq!(h.undo(&mut text), Some(Cursor::at(0)));
        assert_eq!(text, "");
    }

    #[test]
    fn break_group_splits() {
        let base = Instant::now();
        let mut h = EditHistory::default();
        let mut text = String::new();
        apply(&mut h, &mut text, insert(0, "a"), EditKind::Typing, base);
        h.break_group();
        apply(
            &mut h,
            &mut text,
            insert(1, "b"),
            EditKind::Typing,
            ms(base, 10),
        );
        assert_eq!(h.undo(&mut text), Some(Cursor::at(1)));
        assert_eq!(text, "a");
    }

    #[test]
    fn record_group_undoes_in_one_step_and_does_not_coalesce() {
        let base = Instant::now();
        let mut h = EditHistory::default();
        let mut text = String::from("x\ny");
        // An indent of both lines, applied front to back.
        replace_range(&mut text, 0, 0, "  ");
        replace_range(&mut text, 4, 4, "  ");
        assert_eq!(text, "  x\n  y");
        let mut first = insert(0, "  ");
        first.before = Cursor::spanning(0, 3);
        let mut second = insert(4, "  ");
        second.after = Cursor::spanning(0, 7);
        h.record_group(vec![first, second]);
        apply(
            &mut h,
            &mut text,
            insert(7, "z"),
            EditKind::Typing,
            ms(base, 10),
        );
        assert_eq!(h.undo(&mut text), Some(Cursor::at(7)));
        assert_eq!(text, "  x\n  y");
        assert_eq!(h.undo(&mut text), Some(Cursor::spanning(0, 3)));
        assert_eq!(text, "x\ny");
        assert!(!h.can_undo());
    }

    #[test]
    fn record_group_of_nothing_is_a_no_op() {
        let base = Instant::now();
        let mut h = EditHistory::default();
        let mut text = String::new();
        apply(&mut h, &mut text, insert(0, "a"), EditKind::Typing, base);
        h.undo(&mut text).unwrap();
        h.record_group(Vec::new());
        assert!(h.can_redo());
        assert!(!h.can_undo());
    }

    #[test]
    fn a_backspace_run_coalesces() {
        let base = Instant::now();
        let mut h = EditHistory::default();
        let mut text = String::from("abc");
        apply(
            &mut h,
            &mut text,
            remove(2, "c", 3),
            EditKind::Deleting,
            base,
        );
        apply(
            &mut h,
            &mut text,
            remove(1, "b", 2),
            EditKind::Deleting,
            ms(base, 100),
        );
        apply(
            &mut h,
            &mut text,
            remove(0, "a", 1),
            EditKind::Deleting,
            ms(base, 200),
        );
        assert_eq!(text, "");
        assert_eq!(h.undo(&mut text), Some(Cursor::at(3)));
        assert_eq!(text, "abc");
        assert!(!h.can_undo());
    }

    #[test]
    fn a_delete_run_coalesces() {
        let base = Instant::now();
        let mut h = EditHistory::default();
        let mut text = String::from("abc");
        for i in 0..3 {
            let c = text.chars().next().unwrap().to_string();
            apply(
                &mut h,
                &mut text,
                remove(0, &c, 0),
                EditKind::Deleting,
                ms(base, i * 100),
            );
        }
        assert_eq!(text, "");
        assert_eq!(h.undo(&mut text), Some(Cursor::at(0)));
        assert_eq!(text, "abc");
        assert!(!h.can_undo());
    }

    #[test]
    fn typing_then_deleting_are_two_groups() {
        let base = Instant::now();
        let mut h = EditHistory::default();
        let mut text = String::new();
        type_str(&mut h, &mut text, 0, "ab", base, 0);
        apply(
            &mut h,
            &mut text,
            remove(1, "b", 2),
            EditKind::Deleting,
            ms(base, 300),
        );
        assert_eq!(text, "a");
        assert_eq!(h.undo(&mut text), Some(Cursor::at(2)));
        assert_eq!(text, "ab");
        assert_eq!(h.undo(&mut text), Some(Cursor::at(0)));
        assert_eq!(text, "");
    }

    #[test]
    fn a_non_contiguous_insert_splits() {
        let base = Instant::now();
        let mut h = EditHistory::default();
        let mut text = String::new();
        type_str(&mut h, &mut text, 0, "ab", base, 0);
        apply(
            &mut h,
            &mut text,
            insert(0, "z"),
            EditKind::Typing,
            ms(base, 300),
        );
        assert_eq!(text, "zab");
        assert_eq!(h.undo(&mut text), Some(Cursor::at(0)));
        assert_eq!(text, "ab");
        assert!(h.can_undo());
    }

    #[test]
    fn redo_after_undo_and_a_new_edit_clears_redo() {
        let base = Instant::now();
        let mut h = EditHistory::default();
        let mut text = String::new();
        type_str(&mut h, &mut text, 0, "abc", base, 0);
        h.undo(&mut text).unwrap();
        assert!(h.can_redo());
        assert_eq!(h.redo(&mut text), Some(Cursor::at(3)));
        assert_eq!(text, "abc");
        assert!(!h.can_redo());

        h.undo(&mut text).unwrap();
        apply(
            &mut h,
            &mut text,
            insert(0, "x"),
            EditKind::Typing,
            ms(base, 5000),
        );
        assert!(!h.can_redo());
        assert_eq!(h.redo(&mut text), None);
        assert_eq!(text, "x");
    }

    #[test]
    fn the_101st_group_drops_the_oldest() {
        let base = Instant::now();
        let mut h = EditHistory::default();
        let mut text = String::new();
        for i in 0..=MAX_GROUPS {
            apply(
                &mut h,
                &mut text,
                insert(i, "a"),
                EditKind::Other,
                ms(base, i as u64),
            );
        }
        let mut undone = 0;
        while h.undo(&mut text).is_some() {
            undone += 1;
        }
        assert_eq!(undone, MAX_GROUPS);
        assert_eq!(text, "a");
    }

    #[test]
    fn undo_after_an_outside_change_is_refused_and_clears() {
        let base = Instant::now();
        let mut h = EditHistory::default();
        let mut text = String::new();
        type_str(&mut h, &mut text, 0, "ab", base, 0);
        h.break_group();
        type_str(&mut h, &mut text, 2, "cd", base, 5000);
        text = String::from("abXY");
        assert_eq!(h.undo(&mut text), None);
        assert_eq!(text, "abXY");
        assert!(!h.can_undo());
        assert!(!h.can_redo());
    }

    #[test]
    fn a_partial_mismatch_leaves_the_text_untouched() {
        let base = Instant::now();
        let mut h = EditHistory::default();
        let mut text = String::new();
        type_str(&mut h, &mut text, 0, "abc", base, 0);
        // The last edit still matches, the first does not.
        text = String::from("Xbc");
        assert_eq!(h.undo(&mut text), None);
        assert_eq!(text, "Xbc");
        assert!(!h.can_undo());
    }

    #[test]
    fn redo_after_an_outside_change_is_refused_and_clears() {
        let base = Instant::now();
        let mut h = EditHistory::default();
        let mut text = String::from("ab");
        apply(
            &mut h,
            &mut text,
            remove(0, "ab", 2),
            EditKind::Deleting,
            base,
        );
        h.undo(&mut text).unwrap();
        assert_eq!(text, "ab");
        text = String::from("zz");
        assert_eq!(h.redo(&mut text), None);
        assert_eq!(text, "zz");
        assert!(!h.can_redo());
    }

    #[test]
    fn multi_byte_text_round_trips() {
        let base = Instant::now();
        let mut h = EditHistory::default();
        let mut text = String::from("a");
        type_str(&mut h, &mut text, 1, "日本", base, 0);
        assert_eq!(text, "a日本");
        apply(
            &mut h,
            &mut text,
            remove(1, "日", 2),
            EditKind::Deleting,
            ms(base, 2000),
        );
        assert_eq!(text, "a本");
        assert_eq!(h.undo(&mut text), Some(Cursor::at(2)));
        assert_eq!(text, "a日本");
        assert_eq!(h.undo(&mut text), Some(Cursor::at(1)));
        assert_eq!(text, "a");
        assert_eq!(h.redo(&mut text), Some(Cursor::at(3)));
        assert_eq!(text, "a日本");
        assert_eq!(h.redo(&mut text), Some(Cursor::at(1)));
        assert_eq!(text, "a本");
    }
}
