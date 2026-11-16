//! The frext eframe application: tab bar, editor surface, and the wiring
//! that keeps swap files and the session index up to date.

use std::ops::Range;
use std::path::{Path, PathBuf};

use eframe::egui;

use frext_core::{
    clipboard::SystemClipboard,
    editor::{
        BufferChange, CloseChoice, Command, Editor, EditorFocus, Effect, FolderState, NameKind,
        Prompt, SearchBar, SearchOption, StepDirection,
    },
    persistence::Store,
    proofread::{Diagnostic, DiagnosticKind, IgnoreKey},
    tab::Tab,
    workspace::Workspace,
};

/// The top-level application state.
pub struct FrextApp {
    /// The editor controller: tabs, session, workspace, prompts,
    /// proofreading and clipboard.
    editor: Editor,
    /// The diagnostic under the pointer when the editor was last
    /// right-clicked; drives the suggestion entries of the context menu.
    menu_diagnostic: Option<Diagnostic>,
    /// Effects returned by the editor, applied during the next frame.
    effects: Vec<Effect>,
}

/// This frame's view-state changes, drained from the queued [`Effect`]s
/// before anything renders.
#[derive(Default)]
struct FrameEffects {
    /// The last `SetSelection` queued, applied to the editor before it renders.
    selection: Option<(Range<usize>, EditorFocus)>,
    /// Scroll the editor so its selection is visible (`RevealSelection`).
    reveal: bool,
    /// Focus the find field (`FocusSearchField`).
    focus_search: bool,
    /// Focus the name prompt's field (`FocusPromptField`).
    focus_prompt: bool,
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

    /// Build the app like [`FrextApp::new`], but backed by `clipboard`
    /// instead of the system clipboard.
    #[cfg(test)]
    fn with_clipboard(store: Store, clipboard: Box<dyn frext_core::clipboard::Clipboard>) -> Self {
        Self::from_editor(Editor::open(store, clipboard, &[], None))
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
        Self::from_editor(Editor::open(store, Box::new(SystemClipboard), files, dir))
    }

    /// Wrap `editor` with fresh view state.
    fn from_editor(editor: Editor) -> Self {
        Self {
            editor,
            menu_diagnostic: None,
            effects: Vec::new(),
        }
    }

    /// Apply `command` to the editor, queueing its effects for the next
    /// frame. Returns whether the buffer changed.
    fn send(&mut self, command: Command) -> BufferChange {
        let outcome = self.editor.apply(command);
        self.effects.extend(outcome.effects);
        outcome.buffer
    }

    /// Drain the queued effects into this frame's view-state changes.
    fn apply_effects(&mut self) -> FrameEffects {
        let mut frame = FrameEffects::default();
        for effect in self.effects.drain(..) {
            match effect {
                Effect::SetSelection { chars, focus } => frame.selection = Some((chars, focus)),
                Effect::RevealSelection => frame.reveal = true,
                Effect::FocusSearchField => frame.focus_search = true,
                Effect::FocusPromptField => frame.focus_prompt = true,
            }
        }
        frame
    }

    /// Reserve the line-number gutter column to the left of the editor.
    ///
    /// This only allocates horizontal space (sized for the widest line number)
    /// and remembers where to paint; the numbers are drawn later by
    /// [`Self::paint_line_numbers`], aligned to the editor's real rows so
    /// soft-wrapped continuation rows are left blank.
    fn reserve_line_number_gutter(ui: &mut egui::Ui, text: &str) -> Gutter {
        let line_count = frext_core::lines::line_count(text);
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
        for (row_index, number) in frext_core::lines::line_numbers_for_rows(ends_with_newline) {
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

    /// Render and resolve the unsaved-changes confirmation modal.
    ///
    /// A no-op unless the editor is waiting on [`Prompt::ConfirmClose`].
    /// Otherwise it shows a modal with "Save and close", "Discard changes",
    /// and "Cancel". Clicking the backdrop or pressing Escape cancels
    /// (keeping the tab open). A "Save and close" that is itself cancelled
    /// (an untitled tab whose save-as dialog is dismissed, or a write error)
    /// leaves the tab open and dismisses the modal rather than silently
    /// discarding the buffer.
    fn resolve_close_prompt(&mut self, ctx: &egui::Context) {
        let Some(&Prompt::ConfirmClose { tab_id }) = self.editor.prompt() else {
            return;
        };

        // The tab may have been removed out from under a stale request.
        let Some(title) = self
            .editor
            .tabs()
            .iter()
            .find(|tab| tab.id == tab_id)
            .map(Tab::display_name)
        else {
            self.send(Command::CancelPrompt);
            return;
        };

        let mut choice: Option<CloseChoice> = None;
        let modal =
            egui::Modal::new(egui::Id::unique(("frext_confirm_close", tab_id))).show(ctx, |ui| {
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
                        choice = Some(CloseChoice::Save);
                    }
                    if ui.button("Discard changes").clicked() {
                        choice = Some(CloseChoice::Discard);
                    }
                    if ui.button("Cancel").clicked() {
                        choice = Some(CloseChoice::Cancel);
                    }
                });
            });

