//! The frext eframe application: tab bar, editor surface, and the wiring
//! that keeps swap files and the session index up to date.

use std::ops::Range;
use std::path::{Path, PathBuf};

use eframe::egui;

use crate::{
    highlight::MatchHighlights, persistence::Store, search::SearchQuery, tab::Tab,
    workspace::Workspace,
};

/// Live state for the find/replace bar, kept separate from the persisted
/// [`SearchQuery`] (only the query itself survives across sessions).
#[derive(Default)]
struct SearchState {
    /// The query and its toggles (persisted).
    query: SearchQuery,
    /// Whether the find/replace bar is currently shown.
    open: bool,
    /// Whether the replace row is shown.
    replace_open: bool,
    /// The replacement text for find/replace.
    replacement: String,
    /// Restrict matches to the editor's current selection when `true`.
    in_selection: bool,
    /// The active editor's current selection as a byte range, captured from
    /// the `TextEdit` each frame, used to scope a search-within-selection.
    selection: Option<Range<usize>>,
    /// Cached match byte ranges for the active buffer (recomputed each frame
    /// the bar is open).
    matches: Vec<Range<usize>>,
    /// Index into `matches` of the focused match, if any.
    current: Option<usize>,
    /// The most recent regex-compile error message, shown inline in red.
    error: Option<String>,
    /// Set when the search box should grab keyboard focus next frame (e.g.
    /// just after opening with Ctrl+F).
    focus_requested: bool,
    /// When `Some`, the editor should select this byte range and scroll it
    /// into view next frame (the focused match).
    scroll_to: Option<Range<usize>>,
}

/// The top-level application state.
pub struct FrextApp {
    store: Store,
    tabs: Vec<Tab>,
    active: usize,
    next_id: u64,
    /// The sidebar workspace (root directory + expanded folders), if open.
    workspace: Option<Workspace>,
    /// Find/replace bar state.
    search: SearchState,
    /// Index of a tab awaiting unsaved-changes close confirmation, if any.
    /// While set, a modal dialog asks whether to save, discard, or cancel.
    pending_close: Option<usize>,
    /// A pending name-entry prompt (rename / new file / new folder) from a
    /// sidebar context menu, if any. While set, a modal collects the name.
    pending_fs: Option<FsPrompt>,
}

/// A pending filesystem name-entry prompt driven by a modal dialog.
struct FsPrompt {
    /// Which operation the entered name applies to.
    kind: FsPromptKind,
    /// The path the operation targets: the entry being renamed, or the
    /// directory a new file/folder is created inside.
    target: PathBuf,
    /// The live text of the name field.
    name: String,
    /// Set on the first frame so the text field can grab keyboard focus.
    focus_requested: bool,
    /// An error message from the previous attempt, shown inline in red.
    error: Option<String>,
}

/// The operation a [`FsPrompt`] collects a name for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FsPromptKind {
    /// Rename `target` to the entered name.
    Rename,
    /// Create a new file named by the entry inside `target` (a directory).
    NewFile,
    /// Create a new folder named by the entry inside `target` (a directory).
    NewFolder,
}

/// A reserved line-number gutter column, to be filled by
/// [`FrextApp::paint_line_numbers`] once the editor has laid out.
struct Gutter {
    /// The allocated column rectangle (full available height).
    rect: egui::Rect,
    /// The monospace font the numbers are drawn in.
    font_id: egui::FontId,
    /// Right edge (x) the numbers are right-aligned to.
    right_x: f32,
    /// Number of digits the widest line number needs.
    digits: usize,
}

impl FrextApp {
    /// Build the app, restoring the previous session from the store.
    #[must_use]
    pub fn new(store: Store) -> Self {
        Self::with_args(store, &[], None)
    }

    /// Build the app, restoring the previous session and then opening any
    /// `files` passed on the command line. A file already present in the
    /// restored session is focused rather than opened a second time; the
    /// last successfully opened file becomes the active tab.
    #[must_use]
    pub fn with_files(store: Store, files: &[PathBuf]) -> Self {
        Self::with_args(store, files, None)
    }

    /// Build the app, restoring the previous session, opening any command
    /// line `files`, and optionally setting the sidebar workspace to `dir`.
    ///
    /// A `dir` passed on the command line replaces any restored workspace
    /// root (but keeps the previously-expanded folder set when the root is
    /// unchanged).
    #[must_use]
    pub fn with_args(store: Store, files: &[PathBuf], dir: Option<&Path>) -> Self {
        let restored = store.load().unwrap_or_else(|err| {
            log::error!("failed to load session, starting fresh: {err}");
            crate::persistence::RestoredSession::default()
        });

        let mut tabs = restored.tabs;
        let active = restored.active;
        let mut next_id = restored.next_id;
        let mut workspace = restored.workspace;
        let restored_search = restored.search;

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

        let mut app = Self {
            store,
            tabs,
            active,
            next_id,
            workspace,
            search: SearchState {
                query: restored_search,
                ..SearchState::default()
            },
            pending_close: None,
            pending_fs: None,
        };

        for file in files {
            app.open_path(file);
        }
        if !files.is_empty() || dir.is_some() {
            app.persist_session();
        }

        app
    }

    /// Open `path` into a tab and focus it. If a tab with the same path is
    /// already open it is focused instead of opening a duplicate. Returns
    /// `true` when a tab is now focused on the path (whether reused or newly
    /// opened), `false` when the file could not be read.
    ///
    /// Does not itself persist the session index; callers batch that.
    fn open_path(&mut self, path: &std::path::Path) -> bool {
        // Reuse an already-open tab. Compare canonicalized paths so that
        // e.g. `./foo.txt` and an absolute `foo.txt` are treated as one.
        let canonical = path.canonicalize().ok();
        if let Some(index) = self.tabs.iter().position(|tab| {
            tab.path.as_ref().is_some_and(|tab_path| {
                tab_path == path
                    || (canonical.is_some() && tab_path.canonicalize().ok() == canonical)
            })
        }) {
            self.active = index;
            return true;
        }

        match std::fs::read_to_string(path) {
            Ok(text) => {
                let id = self.alloc_id();
                let tab = Tab::from_file(id, path.to_path_buf(), text);
                if let Err(err) = self.store.save_swap(&tab) {
                    log::error!("failed to write swap for opened file: {err}");
                }
                self.tabs.push(tab);
                self.active = self.tabs.len() - 1;
                true
            }
            Err(err) => {
                log::error!("failed to open {}: {err}", path.display());
                false
            }
        }
    }

    /// Hand out a fresh, unique tab id.
    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Persist the session index, logging (but not propagating) failures so
    /// a transient write error never crashes the editor.
    fn persist_session(&self) {
        if let Err(err) = self.store.save_session(
            &self.tabs,
            self.active,
            self.next_id,
            self.workspace.as_ref(),
            &self.search.query,
        ) {
            log::error!("failed to save session index: {err}");
        }
    }

    /// Write the active tab's buffer to its swap file.
    fn persist_active_swap(&self) {
        if let Some(tab) = self.tabs.get(self.active) {
            if let Err(err) = self.store.save_swap(tab) {
                log::error!("failed to write swap file for tab {}: {err}", tab.id);
            }
        }
    }

