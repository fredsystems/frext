//! The sidebar workspace and its filesystem operations: expanding folders,
//! copying paths, and the rename / new file / new folder / trash actions,
//! reconciling open tabs and the expanded-folder set with the result.

use std::path::{Path, PathBuf};

use super::{Editor, Effect, FolderState, NameKind, NamePrompt, Outcome, Prompt};
use crate::{error::FsError, fs_ops};

impl Editor {
    /// Render `path` relative to the workspace root for "Copy relative path".
    /// Falls back to the full path display when there is no workspace or the
    /// path is not under the root.
    #[must_use]
    pub fn relative_to_root(&self, path: &Path) -> String {
        if let Some(ws) = self.workspace.as_ref()
            && let Ok(rel) = path.strip_prefix(&ws.root)
        {
            return rel.display().to_string();
        }
        path.display().to_string()
    }

    /// The live text of the name field, while the editor is waiting on a
    /// [`Prompt::Name`]; `None` for any other (or no) prompt.
    pub fn prompt_name_mut(&mut self) -> Option<&mut String> {
        match self.prompt.as_mut() {
            Some(Prompt::Name(prompt)) => Some(&mut prompt.name),
            _ => None,
        }
    }

    /// Record `dir` as expanded or collapsed, persisting the session when
    /// the workspace's expanded set actually changed.
    pub(super) fn set_expanded(&mut self, dir: &Path, state: FolderState) {
        if let Some(ws) = self.workspace.as_mut()
            && ws.set_expanded(dir, state == FolderState::Expanded)
        {
            self.persist_session();
        }
    }

    /// Close the sidebar workspace, removing it from the persisted session so
    /// it stays closed on the next launch. A no-op when no workspace is open.
    pub(super) fn close_workspace(&mut self) {
        if self.workspace.take().is_some() {
            self.persist_session();
        }
    }

    /// Copy `path`'s absolute path to the clipboard.
    pub(super) fn copy_path(&mut self, path: &Path) {
        self.clipboard.write_text(path.display().to_string());
    }

    /// Copy `path`, relative to the workspace root, to the clipboard.
    pub(super) fn copy_relative_path(&mut self, path: &Path) {
        let relative = self.relative_to_root(path);
        self.clipboard.write_text(relative);
    }

    /// Begin renaming `path`: a [`Prompt::Name`] seeded with its file name.
    pub(super) fn begin_rename(&mut self, path: PathBuf) -> Outcome {
        let name = path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        self.raise_name_prompt(NameKind::Rename, path, name)
    }

    /// Begin creating a new file or folder inside `dir`: an empty
    /// [`Prompt::Name`].
    pub(super) fn begin_name(&mut self, kind: NameKind, dir: PathBuf) -> Outcome {
        self.raise_name_prompt(kind, dir, String::new())
    }

    /// Set a [`Prompt::Name`] (replacing any pending prompt) and ask the
    /// front end to focus its field.
    fn raise_name_prompt(&mut self, kind: NameKind, target: PathBuf, name: String) -> Outcome {
        self.prompt = Some(Prompt::Name(NamePrompt {
            kind,
            target,
            name,
            error: None,
        }));
        Outcome {
            effects: vec![Effect::FocusPromptField],
            ..Outcome::default()
        }
    }

    /// Answer a pending [`Prompt::Name`] with the entered name; a no-op for
    /// any other prompt. On success the prompt is cleared (and a new file is
    /// opened in a tab); on failure the prompt stays open carrying the error
    /// and its field is refocused.
    pub(super) fn submit_name(&mut self) -> Outcome {
        if !matches!(self.prompt, Some(Prompt::Name(_))) {
            return Outcome::default();
        }
        let Some(Prompt::Name(prompt)) = self.prompt.take() else {
            return Outcome::default();
        };

        let result = match prompt.kind {
            NameKind::Rename => self
                .rename_path(&prompt.target, &prompt.name)
                .map(|()| None),
            NameKind::NewFile => fs_ops::create_file(&prompt.target, &prompt.name).map(Some),
            NameKind::NewFolder => fs_ops::create_dir(&prompt.target, &prompt.name).map(|_| None),
        };

        match result {
            Ok(created_file) => {
                if let Some(path) = created_file
                    && self.open_path(&path)
                {
                    self.persist_session();
                }
                Outcome::default()
            }
            Err(err) => {
                // Re-open the prompt carrying the error message.
                self.prompt = Some(Prompt::Name(NamePrompt {
                    error: Some(err.to_string()),
                    ..prompt
                }));
                Outcome {
                    effects: vec![Effect::FocusPromptField],
                    ..Outcome::default()
                }
            }
        }
    }