        // Clicking outside the dialog, or pressing Escape, cancels the close.
        if modal.should_close() {
            choice = Some(CloseChoice::Cancel);
        }

        if let Some(choice) = choice {
            self.send(Command::ResolveClose(choice));
        }
    }

    /// Answer a pending [`Prompt::SavePath`] straight away with the native
    /// save-file dialog: the chosen path is submitted, a dismissed dialog
    /// cancels the save.
    fn resolve_save_path_prompt(&mut self) {
        if !matches!(self.editor.prompt(), Some(Prompt::SavePath { .. })) {
            return;
        }
        let command = match rfd_save_file() {
            Some(path) => Command::SubmitSavePath(path),
            None => Command::CancelPrompt,
        };
        self.send(command);
    }

    /// Apply a deferred editor context-menu action after the editor borrow has
    /// ended.
    ///
    /// Clipboard text edits (cut/copy/paste) act on the editor's char
    /// selection over the active buffer, then queue the resulting caret as a
    /// [`Effect::SetSelection`] so the cursor follows the edit next frame.
    /// Find/replace and save reuse the existing deferred actions.
    ///
    /// Returns `true` when the buffer changed (so the caller can persist and
    /// refresh matches), matching how the editor's own `changed()` is handled.
    fn apply_editor_action(&mut self, action: EditorAction) -> bool {
        let selection = self.editor.selection().unwrap_or(0..0);

        match action {
            EditorAction::None => false,
            EditorAction::Copy => {
                if let Some(tab) = self.editor.active_tab() {
                    let text = frext_core::edit_ops::selected_text(
                        &tab.text,
                        selection.start,
                        selection.end,
                    );
                    if !text.is_empty() {
                        self.editor.clipboard_mut().write_text(text);
                    }
                }
                false
            }
            EditorAction::Cut => {
                let Some(tab) = self.editor.active_tab() else {
                    return false;
                };
                let text =
                    frext_core::edit_ops::selected_text(&tab.text, selection.start, selection.end);
                if text.is_empty() {
                    return false;
                }
                self.editor.clipboard_mut().write_text(text);
                let Some(view) = self.editor.active_view_mut() else {
                    return false;
                };
                let caret = frext_core::edit_ops::delete_range(
                    &mut view.tab.text,
                    selection.start,
                    selection.end,
                );
                self.focus_caret(caret);
                true
            }
            EditorAction::Paste => {
                let Some(clip) = self.editor.clipboard_mut().read_text() else {
                    return false;
                };
                if clip.is_empty() {
                    return false;
                }
                let Some(view) = self.editor.active_view_mut() else {
                    return false;
                };
                let caret = frext_core::edit_ops::replace_range(
                    &mut view.tab.text,
                    selection.start,
                    selection.end,
                    &clip,
                );
                self.focus_caret(caret);
                true
            }
            EditorAction::SelectAll => {
                if let Some(tab) = self.editor.active_tab() {
                    let len = tab.text.chars().count();
                    self.effects.push(Effect::SetSelection {
                        chars: 0..len,
                        focus: EditorFocus::Focus,
                    });
                }
                false
            }
            EditorAction::Find => {
                self.send(Command::OpenSearch(SearchBar::Find));
                false
            }
            EditorAction::FindReplace => {
                self.send(Command::OpenSearch(SearchBar::FindReplace));
                false
            }
            EditorAction::Save => {
                self.send(Command::Save);
                false
            }
            EditorAction::ApplySuggestion {
                bytes,
                flagged,
                replacement,
            } => {
                let Some(view) = self.editor.active_view_mut() else {
                    return false;
                };
                let tab = view.tab;
                // The buffer may have changed since the menu opened; only
                // replace text that is still exactly what was flagged.
                if tab.text.get(bytes.clone()) != Some(flagged.as_str()) {
                    return false;
                }
                let caret = tab.text.get(..bytes.start).map_or(0, |s| s.chars().count())
                    + replacement.chars().count();
                tab.text.replace_range(bytes, &replacement);
                self.focus_caret(caret);
                true
            }
            EditorAction::AddToDictionary(word) => {
                self.editor.add_to_dictionary(&word);
                false
            }
            EditorAction::Ignore(key) => {
                self.editor.proofread_mut().ignore(key);
                false
            }
        }
    }

    /// Queue `caret` (a character index) as a collapsed, focused editor
    /// selection, so the cursor tracks a programmatic edit.
    fn focus_caret(&mut self, caret: usize) {
        self.effects.push(Effect::SetSelection {
            chars: caret..caret,
            focus: EditorFocus::Focus,
        });
    }

    /// Render and resolve the filesystem name-entry modal (rename / new file /
    /// new folder). A no-op unless the editor is waiting on
    /// [`Prompt::Name`]. `focus_prompt` gives the name field keyboard focus.
    fn resolve_name_prompt(&mut self, ctx: &egui::Context, focus_prompt: bool) {
        let Some(Prompt::Name(prompt)) = self.editor.prompt() else {
            return;
        };

        let (title, hint, action_label) = match prompt.kind {
            NameKind::Rename => ("Rename", "New name", "Rename"),
            NameKind::NewFile => ("New file", "File name", "Create"),
            NameKind::NewFolder => ("New folder", "Folder name", "Create"),
        };
        let error = prompt.error.clone();

        let mut submit = false;
        let mut cancel = false;

        let modal = egui::Modal::new(egui::Id::unique("frext_fs_prompt")).show(ctx, |ui| {
            ui.set_min_width(320.0);
            ui.heading(title);
            ui.add_space(8.0);

            if let Some(name) = self.editor.prompt_name_mut() {
                let field = ui.add(
                    egui::TextEdit::singleline(name)
                        .hint_text(hint)
                        .desired_width(f32::INFINITY),
                );
                if focus_prompt {
                    field.request_focus();
                }
                if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    submit = true;
                }
            }

            if let Some(error) = &error {
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
            self.send(Command::CancelPrompt);
        } else if submit {
            // On failure the editor keeps the prompt open with the error.
            self.send(Command::SubmitName);
        }
    }

    /// Render the find / replace bar. Pushes the chosen search commands into
    /// `commands` in the order they occur; editing the pattern or flipping a
    /// toggle re-finds so highlights track typing live.
    fn search_bar_ui(
        &mut self,
        ui: &mut egui::Ui,
        commands: &mut Vec<Command>,
        focus_search: bool,
    ) {
        let panel_frame =
            egui::Frame::side_top_panel(ui.style()).fill(ui.style().visuals.panel_fill);

        egui::Panel::top("frext_search_bar")
            .frame(panel_frame)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Find");
                    let field = ui.add(
                        egui::TextEdit::singleline(self.editor.search_pattern_mut())
                            .desired_width(220.0)
                            .hint_text("search…"),
                    );
                    if focus_search {
                        field.request_focus();
                    }
                    if field.changed() {
                        commands.push(Command::RecomputeMatches);
                    }
                    // Enter / Shift+Enter step matches while the field is
                    // focused.
                    if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        let direction = if ui.input(|i| i.modifiers.shift) {
                            StepDirection::Backward
                        } else {
                            StepDirection::Forward
                        };
                        commands.push(Command::StepMatch(direction));
                        field.request_focus();
                    }

                    // Toggles.
                    let search = self.editor.search();
                    if ui
                        .selectable_label(search.query().case_sensitive, "Aa")
                        .on_hover_text("Match case")
                        .clicked()
                    {
                        commands.push(Command::ToggleSearchOption(SearchOption::CaseSensitive));
                    }
                    if ui
                        .selectable_label(search.query().whole_word, "W")
                        .on_hover_text("Whole word")
                        .clicked()
                    {
                        commands.push(Command::ToggleSearchOption(SearchOption::WholeWord));
                    }
                    if ui
                        .selectable_label(search.query().regex, ".*")
                        .on_hover_text("Regular expression")
                        .clicked()
                    {
                        commands.push(Command::ToggleSearchOption(SearchOption::Regex));
                    }
                    if ui
                        .selectable_label(search.in_selection(), "In sel")
                        .on_hover_text("Search within the current selection")
                        .clicked()
                    {
                        commands.push(Command::ToggleSearchOption(SearchOption::InSelection));
                    }

                    // Navigation + count. Plain-text labels because egui's
                    // default fonts (Ubuntu-Light + NotoEmoji) do not cover the
                    // geometric triangle glyphs, which render as tofu squares.
                    if ui
                        .button("Prev")
                        .on_hover_text("Previous match (Shift+F3)")
                        .clicked()
                    {
                        commands.push(Command::StepMatch(StepDirection::Backward));
                    }
                    if ui
                        .button("Next")
                        .on_hover_text("Next match (F3 / Enter)")
                        .clicked()
                    {
                        commands.push(Command::StepMatch(StepDirection::Forward));
                    }
                    ui.label(search.count_label());

                    if ui
                        .selectable_label(search.replace_open(), "Replace")
                        .on_hover_text("Toggle replace")
                        .clicked()
                    {
                        commands.push(Command::ToggleReplaceRow);
                    }
                    if ui.button("\u{00d7}").on_hover_text("Close (Esc)").clicked() {
                        commands.push(Command::CloseSearch);
                    }
                });

                if let Some(err) = self.editor.search().error() {
                    ui.colored_label(crate::theme::error(), format!("regex: {err}"));
                }

                if self.editor.search().replace_open() {
                    ui.horizontal(|ui| {
                        ui.label("With");
                        let hint = if self.editor.search().query().regex {
                            "replacement ($1 for groups)…"
                        } else {
                            "replacement…"
                        };
                        ui.add(
                            egui::TextEdit::singleline(self.editor.search_replacement_mut())
                                .desired_width(220.0)
                                .hint_text(hint),
                        );
                        if ui.button("Replace").clicked() {
                            commands.push(Command::ReplaceCurrent);
                        }
                        if ui.button("Replace all").clicked() {
                            commands.push(Command::ReplaceAll);
                        }
                    });
                }
            });
    }

    /// Show a native open-file dialog and load the chosen file into a tab.
    fn open_file(&mut self) {
        let Some(path) = rfd_pick_file() else {
            return;
        };

        self.send(Command::OpenPath(path));
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
        expand_changes: &mut Vec<Command>,
        tree_action: &mut Option<Command>,
    ) {
        let (dirs, files) = frext_core::workspace::read_dir_split(dir);

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
                    Self::file_icon(ui, frext_core::file_icon::icon_for_dir(&name, is_open));
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
                let state = if was_open {
                    FolderState::Collapsed
                } else {
                    FolderState::Expanded
                };
                expand_changes.push(Command::SetExpanded {
                    dir: sub.clone(),
                    state,
                });
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
                    Self::file_icon(ui, frext_core::file_icon::icon_for_file(&name));
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
    fn file_context_menu(ui: &mut egui::Ui, file: &Path, tree_action: &mut Option<Command>) {
        if ui.button("Open").clicked() {
            *tree_action = Some(Command::OpenPath(file.to_path_buf()));
            ui.close();
        }
        ui.separator();
        if ui.button("Copy path").clicked() {
            *tree_action = Some(Command::CopyPath(file.to_path_buf()));
            ui.close();
        }
        if ui.button("Copy relative path").clicked() {
            *tree_action = Some(Command::CopyRelativePath(file.to_path_buf()));
            ui.close();
        }
        if ui.button("Reveal in file manager").clicked() {
            *tree_action = Some(Command::Reveal(file.to_path_buf()));
            ui.close();
        }
        ui.separator();
        if ui.button("Rename\u{2026}").clicked() {
            *tree_action = Some(Command::BeginRename(file.to_path_buf()));
            ui.close();
        }
        if ui.button("Delete (move to trash)").clicked() {
            *tree_action = Some(Command::Trash(file.to_path_buf()));
            ui.close();
        }
    }

    /// The right-click menu for a folder header. Choices are recorded in
    /// `tree_action` and applied after the tree borrow ends.
    fn dir_context_menu(ui: &mut egui::Ui, dir: &Path, tree_action: &mut Option<Command>) {
        if ui.button("New file\u{2026}").clicked() {
            *tree_action = Some(Command::BeginNewFile(dir.to_path_buf()));
            ui.close();
        }
        if ui.button("New folder\u{2026}").clicked() {
            *tree_action = Some(Command::BeginNewFolder(dir.to_path_buf()));
            ui.close();
        }
        ui.separator();
        if ui.button("Copy path").clicked() {
            *tree_action = Some(Command::CopyPath(dir.to_path_buf()));
            ui.close();
        }
        if ui.button("Copy relative path").clicked() {
            *tree_action = Some(Command::CopyRelativePath(dir.to_path_buf()));
            ui.close();
        }
        if ui.button("Reveal in file manager").clicked() {
            *tree_action = Some(Command::Reveal(dir.to_path_buf()));
            ui.close();
        }
        ui.separator();
        if ui.button("Rename\u{2026}").clicked() {
            *tree_action = Some(Command::BeginRename(dir.to_path_buf()));
            ui.close();
        }
        if ui.button("Delete (move to trash)").clicked() {
            *tree_action = Some(Command::Trash(dir.to_path_buf()));
            ui.close();
        }
    }

    /// The right-click menu for the editor surface. `has_selection` greys out
    /// Cut/Copy when there is nothing selected. When the click landed on a
    /// proofreading `diagnostic`, its explanation, suggested fixes, and
    /// dictionary/ignore actions lead the menu. Choices are recorded in
    /// `editor_action` and applied after the editor borrow ends.
    fn editor_context_menu(
        ui: &mut egui::Ui,
        has_selection: bool,
        diagnostic: Option<&Diagnostic>,
        editor_action: &mut EditorAction,
    ) {
        if let Some(diagnostic) = diagnostic {
            Self::diagnostic_menu_items(ui, diagnostic, editor_action);
            ui.separator();
        }
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

    /// The proofreading entries of the editor context menu: Harper's message,
    /// one button per suggested fix, then "Add to dictionary" (spelling only)
    /// and "Ignore".
    fn diagnostic_menu_items(
        ui: &mut egui::Ui,
        diagnostic: &Diagnostic,
        editor_action: &mut EditorAction,
    ) {
        ui.add(egui::Label::new(egui::RichText::new(&diagnostic.message).weak()).wrap());
        if diagnostic.suggestions.is_empty() {
            ui.add_enabled(false, egui::Button::new("No suggestions"));
        }
        for suggestion in &diagnostic.suggestions {
            let label = if suggestion.is_empty() {
                format!("Remove \u{201c}{}\u{201d}", diagnostic.flagged)
            } else {
                suggestion.clone()
            };
            if ui.button(egui::RichText::new(label).strong()).clicked() {
                *editor_action = EditorAction::ApplySuggestion {
                    bytes: diagnostic.bytes.clone(),
                    flagged: diagnostic.flagged.clone(),
                    replacement: suggestion.clone(),
                };
                ui.close();
            }
        }
        ui.separator();
        if diagnostic.kind == DiagnosticKind::Spelling
            && ui
                .button(format!(
                    "Add \u{201c}{}\u{201d} to dictionary",
                    diagnostic.flagged
                ))
                .clicked()
        {
            *editor_action = EditorAction::AddToDictionary(diagnostic.flagged.clone());
            ui.close();
        }
        if ui.button("Ignore").clicked() {
            *editor_action = EditorAction::Ignore(diagnostic.ignore_key());
            ui.close();
        }
    }

    /// Draw the file-type icon for `stem` inline, sized to the current text
    /// row. A stem with no embedded artwork (which should not occur for stems
    /// produced by [`frext_core::file_icon`]) simply renders nothing, leaving the
    /// row icon-less rather than failing.
    fn file_icon(ui: &mut egui::Ui, stem: &'static str) {
        let size = ui.text_style_height(&egui::TextStyle::Body);
        if let Some(source) = image_source(stem) {
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
        // Effects queued by last frame's commands, applied while rendering.
        let effects = self.apply_effects();
        // A deferred tab-bar / shortcut command (the last one chosen wins),
        // and whether the open-file dialog was asked for.
        let mut action: Option<Command> = None;
        let mut open_dialog = false;

        // Proofreading runs on a background thread that repaints when a
        // result lands, so squiggles appear without further input.
        self.editor.proofread_mut().ensure_started({
            let ctx = ctx.clone();
            move || ctx.request_repaint()
        });
        self.editor.proofread_mut().poll();

        // Pick up external modifications to the active tab's file (detected
        // by a change in on-disk size). A clean buffer is reloaded; a dirty
        // one is left untouched. Keeping the swap file and session index in
        // step after a reload preserves crash-safety.
        if let Some(view) = self.editor.active_view_mut()
            && view.tab.reload_if_changed_on_disk()
        {
            self.editor.persist_active_swap();
            self.editor.persist_session();
        }

        // Keyboard shortcuts. Search commands are deferred like menu actions
        // so they run after the editor borrow ends, in the order they occur.
        let mut search_commands: Vec<Command> = Vec::new();
        ctx.input_mut(|i| {
            if i.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND,
                egui::Key::N,
            )) {
                action = Some(Command::NewTab);
                open_dialog = false;
            }
            if i.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND,
                egui::Key::O,
            )) {
                action = None;
                open_dialog = true;
            }
            if i.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND,
                egui::Key::S,
            )) {
                action = Some(Command::Save);
                open_dialog = false;
            }
            if i.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND,
                egui::Key::W,
            )) {
                action = Some(Command::RequestCloseActive);
                open_dialog = false;
            }
            if i.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND,
                egui::Key::F,
            )) {
                search_commands.push(Command::OpenSearch(SearchBar::Find));
            }
            if i.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND,
                egui::Key::H,
            )) {
                search_commands.push(Command::OpenSearch(SearchBar::FindReplace));
            }
            // While the bar is open, F3 / Shift+F3 cycle matches and Esc
            // closes it.
            if self.editor.search().is_open() {
                if i.consume_shortcut(&egui::KeyboardShortcut::new(
                    egui::Modifiers::NONE,
                    egui::Key::F3,
                )) {
                    search_commands.push(Command::StepMatch(StepDirection::Forward));
                }
                if i.consume_shortcut(&egui::KeyboardShortcut::new(
                    egui::Modifiers::SHIFT,
                    egui::Key::F3,
                )) {
                    search_commands.push(Command::StepMatch(StepDirection::Backward));
                }
                if i.key_pressed(egui::Key::Escape) {
                    search_commands.push(Command::CloseSearch);
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
                    action = Some(Command::NewTab);
                    open_dialog = false;
                }
                if ui
                    .button("Open")
                    .on_hover_text("Open file (Ctrl+O)")
                    .clicked()
                {
                    action = None;
                    open_dialog = true;
                }
                if ui.button("Save").on_hover_text("Save (Ctrl+S)").clicked() {
                    action = Some(Command::Save);
                    open_dialog = false;
                }
                ui.separator();

                for index in 0..self.editor.tabs().len() {
                    let selected = index == self.editor.active_index();
                    let title = self.editor.tabs()[index].title();
                    let label = if selected {
                        egui::RichText::new(title).color(crate::theme::accent())
                    } else {
                        egui::RichText::new(title)
                    };
                    if ui.selectable_label(selected, label).clicked() {
                        action = Some(Command::SelectTab(index));
                        open_dialog = false;
                    }
                    if ui
                        .small_button("\u{00d7}")
                        .on_hover_text("Close tab (Ctrl+W)")
                        .clicked()
                    {
                        action = Some(Command::RequestClose(index));
                        open_dialog = false;
                    }
                    ui.separator();
                }
            });
        });

        // Find / replace bar, shown directly under the tab bar.
        if self.editor.search().is_open() {
            self.search_bar_ui(ui, &mut search_commands, effects.focus_search);
        }

        // Left sidebar: file tree for the open workspace, if any.
        if self.editor.workspace().is_some() {
            let mut file_to_open: Option<PathBuf> = None;
            let mut expand_changes: Vec<Command> = Vec::new();
            let mut tree_action: Option<Command> = None;
            let mut close_workspace = false;

            // The active tab's file, so the tree can highlight it. Compared
            // canonicalised so a relative tree path matches an absolute tab
            // path (and vice versa).
            let active_path = self.editor.active_tab().and_then(|tab| tab.path.clone());
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
                    if let Some(ws) = self.editor.workspace() {
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
                self.send(Command::CloseWorkspace);
            } else {
                for command in expand_changes {
                    self.send(command);
                }
                if let Some(path) = file_to_open {
                    self.send(Command::OpenPath(path));
                }
                if let Some(command) = tree_action {
                    self.send(command);
                }
            }
        }

        let highlights = self.editor.match_highlights();
        let mut changed = false;
        // Editor context-menu plumbing: the chosen action plus the live char
        // selection it acts on, captured inside the closure and applied after
        // the borrow ends.
        let mut editor_action = EditorAction::None;
        let mut editor_char_selection: Option<Range<usize>> = None;

        egui::CentralPanel::default().show(ui, |ui| {
            if let Some(view) = self.editor.active_view_mut() {
                let tab = view.tab;
                let proofread = view.proofread;
                let language = frext_core::syntax::language_from_path(tab.path.as_deref());
                let mut layouter = crate::editor_layout::layouter(&language, &highlights);

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
                            let editor_id = egui::Id::unique(("frext_editor", tab.id));
                            // Apply a queued selection (and focus) before the
                            // editor reads its state this frame.
                            if let Some((chars, focus)) = &effects.selection {
                                let mut state =
                                    egui::text_edit::TextEditState::load(ui.ctx(), editor_id)
                                        .unwrap_or_default();
                                state
                                    .cursor
                                    .set_char_range(Some(egui::text::CCursorRange::two(
                                        egui::text::CCursor::new(chars.start),
                                        egui::text::CCursor::new(chars.end),
                                    )));
                                state.store(ui.ctx(), editor_id);
                                if *focus == EditorFocus::Focus {
                                    ui.ctx().memory_mut(|m| m.request_focus(editor_id));
                                }
                            }
                            let output = egui::TextEdit::multiline(&mut tab.text)
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

                            // Wavy underlines for spelling (red) and grammar
                            // (green) issues, painted over the laid-out text.
                            let underlines = proofread
                                .diagnostics(tab.id, &tab.text)
                                .into_iter()
                                .map(|d| {
                                    let color = match d.kind {
                                        DiagnosticKind::Spelling => {
                                            crate::theme::spelling_squiggle()
                                        }
                                        DiagnosticKind::Grammar => crate::theme::grammar_squiggle(),
                                    };
                                    (&d.chars, color)
                                });
                            crate::squiggle::paint(
                                ui.painter(),
                                &output.galley,
                                output.galley_pos,
                                underlines,
                            );

                            // Remember which diagnostic (if any) a right-click
                            // landed on, for the context menu's fixes.
                            if output.response.secondary_clicked() {
                                self.menu_diagnostic =
                                    output.response.interact_pointer_pos().and_then(|pos| {
                                        let cursor =
                                            output.galley.cursor_from_pos(pos - output.galley_pos);
                                        proofread
                                            .diagnostic_at(tab.id, &tab.text, cursor.index.0)
                                            .cloned()
                                    });
                            }

                            // A queued `RevealSelection`: scroll the editor's
                            // selection into view.
                            if effects.reveal {
                                output.response.scroll_to_me(Some(egui::Align::Center));
                            }

                            // Capture the live selection as a char range (egui
                            // cursors are char-based): it scopes a
                            // search-within-selection and drives the
                            // cut/copy/paste menu.
                            if let Some(range) = output.cursor_range {
                                let lo = range.primary.index.0.min(range.secondary.index.0);
                                let hi = range.primary.index.0.max(range.secondary.index.0);
                                editor_char_selection = Some(lo..hi);
                            } else {
                                editor_char_selection = None;
                            }

                            // Right-click menu for the editing surface. egui's
                            // `TextEdit` ships no context menu of its own, so
                            // this adds one; choices are deferred like the menu
                            // bar's and applied after the borrow ends.
                            let has_selection = editor_char_selection
                                .as_ref()
                                .is_some_and(|r| r.start != r.end);
                            let menu_diagnostic = self.menu_diagnostic.as_ref();
                            output.response.context_menu(|ui| {
                                Self::editor_context_menu(
                                    ui,
                                    has_selection,
                                    menu_diagnostic,
                                    &mut editor_action,
                                );
                            });

                            output.response
                        })
                        .inner
                    })
                    .inner;

                changed = output.changed();
            }
        });

        // Report the live char selection to the editor; the context-menu
        // actions below act on it.
        self.editor.set_selection(editor_char_selection);

        // Apply a deferred editor context-menu action (cut/copy/paste/select
        // all/find/save). A clipboard edit that mutates the buffer is folded
        // into `changed` so it persists and refreshes matches just like a
        // typed edit.
        if !matches!(editor_action, EditorAction::None) {
            let edited = self.apply_editor_action(editor_action);
            changed = changed || edited;
        }

        // The buffer was edited this frame: persist it and refresh matches so
        // the search bar tracks live edits, not just query changes.
        if changed {
            self.editor.commit_edit();
        }

        // Queue a proofread of the active buffer; a no-op unless its text
        // (or path) changed since the last request.
        self.editor.end_frame();

        if open_dialog {
            self.open_file();
        }
        if let Some(command) = action {
            self.send(command);
        }

        // The core persists and re-finds after a search edit itself, so the
        // returned buffer change needs no handling here.
        for command in search_commands {
            self.send(command);
        }

        // Unsaved-changes confirmation modal, shown when a dirty tab's close
        // was requested. Resolved after rendering to keep the borrow simple.
        self.resolve_close_prompt(&ctx);

        // An untitled save (Ctrl+S, or "Save and close" above) waits on a
        // path: ask for it straight away with the native dialog.
        self.resolve_save_path_prompt();

        // Filesystem name-entry modal (rename / new file / new folder),
        // resolved after rendering for the same borrow-simplicity reason.
        self.resolve_name_prompt(&ctx, effects.focus_prompt);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Final flush: make sure swap files and the index are current.
        self.editor.shutdown();
    }
}

