//! Tab lifecycle: opening, switching, saving and closing tabs, and the
//! prompts that closing a dirty tab or saving an untitled one raise.

use std::path::{Path, PathBuf};

use super::{AfterSave, CloseChoice, Editor, Prompt};
use crate::tab::Tab;

/// The result of trying to save a tab without asking for a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SaveOutcome {
    /// The buffer was written and is now clean.
    Saved,
    /// The tab is untitled; a path must be chosen first.
    NeedsPath,
    /// Nothing was written: an out-of-range index or an I/O error.
    Failed,
}

impl Editor {
    /// Open `path` into a tab and focus it. If a tab with the same path is
    /// already open it is focused instead of opening a duplicate. Returns
    /// `true` when a tab is now focused on the path (whether reused or newly
    /// opened), `false` when the file could not be read.
    ///
    /// Does not itself persist the session index; callers batch that.
    pub(super) fn open_path(&mut self, path: &Path) -> bool {
        // Reuse an already-open tab. Compare canonicalized paths so that
        // e.g. `./foo.txt` and an absolute `foo.txt` are treated as one.
        let canonical = path.canonicalize().ok();
        if let Some(index) = self.tabs.iter().position(|tab| {
            tab.path.as_ref().is_some_and(|tab_path| {
                tab_path == path
                    || (canonical.is_some() && tab_path.canonicalize().ok() == canonical)
            })
        }) {
            self.active = index;
            return true;
        }

        match std::fs::read_to_string(path) {
            Ok(text) => {
                let id = self.alloc_id();
                let tab = Tab::from_file(id, path.to_path_buf(), text);
                if let Err(err) = self.store.save_swap(&tab) {
                    log::error!("failed to write swap for opened file: {err}");
                }
                self.tabs.push(tab);
                self.active = self.tabs.len() - 1;
                true
            }
            Err(err) => {
                log::error!("failed to open {}: {err}", path.display());
                false
            }
        }
    }

    /// Hand out a fresh, unique tab id.
    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// The index of the tab with id `tab_id`, if it is still open.
    fn index_of(&self, tab_id: u64) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.id == tab_id)
    }

    /// Open a new, empty, untitled tab and focus it.
    pub(super) fn new_tab(&mut self) {
        let id = self.alloc_id();
        self.tabs.push(Tab::new_untitled(id));
        self.active = self.tabs.len() - 1;
        self.persist_active_swap();
        self.persist_session();
    }

    /// Focus the tab at `index` (ignored when out of range), then re-find in
    /// whichever buffer is active.
    pub(super) fn select_tab(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active = index;
            self.persist_session();
        }
        self.recompute_matches();
    }

    /// Request to close the tab at `index`.
    ///
    /// A clean buffer is closed immediately. A buffer with unsaved changes
    /// instead raises [`Prompt::ConfirmClose`] (replacing any pending
    /// prompt); the actual close happens when the user chooses save or
    /// discard.
    pub(super) fn request_close_tab(&mut self, index: usize) {
        let Some(tab) = self.tabs.get(index) else {
            return;
        };
        if tab.is_dirty() {
            self.prompt = Some(Prompt::ConfirmClose { tab_id: tab.id });
        } else {
            self.close_tab(index);
        }
    }

    /// Save the active tab, raising [`Prompt::SavePath`] when it is
    /// untitled.
    pub(super) fn save_active(&mut self) {
        if self.save_tab(self.active) == SaveOutcome::NeedsPath
            && let Some(tab) = self.tabs.get(self.active)
        {
            self.prompt = Some(Prompt::SavePath {
                tab_id: tab.id,
                then: AfterSave::Nothing,
            });
        }
    }

    /// Save the tab at `index` to its own path. An untitled tab is not
    /// written and reports [`SaveOutcome::NeedsPath`].
    fn save_tab(&mut self, index: usize) -> SaveOutcome {
        let Some(tab) = self.tabs.get(index) else {
            return SaveOutcome::Failed;
        };
        match tab.path.clone() {
            Some(path) => {
                if self.write_tab(index, path) {
                    SaveOutcome::Saved
                } else {
                    SaveOutcome::Failed
                }
            }
            None => SaveOutcome::NeedsPath,
        }
    }

    /// Write the tab at `index` to `path`, making `path` its backing file.
    /// Returns `true` when the buffer was written (so it is now clean), and
    /// `false` on an out-of-range index or an I/O error.
    fn write_tab(&mut self, index: usize, path: PathBuf) -> bool {
        let Some(tab) = self.tabs.get(index) else {
            return false;
        };

        if let Err(err) = std::fs::write(&path, &tab.text) {
            log::error!("failed to save {}: {err}", path.display());
            return false;
        }

        if let Some(tab) = self.tabs.get_mut(index) {
            tab.path = Some(path);
            tab.saved_text = tab.text.clone();
        }
        self.persist_session();
        true
    }

    /// Answer a pending [`Prompt::ConfirmClose`]; a no-op for any other
    /// prompt. "Save" on an untitled tab chains into
    /// [`Prompt::SavePath`] with [`AfterSave::CloseTab`]; a failed save
    /// keeps the tab open.
    pub(super) fn resolve_close(&mut self, choice: CloseChoice) {
        let Some(Prompt::ConfirmClose { tab_id }) = self.prompt else {
            return;
        };
        self.prompt = None;
        // The tab may have been removed out from under a stale request.
        let Some(index) = self.index_of(tab_id) else {
            return;
        };

        match choice {
            CloseChoice::Cancel => {}
            CloseChoice::Discard => self.close_tab(index),
            CloseChoice::Save => match self.save_tab(index) {
                SaveOutcome::Saved => self.close_tab(index),
                SaveOutcome::Failed => {}
                SaveOutcome::NeedsPath => {
                    self.prompt = Some(Prompt::SavePath {
                        tab_id,
                        then: AfterSave::CloseTab,
                    });
                }
            },
        }
    }

    /// Answer a pending [`Prompt::SavePath`] by writing the tab to `path`
    /// (then closing it if the prompt asked for that); a no-op for any other
    /// prompt. The prompt is cleared whether or not the write succeeds.
    pub(super) fn submit_save_path(&mut self, path: PathBuf) {
        let Some(Prompt::SavePath { tab_id, then }) = self.prompt else {
            return;
        };
        self.prompt = None;
        let Some(index) = self.index_of(tab_id) else {
            return;
        };
        if self.write_tab(index, path) && then == AfterSave::CloseTab {
            self.close_tab(index);
        }
    }
}

