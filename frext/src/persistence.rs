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

use crate::{error::PersistenceError, tab::Tab};

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
    /// Returns an empty vector (and `active = 0`, `next_id = 0`) when no
    /// session exists yet.
    ///
    /// # Errors
    ///
    /// Returns [`PersistenceError`] on I/O or deserialization failure.
    pub fn load(&self) -> Result<(Vec<Tab>, usize, u64), PersistenceError> {
        let session_path = self.session_path();
        let raw = match fs::read_to_string(&session_path) {
            Ok(raw) => raw,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok((Vec::new(), 0, 0));
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
            });
        }

        let active = session.active.min(tabs.len().saturating_sub(1));
        Ok((tabs, active, session.next_id))
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
    // `unwrap` is acceptable in test code: a panic on an unexpected `Err`
    // is exactly the failure signal we want from a test.
    #![allow(clippy::unwrap_used)]

    use super::*;

    /// A `Store` rooted at an arbitrary directory, for tests.
    fn store_at(root: &Path) -> Store {
        fs::create_dir_all(root.join("swap")).unwrap();
        Store {
            root: root.to_owned(),
        }
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
    fn load_missing_session_is_empty() {
        let dir = temp_dir("empty");
        let store = store_at(&dir);
        let (tabs, active, next_id) = store.load().unwrap();
        assert!(tabs.is_empty());
        assert_eq!(active, 0);
        assert_eq!(next_id, 0);
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
            .save_session(std::slice::from_ref(&tab), 0, 8)
            .unwrap();

        let (tabs, active, next_id) = store.load().unwrap();
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs[0].id, 7);
        assert_eq!(tabs[0].text, "unsaved work");
        assert!(tabs[0].is_dirty());
        assert_eq!(active, 0);
        assert_eq!(next_id, 8);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn remove_swap_then_load_skips_tab() {
        let dir = temp_dir("remove");
        let store = store_at(&dir);

        let tab = Tab::new_untitled(3);
        store.save_swap(&tab).unwrap();
        store
            .save_session(std::slice::from_ref(&tab), 0, 4)
            .unwrap();
        store.remove_swap(3).unwrap();

        let (tabs, _, _) = store.load().unwrap();
        assert!(tabs.is_empty());

        // Removing an already-missing swap file is a no-op.
        store.remove_swap(3).unwrap();

        fs::remove_dir_all(&dir).unwrap();
    }
}
