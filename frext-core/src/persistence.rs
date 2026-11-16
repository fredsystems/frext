//! Session persistence: continuous swap-file autosave so unsaved buffers
//! survive closing and reopening frext (and crashes).
//!
//! Layout under the platform state directory (`$XDG_STATE_HOME/frext` on
//! Linux). Each [`Profile`] has its own session and swap files; both share
//! one personal dictionary:
//!
//! ```text
//! frext/
//!   session.json     # Gui profile: tab order, ids, paths, active tab
//!   dictionary.txt   # shared personal proofreading dictionary, one word per line
//!   swap/
//!     <id>.swp       # Gui profile: full text of each tab's buffer
//!   term/
//!     session.json   # Term profile session index
//!     swap/
//!       <id>.swp     # Term profile swap files
//! ```
//!
//! On launch the swap file is the source of truth for a tab's content, so
//! unsaved edits always win over what is currently on disk.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{error::PersistenceError, search::SearchQuery, tab::Tab, workspace::Workspace};

/// One entry in the persisted session index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabRecord {
    /// Stable tab id; matches the `<id>.swp` swap file.
    pub id: u64,
    /// The file the tab is associated with, if any.
    pub path: Option<PathBuf>,
    /// The on-disk-saved content as frext last knew it. Persisted so dirty
    /// state can be recomputed without re-reading the file from disk.
    pub saved_text: String,
}

/// The persisted session index.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Session {
    /// Tabs in display order.
    pub tabs: Vec<TabRecord>,
    /// Index of the active tab within `tabs`.
    pub active: usize,
    /// The next tab id to hand out, so ids stay unique across sessions.
    pub next_id: u64,
    /// The sidebar workspace (root directory + expanded folders), if one is
    /// open. `#[serde(default)]` keeps sessions written before this field
    /// existed loadable.
    #[serde(default)]
    pub workspace: Option<Workspace>,
    /// The last search query and its toggles, restored so the search bar
    /// reopens with the user's previous pattern. `#[serde(default)]` keeps
    /// older sessions loadable.
    #[serde(default)]
    pub search: SearchQuery,
}

/// The fully-restored session handed back by [`Store::load`]: the live tabs
/// plus the surrounding editor state.
#[derive(Debug, Clone, Default)]
pub struct RestoredSession {
    /// Tabs in display order, with their buffer contents loaded from swap.
    pub tabs: Vec<Tab>,
    /// Index of the active tab.
    pub active: usize,
    /// The next unique tab id to hand out.
    pub next_id: u64,
    /// The sidebar workspace, if one was open.
    pub workspace: Option<Workspace>,
    /// The last search query and its toggles.
    pub search: SearchQuery,
}

/// Which front end a [`Store`] persists for. Each profile keeps its own
/// session and swap files; all profiles share one personal dictionary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    /// The egui GUI front end: state lives directly in the base directory.
    Gui,
    /// The terminal front end: state lives in `term/` under the base
    /// directory.
    Term,
}

/// Owns the on-disk paths used for persistence.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
    dictionary_dir: PathBuf,
}

impl Store {
    /// Resolve the platform state directory and create the layout for
    /// `profile`.
    ///
    /// # Errors
    ///
    /// Returns [`PersistenceError`] if the state directory cannot be
    /// resolved or created.
    pub fn new(profile: Profile) -> Result<Self, PersistenceError> {
        let dirs = directories::ProjectDirs::from("io.github", "fredsystems", "frext")
            .ok_or(PersistenceError::NoStateDir)?;

        // Prefer a dedicated state dir; fall back to data dir on platforms
        // (macOS / Windows) where `directories` does not expose one.
        let base = dirs
            .state_dir()
            .map_or_else(|| dirs.data_dir().to_owned(), Path::to_owned);

        Self::for_profile(base, profile)
    }

    /// Lay out `profile`'s store under `base`: the GUI uses `base` itself,
    /// the terminal front end uses `base/term`, and both keep the
    /// dictionary in `base`.
    fn for_profile(base: PathBuf, profile: Profile) -> Result<Self, PersistenceError> {
        let root = match profile {
            Profile::Gui => base.clone(),
            Profile::Term => base.join("term"),
        };
        Self::at_with_dictionary_dir(root, base)
    }