/// An [`egui::ImageSource`] for the icon `stem`, ready to hand to
/// [`egui::Image::new`].
///
/// The `uri` is stable per stem so egui's image cache reuses a single rendered
/// texture across frames and rows. Returns `None` when the stem is not bundled.
#[must_use]
fn image_source(stem: &'static str) -> Option<egui::ImageSource<'static>> {
    let bytes = frext_core::file_icon::icon_bytes(stem)?;
    Some(egui::ImageSource::Bytes {
        // The `.svg` extension routes the bytes to egui_extras' SVG loader,
        // and the `frext-file-icon://` scheme keeps these URIs from colliding
        // with any other image source in the app.
        uri: stem_uri(stem).into(),
        bytes: egui::load::Bytes::Static(bytes),
    })
}

/// The cache URI for an icon `stem`. Pure and deterministic so it can be tested
/// without an egui context.
#[must_use]
fn stem_uri(stem: &str) -> String {
    format!("frext-file-icon://{stem}.svg")
}

/// A deferred editor context-menu action, collected while the editor is
/// borrowed and applied after the borrow ends.
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
    /// Replace the proofreading-flagged `bytes` (still holding `flagged`)
    /// with `replacement`.
    ApplySuggestion {
        bytes: Range<usize>,
        flagged: String,
        replacement: String,
    },
    /// Add a word to the personal dictionary.
    AddToDictionary(String),
    /// Hide matching diagnostics for the rest of the session.
    Ignore(IgnoreKey),
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
    use frext_core::clipboard::MemoryClipboard;

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
        FrextApp::with_clipboard(store, Box::new(MemoryClipboard::default()))
    }

    #[test]
    fn cut_deletes_the_selection_from_the_buffer() {
        let dir = temp_dir("editor-cut");
        let mut app = app_in(&dir);
        let active = app.editor.active_index();
        app.editor.tabs_mut()[active].text = "hello world".to_owned();

        // Cut "hello " (chars 0..6).
        app.editor.set_selection(Some(0..6));
        let changed = app.apply_editor_action(EditorAction::Cut);

        assert!(changed);
        assert_eq!(app.editor.active_tab().unwrap().text, "world");
        assert_eq!(
            app.editor.clipboard_mut().read_text().as_deref(),
            Some("hello ")
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn paste_replaces_the_selection_with_clipboard_text() {
        let dir = temp_dir("editor-paste");
        let mut app = app_in(&dir);
        let active = app.editor.active_index();
        app.editor.tabs_mut()[active].text = "hello world".to_owned();
        assert!(app.editor.clipboard_mut().write_text("there".to_owned()));

        // Paste over "world" (chars 6..11).
        app.editor.set_selection(Some(6..11));
        let changed = app.apply_editor_action(EditorAction::Paste);

        assert!(changed);
        assert_eq!(app.editor.active_tab().unwrap().text, "hello there");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn paste_with_an_empty_clipboard_is_a_noop() {
        let dir = temp_dir("editor-paste-empty");
        let mut app = app_in(&dir);
        let active = app.editor.active_index();
        app.editor.tabs_mut()[active].text = "unchanged".to_owned();

        app.editor.set_selection(Some(0..2));
        let changed = app.apply_editor_action(EditorAction::Paste);

        assert!(!changed);
        assert_eq!(app.editor.active_tab().unwrap().text, "unchanged");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn copy_does_not_mutate_the_buffer() {
        let dir = temp_dir("editor-copy");
        let mut app = app_in(&dir);
        let active = app.editor.active_index();
        app.editor.tabs_mut()[active].text = "keep me".to_owned();

        app.editor.set_selection(Some(0..4));
        let changed = app.apply_editor_action(EditorAction::Copy);

        assert!(!changed);
        assert_eq!(app.editor.active_tab().unwrap().text, "keep me");
        assert_eq!(
            app.editor.clipboard_mut().read_text().as_deref(),
            Some("keep")
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn find_action_opens_the_search_bar() {
        let dir = temp_dir("editor-find");
        let mut app = app_in(&dir);
        assert!(!app.editor.search().is_open());

        let changed = app.apply_editor_action(EditorAction::Find);

        assert!(!changed);
        assert!(app.editor.search().is_open());
        assert!(!app.editor.search().replace_open());

        fs::remove_dir_all(&dir).unwrap();
    }

    fn suggestion(bytes: Range<usize>, flagged: &str, replacement: &str) -> EditorAction {
        EditorAction::ApplySuggestion {
            bytes,
            flagged: flagged.to_owned(),
            replacement: replacement.to_owned(),
        }
    }

    #[test]
    fn applying_a_suggestion_replaces_the_flagged_text() {
        let dir = temp_dir("editor-suggest");
        let mut app = app_in(&dir);
        let active = app.editor.active_index();
        app.editor.tabs_mut()[active].text = "café wrrod here".to_owned();

        // "wrrod" sits after a multi-byte char: bytes 6..11.
        let changed = app.apply_editor_action(suggestion(6..11, "wrrod", "word"));

        assert!(changed);
        assert_eq!(app.editor.active_tab().unwrap().text, "café word here");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_stale_suggestion_leaves_the_buffer_alone() {
        let dir = temp_dir("editor-suggest-stale");
        let mut app = app_in(&dir);
        // The buffer was edited after the menu opened: the range no longer
        // holds the flagged word.
        let active = app.editor.active_index();
        app.editor.tabs_mut()[active].text = "a word here".to_owned();

        let changed = app.apply_editor_action(suggestion(2..7, "wrrod", "word"));

        assert!(!changed);
        assert_eq!(app.editor.active_tab().unwrap().text, "a word here");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_removal_suggestion_deletes_the_flagged_text() {
        let dir = temp_dir("editor-suggest-remove");
        let mut app = app_in(&dir);
        let active = app.editor.active_index();
        app.editor.tabs_mut()[active].text = "the the cat".to_owned();

        let changed = app.apply_editor_action(suggestion(0..4, "the ", ""));

        assert!(changed);
        assert_eq!(app.editor.active_tab().unwrap().text, "the cat");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn adding_to_the_dictionary_persists_across_launches() {
        let dir = temp_dir("editor-dictionary");
        let mut app = app_in(&dir);

        let changed = app.apply_editor_action(EditorAction::AddToDictionary("frext".to_owned()));

        assert!(!changed, "the buffer is untouched");
        assert_eq!(
            app.editor.proofread().user_words().collect::<Vec<_>>(),
            vec!["frext"]
        );

        // A fresh app over the same store loads the saved word.
        let reopened = app_in(&dir);
        assert_eq!(
            reopened.editor.proofread().user_words().collect::<Vec<_>>(),
            vec!["frext"]
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_unreadable_dictionary_is_never_overwritten() {
        let dir = temp_dir("editor-dictionary-unreadable");
        let state = dir.join("state");
        fs::create_dir_all(&state).unwrap();
        // Invalid UTF-8: `load_dictionary` fails rather than returning words.
        let original: &[u8] = b"kept\n\xff\xfe broken\n";
        fs::write(state.join("dictionary.txt"), original).unwrap();

        let mut app = app_in(&dir);
        app.apply_editor_action(EditorAction::AddToDictionary("frext".to_owned()));

        // The word works for this session...
        assert_eq!(
            app.editor.proofread().user_words().collect::<Vec<_>>(),
            vec!["frext"]
        );
        // ...but the unreadable file is left exactly as it was.
        assert_eq!(fs::read(state.join("dictionary.txt")).unwrap(), original);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn find_replace_action_opens_the_replace_row() {
        let dir = temp_dir("editor-replace");
        let mut app = app_in(&dir);

        app.apply_editor_action(EditorAction::FindReplace);

        assert!(app.editor.search().is_open());
        assert!(app.editor.search().replace_open());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn stem_uri_is_stable_and_svg_suffixed() {
        assert_eq!(stem_uri("rust"), "frext-file-icon://rust.svg");
    }
}
