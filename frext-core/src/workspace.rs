//! The optional file-tree workspace: a root directory shown in the sidebar
//! plus the set of directories the user has expanded.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::editor::FolderState;

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

    /// The root rendered for the sidebar header: absolute where possible and
    /// with the user's home directory collapsed to `~`.
    ///
    /// A root passed as `.` on the command line is resolved against the
    /// current working directory so the header shows a real location rather
    /// than a bare dot.
    #[must_use]
    pub fn display_root(&self) -> String {
        display_path(&self.root)
    }
}

/// Render `path` for display: resolve it to an absolute path where possible,
/// then collapse a leading home-directory prefix to `~`.
///
/// `/home/fred` becomes `~`, `/home/fred/test` becomes `~/test`, and a path
/// outside home (or one that cannot be resolved) is shown as its lossy
/// absolute (or, failing that, original) form.
#[must_use]
pub fn display_path(path: &Path) -> String {
    let resolved = path.canonicalize().unwrap_or_else(|_| absolutize(path));

    collapse_home(&resolved, home_dir().as_deref())
}

/// Make `path` absolute by joining it onto the current working directory when
/// it is relative. Falls back to the path as given if the cwd is unavailable.
fn absolutize(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    match std::env::current_dir() {
        Ok(cwd) => cwd.join(path),
        Err(_) => path.to_path_buf(),
    }
}

/// The user's home directory, if the platform exposes one.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Collapse a leading `home` prefix of `path` to `~`. Pure (no I/O) so it is
/// straightforward to test with synthetic paths.
fn collapse_home(path: &Path, home: Option<&Path>) -> String {
    if let Some(home) = home {
        if path == home {
            return "~".to_owned();
        }
        if let Ok(rest) = path.strip_prefix(home) {
            return format!("~/{}", rest.display());
        }
    }
    path.display().to_string()
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

/// What a [`TreeRow`] shows: a folder (with its open/closed state) or a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    /// A directory, expanded or collapsed.
    Dir(FolderState),
    /// Anything that is not a directory (symlinks included).
    File,
}

/// One visible line of the file tree, as listed by [`visible_rows`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRow {
    /// The entry's full path.
    pub path: PathBuf,
    /// Nesting level below the root: the root's children are depth 0.
    pub depth: usize,
    /// Whether the entry is a folder (and its state) or a file.
    pub kind: RowKind,
}

/// The rows of the file tree as currently visible, top to bottom.
///
/// The root's children are listed at depth 0, folders first and then files,
/// each sorted as [`read_dir_split`] sorts them. A folder that
/// [`Workspace::is_expanded`] reports as expanded is immediately followed by
/// its own rows one level deeper; collapsed folders are not descended into,
/// so an expanded folder inside a collapsed one is not listed.
///
/// This re-reads the file system on every call, but only the root and the
/// expanded folders reachable from it are read. An unreadable or missing
/// root gives no rows.
#[must_use]
pub fn visible_rows(workspace: &Workspace) -> Vec<TreeRow> {
    let mut rows = Vec::new();
    push_rows(workspace, &workspace.root, 0, &mut rows);
    rows
}