    /// Create a store rooted at `root`, creating the `swap/` layout. The
    /// dictionary also lives in `root`.
    ///
    /// # Errors
    ///
    /// Returns [`PersistenceError`] if the directory layout cannot be created.
    pub fn at(root: PathBuf) -> Result<Self, PersistenceError> {
        let dictionary_dir = root.clone();
        Self::at_with_dictionary_dir(root, dictionary_dir)
    }

    /// Create a store rooted at `root` whose dictionary lives in
    /// `dictionary_dir`, creating `root/swap` and `dictionary_dir`.
    ///
    /// # Errors
    ///
    /// Returns [`PersistenceError`] if the directory layout cannot be created.
    pub fn at_with_dictionary_dir(
        root: PathBuf,
        dictionary_dir: PathBuf,
    ) -> Result<Self, PersistenceError> {
        let swap = root.join("swap");
        fs::create_dir_all(&swap).map_err(|source| PersistenceError::Io { path: swap, source })?;
        fs::create_dir_all(&dictionary_dir).map_err(|source| PersistenceError::Io {
            path: dictionary_dir.clone(),
            source,
        })?;

        Ok(Self {
            root,
            dictionary_dir,
        })
    }

    /// The state root: the directory holding this store's session index and
    /// swap files.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn session_path(&self) -> PathBuf {
        self.root.join("session.json")
    }

    fn swap_path(&self, id: u64) -> PathBuf {
        self.root.join("swap").join(format!("{id}.swp"))
    }

    fn dictionary_path(&self) -> PathBuf {
        self.dictionary_dir.join("dictionary.txt")
    }

    /// Load the personal proofreading dictionary: one word per line, blank
    /// lines ignored. A missing file is an empty dictionary.
    ///
    /// # Errors
    ///
    /// Returns [`PersistenceError`] on I/O failure other than "not found".
    pub fn load_dictionary(&self) -> Result<Vec<String>, PersistenceError> {
        let path = self.dictionary_path();
        match fs::read_to_string(&path) {
            Ok(raw) => Ok(raw
                .lines()
                .map(str::trim)
                .filter(|w| !w.is_empty())
                .map(str::to_owned)
                .collect()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(source) => Err(PersistenceError::Io { path, source }),
        }
    }

    /// Merge `words` into the personal proofreading dictionary, one word per
    /// line, keeping every word already in the file.
    ///
    /// The whole read-merge-write runs under an exclusive lock on
    /// `dictionary.txt.lock`, so concurrent merges from other stores or
    /// processes never lose each other's words. If the existing file cannot
    /// be read for any reason other than "not found", nothing is written.
    ///
    /// The merged words are written to a temporary file that is then renamed
    /// over `dictionary.txt`, so a failed write (e.g. a full disk) leaves the
    /// previous dictionary intact instead of truncated.
    ///
    /// # Errors
    ///
    /// Returns [`PersistenceError`] on I/O failure.
    pub fn merge_dictionary<'a>(
        &self,
        words: impl IntoIterator<Item = &'a str>,
    ) -> Result<(), PersistenceError> {
        let lock_path = self.dictionary_dir.join("dictionary.txt.lock");
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .map_err(|source| PersistenceError::Io {
                path: lock_path.clone(),
                source,
            })?;
        lock.lock().map_err(|source| PersistenceError::Io {
            path: lock_path,
            source,
        })?;

        let mut merged: BTreeSet<String> = self.load_dictionary()?.into_iter().collect();
        merged.extend(
            words
                .into_iter()
                .map(str::trim)
                .filter(|w| !w.is_empty())
                .map(str::to_owned),
        );

        let mut raw = String::new();
        for word in &merged {
            raw.push_str(word);
            raw.push('\n');
        }
        let path = self.dictionary_path();
        let temp = self.dictionary_dir.join("dictionary.txt.tmp");
        if let Err(source) = fs::write(&temp, raw) {
            // Best effort: a partial temp file is harmless but untidy.
            let _ = fs::remove_file(&temp);
            return Err(PersistenceError::Io { path: temp, source });
        }
        fs::rename(&temp, &path).map_err(|source| PersistenceError::Io { path, source })
    }

    /// Load the previous session, restoring each tab's content from its
    /// swap file. Tabs whose swap file is missing are skipped.
    ///
    /// Returns a default (empty) [`RestoredSession`] when no session exists
    /// yet.
    ///
    /// # Errors
    ///
    /// Returns [`PersistenceError`] on I/O or deserialization failure.
    pub fn load(&self) -> Result<RestoredSession, PersistenceError> {
        let session_path = self.session_path();
        let raw = match fs::read_to_string(&session_path) {
            Ok(raw) => raw,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(RestoredSession::default());
            }
            Err(source) => {
                return Err(PersistenceError::Io {
                    path: session_path,
                    source,
                });
            }
        };

        let session: Session = serde_json::from_str(&raw)?;

        let mut tabs = Vec::with_capacity(session.tabs.len());
        for record in &session.tabs {
            let swap = self.swap_path(record.id);
            let text = match fs::read_to_string(&swap) {
                Ok(text) => text,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
                Err(source) => return Err(PersistenceError::Io { path: swap, source }),
            };

            tabs.push(Tab {
                id: record.id,
                path: record.path.clone(),
                text,
                saved_text: record.saved_text.clone(),
                // Recomputed lazily on the first external-change check.
                disk_len: None,
            });
        }

        let active = session.active.min(tabs.len().saturating_sub(1));
        Ok(RestoredSession {
            tabs,
            active,
            next_id: session.next_id,
            workspace: session.workspace,
            search: session.search,
        })
    }

    /// Persist the session index. Call whenever the tab set, ordering, or
    /// active tab changes.
    ///
    /// # Errors
    ///
    /// Returns [`PersistenceError`] on I/O or serialization failure.
    pub fn save_session(
        &self,
        tabs: &[Tab],
        active: usize,
        next_id: u64,
        workspace: Option<&Workspace>,
        search: &SearchQuery,
    ) -> Result<(), PersistenceError> {
        let session = Session {
            tabs: tabs
                .iter()
                .map(|tab| TabRecord {
                    id: tab.id,
                    path: tab.path.clone(),
                    saved_text: tab.saved_text.clone(),
                })
                .collect(),
            active,
            next_id,
            workspace: workspace.cloned(),
            search: search.clone(),
        };

        let raw = serde_json::to_string_pretty(&session)?;
        let path = self.session_path();
        fs::write(&path, raw).map_err(|source| PersistenceError::Io { path, source })
    }

    /// Write a single tab's buffer to its swap file. Call on every edit so
    /// the on-disk swap always reflects the live buffer.
    ///
    /// # Errors
    ///
    /// Returns [`PersistenceError`] on I/O failure.
    pub fn save_swap(&self, tab: &Tab) -> Result<(), PersistenceError> {
        let path = self.swap_path(tab.id);
        fs::write(&path, &tab.text).map_err(|source| PersistenceError::Io { path, source })
    }

    /// Remove a tab's swap file (e.g. when the tab is closed).
    ///
    /// A missing swap file is treated as success.
    ///
    /// # Errors
    ///
    /// Returns [`PersistenceError`] on I/O failure other than "not found".
    pub fn remove_swap(&self, id: u64) -> Result<(), PersistenceError> {
        let path = self.swap_path(id);
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(PersistenceError::Io { path, source }),
        }
    }
}