    /// Reserve the line-number gutter column to the left of the editor.
    ///
    /// This only allocates horizontal space (sized for the widest line number)
    /// and remembers where to paint; the numbers are drawn later by
    /// [`Self::paint_line_numbers`], aligned to the editor's real rows so
    /// soft-wrapped continuation rows are left blank.
    fn reserve_line_number_gutter(ui: &mut egui::Ui, text: &str) -> Gutter {
        let line_count = line_count(text);
        let font_id = egui::TextStyle::Monospace.resolve(ui.style());
        let digits = line_count.to_string().len();

        // Width of `digits` monospace glyphs, measured from the digit '0'.
        let digit_w = ui.fonts_mut(|f| f.glyph_width(&font_id, '0'));
        #[expect(
            clippy::cast_precision_loss,
            reason = "digit count is tiny; the f32 cast is exact in practice"
        )]
        let numbers_w = digit_w * digits as f32;
        // A small gap between the gutter and the editor text.
        const GUTTER_GAP: f32 = 4.0;

        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(numbers_w + GUTTER_GAP, ui.available_height()),
            egui::Sense::hover(),
        );

        Gutter {
            rect,
            font_id,
            // Numbers are right-aligned to the inner edge (before the gap).
            right_x: rect.right() - GUTTER_GAP,
            digits,
        }
    }

    /// Paint line numbers into the reserved `gutter`, aligned to the editor's
    /// laid-out rows.
    ///
    /// A number is drawn only on a row that begins a new logical line; rows
    /// produced by soft wrapping are left blank. This mirrors the editor's
    /// actual galley (`output.galley`), so the numbering stays correct
    /// regardless of wrapping — including right after a newline is typed, which
    /// previously left a stray number on a wrap-continuation row.
    fn paint_line_numbers(
        ui: &egui::Ui,
        gutter: &Gutter,
        output: &egui::widgets::text_edit::TextEditOutput,
    ) {
        use egui::text::{LayoutJob, TextFormat};

        let color = crate::theme::gutter();
        let painter = ui.painter().with_clip_rect(gutter.rect);
        let galley = &output.galley;

        let ends_with_newline = galley.rows.iter().map(|r| r.ends_with_newline);
        for (row_index, number) in line_numbers_for_rows(ends_with_newline) {
            let Some(placed) = galley.rows.get(row_index) else {
                continue;
            };
            let digits = gutter.digits;
            let text = format!("{number:>digits$}");
            let mut job = LayoutJob::default();
            job.append(
                &text,
                0.0,
                TextFormat::simple(gutter.font_id.clone(), color),
            );
            let num_galley = ui.fonts_mut(|f| f.layout_job(job));

            // Align the number with the editor row: same y as the row,
            // right-aligned to the gutter's inner edge.
            let row_top_y = output.galley_pos.y + placed.pos.y;
            let pos = egui::pos2(gutter.right_x - num_galley.size().x, row_top_y);
            painter.galley(pos, num_galley, color);
        }
    }

    /// Open a new, empty, untitled tab and focus it.
    fn new_tab(&mut self) {
        let id = self.alloc_id();
        self.tabs.push(Tab::new_untitled(id));
        self.active = self.tabs.len() - 1;
        self.persist_active_swap();
        self.persist_session();
    }

    /// Request to close the tab at `index`.
    ///
    /// A clean buffer is closed immediately. A buffer with unsaved changes
    /// instead opens the confirmation modal (save / discard / cancel) by
    /// recording `index` in `pending_close`; the actual close happens when the
    /// user chooses save or discard.
    fn request_close_tab(&mut self, index: usize) {
        let Some(tab) = self.tabs.get(index) else {
            return;
        };
        if tab.is_dirty() {
            self.pending_close = Some(index);
        } else {
            self.close_tab(index);
        }
    }

    /// Render and resolve the unsaved-changes confirmation modal.
    ///
    /// A no-op when no close is pending. Otherwise it shows a modal with
    /// "Save and close", "Discard changes", and "Cancel". Clicking the
    /// backdrop or pressing Escape cancels (keeping the tab open). A
    /// "Save and close" that is itself cancelled (an untitled tab whose
    /// save-as dialog is dismissed, or a write error) leaves the tab open and
    /// dismisses the modal rather than silently discarding the buffer.
    fn resolve_pending_close(&mut self, ctx: &egui::Context) {
        let Some(index) = self.pending_close else {
            return;
        };

        // The tab may have been removed out from under a stale request.
        let Some(title) = self.tabs.get(index).map(Tab::display_name) else {
            self.pending_close = None;
            return;
        };

        let mut choice = CloseChoice::Pending;
        let modal =
            egui::Modal::new(egui::Id::new(("frext_confirm_close", index))).show(ctx, |ui| {
                ui.set_min_width(320.0);
                ui.heading("Unsaved changes");
                ui.add_space(4.0);
                ui.label(format!(
                    "\u{201c}{title}\u{201d} has unsaved changes. \
                     What would you like to do?"
                ));
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("Save and close").clicked() {
                        choice = CloseChoice::Save;
                    }
                    if ui.button("Discard changes").clicked() {
                        choice = CloseChoice::Discard;
                    }
                    if ui.button("Cancel").clicked() {
                        choice = CloseChoice::Cancel;
                    }
                });
            });

        // Clicking outside the dialog, or pressing Escape, cancels the close.
        if modal.should_close() {
            choice = CloseChoice::Cancel;
        }

        match choice {
            CloseChoice::Pending => {}
            CloseChoice::Cancel => self.pending_close = None,
            CloseChoice::Discard => {
                self.pending_close = None;
                self.close_tab(index);
            }
            CloseChoice::Save => {
                self.pending_close = None;
                // Only close if the save actually succeeded; a cancelled
                // save-as or a write error keeps the buffer open.
                if self.save_tab(index) {
                    self.close_tab(index);
                }
            }
        }
    }

    /// Close the tab at `index`, removing its swap file.
    fn close_tab(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }

        let tab = self.tabs.remove(index);
        if let Err(err) = self.store.remove_swap(tab.id) {
            log::error!("failed to remove swap file for tab {}: {err}", tab.id);
        }

        if self.tabs.is_empty() {
            let id = self.alloc_id();
            self.tabs.push(Tab::new_untitled(id));
            self.active = 0;
            self.persist_active_swap();
        } else if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
        }

        self.persist_session();
    }

    /// Close the sidebar workspace, removing it from the persisted session so
    /// it stays closed on the next launch. A no-op when no workspace is open.
    fn close_workspace(&mut self) {
        if self.workspace.take().is_some() {
            self.persist_session();
        }
    }

    /// Apply a deferred editor context-menu action after the editor borrow has
    /// ended.
    ///
    /// Clipboard text edits (cut/copy/paste) act on the captured char
    /// `selection` over the active buffer, then store the resulting caret back
    /// into the live `TextEdit` state under `editor_id` so the cursor follows
    /// the edit. Find/replace and save reuse the existing deferred actions.
    ///
    /// Returns `true` when the buffer changed (so the caller can persist and
    /// refresh matches), matching how the editor's own `changed()` is handled.
    fn apply_editor_action(
        &mut self,
        ctx: &egui::Context,
        action: EditorAction,
        editor_id: Option<egui::Id>,
        selection: Option<Range<usize>>,
    ) -> bool {
        let selection = selection.unwrap_or(0..0);

        match action {
            EditorAction::None => false,
            EditorAction::Copy => {
                if let Some(tab) = self.tabs.get(self.active) {
                    let text =
                        crate::edit_ops::selected_text(&tab.text, selection.start, selection.end);
                    if !text.is_empty() {
                        crate::clipboard::write_text(text);
                    }
                }
                false
            }
            EditorAction::Cut => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return false;
                };
                let text =
                    crate::edit_ops::selected_text(&tab.text, selection.start, selection.end);
                if text.is_empty() {
                    return false;
                }
                crate::clipboard::write_text(text);
                let caret =
                    crate::edit_ops::delete_range(&mut tab.text, selection.start, selection.end);
                Self::store_caret(ctx, editor_id, caret);
                true
            }
            EditorAction::Paste => {
                let Some(clip) = crate::clipboard::read_text() else {
                    return false;
                };
                if clip.is_empty() {
                    return false;
                }
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return false;
                };
                let caret = crate::edit_ops::replace_range(
                    &mut tab.text,
                    selection.start,
                    selection.end,
                    &clip,
                );
                Self::store_caret(ctx, editor_id, caret);
                true
            }
            EditorAction::SelectAll => {
                if let (Some(tab), Some(id)) = (self.tabs.get(self.active), editor_id) {
                    let len = tab.text.chars().count();
                    if let Some(mut state) = egui::text_edit::TextEditState::load(ctx, id) {
                        state
                            .cursor
                            .set_char_range(Some(egui::text::CCursorRange::two(
                                egui::text::CCursor::new(0),
                                egui::text::CCursor::new(len),
                            )));
                        state.store(ctx, id);
                    }
                    ctx.memory_mut(|m| m.request_focus(id));
                }
                false
            }
            EditorAction::Find => {
                self.open_search(false);
                false
            }
            EditorAction::FindReplace => {
                self.open_search(true);
                false
            }
            EditorAction::Save => {
                self.save_active();
                false
            }
        }
    }

    /// Store `caret` (a character index) as a collapsed selection into the
    /// `TextEdit` state under `editor_id`, and refocus the editor, so the
    /// cursor tracks a programmatic edit. A no-op when there is no id or no
    /// stored state yet.
    fn store_caret(ctx: &egui::Context, editor_id: Option<egui::Id>, caret: usize) {
        let Some(id) = editor_id else {
            return;
        };
        if let Some(mut state) = egui::text_edit::TextEditState::load(ctx, id) {
            let cursor = egui::text::CCursor::new(caret);
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::one(cursor)));
            state.store(ctx, id);
        }
        ctx.memory_mut(|m| m.request_focus(id));
    }

    /// Apply a deferred sidebar context-menu action after the tree borrow has
    /// ended. The mutating actions (rename, create, trash) open a modal or
    /// touch the filesystem and reconcile any affected open tabs.
    fn apply_tree_action(&mut self, ctx: &egui::Context, action: TreeAction) {
        match action {
            TreeAction::Open(path) => {
                if self.open_path(&path) {
                    self.persist_session();
                }
            }
            TreeAction::CopyPath(path) => {
                ctx.copy_text(path.display().to_string());
            }
            TreeAction::CopyRelativePath(path) => {
                ctx.copy_text(self.relative_to_root(&path));
            }
            TreeAction::Reveal(path) => reveal_in_file_manager(&path),
            TreeAction::Rename(path) => {
                self.pending_fs = Some(FsPrompt {
                    kind: FsPromptKind::Rename,
                    name: path
                        .file_name()
                        .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
                    target: path,
                    focus_requested: true,
                    error: None,
                });
            }
            TreeAction::NewFile(dir) => {
                self.pending_fs = Some(FsPrompt {
                    kind: FsPromptKind::NewFile,
                    target: dir,
                    name: String::new(),
                    focus_requested: true,
                    error: None,
                });
            }
            TreeAction::NewFolder(dir) => {
                self.pending_fs = Some(FsPrompt {
                    kind: FsPromptKind::NewFolder,
                    target: dir,
                    name: String::new(),
                    focus_requested: true,
                    error: None,
                });
            }
            TreeAction::Trash(path) => self.trash_path(&path),
        }
    }

    /// Render `path` relative to the workspace root for "Copy relative path".
    /// Falls back to the full path display when there is no workspace or the
    /// path is not under the root.
    fn relative_to_root(&self, path: &Path) -> String {
        if let Some(ws) = self.workspace.as_ref() {
            if let Ok(rel) = path.strip_prefix(&ws.root) {
                return rel.display().to_string();
            }
        }
        path.display().to_string()
    }

    /// Move `path` to the OS trash, then reconcile editor state: any tab whose
    /// file was trashed (the file itself, or a file under a trashed directory)
    /// is closed, since its backing file no longer exists.
    fn trash_path(&mut self, path: &Path) {
        if let Err(err) = crate::fs_ops::move_to_trash(path) {
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
    /// Returns the rename result so the caller can keep the modal open and
    /// show the error on failure.
    fn rename_path(&mut self, from: &Path, new_name: &str) -> Result<(), crate::error::FsError> {
        let to = crate::fs_ops::rename(from, new_name)?;
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

    /// Render and resolve the filesystem name-entry modal (rename / new file /
    /// new folder). A no-op when nothing is pending. On a successful create,
    /// a new file is also opened in a tab.
    fn resolve_pending_fs(&mut self, ctx: &egui::Context) {
        let Some(prompt) = self.pending_fs.as_mut() else {
            return;
        };

        let (title, hint, action_label) = match prompt.kind {
            FsPromptKind::Rename => ("Rename", "New name", "Rename"),
            FsPromptKind::NewFile => ("New file", "File name", "Create"),
            FsPromptKind::NewFolder => ("New folder", "Folder name", "Create"),
        };

        let mut submit = false;
        let mut cancel = false;

        let modal = egui::Modal::new(egui::Id::new("frext_fs_prompt")).show(ctx, |ui| {
            ui.set_min_width(320.0);
            ui.heading(title);
            ui.add_space(8.0);

            let field = ui.add(
                egui::TextEdit::singleline(&mut prompt.name)
                    .hint_text(hint)
                    .desired_width(f32::INFINITY),
            );
            if prompt.focus_requested {
                field.request_focus();
                prompt.focus_requested = false;
            }
            if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                submit = true;
            }

            if let Some(error) = &prompt.error {
                ui.add_space(4.0);
                ui.colored_label(ui.visuals().error_fg_color, error);
            }

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button(action_label).clicked() {
                    submit = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });

        if modal.should_close() {
            cancel = true;
        }

        if cancel {
            self.pending_fs = None;
            return;
        }
        if !submit {
            return;
        }

        // Apply the submitted name. On failure, keep the modal open and show
        // the error inline.
        // `pending_fs` is Some by the guard at the top of this method.
        let Some(prompt) = self.pending_fs.take() else {
            return;
        };
        let result = match prompt.kind {
            FsPromptKind::Rename => self
                .rename_path(&prompt.target, &prompt.name)
                .map(|()| None),
            FsPromptKind::NewFile => {
                crate::fs_ops::create_file(&prompt.target, &prompt.name).map(Some)
            }
            FsPromptKind::NewFolder => {
                crate::fs_ops::create_dir(&prompt.target, &prompt.name).map(|_| None)
            }
        };

        match result {
            Ok(created_file) => {
                if let Some(path) = created_file {
                    if self.open_path(&path) {
                        self.persist_session();
                    }
                }
            }
            Err(err) => {
                // Re-open the prompt carrying the error message.
                self.pending_fs = Some(FsPrompt {
                    error: Some(err.to_string()),
                    focus_requested: true,
                    ..prompt
                });
            }
        }
    }

    /// Open the find bar, seeding the pattern from the editor's current
    /// selection when there is a non-empty one, and request keyboard focus.
    fn open_search(&mut self, replace: bool) {
        if let Some(selected) = self.active_selection_text() {
            if !selected.is_empty() && !selected.contains('\n') {
                // A single-line selection is a sensible search seed; multi-line
                // selections are left alone (they are usually a scope, not a
                // pattern).
                self.search.query.pattern = selected;
                // Plain mode for a seeded literal, so regex metacharacters in
                // the selected text match literally.
                self.search.query.regex = false;
            }
        }
        self.search.open = true;
        self.search.replace_open = replace;
        self.search.focus_requested = true;
        self.recompute_matches();
    }

    /// The active editor's currently-selected text, derived from the selection
    /// byte range captured from the `TextEdit` on the previous frame.
    fn active_selection_text(&self) -> Option<String> {
        let range = self.search.selection.clone()?;
        if range.start >= range.end {
            return None;
        }
        let tab = self.tabs.get(self.active)?;
        tab.text.get(range).map(str::to_owned)
    }

    /// Close the find bar, clearing the live match cache so highlights vanish.
    fn close_search(&mut self) {
        self.search.open = false;
        self.search.matches.clear();
        self.search.current = None;
        self.search.scroll_to = None;
    }

    /// Recompute the match set for the active buffer from the current query.
    /// Updates the inline error on an invalid regex and clamps `current`.
    fn recompute_matches(&mut self) {
        self.search.error = None;
        self.search.matches.clear();

        if self.search.query.is_empty() {
            self.search.current = None;
            return;
        }

        let Some(tab) = self.tabs.get(self.active) else {
            self.search.current = None;
            return;
        };

        let matcher = match self.search.query.compile() {
            Ok(matcher) => matcher,
            Err(err) => {
                self.search.error = Some(err.to_string());
                self.search.current = None;
                return;
            }
        };

        // Search-within-selection restricts to the active selection's bytes.
        let matches = match (self.search.in_selection, self.search.selection.clone()) {
            (true, Some(scope)) if scope.start < scope.end => {
                matcher.find_matches_in(&tab.text, scope)
            }
            _ => matcher.find_matches(&tab.text),
        };

        self.search.matches = matches;
        self.search.current = match self.search.current {
            Some(i) if i < self.search.matches.len() => Some(i),
            _ if self.search.matches.is_empty() => None,
            _ => Some(0),
        };
    }

    /// Move to the next (`forward`) or previous match, wrapping around, and
    /// request that the editor scroll it into view.
    fn step_match(&mut self, forward: bool) {
        let len = self.search.matches.len();
        if len == 0 {
            return;
        }
        let next = match self.search.current {
            Some(i) if forward => (i + 1) % len,
            Some(i) => (i + len - 1) % len,
            None => 0,
        };
        self.search.current = Some(next);
        if let Some(range) = self.search.matches.get(next).cloned() {
            self.search.scroll_to = Some(range);
        }
    }

    /// Replace the currently-focused match with the replacement text, then
    /// re-find so the highlights and indices stay correct.
    fn replace_current(&mut self) {
        let Some(index) = self.search.current else {
            return;
        };
        let Some(range) = self.search.matches.get(index).cloned() else {
            return;
        };
        let matcher = match self.search.query.compile() {
            Ok(matcher) => matcher,
            Err(err) => {
                self.search.error = Some(err.to_string());
                return;
            }
        };
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };

        if let Some((new_text, _)) =
            matcher.replace_one_at(&tab.text, range.start, &self.search.replacement)
        {
            tab.text = new_text;
            self.persist_active_swap();
            // Keep the cursor near where we just replaced.
            self.recompute_matches();
            // Focus the match at (or after) the replaced position.
            if let Some(pos) = self
                .search
                .matches
                .iter()
                .position(|m| m.start >= range.start)
            {
                self.search.current = Some(pos);
            }
        }
    }

    /// Replace every match in the active buffer with the replacement text.
    fn replace_all(&mut self) {
        if self.search.query.is_empty() {
            return;
        }
        let matcher = match self.search.query.compile() {
            Ok(matcher) => matcher,
            Err(err) => {
                self.search.error = Some(err.to_string());
                return;
            }
        };
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };

        let new_text = matcher.replace_all(&tab.text, &self.search.replacement);
        if new_text != tab.text {
            tab.text = new_text;
            self.persist_active_swap();
        }
        self.search.current = None;
        self.recompute_matches();
    }

    /// The match highlights to hand to the editor layouter.
    fn match_highlights(&self) -> MatchHighlights {
        if self.search.open {
            MatchHighlights {
                ranges: self.search.matches.clone(),
                current: self.search.current,
            }
        } else {
            MatchHighlights::default()
        }
    }

    /// Render the find / replace bar. Pushes the chosen action into `action`
    /// and recomputes matches when the query or toggles change so highlights
    /// track typing live.
    fn search_bar_ui(&mut self, ui: &mut egui::Ui, action: &mut SearchAction) {
        let panel_frame =
            egui::Frame::side_top_panel(ui.style()).fill(ui.style().visuals.panel_fill);

        egui::Panel::top("frext_search_bar")
            .frame(panel_frame)
            .show(ui, |ui| {
                let mut query_changed = false;

                ui.horizontal(|ui| {
                    ui.label("Find");
                    let field = ui.add(
                        egui::TextEdit::singleline(&mut self.search.query.pattern)
                            .desired_width(220.0)
                            .hint_text("search…"),
                    );
                    if self.search.focus_requested {
                        field.request_focus();
                        self.search.focus_requested = false;
                    }
                    if field.changed() {
                        query_changed = true;
                    }
                    // Enter / Shift+Enter step matches while the field is
                    // focused.
                    if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        let forward = !ui.input(|i| i.modifiers.shift);
                        *action = SearchAction::Step { forward };
                        field.request_focus();
                    }

                    // Toggles.
                    if ui
                        .selectable_label(self.search.query.case_sensitive, "Aa")
                        .on_hover_text("Match case")
                        .clicked()
                    {
                        self.search.query.case_sensitive = !self.search.query.case_sensitive;
                        query_changed = true;
                    }
                    if ui
                        .selectable_label(self.search.query.whole_word, "W")
                        .on_hover_text("Whole word")
                        .clicked()
                    {
                        self.search.query.whole_word = !self.search.query.whole_word;
                        query_changed = true;
                    }
                    if ui
                        .selectable_label(self.search.query.regex, ".*")
                        .on_hover_text("Regular expression")
                        .clicked()
                    {
                        self.search.query.regex = !self.search.query.regex;
                        query_changed = true;
                    }
                    if ui
                        .selectable_label(self.search.in_selection, "In sel")
                        .on_hover_text("Search within the current selection")
                        .clicked()
                    {
                        self.search.in_selection = !self.search.in_selection;
                        query_changed = true;
                    }

                    // Navigation + count. Plain-text labels because egui's
                    // default fonts (Ubuntu-Light + NotoEmoji) do not cover the
                    // geometric triangle glyphs, which render as tofu squares.
                    if ui
                        .button("Prev")
                        .on_hover_text("Previous match (Shift+F3)")
                        .clicked()
                    {
                        *action = SearchAction::Step { forward: false };
                    }
                    if ui
                        .button("Next")
                        .on_hover_text("Next match (F3 / Enter)")
                        .clicked()
                    {
                        *action = SearchAction::Step { forward: true };
                    }
                    ui.label(self.match_count_label());

                    if ui
                        .selectable_label(self.search.replace_open, "Replace")
                        .on_hover_text("Toggle replace")
                        .clicked()
                    {
                        self.search.replace_open = !self.search.replace_open;
                    }
                    if ui.button("\u{00d7}").on_hover_text("Close (Esc)").clicked() {
                        *action = SearchAction::Close;
                    }
                });

                if let Some(err) = &self.search.error {
                    ui.colored_label(crate::theme::error(), format!("regex: {err}"));
                }

                if self.search.replace_open {
                    ui.horizontal(|ui| {
                        ui.label("With");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.search.replacement)
                                .desired_width(220.0)
                                .hint_text(if self.search.query.regex {
                                    "replacement ($1 for groups)…"
                                } else {
                                    "replacement…"
                                }),
                        );
                        if ui.button("Replace").clicked() {
                            *action = SearchAction::ReplaceCurrent;
                        }
                        if ui.button("Replace all").clicked() {
                            *action = SearchAction::ReplaceAll;
                        }
                    });
                }

                if query_changed && matches!(action, SearchAction::None) {
                    *action = SearchAction::Recompute;
                }
            });
    }

    /// A short "current/total" (or "no results") label for the find bar.
    fn match_count_label(&self) -> String {
        if self.search.query.is_empty() {
            String::new()
        } else if self.search.matches.is_empty() {
            "no results".to_owned()
        } else {
            let current = self.search.current.map_or(0, |i| i + 1);
            format!("{current}/{}", self.search.matches.len())
        }
    }

    /// Show a native open-file dialog and load the chosen file into a tab.
    fn open_file(&mut self) {
        let Some(path) = rfd_pick_file() else {
            return;
        };

        if self.open_path(&path) {
            self.persist_session();
        }
    }

    /// Save the active tab. Prompts for a path when the buffer is untitled.
    fn save_active(&mut self) {
        self.save_tab(self.active);
    }

    /// Save the tab at `index` to disk, prompting for a path when the tab is
    /// untitled. Returns `true` when the buffer was written (so it is now
    /// clean), and `false` when the save did not happen — an out-of-range
    /// index, a cancelled save-as dialog, or an I/O error.
    fn save_tab(&mut self, index: usize) -> bool {
        let Some(tab) = self.tabs.get(index) else {
            return false;
        };

        let path = match tab.path.clone() {
            Some(path) => path,
            None => match rfd_save_file() {
                Some(path) => path,
                None => return false,
            },
        };

        if let Err(err) = std::fs::write(&path, &tab.text) {
            log::error!("failed to save {}: {err}", path.display());
            return false;
        }

        if let Some(tab) = self.tabs.get_mut(index) {
            tab.path = Some(path);
            tab.saved_text = tab.text.clone();
        }
        self.persist_session();
        true
    }

    /// Render the contents of one directory in the file tree.
    ///
    /// Sub-directories are shown as collapsible headers whose open/closed
    /// state is driven by (and recorded back into) the workspace's expanded
    /// set via `expand_changes`. Files are clickable; a click records the
    /// path in `file_to_open`. The directory contents are read lazily, so a
    /// large tree only costs what the user expands.
    fn tree_dir(
        ui: &mut egui::Ui,
        ws: &Workspace,
        dir: &Path,
        active_canonical: Option<&Path>,
        file_to_open: &mut Option<PathBuf>,
        expand_changes: &mut Vec<(PathBuf, bool)>,
        tree_action: &mut Option<TreeAction>,
    ) {
        let (dirs, files) = crate::workspace::read_dir_split(dir);

        for sub in dirs {
            let name = sub
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            let was_open = ws.is_expanded(&sub);

            // Drive a `CollapsingState` directly (rather than the simpler
            // `CollapsingHeader`) so the folder's type-specific icon can sit
            // in the header next to its name. The persisted expanded set seeds
            // the state on first sight; thereafter egui owns the live
            // open/closed state and a header click toggles it, which we mirror
            // back into the workspace via `expand_changes`.
            let id = ui.make_persistent_id(&sub);
            let state = egui::collapsing_header::CollapsingState::load_with_default_open(
                ui.ctx(),
                id,
                was_open,
            );
            let is_open = state.is_open();

            let (_toggle, header, _body) = state
                .show_header(ui, |ui| {
                    Self::file_icon(ui, crate::file_icon::icon_for_dir(&name, is_open));
                    ui.label(name);
                })
                .body(|ui| {
                    Self::tree_dir(
                        ui,
                        ws,
                        &sub,
                        active_canonical,
                        file_to_open,
                        expand_changes,
                        tree_action,
                    );
                });

            header
                .response
                .context_menu(|ui| Self::dir_context_menu(ui, &sub, tree_action));

            if header.response.clicked() {
                expand_changes.push((sub.clone(), !was_open));
            }
        }

        for file in files {
            let name = file
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned());

            // Highlight the file backing the active tab.
            let is_active = active_canonical
                .is_some_and(|active| file.canonicalize().ok().as_deref() == Some(active));

            let row = ui
                .horizontal(|ui| {
                    Self::file_icon(ui, crate::file_icon::icon_for_file(&name));
                    ui.selectable_label(is_active, name)
                })
                .inner;

            row.context_menu(|ui| Self::file_context_menu(ui, &file, tree_action));

            if row.clicked() {
                *file_to_open = Some(file);
            }
        }
    }

    /// The right-click menu for a file row. Choices are recorded in
    /// `tree_action` and applied after the tree borrow ends.
    fn file_context_menu(ui: &mut egui::Ui, file: &Path, tree_action: &mut Option<TreeAction>) {
        if ui.button("Open").clicked() {
            *tree_action = Some(TreeAction::Open(file.to_path_buf()));
            ui.close();
        }
        ui.separator();
        if ui.button("Copy path").clicked() {
            *tree_action = Some(TreeAction::CopyPath(file.to_path_buf()));
            ui.close();
        }
        if ui.button("Copy relative path").clicked() {
            *tree_action = Some(TreeAction::CopyRelativePath(file.to_path_buf()));
            ui.close();
        }
        if ui.button("Reveal in file manager").clicked() {
            *tree_action = Some(TreeAction::Reveal(file.to_path_buf()));
            ui.close();
        }
        ui.separator();
        if ui.button("Rename\u{2026}").clicked() {
            *tree_action = Some(TreeAction::Rename(file.to_path_buf()));
            ui.close();
        }
        if ui.button("Delete (move to trash)").clicked() {
            *tree_action = Some(TreeAction::Trash(file.to_path_buf()));
            ui.close();
        }
    }

    /// The right-click menu for a folder header. Choices are recorded in
    /// `tree_action` and applied after the tree borrow ends.
    fn dir_context_menu(ui: &mut egui::Ui, dir: &Path, tree_action: &mut Option<TreeAction>) {
        if ui.button("New file\u{2026}").clicked() {
            *tree_action = Some(TreeAction::NewFile(dir.to_path_buf()));
            ui.close();
        }
        if ui.button("New folder\u{2026}").clicked() {
            *tree_action = Some(TreeAction::NewFolder(dir.to_path_buf()));
            ui.close();
        }
        ui.separator();
        if ui.button("Copy path").clicked() {
            *tree_action = Some(TreeAction::CopyPath(dir.to_path_buf()));
            ui.close();
        }
        if ui.button("Copy relative path").clicked() {
            *tree_action = Some(TreeAction::CopyRelativePath(dir.to_path_buf()));
            ui.close();
        }
        if ui.button("Reveal in file manager").clicked() {
            *tree_action = Some(TreeAction::Reveal(dir.to_path_buf()));
            ui.close();
        }
        ui.separator();
        if ui.button("Rename\u{2026}").clicked() {
            *tree_action = Some(TreeAction::Rename(dir.to_path_buf()));
            ui.close();
        }
        if ui.button("Delete (move to trash)").clicked() {
            *tree_action = Some(TreeAction::Trash(dir.to_path_buf()));
            ui.close();
        }
    }

    /// The right-click menu for the editor surface. `has_selection` greys out
    /// Cut/Copy when there is nothing selected. Choices are recorded in
    /// `editor_action` and applied after the editor borrow ends.
    fn editor_context_menu(
        ui: &mut egui::Ui,
        has_selection: bool,
        editor_action: &mut EditorAction,
    ) {
        if ui
            .add_enabled(has_selection, egui::Button::new("Cut"))
            .clicked()
        {
            *editor_action = EditorAction::Cut;
            ui.close();
        }
        if ui
            .add_enabled(has_selection, egui::Button::new("Copy"))
            .clicked()
        {
            *editor_action = EditorAction::Copy;
            ui.close();
        }
        if ui.button("Paste").clicked() {
            *editor_action = EditorAction::Paste;
            ui.close();
        }
        if ui.button("Select all").clicked() {
            *editor_action = EditorAction::SelectAll;
            ui.close();
        }
        ui.separator();
        if ui.button("Find\u{2026}").clicked() {
            *editor_action = EditorAction::Find;
            ui.close();
        }
        if ui.button("Find and replace\u{2026}").clicked() {
            *editor_action = EditorAction::FindReplace;
            ui.close();
        }
        ui.separator();
        if ui.button("Save").clicked() {
            *editor_action = EditorAction::Save;
            ui.close();
        }
    }

    /// Draw the file-type icon for `stem` inline, sized to the current text
    /// row. A stem with no embedded artwork (which should not occur for stems
    /// produced by [`crate::file_icon`]) simply renders nothing, leaving the
    /// row icon-less rather than failing.
    fn file_icon(ui: &mut egui::Ui, stem: &'static str) {
        let size = ui.text_style_height(&egui::TextStyle::Body);
        if let Some(source) = crate::file_icon::image_source(stem) {
            ui.add(
                egui::Image::new(source)
                    .fit_to_exact_size(egui::vec2(size, size))
                    .maintain_aspect_ratio(true),
            );
        }
    }
}