// Transitional API: persistence that the front end's frame-top reload and
// editing code still call directly. Made private again as editing plus the
// frame hooks (01.11) move into the editor.
impl Editor {
    /// Persist the session index, logging (but not propagating) failures so
    /// a transient write error never crashes the editor.
    ///
    /// Transitional: removed from the public API in 01.11.
    pub fn persist_session(&self) {
        if let Err(err) = self.store.save_session(
            &self.tabs,
            self.active,
            self.next_id,
            self.workspace.as_ref(),
            self.search.query(),
        ) {
            log::error!("failed to save session index: {err}");
        }
    }

    /// Write the active tab's buffer to its swap file.
    ///
    /// Transitional: removed from the public API in 01.11.
    pub fn persist_active_swap(&self) {
        if let Some(tab) = self.tabs.get(self.active)
            && let Err(err) = self.store.save_swap(tab)
        {
            log::error!("failed to write swap file for tab {}: {err}", tab.id);
        }
    }
}

impl Editor {
    /// Close the tab at `index`, removing its swap file. Closing the last
    /// tab replaces it with a fresh untitled one.
    pub(super) fn close_tab(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }

        let tab = self.tabs.remove(index);
        self.proofread.forget_tab(tab.id);
        if let Err(err) = self.store.remove_swap(tab.id) {
            log::error!("failed to remove swap file for tab {}: {err}", tab.id);
        }

        if self.tabs.is_empty() {
            let id = self.alloc_id();
            self.tabs.push(Tab::new_untitled(id));
            self.active = 0;
            self.persist_active_swap();
        } else if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
        }

        self.persist_session();
    }
}

#[cfg(test)]
mod tests {
    // `unwrap`/`expect` are acceptable in test code: a panic on an unexpected
    // `Err`/`None` is exactly the failure signal we want from a test.
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::fs;

    use super::*;
    use crate::clipboard::MemoryClipboard;
    use crate::editor::Command;
    use crate::persistence::Store;

    /// Create an isolated temp directory unique to this test.
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "frext-tabs-test-{tag}-{}",
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

