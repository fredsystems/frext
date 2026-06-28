//! Session persistence: continuous swap-file autosave so unsaved buffers
//! survive closing and reopening frext (and crashes).
//!
//! Layout under the platform state directory (`$XDG_STATE_HOME/frext` on
//! Linux):
//!
//! ```text
//! frext/
//!   session.json     # index: tab order, ids, paths, active tab
//!   swap/
//!     <id>.swp       # full text of each tab's buffer
//! ```
//!
//! On launch the swap file is the source of truth for a tab's content, so
//! unsaved edits always win over what is currently on disk.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{error::PersistenceError, tab::Tab, workspace::Workspace};

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
}

/// Owns the on-disk paths used for persistence.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    /// Resolve the platform state directory and create the layout.
    ///
    /// # Errors
    ///
    /// Returns [`PersistenceError`] if the state directory cannot be
    /// resolved or created.
    pub fn new() -> Result<Self, PersistenceError> {
        let dirs = directories::ProjectDirs::from("io.github", "fredsystems", "frext")
            .ok_or(PersistenceError::NoStateDir)?;

        // Prefer a dedicated state dir; fall back to data dir on platforms
        // (macOS / Windows) where `directories` does not expose one.
        let root = dirs
            .state_dir()
            .map_or_else(|| dirs.data_dir().to_owned(), Path::to_owned);

        Self::at(root)
    }

    /// Create a store rooted at `root`, creating the `swap/` layout.
    ///
    /// # Errors
    ///
    /// Returns [`PersistenceError`] if the directory layout cannot be created.
    pub fn at(root: PathBuf) -> Result<Self, PersistenceError> {
        let swap = root.join("swap");
        fs::create_dir_all(&swap).map_err(|source| PersistenceError::Io { path: swap, source })?;

        Ok(Self { root })
    }

    fn session_path(&self) -> PathBuf {
        self.root.join("session.json")
    }

    fn swap_path(&self, id: u64) -> PathBuf {
        self.root.join("swap").join(format!("{id}.swp"))
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
            .save_session(std::slice::from_ref(&tab), 0, 8, None)
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
            .save_session(std::slice::from_ref(&tab), 0, 4, None)
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

        store.save_session(&[], 0, 0, Some(&ws)).unwrap();

        let restored = store.load().unwrap();
        let loaded = restored.workspace.expect("workspace persisted");
        assert_eq!(loaded.root, PathBuf::from("/projects/demo"));
        assert!(loaded.is_expanded(Path::new("/projects/demo/src")));

        fs::remove_dir_all(&dir).unwrap();
    }
}