impl eframe::App for FrextApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let mut action = MenuAction::None;

        // Pick up external modifications to the active tab's file (detected
        // by a change in on-disk size). A clean buffer is reloaded; a dirty
        // one is left untouched. Keeping the swap file and session index in
        // step after a reload preserves crash-safety.
        if let Some(tab) = self.tabs.get_mut(self.active) {
            if tab.reload_if_changed_on_disk() {
                self.persist_active_swap();
                self.persist_session();
            }
        }

        // Keyboard shortcuts. Search actions are deferred like menu actions so
        // they run after the editor borrow ends.
        let mut search_action = SearchAction::None;
        ctx.input_mut(|i| {
            if i.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND,
                egui::Key::N,
            )) {
                action = MenuAction::NewTab;
            }
            if i.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND,
                egui::Key::O,
            )) {
                action = MenuAction::Open;
            }
            if i.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND,
                egui::Key::S,
            )) {
                action = MenuAction::Save;
            }
            if i.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND,
                egui::Key::W,
            )) {
                action = MenuAction::CloseActive;
            }
            if i.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND,
                egui::Key::F,
            )) {
                search_action = SearchAction::Open { replace: false };
            }
            if i.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND,
                egui::Key::H,
            )) {
                search_action = SearchAction::Open { replace: true };
            }
            // While the bar is open, F3 / Shift+F3 cycle matches and Esc
            // closes it.
            if self.search.open {
                if i.consume_shortcut(&egui::KeyboardShortcut::new(
                    egui::Modifiers::NONE,
                    egui::Key::F3,
                )) {
                    search_action = SearchAction::Step { forward: true };
                }
                if i.consume_shortcut(&egui::KeyboardShortcut::new(
                    egui::Modifiers::SHIFT,
                    egui::Key::F3,
                )) {
                    search_action = SearchAction::Step { forward: false };
                }
                if i.key_pressed(egui::Key::Escape) {
                    search_action = SearchAction::Close;
                }
            }
        });

        egui::Panel::top("frext_tab_bar").show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .button("\u{002b}")
                    .on_hover_text("New tab (Ctrl+N)")
                    .clicked()
                {
                    action = MenuAction::NewTab;
                }
                if ui
                    .button("Open")
                    .on_hover_text("Open file (Ctrl+O)")
                    .clicked()
                {
                    action = MenuAction::Open;
                }
                if ui.button("Save").on_hover_text("Save (Ctrl+S)").clicked() {
                    action = MenuAction::Save;
                }
                ui.separator();

                for index in 0..self.tabs.len() {
                    let selected = index == self.active;
                    let title = self.tabs[index].title();
                    let label = if selected {
                        egui::RichText::new(title).color(crate::theme::accent())
                    } else {
                        egui::RichText::new(title)
                    };
                    if ui.selectable_label(selected, label).clicked() {
                        action = MenuAction::Select(index);
                    }
                    if ui
                        .small_button("\u{00d7}")
                        .on_hover_text("Close tab (Ctrl+W)")
                        .clicked()
                    {
                        action = MenuAction::Close(index);
                    }
                    ui.separator();
                }
            });
        });

        // Find / replace bar, shown directly under the tab bar.
        if self.search.open {
            self.search_bar_ui(ui, &mut search_action);
        }

        // Left sidebar: file tree for the open workspace, if any.
        if self.workspace.is_some() {
            let mut file_to_open: Option<PathBuf> = None;
            let mut expand_changes: Vec<(PathBuf, bool)> = Vec::new();
            let mut tree_action: Option<TreeAction> = None;
            let mut close_workspace = false;

            // The active tab's file, so the tree can highlight it. Compared
            // canonicalised so a relative tree path matches an absolute tab
            // path (and vice versa).
            let active_path = self.tabs.get(self.active).and_then(|tab| tab.path.clone());
            let active_canonical = active_path.as_ref().and_then(|p| p.canonicalize().ok());

            // Match the sidebar fill to the central panel so the two panes
            // share one background colour.
            let panel_frame =
                egui::Frame::side_top_panel(ui.style()).fill(ui.style().visuals.panel_fill);

            egui::Panel::left("frext_file_tree")
                .resizable(true)
                .frame(panel_frame)
                .show(ui, |ui| {
                    // `workspace` is Some by the guard above.
                    if let Some(ws) = self.workspace.as_ref() {
                        let root = ws.root.clone();
                        // Show the resolved root with the home directory
                        // collapsed to `~` (so `frext .` reads as a real
                        // location, not a bare dot).
                        let root_label = ws.display_root();
                        ui.horizontal(|ui| {
                            if ui
                                .small_button("\u{00d7}")
                                .on_hover_text("Close file tree")
                                .clicked()
                            {
                                close_workspace = true;
                            }
                            ui.strong(root_label);
                        });
                        ui.separator();
                        let empty_area = egui::ScrollArea::vertical()
                            .show(ui, |ui| {
                                Self::tree_dir(
                                    ui,
                                    ws,
                                    &root,
                                    active_canonical.as_deref(),
                                    &mut file_to_open,
                                    &mut expand_changes,
                                    &mut tree_action,
                                );
                                // Claim the remaining empty space below the last
                                // row so a right-click there targets the root,
                                // while row right-clicks keep their own menus.
                                ui.allocate_response(ui.available_size(), egui::Sense::click())
                            })
                            .inner;
                        // Right-clicking the empty tree area acts on the root
                        // directory (new file / new folder there).
                        empty_area
                            .context_menu(|ui| Self::dir_context_menu(ui, &root, &mut tree_action));
                    }
                });

            if close_workspace {
                // Drop the sidebar and remove it from the persisted session
                // so it stays closed on the next launch.
                self.close_workspace();
            } else {
                for (dir, expanded) in expand_changes {
                    if let Some(ws) = self.workspace.as_mut() {
                        if ws.set_expanded(&dir, expanded) {
                            self.persist_session();
                        }
                    }
                }
                if let Some(path) = file_to_open {
                    if self.open_path(&path) {
                        self.persist_session();
                    }
                }
                if let Some(action) = tree_action {
                    self.apply_tree_action(&ctx, action);
                }
            }
        }

        let highlights = self.match_highlights();
        // A scroll-to-match request becomes a cursor selection on the editor;
        // converted from byte to char indices below.
        let scroll_to = self.search.scroll_to.take();
        let mut new_selection: Option<Range<usize>> = None;
        let mut changed = false;
        // Editor context-menu plumbing: the chosen action plus what applying
        // it needs (the editor's id and the live char selection), captured
        // inside the closure and applied after the borrow ends.
        let mut editor_action = EditorAction::None;
        let mut editor_char_selection: Option<Range<usize>> = None;
        let mut editor_id_out: Option<egui::Id> = None;

        egui::CentralPanel::default().show(ui, |ui| {
            if let Some(tab) = self.tabs.get_mut(self.active) {
                let language = crate::highlight::language_from_path(tab.path.as_deref());
                let mut layouter = crate::highlight::layouter(&language, &highlights);

                // The editor must be wrapped in a ScrollArea: a bare
                // `add_sized(available_size, ...)` clamps the TextEdit to the
                // viewport, so content past the bottom is clipped and cannot
                // be scrolled to. Letting the TextEdit grow to its natural
                // height inside a scroll area is what makes scrolling work.
                //
                // A line-number gutter is laid out to the left of the editor,
                // inside the same scroll area so it scrolls in lockstep. The
                // gutter is painted from the editor's own laid-out galley so it
                // tracks soft-wrapped rows: a number sits only on a row that
                // begins a logical line, and wrap-continuation rows stay blank.
                let output = egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.horizontal_top(|ui| {
                            // Reserve the gutter column to the left of the
                            // editor. The numbers themselves are painted after
                            // the editor lays out, so they can be aligned to
                            // the editor's real (possibly soft-wrapped) rows.
                            let gutter = Self::reserve_line_number_gutter(ui, &tab.text);
                            // Supply an explicit frame so egui does not draw
                            // its focus-stroke (the coloured ring it paints
                            // around a focused text edit from
                            // `visuals.selection.stroke`). A custom frame with
                            // no stroke keeps the editing surface borderless
                            // whether or not it has focus.
                            let editor_frame = egui::Frame::new()
                                .fill(ui.visuals().extreme_bg_color)
                                .inner_margin(egui::Margin::symmetric(4, 2));
                            let editor_id = egui::Id::new(("frext_editor", tab.id));
                            let mut output = egui::TextEdit::multiline(&mut tab.text)
                                .id(editor_id)
                                .code_editor()
                                .desired_width(f32::INFINITY)
                                .frame(editor_frame)
                                .layouter(&mut layouter)
                                .show(ui);

                            // Now that the editor has laid out, paint the line
                            // numbers aligned to its actual rows: a number only
                            // on rows that begin a new logical line, so soft-wrap
                            // continuation rows stay blank.
                            Self::paint_line_numbers(ui, &gutter, &output);

                            // Apply a pending scroll-to-match: select the match
                            // (byte range -> char range) so egui scrolls it
                            // into view on this frame.
                            if let Some(range) = &scroll_to {
                                let start = char_index(&tab.text, range.start);
                                let end = char_index(&tab.text, range.end);
                                let ccursor_range = egui::text::CCursorRange::two(
                                    egui::text::CCursor::new(start),
                                    egui::text::CCursor::new(end),
                                );
                                output.state.cursor.set_char_range(Some(ccursor_range));
                                output.state.clone().store(ui.ctx(), editor_id);
                                output.response.scroll_to_me(Some(egui::Align::Center));
                            }

                            // Capture the live selection as char and byte
                            // ranges: the byte range scopes a
                            // search-within-selection, the char range drives
                            // the cut/copy/paste menu (egui cursors are
                            // char-based).
                            if let Some(range) = output.cursor_range {
                                let lo = range.primary.index.0.min(range.secondary.index.0);
                                let hi = range.primary.index.0.max(range.secondary.index.0);
                                editor_char_selection = Some(lo..hi);
                                new_selection =
                                    Some(byte_index(&tab.text, lo)..byte_index(&tab.text, hi));
                            } else {
                                editor_char_selection = None;
                                new_selection = None;
                            }
                            editor_id_out = Some(editor_id);

                            // Right-click menu for the editing surface. egui's
                            // `TextEdit` ships no context menu of its own, so
                            // this adds one; choices are deferred like the menu
                            // bar's and applied after the borrow ends.
                            let has_selection = editor_char_selection
                                .as_ref()
                                .is_some_and(|r| r.start != r.end);
                            output.response.context_menu(|ui| {
                                Self::editor_context_menu(ui, has_selection, &mut editor_action);
                            });

                            output.response
                        })
                        .inner
                    })
                    .inner;

                changed = output.changed();
            }
        });

        // Apply a deferred editor context-menu action (cut/copy/paste/select
        // all/find/save). A clipboard edit that mutates the buffer is folded
        // into `changed` so it persists and refreshes matches just like a
        // typed edit.
        if !matches!(editor_action, EditorAction::None) {
            let edited =
                self.apply_editor_action(&ctx, editor_action, editor_id_out, editor_char_selection);
            changed = changed || edited;
        }

        if changed {
            self.persist_active_swap();
            // The buffer was edited this frame: refresh matches and the count
            // so the search bar tracks live edits, not just query changes.
            if self.search.open {
                self.recompute_matches();
            }
        }
        self.search.selection = new_selection;

        match action {
            MenuAction::None => {}
            MenuAction::NewTab => self.new_tab(),
            MenuAction::Open => self.open_file(),
            MenuAction::Save => self.save_active(),
            MenuAction::CloseActive => self.request_close_tab(self.active),
            MenuAction::Close(index) => self.request_close_tab(index),
            MenuAction::Select(index) => {
                self.active = index;
                self.persist_session();
                self.recompute_matches();
            }
        }

        match search_action {
            SearchAction::None => {}
            SearchAction::Open { replace } => self.open_search(replace),
            SearchAction::Close => {
                self.close_search();
                // Persist the (possibly edited) query when the bar closes.
                self.persist_session();
            }
            SearchAction::Step { forward } => self.step_match(forward),
            SearchAction::Recompute => self.recompute_matches(),
            SearchAction::ReplaceCurrent => self.replace_current(),
            SearchAction::ReplaceAll => self.replace_all(),
        }

        // Unsaved-changes confirmation modal, shown when a dirty tab's close
        // was requested. Resolved after rendering to keep the borrow simple.
        self.resolve_pending_close(&ctx);

        // Filesystem name-entry modal (rename / new file / new folder),
        // resolved after rendering for the same borrow-simplicity reason.
        self.resolve_pending_fs(&ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Final flush: make sure swap files and the index are current.
        for tab in &self.tabs {
            if let Err(err) = self.store.save_swap(tab) {
                log::error!("failed to flush swap for tab {} on exit: {err}", tab.id);
            }
        }
        self.persist_session();
    }
}