    /// Move `path` to the OS trash, then reconcile editor state: any tab whose
    /// file was trashed (the file itself, or a file under a trashed directory)
    /// is closed, since its backing file no longer exists.
    pub(super) fn trash_path(&mut self, path: &Path) {
        if let Err(err) = fs_ops::move_to_trash(path) {
            log::error!("{err}");
            return;
        }

        self.close_tabs_under(path);

        // Drop any now-stale expanded-folder entries under the trashed path.
        self.prune_workspace_expanded();
    }

    /// Close any tab whose backing file is `path` or lives under it (when
    /// `path` is a directory). Iterates from the end so removals do not shift
    /// unscanned indices. Split out from [`Self::trash_path`] so the
    /// tab-reconciliation can be tested without touching the OS trash.
    fn close_tabs_under(&mut self, path: &Path) {
        for index in (0..self.tabs.len()).rev() {
            let affected = self.tabs[index]
                .path
                .as_ref()
                .is_some_and(|p| p == path || p.starts_with(path));
            if affected {
                self.close_tab(index);
            }
        }
    }

    /// Rename `from` to `new_name`, then reconcile editor and workspace state:
    /// rewrite the path of any open tab whose file lived at (or under) `from`,
    /// and migrate matching entries in the workspace's expanded-folder set.
    ///
    /// Returns the rename result so the caller can keep the prompt open and
    /// show the error on failure.
    fn rename_path(&mut self, from: &Path, new_name: &str) -> Result<(), FsError> {
        let to = fs_ops::rename(from, new_name)?;
        if to == from {
            return Ok(());
        }

        // Repoint any open tab whose path was `from` or lived beneath it.
        let mut touched_tabs = false;
        for tab in &mut self.tabs {
            if let Some(tab_path) = tab.path.as_ref() {
                if tab_path == from {
                    tab.path = Some(to.clone());
                    touched_tabs = true;
                } else if let Ok(rest) = tab_path.strip_prefix(from) {
                    tab.path = Some(to.join(rest));
                    touched_tabs = true;
                }
            }
        }

        // Migrate expanded-folder entries under the old path to the new one.
        if let Some(ws) = self.workspace.as_mut() {
            let migrated: Vec<(PathBuf, PathBuf)> = ws
                .expanded
                .iter()
                .filter_map(|dir| {
                    if dir == from {
                        Some((dir.clone(), to.clone()))
                    } else {
                        dir.strip_prefix(from)
                            .ok()
                            .map(|rest| (dir.clone(), to.join(rest)))
                    }
                })
                .collect();
            for (old, new) in migrated {
                ws.expanded.remove(&old);
                ws.expanded.insert(new);
            }
        }

        if touched_tabs {
            self.persist_active_swap();
        }
        self.persist_session();
        Ok(())
    }

