//! The optional file-tree workspace: a root directory shown in the sidebar
//! plus the set of directories the user has expanded.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

/// A directory opened in the sidebar, with the set of expanded sub-folders.
///
/// The expanded set is stored so the tree reopens exactly as the user left
/// it across sessions.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Workspace {
    /// The root directory shown at the top of the file tree.
    pub root: PathBuf,

    /// Absolute paths of directories that are currently expanded. A
    /// `BTreeSet` keeps the persisted order stable and lookups cheap.
    #[serde(default)]
    pub expanded: BTreeSet<PathBuf>,
}

impl Workspace {
    /// Create a workspace rooted at `root` with nothing expanded.
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            expanded: BTreeSet::new(),
        }
    }

    /// Whether `dir` is currently expanded.
    #[must_use]
    pub fn is_expanded(&self, dir: &Path) -> bool {
        self.expanded.contains(dir)
    }

    /// Record `dir` as expanded or collapsed. Returns `true` if the expanded
    /// set actually changed (so the caller can decide whether to persist).
    pub fn set_expanded(&mut self, dir: &Path, expanded: bool) -> bool {
        if expanded {
            self.expanded.insert(dir.to_path_buf())
        } else {
            self.expanded.remove(dir)
        }
    }
}

/// Read the immediate children of `dir`, split into sub-directories and
/// files, each sorted case-insensitively by file name. Entries that cannot
/// be read are skipped. Returns `(dirs, files)`.
///
/// This is a single shallow read so a large tree is only paid for as the
/// user expands it.
#[must_use]
pub fn read_dir_split(dir: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut dirs = Vec::new();
    let mut files = Vec::new();

    let Ok(entries) = std::fs::read_dir(dir) else {
        return (dirs, files);
    };

    for entry in entries.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(ft) if ft.is_dir() => dirs.push(path),
            Ok(_) => files.push(path),
            Err(_) => {}
        }
    }

    let key = |p: &PathBuf| {
        p.file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    };
    dirs.sort_by_key(key);
    files.sort_by_key(key);

    (dirs, files)
}

#[cfg(test)]
mod tests {
    // `unwrap` is acceptable in test code: a panic on an unexpected `Err`
    // is exactly the failure signal we want from a test.
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn set_expanded_reports_changes() {
        let mut ws = Workspace::new(PathBuf::from("/root"));
        let dir = Path::new("/root/sub");

        assert!(!ws.is_expanded(dir));
        assert!(ws.set_expanded(dir, true)); // changed
        assert!(ws.is_expanded(dir));
        assert!(!ws.set_expanded(dir, true)); // no-op
        assert!(ws.set_expanded(dir, false)); // changed
        assert!(!ws.is_expanded(dir));
        assert!(!ws.set_expanded(dir, false)); // no-op
    }

    #[test]
    fn read_dir_split_sorts_and_separates() {
        let dir = std::env::temp_dir().join(format!(
            "frext-ws-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("Zebra")).unwrap();
        std::fs::create_dir_all(dir.join("apple")).unwrap();
        std::fs::write(dir.join("Banana.txt"), "b").unwrap();
        std::fs::write(dir.join("avocado.txt"), "a").unwrap();

        let (dirs, files) = read_dir_split(&dir);

        let dir_names: Vec<_> = dirs
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        let file_names: Vec<_> = files
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();

        assert_eq!(dir_names, vec!["apple", "Zebra"]);
        assert_eq!(file_names, vec!["avocado.txt", "Banana.txt"]);

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
