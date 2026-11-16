//! The front-end-independent editor controller.
//!
//! [`Editor`] owns every piece of editor state that is not view state: the
//! open tabs, the session store, the workspace, the pending prompt,
//! proofreading, and the clipboard. A front end drives it by sending
//! [`Command`]s through [`Editor::apply`] and applying the returned
//! [`Outcome`]'s [`Effect`]s to its own view state.

mod command;
mod effect;
mod prompt;
mod search;
mod tabs;

use std::ops::Range;
use std::path::{Path, PathBuf};

pub use command::{Command, SearchBar, SearchOption, StepDirection};
pub use effect::{BufferChange, EditorFocus, Effect, Outcome};
pub use prompt::{AfterSave, CloseChoice, Prompt};
pub use search::SearchState;

use crate::{
    clipboard::Clipboard,
    persistence::{RestoredSession, Store},
    proofread::state::Proofreading,
    tab::Tab,
    workspace::Workspace,
};

/// What happened when the personal dictionary was loaded at startup.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DictionaryFile {
    /// Loaded (or absent, which is an empty dictionary): safe to rewrite.
    Loaded,
    /// The file exists but could not be read (e.g. invalid UTF-8). Saving
    /// the in-memory word set would destroy its contents, so new words are
    /// kept for this session only.
    Unreadable,
}

/// The editor controller: tabs, session persistence, workspace, prompts,
/// proofreading and clipboard, independent of any UI toolkit.
pub struct Editor {
    store: Store,
    tabs: Vec<Tab>,
    active: usize,
    next_id: u64,
    /// The sidebar workspace (root directory + expanded folders), if open.
    workspace: Option<Workspace>,
    /// Find/replace state; its query is persisted in the session.
    search: SearchState,
    /// The prompt the editor is waiting on, if any.
    prompt: Option<Prompt>,
    /// Spelling/grammar checking: background worker, per-tab results, the
    /// personal dictionary, and ignored diagnostics.
    proofread: Proofreading,
    /// Whether the personal dictionary file may be rewritten.
    dictionary_file: DictionaryFile,
    /// The clipboard used by cut/copy/paste and the copy-path actions.
    clipboard: Box<dyn Clipboard>,
    /// The editor's current selection as a char range, as reported by the
    /// view.
    selection: Option<Range<usize>>,
}

/// Split borrow for the editor surface: the active tab's buffer (mutable,
/// so an immediate-mode text widget can bind to it) alongside the
/// proofreading results it renders.
pub struct ActiveView<'a> {
    /// The active tab.
    pub tab: &'a mut Tab,
    /// Proofreading results for rendering the tab's diagnostics.
    pub proofread: &'a Proofreading,
}

impl Editor {
    /// Build the editor, restoring the previous session from `store`,
    /// opening any command line `files`, and optionally setting the
    /// sidebar workspace to `dir`.
    ///
    /// A file already present in the restored session is focused rather
    /// than opened a second time; the last successfully opened file becomes
    /// the active tab. A `dir` replaces any restored workspace root (but
    /// keeps the previously-expanded folder set when the root is
    /// unchanged).
    #[must_use]
    pub fn open(
        store: Store,
        clipboard: Box<dyn Clipboard>,
        files: &[PathBuf],
        dir: Option<&Path>,
    ) -> Self {
        let restored = store.load().unwrap_or_else(|err| {
            log::error!("failed to load session, starting fresh: {err}");
            RestoredSession::default()
        });

        let mut tabs = restored.tabs;
        let active = restored.active;
        let mut next_id = restored.next_id;
        let mut workspace = restored.workspace;
        let restored_search = restored.search;
        let (user_words, dictionary_file) = match store.load_dictionary() {
            Ok(words) => (words, DictionaryFile::Loaded),
            Err(err) => {
                log::error!(
                    "failed to load the personal dictionary; words added this \
                     session will not be saved over it: {err}"
                );
                (Vec::new(), DictionaryFile::Unreadable)
            }
        };

        // Always guarantee at least one tab to edit.
        if tabs.is_empty() {
            tabs.push(Tab::new_untitled(next_id));
            next_id += 1;
        }

        // A directory argument sets / replaces the workspace root. If it
        // matches the restored root, keep the expanded set; otherwise start
        // fresh.
        if let Some(dir) = dir {
            let dir = dir.to_path_buf();
            match workspace.as_ref() {
                Some(ws) if ws.root == dir => {}
                _ => workspace = Some(Workspace::new(dir)),
            }
        }

        let mut editor = Self {
            store,
            tabs,
            active,
            next_id,
            workspace,
            search: SearchState::restored(restored_search),
            prompt: None,
            proofread: Proofreading::new(user_words),
            dictionary_file,
            clipboard,
            selection: None,
        };

        for file in files {
            editor.open_path(file);
        }
        if !files.is_empty() || dir.is_some() {
            editor.persist_session();
        }

        editor
    }