/// Open `path` in the platform file manager, selecting it where the manager
/// supports it. On Linux this opens the containing directory via `xdg-open`
/// (there is no portable "reveal and select" call); failures are logged, not
/// propagated, so a missing opener never crashes the editor.
fn reveal_in_file_manager(path: &Path) {
    // Reveal the containing directory so the manager opens somewhere useful
    // even for a file (which most managers would otherwise try to *run*).
    let target = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent()
            .map_or_else(|| path.to_path_buf(), Path::to_path_buf)
    };

    #[cfg(target_os = "linux")]
    let program = "xdg-open";
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(target_os = "windows")]
    let program = "explorer";

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    if let Err(err) = std::process::Command::new(program).arg(&target).spawn() {
        log::error!(
            "failed to reveal {} in file manager: {err}",
            target.display()
        );
    }
}

/// Number of editor lines in `text`.
///
/// At least one row, even for an empty buffer. A trailing newline opens a new
/// (empty) final line in the editor, so this is `newlines + 1`.
fn line_count(text: &str) -> usize {
    text.bytes().filter(|&b| b == b'\n').count() + 1
}

/// Decide which visual rows of a laid-out editor galley carry a line number.
///
/// `ends_with_newline` yields, in row order, each row's
/// [`egui::epaint::text::PlacedRow::ends_with_newline`] flag. A row begins a
/// new logical line — and therefore gets the next line number — when it is the
/// first row or the previous row ended with a real `\n`. Rows produced by soft
/// wrapping (whose predecessor did *not* end with a newline) are skipped, so
/// continuation rows stay blank.
///
/// Returns `(row_index, line_number)` pairs, with `line_number` starting at 1.
/// Splitting this out from the painting keeps the wrap-vs-newline logic — the
/// part that previously mis-numbered a continuation row right after Enter —
/// unit-testable without a live `egui::Ui`.
fn line_numbers_for_rows(ends_with_newline: impl IntoIterator<Item = bool>) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut starts_logical_line = true;
    let mut number = 0usize;
    for (row_index, row_ends_with_newline) in ends_with_newline.into_iter().enumerate() {
        if starts_logical_line {
            number += 1;
            out.push((row_index, number));
        }
        starts_logical_line = row_ends_with_newline;
    }
    out
}