    /// An editor whose active tab is a new untitled tab holding unsaved
    /// text. Returns the editor and that tab's id.
    fn editor_with_dirty_untitled(dir: &Path) -> (Editor, u64) {
        let mut editor = editor_in(dir);
        editor.apply(Command::NewTab);
        let active = editor.active_index();
        editor.tabs_mut()[active].text.push_str("unsaved work");
        assert!(editor.tabs()[active].is_dirty());
        let id = editor.tabs()[active].id;
        (editor, id)
    }

    #[test]
    fn requesting_close_of_a_clean_tab_closes_immediately() {
        let dir = temp_dir("close-clean");
        let mut editor = editor_in(&dir);

        // Two clean tabs so closing one does not hit the "always keep one"
        // replacement path.
        editor.apply(Command::NewTab);
        let before = editor.tabs().len();
        assert!(!editor.tabs()[0].is_dirty());

        editor.apply(Command::RequestClose(0));

        assert!(editor.prompt().is_none(), "a clean close needs no prompt");
        assert_eq!(editor.tabs().len(), before - 1);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn requesting_close_of_a_dirty_tab_prompts_instead_of_closing() {
        let dir = temp_dir("close-dirty");
        let mut editor = editor_in(&dir);
        editor.apply(Command::NewTab);
        let before = editor.tabs().len();

        // Make tab 0 dirty.
        editor.tabs_mut()[0].text.push_str("unsaved work");
        assert!(editor.tabs()[0].is_dirty());
        let tab_id = editor.tabs()[0].id;

        editor.apply(Command::RequestClose(0));

        assert_eq!(
            editor.prompt(),
            Some(&Prompt::ConfirmClose { tab_id }),
            "a dirty close must open the confirmation prompt"
        );
        assert_eq!(
            editor.tabs().len(),
            before,
            "the tab must not be closed yet"
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn saving_a_dirty_tab_clears_its_dirty_state() {
        let dir = temp_dir("save-clears-dirty");
        let file = dir.join("doc.txt");
        fs::write(&file, "original").unwrap();

        let mut editor = editor_in(&dir);
        editor.apply(Command::OpenPath(file.clone()));
        let index = editor.active_index();
        assert_eq!(editor.tabs()[index].path.as_deref(), Some(file.as_path()));

        editor.tabs_mut()[index].text = "edited".to_owned();
        assert!(editor.tabs()[index].is_dirty());

        // save_tab is what "Save and close" calls; it must succeed and clean
        // the buffer, gating the subsequent close.
        assert_eq!(editor.save_tab(index), SaveOutcome::Saved);
        assert!(!editor.tabs()[index].is_dirty());
        assert_eq!(fs::read_to_string(&file).unwrap(), "edited");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn save_tab_on_out_of_range_index_is_a_noop_failure() {
        let dir = temp_dir("save-oob");
        let mut editor = editor_in(&dir);

        let index = editor.tabs().len() + 5;
        assert_eq!(editor.save_tab(index), SaveOutcome::Failed);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn opening_same_path_twice_reuses_the_tab() {
        let dir = temp_dir("reuse");
        let file = dir.join("notes.txt");
        fs::write(&file, "body").unwrap();

        let mut editor = editor_in(&dir);
        let baseline = editor.tabs().len();

        assert!(editor.open_path(&file));
        let after_first = editor.tabs().len();
        assert_eq!(after_first, baseline + 1);
        let first_index = editor.active_index();

        // Opening the same file again must not add a second tab.
        assert!(editor.open_path(&file));
        assert_eq!(editor.tabs().len(), after_first);
        assert_eq!(editor.active_index(), first_index);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn opening_nonexistent_file_reports_failure_and_adds_no_tab() {
        let dir = temp_dir("missing");
        let mut editor = editor_in(&dir);
        let before = editor.tabs().len();

        assert!(!editor.open_path(&dir.join("does-not-exist.txt")));
        assert_eq!(editor.tabs().len(), before);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn saving_an_untitled_tab_asks_for_a_path_and_writes_nothing() {
        let dir = temp_dir("save-untitled");
        let (mut editor, tab_id) = editor_with_dirty_untitled(&dir);
        let before: Vec<_> = fs::read_dir(&dir).unwrap().collect();

        editor.apply(Command::Save);

        assert_eq!(
            editor.prompt(),
            Some(&Prompt::SavePath {
                tab_id,
                then: AfterSave::Nothing,
            })
        );
        assert!(editor.active_tab().unwrap().path.is_none());
        assert!(editor.active_tab().unwrap().is_dirty());
        // Nothing new was written next to the state directory.
        assert_eq!(fs::read_dir(&dir).unwrap().count(), before.len());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn submitting_a_save_path_writes_the_file_and_clears_the_prompt() {
        let dir = temp_dir("submit-save-path");
        let (mut editor, tab_id) = editor_with_dirty_untitled(&dir);
        let target = dir.join("saved.txt");

        editor.apply(Command::Save);
        editor.apply(Command::SubmitSavePath(target.clone()));

        assert!(editor.prompt().is_none());
        assert_eq!(fs::read_to_string(&target).unwrap(), "unsaved work");
        let tab = editor.active_tab().unwrap();
        assert_eq!(tab.id, tab_id);
        assert_eq!(tab.path.as_deref(), Some(target.as_path()));
        assert!(!tab.is_dirty());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn save_and_close_on_an_untitled_tab_chains_into_a_save_path_prompt() {
        let dir = temp_dir("save-and-close-untitled");
        let (mut editor, tab_id) = editor_with_dirty_untitled(&dir);
        let target = dir.join("closed.txt");

        editor.apply(Command::RequestCloseActive);
        assert_eq!(editor.prompt(), Some(&Prompt::ConfirmClose { tab_id }));

        editor.apply(Command::ResolveClose(CloseChoice::Save));
        assert_eq!(
            editor.prompt(),
            Some(&Prompt::SavePath {
                tab_id,
                then: AfterSave::CloseTab,
            })
        );
        // Not closed until the save actually happens.
        assert!(editor.tabs().iter().any(|t| t.id == tab_id));

        editor.apply(Command::SubmitSavePath(target.clone()));
        assert!(editor.prompt().is_none());
        assert_eq!(fs::read_to_string(&target).unwrap(), "unsaved work");
        assert!(!editor.tabs().iter().any(|t| t.id == tab_id));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cancelling_the_save_path_of_a_save_and_close_keeps_the_tab() {
        let dir = temp_dir("save-and-close-cancel");
        let (mut editor, tab_id) = editor_with_dirty_untitled(&dir);

        editor.apply(Command::RequestCloseActive);
        editor.apply(Command::ResolveClose(CloseChoice::Save));
        editor.apply(Command::CancelPrompt);

        assert!(editor.prompt().is_none());
        let tab = editor.tabs().iter().find(|t| t.id == tab_id).unwrap();
        assert!(tab.is_dirty());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn discarding_a_dirty_tab_closes_it() {
        let dir = temp_dir("discard");
        let (mut editor, tab_id) = editor_with_dirty_untitled(&dir);

        editor.apply(Command::RequestCloseActive);
        editor.apply(Command::ResolveClose(CloseChoice::Discard));

        assert!(editor.prompt().is_none());
        assert!(!editor.tabs().iter().any(|t| t.id == tab_id));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cancelling_a_close_keeps_the_dirty_tab() {
        let dir = temp_dir("cancel-close");
        let (mut editor, tab_id) = editor_with_dirty_untitled(&dir);

        editor.apply(Command::RequestCloseActive);
        editor.apply(Command::ResolveClose(CloseChoice::Cancel));

        assert!(editor.prompt().is_none());
        let tab = editor.tabs().iter().find(|t| t.id == tab_id).unwrap();
        assert!(tab.is_dirty());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_failed_save_path_write_keeps_the_tab_open_and_dirty() {
        let dir = temp_dir("save-path-unwritable");
        let (mut editor, tab_id) = editor_with_dirty_untitled(&dir);
        let unwritable = dir.join("no-such-dir").join("file.txt");

        editor.apply(Command::RequestCloseActive);
        editor.apply(Command::ResolveClose(CloseChoice::Save));
        editor.apply(Command::SubmitSavePath(unwritable.clone()));

        assert!(editor.prompt().is_none());
        assert!(!unwritable.exists());
        let tab = editor.tabs().iter().find(|t| t.id == tab_id).unwrap();
        assert!(tab.is_dirty());
        assert!(tab.path.is_none());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn selecting_an_out_of_range_tab_is_ignored() {
        let dir = temp_dir("select-oob");
        let mut editor = editor_in(&dir);
        editor.apply(Command::NewTab);
        let active = editor.active_index();

        editor.apply(Command::SelectTab(editor.tabs().len()));
        assert_eq!(editor.active_index(), active);

        editor.apply(Command::SelectTab(0));
        assert_eq!(editor.active_index(), 0);

        fs::remove_dir_all(&dir).unwrap();
    }
}