    /// Queue a proofread of the active buffer; a no-op unless its text (or
    /// path) changed since the last request.
    pub fn end_frame(&mut self) {
        if let Some(tab) = self.tabs.get(self.active) {
            self.proofread
                .request(tab.id, &tab.text, tab.path.as_deref());
        }
    }

    /// Final flush: write every tab's swap file and the session index.
    pub fn shutdown(&self) {
        for tab in &self.tabs {
            if let Err(err) = self.store.save_swap(tab) {
                log::error!("failed to flush swap for tab {} on exit: {err}", tab.id);
            }
        }
        self.persist_session();
    }

    /// Apply `command`, returning whether the buffer changed and the
    /// effects the front end should apply.
    ///
    /// A command that edits the buffer leaves the editor consistent itself
    /// (swap file persisted, search matches refreshed);
    /// [`BufferChange::Edited`] only informs the front end. [`Self::commit_edit`]
    /// is solely for edits the view made through [`Self::active_view_mut`].
    pub fn apply(&mut self, command: Command) -> Outcome {
        match command {
            Command::NewTab => self.new_tab(),
            Command::SelectTab(index) => self.select_tab(index),
            Command::RequestClose(index) => self.request_close_tab(index),
            Command::RequestCloseActive => self.request_close_tab(self.active),
            Command::OpenPath(path) => {
                if self.open_path(&path) {
                    self.persist_session();
                }
            }
            Command::Save => self.save_active(),
            Command::ResolveClose(choice) => self.resolve_close(choice),
            Command::SubmitSavePath(path) => self.submit_save_path(path),
            Command::CancelPrompt => self.prompt = None,
            Command::OpenSearch(bar) => return self.open_search(bar),
            Command::CloseSearch => {
                self.close_search();
                // Persist the (possibly edited) query when the bar closes.
                self.persist_session();
            }
            Command::StepMatch(direction) => return self.step_match(direction),
            Command::RecomputeMatches => self.recompute_matches(),
            Command::ToggleSearchOption(option) => self.toggle_search_option(option),
            Command::ToggleReplaceRow => self.toggle_replace_row(),
            Command::ReplaceCurrent => {
                return Outcome {
                    buffer: self.replace_current(),
                    effects: Vec::new(),
                };
            }
            Command::ReplaceAll => {
                return Outcome {
                    buffer: self.replace_all(),
                    effects: Vec::new(),
                };
            }
        }
        Outcome::default()
    }

    /// Record the editor's current selection as a char range (`None` when
    /// the view has no cursor).
    pub fn set_selection(&mut self, chars: Option<Range<usize>>) {
        self.selection = chars;
    }

    /// The active tab and the proofreading results, borrowed together for
    /// the editor surface. `None` only if there is no active tab.
    pub fn active_view_mut(&mut self) -> Option<ActiveView<'_>> {
        let tab = self.tabs.get_mut(self.active)?;
        Some(ActiveView {
            tab,
            proofread: &self.proofread,
        })
    }

    /// The view mutated the active buffer (through
    /// [`Self::active_view_mut`]): persist its swap file and, while the find
    /// bar is open, refresh the matches so they track the edit.
    pub fn commit_edit(&mut self) {
        self.persist_active_swap();
        if self.search.is_open() {
            self.recompute_matches();
        }
    }

    /// The open tabs, in tab-bar order.
    #[must_use]
    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    /// The index of the active tab.
    #[must_use]
    pub fn active_index(&self) -> usize {
        self.active
    }

    /// The active tab, if any.
    #[must_use]
    pub fn active_tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    /// The sidebar workspace, if one is open.
    #[must_use]
    pub fn workspace(&self) -> Option<&Workspace> {
        self.workspace.as_ref()
    }

    /// The prompt the editor is waiting on, if any.
    #[must_use]
    pub fn prompt(&self) -> Option<&Prompt> {
        self.prompt.as_ref()
    }

    /// Spelling/grammar checking state.
    #[must_use]
    pub fn proofread(&self) -> &Proofreading {
        &self.proofread
    }
}

// Transitional API: direct access for logic that still lives in the front
// end. These are removed as the workspace tree (01.10) and editing plus
// the frame hooks (01.11) move into the editor.
impl Editor {
    /// Mutable access to every open tab.
    ///
    /// Transitional: removed in 01.10.
    pub fn tabs_mut(&mut self) -> &mut [Tab] {
        &mut self.tabs
    }

    /// Mutable access to the workspace slot (to open, close or edit it).
    ///
    /// Transitional: removed in 01.10.
    pub fn workspace_slot_mut(&mut self) -> &mut Option<Workspace> {
        &mut self.workspace
    }

    /// The clipboard.
    ///
    /// Transitional: removed in 01.11.
    pub fn clipboard_mut(&mut self) -> &mut dyn Clipboard {
        self.clipboard.as_mut()
    }

    /// Mutable access to proofreading state.
    ///
    /// Transitional: removed in 01.11.
    pub fn proofread_mut(&mut self) -> &mut Proofreading {
        &mut self.proofread
    }

