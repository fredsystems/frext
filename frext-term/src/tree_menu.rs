//! The file tree's right-click menus.
//!
//! [`file_entries`] and [`dir_entries`] mirror the GUI's
//! `file_context_menu` and `dir_context_menu` in `frext`: the same items,
//! labels and order. Every item is enabled.

use std::path::Path;

use frext_core::editor::Command;
use frext_core::workspace::RowKind;

use crate::menu::{Enablement, MenuEntry};

/// The menu for a file row: Open, a separator, Copy path, Copy relative
/// path and Reveal in file manager, a separator, then Rename… and Delete
/// (move to trash). Every command carries `path`.
#[must_use]
pub fn file_entries(path: &Path) -> Vec<MenuEntry<Command>> {
    let path = path.to_path_buf();
    vec![
        item("Open", Command::OpenPath(path.clone())),
        MenuEntry::Separator,
        item("Copy path", Command::CopyPath(path.clone())),
        item(
            "Copy relative path",
            Command::CopyRelativePath(path.clone()),
        ),
        item("Reveal in file manager", Command::Reveal(path.clone())),
        MenuEntry::Separator,
        item("Rename\u{2026}", Command::BeginRename(path.clone())),
        item("Delete (move to trash)", Command::Trash(path)),
    ]
}

/// The menu for a folder row (and the root): New file… and New folder…,
/// a separator, Copy path, Copy relative path and Reveal in file manager,
/// a separator, then Rename… and Delete (move to trash). Every command
/// carries `path`.
#[must_use]
pub fn dir_entries(path: &Path) -> Vec<MenuEntry<Command>> {
    let path = path.to_path_buf();
    vec![
        item("New file\u{2026}", Command::BeginNewFile(path.clone())),
        item("New folder\u{2026}", Command::BeginNewFolder(path.clone())),
        MenuEntry::Separator,
        item("Copy path", Command::CopyPath(path.clone())),
        item(
            "Copy relative path",
            Command::CopyRelativePath(path.clone()),
        ),
        item("Reveal in file manager", Command::Reveal(path.clone())),
        MenuEntry::Separator,
        item("Rename\u{2026}", Command::BeginRename(path.clone())),
        item("Delete (move to trash)", Command::Trash(path)),
    ]
}

/// The menu for a tree row at `path`: [`dir_entries`] for a folder,
/// [`file_entries`] for anything else.
#[must_use]
pub fn entries(path: &Path, kind: RowKind) -> Vec<MenuEntry<Command>> {
    match kind {
        RowKind::Dir(_) => dir_entries(path),
        RowKind::File => file_entries(path),
    }
}

/// An enabled menu item labelled `label`.
fn item(label: &str, value: Command) -> MenuEntry<Command> {
    MenuEntry::Item {
        label: label.to_owned(),
        enabled: Enablement::Enabled,
        value,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use frext_core::editor::FolderState;

    use super::*;

    fn enabled(label: &str, value: Command) -> MenuEntry<Command> {
        MenuEntry::Item {
            label: label.to_owned(),
            enabled: Enablement::Enabled,
            value,
        }
    }

    fn expected_file(path: &Path) -> Vec<MenuEntry<Command>> {
        let p = || path.to_path_buf();
        vec![
            enabled("Open", Command::OpenPath(p())),
            MenuEntry::Separator,
            enabled("Copy path", Command::CopyPath(p())),
            enabled("Copy relative path", Command::CopyRelativePath(p())),
            enabled("Reveal in file manager", Command::Reveal(p())),
            MenuEntry::Separator,
            enabled("Rename…", Command::BeginRename(p())),
            enabled("Delete (move to trash)", Command::Trash(p())),
        ]
    }

    fn expected_dir(path: &Path) -> Vec<MenuEntry<Command>> {
        let p = || path.to_path_buf();
        vec![
            enabled("New file…", Command::BeginNewFile(p())),
            enabled("New folder…", Command::BeginNewFolder(p())),
            MenuEntry::Separator,
            enabled("Copy path", Command::CopyPath(p())),
            enabled("Copy relative path", Command::CopyRelativePath(p())),
            enabled("Reveal in file manager", Command::Reveal(p())),
            MenuEntry::Separator,
            enabled("Rename…", Command::BeginRename(p())),
            enabled("Delete (move to trash)", Command::Trash(p())),
        ]
    }

    #[test]
    fn file_menu_matches_the_gui() {
        let path = PathBuf::from("/work/src/main.rs");
        assert_eq!(file_entries(&path), expected_file(&path));
    }

    #[test]
    fn dir_menu_matches_the_gui() {
        let path = PathBuf::from("/work/src");
        assert_eq!(dir_entries(&path), expected_dir(&path));
    }

    #[test]
    fn entries_picks_the_dir_menu_for_folders_in_either_state() {
        let path = PathBuf::from("/work/src");
        assert_eq!(
            entries(&path, RowKind::Dir(FolderState::Expanded)),
            expected_dir(&path)
        );
        assert_eq!(
            entries(&path, RowKind::Dir(FolderState::Collapsed)),
            expected_dir(&path)
        );
    }

    #[test]
    fn entries_picks_the_file_menu_for_files() {
        let path = PathBuf::from("/work/README.md");
        assert_eq!(entries(&path, RowKind::File), expected_file(&path));
    }
}
