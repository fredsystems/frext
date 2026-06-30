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
                    Self::tree_dir(ui, ws, &sub, active_canonical, file_to_open, expand_changes);
                });

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

            let clicked = ui
                .horizontal(|ui| {
                    Self::file_icon(ui, crate::file_icon::icon_for_file(&name));
                    ui.selectable_label(is_active, name).clicked()
                })
                .inner;
            if clicked {
                *file_to_open = Some(file);
            }
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

        let highlights = self.match_highlights();
        // A scroll-to-match request becomes a cursor selection on the editor;
        // converted from byte to char indices below.
        let scroll_to = self.search.scroll_to.take();
        let mut new_selection: Option<Range<usize>> = None;
        let mut changed = false;

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

                            // Capture the live selection (char -> byte) so a
                            // search-within-selection can scope to it.
                            new_selection = output.cursor_range.map(|range| {
                                let lo = range.primary.index.0.min(range.secondary.index.0);
                                let hi = range.primary.index.0.max(range.secondary.index.0);
                                byte_index(&tab.text, lo)..byte_index(&tab.text, hi)
                            });

                            output.response
                        })
                        .inner
                    })
                    .inner;

                changed = output.changed();
            }
        });

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
}
