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
        }
    }

    /// Create a tab from an existing file's path and content.
    #[must_use]
    pub fn from_file(id: u64, path: std::path::PathBuf, text: String) -> Self {
        Self {
            id,
            path: Some(path),
            saved_text: text.clone(),
            text,
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
    use super::*;

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
}