/// A deferred action chosen from the menu/tab bar, applied after the borrow
/// of `self.tabs` ends.
enum MenuAction {
    None,
    NewTab,
    Open,
    Save,
    CloseActive,
    Close(usize),
    Select(usize),
}

/// The user's choice in the unsaved-changes confirmation modal, collected
/// while the modal is borrowed and applied afterwards.
enum CloseChoice {
    /// No button pressed yet this frame.
    Pending,
    /// Save the buffer, then close the tab.
    Save,
    /// Close the tab without saving.
    Discard,
    /// Keep the tab open.
    Cancel,
}

/// A deferred editor context-menu action, collected while the editor is
/// borrowed and applied after the borrow ends (mirroring [`MenuAction`]).
enum EditorAction {
    None,
    /// Copy the selection to the clipboard, then delete it from the buffer.
    Cut,
    /// Copy the selection to the clipboard.
    Copy,
    /// Replace the selection (or insert at the caret) with the clipboard text.
    Paste,
    /// Select the entire buffer.
    SelectAll,
    /// Open the find bar.
    Find,
    /// Open the find-and-replace bar.
    FindReplace,
    /// Save the active buffer.
    Save,
}

/// A deferred sidebar context-menu action, collected while the file tree is
/// borrowed and applied after the borrow ends (mirroring [`MenuAction`]).
enum TreeAction {
    /// Open `path` in a tab (reusing one already on that path).
    Open(PathBuf),
    /// Copy `path`'s absolute path to the clipboard.
    CopyPath(PathBuf),
    /// Copy `path`'s path relative to the workspace root to the clipboard.
    CopyRelativePath(PathBuf),
    /// Open `path` (or its containing directory) in the OS file manager.
    Reveal(PathBuf),
    /// Begin a rename of `path` (opens the name-entry modal).
    Rename(PathBuf),
    /// Begin creating a new file inside the directory `path`.
    NewFile(PathBuf),
    /// Begin creating a new folder inside the directory `path`.
    NewFolder(PathBuf),
    /// Move `path` to the OS trash.
    Trash(PathBuf),
}

