//! Editing commands: clipboard cut/copy/paste, select all, and the
//! proofreading fixes (apply a suggestion, add to the dictionary, ignore).
//!
//! They act on the selection last reported through
//! [`Editor::set_selection`] (a char range; a missing selection is a caret
//! at the start of the buffer).

use std::ops::Range;

use super::{BufferChange, DictionaryFile, Editor, EditorFocus, Effect, Outcome};
use crate::edit_ops;

impl Editor {
    /// Copy the selection to the clipboard, then delete it from the buffer,
    /// leaving the caret at the selection's start. A failed clipboard write
    /// leaves the buffer untouched, so the text is never lost.
    pub(super) fn cut(&mut self) -> Outcome {
        let selection = self.current_selection();
        let Some(tab) = self.tabs.get(self.active) else {
            return Outcome::default();
        };
        let text = edit_ops::selected_text(&tab.text, selection.start, selection.end);
        if text.is_empty() {
            return Outcome::default();
        }
        if !self.clipboard.write_text(text) {
            return Outcome::default();
        }
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return Outcome::default();
        };
        let caret = edit_ops::delete_range(&mut tab.text, selection.start, selection.end);
        self.edited_with_caret(caret)
    }

    /// Copy the selection to the clipboard; an empty selection copies
    /// nothing.
    pub(super) fn copy(&mut self) {
        let selection = self.current_selection();
        if let Some(tab) = self.tabs.get(self.active) {
            let text = edit_ops::selected_text(&tab.text, selection.start, selection.end);
            if !text.is_empty() {
                self.clipboard.write_text(text);
            }
        }
    }

    /// Replace the selection (or insert at the caret) with the clipboard
    /// text, leaving the caret just past it. An empty or unreadable
    /// clipboard is a no-op.
    pub(super) fn paste(&mut self) -> Outcome {
        let selection = self.current_selection();
        let Some(clip) = self.clipboard.read_text() else {
            return Outcome::default();
        };
        if clip.is_empty() {
            return Outcome::default();
        }
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return Outcome::default();
        };
        let caret = edit_ops::replace_range(&mut tab.text, selection.start, selection.end, &clip);
        self.edited_with_caret(caret)
    }

    /// Select the entire active buffer and focus the editor.
    pub(super) fn select_all(&self) -> Outcome {
        let mut outcome = Outcome::default();
        if let Some(tab) = self.tabs.get(self.active) {
            let len = tab.text.chars().count();
            outcome.effects.push(Effect::SetSelection {
                tab_id: tab.id,
                chars: 0..len,
                focus: EditorFocus::Focus,
            });
        }
        outcome
    }

    /// Replace the flagged `bytes` with `replacement`, unless the buffer
    /// changed since the diagnostic was shown and the range no longer holds
    /// `flagged`. The caret lands just past the replacement.
    pub(super) fn apply_suggestion(
        &mut self,
        bytes: Range<usize>,
        flagged: &str,
        replacement: &str,
    ) -> Outcome {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return Outcome::default();
        };
        // The buffer may have changed since the menu opened; only replace
        // text that is still exactly what was flagged.
        if tab.text.get(bytes.clone()) != Some(flagged) {
            return Outcome::default();
        }
        let caret = tab.text.get(..bytes.start).map_or(0, |s| s.chars().count())
            + replacement.chars().count();
        tab.text.replace_range(bytes, replacement);
        self.edited_with_caret(caret)
    }

    /// Add `word` to the personal dictionary, saving it unless the
    /// dictionary file was unreadable at startup.
    pub(super) fn add_to_dictionary(&mut self, word: &str) {
        if !self.proofread.add_word(word) {
            return;
        }
        match self.dictionary_file {
            DictionaryFile::Loaded => {
                if let Err(err) = self.store.merge_dictionary(self.proofread.user_words()) {
                    log::error!("failed to save the personal dictionary: {err}");
                }
            }
            DictionaryFile::Unreadable => log::warn!(
                "added {word:?} for this session only: the dictionary file could \
                 not be read at startup, so it is not being overwritten"
            ),
        }
    }

    /// The selection the editing commands act on.
    fn current_selection(&self) -> Range<usize> {
        self.active_selection().unwrap_or(0..0)
    }

    /// The active buffer was just edited: persist it, refresh any open
    /// search, and move the (focused) caret to `caret`.
    fn edited_with_caret(&mut self, caret: usize) -> Outcome {
        self.commit_edit();
        let effects = self
            .tabs
            .get(self.active)
            .map(|tab| Effect::SetSelection {
                tab_id: tab.id,
                chars: caret..caret,
                focus: EditorFocus::Focus,
            })
            .into_iter()
            .collect();
        Outcome {
            buffer: BufferChange::Edited,
            effects,
        }
    }
}

