//! The per-tab buffer model.

use serde::{Deserialize, Serialize};

/// A single editor tab: one buffer, optionally backed by a file on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tab {
    /// Stable identifier, also used as the swap-file stem for this tab.
    pub id: u64,

    /// The file this buffer is associated with, if any. `None` means the
    /// buffer has never been saved (a scratch/untitled buffer).
    pub path: Option<std::path::PathBuf>,

    /// The current text content of the buffer.
    pub text: String,

    /// The text content as it was last read from / written to disk. Used to
    /// compute whether the buffer is dirty. For an untitled buffer this is
    /// the empty string.
    pub saved_text: String,

    /// The byte length of the backing file the last time frext read or wrote
    /// it. Used to detect external modifications (the file changing size on
    /// disk). Runtime-only: it is not persisted and is recomputed when a tab
    /// is restored from a swap file.
    #[serde(skip)]
    pub disk_len: Option<u64>,
}

impl Tab {
    /// Create a new, empty, untitled tab with the given id.
    #[must_use]
    pub fn new_untitled(id: u64) -> Self {
        Self {
            id,
            path: None,
            text: String::new(),
            saved_text: String::new(),
            disk_len: None,
        }
    }

    /// Create a tab from an existing file's path and content.
    #[must_use]
    pub fn from_file(id: u64, path: std::path::PathBuf, text: String) -> Self {
        let disk_len = u64::try_from(text.len()).ok();
        Self {
            id,
            path: Some(path),
            saved_text: text.clone(),
            text,
            disk_len,
        }
    }

    /// If the backing file's size on disk differs from what frext last saw
    /// AND the buffer has no unsaved local edits, reload the buffer from
    /// disk. Returns `true` if the buffer was reloaded.
    ///
    /// A dirty buffer is never clobbered: when the file changed on disk but
    /// the user has unsaved edits, the recorded disk length is still updated
    /// (so the change is only reported once) but the buffer text is left
    /// untouched.
    pub fn reload_if_changed_on_disk(&mut self) -> bool {
        let Some(path) = self.path.clone() else {
            return false;
        };

        let Ok(metadata) = std::fs::metadata(&path) else {
            return false;
        };
        let current_len = metadata.len();

        // No change since we last looked.
        if self.disk_len == Some(current_len) {
            return false;
        }

        if self.is_dirty() {
            // Don't lose the user's unsaved work; just acknowledge the new
            // size so we don't keep re-detecting the same change.
            self.disk_len = Some(current_len);
            return false;
        }

        match std::fs::read_to_string(&path) {
            Ok(text) => {
                self.disk_len = u64::try_from(text.len()).ok();
                self.saved_text = text.clone();
                self.text = text;
                true
            }
            Err(err) => {
                log::error!("failed to reload {}: {err}", path.display());
                self.disk_len = Some(current_len);
                false
            }
        }
    }

    /// Whether the buffer has unsaved changes relative to its saved content.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.text != self.saved_text
    }

    /// The short title to display on the tab (file name or "untitled"),
    /// with a leading marker when the buffer is dirty.
    #[must_use]
    pub fn title(&self) -> String {
        let base = self.path.as_ref().and_then(|p| p.file_name()).map_or_else(
            || "untitled".to_owned(),
            |name| name.to_string_lossy().into_owned(),
        );

        if self.is_dirty() {
            format!("\u{2022} {base}")
        } else {
            base
        }
    }
}

#[cfg(test)]
mod tests {
    // `unwrap` is acceptable in test code: a panic on an unexpected `Err`
    // is exactly the failure signal we want from a test.
    #![allow(clippy::unwrap_used)]

    use super::*;

    /// Create an isolated temp file path unique to this test.
    fn temp_file(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "frext-tab-test-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("file.txt")
    }

    #[test]
    fn untitled_tab_is_clean_and_named_untitled() {
        let tab = Tab::new_untitled(1);
        assert!(!tab.is_dirty());
        assert_eq!(tab.title(), "untitled");
    }

    #[test]
    fn editing_marks_tab_dirty_with_bullet_prefix() {
        let mut tab = Tab::new_untitled(1);
        tab.text.push_str("hi");
        assert!(tab.is_dirty());
        assert_eq!(tab.title(), "\u{2022} untitled");
    }

    #[test]
    fn file_tab_uses_file_name_and_is_clean_when_unchanged() {
        let tab = Tab::from_file(2, "/tmp/notes.txt".into(), "body".to_owned());
        assert!(!tab.is_dirty());
        assert_eq!(tab.title(), "notes.txt");
    }

    #[test]
    fn file_tab_becomes_dirty_after_edit() {
        let mut tab = Tab::from_file(2, "/tmp/notes.txt".into(), "body".to_owned());
        tab.text.push_str(" more");
        assert!(tab.is_dirty());
        assert_eq!(tab.title(), "\u{2022} notes.txt");
    }

    #[test]
    fn untitled_tab_never_reloads() {
        let mut tab = Tab::new_untitled(1);
        assert!(!tab.reload_if_changed_on_disk());
    }

    #[test]
    fn clean_tab_reloads_when_file_grows_on_disk() {
        let path = temp_file("grow");
        std::fs::write(&path, "one").unwrap();
        let mut tab = Tab::from_file(1, path.clone(), "one".to_owned());

        // External process appends to the file.
        std::fs::write(&path, "one and two").unwrap();

        assert!(tab.reload_if_changed_on_disk());
        assert_eq!(tab.text, "one and two");
        assert!(!tab.is_dirty());

        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn dirty_tab_is_not_clobbered_by_external_change() {
        let path = temp_file("dirty");
        std::fs::write(&path, "original").unwrap();
        let mut tab = Tab::from_file(1, path.clone(), "original".to_owned());

        // Local unsaved edit.
        tab.text = "my local edits".to_owned();
        assert!(tab.is_dirty());

        // File changes size on disk.
        std::fs::write(&path, "totally different content").unwrap();

        // The reload must be suppressed and the local edits preserved.
        assert!(!tab.reload_if_changed_on_disk());
        assert_eq!(tab.text, "my local edits");

        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn unchanged_file_does_not_reload() {
        let path = temp_file("same");
        std::fs::write(&path, "stable").unwrap();
        let mut tab = Tab::from_file(1, path.clone(), "stable".to_owned());

        assert!(!tab.reload_if_changed_on_disk());

        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