/// A deferred find/replace action, applied after the editor borrow ends.
enum SearchAction {
    None,
    /// Open the find bar; `replace` also opens the replace row.
    Open {
        replace: bool,
    },
    /// Close the find bar.
    Close,
    /// Move to the next (`forward`) or previous match.
    Step {
        forward: bool,
    },
    /// Re-run the search (query or toggles changed).
    Recompute,
    /// Replace the focused match.
    ReplaceCurrent,
    /// Replace every match.
    ReplaceAll,
}

/// The byte offset of char index `char_idx` within `text` (clamped to the
/// end). Used to convert egui char-cursor positions to the byte ranges the
/// search model speaks.
fn byte_index(text: &str, char_idx: usize) -> usize {
    text.char_indices()
        .nth(char_idx)
        .map_or_else(|| text.len(), |(b, _)| b)
}

/// The char index of byte offset `byte` within `text` (clamped). The inverse
/// of [`byte_index`], for handing byte ranges back to egui's char cursors.
fn char_index(text: &str, byte: usize) -> usize {
    text.char_indices()
        .position(|(b, _)| b >= byte)
        .unwrap_or_else(|| text.chars().count())
}

/// Show a native "open file" picker. Returns `None` if cancelled or if no
/// picker backend is available.
fn rfd_pick_file() -> Option<std::path::PathBuf> {
    rfd::FileDialog::new().pick_file()
}