#[cfg(test)]
mod tests {
    // `unwrap`/`expect` are acceptable in test code: a panic on an
    // unexpected `Err`/`None` is exactly the failure signal we want.
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    /// A `Store` rooted at an arbitrary directory, for tests.
    fn store_at(root: &Path) -> Store {
        Store::at(root.to_owned()).unwrap()
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "frext-test-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn at_creates_swap_layout() {
        let dir = temp_dir("layout");
        let _store = Store::at(dir.clone()).unwrap();
        assert!(dir.join("swap").is_dir());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn term_profile_layout_is_under_term() {
        let base = temp_dir("profile-term");
        let store = Store::for_profile(base.clone(), Profile::Term).unwrap();
        assert_eq!(store.root(), base.join("term"));
        assert!(base.join("term").join("swap").is_dir());
        assert_eq!(store.dictionary_path(), base.join("dictionary.txt"));
        assert_eq!(store.session_path(), base.join("term").join("session.json"));
        assert_eq!(
            store.swap_path(5),
            base.join("term").join("swap").join("5.swp")
        );
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn gui_profile_layout_is_unchanged() {
        let base = temp_dir("profile-gui");
        let store = Store::for_profile(base.clone(), Profile::Gui).unwrap();
        assert_eq!(store.root(), base);
        assert!(base.join("swap").is_dir());
        assert!(!base.join("term").exists());
        assert_eq!(store.dictionary_path(), base.join("dictionary.txt"));
        assert_eq!(store.session_path(), base.join("session.json"));
        assert_eq!(store.swap_path(5), base.join("swap").join("5.swp"));
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn profiles_share_one_dictionary() {
        let base = temp_dir("profile-dict");
        let gui = Store::for_profile(base.clone(), Profile::Gui).unwrap();
        let term = Store::for_profile(base.clone(), Profile::Term).unwrap();

        gui.merge_dictionary(["frext"]).unwrap();
        assert_eq!(term.load_dictionary().unwrap(), vec!["frext"]);

        term.merge_dictionary(["ratatui"]).unwrap();
        assert_eq!(gui.load_dictionary().unwrap(), vec!["frext", "ratatui"]);

        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn root_reports_the_state_root() {
        let dir = temp_dir("root");
        let store = store_at(&dir);
        assert_eq!(store.root(), dir.as_path());

        let dict_dir = dir.join("dict");
        let split = Store::at_with_dictionary_dir(dir.join("state"), dict_dir.clone()).unwrap();
        assert_eq!(split.root(), dir.join("state"));
        assert!(dict_dir.is_dir());
        assert_eq!(split.dictionary_path(), dict_dir.join("dictionary.txt"));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn load_missing_session_is_empty() {
        let dir = temp_dir("empty");
        let store = store_at(&dir);
        let restored = store.load().unwrap();
        assert!(restored.tabs.is_empty());
        assert_eq!(restored.active, 0);
        assert_eq!(restored.next_id, 0);
        assert!(restored.workspace.is_none());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn roundtrip_preserves_unsaved_buffer() {
        let dir = temp_dir("roundtrip");
        let store = store_at(&dir);

        let mut tab = Tab::new_untitled(7);
        tab.text = "unsaved work".to_owned();

        store.save_swap(&tab).unwrap();
        store
            .save_session(
                std::slice::from_ref(&tab),
                0,
                8,
                None,
                &SearchQuery::default(),
            )
            .unwrap();

        let restored = store.load().unwrap();
        assert_eq!(restored.tabs.len(), 1);
        assert_eq!(restored.tabs[0].id, 7);
        assert_eq!(restored.tabs[0].text, "unsaved work");
        assert!(restored.tabs[0].is_dirty());
        assert_eq!(restored.active, 0);
        assert_eq!(restored.next_id, 8);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn remove_swap_then_load_skips_tab() {
        let dir = temp_dir("remove");
        let store = store_at(&dir);

        let tab = Tab::new_untitled(3);
        store.save_swap(&tab).unwrap();
        store
            .save_session(
                std::slice::from_ref(&tab),
                0,
                4,
                None,
                &SearchQuery::default(),
            )
            .unwrap();
        store.remove_swap(3).unwrap();

        let restored = store.load().unwrap();
        assert!(restored.tabs.is_empty());

        // Removing an already-missing swap file is a no-op.
        store.remove_swap(3).unwrap();

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn workspace_round_trips_through_session() {
        let dir = temp_dir("workspace");
        let store = store_at(&dir);

        let mut ws = Workspace::new(PathBuf::from("/projects/demo"));
        ws.set_expanded(Path::new("/projects/demo/src"), true);

        store
            .save_session(&[], 0, 0, Some(&ws), &SearchQuery::default())
            .unwrap();

        let restored = store.load().unwrap();
        let loaded = restored.workspace.expect("workspace persisted");
        assert_eq!(loaded.root, PathBuf::from("/projects/demo"));
        assert!(loaded.is_expanded(Path::new("/projects/demo/src")));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_dictionary_is_empty() {
        let dir = temp_dir("dict-missing");
        let store = store_at(&dir);
        assert!(store.load_dictionary().unwrap().is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn dictionary_round_trips_and_skips_blank_lines() {
        let dir = temp_dir("dict-roundtrip");
        let store = store_at(&dir);
        store.merge_dictionary(["frext", "syntect"]).unwrap();
        assert_eq!(store.load_dictionary().unwrap(), vec!["frext", "syntect"]);

        // A hand-edited file with stray blank lines and padding still loads.
        fs::write(dir.join("dictionary.txt"), "\n  nixos \n\nharper\n").unwrap();
        assert_eq!(store.load_dictionary().unwrap(), vec!["nixos", "harper"]);

        // Merging keeps the existing words, skips blank or duplicate input,
        // writes them sorted, and leaves no temporary behind.
        store
            .merge_dictionary(["added", " ", "nixos", "  padded "])
            .unwrap();
        assert_eq!(
            store.load_dictionary().unwrap(),
            vec!["added", "harper", "nixos", "padded"]
        );
        assert!(!dir.join("dictionary.txt.tmp").exists());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_failed_dictionary_save_keeps_the_old_file() {
        let dir = temp_dir("dict-failed-save");
        let store = store_at(&dir);
        store.merge_dictionary(["original"]).unwrap();

        // A directory squatting on the temp path makes the write fail.
        fs::create_dir(dir.join("dictionary.txt.tmp")).unwrap();
        assert!(store.merge_dictionary(["new"]).is_err());
        assert_eq!(store.load_dictionary().unwrap(), vec!["original"]);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn merge_keeps_words_written_by_another_store() {
        let base = temp_dir("dict-two-stores");
        let a = Store::at_with_dictionary_dir(base.join("a"), base.clone()).unwrap();
        let b = Store::at_with_dictionary_dir(base.join("b"), base.clone()).unwrap();
        assert!(a.load_dictionary().unwrap().is_empty());
        assert!(b.load_dictionary().unwrap().is_empty());

        a.merge_dictionary(["alpha"]).unwrap();
        b.merge_dictionary(["beta"]).unwrap();
        assert_eq!(a.load_dictionary().unwrap(), vec!["alpha", "beta"]);

        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn concurrent_merges_lose_no_words() {
        const THREADS: usize = 8;
        let base = temp_dir("dict-concurrent");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(THREADS));

        let handles: Vec<_> = (0..THREADS)
            .map(|i| {
                let store = Store::at_with_dictionary_dir(base.join(format!("s{i}")), base.clone())
                    .unwrap();
                let barrier = std::sync::Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let word = format!("word{i}");
                    barrier.wait();
                    store.merge_dictionary([word.as_str()]).unwrap();
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }

        let expected: Vec<String> = (0..THREADS).map(|i| format!("word{i}")).collect();
        assert_eq!(store_at(&base).load_dictionary().unwrap(), expected);

        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn merge_does_not_write_when_the_file_is_unreadable() {
        let dir = temp_dir("dict-unreadable");
        let store = store_at(&dir);
        // A directory squatting on the dictionary path makes the re-read fail.
        fs::create_dir(dir.join("dictionary.txt")).unwrap();

        assert!(store.merge_dictionary(["frext"]).is_err());
        assert!(dir.join("dictionary.txt").is_dir());
        assert!(!dir.join("dictionary.txt.tmp").exists());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn search_query_round_trips_through_session() {
        let dir = temp_dir("search");
        let store = store_at(&dir);

        let query = SearchQuery {
            pattern: r"\bfoo\b".to_owned(),
            case_sensitive: true,
            regex: true,
            whole_word: false,
        };

        store.save_session(&[], 0, 0, None, &query).unwrap();

        let restored = store.load().unwrap();
        assert_eq!(restored.search, query);

        fs::remove_dir_all(&dir).unwrap();
    }
}