/// Append the rows for the children of `dir`, at `depth`, to `rows`.
fn push_rows(workspace: &Workspace, dir: &Path, depth: usize, rows: &mut Vec<TreeRow>) {
    let (dirs, files) = read_dir_split(dir);

    for path in dirs {
        let expanded = workspace.is_expanded(&path);
        let state = if expanded {
            FolderState::Expanded
        } else {
            FolderState::Collapsed
        };
        rows.push(TreeRow {
            path: path.clone(),
            depth,
            kind: RowKind::Dir(state),
        });
        if expanded {
            push_rows(workspace, &path, depth + 1, rows);
        }
    }

    rows.extend(files.into_iter().map(|path| TreeRow {
        path,
        depth,
        kind: RowKind::File,
    }));
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
    fn collapse_home_replaces_home_prefix_with_tilde() {
        let home = Path::new("/home/fred");

        assert_eq!(collapse_home(Path::new("/home/fred"), Some(home)), "~");
        assert_eq!(
            collapse_home(Path::new("/home/fred/test"), Some(home)),
            "~/test"
        );
        assert_eq!(
            collapse_home(Path::new("/home/fred/a/b"), Some(home)),
            "~/a/b"
        );
    }

    #[test]
    fn collapse_home_leaves_paths_outside_home_untouched() {
        let home = Path::new("/home/fred");

        assert_eq!(
            collapse_home(Path::new("/etc/hosts"), Some(home)),
            "/etc/hosts"
        );
        // A path that merely shares a name prefix is not under home.
        assert_eq!(
            collapse_home(Path::new("/home/fredrick"), Some(home)),
            "/home/fredrick"
        );
    }

    #[test]
    fn collapse_home_without_a_home_is_a_passthrough() {
        assert_eq!(collapse_home(Path::new("/home/fred"), None), "/home/fred");
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

    /// A fresh, uniquely named temp directory path (not yet created).
    fn temp_root(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "frext-ws-rows-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    /// Build the fixture tree used by the `visible_rows` tests:
    ///
    /// ```text
    /// root/
    ///   alpha/
    ///     inner/
    ///       deep.txt
    ///     a1.txt
    ///   beta/
    ///     b1.txt
    ///   empty/
    ///   top.txt
    /// ```
    fn fixture(tag: &str) -> PathBuf {
        let root = temp_root(tag);
        std::fs::create_dir_all(root.join("alpha/inner")).unwrap();
        std::fs::create_dir_all(root.join("beta")).unwrap();
        std::fs::create_dir_all(root.join("empty")).unwrap();
        std::fs::write(root.join("alpha/inner/deep.txt"), "d").unwrap();
        std::fs::write(root.join("alpha/a1.txt"), "a").unwrap();
        std::fs::write(root.join("beta/b1.txt"), "b").unwrap();
        std::fs::write(root.join("top.txt"), "t").unwrap();
        root
    }

    fn dir_row(path: PathBuf, depth: usize, state: FolderState) -> TreeRow {
        TreeRow {
            path,
            depth,
            kind: RowKind::Dir(state),
        }
    }

    fn file_row(path: PathBuf, depth: usize) -> TreeRow {
        TreeRow {
            path,
            depth,
            kind: RowKind::File,
        }
    }

    #[test]
    fn visible_rows_nothing_expanded_lists_root_children() {
        let root = fixture("none");
        let ws = Workspace::new(root.clone());

        let rows = visible_rows(&ws);

        assert_eq!(
            rows,
            vec![
                dir_row(root.join("alpha"), 0, FolderState::Collapsed),
                dir_row(root.join("beta"), 0, FolderState::Collapsed),
                dir_row(root.join("empty"), 0, FolderState::Collapsed),
                file_row(root.join("top.txt"), 0),
            ]
        );

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn visible_rows_one_folder_expanded_nests_its_children() {
        let root = fixture("one");
        let mut ws = Workspace::new(root.clone());
        ws.set_expanded(&root.join("alpha"), true);

        let rows = visible_rows(&ws);

        assert_eq!(
            rows,
            vec![
                dir_row(root.join("alpha"), 0, FolderState::Expanded),
                dir_row(root.join("alpha/inner"), 1, FolderState::Collapsed),
                file_row(root.join("alpha/a1.txt"), 1),
                dir_row(root.join("beta"), 0, FolderState::Collapsed),
                dir_row(root.join("empty"), 0, FolderState::Collapsed),
                file_row(root.join("top.txt"), 0),
            ]
        );

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn visible_rows_two_levels_expanded_reach_depth_two() {
        let root = fixture("two");
        let mut ws = Workspace::new(root.clone());
        ws.set_expanded(&root.join("alpha"), true);
        ws.set_expanded(&root.join("alpha/inner"), true);

        let rows = visible_rows(&ws);

        assert_eq!(
            rows,
            vec![
                dir_row(root.join("alpha"), 0, FolderState::Expanded),
                dir_row(root.join("alpha/inner"), 1, FolderState::Expanded),
                file_row(root.join("alpha/inner/deep.txt"), 2),
                file_row(root.join("alpha/a1.txt"), 1),
                dir_row(root.join("beta"), 0, FolderState::Collapsed),
                dir_row(root.join("empty"), 0, FolderState::Collapsed),
                file_row(root.join("top.txt"), 0),
            ]
        );

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn visible_rows_hides_expanded_folder_inside_collapsed_one() {
        let root = fixture("hidden");
        let mut ws = Workspace::new(root.clone());
        ws.set_expanded(&root.join("alpha/inner"), true);

        let rows = visible_rows(&ws);

        assert_eq!(
            rows,
            vec![
                dir_row(root.join("alpha"), 0, FolderState::Collapsed),
                dir_row(root.join("beta"), 0, FolderState::Collapsed),
                dir_row(root.join("empty"), 0, FolderState::Collapsed),
                file_row(root.join("top.txt"), 0),
            ]
        );

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn visible_rows_missing_root_is_empty() {
        let root = temp_root("missing");
        let ws = Workspace::new(root);

        assert!(visible_rows(&ws).is_empty());
    }

    #[test]
    fn visible_rows_empty_expanded_folder_is_just_its_row() {
        let root = fixture("empty");
        let mut ws = Workspace::new(root.clone());
        ws.set_expanded(&root.join("empty"), true);

        let rows = visible_rows(&ws);

        assert_eq!(
            rows,
            vec![
                dir_row(root.join("alpha"), 0, FolderState::Collapsed),
                dir_row(root.join("beta"), 0, FolderState::Collapsed),
                dir_row(root.join("empty"), 0, FolderState::Expanded),
                file_row(root.join("top.txt"), 0),
            ]
        );

        std::fs::remove_dir_all(&root).unwrap();
    }
}