/// Show a native "save file" picker. Returns `None` if cancelled.
fn rfd_save_file() -> Option<std::path::PathBuf> {
    rfd::FileDialog::new().save_file()
}

#[cfg(test)]
mod tests {
    // `unwrap`/`expect` are acceptable in test code: a panic on an unexpected
    // `Err`/`None` is exactly the failure signal we want from a test.
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::{fs, path::PathBuf};

    use super::*;

    #[test]
    fn line_count_handles_empty_and_trailing_newlines() {
        assert_eq!(line_count(""), 1);
        assert_eq!(line_count("one line"), 1);
        assert_eq!(line_count("a\nb\nc"), 3);
        // A trailing newline opens a new empty final line.
        assert_eq!(line_count("a\nb\n"), 3);
    }

    #[test]
    fn line_numbers_unwrapped_rows_number_consecutively() {
        // Three logical lines, none wrapped: every row ends with a newline
        // except the last, and each row carries the next number.
        let rows = [true, true, false];
        assert_eq!(line_numbers_for_rows(rows), vec![(0, 1), (1, 2), (2, 3)],);
    }

    #[test]
    fn line_numbers_skip_soft_wrap_continuation_rows() {
        // Row 0 is a logical line that soft-wraps into row 1 (row 0 does NOT
        // end with a newline). Row 1 ends the logical line with a newline, so
        // row 2 starts the next logical line. The wrap-continuation row 1 must
        // be left blank.
        let rows = [false, true, false];
        assert_eq!(line_numbers_for_rows(rows), vec![(0, 1), (2, 2)]);
    }

    #[test]
    fn line_numbers_after_newline_do_not_mark_a_wrapped_row() {
        // Regression for the reported bug: a wrapped first logical line
        // (rows 0+1) followed by a freshly typed newline (row 1 ends with a
        // newline) and then a new empty line (row 2). The wrap-continuation
        // row 1 must stay blank; only rows 0 and 2 get numbers.
        let rows = [false, true, false];
        let numbered: Vec<usize> = line_numbers_for_rows(rows)
            .into_iter()
            .map(|(row, _)| row)
            .collect();
        assert!(
            !numbered.contains(&1),
            "the soft-wrap continuation row must not be numbered"
        );
        assert_eq!(numbered, vec![0, 2]);
    }

    #[test]
    fn line_numbers_empty_galley_yields_nothing() {
        assert_eq!(line_numbers_for_rows(std::iter::empty()), vec![]);
    }

    #[test]
    fn line_numbers_single_row() {
        assert_eq!(line_numbers_for_rows([false]), vec![(0, 1)]);
    }