#[cfg(test)]
mod tests {
    // `unwrap`/`expect` are acceptable in test code: a panic on an unexpected
    // `Err`/`None` is exactly the failure signal we want from a test.
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::fs;
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::clipboard::{Clipboard, MemoryClipboard};
    use crate::editor::{Command, SearchBar};
    use crate::persistence::Store;

    /// Create an isolated temp directory unique to this test.
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "frext-editing-test-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A fresh editor backed by a store rooted in `dir/state`.
    fn editor_in(dir: &Path) -> Editor {
        let store = Store::at(dir.join("state")).unwrap();
        Editor::open(store, Box::new(MemoryClipboard::default()), &[], None)
    }

    /// An editor whose active tab holds `text` (not yet persisted).
    fn editor_with_text(dir: &Path, text: &str) -> Editor {
        let mut editor = editor_in(dir);
        let active = editor.active_index();
        editor.tabs[active].text = text.to_owned();
        editor
    }

    fn suggestion(bytes: Range<usize>, flagged: &str, replacement: &str) -> Command {
        Command::ApplySuggestion {
            bytes,
            flagged: flagged.to_owned(),
            replacement: replacement.to_owned(),
        }
    }

    /// The effect placing a focused caret at `chars` in the active tab.
    fn caret(editor: &Editor, chars: usize) -> Vec<Effect> {
        vec![Effect::SetSelection {
            tab_id: editor.active_tab().unwrap().id,
            chars: chars..chars,
            focus: EditorFocus::Focus,
        }]
    }