    /// Add `word` to the personal dictionary, saving it unless the
    /// dictionary file was unreadable at startup.
    ///
    /// Transitional: removed in 01.11 (becomes a command).
    pub fn add_to_dictionary(&mut self, word: &str) {
        if !self.proofread.add_word(word) {
            return;
        }
        match self.dictionary_file {
            DictionaryFile::Loaded => {
                if let Err(err) = self.store.save_dictionary(self.proofread.user_words()) {
                    log::error!("failed to save the personal dictionary: {err}");
                }
            }
            DictionaryFile::Unreadable => log::warn!(
                "added {word:?} for this session only: the dictionary file could \
                 not be read at startup, so it is not being overwritten"
            ),
        }
    }

    /// The selection last reported through [`Self::set_selection`].
    ///
    /// Transitional: removed in 01.11.
    #[must_use]
    pub fn selection(&self) -> Option<Range<usize>> {
        self.selection.clone()
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

    /// Create an isolated temp directory unique to this test.
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "frext-editor-test-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn open_in(dir: &Path, files: &[PathBuf], workspace: Option<&Path>) -> Editor {
        let store = Store::at(dir.join("state")).unwrap();
        Editor::open(
            store,
            Box::new(MemoryClipboard::default()),
            files,
            workspace,
        )
    }

    #[test]
    fn opening_cli_file_loads_its_contents_and_focuses_it() {
        let dir = temp_dir("cli-open");
        let file = dir.join("hello.txt");
        fs::write(&file, "hello from disk").unwrap();

        let editor = open_in(&dir, std::slice::from_ref(&file), None);

        let active = editor.active_tab().unwrap();
        assert_eq!(active.path.as_deref(), Some(file.as_path()));
        assert_eq!(active.text, "hello from disk");
        assert!(!active.is_dirty());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cli_files_persist_into_session_for_next_launch() {
        let dir = temp_dir("persist");
        let file = dir.join("restore-me.txt");
        fs::write(&file, "keep me").unwrap();

        {
            let _editor = open_in(&dir, std::slice::from_ref(&file), None);
        }

        // A fresh store at the same root must see the opened file restored.
        let store = Store::at(dir.join("state")).unwrap();
        let restored = store.load().unwrap();
        assert!(
            restored
                .tabs
                .iter()
                .any(|tab| tab.path.as_deref() == Some(file.as_path()))
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn directory_argument_opens_a_workspace_and_persists_it() {
        let dir = temp_dir("ws-arg");
        let project = dir.join("project");
        fs::create_dir_all(&project).unwrap();

        {
            let editor = open_in(&dir, &[], Some(project.as_path()));
            assert_eq!(
                editor.workspace().map(|ws| ws.root.clone()),
                Some(project.clone())
            );
        }

        // The workspace root must survive into the next launch.
        let store = Store::at(dir.join("state")).unwrap();
        let restored = store.load().unwrap();
        assert_eq!(restored.workspace.map(|ws| ws.root), Some(project.clone()));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn active_view_mut_returns_the_active_tab_and_commit_edit_writes_its_swap() {
        let dir = temp_dir("active-view");
        let mut editor = open_in(&dir, &[], None);
        editor.apply(Command::NewTab);
        let active_id = editor.active_tab().unwrap().id;

        let view = editor.active_view_mut().unwrap();
        assert_eq!(view.tab.id, active_id);
        view.tab.text.push_str("typed in the view");
        editor.commit_edit();
        drop(editor);

        // A fresh store restores the edit from the swap file.
        let store = Store::at(dir.join("state")).unwrap();
        let restored = store.load().unwrap();
        let tab = restored.tabs.iter().find(|t| t.id == active_id).unwrap();
        assert_eq!(tab.text, "typed in the view");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn end_frame_leaves_the_buffer_untouched() {
        let dir = temp_dir("end-frame");
        let file = dir.join("doc.txt");
        fs::write(&file, "some text").unwrap();
        let mut editor = open_in(&dir, std::slice::from_ref(&file), None);
        let before = editor.active_tab().unwrap().clone();

        editor.end_frame();
        editor.end_frame();

        let after = editor.active_tab().unwrap();
        assert_eq!(after.id, before.id);
        assert_eq!(after.text, before.text);
        assert_eq!(after.path, before.path);
        assert_eq!(editor.active_index(), editor.tabs().len() - 1);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn shutdown_flushes_every_tabs_swap() {
        let dir = temp_dir("shutdown");
        let mut editor = open_in(&dir, &[], None);
        editor.apply(Command::NewTab);

        // Edit both tabs without committing either.
        for (i, tab) in editor.tabs_mut().iter_mut().enumerate() {
            tab.text = format!("tab {i}");
        }
        let ids: Vec<u64> = editor.tabs().iter().map(|t| t.id).collect();
        editor.shutdown();
        drop(editor);

        let store = Store::at(dir.join("state")).unwrap();
        let restored = store.load().unwrap();
        for (i, id) in ids.iter().enumerate() {
            let tab = restored.tabs.iter().find(|t| t.id == *id).unwrap();
            assert_eq!(tab.text, format!("tab {i}"));
        }

        fs::remove_dir_all(&dir).unwrap();
    }
}