    /// Drop expanded-folder entries that no longer point at a directory on
    /// disk (e.g. after the folder was renamed away or trashed).
    fn prune_workspace_expanded(&mut self) {
        if let Some(ws) = self.workspace.as_mut() {
            let stale: Vec<PathBuf> = ws
                .expanded
                .iter()
                .filter(|dir| !dir.is_dir())
                .cloned()
                .collect();
            if !stale.is_empty() {
                for dir in stale {
                    ws.expanded.remove(&dir);
                }
                self.persist_session();
            }
        }
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
            "frext-tree-test-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// An editor backed by a store rooted in `dir/state`, opening `files`
    /// and optionally the workspace `workspace`.
    fn editor_in(dir: &Path, files: &[PathBuf], workspace: Option<&Path>) -> Editor {
        let store = Store::at(dir.join("state")).unwrap();
        Editor::open(
            store,
            Box::new(MemoryClipboard::default()),
            files,
            workspace,
        )
    }

    /// The pending name prompt, panicking if there is none.
    fn name_prompt(editor: &Editor) -> &NamePrompt {
        match editor.prompt() {
            Some(Prompt::Name(prompt)) => prompt,
            other => panic!("expected a name prompt, got {other:?}"),
        }
    }

    #[test]
    fn closing_the_workspace_drops_it_from_the_session() {
        let dir = temp_dir("ws-close");
        let project = dir.join("project");
        fs::create_dir_all(&project).unwrap();

        {
            let mut editor = editor_in(&dir, &[], Some(project.as_path()));
            assert!(editor.workspace().is_some());

            editor.apply(Command::CloseWorkspace);
            assert!(editor.workspace().is_none());

            // Closing again is a harmless no-op.
            editor.apply(Command::CloseWorkspace);
            assert!(editor.workspace().is_none());
        }

        // The closed workspace must not come back on the next launch.
        let store = Store::at(dir.join("state")).unwrap();
        let restored = store.load().unwrap();
        assert!(restored.workspace.is_none());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn renaming_a_file_repoints_its_open_tab() {
        let dir = temp_dir("rename-tab");
        let file = dir.join("old.txt");
        fs::write(&file, "body").unwrap();

        let mut editor = editor_in(&dir, std::slice::from_ref(&file), None);
        assert_eq!(
            editor.active_tab().unwrap().path.as_deref(),
            Some(file.as_path())
        );

        editor.rename_path(&file, "new.txt").unwrap();

        let renamed = dir.join("new.txt");
        assert!(renamed.is_file());
        assert!(!file.exists());
        // The open tab now points at the new path.
        assert_eq!(
            editor.active_tab().unwrap().path.as_deref(),
            Some(renamed.as_path())
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn renaming_a_directory_repoints_tabs_and_expanded_set() {
        let dir = temp_dir("rename-dir");
        let sub = dir.join("src");
        fs::create_dir(&sub).unwrap();
        let file = sub.join("main.rs");
        fs::write(&file, "fn main() {}").unwrap();

        let mut editor = editor_in(&dir, std::slice::from_ref(&file), Some(dir.as_path()));
        // Mark the sub-directory expanded so the rename must migrate it.
        editor.apply(Command::SetExpanded {
            dir: sub.clone(),
            state: FolderState::Expanded,
        });

        editor.rename_path(&sub, "lib").unwrap();

        let new_sub = dir.join("lib");
        let new_file = new_sub.join("main.rs");
        assert!(new_file.is_file());
        // The tab under the renamed directory was repointed.
        assert_eq!(
            editor.active_tab().unwrap().path.as_deref(),
            Some(new_file.as_path())
        );
        // The expanded-folder entry migrated to the new path.
        let ws = editor.workspace().unwrap();
        assert!(ws.is_expanded(&new_sub));
        assert!(!ws.is_expanded(&sub));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rename_refusing_to_clobber_surfaces_an_error() {
        let dir = temp_dir("rename-clobber");
        let a = dir.join("a.txt");
        let b = dir.join("b.txt");
        fs::write(&a, "a").unwrap();
        fs::write(&b, "b").unwrap();

        let mut editor = editor_in(&dir, &[], None);
        let err = editor.rename_path(&a, "b.txt").unwrap_err();
        assert!(matches!(err, FsError::AlreadyExists(_)));
        // Both files remain.
        assert!(a.exists() && b.exists());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn close_tabs_under_closes_open_files_in_a_directory() {
        let dir = temp_dir("close-under");
        let sub = dir.join("pkg");
        fs::create_dir(&sub).unwrap();
        let inside = sub.join("a.txt");
        let outside = dir.join("b.txt");
        fs::write(&inside, "a").unwrap();
        fs::write(&outside, "b").unwrap();

        let mut editor = editor_in(&dir, &[inside.clone(), outside.clone()], None);
        // Both files are open (alongside the initial untitled tab).
        let opened: Vec<_> = editor
            .tabs()
            .iter()
            .filter_map(|t| t.path.clone())
            .collect();
        assert!(opened.contains(&inside) && opened.contains(&outside));

        // Simulate the bookkeeping half of trashing the directory.
        editor.close_tabs_under(&sub);

        // The tab inside the directory is gone; the outside one survives.
        let remaining: Vec<_> = editor
            .tabs()
            .iter()
            .filter_map(|t| t.path.clone())
            .collect();
        assert!(remaining.contains(&outside));
        assert!(!remaining.contains(&inside));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn relative_to_root_strips_the_workspace_root() {
        let dir = temp_dir("relpath");
        let nested = dir.join("a").join("b.txt");

        let editor = editor_in(&dir, &[], Some(dir.as_path()));

        assert_eq!(
            editor.relative_to_root(&nested),
            PathBuf::from("a/b.txt").display().to_string()
        );
        // A path outside the root falls back to its full display.
        let outside = PathBuf::from("/etc/hosts");
        assert_eq!(
            editor.relative_to_root(&outside),
            outside.display().to_string()
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn copy_path_writes_the_absolute_path_to_the_clipboard() {
        let dir = temp_dir("copy-path");
        let nested = dir.join("a").join("b.txt");

        let mut editor = editor_in(&dir, &[], Some(dir.as_path()));

        editor.apply(Command::CopyPath(nested.clone()));

        assert_eq!(
            editor.clipboard.read_text(),
            Some(nested.display().to_string())
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn copy_relative_path_writes_the_root_relative_path() {
        let dir = temp_dir("copy-relpath");
        let nested = dir.join("a").join("b.txt");

        let mut editor = editor_in(&dir, &[], Some(dir.as_path()));

        editor.apply(Command::CopyRelativePath(nested));

        assert_eq!(
            editor.clipboard.read_text(),
            Some(PathBuf::from("a/b.txt").display().to_string())
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn begin_rename_opens_a_name_prompt_with_the_file_name_and_focuses_it() {
        let dir = temp_dir("begin-rename");
        let file = dir.join("notes.txt");
        let mut editor = editor_in(&dir, &[], None);

        let outcome = editor.apply(Command::BeginRename(file.clone()));

        assert_eq!(outcome.effects, vec![Effect::FocusPromptField]);
        assert_eq!(
            name_prompt(&editor),
            &NamePrompt {
                kind: NameKind::Rename,
                target: file,
                name: "notes.txt".to_owned(),
                error: None,
            }
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn begin_new_folder_opens_an_empty_name_prompt() {
        let dir = temp_dir("begin-new-folder");
        let mut editor = editor_in(&dir, &[], None);

        let outcome = editor.apply(Command::BeginNewFolder(dir.clone()));

        assert_eq!(outcome.effects, vec![Effect::FocusPromptField]);
        assert_eq!(
            name_prompt(&editor),
            &NamePrompt {
                kind: NameKind::NewFolder,
                target: dir.clone(),
                name: String::new(),
                error: None,
            }
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn submitting_a_new_file_name_creates_and_opens_it() {
        let dir = temp_dir("submit-new-file");
        let mut editor = editor_in(&dir, &[], None);

        editor.apply(Command::BeginNewFile(dir.clone()));
        *editor.prompt_name_mut().unwrap() = "fresh.txt".to_owned();
        let outcome = editor.apply(Command::SubmitName);

        let created = dir.join("fresh.txt");
        assert!(created.is_file());
        assert!(editor.prompt().is_none());
        assert!(outcome.effects.is_empty());
        assert_eq!(
            editor.active_tab().unwrap().path.as_deref(),
            Some(created.as_path())
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn submitting_a_new_folder_name_creates_the_folder() {
        let dir = temp_dir("submit-new-folder");
        let mut editor = editor_in(&dir, &[], None);

        editor.apply(Command::BeginNewFolder(dir.clone()));
        *editor.prompt_name_mut().unwrap() = "pkg".to_owned();
        editor.apply(Command::SubmitName);

        assert!(dir.join("pkg").is_dir());
        assert!(editor.prompt().is_none());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_failed_submit_keeps_the_prompt_with_an_error_and_refocuses() {
        let dir = temp_dir("submit-fails");
        fs::write(dir.join("taken.txt"), "already here").unwrap();
        let mut editor = editor_in(&dir, &[], None);
        let tabs_before = editor.tabs().len();

        editor.apply(Command::BeginNewFile(dir.clone()));
        *editor.prompt_name_mut().unwrap() = "taken.txt".to_owned();
        let outcome = editor.apply(Command::SubmitName);

        assert_eq!(outcome.effects, vec![Effect::FocusPromptField]);
        let prompt = name_prompt(&editor);
        assert_eq!(prompt.kind, NameKind::NewFile);
        assert_eq!(prompt.target, dir);
        assert_eq!(prompt.name, "taken.txt");
        assert!(prompt.error.is_some());
        // The existing file is untouched and nothing was opened.
        assert_eq!(
            fs::read_to_string(dir.join("taken.txt")).unwrap(),
            "already here"
        );
        assert_eq!(editor.tabs().len(), tabs_before);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn submit_name_without_a_name_prompt_is_a_noop() {
        let dir = temp_dir("submit-noop");
        let mut editor = editor_in(&dir, &[], None);
        editor.apply(Command::NewTab);
        let index = editor.active_index();
        editor.tabs_mut()[index].text.push_str("unsaved");
        editor.apply(Command::RequestCloseActive);
        let pending = editor.prompt().cloned();
        assert!(matches!(pending, Some(Prompt::ConfirmClose { .. })));

        let outcome = editor.apply(Command::SubmitName);

        assert_eq!(outcome, Outcome::default());
        assert_eq!(editor.prompt().cloned(), pending);

        // And with no prompt at all.
        editor.apply(Command::CancelPrompt);
        let outcome = editor.apply(Command::SubmitName);
        assert_eq!(outcome, Outcome::default());
        assert!(editor.prompt().is_none());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn prompt_name_mut_is_only_available_for_a_name_prompt() {
        let dir = temp_dir("prompt-name-mut");
        let mut editor = editor_in(&dir, &[], None);
        assert!(editor.prompt_name_mut().is_none());

        // A non-name prompt exposes no name field.
        editor.apply(Command::NewTab);
        let index = editor.active_index();
        editor.tabs_mut()[index].text.push_str("unsaved");
        editor.apply(Command::RequestCloseActive);
        assert!(editor.prompt_name_mut().is_none());

        editor.apply(Command::BeginNewFile(dir.clone()));
        editor.prompt_name_mut().unwrap().push_str("typed");
        assert_eq!(name_prompt(&editor).name, "typed");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cancel_prompt_clears_a_name_prompt() {
        let dir = temp_dir("cancel-name");
        let mut editor = editor_in(&dir, &[], None);

        editor.apply(Command::BeginNewFile(dir.clone()));
        editor.prompt_name_mut().unwrap().push_str("never.txt");
        editor.apply(Command::CancelPrompt);

        assert!(editor.prompt().is_none());
        assert!(!dir.join("never.txt").exists());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn set_expanded_persists_the_expanded_set() {
        let dir = temp_dir("set-expanded");
        let sub = dir.join("src");
        let other = dir.join("docs");
        fs::create_dir(&sub).unwrap();
        fs::create_dir(&other).unwrap();

        {
            let mut editor = editor_in(&dir, &[], Some(dir.as_path()));
            editor.apply(Command::SetExpanded {
                dir: sub.clone(),
                state: FolderState::Expanded,
            });
            editor.apply(Command::SetExpanded {
                dir: other.clone(),
                state: FolderState::Expanded,
            });
            editor.apply(Command::SetExpanded {
                dir: other.clone(),
                state: FolderState::Collapsed,
            });
        }

        let store = Store::at(dir.join("state")).unwrap();
        let ws = store.load().unwrap().workspace.unwrap();
        assert!(ws.is_expanded(&sub));
        assert!(!ws.is_expanded(&other));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_name_prompt_replaces_a_pending_close_prompt() {
        let dir = temp_dir("name-replaces-close");
        let mut editor = editor_in(&dir, &[], None);
        editor.apply(Command::NewTab);
        let index = editor.active_index();
        editor.tabs_mut()[index].text.push_str("unsaved");
        editor.apply(Command::RequestCloseActive);
        assert!(matches!(editor.prompt(), Some(Prompt::ConfirmClose { .. })));

        editor.apply(Command::BeginNewFolder(dir.clone()));

        assert_eq!(name_prompt(&editor).kind, NameKind::NewFolder);
        // The dirty tab was not closed by the replaced prompt.
        assert!(editor.tabs()[index].is_dirty());

        fs::remove_dir_all(&dir).unwrap();
    }
}