    #[test]
    fn cut_deletes_the_selection_from_the_buffer() {
        let dir = temp_dir("cut");
        let mut editor = editor_with_text(&dir, "hello world");

        // Cut "hello " (chars 0..6).
        editor.set_selection(Some(0..6));
        let outcome = editor.apply(Command::Cut);

        assert_eq!(outcome.buffer, BufferChange::Edited);
        assert_eq!(editor.active_tab().unwrap().text, "world");
        assert_eq!(editor.clipboard.read_text().as_deref(), Some("hello "));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cut_moves_the_caret_to_the_selection_start() {
        let dir = temp_dir("cut-caret");
        let mut editor = editor_with_text(&dir, "hello world");

        // Cut "o w" (chars 4..7).
        editor.set_selection(Some(4..7));
        let outcome = editor.apply(Command::Cut);

        assert_eq!(editor.active_tab().unwrap().text, "hellorld");
        assert_eq!(outcome.effects, caret(&editor, 4));

        fs::remove_dir_all(&dir).unwrap();
    }

    /// A clipboard that is never available: every write fails.
    struct FailingClipboard;

    impl Clipboard for FailingClipboard {
        fn read_text(&mut self) -> Option<String> {
            None
        }

        fn write_text(&mut self, _text: String) -> bool {
            false
        }
    }

    #[test]
    fn cut_keeps_the_selection_when_the_clipboard_write_fails() {
        let dir = temp_dir("cut-clipboard-fails");
        let mut editor = editor_with_text(&dir, "hello world");
        editor.clipboard = Box::new(FailingClipboard);
        let dirty_before = editor.active_tab().unwrap().is_dirty();

        editor.set_selection(Some(0..6));
        let outcome = editor.apply(Command::Cut);

        assert_eq!(outcome, Outcome::default());
        assert_eq!(editor.active_tab().unwrap().text, "hello world");
        assert_eq!(editor.active_tab().unwrap().is_dirty(), dirty_before);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cut_with_an_empty_selection_is_a_noop() {
        let dir = temp_dir("cut-empty");
        let mut editor = editor_with_text(&dir, "hello world");
        assert!(editor.clipboard.write_text("before".to_owned()));

        editor.set_selection(Some(3..3));
        let outcome = editor.apply(Command::Cut);

        assert_eq!(outcome, Outcome::default());
        assert_eq!(editor.active_tab().unwrap().text, "hello world");
        assert_eq!(editor.clipboard.read_text().as_deref(), Some("before"));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn paste_replaces_the_selection_with_clipboard_text() {
        let dir = temp_dir("paste");
        let mut editor = editor_with_text(&dir, "hello world");
        assert!(editor.clipboard.write_text("there".to_owned()));

        // Paste over "world" (chars 6..11).
        editor.set_selection(Some(6..11));
        let outcome = editor.apply(Command::Paste);

        assert_eq!(outcome.buffer, BufferChange::Edited);
        assert_eq!(editor.active_tab().unwrap().text, "hello there");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn paste_is_char_aware() {
        let dir = temp_dir("paste-chars");
        let mut editor = editor_with_text(&dir, "café wörld");
        assert!(editor.clipboard.write_text("naïve ".to_owned()));

        // Paste over "wörld" (chars 5..10), after the multi-byte "é".
        editor.set_selection(Some(5..10));
        let outcome = editor.apply(Command::Paste);

        assert_eq!(outcome.buffer, BufferChange::Edited);
        assert_eq!(editor.active_tab().unwrap().text, "café naïve ");
        assert_eq!(outcome.effects, caret(&editor, 11));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn paste_with_an_empty_clipboard_is_a_noop() {
        let dir = temp_dir("paste-empty");
        let mut editor = editor_with_text(&dir, "unchanged");

        editor.set_selection(Some(0..2));
        let outcome = editor.apply(Command::Paste);

        assert_eq!(outcome.buffer, BufferChange::Unchanged);
        assert_eq!(editor.active_tab().unwrap().text, "unchanged");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn copy_does_not_mutate_the_buffer() {
        let dir = temp_dir("copy");
        let mut editor = editor_with_text(&dir, "keep me");

        editor.set_selection(Some(0..4));
        let outcome = editor.apply(Command::Copy);

        assert_eq!(outcome.buffer, BufferChange::Unchanged);
        assert_eq!(editor.active_tab().unwrap().text, "keep me");
        assert_eq!(editor.clipboard.read_text().as_deref(), Some("keep"));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn select_all_selects_every_char() {
        let dir = temp_dir("select-all");
        let mut editor = editor_with_text(&dir, "café naïve");

        let outcome = editor.apply(Command::SelectAll);

        assert_eq!(outcome.buffer, BufferChange::Unchanged);
        assert_eq!(
            outcome.effects,
            vec![Effect::SetSelection {
                tab_id: editor.active_tab().unwrap().id,
                chars: 0..10,
                focus: EditorFocus::Focus,
            }]
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_edit_command_refreshes_open_search_matches() {
        let dir = temp_dir("edit-refreshes-search");
        let mut editor = editor_with_text(&dir, "foo bar foo");
        editor.apply(Command::OpenSearch(SearchBar::Find));
        *editor.search_pattern_mut() = "foo".to_owned();
        editor.apply(Command::RecomputeMatches);
        assert_eq!(editor.search().matches(), [0..3, 8..11]);

        // Cut the first "foo " (chars 0..4); no `commit_edit` call.
        editor.set_selection(Some(0..4));
        editor.apply(Command::Cut);

        assert_eq!(editor.active_tab().unwrap().text, "bar foo");
        assert_eq!(editor.search().matches(), [Range { start: 4, end: 7 }]);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_edit_command_persists_the_swap() {
        let dir = temp_dir("edit-persists");
        // A new tab is recorded in the session index, so it is restorable.
        let mut editor = editor_in(&dir);
        editor.apply(Command::NewTab);
        let active = editor.active_index();
        editor.tabs[active].text = "keep this, drop this".to_owned();
        let id = editor.active_tab().unwrap().id;

        // Cut ", drop this" (chars 9..20).
        editor.set_selection(Some(9..20));
        editor.apply(Command::Cut);
        drop(editor);

        let restored = Store::at(dir.join("state")).unwrap().load().unwrap();
        let tab = restored.tabs.iter().find(|t| t.id == id).unwrap();
        assert_eq!(tab.text, "keep this");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn applying_a_suggestion_replaces_the_flagged_text() {
        let dir = temp_dir("suggest");
        let mut editor = editor_with_text(&dir, "café wrrod here");

        // "wrrod" sits after a multi-byte char: bytes 6..11.
        let outcome = editor.apply(suggestion(6..11, "wrrod", "word"));

        assert_eq!(outcome.buffer, BufferChange::Edited);
        assert_eq!(editor.active_tab().unwrap().text, "café word here");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_stale_suggestion_leaves_the_buffer_alone() {
        let dir = temp_dir("suggest-stale");
        // The buffer was edited after the menu opened: the range no longer
        // holds the flagged word.
        let mut editor = editor_with_text(&dir, "a word here");

        let outcome = editor.apply(suggestion(2..7, "wrrod", "word"));

        assert_eq!(outcome.buffer, BufferChange::Unchanged);
        assert_eq!(editor.active_tab().unwrap().text, "a word here");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_removal_suggestion_deletes_the_flagged_text() {
        let dir = temp_dir("suggest-remove");
        let mut editor = editor_with_text(&dir, "the the cat");

        let outcome = editor.apply(suggestion(0..4, "the ", ""));

        assert_eq!(outcome.buffer, BufferChange::Edited);
        assert_eq!(editor.active_tab().unwrap().text, "the cat");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn adding_to_the_dictionary_persists_across_launches() {
        let dir = temp_dir("dictionary");
        let mut editor = editor_in(&dir);

        let outcome = editor.apply(Command::AddToDictionary("frext".to_owned()));

        assert_eq!(
            outcome.buffer,
            BufferChange::Unchanged,
            "the buffer is untouched"
        );
        assert_eq!(
            editor.proofread().user_words().collect::<Vec<_>>(),
            vec!["frext"]
        );

        // A fresh editor over the same store loads the saved word.
        let reopened = editor_in(&dir);
        assert_eq!(
            reopened.proofread().user_words().collect::<Vec<_>>(),
            vec!["frext"]
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_unreadable_dictionary_is_never_overwritten() {
        let dir = temp_dir("dictionary-unreadable");
        let state = dir.join("state");
        fs::create_dir_all(&state).unwrap();
        // Invalid UTF-8: `load_dictionary` fails rather than returning words.
        let original: &[u8] = b"kept\n\xff\xfe broken\n";
        fs::write(state.join("dictionary.txt"), original).unwrap();

        let mut editor = editor_in(&dir);
        editor.apply(Command::AddToDictionary("frext".to_owned()));

        // The word works for this session...
        assert_eq!(
            editor.proofread().user_words().collect::<Vec<_>>(),
            vec!["frext"]
        );
        // ...but the unreadable file is left exactly as it was.
        assert_eq!(fs::read(state.join("dictionary.txt")).unwrap(), original);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn editing_ignores_another_tabs_selection() {
        let dir = temp_dir("edit-other-tab");
        let mut editor = editor_with_text(&dir, "hello world");
        let tab_a = editor.active_index();
        editor.set_selection(Some(0..6));

        editor.apply(Command::NewTab);
        let tab_b = editor.active_index();
        editor.tabs[tab_b].text = "other text".to_owned();
        let outcome = editor.apply(Command::Cut);

        // B's selection is the empty caret, so nothing is cut from either tab.
        assert_eq!(outcome.buffer, BufferChange::Unchanged);
        assert_eq!(editor.tabs[tab_b].text, "other text");
        assert_eq!(editor.tabs[tab_a].text, "hello world");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_selection_reported_for_the_active_tab_is_used() {
        let dir = temp_dir("edit-active-tab-selection");
        let mut editor = editor_with_text(&dir, "hello world");
        editor.set_selection(Some(0..6));

        editor.apply(Command::NewTab);
        let tab_b = editor.active_index();
        editor.tabs[tab_b].text = "other text".to_owned();
        editor.set_selection(Some(0..6));
        let outcome = editor.apply(Command::Cut);

        assert_eq!(outcome.buffer, BufferChange::Edited);
        assert_eq!(editor.tabs[tab_b].text, "text");

        fs::remove_dir_all(&dir).unwrap();
    }
}