    #[test]
    fn requesting_close_of_a_clean_tab_closes_immediately() {
        let dir = temp_dir("close-clean");
        let mut app = app_in(&dir);

        // Two clean tabs so closing one does not hit the "always keep one"
        // replacement path.
        app.new_tab();
        let before = app.tabs.len();
        assert!(!app.tabs[0].is_dirty());

        app.request_close_tab(0);

        assert!(app.pending_close.is_none(), "a clean close needs no prompt");
        assert_eq!(app.tabs.len(), before - 1);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn requesting_close_of_a_dirty_tab_prompts_instead_of_closing() {
        let dir = temp_dir("close-dirty");
        let mut app = app_in(&dir);
        app.new_tab();
        let before = app.tabs.len();

        // Make tab 0 dirty.
        app.tabs[0].text.push_str("unsaved work");
        assert!(app.tabs[0].is_dirty());

        app.request_close_tab(0);

        assert_eq!(
            app.pending_close,
            Some(0),
            "a dirty close must open the confirmation prompt"
        );
        assert_eq!(app.tabs.len(), before, "the tab must not be closed yet");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn saving_a_dirty_tab_clears_its_dirty_state() {
        let dir = temp_dir("save-clears-dirty");
        let file = dir.join("doc.txt");
        fs::write(&file, "original").unwrap();

        let mut app = app_in(&dir);
        assert!(app.open_path(&file));
        let index = app.active;

        app.tabs[index].text = "edited".to_owned();
        assert!(app.tabs[index].is_dirty());

        // save_tab is what "Save and close" calls; it must succeed and clean
        // the buffer, gating the subsequent close.
        assert!(app.save_tab(index));
        assert!(!app.tabs[index].is_dirty());
        assert_eq!(fs::read_to_string(&file).unwrap(), "edited");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn save_tab_on_out_of_range_index_is_a_noop_failure() {
        let dir = temp_dir("save-oob");
        let mut app = app_in(&dir);

        assert!(!app.save_tab(app.tabs.len() + 5));

        fs::remove_dir_all(&dir).unwrap();
    }

    /// Create an isolated temp directory unique to this test.
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "frext-app-test-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A fresh app backed by a store rooted in `dir/state`.
    fn app_in(dir: &std::path::Path) -> FrextApp {
        let store = Store::at(dir.join("state")).unwrap();
        FrextApp::new(store)
    }

    #[test]
    fn opening_cli_file_loads_its_contents_and_focuses_it() {
        let dir = temp_dir("cli-open");
        let file = dir.join("hello.txt");
        fs::write(&file, "hello from disk").unwrap();

        let store = Store::at(dir.join("state")).unwrap();
        let app = FrextApp::with_files(store, std::slice::from_ref(&file));

        let active = &app.tabs[app.active];
        assert_eq!(active.path.as_deref(), Some(file.as_path()));
        assert_eq!(active.text, "hello from disk");
        assert!(!active.is_dirty());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn opening_same_path_twice_reuses_the_tab() {
        let dir = temp_dir("reuse");
        let file = dir.join("notes.txt");
        fs::write(&file, "body").unwrap();

        let mut app = app_in(&dir);
        let baseline = app.tabs.len();

        assert!(app.open_path(&file));
        let after_first = app.tabs.len();
        assert_eq!(after_first, baseline + 1);
        let first_index = app.active;

        // Opening the same file again must not add a second tab.
        assert!(app.open_path(&file));
        assert_eq!(app.tabs.len(), after_first);
        assert_eq!(app.active, first_index);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn opening_nonexistent_file_reports_failure_and_adds_no_tab() {
        let dir = temp_dir("missing");
        let mut app = app_in(&dir);
        let before = app.tabs.len();

        assert!(!app.open_path(&dir.join("does-not-exist.txt")));
        assert_eq!(app.tabs.len(), before);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cli_files_persist_into_session_for_next_launch() {
        let dir = temp_dir("persist");
        let file = dir.join("restore-me.txt");
        fs::write(&file, "keep me").unwrap();

        {
            let store = Store::at(dir.join("state")).unwrap();
            let _app = FrextApp::with_files(store, std::slice::from_ref(&file));
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
    fn closing_the_workspace_drops_it_from_the_session() {
        let dir = temp_dir("ws-close");
        let project = dir.join("project");
        fs::create_dir_all(&project).unwrap();

        {
            let store = Store::at(dir.join("state")).unwrap();
            let mut app = FrextApp::with_args(store, &[], Some(project.as_path()));
            assert!(app.workspace.is_some());

            app.close_workspace();
            assert!(app.workspace.is_none());

            // Closing again is a harmless no-op.
            app.close_workspace();
            assert!(app.workspace.is_none());
        }

        // The closed workspace must not come back on the next launch.
        let store = Store::at(dir.join("state")).unwrap();
        let restored = store.load().unwrap();
        assert!(restored.workspace.is_none());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn directory_argument_opens_a_workspace_and_persists_it() {
        let dir = temp_dir("ws-arg");
        let project = dir.join("project");
        fs::create_dir_all(&project).unwrap();

        {
            let store = Store::at(dir.join("state")).unwrap();
            let app = FrextApp::with_args(store, &[], Some(project.as_path()));
            assert_eq!(
                app.workspace.as_ref().map(|ws| ws.root.clone()),
                Some(project.clone())
            );
        }

        // The workspace root must survive into the next launch.
        let store = Store::at(dir.join("state")).unwrap();
        let restored = store.load().unwrap();
        assert_eq!(restored.workspace.map(|ws| ws.root), Some(project.clone()));

        fs::remove_dir_all(&dir).unwrap();
    }

    /// Give `app` two file-backed tabs with distinct contents and focus the
    /// one holding `first`. (A fresh app always starts with an empty untitled
    /// tab, so the file tabs land after it.)
    fn app_with_two_tabs(dir: &std::path::Path, first: &str, second: &str) -> FrextApp {
        let a = dir.join("a.txt");
        let b = dir.join("b.txt");
        fs::write(&a, first).unwrap();
        fs::write(&b, second).unwrap();
        let store = Store::at(dir.join("state")).unwrap();
        let mut app = FrextApp::with_files(store, &[a, b]);
        // Focus the tab backed by a.txt (its content is `first`).
        app.active = app
            .tabs
            .iter()
            .position(|t| t.text == first)
            .expect("a tab holding the first content");
        app
    }

    #[test]
    fn search_targets_only_the_active_tab() {
        let dir = temp_dir("search-active");
        let mut app = app_with_two_tabs(&dir, "foo here foo", "foo and foo and foo");

        app.search.query.pattern = "foo".to_owned();
        app.recompute_matches();

        // The active tab ("foo here foo") has two "foo"s; the other tab's
        // three are never considered.
        assert_eq!(app.search.matches, vec![0..3, 9..12]);

        // Switching the active tab re-scopes the search to that buffer.
        app.active = app
            .tabs
            .iter()
            .position(|t| t.text == "foo and foo and foo")
            .unwrap();
        app.recompute_matches();
        assert_eq!(app.search.matches.len(), 3);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn stepping_matches_wraps_around() {
        let dir = temp_dir("search-step");
        let mut app = app_with_two_tabs(&dir, "x x x", "");
        app.search.query.pattern = "x".to_owned();
        app.recompute_matches();
        assert_eq!(app.search.current, Some(0));

        app.step_match(true);
        assert_eq!(app.search.current, Some(1));
        app.step_match(true);
        assert_eq!(app.search.current, Some(2));
        // Wrap forward to the first.
        app.step_match(true);
        assert_eq!(app.search.current, Some(0));
        // Wrap backward to the last.
        app.step_match(false);
        assert_eq!(app.search.current, Some(2));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn replace_all_only_changes_the_active_tab() {
        let dir = temp_dir("search-replace-all");
        // Distinct contents so we can tell the two file tabs apart.
        let mut app = app_with_two_tabs(&dir, "cat cat", "cat in tab two");
        let active = app.active;
        let other = app
            .tabs
            .iter()
            .position(|t| t.text == "cat in tab two")
            .unwrap();

        app.search.query.pattern = "cat".to_owned();
        app.search.replacement = "dog".to_owned();
        app.recompute_matches();
        app.replace_all();

        assert_eq!(app.tabs[active].text, "dog dog");
        // The inactive tab is untouched.
        assert_eq!(app.tabs[other].text, "cat in tab two");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn replace_current_replaces_one_match_and_advances() {
        let dir = temp_dir("search-replace-one");
        let mut app = app_with_two_tabs(&dir, "cat cat cat", "");
        app.search.query.pattern = "cat".to_owned();
        app.search.replacement = "dog".to_owned();
        app.recompute_matches();

        // Replace the first match only.
        app.search.current = Some(0);
        app.replace_current();
        let active = app.active;
        assert_eq!(app.tabs[active].text, "dog cat cat");
        // Two "cat" matches remain.
        assert_eq!(app.search.matches.len(), 2);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn regex_replace_expands_capture_groups() {
        let dir = temp_dir("search-regex-replace");
        let mut app = app_with_two_tabs(&dir, "user@host", "");
        app.search.query.pattern = r"(\w+)@(\w+)".to_owned();
        app.search.query.regex = true;
        app.search.replacement = "$2.$1".to_owned();
        app.recompute_matches();
        app.replace_all();

        let active = app.active;
        assert_eq!(app.tabs[active].text, "host.user");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn matches_refresh_after_the_buffer_is_edited() {
        let dir = temp_dir("search-live-edit");
        let mut app = app_with_two_tabs(&dir, "foo", "");
        app.search.open = true;
        app.search.query.pattern = "foo".to_owned();
        app.recompute_matches();
        assert_eq!(app.search.matches.len(), 1);

        // Simulate a buffer edit that adds two more occurrences, the way the
        // editor mutates the active tab in place.
        let active = app.active;
        app.tabs[active].text = "foo foo foo".to_owned();
        // The same recompute the edit path triggers.
        app.recompute_matches();

        assert_eq!(app.search.matches, vec![0..3, 4..7, 8..11]);

        // And removing all occurrences clears the count.
        app.tabs[active].text = "nothing here".to_owned();
        app.recompute_matches();
        assert!(app.search.matches.is_empty());
        assert_eq!(app.search.current, None);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn invalid_regex_sets_inline_error_and_no_matches() {
        let dir = temp_dir("search-bad-regex");
        let mut app = app_with_two_tabs(&dir, "anything", "");
        app.search.query.pattern = "(".to_owned();
        app.search.query.regex = true;
        app.recompute_matches();

        assert!(app.search.error.is_some());
        assert!(app.search.matches.is_empty());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn renaming_a_file_repoints_its_open_tab() {
        let dir = temp_dir("rename-tab");
        let file = dir.join("old.txt");
        fs::write(&file, "body").unwrap();

        let store = Store::at(dir.join("state")).unwrap();
        let mut app = FrextApp::with_files(store, std::slice::from_ref(&file));
        assert_eq!(app.tabs[app.active].path.as_deref(), Some(file.as_path()));

        app.rename_path(&file, "new.txt").unwrap();

        let renamed = dir.join("new.txt");
        assert!(renamed.is_file());
        assert!(!file.exists());
        // The open tab now points at the new path.
        assert_eq!(
            app.tabs[app.active].path.as_deref(),
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

        let store = Store::at(dir.join("state")).unwrap();
        let mut app = FrextApp::with_args(store, std::slice::from_ref(&file), Some(dir.as_path()));
        // Mark the sub-directory expanded so the rename must migrate it.
        if let Some(ws) = app.workspace.as_mut() {
            ws.set_expanded(&sub, true);
        }

        app.rename_path(&sub, "lib").unwrap();

        let new_sub = dir.join("lib");
        let new_file = new_sub.join("main.rs");
        assert!(new_file.is_file());
        // The tab under the renamed directory was repointed.
        assert_eq!(
            app.tabs[app.active].path.as_deref(),
            Some(new_file.as_path())
        );
        // The expanded-folder entry migrated to the new path.
        let ws = app.workspace.as_ref().unwrap();
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

        let mut app = app_in(&dir);
        let err = app.rename_path(&a, "b.txt").unwrap_err();
        assert!(matches!(err, crate::error::FsError::AlreadyExists(_)));
        // Both files remain.
        assert!(a.exists() && b.exists());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn close_tabs_under_closes_a_directorys_open_files() {
        let dir = temp_dir("close-under");
        let sub = dir.join("pkg");
        fs::create_dir(&sub).unwrap();
        let inside = sub.join("a.txt");
        let outside = dir.join("b.txt");
        fs::write(&inside, "a").unwrap();
        fs::write(&outside, "b").unwrap();

        let store = Store::at(dir.join("state")).unwrap();
        let mut app = FrextApp::with_files(store, &[inside.clone(), outside.clone()]);
        // Both files are open (alongside the initial untitled tab).
        let opened: Vec<_> = app.tabs.iter().filter_map(|t| t.path.clone()).collect();
        assert!(opened.contains(&inside) && opened.contains(&outside));

        // Simulate the bookkeeping half of trashing the directory.
        app.close_tabs_under(&sub);

        // The tab inside the directory is gone; the outside one survives.
        let remaining: Vec<_> = app.tabs.iter().filter_map(|t| t.path.clone()).collect();
        assert!(remaining.contains(&outside));
        assert!(!remaining.contains(&inside));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn relative_to_root_strips_the_workspace_root() {
        let dir = temp_dir("relpath");
        let nested = dir.join("a").join("b.txt");

        let store = Store::at(dir.join("state")).unwrap();
        let app = FrextApp::with_args(store, &[], Some(dir.as_path()));

        assert_eq!(
            app.relative_to_root(&nested),
            PathBuf::from("a/b.txt").display().to_string()
        );
        // A path outside the root falls back to its full display.
        let outside = PathBuf::from("/etc/hosts");
        assert_eq!(
            app.relative_to_root(&outside),
            outside.display().to_string()
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cut_deletes_the_selection_from_the_buffer() {
        let dir = temp_dir("editor-cut");
        let mut app = app_in(&dir);
        app.tabs[app.active].text = "hello world".to_owned();

        // Cut "hello " (chars 0..6). The clipboard write may be unavailable in
        // a headless test environment, but the buffer deletion is independent
        // of that and must happen.
        let ctx = egui::Context::default();
        let changed = app.apply_editor_action(&ctx, EditorAction::Cut, None, Some(0..6));

        assert!(changed);
        assert_eq!(app.tabs[app.active].text, "world");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn copy_does_not_mutate_the_buffer() {
        let dir = temp_dir("editor-copy");
        let mut app = app_in(&dir);
        app.tabs[app.active].text = "keep me".to_owned();

        let ctx = egui::Context::default();
        let changed = app.apply_editor_action(&ctx, EditorAction::Copy, None, Some(0..4));

        assert!(!changed);
        assert_eq!(app.tabs[app.active].text, "keep me");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn find_action_opens_the_search_bar() {
        let dir = temp_dir("editor-find");
        let mut app = app_in(&dir);
        assert!(!app.search.open);

        let ctx = egui::Context::default();
        let changed = app.apply_editor_action(&ctx, EditorAction::Find, None, None);

        assert!(!changed);
        assert!(app.search.open);
        assert!(!app.search.replace_open);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn find_replace_action_opens_the_replace_row() {
        let dir = temp_dir("editor-replace");
        let mut app = app_in(&dir);

        let ctx = egui::Context::default();
        app.apply_editor_action(&ctx, EditorAction::FindReplace, None, None);

        assert!(app.search.open);
        assert!(app.search.replace_open);

        fs::remove_dir_all(&dir).unwrap();
    }
}
