//! The frext eframe application: tab bar, editor surface, and the wiring
//! that keeps swap files and the session index up to date.

use std::path::{Path, PathBuf};

use eframe::egui;

use crate::{persistence::Store, tab::Tab, workspace::Workspace};

/// The top-level application state.
pub struct FrextApp {
    store: Store,
    tabs: Vec<Tab>,
    active: usize,
    next_id: u64,
    /// The sidebar workspace (root directory + expanded folders), if open.
    workspace: Option<Workspace>,
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

    /// Draw the line-number gutter for `text` to the left of the editor.
    ///
    /// Numbers are right-aligned, dimmed, and rendered in the same monospace
    /// font the editor uses, so each number sits on the same baseline as its
    /// line. The editor never wraps (it uses `desired_width(INFINITY)`), so a
    /// straight `1..=line_count` sequence aligns row-for-row.
    fn line_number_gutter(ui: &mut egui::Ui, text: &str) {
        use egui::text::{LayoutJob, TextFormat};

        let line_count = line_count(text);

        let font_id = egui::TextStyle::Monospace.resolve(ui.style());
        let color = crate::theme::gutter();

        // Right-align by padding each number to the width of the largest one.
        let width = line_count.to_string().len();
        let mut job = LayoutJob::default();
        for n in 1..=line_count {
            let line = if n == line_count {
                format!("{n:>width$}")
            } else {
                format!("{n:>width$}\n")
            };
            job.append(&line, 0.0, TextFormat::simple(font_id.clone(), color));
        }
        let galley = ui.fonts_mut(|f| f.layout_job(job));

        // `TextEdit::multiline` insets its text by `Margin::symmetric(4, 2)`,
        // so the first line starts 2px below the widget top. Mirror that top
        // inset so row 1 of the gutter lines up with line 1 of the editor.
        const TEXT_EDIT_MARGIN_TOP: f32 = 2.0;
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(
                galley.size().x,
                galley.size().y + 2.0 * TEXT_EDIT_MARGIN_TOP,
            ),
            egui::Sense::hover(),
        );
        let text_pos = rect.left_top() + egui::vec2(0.0, TEXT_EDIT_MARGIN_TOP);
        ui.painter().galley(text_pos, galley, color);

        // A small gap between the gutter and the editor text.
        ui.add_space(4.0);
    }

    /// Open a new, empty, untitled tab and focus it.
    fn new_tab(&mut self) {
        let id = self.alloc_id();
        self.tabs.push(Tab::new_untitled(id));
        self.active = self.tabs.len() - 1;
        self.persist_active_swap();
        self.persist_session();
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
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };

        let path = match tab.path.clone() {
            Some(path) => path,
            None => match rfd_save_file() {
                Some(path) => path,
                None => return,
            },
        };

        if let Err(err) = std::fs::write(&path, &tab.text) {
            log::error!("failed to save {}: {err}", path.display());
            return;
        }

        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.path = Some(path);
            tab.saved_text = tab.text.clone();
        }
        self.persist_session();
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
    ) {
        let (dirs, files) = crate::workspace::read_dir_split(dir);

        for sub in dirs {
            let name = sub
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            let was_open = ws.is_expanded(&sub);

            // `default_open` seeds the header's state from the persisted
            // expanded set on first sight; thereafter egui owns the live
            // open/closed state and a header click toggles it. We mirror
            // each toggle back into the workspace via `expand_changes`.
            let response = egui::CollapsingHeader::new(name)
                .id_salt(&sub)
                .default_open(was_open)
                .show(ui, |ui| {
                    Self::tree_dir(ui, ws, &sub, active_canonical, file_to_open, expand_changes);
                });

            if response.header_response.clicked() {
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

            if ui.selectable_label(is_active, name).clicked() {
                *file_to_open = Some(file);
            }
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

        // Keyboard shortcuts.
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

        // Left sidebar: file tree for the open workspace, if any.
        if self.workspace.is_some() {
            let mut file_to_open: Option<PathBuf> = None;
            let mut expand_changes: Vec<(PathBuf, bool)> = Vec::new();
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
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            Self::tree_dir(
                                ui,
                                ws,
                                &root,
                                active_canonical.as_deref(),
                                &mut file_to_open,
                                &mut expand_changes,
                            );
                        });
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
            }
        }

        egui::CentralPanel::default().show(ui, |ui| {
            if let Some(tab) = self.tabs.get_mut(self.active) {
                let language = crate::highlight::language_from_path(tab.path.as_deref());
                let mut layouter = crate::highlight::layouter(&language);

                // The editor must be wrapped in a ScrollArea: a bare
                // `add_sized(available_size, ...)` clamps the TextEdit to the
                // viewport, so content past the bottom is clipped and cannot
                // be scrolled to. Letting the TextEdit grow to its natural
                // height inside a scroll area is what makes scrolling work.
                //
                // A line-number gutter is laid out to the left of the editor,
                // inside the same scroll area so it scrolls in lockstep. The
                // editor uses `desired_width(INFINITY)`, so lines never wrap
                // and a simple `1..=N` gutter stays aligned row-for-row.
                let response = egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.horizontal_top(|ui| {
                            Self::line_number_gutter(ui, &tab.text);
                            // Supply an explicit frame so egui does not draw
                            // its focus-stroke (the coloured ring it paints
                            // around a focused text edit from
                            // `visuals.selection.stroke`). A custom frame with
                            // no stroke keeps the editing surface borderless
                            // whether or not it has focus.
                            let editor_frame = egui::Frame::new()
                                .fill(ui.visuals().extreme_bg_color)
                                .inner_margin(egui::Margin::symmetric(4, 2));
                            ui.add(
                                egui::TextEdit::multiline(&mut tab.text)
                                    .code_editor()
                                    .desired_width(f32::INFINITY)
                                    .frame(editor_frame)
                                    .layouter(&mut layouter),
                            )
                        })
                        .inner
                    })
                    .inner;

                if response.changed() {
                    self.persist_active_swap();
                }
            }
        });

        match action {
            MenuAction::None => {}
            MenuAction::NewTab => self.new_tab(),
            MenuAction::Open => self.open_file(),
            MenuAction::Save => self.save_active(),
            MenuAction::CloseActive => self.close_tab(self.active),
            MenuAction::Close(index) => self.close_tab(index),
            MenuAction::Select(index) => {
                self.active = index;
                self.persist_session();
            }
        }
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

/// Number of editor lines in `text`.
///
/// At least one row, even for an empty buffer. A trailing newline opens a new
/// (empty) final line in the editor, so this is `newlines + 1`.
fn line_count(text: &str) -> usize {
    text.bytes().filter(|&b| b == b'\n').count() + 1
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
    // `unwrap` is acceptable in test code: a panic on an unexpected `Err`
    // is exactly the failure signal we want from a test.
    #![allow(clippy::unwrap_used)]

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
}
