//! [`TermApp`]: the terminal front end's state, event routing and
//! rendering.
//!
//! `TermApp` turns [`AppEvent`]s into [`Command`]s for the core
//! [`Editor`] (or into local view changes), queues the [`Effect`]s the
//! editor returns, and draws the whole screen into a ratatui [`Frame`]:
//! the tab bar, the find bar, the sidebar (the workspace's file tree,
//! see [`FileTree`]), the editor, the status line, and any open menu or
//! prompt. While the sidebar has focus, keys the keymap leaves unbound go
//! to the tree.

use std::path::PathBuf;
use std::time::Instant;

use crossterm::event::{
    Event, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use frext_core::editor::{
    BufferChange, Command, Editor, EditorFocus, Effect, FolderState, SearchBar, StepDirection,
};
use frext_core::lines::line_count;
use frext_core::proofread::Diagnostic;
use frext_core::proofread::state::diagnostic_in;
use frext_core::syntax::language_from_path;
use frext_core::workspace::RowKind;
use ratatui::Frame;
use ratatui::layout::Position;

use crate::buffer::cursor::Selecting;
use crate::buffer::edit::{Applied, EditAction};
use crate::buffer::{BufferView, Overlays, WHEEL_ROWS};
use crate::editor_menu;
use crate::events::AppEvent;
use crate::find_bar::{self, FindBar, FindField, FindReply};
use crate::hit::{HitMap, Target};
use crate::icons::IconProvider;
use crate::keymap::{self, Action, Focus, KeyContext};
use crate::layout;
use crate::menu::{ContextMenu, Enablement, MenuAnchor, MenuReply};
use crate::prompt::{PromptReply, PromptUi};
use crate::status::{MessageKind, StatusLine};
use crate::tab_bar;
use crate::terminal::KeyboardMode;
use crate::tree::{FileTree, Refresh, TreeFocus, TreeReply};
use crate::tree_menu;

/// Shown on startup when the kitty keyboard protocol is unavailable.
const LEGACY_MESSAGE: &str = "Kitty keyboard protocol unavailable: use Ctrl+R for find and replace";

/// Whether the user wants the sidebar shown (when there is a workspace).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SidebarVisibility {
    Shown,
    Hidden,
}

/// How the open menu was placed. A `Caret` menu is re-anchored to the
/// caret on every render, so a layout change between opening it and the
/// next draw (the find bar closing, say) cannot leave it stale; a
/// `Pointer` menu stays where it opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuOrigin {
    /// At a fixed cell: a right-click, or the tree's own keyboard menu.
    Pointer,
    /// From the keyboard at the editor's caret.
    Caret,
}

/// Whether the user asked to quit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Quit {
    Running,
    Requested,
}

/// The terminal front end: the core editor plus all view state.
pub struct TermApp {
    editor: Editor,
    keyboard: KeyboardMode,
    focus: Focus,
    sidebar: SidebarVisibility,
    prompt: PromptUi,
    menu: Option<ContextMenu<Command>>,
    menu_origin: MenuOrigin,
    find: FindBar,
    tree: FileTree,
    buffer: BufferView,
    status: StatusLine,
    hovered: Option<Target>,
    hits: HitMap,
    effects: Vec<Effect>,
    quit: Quit,
}

impl TermApp {
    /// A front end over `editor`, resolving relative paths in prompts
    /// against `base` and `~` against `home`.
    ///
    /// In [`KeyboardMode::Legacy`] the status line starts with a note
    /// that Ctrl+R is the find-and-replace key.
    #[must_use]
    pub fn new(
        editor: Editor,
        keyboard: KeyboardMode,
        base: PathBuf,
        home: Option<PathBuf>,
    ) -> Self {
        let mut status = StatusLine::new();
        if keyboard == KeyboardMode::Legacy {
            status.set_message(LEGACY_MESSAGE, MessageKind::Info, Instant::now());
        }
        Self {
            editor,
            keyboard,
            focus: Focus::Editor,
            sidebar: SidebarVisibility::Shown,
            prompt: PromptUi::new(base, home),
            menu: None,
            menu_origin: MenuOrigin::Pointer,
            find: FindBar::new(),
            tree: FileTree::new(),
            buffer: BufferView::default(),
            status,
            hovered: None,
            hits: HitMap::default(),
            effects: Vec::new(),
            quit: Quit::Running,
        }
    }

    /// The core editor.
    #[must_use]
    pub const fn editor(&self) -> &Editor {
        &self.editor
    }

    /// Whether the user asked to quit (or input closed).
    #[must_use]
    pub fn should_quit(&self) -> bool {
        self.quit == Quit::Requested
    }

    /// Start of a frame: passes through to [`Editor::begin_frame`].
    pub fn begin_frame(&mut self, notify: impl Fn() + Send + 'static) {
        self.editor.begin_frame(notify);
    }

    /// End of a frame: passes through to [`Editor::end_frame`].
    pub fn end_frame(&mut self) {
        self.editor.end_frame();
    }

    /// Final flush before exit: passes through to [`Editor::shutdown`].
    pub fn shutdown(&self) {
        self.editor.shutdown();
    }

    /// Whether the sidebar column is drawn.
    fn sidebar_shown(&self) -> bool {
        self.editor.workspace().is_some() && self.sidebar == SidebarVisibility::Shown
    }

    /// Moves focus off a pane that is not shown: the sidebar or the find
    /// bar back to the editor, and the "With" field back to the find
    /// field when the replace row is hidden.
    fn settle_focus(&mut self) {
        if self.focus == Focus::Sidebar && !self.sidebar_shown() {
            self.set_focus(Focus::Editor);
        }
        let search = self.editor.search();
        if matches!(self.focus, Focus::Find(_)) && !search.is_open() {
            self.set_focus(Focus::Editor);
        } else if self.focus == Focus::Find(FindField::Replacement) && !search.replace_open() {
            self.set_focus(Focus::Find(FindField::Pattern));
        }
    }

    /// Moves focus off a find field the last render did not draw: a
    /// terminal too short for the whole find bar cuts its rows. The
    /// "With" field falls back to the find field if that is drawn, else
    /// to the editor; an undrawn find field falls back to the editor. A
    /// closed search is left to [`Self::settle_focus`].
    fn settle_find_focus(&mut self) {
        if !self.editor.search().is_open() {
            return;
        }
        if self.focus == Focus::Find(FindField::Replacement)
            && !self.find.is_drawn(FindField::Replacement)
        {
            if self.find.is_drawn(FindField::Pattern) {
                self.set_focus(Focus::Find(FindField::Pattern));
            } else {
                self.set_focus(Focus::Editor);
            }
        } else if self.focus == Focus::Find(FindField::Pattern)
            && !self.find.is_drawn(FindField::Pattern)
        {
            self.set_focus(Focus::Editor);
        }
    }

    /// Moves focus to `focus`. An actual change closes the active tab's
    /// typing group, so typing after it starts a new undo step.
    fn set_focus(&mut self, focus: Focus) {
        if focus == self.focus {
            return;
        }
        if let Some(tab) = self.editor.active_tab() {
            self.buffer.break_group(tab.id);
        }
        self.focus = focus;
    }

    /// Routes one event, then applies the effects it queued, so the next
    /// event in the same batch sees them, and settles the focus again.
    pub fn handle(&mut self, event: AppEvent, now: Instant) {
        self.prompt.sync(&self.editor);
        self.settle_focus();
        match event {
            AppEvent::Input(Event::Key(key)) => self.handle_key(key, now),
            AppEvent::Input(Event::Mouse(mouse)) => self.handle_mouse(mouse, now),
            AppEvent::Input(Event::Paste(text)) => {
                if self.menu.is_some() {
                    // The open menu is modal: a paste is dropped.
                } else if self.prompt.is_open(&self.editor) {
                    self.prompt.handle_paste(&mut self.editor, &text);
                } else if let Focus::Find(field) = self.focus {
                    if let FindReply::Send(command) =
                        self.find.handle_paste(&mut self.editor, field, &text)
                    {
                        self.send(command, now);
                    }
                } else if self.focus == Focus::Editor
                    && let Some(view) = self.editor.active_view_mut()
                {
                    let applied = self.buffer.insert_text(view.tab, &text, now);
                    self.after_edit(applied);
                }
            }
            AppEvent::Input(Event::Resize(..)) => self.forget_layout(),
            AppEvent::Input(Event::FocusGained | Event::FocusLost) | AppEvent::Wake => {}
            AppEvent::Tick => {
                self.status.expire(now);
            }
            AppEvent::InputClosed => self.quit = Quit::Requested,
        }
        self.apply_effects();
        self.settle_focus();
    }

    /// Persists an edit the buffer view made to the active tab.
    fn after_edit(&mut self, applied: Applied) {
        if applied == Applied::Edited {
            // The caret places the edit for the search-within-selection
            // scope, so core needs it before the commit.
            let selection = self
                .editor
                .active_tab()
                .and_then(|tab| self.buffer.selection(tab.id));
            self.editor.set_selection(selection);
            self.editor.commit_edit();
            // The dirty marker changes the tab's width in the tab bar.
            self.forget_layout();
        }
    }

    /// Applies `action` to the active tab through the buffer view.
    fn edit(&mut self, action: EditAction, now: Instant) {
        if let Some(view) = self.editor.active_view_mut() {
            let applied = self.buffer.handle(view.tab, action, now);
            self.after_edit(applied);
        }
    }

    fn handle_key(&mut self, key: KeyEvent, now: Instant) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        if let Some(menu) = self.menu.as_mut() {
            let reply = menu.handle_key(key);
            self.menu_reply(reply, now);
            return;
        }
        if self.prompt.is_open(&self.editor) {
            match self.prompt.handle_key(&mut self.editor, key) {
                PromptReply::Send(command) => self.send(command, now),
                PromptReply::Handled | PromptReply::Ignored => {}
            }
            return;
        }
        let ctx = KeyContext {
            keyboard: self.keyboard,
            search_open: self.editor.search().is_open(),
            focus: self.focus,
        };
        let Some(action) = keymap::action_for(key, ctx) else {
            if let Focus::Find(field) = self.focus {
                match self.find.handle_key(&mut self.editor, field, key) {
                    FindReply::Send(command) => self.send(command, now),
                    FindReply::Focus(field) => {
                        self.find.focus_field(&self.editor, field);
                        self.set_focus(Focus::Find(field));
                    }
                    FindReply::Handled | FindReply::Ignored => {}
                }
            } else if self.focus == Focus::Sidebar {
                self.tree_key(key, now);
            }
            return;
        };
        match action {
            // From the bar, Ctrl+F / Ctrl+H only switch the replace row and
            // refocus the find field: reopening would re-seed the pattern
            // from the editor selection (such as the match just stepped to).
            Action::Command(Command::OpenSearch(bar)) if matches!(self.focus, Focus::Find(_)) => {
                let want_replace = bar == SearchBar::FindReplace;
                if want_replace != self.editor.search().replace_open() {
                    self.send(Command::ToggleReplaceRow, now);
                }
                self.find.focus_field(&self.editor, FindField::Pattern);
                self.set_focus(Focus::Find(FindField::Pattern));
            }
            Action::Command(command) => self.send(command, now),
            Action::OpenFilePrompt => self.open_file_prompt(),
            Action::Quit => self.quit = Quit::Requested,
            Action::ToggleSidebar => {
                self.sidebar = match self.sidebar {
                    SidebarVisibility::Shown => SidebarVisibility::Hidden,
                    SidebarVisibility::Hidden => SidebarVisibility::Shown,
                };
                self.settle_focus();
                self.forget_layout();
            }
            Action::ToggleFocus => match self.focus {
                Focus::Editor => {
                    if self.sidebar_shown() {
                        self.set_focus(Focus::Sidebar);
                    }
                }
                Focus::Sidebar | Focus::Find(_) => self.set_focus(Focus::Editor),
            },
            Action::FocusEditor => self.set_focus(Focus::Editor),
            Action::Edit(action) => self.edit(action, now),
            Action::OpenEditorMenu => self.open_editor_menu_at_caret(),
        }
    }

    /// Opens the editor menu from the keyboard: with the diagnostic at
    /// the caret, below the caret's cell, or at the editor's top-left
    /// cell when the caret is not on screen. That placement comes from
    /// the last render's layout; the next render re-anchors the menu to
    /// the caret as drawn then ([`MenuOrigin::Caret`]).
    fn open_editor_menu_at_caret(&mut self) {
        let Some(tab) = self.editor.active_tab() else {
            return;
        };
        let diagnostic = self.buffer.caret_span(tab).and_then(|span| {
            let diagnostics = self.editor.proofread().diagnostics(tab.id, &tab.text);
            diagnostic_in(&diagnostics, span).cloned()
        });
        let anchor = self.buffer.caret_cell(tab).map_or_else(
            || {
                let rect = self.buffer.rect();
                MenuAnchor::At(Position::new(rect.x, rect.y))
            },
            MenuAnchor::Below,
        );
        self.open_editor_menu(diagnostic, anchor, MenuOrigin::Caret);
    }

    /// Routes a key the keymap left unbound to the file tree, over rows
    /// refreshed first so they include what earlier keys in the same
    /// batch changed.
    fn tree_key(&mut self, key: KeyEvent, now: Instant) {
        if self.tree.refresh(self.editor.workspace()) == Refresh::Changed {
            self.forget_layout();
        }
        match self.tree.handle_key(key) {
            TreeReply::Send(command) => self.send(command, now),
            TreeReply::OpenMenu { path, kind, anchor } => {
                // The tree places its menu from its own rows: it stays put.
                self.open_menu(
                    ContextMenu::new(tree_menu::entries(&path, kind), anchor),
                    MenuOrigin::Pointer,
                );
            }
            TreeReply::Resized => self.forget_layout(),
            TreeReply::Handled | TreeReply::Ignored => {}
        }
    }

    /// Acts on the open menu's reply: a choice closes it and is sent, a
    /// dismissal closes it. Either way the hit map, which still holds the
    /// menu's rows, is dropped until the next render.
    fn menu_reply(&mut self, reply: MenuReply<Command>, now: Instant) {
        match reply {
            MenuReply::Chosen(command) => {
                self.menu = None;
                self.send(command, now);
            }
            MenuReply::Dismissed => self.forget_layout(),
            MenuReply::Handled => {}
        }
    }

    /// Records the mouse's hover target and shows its hint. While a
    /// prompt is open only its own targets count: what lies under it
    /// hovers nothing.
    fn hover(&mut self, target: Option<Target>) {
        let target = target.filter(|target| {
            !self.prompt.is_open(&self.editor)
                || matches!(
                    target,
                    Target::PromptPopup
                        | Target::PromptInput
                        | Target::PromptButton(_)
                        | Target::PromptCandidate(_)
                )
        });
        self.hovered = target;
        self.status.set_hint(target.and_then(Target::hint));
    }

    /// Routes a mouse event while the editor menu is open: it captures
    /// clicks (one outside dismisses it without acting on what is under
    /// it), and the wheel does nothing.
    fn handle_menu_mouse(&mut self, mouse: MouseEvent, target: Option<Target>, now: Instant) {
        match mouse.kind {
            MouseEventKind::Moved | MouseEventKind::Drag(_) => {
                if mouse.kind == MouseEventKind::Moved {
                    // Sent only with no button held: a release was lost.
                    self.tree.end_resize();
                }
                self.hover(target);
                if let Some(menu) = self.menu.as_mut() {
                    menu.hover(match target {
                        Some(Target::MenuItem(index)) => Some(index),
                        _ => None,
                    });
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.buffer.mouse_up();
                self.tree.end_resize();
            }
            MouseEventKind::Down(MouseButton::Left | MouseButton::Right) => {
                if let Some(menu) = self.menu.as_mut() {
                    let reply = menu.click(target);
                    self.menu_reply(reply, now);
                }
            }
            _ => {}
        }
    }

    fn handle_mouse(&mut self, mouse: MouseEvent, now: Instant) {
        let target = self.hits.hit(mouse.column, mouse.row);
        if self.menu.is_some() {
            self.handle_menu_mouse(mouse, target, now);
            return;
        }
        match mouse.kind {
            MouseEventKind::Moved | MouseEventKind::Drag(_) => {
                if mouse.kind == MouseEventKind::Moved {
                    // Sent only with no button held: a release was lost.
                    self.tree.end_resize();
                }
                self.hover(target);
                if mouse.kind == MouseEventKind::Drag(MouseButton::Left)
                    && self.buffer.is_dragging()
                    && !self.prompt.is_open(&self.editor)
                    && let Some(tab) = self.editor.active_tab()
                {
                    self.buffer.mouse_drag(tab, mouse.column, mouse.row);
                }
                if mouse.kind == MouseEventKind::Drag(MouseButton::Left) && self.tree.is_resizing()
                {
                    if self.prompt.is_open(&self.editor) {
                        // A prompt opened mid-drag: the border stays put.
                        self.tree.end_resize();
                    } else {
                        self.tree.drag_to(mouse.column);
                        self.forget_layout();
                    }
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.buffer.mouse_up();
                self.tree.end_resize();
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if target != Some(Target::TreeBorder) {
                    // A press elsewhere means the drag's release was lost.
                    self.tree.end_resize();
                }
                if self.prompt.is_open(&self.editor) {
                    let reply = self
                        .prompt
                        .handle_click(&mut self.editor, target, mouse.column);
                    if let PromptReply::Send(command) = reply {
                        self.send(command, now);
                    }
                    return;
                }
                match target {
                    Some(Target::NewTab) => self.send(Command::NewTab, now),
                    Some(Target::Save) => self.send(Command::Save, now),
                    Some(Target::SelectTab(index)) => self.send(Command::SelectTab(index), now),
                    Some(Target::CloseTab(index)) => {
                        self.send(Command::RequestClose(index), now);
                    }
                    Some(Target::OpenFile) => self.open_file_prompt(),
                    Some(Target::Editor) => {
                        if let Some(tab) = self.editor.active_tab() {
                            let selecting = if mouse.modifiers.contains(KeyModifiers::SHIFT) {
                                Selecting::Extend
                            } else {
                                Selecting::Move
                            };
                            self.buffer
                                .mouse_down(tab, mouse.column, mouse.row, selecting, now);
                        }
                        self.set_focus(Focus::Editor);
                    }
                    Some(Target::FindPattern) => {
                        self.find
                            .handle_click(&self.editor, FindField::Pattern, mouse.column);
                        self.set_focus(Focus::Find(FindField::Pattern));
                    }
                    Some(Target::FindReplacement) => {
                        self.find
                            .handle_click(&self.editor, FindField::Replacement, mouse.column);
                        self.set_focus(Focus::Find(FindField::Replacement));
                    }
                    Some(Target::FindOption(option)) => {
                        self.send(Command::ToggleSearchOption(option), now);
                    }
                    Some(Target::FindPrev) => {
                        self.send(Command::StepMatch(StepDirection::Backward), now);
                    }
                    Some(Target::FindNext) => {
                        self.send(Command::StepMatch(StepDirection::Forward), now);
                    }
                    Some(Target::FindReplaceRow) => self.send(Command::ToggleReplaceRow, now),
                    Some(Target::FindClose) => self.send(Command::CloseSearch, now),
                    Some(Target::FindReplaceCurrent) => self.send(Command::ReplaceCurrent, now),
                    Some(Target::FindReplaceAll) => self.send(Command::ReplaceAll, now),
                    Some(Target::TreeClose) => self.send(Command::CloseWorkspace, now),
                    Some(Target::TreeRow(index)) => self.click_tree_row(index, now),
                    Some(Target::TreeEmpty) => self.set_focus(Focus::Sidebar),
                    Some(Target::TreeBorder) => self.tree.begin_resize(),
                    Some(
                        Target::TreeScrolled
                        | Target::PromptPopup
                        | Target::PromptInput
                        | Target::PromptButton(_)
                        | Target::PromptCandidate(_)
                        | Target::MenuPopup
                        | Target::MenuItem(_),
                    )
                    | None => {}
                }
            }
            MouseEventKind::Down(MouseButton::Right) => {
                if self.prompt.is_open(&self.editor) {
                    return;
                }
                let at = Position::new(mouse.column, mouse.row);
                match target {
                    Some(Target::Editor) => {
                        // The caret stays put: the cell only picks the
                        // diagnostic the menu shows.
                        let diagnostic = self.editor.active_tab().and_then(|tab| {
                            let chars =
                                self.buffer.grapheme_at_cell(tab, mouse.column, mouse.row)?;
                            let diagnostics =
                                self.editor.proofread().diagnostics(tab.id, &tab.text);
                            diagnostic_in(&diagnostics, chars).cloned()
                        });
                        self.open_editor_menu(diagnostic, MenuAnchor::At(at), MenuOrigin::Pointer);
                    }
                    Some(Target::TreeRow(index)) => {
                        if let Some(row) = self.tree.row(index).cloned() {
                            self.tree.select(index);
                            self.open_menu(
                                ContextMenu::new(tree_menu::entries(&row.path, row.kind), at),
                                MenuOrigin::Pointer,
                            );
                            self.set_focus(Focus::Sidebar);
                        }
                    }
                    Some(Target::TreeEmpty) => {
                        if let Some(workspace) = self.editor.workspace() {
                            self.open_menu(
                                ContextMenu::new(tree_menu::dir_entries(&workspace.root), at),
                                MenuOrigin::Pointer,
                            );
                            self.set_focus(Focus::Sidebar);
                        }
                    }
                    _ => {}
                }
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                if self.prompt.is_open(&self.editor) {
                    return;
                }
                let rows = if mouse.kind == MouseEventKind::ScrollUp {
                    -WHEEL_ROWS
                } else {
                    WHEEL_ROWS
                };
                match target {
                    Some(Target::Editor) => {
                        if let Some(tab) = self.editor.active_tab() {
                            self.buffer.wheel(tab, rows);
                        }
                    }
                    Some(
                        Target::TreeRow(_)
                        | Target::TreeEmpty
                        | Target::TreeBorder
                        | Target::TreeScrolled,
                    ) => {
                        self.tree.wheel(rows);
                        // The rows moved under their regions: later wheel
                        // events in the batch still scroll, clicks do not.
                        self.hits.retarget(|target| match target {
                            Target::TreeRow(_) | Target::TreeEmpty => Target::TreeScrolled,
                            other => other,
                        });
                        if matches!(self.hovered, Some(Target::TreeRow(_))) {
                            self.hovered = None;
                            self.status.set_hint(None);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// Opens `menu`, recording how it was placed.
    fn open_menu(&mut self, menu: ContextMenu<Command>, origin: MenuOrigin) {
        self.menu = Some(menu);
        self.menu_origin = origin;
    }

    /// Opens the editor menu placed by `anchor` from `origin`, leading
    /// with `diagnostic`'s items when there is one, its cut and copy
    /// enabled only over a selection.
    fn open_editor_menu(
        &mut self,
        diagnostic: Option<Diagnostic>,
        anchor: MenuAnchor,
        origin: MenuOrigin,
    ) {
        if let Some(tab) = self.editor.active_tab() {
            let selection = if self
                .buffer
                .selection(tab.id)
                .is_some_and(|chars| !chars.is_empty())
            {
                Enablement::Enabled
            } else {
                Enablement::Disabled
            };
            let entries = editor_menu::entries(selection, diagnostic.as_ref());
            self.open_menu(ContextMenu::anchored(entries, anchor), origin);
            // The chosen command acts on the editor's selection.
            self.set_focus(Focus::Editor);
        }
    }

    /// A left click on tree row `index` (as last rendered): selects it,
    /// then opens a file and focuses the editor, or toggles a folder and
    /// focuses the sidebar.
    fn click_tree_row(&mut self, index: usize, now: Instant) {
        let Some(row) = self.tree.row(index).cloned() else {
            return;
        };
        self.tree.select(index);
        match row.kind {
            RowKind::File => {
                self.send(Command::OpenPath(row.path), now);
                self.set_focus(Focus::Editor);
            }
            RowKind::Dir(state) => {
                let state = match state {
                    FolderState::Expanded => FolderState::Collapsed,
                    FolderState::Collapsed => FolderState::Expanded,
                };
                self.send(
                    Command::SetExpanded {
                        dir: row.path,
                        state,
                    },
                    now,
                );
                self.set_focus(Focus::Sidebar);
            }
        }
    }

    /// Applies `command` and queues the effects it returns.
    ///
    /// The editor's selection is passed to core first, whatever has
    /// focus, as the GUI keeps reporting the editor's cursor range while
    /// the editor is unfocused. A text-editing command is recorded in
    /// the active tab's undo history as one group, redoing to the
    /// selection the command sets for that tab, if any.
    fn send(&mut self, command: Command, now: Instant) {
        let selection = self
            .editor
            .active_tab()
            .and_then(|tab| self.buffer.selection(tab.id));
        self.editor.set_selection(selection);
        let edits_text = matches!(
            command,
            Command::Cut
                | Command::Paste
                | Command::ApplySuggestion { .. }
                | Command::ReplaceCurrent
                | Command::ReplaceAll
        );
        let before = self
            .editor
            .active_tab()
            .filter(|_| edits_text)
            .map(|tab| (tab.id, tab.text.clone()));
        if let Some(tab) = self.editor.active_tab() {
            self.buffer.break_group(tab.id);
        }
        let outcome = self.editor.apply(command);
        if outcome.buffer == BufferChange::Edited
            && let Some((id, before)) = before
            && let Some(tab) = self.editor.active_tab().filter(|tab| tab.id == id)
        {
            let after = outcome.effects.iter().find_map(|effect| match effect {
                Effect::SetSelection { tab_id, chars, .. } if *tab_id == id => Some(chars.clone()),
                _ => None,
            });
            self.buffer.record_outside_edit(tab, &before, after, now);
        }
        self.effects.extend(outcome.effects);
        self.prompt.sync(&self.editor);
        self.forget_layout();
    }

    /// Opens the open-file prompt over the current layout.
    fn open_file_prompt(&mut self) {
        self.prompt.open_file(&self.editor);
        self.forget_layout();
    }

    /// Drops the hit map and hover target, which describe the last
    /// rendered layout. Called whenever the layout may have changed
    /// before the next render, so a queued click cannot hit a target
    /// that has moved (such as a tab index shifted by a close); until
    /// the next render, mouse events hit nothing. The hover hint goes
    /// with the hover target. The editor menu, which is anchored to that
    /// layout, closes.
    fn forget_layout(&mut self) {
        self.hits.clear();
        self.hovered = None;
        self.status.set_hint(None);
        self.menu = None;
    }

    /// Carries out the queued effects, then drops the view state of tabs
    /// that no longer exist.
    pub fn apply_effects(&mut self) {
        for effect in std::mem::take(&mut self.effects) {
            match effect {
                Effect::SetSelection {
                    tab_id,
                    chars,
                    focus,
                } => {
                    if let Some(tab) = self.editor.tabs().iter().find(|tab| tab.id == tab_id) {
                        self.buffer.set_selection(tab, chars);
                    }
                    if focus == EditorFocus::Focus {
                        self.set_focus(Focus::Editor);
                    }
                }
                Effect::RevealSelection { tab_id } => {
                    if let Some(tab) = self.editor.active_tab().filter(|tab| tab.id == tab_id) {
                        self.buffer.reveal(tab);
                    }
                }
                Effect::FocusPromptField => self.prompt.focus_field(&self.editor),
                Effect::FocusSearchField => {
                    if self.editor.search().is_open() {
                        self.find.focus_field(&self.editor, FindField::Pattern);
                        self.set_focus(Focus::Find(FindField::Pattern));
                    }
                }
            }
        }
        self.buffer
            .forget(self.editor.tabs().iter().map(|tab| tab.id));
    }

    /// Uses `icons` for the sidebar's file-tree icons.
    pub fn set_icons(&mut self, icons: Box<dyn IconProvider>) {
        self.tree.set_icons(icons);
    }

    /// The graphics bytes (icon uploads) queued since the last call, to
    /// be written to the terminal before the frame that shows them.
    pub fn take_graphics(&mut self) -> Vec<u8> {
        self.tree.take_uploads()
    }

    /// The graphics bytes that delete every uploaded icon image, to be
    /// written to the terminal before it is restored on exit.
    pub fn release_graphics(&mut self) -> Vec<u8> {
        self.tree.release_icons()
    }

    /// Draws the whole screen into `frame`, rebuilding the hit map.
    pub fn render(&mut self, frame: &mut Frame) {
        if let Some(position) = self.render_frame(frame) {
            frame.set_cursor_position(position);
        }
    }

    /// Draws the whole screen into `frame`, rebuilding the hit map, and
    /// returns where the terminal cursor belongs (`None` hides it).
    ///
    /// [`TermApp::render`] sets this position on the frame; a caller that
    /// drives the terminal itself passes it on, since a `Frame`'s cursor
    /// cannot be read back.
    #[must_use]
    pub fn render_frame(&mut self, frame: &mut Frame) -> Option<Position> {
        self.hits.clear();
        let area = frame.area();
        let gutter_width = self
            .editor
            .active_tab()
            .map_or(0, |tab| layout::gutter_width(line_count(&tab.text)));
        self.tree.set_space(area.width.saturating_sub(gutter_width));
        let sidebar_width = self.sidebar_shown().then(|| self.tree.width());
        let areas = layout::areas(
            area,
            find_bar::height(self.editor.search()),
            sidebar_width,
            gutter_width,
        );

        let active = self.editor.active_index();
        let items = tab_bar::items(self.editor.tabs(), active, areas.tab_bar.width);
        let buf = frame.buffer_mut();
        tab_bar::render(
            &items,
            active,
            self.hovered,
            areas.tab_bar,
            buf,
            &mut self.hits,
        );
        let find_cursor = self.find.render(
            &self.editor,
            self.focus.find_field(),
            self.hovered,
            areas.find_bar,
            buf,
            &mut self.hits,
        );
        self.settle_find_focus();
        let tab = self.editor.active_tab();
        if self.sidebar_shown() {
            let focus = if self.focus == Focus::Sidebar {
                TreeFocus::Focused
            } else {
                TreeFocus::Unfocused
            };
            let refresh = self.tree.render(
                &self.editor,
                focus,
                self.hovered,
                areas.sidebar,
                buf,
                &mut self.hits,
            );
            // A hovered row index refers to the rows before the change.
            if refresh == Refresh::Changed && matches!(self.hovered, Some(Target::TreeRow(_))) {
                self.hovered = None;
                self.status.set_hint(None);
            }
        }
        let proofread = self.editor.proofread();
        let diagnostics = tab.map_or_else(Vec::new, |tab| proofread.diagnostics(tab.id, &tab.text));
        let caret = self.buffer.render(
            tab,
            areas.gutter,
            areas.editor,
            Overlays {
                matches: self.editor.match_highlights_ref(),
                diagnostics: &diagnostics,
            },
            buf,
            &mut self.hits,
        );
        // A keyboard menu follows the caret as this frame drew it: the
        // layout may have moved since the menu was opened.
        if self.menu_origin == MenuOrigin::Caret
            && let Some(menu) = self.menu.as_mut()
        {
            menu.set_anchor(caret.map_or(
                MenuAnchor::At(Position::new(areas.editor.x, areas.editor.y)),
                MenuAnchor::Below,
            ));
        }

        let (left, right) = tab.map_or_else(
            || (String::new(), String::new()),
            |tab| {
                // The caret as the view holds it after rendering; the
                // message only describes it while the editor has focus.
                let message = self
                    .buffer
                    .caret_span(tab)
                    .filter(|_| self.focus == Focus::Editor)
                    .and_then(|span| diagnostic_in(&diagnostics, span))
                    .and_then(|diagnostic| status_text(&diagnostic.message));
                let left = message.unwrap_or_else(|| {
                    tab.path.as_deref().map_or_else(
                        || tab.display_name(),
                        |path| self.editor.relative_to_root(path),
                    )
                });
                let mut language = language_from_path(tab.path.as_deref());
                if language.is_empty() {
                    "plain text".clone_into(&mut language);
                }
                let right = format!(
                    "{language} \u{b7} {}",
                    line_count_label(line_count(&tab.text))
                );
                (left, right)
            },
        );
        self.status.render(areas.status, buf, &left, &right);
        if let Some(menu) = &self.menu {
            menu.render(area, buf, &mut self.hits);
        }

        let prompt_cursor = self.prompt.render(&self.editor, area, buf, &mut self.hits);
        let unobscured = self.menu.is_none() && !self.prompt.is_open(&self.editor);
        let editor_cursor = caret.filter(|_| self.focus == Focus::Editor && unobscured);
        let find_cursor = find_cursor.filter(|_| unobscured);
        prompt_cursor.or(editor_cursor).or(find_cursor)
    }
}

/// A diagnostic `message` as one status line: its words joined by single
/// spaces, or `None` when it has none.
fn status_text(message: &str) -> Option<String> {
    let line = message.split_whitespace().collect::<Vec<_>>().join(" ");
    (!line.is_empty()).then_some(line)
}

/// `"1 line"` for a single line, `"{n} lines"` otherwise.
fn line_count_label(n: usize) -> String {
    if n == 1 {
        "1 line".to_owned()
    } else {
        format!("{n} lines")
    }
}

#[cfg(test)]
mod tests {
    // Tests unwrap freely: a panic is a test failure.
    #![allow(clippy::unwrap_used)]

    use std::fs;
    use std::path::Path;
    use std::time::Duration;

    use crossterm::event::{KeyCode, KeyModifiers};
    use frext_core::clipboard::MemoryClipboard;
    use frext_core::editor::{Prompt, SearchOption};
    use frext_core::highlight::MatchBackground;
    use frext_core::palette::BASE;
    use frext_core::persistence::Store;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    use super::*;
    use crate::theme;
    use crate::tree::{DEFAULT_WIDTH, MIN_EDITOR_WIDTH, MIN_WIDTH};

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "frext-term-app-test-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn open_editor(dir: &Path, files: &[PathBuf], workspace: Option<&Path>) -> Editor {
        Editor::open(
            Store::at(dir.join("state")).unwrap(),
            Box::new(MemoryClipboard::default()),
            files,
            workspace,
        )
    }

    fn app(dir: &Path, files: &[PathBuf], keyboard: KeyboardMode) -> TermApp {
        TermApp::new(
            open_editor(dir, files, None),
            keyboard,
            dir.to_path_buf(),
            None,
        )
    }

    fn draw(app: &mut TermApp) -> Buffer {
        draw_sized(app, 80, 24)
    }

    /// Draws `app` on a `width` x `height` terminal.
    fn draw_sized(app: &mut TermApp, width: u16, height: u16) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();
        terminal.backend().buffer().clone()
    }

    fn row_text(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
    }

    /// The column of `needle` in a row of single-width cells.
    fn column_of(row: &str, needle: &str) -> u16 {
        let byte = row.find(needle).unwrap();
        u16::try_from(row[..byte].chars().count()).unwrap()
    }

    fn key(code: KeyCode, modifiers: KeyModifiers) -> AppEvent {
        AppEvent::Input(Event::Key(KeyEvent::new(code, modifiers)))
    }

    fn ctrl(c: char) -> AppEvent {
        key(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> AppEvent {
        AppEvent::Input(Event::Mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }))
    }

    #[test]
    fn first_frame_shows_tab_bar_gutter_and_status() {
        let dir = temp_dir("first-frame");
        let ws = dir.join("ws");
        fs::create_dir_all(ws.join("src")).unwrap();
        let file = ws.join("src").join("notes.txt");
        fs::write(&file, "one\ntwo\n").unwrap();
        let editor = open_editor(&dir, std::slice::from_ref(&file), Some(&ws));
        let mut app = TermApp::new(editor, KeyboardMode::Enhanced, ws.clone(), None);
        // Hide the sidebar so the gutter starts at column 0.
        app.handle(ctrl('b'), Instant::now());
        let buf = draw(&mut app);

        let tabs = row_text(&buf, 0);
        assert!(tabs.starts_with(" + "), "{tabs:?}");
        assert!(tabs.contains(" Open "), "{tabs:?}");
        assert!(tabs.contains(" Save "), "{tabs:?}");
        assert!(tabs.contains("notes.txt"), "{tabs:?}");

        assert_eq!(row_text(&buf, 1), format!(" 1 one{}", " ".repeat(74)));
        assert!(row_text(&buf, 2).starts_with(" 2 two"));
        assert!(row_text(&buf, 3).starts_with(" 3 "));

        let status = row_text(&buf, 23);
        assert!(status.starts_with(" src/notes.txt "), "{status:?}");
        assert!(status.ends_with("txt \u{b7} 3 lines "), "{status:?}");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn untitled_status_is_plain_text() {
        let dir = temp_dir("untitled");
        let mut app = app(&dir, &[], KeyboardMode::Enhanced);
        let buf = draw(&mut app);
        let status = row_text(&buf, 23);
        assert!(status.starts_with(" untitled "), "{status:?}");
        assert!(status.ends_with("plain text \u{b7} 1 line "), "{status:?}");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn line_count_label_is_singular_only_for_one() {
        assert_eq!(line_count_label(0), "0 lines");
        assert_eq!(line_count_label(1), "1 line");
        assert_eq!(line_count_label(2), "2 lines");
        assert_eq!(line_count_label(10), "10 lines");
    }

    #[test]
    fn one_line_buffer_status_says_one_line() {
        let dir = temp_dir("one-line");
        let file = dir.join("single.txt");
        fs::write(&file, "only").unwrap();
        let editor = open_editor(&dir, std::slice::from_ref(&file), Some(&dir));
        let mut app = TermApp::new(editor, KeyboardMode::Enhanced, dir.clone(), None);
        let buf = draw(&mut app);
        let status = row_text(&buf, 23);
        assert!(status.starts_with(" single.txt "), "{status:?}");
        assert!(status.ends_with("txt \u{b7} 1 line "), "{status:?}");
        assert!(!status.contains("1 lines"), "{status:?}");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ctrl_n_adds_a_tab() {
        let dir = temp_dir("ctrl-n");
        let mut app = app(&dir, &[], KeyboardMode::Enhanced);
        assert_eq!(app.editor().tabs().len(), 1);
        app.handle(ctrl('n'), Instant::now());
        assert_eq!(app.editor().tabs().len(), 2);
        assert_eq!(app.editor().active_index(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn releases_are_ignored() {
        let dir = temp_dir("release");
        let mut app = app(&dir, &[], KeyboardMode::Enhanced);
        let mut event = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL);
        event.kind = KeyEventKind::Release;
        app.handle(AppEvent::Input(Event::Key(event)), Instant::now());
        assert_eq!(app.editor().tabs().len(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn clicking_close_on_a_dirty_tab_confirms_and_esc_cancels() {
        let dir = temp_dir("close-dirty");
        let file = dir.join("notes.txt");
        fs::write(&file, "hello").unwrap();
        let mut app = app(&dir, &[file], KeyboardMode::Enhanced);
        app.editor.active_view_mut().unwrap().tab.text.push('x');
        app.editor.commit_edit();

        let tabs_before = app.editor().tabs().len();
        let index = app.editor().active_index();
        let buf = draw(&mut app);
        let row = row_text(&buf, 0);
        let title = column_of(&row, "notes.txt");
        let after: String = row.chars().skip(usize::from(title)).collect();
        let col = title + column_of(&after, "\u{d7}");
        assert_eq!(app.hits.hit(col, 0), Some(Target::CloseTab(index)));
        app.handle(
            mouse(MouseEventKind::Down(MouseButton::Left), col, 0),
            Instant::now(),
        );
        assert!(matches!(
            app.editor().prompt(),
            Some(Prompt::ConfirmClose { .. })
        ));
        let buf = draw(&mut app);
        assert!((0..24).any(|y| row_text(&buf, y).contains("[ Discard changes ]")));

        app.handle(key(KeyCode::Esc, KeyModifiers::NONE), Instant::now());
        assert!(app.editor().prompt().is_none());
        assert_eq!(app.editor().tabs().len(), tabs_before);
        assert!(app.editor().tabs()[index].is_dirty());
        fs::remove_dir_all(&dir).unwrap();
    }

    /// The column of tab `index`'s close glyph as last rendered at 80
    /// columns.
    fn close_column(app: &TermApp, index: usize) -> u16 {
        let items = tab_bar::items(app.editor().tabs(), app.editor().active_index(), 80);
        let item = items
            .iter()
            .find(|item| item.target == Target::CloseTab(index))
            .unwrap();
        assert_eq!(app.hits.hit(item.x, 0), Some(Target::CloseTab(index)));
        item.x
    }

    #[test]
    fn queued_clicks_after_a_close_do_not_hit_a_shifted_tab() {
        let dir = temp_dir("queued-close");
        let files: Vec<PathBuf> = ["a.txt", "b.txt", "c.txt"]
            .iter()
            .map(|name| {
                let file = dir.join(name);
                fs::write(&file, *name).unwrap();
                file
            })
            .collect();
        let mut app = app(&dir, &files, KeyboardMode::Enhanced);
        let tabs_before = app.editor().tabs().len();
        let index_of = |app: &TermApp, file: &Path| {
            app.editor()
                .tabs()
                .iter()
                .position(|tab| tab.path.as_deref() == Some(file))
        };
        let first_index = index_of(&app, &files[0]).unwrap();
        assert_eq!(index_of(&app, &files[1]), Some(first_index + 1));
        assert_eq!(index_of(&app, &files[2]), Some(first_index + 2));
        draw(&mut app);
        let first = close_column(&app, first_index);
        let second = close_column(&app, first_index + 1);
        let now = Instant::now();

        app.handle(
            mouse(MouseEventKind::Down(MouseButton::Left), first, 0),
            now,
        );
        app.handle(
            mouse(MouseEventKind::Down(MouseButton::Left), second, 0),
            now,
        );
        app.apply_effects();

        assert_eq!(app.editor().tabs().len(), tabs_before - 1);
        assert_eq!(index_of(&app, &files[0]), None);
        assert_eq!(index_of(&app, &files[1]), Some(first_index));
        assert_eq!(index_of(&app, &files[2]), Some(first_index + 1));
        assert_eq!(app.hovered, None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_resize_clears_the_hit_map_until_the_next_render() {
        let dir = temp_dir("resize-hits");
        let mut app = app(&dir, &[], KeyboardMode::Enhanced);
        let now = Instant::now();
        draw(&mut app);
        app.handle(mouse(MouseEventKind::Moved, 1, 0), now);
        assert_eq!(app.hovered, Some(Target::NewTab));

        app.handle(AppEvent::Input(Event::Resize(100, 30)), now);
        assert_eq!(app.hovered, None);
        app.handle(mouse(MouseEventKind::Down(MouseButton::Left), 1, 0), now);
        assert_eq!(app.editor().tabs().len(), 1);

        draw(&mut app);
        app.handle(mouse(MouseEventKind::Down(MouseButton::Left), 1, 0), now);
        assert_eq!(app.editor().tabs().len(), 2);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn toggling_the_sidebar_and_opening_the_prompt_clear_the_hit_map() {
        let dir = temp_dir("toggle-hits");
        let mut app = app(&dir, &[], KeyboardMode::Enhanced);
        let now = Instant::now();
        draw(&mut app);
        app.handle(ctrl('b'), now);
        assert_eq!(app.hits.hit(1, 0), None);

        draw(&mut app);
        app.handle(ctrl('o'), now);
        assert_eq!(app.hits.hit(1, 0), None);

        app.handle(key(KeyCode::Esc, KeyModifiers::NONE), now);
        let buf = draw(&mut app);
        let open = column_of(&row_text(&buf, 0), "Open");
        app.handle(mouse(MouseEventKind::Down(MouseButton::Left), open, 0), now);
        assert!(app.prompt.is_open(&app.editor));
        assert_eq!(app.hits.hit(1, 0), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn hovering_plus_shows_its_hint() {
        let dir = temp_dir("hover");
        let mut app = app(&dir, &[], KeyboardMode::Enhanced);
        draw(&mut app);
        app.handle(mouse(MouseEventKind::Moved, 1, 0), Instant::now());
        assert_eq!(app.hovered, Some(Target::NewTab));
        let buf = draw(&mut app);
        assert!(row_text(&buf, 23).starts_with(" New tab (Ctrl+N)"));
        assert_eq!(buf[(1, 0)].bg, theme::hover_bg());

        app.handle(mouse(MouseEventKind::Moved, 40, 10), Instant::now());
        assert_eq!(app.hovered, Some(Target::Editor));
        let buf = draw(&mut app);
        assert!(row_text(&buf, 23).starts_with(" untitled "));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn clicking_plus_adds_a_tab() {
        let dir = temp_dir("click-plus");
        let mut app = app(&dir, &[], KeyboardMode::Enhanced);
        draw(&mut app);
        app.handle(
            mouse(MouseEventKind::Down(MouseButton::Left), 1, 0),
            Instant::now(),
        );
        assert_eq!(app.editor().tabs().len(), 2);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ctrl_o_typing_a_path_and_enter_opens_the_file() {
        let dir = temp_dir("ctrl-o");
        let file = dir.join("other.txt");
        fs::write(&file, "content").unwrap();
        let mut app = app(&dir, &[], KeyboardMode::Enhanced);
        let now = Instant::now();
        app.handle(ctrl('o'), now);
        assert!(app.prompt.is_open(&app.editor));
        for c in "other.txt".chars() {
            app.handle(key(KeyCode::Char(c), KeyModifiers::NONE), now);
        }
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();
        let cursor = terminal.get_cursor_position().unwrap();
        let row = row_text(terminal.backend().buffer(), cursor.y);
        assert!(row.contains("other.txt"), "{row:?}");
        assert_eq!(cursor.x, column_of(&row, "other.txt") + 9);

        app.handle(key(KeyCode::Enter, KeyModifiers::NONE), now);
        app.apply_effects();
        assert!(!app.prompt.is_open(&app.editor));
        let tab = app.editor().active_tab().unwrap();
        assert_eq!(tab.path.as_deref(), Some(file.as_path()));
        assert_eq!(tab.text, "content");
        assert_eq!(app.editor().tabs().len(), 2);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn paste_goes_to_the_prompt_or_the_caret() {
        let dir = temp_dir("paste");
        let file = dir.join("pasted.txt");
        fs::write(&file, "x").unwrap();
        let mut app = app(&dir, &[], KeyboardMode::Enhanced);
        let now = Instant::now();
        app.handle(AppEvent::Input(Event::Paste("abc".to_owned())), now);
        assert_eq!(app.editor().active_tab().unwrap().text, "abc");
        assert_eq!(app.status.message(), None);

        app.handle(ctrl('o'), now);
        app.handle(AppEvent::Input(Event::Paste("pasted.txt".to_owned())), now);
        app.handle(key(KeyCode::Enter, KeyModifiers::NONE), now);
        let tab = app.editor().active_tab().unwrap();
        assert_eq!(tab.path.as_deref(), Some(file.as_path()));
        assert_eq!(tab.text, "x");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ctrl_q_quits() {
        let dir = temp_dir("ctrl-q");
        let mut app = app(&dir, &[], KeyboardMode::Enhanced);
        assert!(!app.should_quit());
        app.handle(ctrl('q'), Instant::now());
        assert!(app.should_quit());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn legacy_mode_binds_ctrl_r_and_ctrl_h_is_not_replace() {
        let dir = temp_dir("legacy");
        let mut app = app(&dir, &[], KeyboardMode::Legacy);
        assert_eq!(
            app.status.message(),
            Some((LEGACY_MESSAGE, MessageKind::Info))
        );
        let now = Instant::now();
        app.handle(ctrl('h'), now);
        assert!(!app.editor().search().is_open());
        app.handle(ctrl('r'), now);
        assert!(app.editor().search().is_open());

        // The find and replace rows push the editor down two rows.
        let buf = draw(&mut app);
        assert!(row_text(&buf, 1).starts_with(" Find "));
        assert!(row_text(&buf, 2).starts_with(" With "));
        assert!(row_text(&buf, 3).starts_with(" 1 "));

        app.handle(key(KeyCode::Esc, KeyModifiers::NONE), now);
        assert!(!app.editor().search().is_open());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn enhanced_mode_binds_ctrl_h_and_has_no_startup_message() {
        let dir = temp_dir("enhanced");
        let mut app = app(&dir, &[], KeyboardMode::Enhanced);
        assert_eq!(app.status.message(), None);
        app.handle(ctrl('h'), Instant::now());
        assert!(app.editor().search().is_open());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ctrl_b_hides_and_shows_the_sidebar() {
        let dir = temp_dir("sidebar");
        let ws = dir.join("ws");
        fs::create_dir_all(&ws).unwrap();
        let editor = open_editor(&dir, &[], Some(&ws));
        let mut app = TermApp::new(editor, KeyboardMode::Enhanced, ws.clone(), None);
        let now = Instant::now();

        let buf = draw(&mut app);
        assert_eq!(buf[(31, 1)].symbol(), "1");
        // The tree's header and its border column fill columns 0..30.
        assert!(row_text(&buf, 1).starts_with(" [x] "));
        assert_eq!(buf[(0, 10)].bg, theme::chrome().bg.unwrap());
        assert_eq!(buf[(29, 10)].bg, theme::tree_border().bg.unwrap());

        app.handle(ctrl('e'), now);
        assert_eq!(app.focus, Focus::Sidebar);

        app.handle(ctrl('b'), now);
        assert_eq!(app.focus, Focus::Editor);
        let buf = draw(&mut app);
        assert_eq!(buf[(1, 1)].symbol(), "1");
        assert_eq!(buf[(31, 1)].symbol(), " ");
        assert!(!row_text(&buf, 1).contains("[x]"));
        assert_ne!(buf[(29, 10)].bg, theme::tree_border().bg.unwrap());

        app.handle(ctrl('e'), now);
        assert_eq!(app.focus, Focus::Editor);

        app.handle(ctrl('b'), now);
        let buf = draw(&mut app);
        assert_eq!(buf[(31, 1)].symbol(), "1");
        assert!(row_text(&buf, 1).starts_with(" [x] "));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn without_a_workspace_there_is_no_sidebar() {
        let dir = temp_dir("no-sidebar");
        let mut app = app(&dir, &[], KeyboardMode::Enhanced);
        let buf = draw(&mut app);
        assert_eq!(buf[(1, 1)].symbol(), "1");
        app.handle(ctrl('e'), Instant::now());
        assert_eq!(app.focus, Focus::Editor);
        fs::remove_dir_all(&dir).unwrap();
    }

    fn long_file(dir: &Path) -> PathBuf {
        let file = dir.join("long.txt");
        let text: String = (1..=100).map(|n| format!("line {n}\n")).collect();
        fs::write(&file, text).unwrap();
        file
    }

    #[test]
    fn the_wheel_over_the_editor_scrolls() {
        let dir = temp_dir("wheel");
        let file = long_file(&dir);
        let mut app = app(&dir, &[file], KeyboardMode::Enhanced);
        let now = Instant::now();
        draw(&mut app);

        app.handle(mouse(MouseEventKind::ScrollDown, 40, 10), now);
        let buf = draw(&mut app);
        assert!(row_text(&buf, 1).starts_with("  4 line 4 "));

        app.handle(mouse(MouseEventKind::ScrollUp, 40, 10), now);
        let buf = draw(&mut app);
        assert!(row_text(&buf, 1).starts_with("  1 line 1 "));

        // Off the editor (the tab bar) the wheel does nothing.
        app.handle(mouse(MouseEventKind::ScrollDown, 1, 0), now);
        let buf = draw(&mut app);
        assert!(row_text(&buf, 1).starts_with("  1 line 1 "));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_view_follows_the_caret() {
        let dir = temp_dir("keys-scroll");
        let file = long_file(&dir);
        let mut app = app(&dir, &[file], KeyboardMode::Enhanced);
        let now = Instant::now();
        draw(&mut app);

        // 101 rows (the trailing newline adds one) in 22: the last 22.
        app.handle(key(KeyCode::End, KeyModifiers::CONTROL), now);
        let buf = draw(&mut app);
        assert!(row_text(&buf, 1).starts_with(" 80 line 80 "));

        app.handle(key(KeyCode::Home, KeyModifiers::CONTROL), now);
        let buf = draw(&mut app);
        assert!(row_text(&buf, 1).starts_with("  1 line 1 "));

        // A page is 21 rows: the first stays on screen, the second
        // scrolls just enough.
        app.handle(key(KeyCode::PageDown, KeyModifiers::NONE), now);
        let buf = draw(&mut app);
        assert!(row_text(&buf, 1).starts_with("  1 line 1 "));
        app.handle(key(KeyCode::PageDown, KeyModifiers::NONE), now);
        let buf = draw(&mut app);
        assert!(row_text(&buf, 1).starts_with(" 22 line 22 "));

        app.handle(key(KeyCode::Down, KeyModifiers::NONE), now);
        let buf = draw(&mut app);
        assert!(row_text(&buf, 1).starts_with(" 23 line 23 "));
        let tab_id = app.editor().active_tab().unwrap().id;
        assert_eq!(
            app.buffer.caret(tab_id),
            Some(text_index(&app, "line 44\n"))
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    /// The char index of `needle` in the active tab's text.
    fn text_index(app: &TermApp, needle: &str) -> usize {
        let text = &app.editor().active_tab().unwrap().text;
        text[..text.find(needle).unwrap()].chars().count()
    }

    #[test]
    fn reveal_selection_centres_the_caret() {
        let dir = temp_dir("reveal");
        let file = long_file(&dir);
        let mut app = app(&dir, &[file], KeyboardMode::Enhanced);
        draw(&mut app);
        let tab_id = app.editor().active_tab().unwrap().id;
        let start = text_index(&app, "line 50\n");
        app.effects.push(Effect::SetSelection {
            tab_id,
            chars: start..start + 4,
            focus: EditorFocus::Keep,
        });
        app.effects.push(Effect::RevealSelection { tab_id });
        app.apply_effects();
        assert!(app.effects.is_empty());
        assert_eq!(app.buffer.selection(tab_id), Some(start..start + 4));
        // Line 50 (index 49) centred in 22 rows: top index 38.
        let buf = draw(&mut app);
        assert!(row_text(&buf, 1).starts_with(" 39 line 39 "));

        // A reveal for another tab does nothing.
        app.handle(key(KeyCode::Home, KeyModifiers::CONTROL), Instant::now());
        app.effects
            .push(Effect::RevealSelection { tab_id: tab_id + 1 });
        app.apply_effects();
        let buf = draw(&mut app);
        assert!(row_text(&buf, 1).starts_with("  1 line 1 "));
        fs::remove_dir_all(&dir).unwrap();
    }

    /// An app over one file holding `text`, drawn once.
    fn app_with_text(dir: &Path, text: &str, keyboard: KeyboardMode) -> TermApp {
        let file = dir.join("text.txt");
        fs::write(&file, text).unwrap();
        let mut app = app(dir, &[file], keyboard);
        draw(&mut app);
        app
    }

    fn text_of(app: &TermApp) -> &str {
        &app.editor().active_tab().unwrap().text
    }

    fn press(app: &mut TermApp, code: KeyCode, modifiers: KeyModifiers, times: usize) {
        for _ in 0..times {
            app.handle(key(code, modifiers), Instant::now());
        }
    }

    fn type_text(app: &mut TermApp, text: &str) {
        for c in text.chars() {
            app.handle(key(KeyCode::Char(c), KeyModifiers::NONE), Instant::now());
        }
    }

    #[test]
    fn typing_shows_the_text_marks_dirty_and_writes_the_swap() {
        let dir = temp_dir("typing");
        let mut app = app(&dir, &[], KeyboardMode::Enhanced);
        draw(&mut app);
        type_text(&mut app, "hi");
        let tab = app.editor().active_tab().unwrap();
        assert_eq!(tab.text, "hi");
        assert!(tab.is_dirty());
        let swap = dir
            .join("state")
            .join("swap")
            .join(format!("{}.swp", tab.id));
        assert_eq!(fs::read_to_string(swap).unwrap(), "hi");
        let buf = draw(&mut app);
        assert!(row_text(&buf, 1).starts_with(" 1 hi "));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn select_all_then_backspace_in_one_batch_empties_the_buffer() {
        let dir = temp_dir("select-all");
        let mut app = app_with_text(&dir, "hello\nworld", KeyboardMode::Enhanced);
        let now = Instant::now();
        app.handle(ctrl('a'), now);
        app.handle(key(KeyCode::Backspace, KeyModifiers::NONE), now);
        assert_eq!(text_of(&app), "");
        assert!(app.editor().active_tab().unwrap().is_dirty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cut_then_undo_restores_the_text() {
        let dir = temp_dir("cut-undo");
        let mut app = app_with_text(&dir, "hello world", KeyboardMode::Enhanced);
        press(&mut app, KeyCode::Right, KeyModifiers::SHIFT, 6);
        app.handle(ctrl('x'), Instant::now());
        assert_eq!(text_of(&app), "world");
        let tab_id = app.editor().active_tab().unwrap().id;
        assert_eq!(app.buffer.selection(tab_id), Some(0..0));
        app.handle(ctrl('z'), Instant::now());
        assert_eq!(text_of(&app), "hello world");
        assert_eq!(app.buffer.selection(tab_id), Some(0..6));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn copy_and_paste_go_through_the_clipboard() {
        let dir = temp_dir("copy-paste");
        let mut app = app_with_text(&dir, "hello world", KeyboardMode::Enhanced);
        press(&mut app, KeyCode::Right, KeyModifiers::SHIFT, 5);
        app.handle(ctrl('c'), Instant::now());
        assert_eq!(text_of(&app), "hello world");
        assert!(!app.editor().active_tab().unwrap().is_dirty());

        app.handle(key(KeyCode::End, KeyModifiers::CONTROL), Instant::now());
        app.handle(ctrl('v'), Instant::now());
        assert_eq!(text_of(&app), "hello worldhello");
        let tab_id = app.editor().active_tab().unwrap().id;
        assert_eq!(app.buffer.caret(tab_id), Some(16));
        // The paste is one undo step.
        app.handle(ctrl('z'), Instant::now());
        assert_eq!(text_of(&app), "hello world");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn redo_of_a_paste_into_repeated_text_restores_the_paste_caret() {
        let dir = temp_dir("paste-redo-caret");
        let mut app = app_with_text(&dir, "aaa", KeyboardMode::Enhanced);
        press(&mut app, KeyCode::Right, KeyModifiers::SHIFT, 1);
        app.handle(ctrl('c'), Instant::now());
        app.handle(key(KeyCode::Home, KeyModifiers::CONTROL), Instant::now());
        let tab_id = app.editor().active_tab().unwrap().id;
        assert_eq!(app.buffer.selection(tab_id), Some(0..0));

        app.handle(ctrl('v'), Instant::now());
        assert_eq!(text_of(&app), "aaaa");
        assert_eq!(app.buffer.caret(tab_id), Some(1));
        app.handle(ctrl('z'), Instant::now());
        assert_eq!(text_of(&app), "aaa");
        app.handle(ctrl('y'), Instant::now());
        assert_eq!(text_of(&app), "aaaa");
        assert_eq!(app.buffer.caret(tab_id), Some(1));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_paste_into_a_never_rendered_new_tab_is_undoable() {
        let dir = temp_dir("paste-new-tab");
        let mut app = app_with_text(&dir, "hello", KeyboardMode::Enhanced);
        press(&mut app, KeyCode::Right, KeyModifiers::SHIFT, 5);
        app.handle(ctrl('c'), Instant::now());

        // One batch: no render between the new tab, the paste and the undo.
        let tabs = app.editor().tabs().len();
        app.handle(ctrl('n'), Instant::now());
        app.handle(ctrl('v'), Instant::now());
        assert_eq!(app.editor().active_index(), tabs);
        assert_eq!(text_of(&app), "hello");
        app.handle(ctrl('z'), Instant::now());
        assert_eq!(text_of(&app), "");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn bracketed_paste_inserts_at_the_caret() {
        let dir = temp_dir("bracketed");
        let mut app = app_with_text(&dir, "ab", KeyboardMode::Enhanced);
        app.handle(key(KeyCode::Right, KeyModifiers::NONE), Instant::now());
        app.handle(
            AppEvent::Input(Event::Paste("X\r\nY\rZ".to_owned())),
            Instant::now(),
        );
        assert_eq!(text_of(&app), "aX\nY\nZb");
        assert!(app.editor().active_tab().unwrap().is_dirty());
        app.handle(ctrl('z'), Instant::now());
        assert_eq!(text_of(&app), "ab");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn undo_and_both_redo_keys() {
        let dir = temp_dir("undo-redo");
        let mut app = app(&dir, &[], KeyboardMode::Enhanced);
        draw(&mut app);
        type_text(&mut app, "a");
        app.handle(ctrl('z'), Instant::now());
        assert_eq!(text_of(&app), "");
        app.handle(
            key(
                KeyCode::Char('Z'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ),
            Instant::now(),
        );
        assert_eq!(text_of(&app), "a");
        app.handle(ctrl('z'), Instant::now());
        assert_eq!(text_of(&app), "");
        app.handle(ctrl('y'), Instant::now());
        assert_eq!(text_of(&app), "a");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_core_command_breaks_the_typing_group() {
        let dir = temp_dir("save-breaks-group");
        let mut app = app_with_text(&dir, "", KeyboardMode::Enhanced);
        let now = Instant::now();
        app.handle(key(KeyCode::Char('a'), KeyModifiers::NONE), now);
        app.handle(ctrl('s'), now);
        assert!(!app.editor().active_tab().unwrap().is_dirty());
        app.handle(key(KeyCode::Char('b'), KeyModifiers::NONE), now);
        assert_eq!(text_of(&app), "ab");
        app.handle(ctrl('z'), now);
        assert_eq!(text_of(&app), "a");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_focus_change_breaks_the_typing_group() {
        let dir = temp_dir("focus-breaks-group");
        let ws = dir.join("ws");
        fs::create_dir_all(&ws).unwrap();
        let editor = open_editor(&dir, &[], Some(&ws));
        let mut app = TermApp::new(editor, KeyboardMode::Enhanced, ws.clone(), None);
        draw(&mut app);
        let now = Instant::now();
        app.handle(key(KeyCode::Char('a'), KeyModifiers::NONE), now);
        app.handle(ctrl('e'), now);
        assert_eq!(app.focus, Focus::Sidebar);
        app.handle(ctrl('e'), now);
        assert_eq!(app.focus, Focus::Editor);
        app.handle(key(KeyCode::Char('b'), KeyModifiers::NONE), now);
        assert_eq!(text_of(&app), "ab");
        app.handle(ctrl('z'), now);
        assert_eq!(text_of(&app), "a");
        fs::remove_dir_all(&dir).unwrap();
    }

    /// Draws `app` and returns the terminal cursor, if it is shown.
    fn frame_cursor(app: &mut TermApp) -> Option<Position> {
        frame_cursor_sized(app, 80, 24)
    }

    /// Draws `app` on a `width` x `height` terminal and returns the
    /// terminal cursor, if it is shown.
    fn frame_cursor_sized(app: &mut TermApp, width: u16, height: u16) -> Option<Position> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();
        terminal
            .backend()
            .cursor_visible()
            .then(|| terminal.backend().cursor_position())
    }

    /// Draws `app` through [`TermApp::render_frame`] and returns its cursor.
    fn render_frame_cursor(app: &mut TermApp) -> Option<Position> {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut cursor = None;
        terminal
            .draw(|frame| cursor = app.render_frame(frame))
            .unwrap();
        cursor
    }

    #[test]
    fn render_frame_returns_the_cursor_render_sets() {
        let dir = temp_dir("render-frame-cursor");
        let ws = dir.join("ws");
        fs::create_dir_all(&ws).unwrap();
        let editor = open_editor(&dir, &[], Some(&ws));
        let mut app = TermApp::new(editor, KeyboardMode::Enhanced, ws.clone(), None);
        app.handle(ctrl('b'), Instant::now());
        draw(&mut app);
        type_text(&mut app, "hi");
        let expected = Some(Position { x: 5, y: 1 });
        assert_eq!(frame_cursor(&mut app), expected);
        assert_eq!(render_frame_cursor(&mut app), expected);

        // A hidden cursor is `None` both ways.
        app.handle(ctrl('b'), Instant::now());
        app.handle(ctrl('e'), Instant::now());
        assert_eq!(app.focus, Focus::Sidebar);
        assert_eq!(frame_cursor(&mut app), None);
        assert_eq!(render_frame_cursor(&mut app), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn graphics_pass_through_to_the_tree_icons() {
        use crate::icons::kitty::KittyIcons;
        use crate::term::caps::{CellPx, Multiplexer};

        let dir = temp_dir("graphics");
        let ws = dir.join("ws");
        fs::create_dir_all(&ws).unwrap();
        fs::write(ws.join("main.rs"), "fn main() {}\n").unwrap();
        let editor = open_editor(&dir, &[], Some(&ws));
        let mut app = TermApp::new(editor, KeyboardMode::Enhanced, ws.clone(), None);
        assert!(app.take_graphics().is_empty());

        app.set_icons(Box::new(KittyIcons::new(
            CellPx {
                width: 10,
                height: 20,
            },
            Multiplexer::None,
            0,
        )));
        draw(&mut app);
        let uploads = String::from_utf8(app.take_graphics()).unwrap();
        assert!(uploads.contains("\x1b_Ga=T"), "{uploads:?}");
        assert!(app.take_graphics().is_empty());

        let release = String::from_utf8(app.release_graphics()).unwrap();
        assert!(release.contains("a=d,d=I,i=1,q=2"), "{release:?}");
        assert!(app.release_graphics().is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_caret_is_the_frame_cursor_only_in_the_editor() {
        let dir = temp_dir("caret-cursor");
        let ws = dir.join("ws");
        fs::create_dir_all(&ws).unwrap();
        let editor = open_editor(&dir, &[], Some(&ws));
        let mut app = TermApp::new(editor, KeyboardMode::Enhanced, ws.clone(), None);
        app.handle(ctrl('b'), Instant::now());
        draw(&mut app);
        type_text(&mut app, "hi");
        // The gutter " 1 " is three columns wide.
        assert_eq!(frame_cursor(&mut app), Some(Position { x: 5, y: 1 }));

        app.handle(ctrl('o'), Instant::now());
        let cursor = frame_cursor(&mut app).unwrap();
        assert_ne!(cursor, Position { x: 5, y: 1 });
        app.handle(key(KeyCode::Esc, KeyModifiers::NONE), Instant::now());

        app.handle(ctrl('b'), Instant::now());
        app.handle(ctrl('e'), Instant::now());
        assert_eq!(app.focus, Focus::Sidebar);
        assert_eq!(frame_cursor(&mut app), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn set_selection_with_focus_moves_focus_from_the_sidebar() {
        let dir = temp_dir("focus-effect");
        let ws = dir.join("ws");
        fs::create_dir_all(&ws).unwrap();
        let file = ws.join("a.txt");
        fs::write(&file, "hello").unwrap();
        let editor = open_editor(&dir, std::slice::from_ref(&file), Some(&ws));
        let mut app = TermApp::new(editor, KeyboardMode::Enhanced, ws.clone(), None);
        draw(&mut app);
        app.handle(ctrl('e'), Instant::now());
        assert_eq!(app.focus, Focus::Sidebar);
        let tab_id = app.editor().active_tab().unwrap().id;

        app.effects.push(Effect::SetSelection {
            tab_id,
            chars: 1..3,
            focus: EditorFocus::Keep,
        });
        app.apply_effects();
        assert_eq!(app.focus, Focus::Sidebar);
        assert_eq!(app.buffer.selection(tab_id), Some(1..3));

        app.effects.push(Effect::SetSelection {
            tab_id,
            chars: 2..2,
            focus: EditorFocus::Focus,
        });
        app.apply_effects();
        assert_eq!(app.focus, Focus::Editor);
        assert_eq!(app.buffer.selection(tab_id), Some(2..2));

        // A selection for a tab that does not exist is dropped.
        app.effects.push(Effect::SetSelection {
            tab_id: tab_id + 100,
            chars: 0..1,
            focus: EditorFocus::Keep,
        });
        app.apply_effects();
        assert_eq!(app.buffer.selection(tab_id + 100), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn legacy_ctrl_h_deletes_a_char() {
        let dir = temp_dir("legacy-ctrl-h");
        let mut app = app_with_text(&dir, "ab", KeyboardMode::Legacy);
        app.handle(key(KeyCode::End, KeyModifiers::NONE), Instant::now());
        app.handle(ctrl('h'), Instant::now());
        assert_eq!(text_of(&app), "a");
        assert!(!app.editor().search().is_open());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn editing_keys_do_nothing_while_the_sidebar_has_focus() {
        let dir = temp_dir("sidebar-keys");
        let ws = dir.join("ws");
        fs::create_dir_all(&ws).unwrap();
        let file = ws.join("a.txt");
        fs::write(&file, "hello").unwrap();
        let editor = open_editor(&dir, std::slice::from_ref(&file), Some(&ws));
        let mut app = TermApp::new(editor, KeyboardMode::Legacy, ws.clone(), None);
        draw(&mut app);
        app.handle(
            AppEvent::Input(Event::Paste("p".to_owned())),
            Instant::now(),
        );
        app.handle(ctrl('a'), Instant::now());
        app.handle(ctrl('c'), Instant::now());
        app.handle(ctrl('e'), Instant::now());
        assert_eq!(app.focus, Focus::Sidebar);
        // Select `a.txt` in the drawn tree, so Enter below opens the file
        // that is already open rather than editing it.
        app.handle(key(KeyCode::Down, KeyModifiers::NONE), Instant::now());
        let buf = draw(&mut app);
        assert!(
            row_text(&buf, 2).contains("a.txt"),
            "{:?}",
            row_text(&buf, 2)
        );
        assert_eq!(app.tree.selected(), Some(file.as_path()));
        for event in [
            ctrl('a'),
            ctrl('x'),
            ctrl('v'),
            ctrl('z'),
            ctrl('y'),
            ctrl('h'),
            key(KeyCode::Char('q'), KeyModifiers::NONE),
            key(KeyCode::Backspace, KeyModifiers::NONE),
            key(KeyCode::Enter, KeyModifiers::NONE),
            AppEvent::Input(Event::Paste("zz".to_owned())),
        ] {
            app.handle(event, Instant::now());
        }
        assert_eq!(app.focus, Focus::Sidebar);
        assert_eq!(text_of(&app), "phello");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn send_passes_the_selection_to_core() {
        let dir = temp_dir("seed-search");
        let mut app = app_with_text(&dir, "hello world", KeyboardMode::Enhanced);
        press(&mut app, KeyCode::Right, KeyModifiers::SHIFT, 5);
        app.handle(ctrl('f'), Instant::now());
        assert!(app.editor().search().is_open());
        assert_eq!(app.editor().search().query().pattern, "hello");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn open_search_paints_its_matches_in_the_editor() {
        let dir = temp_dir("search-highlights");
        let mut app = app_with_text(&dir, "hello world hello", KeyboardMode::Enhanced);
        press(&mut app, KeyCode::Right, KeyModifiers::SHIFT, 5);
        app.handle(ctrl('f'), Instant::now());
        // The selection colour wins over a match, so collapse it (with a
        // click past the end of the line, which also focuses the editor)
        // to show the first match's own background.
        draw(&mut app);
        left_click(&mut app, 60, 2);
        assert_eq!(app.focus, Focus::Editor);
        let buf = draw(&mut app);
        // The find bar row sits under the tab bar, so the text is on row 2.
        let row = row_text(&buf, 2);
        let first = column_of(&row, "hello world");
        let second = column_of(&row, "world hello") + 6;
        let current = theme::match_bg(MatchBackground::Current);
        let other = theme::match_bg(MatchBackground::Other);
        for x in 0..5 {
            assert_eq!(buf[(first + x, 2)].bg, current);
            assert_eq!(buf[(second + x, 2)].bg, other);
        }

        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, 1);
        let buf = draw(&mut app);
        let row = row_text(&buf, 1);
        let first = column_of(&row, "hello world");
        let second = column_of(&row, "world hello") + 6;
        for x in 0..5 {
            assert_eq!(buf[(first + x, 1)].bg, theme::color(BASE));
            assert_eq!(buf[(second + x, 1)].bg, theme::color(BASE));
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_tick_after_four_seconds_clears_the_message() {
        let dir = temp_dir("tick");
        // The legacy startup message is set when the app is made, just
        // before `start`.
        let mut app = app(&dir, &[], KeyboardMode::Legacy);
        let start = Instant::now();
        app.handle(AppEvent::Tick, start + Duration::from_secs(3));
        assert!(app.status.message().is_some());
        app.handle(AppEvent::Tick, start + Duration::from_secs(4));
        assert_eq!(app.status.message(), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn input_closed_quits() {
        let dir = temp_dir("input-closed");
        let mut app = app(&dir, &[], KeyboardMode::Enhanced);
        app.handle(AppEvent::Wake, Instant::now());
        app.handle(AppEvent::Input(Event::Resize(10, 10)), Instant::now());
        assert!(!app.should_quit());
        app.handle(AppEvent::InputClosed, Instant::now());
        assert!(app.should_quit());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn frame_passthroughs_and_shutdown_do_not_disturb_state() {
        let dir = temp_dir("frame");
        let mut app = app(&dir, &[], KeyboardMode::Enhanced);
        app.begin_frame(|| {});
        app.end_frame();
        app.shutdown();
        assert_eq!(app.editor().tabs().len(), 1);
        assert!(!app.should_quit());
        fs::remove_dir_all(&dir).unwrap();
    }

    fn left(kind: fn(MouseButton) -> MouseEventKind) -> MouseEventKind {
        kind(MouseButton::Left)
    }

    fn shift_down(column: u16, row: u16) -> AppEvent {
        AppEvent::Input(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::SHIFT,
        }))
    }

    fn active_id(app: &TermApp) -> u64 {
        app.editor().active_tab().unwrap().id
    }

    #[test]
    fn a_click_places_the_caret() {
        let dir = temp_dir("click");
        let mut app = app_with_text(&dir, "hello\nworld", KeyboardMode::Enhanced);
        let now = Instant::now();
        // Gutter " 1 " is three columns: text column 2 of line 2 is x=5.
        app.handle(mouse(left(MouseEventKind::Down), 5, 2), now);
        app.handle(mouse(left(MouseEventKind::Up), 5, 2), now);
        let id = active_id(&app);
        assert_eq!(app.buffer.caret(id), Some(8));
        assert_eq!(app.buffer.selection(id), Some(8..8));
        assert_eq!(app.focus, Focus::Editor);
        assert_eq!(frame_cursor(&mut app), Some(Position::new(5, 2)));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_double_click_selects_a_word_and_ctrl_c_copies_it() {
        let dir = temp_dir("double-click");
        let mut app = app_with_text(&dir, "hello world", KeyboardMode::Enhanced);
        let now = Instant::now();
        app.handle(mouse(left(MouseEventKind::Down), 10, 1), now);
        app.handle(mouse(left(MouseEventKind::Up), 10, 1), now);
        app.handle(mouse(left(MouseEventKind::Down), 10, 1), now);
        app.handle(mouse(left(MouseEventKind::Up), 10, 1), now);
        let id = active_id(&app);
        assert_eq!(app.buffer.selection(id), Some(6..11));

        app.handle(ctrl('c'), now);
        app.handle(key(KeyCode::End, KeyModifiers::CONTROL), now);
        app.handle(ctrl('v'), now);
        assert_eq!(text_of(&app), "hello worldworld");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_drag_selects_across_two_lines() {
        let dir = temp_dir("drag");
        let mut app = app_with_text(&dir, "hello\nworld", KeyboardMode::Enhanced);
        let now = Instant::now();
        app.handle(mouse(left(MouseEventKind::Down), 4, 1), now);
        assert!(app.buffer.is_dragging());
        app.handle(mouse(left(MouseEventKind::Drag), 6, 2), now);
        app.handle(mouse(left(MouseEventKind::Up), 6, 2), now);
        assert!(!app.buffer.is_dragging());
        let id = active_id(&app);
        assert_eq!(app.buffer.selection(id), Some(1..9));

        // After the release a drag event no longer selects.
        app.handle(mouse(left(MouseEventKind::Drag), 3, 1), now);
        assert_eq!(app.buffer.selection(id), Some(1..9));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_drag_that_leaves_the_editor_keeps_selecting() {
        let dir = temp_dir("drag-out");
        let file = long_file(&dir);
        let mut app = app(&dir, &[file], KeyboardMode::Enhanced);
        let now = Instant::now();
        draw(&mut app);
        let id = active_id(&app);
        // 101 lines: the gutter "  1 " is four columns.
        app.handle(mouse(left(MouseEventKind::Down), 4, 1), now);

        // The status row (23) is below the editor (rows 1..=22): one row
        // of scroll per event, selecting to the edge row.
        app.handle(mouse(left(MouseEventKind::Drag), 4, 23), now);
        let buf = draw(&mut app);
        assert!(row_text(&buf, 1).starts_with("  2 line 2 "));
        assert_eq!(
            app.buffer.selection(id),
            Some(0..text_index(&app, "line 23\n"))
        );

        app.handle(mouse(left(MouseEventKind::Drag), 4, 23), now);
        let buf = draw(&mut app);
        assert!(row_text(&buf, 1).starts_with("  3 line 3 "));
        assert_eq!(
            app.buffer.selection(id),
            Some(0..text_index(&app, "line 24\n"))
        );

        // The tab bar (row 0) is above it: scrolls back up.
        app.handle(mouse(left(MouseEventKind::Drag), 4, 0), now);
        let buf = draw(&mut app);
        assert!(row_text(&buf, 1).starts_with("  2 line 2 "));
        assert_eq!(
            app.buffer.selection(id),
            Some(0..text_index(&app, "line 2\n"))
        );
        app.handle(mouse(left(MouseEventKind::Up), 4, 0), now);
        assert!(!app.buffer.is_dragging());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn shift_click_extends_the_selection() {
        let dir = temp_dir("shift-click");
        let mut app = app_with_text(&dir, "hello world", KeyboardMode::Enhanced);
        let now = Instant::now();
        app.handle(mouse(left(MouseEventKind::Down), 5, 1), now);
        app.handle(mouse(left(MouseEventKind::Up), 5, 1), now);
        app.handle(shift_down(11, 1), now);
        app.handle(mouse(left(MouseEventKind::Up), 11, 1), now);
        let id = active_id(&app);
        assert_eq!(app.buffer.selection(id), Some(2..8));
        assert_eq!(app.buffer.caret(id), Some(8));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_wheel_scrolls_without_moving_the_caret() {
        let dir = temp_dir("wheel-caret");
        let file = long_file(&dir);
        let mut app = app(&dir, &[file], KeyboardMode::Enhanced);
        let now = Instant::now();
        draw(&mut app);
        app.handle(mouse(left(MouseEventKind::Down), 6, 1), now);
        app.handle(mouse(left(MouseEventKind::Up), 6, 1), now);
        let id = active_id(&app);
        assert_eq!(app.buffer.caret(id), Some(2));

        app.handle(mouse(MouseEventKind::ScrollDown, 40, 10), now);
        let buf = draw(&mut app);
        assert!(row_text(&buf, 1).starts_with("  4 line 4 "));
        assert_eq!(app.buffer.caret(id), Some(2));
        assert_eq!(app.buffer.selection(id), Some(2..2));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_click_while_a_prompt_is_open_does_not_touch_the_buffer() {
        let dir = temp_dir("click-prompt");
        let mut app = app_with_text(&dir, "hello\nworld", KeyboardMode::Enhanced);
        let now = Instant::now();
        let id = active_id(&app);
        let before = app.buffer.selection(id);
        app.handle(ctrl('o'), now);
        draw(&mut app);
        assert!(app.prompt.is_open(&app.editor));

        app.handle(mouse(left(MouseEventKind::Down), 5, 2), now);
        assert!(!app.buffer.is_dragging());
        app.handle(mouse(left(MouseEventKind::Drag), 6, 2), now);
        app.handle(mouse(left(MouseEventKind::Up), 6, 2), now);
        assert_eq!(app.buffer.selection(id), before);
        fs::remove_dir_all(&dir).unwrap();
    }

    fn right_click(app: &mut TermApp, column: u16, row: u16) {
        app.handle(
            mouse(MouseEventKind::Down(MouseButton::Right), column, row),
            Instant::now(),
        );
    }

    fn left_click(app: &mut TermApp, column: u16, row: u16) {
        app.handle(
            mouse(left(MouseEventKind::Down), column, row),
            Instant::now(),
        );
        app.handle(mouse(left(MouseEventKind::Up), column, row), Instant::now());
    }

    /// The screen row of editor-menu entry `index` for a menu opened
    /// downwards at `anchor_row`.
    const fn menu_row(anchor_row: u16, index: u16) -> u16 {
        anchor_row + 1 + index
    }

    /// "hello world" with "hello" selected.
    fn app_with_hello_selected(dir: &Path) -> TermApp {
        let mut app = app_with_text(dir, "hello world", KeyboardMode::Enhanced);
        press(&mut app, KeyCode::Right, KeyModifiers::SHIFT, 5);
        app
    }

    #[test]
    fn a_right_click_opens_the_menu_without_moving_the_caret() {
        let dir = temp_dir("menu-open");
        let mut app = app_with_hello_selected(&dir);
        let id = active_id(&app);
        right_click(&mut app, 20, 1);
        let menu = app.menu.as_ref().unwrap();
        assert_eq!(
            menu,
            &ContextMenu::new(
                editor_menu::entries(Enablement::Enabled, None),
                Position::new(20, 1),
            )
        );
        assert_eq!(app.buffer.selection(id), Some(0..5));
        assert_eq!(app.buffer.caret(id), Some(5));
        assert!(!app.buffer.is_dragging());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn without_a_selection_cut_and_copy_are_disabled_and_do_nothing() {
        let dir = temp_dir("menu-disabled");
        let mut app = app_with_text(&dir, "hello world", KeyboardMode::Enhanced);
        right_click(&mut app, 10, 1);
        assert_eq!(
            app.menu.as_ref().unwrap(),
            &ContextMenu::new(
                editor_menu::entries(Enablement::Disabled, None),
                Position::new(10, 1)
            )
        );
        draw(&mut app);
        app.handle(
            mouse(MouseEventKind::Down(MouseButton::Left), 12, menu_row(1, 0)),
            Instant::now(),
        );
        assert!(app.menu.is_some());
        assert_eq!(text_of(&app), "hello world");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn right_clicks_off_the_editor_or_under_a_prompt_open_nothing() {
        let dir = temp_dir("menu-off-editor");
        let mut app = app_with_text(&dir, "hello", KeyboardMode::Enhanced);
        right_click(&mut app, 1, 0);
        assert!(app.menu.is_none());
        right_click(&mut app, 0, 23);
        assert!(app.menu.is_none());
        app.handle(ctrl('o'), Instant::now());
        draw(&mut app);
        right_click(&mut app, 40, 20);
        assert!(app.menu.is_none());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn clicking_copy_copies_the_selection() {
        let dir = temp_dir("menu-copy");
        let mut app = app_with_hello_selected(&dir);
        right_click(&mut app, 10, 1);
        draw(&mut app);
        assert_eq!(app.hits.hit(12, menu_row(1, 1)), Some(Target::MenuItem(1)));
        app.handle(
            mouse(MouseEventKind::Down(MouseButton::Left), 12, menu_row(1, 1)),
            Instant::now(),
        );
        assert!(app.menu.is_none());
        assert_eq!(text_of(&app), "hello world");

        draw(&mut app);
        app.handle(key(KeyCode::End, KeyModifiers::CONTROL), Instant::now());
        app.handle(ctrl('v'), Instant::now());
        assert_eq!(text_of(&app), "hello worldhello");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn down_and_enter_cut_and_ctrl_z_restores() {
        let dir = temp_dir("menu-cut");
        let mut app = app_with_hello_selected(&dir);
        right_click(&mut app, 10, 1);
        app.handle(key(KeyCode::Down, KeyModifiers::NONE), Instant::now());
        assert_eq!(app.menu.as_ref().unwrap().highlighted(), Some(0));
        // The menu captured the arrow: the selection is untouched.
        let id = active_id(&app);
        assert_eq!(app.buffer.selection(id), Some(0..5));
        app.handle(key(KeyCode::Enter, KeyModifiers::NONE), Instant::now());
        assert!(app.menu.is_none());
        assert_eq!(text_of(&app), " world");

        app.handle(ctrl('z'), Instant::now());
        assert_eq!(text_of(&app), "hello world");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn choosing_find_opens_the_search() {
        let dir = temp_dir("menu-find");
        let mut app = app_with_text(&dir, "hello world", KeyboardMode::Enhanced);
        right_click(&mut app, 10, 1);
        draw(&mut app);
        app.handle(
            mouse(MouseEventKind::Down(MouseButton::Left), 12, menu_row(1, 5)),
            Instant::now(),
        );
        assert!(app.menu.is_none());
        assert!(app.editor().search().is_open());
        assert_eq!(app.focus, Focus::Find(FindField::Pattern));
        let buf = draw(&mut app);
        assert!(row_text(&buf, 1).starts_with(" Find "));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn esc_dismisses_the_menu() {
        let dir = temp_dir("menu-esc");
        let mut app = app_with_hello_selected(&dir);
        right_click(&mut app, 10, 1);
        app.handle(key(KeyCode::Esc, KeyModifiers::NONE), Instant::now());
        assert!(app.menu.is_none());
        assert_eq!(text_of(&app), "hello world");
        assert!(!app.editor().search().is_open());
        let id = active_id(&app);
        assert_eq!(app.buffer.selection(id), Some(0..5));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_outside_click_dismisses_without_moving_the_caret() {
        let dir = temp_dir("menu-outside");
        let mut app = app_with_hello_selected(&dir);
        right_click(&mut app, 10, 1);
        draw(&mut app);
        assert_eq!(app.hits.hit(4, 1), Some(Target::Editor));
        left_click(&mut app, 4, 1);
        assert!(app.menu.is_none());
        // The menu's hit regions are gone until the next render.
        assert_eq!(app.hits.hit(12, menu_row(1, 0)), None);
        let id = active_id(&app);
        assert_eq!(app.buffer.selection(id), Some(0..5));
        assert!(!app.buffer.is_dragging());

        // A right-click outside dismisses too, without reopening.
        draw(&mut app);
        right_click(&mut app, 10, 1);
        assert!(app.menu.is_some());
        draw(&mut app);
        right_click(&mut app, 4, 1);
        assert!(app.menu.is_none());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn hovering_an_item_highlights_it() {
        let dir = temp_dir("menu-hover");
        let mut app = app_with_text(&dir, "hello", KeyboardMode::Enhanced);
        right_click(&mut app, 10, 1);
        draw(&mut app);
        app.handle(
            mouse(MouseEventKind::Moved, 12, menu_row(1, 2)),
            Instant::now(),
        );
        assert_eq!(app.menu.as_ref().unwrap().highlighted(), Some(2));
        app.handle(mouse(MouseEventKind::Moved, 40, 20), Instant::now());
        assert_eq!(app.menu.as_ref().unwrap().highlighted(), None);
        assert_eq!(app.hovered, Some(Target::Editor));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_wheel_paste_and_resize_while_the_menu_is_open() {
        let dir = temp_dir("menu-modal");
        let file = long_file(&dir);
        let mut app = app(&dir, &[file], KeyboardMode::Enhanced);
        draw(&mut app);
        right_click(&mut app, 40, 10);
        draw(&mut app);
        app.handle(mouse(MouseEventKind::ScrollDown, 70, 20), Instant::now());
        app.handle(
            AppEvent::Input(Event::Paste("zz".to_owned())),
            Instant::now(),
        );
        assert!(app.menu.is_some());
        let buf = draw(&mut app);
        assert!(row_text(&buf, 1).starts_with("  1 line 1 "));
        assert!(!text_of(&app).contains("zz"));

        app.handle(AppEvent::Input(Event::Resize(100, 30)), Instant::now());
        assert!(app.menu.is_none());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_menu_is_drawn_and_hides_the_frame_cursor() {
        let dir = temp_dir("menu-draw");
        let mut app = app_with_text(&dir, "hello world", KeyboardMode::Enhanced);
        assert!(frame_cursor(&mut app).is_some());
        right_click(&mut app, 10, 1);
        let buf = draw(&mut app);
        let labels = [
            "Cut",
            "Copy",
            "Paste",
            "Select all",
            "",
            "Find\u{2026}",
            "Find and replace\u{2026}",
            "",
            "Save",
        ];
        for (index, label) in (0u16..).zip(labels) {
            let row = row_text(&buf, menu_row(1, index));
            let cells: String = row.chars().skip(12).take(label.chars().count()).collect();
            assert_eq!(cells, label, "{row:?}");
        }
        assert_eq!(frame_cursor(&mut app), None);

        app.handle(key(KeyCode::Esc, KeyModifiers::NONE), Instant::now());
        assert!(frame_cursor(&mut app).is_some());
        fs::remove_dir_all(&dir).unwrap();
    }

    fn alt(c: char) -> AppEvent {
        key(KeyCode::Char(c), KeyModifiers::ALT)
    }

    /// The screen column of char `n` of a find-bar field, which starts
    /// right after its label.
    fn field_column(n: usize) -> u16 {
        u16::try_from(" Find ".len() + n).unwrap()
    }

    #[test]
    fn ctrl_f_draws_the_bar_and_focuses_the_find_field() {
        let dir = temp_dir("find-open");
        let mut app = app_with_text(&dir, "hello world", KeyboardMode::Enhanced);
        app.handle(ctrl('f'), Instant::now());
        assert_eq!(app.focus, Focus::Find(FindField::Pattern));
        let buf = draw(&mut app);
        assert!(row_text(&buf, 1).starts_with(" Find "));
        assert!(row_text(&buf, 2).starts_with(" 1 hello world"));
        assert_eq!(
            frame_cursor(&mut app),
            Some(Position::new(field_column(0), 1))
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ctrl_e_from_the_bar_focuses_the_editor_without_a_sidebar() {
        let dir = temp_dir("find-ctrl-e");
        let mut app = app_with_text(&dir, "hello world", KeyboardMode::Enhanced);
        app.handle(ctrl('f'), Instant::now());
        assert_eq!(app.focus, Focus::Find(FindField::Pattern));
        app.handle(ctrl('e'), Instant::now());
        assert_eq!(app.focus, Focus::Editor);
        assert!(app.editor().search().is_open());
        let buf = draw(&mut app);
        assert!(row_text(&buf, 1).starts_with(" Find "));
        // The caret at the start of the first line, past the gutter " 1 ".
        assert_eq!(frame_cursor(&mut app), Some(Position { x: 3, y: 2 }));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ctrl_e_from_the_bar_focuses_the_editor_not_the_sidebar() {
        let dir = temp_dir("find-ctrl-e-sidebar");
        let ws = dir.join("ws");
        fs::create_dir_all(&ws).unwrap();
        let editor = open_editor(&dir, &[], Some(&ws));
        let mut app = TermApp::new(editor, KeyboardMode::Enhanced, ws.clone(), None);
        draw(&mut app);
        assert!(app.sidebar_shown());
        app.handle(ctrl('f'), Instant::now());
        assert_eq!(app.focus, Focus::Find(FindField::Pattern));
        app.handle(ctrl('e'), Instant::now());
        assert_eq!(app.focus, Focus::Editor);
        assert!(app.editor().search().is_open());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn typing_in_the_find_field_searches_and_highlights() {
        let dir = temp_dir("find-typing");
        let mut app = app_with_text(&dir, "hello world", KeyboardMode::Enhanced);
        app.handle(ctrl('f'), Instant::now());
        type_text(&mut app, "wor");
        assert_eq!(app.editor().search().query().pattern, "wor");
        assert_eq!(text_of(&app), "hello world");
        let buf = draw(&mut app);
        assert!(row_text(&buf, 1).contains("1/1"), "{:?}", row_text(&buf, 1));
        let row = row_text(&buf, 2);
        let start = column_of(&row, "world");
        for x in 0..3 {
            assert_eq!(
                buf[(start + x, 2)].bg,
                theme::match_bg(MatchBackground::Current)
            );
        }
        assert_eq!(
            frame_cursor(&mut app),
            Some(Position::new(field_column(3), 1))
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn enter_shift_enter_and_f3_step_from_the_find_field() {
        let dir = temp_dir("find-step");
        let mut app = app_with_text(&dir, "hello hello hello", KeyboardMode::Enhanced);
        let id = active_id(&app);
        app.handle(ctrl('f'), Instant::now());
        type_text(&mut app, "hello");
        assert_eq!(app.editor().search().count_label(), "1/3");

        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, 1);
        assert_eq!(app.buffer.selection(id), Some(6..11));
        assert_eq!(app.focus, Focus::Find(FindField::Pattern));
        assert_eq!(app.editor().search().count_label(), "2/3");

        press(&mut app, KeyCode::Enter, KeyModifiers::SHIFT, 1);
        assert_eq!(app.buffer.selection(id), Some(0..5));
        assert_eq!(app.focus, Focus::Find(FindField::Pattern));

        press(&mut app, KeyCode::F(3), KeyModifiers::NONE, 1);
        assert_eq!(app.buffer.selection(id), Some(6..11));
        assert_eq!(app.focus, Focus::Find(FindField::Pattern));
        assert_eq!(app.editor().search().query().pattern, "hello");
        assert_eq!(text_of(&app), "hello hello hello");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn alt_c_toggles_match_case_from_the_find_field() {
        let dir = temp_dir("find-case");
        let mut app = app_with_text(&dir, "Hello hello", KeyboardMode::Enhanced);
        app.handle(ctrl('f'), Instant::now());
        type_text(&mut app, "hello");
        assert_eq!(app.editor().search().count_label(), "1/2");
        app.handle(alt('c'), Instant::now());
        assert!(app.editor().search().query().case_sensitive);
        assert_eq!(app.editor().search().count_label(), "1/1");
        assert_eq!(app.editor().search().query().pattern, "hello");
        assert_eq!(app.focus, Focus::Find(FindField::Pattern));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn replace_from_the_with_field_is_undoable_in_the_editor() {
        let dir = temp_dir("find-replace");
        let mut app = app_with_text(&dir, "hello world", KeyboardMode::Enhanced);
        app.handle(ctrl('h'), Instant::now());
        assert_eq!(app.focus, Focus::Find(FindField::Pattern));
        let buf = draw(&mut app);
        assert!(row_text(&buf, 1).starts_with(" Find "));
        assert!(row_text(&buf, 2).starts_with(" With "));
        assert!(row_text(&buf, 3).starts_with(" 1 hello world"));

        type_text(&mut app, "world");
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, 1);
        assert_eq!(app.focus, Focus::Find(FindField::Replacement));
        type_text(&mut app, "bye");
        assert_eq!(app.editor().search().replacement(), "bye");
        assert_eq!(app.editor().search().query().pattern, "world");

        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, 1);
        assert_eq!(text_of(&app), "hello bye");
        assert_eq!(app.focus, Focus::Find(FindField::Replacement));

        draw(&mut app);
        left_click(&mut app, 5, 3);
        assert_eq!(app.focus, Focus::Editor);
        app.handle(ctrl('z'), Instant::now());
        assert_eq!(text_of(&app), "hello world");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn replace_selects_the_next_match_and_redo_restores_it() {
        let dir = temp_dir("find-replace-select-next");
        let mut app = app_with_text(&dir, "hello hello hello", KeyboardMode::Enhanced);
        let id = active_id(&app);
        app.handle(ctrl('h'), Instant::now());
        type_text(&mut app, "hello");
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, 1);
        type_text(&mut app, "bye");

        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, 1);
        assert_eq!(text_of(&app), "bye hello hello");
        assert_eq!(app.buffer.selection(id), Some(4..9));

        draw(&mut app);
        left_click(&mut app, 5, 3);
        assert_eq!(app.focus, Focus::Editor);
        app.handle(ctrl('z'), Instant::now());
        assert_eq!(text_of(&app), "hello hello hello");
        app.handle(
            key(
                KeyCode::Char('Z'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ),
            Instant::now(),
        );
        assert_eq!(text_of(&app), "bye hello hello");
        assert_eq!(app.buffer.selection(id), Some(4..9));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn hiding_the_replace_row_moves_focus_to_the_find_field() {
        let dir = temp_dir("find-hide-replace");
        let mut app = app_with_text(&dir, "hello world", KeyboardMode::Enhanced);
        app.handle(ctrl('h'), Instant::now());
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, 1);
        assert_eq!(app.focus, Focus::Find(FindField::Replacement));

        // Ctrl+F switches the open bar to find only.
        app.handle(ctrl('f'), Instant::now());
        assert!(!app.editor().search().replace_open());
        assert_eq!(app.focus, Focus::Find(FindField::Pattern));
        let buf = draw(&mut app);
        assert!(row_text(&buf, 1).starts_with(" Find "));
        assert!(!row_text(&buf, 2).starts_with(" With "));
        assert!(row_text(&buf, 2).starts_with(" 1 hello world"));

        // So does the find row's [Replace] toggle.
        app.handle(ctrl('h'), Instant::now());
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, 1);
        assert_eq!(app.focus, Focus::Find(FindField::Replacement));
        let toggle = drawn_column(&mut app, 1, "[Replace]");
        left_click(&mut app, toggle, 1);
        assert!(!app.editor().search().replace_open());
        assert_eq!(app.focus, Focus::Find(FindField::Pattern));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn esc_closes_the_bar_and_returns_focus_to_the_editor() {
        let dir = temp_dir("find-esc");
        let mut app = app_with_text(&dir, "hello world", KeyboardMode::Enhanced);
        app.handle(ctrl('f'), Instant::now());
        type_text(&mut app, "wor");
        assert_eq!(frame_cursor(&mut app).unwrap().y, 1);

        press(&mut app, KeyCode::Esc, KeyModifiers::NONE, 1);
        assert!(!app.editor().search().is_open());
        assert_eq!(app.focus, Focus::Editor);
        let buf = draw(&mut app);
        assert!(row_text(&buf, 1).starts_with(" 1 hello world"));
        // The caret never moved: it is at the start of the text.
        assert_eq!(frame_cursor(&mut app), Some(Position::new(3, 1)));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_seeded_pattern_puts_the_field_cursor_at_its_end() {
        let dir = temp_dir("find-seed-cursor");
        let mut app = app_with_hello_selected(&dir);
        app.handle(ctrl('f'), Instant::now());
        assert_eq!(app.editor().search().query().pattern, "hello");
        assert_eq!(app.focus, Focus::Find(FindField::Pattern));
        assert_eq!(
            frame_cursor(&mut app),
            Some(Position::new(field_column(5), 1))
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn in_selection_from_the_find_field_uses_the_editor_selection() {
        let dir = temp_dir("find-in-selection");
        let mut app = app_with_text(&dir, "a a\na a", KeyboardMode::Enhanced);
        press(&mut app, KeyCode::End, KeyModifiers::SHIFT, 1);
        assert_eq!(app.buffer.selection(active_id(&app)), Some(0..3));
        app.handle(ctrl('f'), Instant::now());
        assert_eq!(app.editor().search().query().pattern, "a a");
        press(&mut app, KeyCode::Backspace, KeyModifiers::NONE, 3);
        assert_eq!(app.editor().search().query().pattern, "");
        type_text(&mut app, "a");
        assert_eq!(app.editor().search().count_label(), "1/4");

        app.handle(alt('l'), Instant::now());
        assert!(app.editor().search().in_selection());
        assert_eq!(app.editor().search().count_label(), "1/2");
        assert!(app.editor().search().matches().iter().all(|m| m.end <= 3));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn in_selection_from_the_sidebar_uses_the_editor_selection() {
        let dir = temp_dir("find-in-selection-sidebar");
        let ws = dir.join("ws");
        fs::create_dir_all(&ws).unwrap();
        let file = ws.join("text.txt");
        fs::write(&file, "a a\na a").unwrap();
        let editor = open_editor(&dir, std::slice::from_ref(&file), Some(&ws));
        let mut app = TermApp::new(editor, KeyboardMode::Enhanced, ws.clone(), None);
        draw(&mut app);
        assert!(app.sidebar_shown());
        press(&mut app, KeyCode::End, KeyModifiers::SHIFT, 1);
        assert_eq!(app.buffer.selection(active_id(&app)), Some(0..3));
        app.handle(ctrl('f'), Instant::now());
        press(&mut app, KeyCode::Backspace, KeyModifiers::NONE, 3);
        type_text(&mut app, "a");
        assert_eq!(app.editor().search().count_label(), "1/4");
        app.handle(ctrl('e'), Instant::now());
        assert_eq!(app.focus, Focus::Editor);
        app.handle(ctrl('e'), Instant::now());
        assert_eq!(app.focus, Focus::Sidebar);

        let x = drawn_column(&mut app, 1, "[sel]");
        left_click(&mut app, x, 1);
        assert!(app.editor().search().in_selection());
        assert_eq!(app.editor().search().count_label(), "1/2");
        assert!(app.editor().search().matches().iter().all(|m| m.end <= 3));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn typing_before_an_in_selection_scope_keeps_the_matches_inside_it() {
        let dir = temp_dir("find-in-selection-edit");
        let mut app = app_with_text(&dir, "a a\na a", KeyboardMode::Enhanced);
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, 1);
        press(&mut app, KeyCode::End, KeyModifiers::SHIFT, 1);
        assert_eq!(app.buffer.selection(active_id(&app)), Some(4..7));
        app.handle(ctrl('f'), Instant::now());
        assert_eq!(app.editor().search().query().pattern, "a a");
        press(&mut app, KeyCode::Backspace, KeyModifiers::NONE, 3);
        type_text(&mut app, "a");
        app.handle(alt('l'), Instant::now());
        assert_eq!(app.editor().search().count_label(), "1/2");

        // Line 1 is on row 2 below the open bar.
        draw(&mut app);
        left_click(&mut app, 3, 2);
        assert_eq!(app.focus, Focus::Editor);
        type_text(&mut app, "xx");

        let text = text_of(&app);
        assert_eq!(text, "xxa a\na a");
        assert_eq!(app.editor().search().count_label(), "1/2");
        let line_two = text.find('\n').unwrap() + 1..text.len();
        for m in app.editor().search().matches() {
            assert!(
                line_two.start <= m.start && m.end <= line_two.end,
                "{m:?} outside {line_two:?}"
            );
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn typing_repeated_text_before_an_in_selection_scope_moves_it() {
        let dir = temp_dir("find-in-selection-repeated");
        let mut app = app_with_text(&dir, "foo foo foo", KeyboardMode::Enhanced);
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, 4);
        press(&mut app, KeyCode::Right, KeyModifiers::SHIFT, 3);
        assert_eq!(app.buffer.selection(active_id(&app)), Some(4..7));
        app.handle(ctrl('f'), Instant::now());
        assert_eq!(app.editor().search().query().pattern, "foo");
        app.handle(alt('l'), Instant::now());
        assert_eq!(app.editor().search().count_label(), "1/1");

        // The start of the text is on row 2 below the open bar.
        draw(&mut app);
        left_click(&mut app, 3, 2);
        assert_eq!(app.focus, Focus::Editor);
        type_text(&mut app, "foo ");

        assert_eq!(text_of(&app), "foo foo foo foo");
        assert_eq!(app.editor().search().count_label(), "1/1");
        assert_eq!(
            app.editor().search().matches(),
            std::slice::from_ref(&(8..11))
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn pasting_repeated_text_after_an_in_selection_scope_leaves_it() {
        let dir = temp_dir("find-in-selection-repeated-paste");
        let mut app = app_with_text(&dir, "foo foo foo", KeyboardMode::Enhanced);
        press(&mut app, KeyCode::Right, KeyModifiers::NONE, 4);
        press(&mut app, KeyCode::Right, KeyModifiers::SHIFT, 3);
        app.handle(ctrl('f'), Instant::now());
        app.handle(alt('l'), Instant::now());
        assert_eq!(
            app.editor().search().matches(),
            std::slice::from_ref(&(4..7))
        );

        // Click just past the scope (char 7, on row 2 below the open bar)
        // and paste: only the caret after the paste tells core the text
        // went after the scope rather than before it.
        draw(&mut app);
        left_click(&mut app, 10, 2);
        assert_eq!(app.focus, Focus::Editor);
        app.handle(
            AppEvent::Input(Event::Paste(" foo".to_owned())),
            Instant::now(),
        );

        assert_eq!(text_of(&app), "foo foo foo foo");
        assert_eq!(
            app.editor().search().matches(),
            std::slice::from_ref(&(4..7))
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn legacy_ctrl_h_in_the_find_field_deletes_a_char() {
        let dir = temp_dir("find-legacy-ctrl-h");
        let mut app = app_with_text(&dir, "abc", KeyboardMode::Legacy);
        app.handle(ctrl('f'), Instant::now());
        type_text(&mut app, "ab");
        assert_eq!(app.editor().search().query().pattern, "ab");
        app.handle(ctrl('h'), Instant::now());
        assert_eq!(app.editor().search().query().pattern, "a");
        assert_eq!(app.focus, Focus::Find(FindField::Pattern));
        assert_eq!(text_of(&app), "abc");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_regex_error_draws_its_rows_and_moves_the_editor_down() {
        let dir = temp_dir("find-regex-error");
        let mut app = app_with_text(&dir, "hello world", KeyboardMode::Enhanced);
        app.handle(ctrl('f'), Instant::now());
        app.handle(alt('r'), Instant::now());
        type_text(&mut app, "(");
        assert!(app.editor().search().error().is_some());
        let errors = u16::try_from(find_bar::error_lines(app.editor().search()).len()).unwrap();
        assert!(errors > 0);

        let buf = draw(&mut app);
        assert!(row_text(&buf, 1).starts_with(" Find "));
        assert!(
            row_text(&buf, 2).starts_with(" regex:"),
            "{:?}",
            row_text(&buf, 2)
        );
        for y in 2..=1 + errors {
            assert!(!row_text(&buf, y).starts_with(" 1 "), "{y}");
        }
        assert!(row_text(&buf, 2 + errors).starts_with(" 1 hello world"));
        fs::remove_dir_all(&dir).unwrap();
    }

    /// The column of `needle` on drawn row `y`.
    fn drawn_column(app: &mut TermApp, y: u16, needle: &str) -> u16 {
        let buf = draw(app);
        column_of(&row_text(&buf, y), needle)
    }

    #[test]
    fn clicking_the_find_field_focuses_it_at_the_clicked_column() {
        let dir = temp_dir("find-click-field");
        let mut app = app_with_text(&dir, "hello world", KeyboardMode::Enhanced);
        app.handle(ctrl('f'), Instant::now());
        type_text(&mut app, "hello");
        draw(&mut app);
        left_click(&mut app, 5, 2);
        assert_eq!(app.focus, Focus::Editor);

        draw(&mut app);
        left_click(&mut app, field_column(2), 1);
        assert_eq!(app.focus, Focus::Find(FindField::Pattern));
        assert_eq!(
            frame_cursor(&mut app),
            Some(Position::new(field_column(2), 1))
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn clicking_match_case_toggles_it_and_keeps_focus() {
        let dir = temp_dir("find-click-case");
        let mut app = app_with_text(&dir, "hello world", KeyboardMode::Enhanced);
        app.handle(ctrl('f'), Instant::now());
        let x = drawn_column(&mut app, 1, "[Aa]");
        left_click(&mut app, x, 1);
        assert!(app.editor().search().query().case_sensitive);
        assert_eq!(app.focus, Focus::Find(FindField::Pattern));

        draw(&mut app);
        left_click(&mut app, 5, 2);
        assert_eq!(app.focus, Focus::Editor);
        draw(&mut app);
        left_click(&mut app, x, 1);
        assert!(!app.editor().search().query().case_sensitive);
        assert_eq!(app.focus, Focus::Editor);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn clicking_next_and_prev_steps_the_match() {
        let dir = temp_dir("find-click-step");
        let mut app = app_with_text(&dir, "hello hello hello", KeyboardMode::Enhanced);
        let id = active_id(&app);
        app.handle(ctrl('f'), Instant::now());
        type_text(&mut app, "hello");
        let next = drawn_column(&mut app, 1, "[Next]");
        left_click(&mut app, next, 1);
        assert_eq!(app.buffer.selection(id), Some(6..11));
        assert_eq!(app.focus, Focus::Find(FindField::Pattern));

        let prev = drawn_column(&mut app, 1, "[Prev]");
        left_click(&mut app, prev, 1);
        assert_eq!(app.buffer.selection(id), Some(0..5));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn clicking_replace_shows_the_replace_row() {
        let dir = temp_dir("find-click-replace-row");
        let mut app = app_with_text(&dir, "hello world", KeyboardMode::Enhanced);
        app.handle(ctrl('f'), Instant::now());
        let x = drawn_column(&mut app, 1, "[Replace]");
        left_click(&mut app, x, 1);
        assert!(app.editor().search().replace_open());
        let buf = draw(&mut app);
        assert!(row_text(&buf, 2).starts_with(" With "));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn clicking_close_closes_the_bar() {
        let dir = temp_dir("find-click-close");
        let mut app = app_with_text(&dir, "hello world", KeyboardMode::Enhanced);
        app.handle(ctrl('f'), Instant::now());
        let x = drawn_column(&mut app, 1, "[x]");
        left_click(&mut app, x, 1);
        assert!(!app.editor().search().is_open());
        let buf = draw(&mut app);
        assert!(row_text(&buf, 1).starts_with(" 1 hello world"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn clicking_replace_all_replaces_every_match() {
        let dir = temp_dir("find-click-replace-all");
        let mut app = app_with_text(&dir, "a b a", KeyboardMode::Enhanced);
        app.handle(ctrl('h'), Instant::now());
        type_text(&mut app, "a");
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, 1);
        type_text(&mut app, "x");
        let x = drawn_column(&mut app, 2, "[Replace all]");
        left_click(&mut app, x, 2);
        assert_eq!(text_of(&app), "x b x");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn hovering_regex_shows_its_hint() {
        let dir = temp_dir("find-hover-regex");
        let mut app = app_with_text(&dir, "hello world", KeyboardMode::Enhanced);
        app.handle(ctrl('f'), Instant::now());
        let x = drawn_column(&mut app, 1, "[.*]");
        app.handle(mouse(MouseEventKind::Moved, x, 1), Instant::now());
        assert_eq!(app.status.hint(), Some("Regular expression (Alt+R)"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_paste_into_the_find_field_sets_the_pattern() {
        let dir = temp_dir("find-paste");
        let mut app = app_with_text(&dir, "hello world", KeyboardMode::Enhanced);
        app.handle(ctrl('f'), Instant::now());
        app.handle(
            AppEvent::Input(Event::Paste("wor".to_owned())),
            Instant::now(),
        );
        assert_eq!(app.editor().search().query().pattern, "wor");
        assert_eq!(app.editor().search().count_label(), "1/1");
        assert_eq!(text_of(&app), "hello world");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_paste_with_the_editor_focused_goes_to_the_buffer() {
        let dir = temp_dir("find-paste-editor");
        let mut app = app_with_text(&dir, "hello world", KeyboardMode::Enhanced);
        app.handle(ctrl('f'), Instant::now());
        type_text(&mut app, "hello");
        draw(&mut app);
        left_click(&mut app, 3, 2);
        assert_eq!(app.focus, Focus::Editor);
        app.handle(
            AppEvent::Input(Event::Paste("zz".to_owned())),
            Instant::now(),
        );
        assert_eq!(text_of(&app), "zzhello world");
        assert_eq!(app.editor().search().query().pattern, "hello");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn clicks_on_the_bar_with_a_prompt_open_do_nothing_to_it() {
        let dir = temp_dir("find-click-prompt");
        let mut app = app_with_text(&dir, "hello world", KeyboardMode::Enhanced);
        app.handle(ctrl('f'), Instant::now());
        let x = drawn_column(&mut app, 1, "[Aa]");
        app.handle(ctrl('o'), Instant::now());
        assert!(app.prompt.is_open(app.editor()));
        let focus = app.focus;
        draw(&mut app);
        left_click(&mut app, x, 1);
        assert!(!app.editor().search().query().case_sensitive);
        assert_eq!(app.focus, focus);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn clicking_the_with_field_focuses_it() {
        let dir = temp_dir("find-click-with");
        let mut app = app_with_text(&dir, "hello world", KeyboardMode::Enhanced);
        app.handle(ctrl('h'), Instant::now());
        draw(&mut app);
        left_click(&mut app, field_column(0), 2);
        assert_eq!(app.focus, Focus::Find(FindField::Replacement));
        assert_eq!(
            frame_cursor(&mut app),
            Some(Position::new(field_column(0), 2))
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn clicking_replace_replaces_the_current_match() {
        let dir = temp_dir("find-click-replace-current");
        let mut app = app_with_text(&dir, "hello hello", KeyboardMode::Enhanced);
        app.handle(ctrl('h'), Instant::now());
        type_text(&mut app, "hello");
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, 1);
        type_text(&mut app, "bye");
        let x = drawn_column(&mut app, 2, "[Replace]");
        left_click(&mut app, x, 2);
        assert_eq!(text_of(&app), "bye hello");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn replace_all_is_undoable_in_the_editor() {
        let dir = temp_dir("find-replace-all-undo");
        let mut app = app_with_text(&dir, "hello hello", KeyboardMode::Enhanced);
        app.handle(ctrl('h'), Instant::now());
        type_text(&mut app, "hello");
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, 1);
        type_text(&mut app, "bye");
        let x = drawn_column(&mut app, 2, "[Replace all]");
        left_click(&mut app, x, 2);
        assert_eq!(text_of(&app), "bye bye");

        // The text is on row 3, below the find and replace rows.
        draw(&mut app);
        left_click(&mut app, 5, 3);
        assert_eq!(app.focus, Focus::Editor);
        app.handle(ctrl('z'), Instant::now());
        assert_eq!(text_of(&app), "hello hello");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn replace_all_collapses_the_editor_selection() {
        let dir = temp_dir("find-replace-all-selection");
        let mut app = app_with_text(&dir, "foo tail foo", KeyboardMode::Enhanced);
        let id = active_id(&app);
        press(&mut app, KeyCode::Right, KeyModifiers::SHIFT, 3);
        assert_eq!(app.buffer.selection(id), Some(0..3));
        app.handle(ctrl('h'), Instant::now());
        assert_eq!(app.editor().search().query().pattern, "foo");
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, 1);
        type_text(&mut app, "X");
        let x = drawn_column(&mut app, 2, "[Replace all]");
        left_click(&mut app, x, 2);
        assert_eq!(text_of(&app), "X tail X");
        // The caret sits at the end of the diffed change, not over the
        // old selection.
        assert_eq!(app.buffer.selection(id), Some(8..8));

        app.handle(ctrl('e'), Instant::now());
        assert_eq!(app.focus, Focus::Editor);
        type_text(&mut app, "!");
        assert_eq!(text_of(&app), "X tail X!");

        app.handle(ctrl('z'), Instant::now());
        assert_eq!(text_of(&app), "X tail X");
        app.handle(ctrl('z'), Instant::now());
        assert_eq!(text_of(&app), "foo tail foo");
        app.handle(ctrl('y'), Instant::now());
        assert_eq!(text_of(&app), "X tail X");
        assert_eq!(app.buffer.selection(id), Some(8..8));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ctrl_f_from_the_sidebar_focuses_the_find_field() {
        let dir = temp_dir("find-from-sidebar");
        let ws = dir.join("ws");
        fs::create_dir_all(&ws).unwrap();
        let editor = open_editor(&dir, &[], Some(&ws));
        let mut app = TermApp::new(editor, KeyboardMode::Enhanced, ws.clone(), None);
        draw(&mut app);
        assert!(app.sidebar_shown());
        app.handle(ctrl('e'), Instant::now());
        assert_eq!(app.focus, Focus::Sidebar);
        app.handle(ctrl('f'), Instant::now());
        assert!(app.editor().search().is_open());
        assert_eq!(app.focus, Focus::Find(FindField::Pattern));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_find_cursor_is_hidden_under_a_prompt() {
        let dir = temp_dir("find-cursor-prompt");
        let mut app = app_with_text(&dir, "hello world", KeyboardMode::Enhanced);
        app.handle(ctrl('f'), Instant::now());
        assert_eq!(frame_cursor(&mut app).unwrap().y, 1);
        app.handle(ctrl('o'), Instant::now());
        assert!(app.prompt.is_open(app.editor()));
        assert_eq!(app.focus, Focus::Find(FindField::Pattern));
        let cursor = frame_cursor(&mut app).unwrap();
        assert_ne!(cursor.y, 1, "{cursor:?}");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_find_cursor_is_hidden_under_the_menu() {
        let dir = temp_dir("find-cursor-menu");
        let mut app = app_with_text(&dir, "hello world", KeyboardMode::Enhanced);
        app.handle(ctrl('f'), Instant::now());
        draw(&mut app);
        right_click(&mut app, 10, 2);
        assert!(app.menu.is_some());
        // Opening the menu focuses the editor, so no input reaches it with
        // a find field focused; set that state directly.
        app.focus = Focus::Find(FindField::Pattern);
        assert_eq!(frame_cursor(&mut app), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_undrawn_field_loses_the_focus() {
        let dir = temp_dir("find-undrawn");
        let mut app = app_with_text(&dir, "hello world", KeyboardMode::Enhanced);
        // Tab bar, find row, status line: no room for the replace row.
        app.handle(ctrl('h'), Instant::now());
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, 1);
        assert_eq!(app.focus, Focus::Find(FindField::Replacement));
        draw_sized(&mut app, 80, 3);
        assert_eq!(app.focus, Focus::Find(FindField::Pattern));
        assert_eq!(
            frame_cursor_sized(&mut app, 80, 3),
            Some(Position::new(field_column(0), 1))
        );

        // Tab bar and status line only: no find row either.
        app.handle(key(KeyCode::Esc, KeyModifiers::NONE), Instant::now());
        app.handle(ctrl('f'), Instant::now());
        assert_eq!(app.focus, Focus::Find(FindField::Pattern));
        draw_sized(&mut app, 80, 2);
        assert_eq!(app.focus, Focus::Editor);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ctrl_f_and_ctrl_h_from_the_bar_do_not_reseed_the_pattern() {
        let dir = temp_dir("find-no-reseed");
        let mut app = app_with_text(&dir, "hello world", KeyboardMode::Enhanced);
        let id = active_id(&app);
        app.handle(ctrl('f'), Instant::now());
        app.handle(alt('r'), Instant::now());
        type_text(&mut app, "h.llo");
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, 1);
        assert_eq!(app.buffer.selection(id), Some(0..5));

        app.handle(ctrl('f'), Instant::now());
        assert_eq!(app.editor().search().query().pattern, "h.llo");
        assert!(app.editor().search().query().regex);
        assert_eq!(app.focus, Focus::Find(FindField::Pattern));
        assert_eq!(
            frame_cursor(&mut app),
            Some(Position::new(field_column(5), 1))
        );

        app.handle(ctrl('h'), Instant::now());
        assert!(app.editor().search().replace_open());
        assert_eq!(app.editor().search().query().pattern, "h.llo");
        assert_eq!(app.focus, Focus::Find(FindField::Pattern));

        app.handle(ctrl('f'), Instant::now());
        assert!(!app.editor().search().replace_open());
        assert_eq!(app.editor().search().query().pattern, "h.llo");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_prompt_hides_the_hint_and_what_is_under_it_hovers_nothing() {
        let dir = temp_dir("find-hover-prompt");
        let mut app = app_with_text(&dir, "hello world", KeyboardMode::Enhanced);
        app.handle(ctrl('f'), Instant::now());
        let x = drawn_column(&mut app, 1, "[.*]");
        app.handle(mouse(MouseEventKind::Moved, x, 1), Instant::now());
        assert_eq!(app.status.hint(), Some("Regular expression (Alt+R)"));

        app.handle(ctrl('o'), Instant::now());
        assert_eq!(app.status.hint(), None);
        draw(&mut app);
        assert_eq!(
            app.hits.hit(x, 1),
            Some(Target::FindOption(SearchOption::Regex))
        );
        app.handle(mouse(MouseEventKind::Moved, x, 1), Instant::now());
        assert_eq!(app.hovered, None);
        assert_eq!(app.status.hint(), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    /// An app over a workspace `ws` holding `docs/`, `src/main.rs`,
    /// `Cargo.toml` and `README.md`, drawn once. Its tree rows are 0
    /// `docs`, 1 `src`, 2 `Cargo.toml` and 3 `README.md`, drawn from
    /// screen row 2 (under the tab bar and the tree's header).
    fn tree_app(tag: &str) -> (PathBuf, PathBuf, TermApp) {
        let dir = temp_dir(tag);
        let ws = dir.join("ws");
        fs::create_dir_all(ws.join("docs")).unwrap();
        fs::create_dir_all(ws.join("src")).unwrap();
        fs::write(ws.join("src").join("main.rs"), "fn main() {}\n").unwrap();
        fs::write(ws.join("Cargo.toml"), "[package]\n").unwrap();
        fs::write(ws.join("README.md"), "# ws\n").unwrap();
        let editor = open_editor(&dir, &[], Some(&ws));
        let mut app = TermApp::new(editor, KeyboardMode::Enhanced, ws.clone(), None);
        draw(&mut app);
        (dir, ws, app)
    }

    /// The screen row of tree row `index` with the tree scrolled to the
    /// top.
    fn tree_y(index: u16) -> u16 {
        2 + index
    }

    /// The sidebar's text at screen row `y` (the border excluded).
    fn sidebar_text(app: &TermApp, buf: &Buffer, y: u16) -> String {
        let width = app.tree.width() - 1;
        row_text(buf, y).chars().take(usize::from(width)).collect()
    }

    #[test]
    fn the_first_frame_shows_the_tree_header_and_rows() {
        let (dir, _ws, mut app) = tree_app("tree-first-frame");
        let buf = draw(&mut app);
        let root = app.editor().workspace().unwrap().display_root();
        let header = sidebar_text(&app, &buf, 1);
        // The root is cut at the border: 24 cells from column 5 to 29.
        let shown: String = root.chars().take(24).collect();
        assert!(header.starts_with(&format!(" [x] {shown}")), "{header:?}");
        assert!(sidebar_text(&app, &buf, tree_y(0)).starts_with(" > / docs "));
        assert!(sidebar_text(&app, &buf, tree_y(1)).starts_with(" > / src "));
        assert!(sidebar_text(&app, &buf, tree_y(2)).starts_with("     Cargo.toml "));
        assert!(sidebar_text(&app, &buf, tree_y(3)).starts_with("     README.md "));
        assert_eq!(sidebar_text(&app, &buf, tree_y(4)).trim(), "");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn right_expands_and_left_collapses_a_folder_from_the_keyboard() {
        let (dir, ws, mut app) = tree_app("tree-expand");
        let src = ws.join("src");
        app.handle(ctrl('e'), Instant::now());
        assert_eq!(app.focus, Focus::Sidebar);
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, 2);
        assert_eq!(app.tree.selected(), Some(src.as_path()));

        app.handle(key(KeyCode::Right, KeyModifiers::NONE), Instant::now());
        assert!(app.editor().workspace().unwrap().is_expanded(&src));
        let buf = draw(&mut app);
        assert!(sidebar_text(&app, &buf, tree_y(1)).starts_with(" v / src "));
        assert!(sidebar_text(&app, &buf, tree_y(2)).starts_with("       main.rs "));
        assert!(sidebar_text(&app, &buf, tree_y(3)).starts_with("     Cargo.toml "));

        app.handle(key(KeyCode::Left, KeyModifiers::NONE), Instant::now());
        assert!(!app.editor().workspace().unwrap().is_expanded(&src));
        let buf = draw(&mut app);
        assert!(sidebar_text(&app, &buf, tree_y(1)).starts_with(" > / src "));
        assert!(sidebar_text(&app, &buf, tree_y(2)).starts_with("     Cargo.toml "));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_second_key_in_the_same_batch_sees_the_expanded_rows() {
        let (dir, ws, mut app) = tree_app("tree-batch");
        app.handle(ctrl('e'), Instant::now());
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, 2);
        // No render between expanding `src` and stepping into it.
        app.handle(key(KeyCode::Right, KeyModifiers::NONE), Instant::now());
        app.handle(key(KeyCode::Right, KeyModifiers::NONE), Instant::now());
        let main = ws.join("src").join("main.rs");
        assert_eq!(app.tree.selected(), Some(main.as_path()));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn enter_on_a_file_opens_it_and_keeps_focus_in_the_tree() {
        let (dir, ws, mut app) = tree_app("tree-enter");
        let cargo = ws.join("Cargo.toml");
        app.handle(ctrl('e'), Instant::now());
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, 3);
        assert_eq!(app.tree.selected(), Some(cargo.as_path()));

        app.handle(key(KeyCode::Enter, KeyModifiers::NONE), Instant::now());
        let tab = app.editor().active_tab().unwrap();
        assert_eq!(tab.path.as_deref(), Some(cargo.as_path()));
        assert_eq!(tab.text, "[package]\n");
        assert_eq!(app.focus, Focus::Sidebar);

        let buf = draw(&mut app);
        let y = tree_y(2);
        let x = column_of(&row_text(&buf, y), "Cargo.toml");
        assert_eq!(buf[(x, y)].fg, theme::tree_active().fg.unwrap());
        let readme = column_of(&row_text(&buf, tree_y(3)), "README.md");
        assert_ne!(
            buf[(readme, tree_y(3))].fg,
            theme::tree_active().fg.unwrap()
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn shift_f10_opens_the_file_menu_and_rename_renames_the_file() {
        let (dir, ws, mut app) = tree_app("tree-rename");
        let cargo = ws.join("Cargo.toml");
        app.handle(ctrl('e'), Instant::now());
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, 3);

        app.handle(key(KeyCode::F(10), KeyModifiers::SHIFT), Instant::now());
        // Cargo.toml's name column: the sidebar's x 0, plus 1, no indent,
        // the marker and its space (2) and the icon (2); on screen row 4.
        let anchor = Position::new(5, tree_y(2));
        assert_eq!(
            app.menu,
            Some(ContextMenu::new(tree_menu::file_entries(&cargo), anchor))
        );
        let buf = draw(&mut app);
        // The first item sits under the menu's top border.
        let first = row_text(&buf, anchor.y + 1);
        let open: String = first.chars().skip(usize::from(anchor.x)).collect();
        assert!(open.starts_with("\u{2502} Open"), "{first:?}");
        assert!((0..24).any(|y| row_text(&buf, y).contains("Rename\u{2026}")));

        // Open, Copy path, Copy relative path, Reveal, then Rename….
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, 5);
        app.handle(key(KeyCode::Enter, KeyModifiers::NONE), Instant::now());
        assert!(app.menu.is_none());
        assert!(matches!(app.editor().prompt(), Some(Prompt::Name(_))));
        let buf = draw(&mut app);
        assert!((0..24).any(|y| row_text(&buf, y).contains("New name")));

        press(
            &mut app,
            KeyCode::Backspace,
            KeyModifiers::NONE,
            "Cargo.toml".len(),
        );
        type_text(&mut app, "Renamed.toml");
        app.handle(key(KeyCode::Enter, KeyModifiers::NONE), Instant::now());
        assert!(app.editor().prompt().is_none());
        let renamed = ws.join("Renamed.toml");
        assert!(!cargo.exists());
        assert_eq!(fs::read_to_string(&renamed).unwrap(), "[package]\n");

        let buf = draw(&mut app);
        let rows: Vec<String> = (2..6).map(|y| sidebar_text(&app, &buf, y)).collect();
        assert!(
            rows.iter().any(|row| row.contains("Renamed.toml")),
            "{rows:?}"
        );
        assert!(
            !rows.iter().any(|row| row.contains("Cargo.toml")),
            "{rows:?}"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn angle_brackets_resize_the_sidebar_down_to_its_minimum() {
        let (dir, _ws, mut app) = tree_app("tree-resize");
        // The border is the sidebar's last column; the gutter " 1 "
        // starts right after it.
        let assert_gutter_at = |buf: &Buffer, x: u16| {
            assert_eq!(buf[(x - 1, 10)].bg, theme::tree_border().bg.unwrap());
            assert_ne!(buf[(x, 10)].bg, theme::tree_border().bg.unwrap());
            assert_eq!(buf[(x, 1)].symbol(), " ");
            assert_eq!(buf[(x + 1, 1)].symbol(), "1");
        };
        let start = DEFAULT_WIDTH;
        assert_gutter_at(&draw(&mut app), start);
        app.handle(ctrl('e'), Instant::now());

        app.handle(key(KeyCode::Char('>'), KeyModifiers::NONE), Instant::now());
        assert_gutter_at(&draw(&mut app), start + 1);

        app.handle(key(KeyCode::Char('<'), KeyModifiers::SHIFT), Instant::now());
        assert_gutter_at(&draw(&mut app), start);
        app.handle(key(KeyCode::Char('<'), KeyModifiers::NONE), Instant::now());
        assert_gutter_at(&draw(&mut app), start - 1);

        press(&mut app, KeyCode::Char('<'), KeyModifiers::NONE, 40);
        assert_eq!(app.tree.width(), MIN_WIDTH);
        assert_gutter_at(&draw(&mut app), MIN_WIDTH);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn esc_in_the_tree_returns_focus_to_the_editor() {
        let (dir, _ws, mut app) = tree_app("tree-esc");
        app.handle(ctrl('e'), Instant::now());
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, 1);
        assert_eq!(app.focus, Focus::Sidebar);
        app.handle(key(KeyCode::Esc, KeyModifiers::NONE), Instant::now());
        assert_eq!(app.focus, Focus::Editor);
        // Back in the editor, Down moves the caret, not the tree.
        let before = app.tree.selected().map(Path::to_path_buf);
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, 1);
        assert_eq!(app.tree.selected().map(Path::to_path_buf), before);
        fs::remove_dir_all(&dir).unwrap();
    }

    /// The `[x]` in the tree's header.
    const TREE_CLOSE: (u16, u16) = (2, 1);

    #[test]
    fn clicking_the_tree_close_closes_the_workspace() {
        let (dir, _ws, mut app) = tree_app("tree-click-close");
        for x in 1..=3 {
            assert_eq!(app.hits.hit(x, 1), Some(Target::TreeClose));
        }
        left_click(&mut app, TREE_CLOSE.0, TREE_CLOSE.1);
        assert!(app.editor().workspace().is_none());
        let buf = draw(&mut app);
        // The gutter is back at the left edge.
        assert!(
            row_text(&buf, 1).starts_with(" 1 "),
            "{:?}",
            row_text(&buf, 1)
        );
        assert_ne!(buf[(0, 10)].bg, theme::tree_border().bg.unwrap());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn clicking_a_folder_row_toggles_it_and_focuses_the_sidebar() {
        let (dir, ws, mut app) = tree_app("tree-click-folder");
        let src = ws.join("src");
        assert_eq!(app.hits.hit(5, tree_y(1)), Some(Target::TreeRow(1)));
        left_click(&mut app, 5, tree_y(1));
        assert!(app.editor().workspace().unwrap().is_expanded(&src));
        assert_eq!(app.tree.selected(), Some(src.as_path()));
        assert_eq!(app.focus, Focus::Sidebar);
        let buf = draw(&mut app);
        assert!(sidebar_text(&app, &buf, tree_y(2)).starts_with("       main.rs "));

        left_click(&mut app, 5, tree_y(1));
        assert!(!app.editor().workspace().unwrap().is_expanded(&src));
        assert_eq!(app.focus, Focus::Sidebar);
        let buf = draw(&mut app);
        assert!(sidebar_text(&app, &buf, tree_y(2)).starts_with("     Cargo.toml "));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn clicking_a_file_row_opens_it_and_focuses_the_editor() {
        let (dir, ws, mut app) = tree_app("tree-click-file");
        let cargo = ws.join("Cargo.toml");
        app.handle(ctrl('e'), Instant::now());
        assert_eq!(app.focus, Focus::Sidebar);
        draw(&mut app);
        left_click(&mut app, 8, tree_y(2));
        let tab = app.editor().active_tab().unwrap();
        assert_eq!(tab.path.as_deref(), Some(cargo.as_path()));
        assert_eq!(app.tree.selected(), Some(cargo.as_path()));
        assert_eq!(app.focus, Focus::Editor);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn clicking_the_empty_tree_area_focuses_the_sidebar() {
        let (dir, _ws, mut app) = tree_app("tree-click-empty");
        assert_eq!(app.hits.hit(5, tree_y(6)), Some(Target::TreeEmpty));
        left_click(&mut app, 5, tree_y(6));
        assert_eq!(app.focus, Focus::Sidebar);
        assert_eq!(app.tree.selected(), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn right_clicks_on_the_tree_open_its_menus_at_the_mouse() {
        let (dir, ws, mut app) = tree_app("tree-right-click");
        let cargo = ws.join("Cargo.toml");
        let src = ws.join("src");
        let root = app.editor().workspace().unwrap().root.clone();

        right_click(&mut app, 8, tree_y(2));
        assert_eq!(
            app.menu,
            Some(ContextMenu::new(
                tree_menu::file_entries(&cargo),
                Position::new(8, tree_y(2))
            ))
        );
        assert_eq!(app.tree.selected(), Some(cargo.as_path()));
        assert_eq!(app.focus, Focus::Sidebar);
        app.handle(key(KeyCode::Esc, KeyModifiers::NONE), Instant::now());

        draw(&mut app);
        right_click(&mut app, 6, tree_y(1));
        assert_eq!(
            app.menu,
            Some(ContextMenu::new(
                tree_menu::dir_entries(&src),
                Position::new(6, tree_y(1))
            ))
        );
        assert_eq!(app.tree.selected(), Some(src.as_path()));
        app.handle(key(KeyCode::Esc, KeyModifiers::NONE), Instant::now());

        // The header has no menu.
        draw(&mut app);
        right_click(&mut app, 10, 1);
        assert!(app.menu.is_none());

        draw(&mut app);
        right_click(&mut app, 4, tree_y(4));
        assert_eq!(
            app.menu,
            Some(ContextMenu::new(
                tree_menu::dir_entries(&root),
                Position::new(4, tree_y(4))
            ))
        );

        // "New file…" is the root menu's first item.
        draw(&mut app);
        let item = (6, menu_row(tree_y(4), 0));
        assert_eq!(app.hits.hit(item.0, item.1), Some(Target::MenuItem(0)));
        left_click(&mut app, item.0, item.1);
        assert!(app.menu.is_none());
        assert!(matches!(app.editor().prompt(), Some(Prompt::Name(_))));

        type_text(&mut app, "notes.txt");
        app.handle(key(KeyCode::Enter, KeyModifiers::NONE), Instant::now());
        assert!(app.editor().prompt().is_none());
        let created = root.join("notes.txt");
        assert!(created.is_file());
        let tab = app.editor().active_tab().unwrap();
        assert_eq!(tab.path.as_deref(), Some(created.as_path()));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn dragging_the_border_resizes_the_sidebar_and_clamps() {
        let (dir, _ws, mut app) = tree_app("tree-drag");
        let border = DEFAULT_WIDTH - 1;
        assert_eq!(app.hits.hit(border, 10), Some(Target::TreeBorder));
        let now = Instant::now();

        app.handle(mouse(left(MouseEventKind::Down), border, 10), now);
        assert!(app.tree.is_resizing());
        // The border follows the mouse: at column 39 the sidebar is 40 wide.
        app.handle(mouse(left(MouseEventKind::Drag), 39, 10), now);
        assert_eq!(app.tree.width(), 40);
        app.handle(mouse(left(MouseEventKind::Up), 39, 10), now);
        assert!(!app.tree.is_resizing());
        let buf = draw(&mut app);
        assert_eq!(buf[(39, 10)].bg, theme::tree_border().bg.unwrap());
        assert_ne!(buf[(40, 10)].bg, theme::tree_border().bg.unwrap());
        assert_eq!(buf[(41, 1)].symbol(), "1");

        // After the release a drag does nothing.
        app.handle(mouse(left(MouseEventKind::Drag), 50, 10), now);
        assert_eq!(app.tree.width(), 40);

        app.handle(mouse(left(MouseEventKind::Down), 39, 10), now);
        app.handle(mouse(left(MouseEventKind::Drag), 2, 10), now);
        assert_eq!(app.tree.width(), MIN_WIDTH);
        // The widest leaves the editor text 20 columns past the gutter " 1 ".
        app.handle(mouse(left(MouseEventKind::Drag), 79, 10), now);
        assert_eq!(app.tree.width(), 80 - 3 - MIN_EDITOR_WIDTH);
        app.handle(mouse(left(MouseEventKind::Up), 79, 10), now);
        assert!(!app.tree.is_resizing());
        let buf = draw(&mut app);
        assert_eq!(buf[(56, 10)].bg, theme::tree_border().bg.unwrap());
        assert_ne!(buf[(57, 10)].bg, theme::tree_border().bg.unwrap());
        // The gutter is columns 57 to 59; the text is the last 20.
        assert_eq!(buf[(58, 1)].symbol(), "1");
        assert_eq!(app.hits.hit(59, 10), None);
        assert_eq!(app.hits.hit(60, 10), Some(Target::Editor));
        assert_eq!(app.hits.hit(79, 10), Some(Target::Editor));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_release_while_the_menu_is_open_ends_a_border_drag() {
        let (dir, _ws, mut app) = tree_app("tree-drag-menu");
        app.tree.begin_resize();
        app.menu = Some(ContextMenu::new(
            tree_menu::dir_entries(Path::new("/")),
            Position::new(5, 5),
        ));
        app.handle(mouse(left(MouseEventKind::Up), 5, 5), Instant::now());
        assert!(!app.tree.is_resizing());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_move_without_a_button_ends_a_border_drag() {
        let (dir, _ws, mut app) = tree_app("tree-drag-moved");
        let border = DEFAULT_WIDTH - 1;
        let now = Instant::now();
        app.handle(mouse(left(MouseEventKind::Down), border, 10), now);
        assert!(app.tree.is_resizing());
        // A drag does not end it.
        app.handle(mouse(left(MouseEventKind::Drag), 34, 10), now);
        assert!(app.tree.is_resizing());
        assert_eq!(app.tree.width(), 35);
        draw(&mut app);
        app.handle(mouse(MouseEventKind::Moved, 34, 10), now);
        assert!(!app.tree.is_resizing());
        app.handle(mouse(left(MouseEventKind::Drag), 50, 10), now);
        assert_eq!(app.tree.width(), 35);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_press_on_the_editor_ends_a_border_drag() {
        let (dir, _ws, mut app) = tree_app("tree-drag-press");
        let border = DEFAULT_WIDTH - 1;
        let now = Instant::now();
        app.handle(mouse(left(MouseEventKind::Down), border, 10), now);
        assert!(app.tree.is_resizing());
        assert_eq!(app.hits.hit(50, 10), Some(Target::Editor));
        app.handle(mouse(left(MouseEventKind::Down), 50, 10), now);
        assert!(!app.tree.is_resizing());
        app.handle(mouse(left(MouseEventKind::Drag), 60, 10), now);
        assert_eq!(app.tree.width(), DEFAULT_WIDTH);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_move_while_the_menu_is_open_ends_a_border_drag() {
        let (dir, _ws, mut app) = tree_app("tree-drag-menu-moved");
        app.tree.begin_resize();
        app.menu = Some(ContextMenu::new(
            tree_menu::dir_entries(Path::new("/")),
            Position::new(5, 5),
        ));
        app.handle(mouse(left(MouseEventKind::Drag), 6, 6), Instant::now());
        assert!(app.tree.is_resizing());
        app.handle(mouse(MouseEventKind::Moved, 6, 6), Instant::now());
        assert!(!app.tree.is_resizing());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn narrowing_after_the_terminal_shrank_moves_the_border_at_once() {
        let (dir, _ws, mut app) = tree_app("tree-shrink");
        let border = DEFAULT_WIDTH - 1;
        let now = Instant::now();
        app.handle(mouse(left(MouseEventKind::Down), border, 10), now);
        app.handle(mouse(left(MouseEventKind::Drag), 59, 10), now);
        app.handle(mouse(left(MouseEventKind::Up), 59, 10), now);
        // 80 columns less the gutter " 1 " and the editor's 20.
        assert_eq!(app.tree.width(), 57);

        // In 40 columns the sidebar is clamped to 40 - 3 - 20 = 17, its
        // border at 16.
        let buf = draw_sized(&mut app, 40, 24);
        assert_eq!(app.tree.width(), 17);
        assert_eq!(buf[(16, 10)].bg, theme::tree_border().bg.unwrap());
        app.handle(ctrl('e'), now);
        app.handle(key(KeyCode::Char('<'), KeyModifiers::NONE), now);
        let buf = draw_sized(&mut app, 40, 24);
        assert_eq!(buf[(15, 10)].bg, theme::tree_border().bg.unwrap());
        assert_ne!(buf[(16, 10)].bg, theme::tree_border().bg.unwrap());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_tree_key_after_a_change_on_disk_drops_the_hit_map() {
        let (dir, ws, mut app) = tree_app("tree-key-refresh");
        app.handle(ctrl('e'), Instant::now());
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, 1);
        assert_eq!(app.hits.hit(5, tree_y(0)), Some(Target::TreeRow(0)));
        fs::write(ws.join("new.txt"), "").unwrap();
        // No render between the change and the key.
        press(&mut app, KeyCode::Down, KeyModifiers::NONE, 1);
        assert_eq!(app.hits.hit(5, tree_y(0)), None);
        assert_eq!(app.hits.hit(DEFAULT_WIDTH - 1, 10), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn clicking_the_tree_close_with_the_tree_focused_focuses_the_editor() {
        let (dir, _ws, mut app) = tree_app("tree-close-focused");
        app.handle(ctrl('e'), Instant::now());
        assert_eq!(app.focus, Focus::Sidebar);
        left_click(&mut app, TREE_CLOSE.0, TREE_CLOSE.1);
        assert!(app.editor().workspace().is_none());
        assert_eq!(app.focus, Focus::Editor);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_wheel_scrolls_a_long_tree_and_stops_at_the_end() {
        let dir = temp_dir("tree-wheel");
        let ws = dir.join("ws");
        fs::create_dir_all(&ws).unwrap();
        for n in 0..40 {
            fs::write(ws.join(format!("f{n:02}.txt")), "").unwrap();
        }
        let editor = open_editor(&dir, &[], Some(&ws));
        let mut app = TermApp::new(editor, KeyboardMode::Enhanced, ws.clone(), None);
        let first_row = |app: &mut TermApp| {
            let buf = draw(app);
            sidebar_text(app, &buf, tree_y(0)).trim().to_owned()
        };
        assert_eq!(first_row(&mut app), "f00.txt");

        app.handle(mouse(MouseEventKind::ScrollDown, 5, 10), Instant::now());
        assert_eq!(first_row(&mut app), "f03.txt");
        // Over the border too.
        app.handle(
            mouse(MouseEventKind::ScrollDown, DEFAULT_WIDTH - 1, 10),
            Instant::now(),
        );
        assert_eq!(first_row(&mut app), "f06.txt");

        // 21 rows fit (screen rows 2 to 22): the last top is row 19.
        for _ in 0..10 {
            app.handle(mouse(MouseEventKind::ScrollDown, 5, 10), Instant::now());
            draw(&mut app);
        }
        let buf = draw(&mut app);
        assert_eq!(sidebar_text(&app, &buf, tree_y(0)).trim(), "f19.txt");
        assert_eq!(sidebar_text(&app, &buf, 22).trim(), "f39.txt");

        app.handle(mouse(MouseEventKind::ScrollUp, 5, 10), Instant::now());
        assert_eq!(first_row(&mut app), "f16.txt");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn hovering_the_tree_close_shows_its_hint() {
        let (dir, _ws, mut app) = tree_app("tree-hover-close");
        app.handle(
            mouse(MouseEventKind::Moved, TREE_CLOSE.0, TREE_CLOSE.1),
            Instant::now(),
        );
        assert_eq!(app.status.hint(), Some("Close file tree"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn clicks_on_the_tree_with_a_prompt_open_do_nothing() {
        let (dir, ws, mut app) = tree_app("tree-click-prompt");
        let src = ws.join("src");
        let tabs = app.editor().tabs().len();
        let border = DEFAULT_WIDTH - 1;
        let events = [
            mouse(left(MouseEventKind::Down), 5, tree_y(1)),
            mouse(left(MouseEventKind::Down), 5, tree_y(2)),
            mouse(left(MouseEventKind::Down), TREE_CLOSE.0, TREE_CLOSE.1),
            mouse(left(MouseEventKind::Down), 5, tree_y(4)),
            mouse(MouseEventKind::Down(MouseButton::Right), 5, tree_y(2)),
            mouse(MouseEventKind::Down(MouseButton::Right), 5, tree_y(4)),
            mouse(left(MouseEventKind::Down), border, tree_y(1)),
            mouse(MouseEventKind::ScrollDown, 5, tree_y(1)),
        ];
        for event in events {
            // Each event meets a freshly drawn prompt: a click outside it
            // only dismisses it.
            app.handle(ctrl('o'), Instant::now());
            draw(&mut app);
            assert!(app.prompt.is_open(&app.editor));
            for (x, y) in [(5, tree_y(1)), (5, tree_y(2)), (5, tree_y(4))] {
                assert!(
                    matches!(
                        app.hits.hit(x, y),
                        Some(Target::TreeRow(_) | Target::TreeEmpty)
                    ),
                    "{x},{y}"
                );
            }
            assert_eq!(app.hits.hit(border, tree_y(1)), Some(Target::TreeBorder));
            app.handle(event, Instant::now());
            app.handle(mouse(left(MouseEventKind::Drag), 40, 10), Instant::now());
            app.handle(key(KeyCode::Esc, KeyModifiers::NONE), Instant::now());

            assert!(app.menu.is_none());
            assert!(app.editor().workspace().is_some());
            assert!(!app.editor().workspace().unwrap().is_expanded(&src));
            assert_eq!(app.editor().tabs().len(), tabs);
            assert_eq!(app.tree.selected(), None);
            assert!(!app.tree.is_resizing());
            assert_eq!(app.tree.width(), DEFAULT_WIDTH);
            assert_eq!(app.focus, Focus::Editor);
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    /// An app over a workspace of 40 files, `f00.txt` to `f39.txt`,
    /// drawn once.
    fn long_tree_app(tag: &str) -> (PathBuf, PathBuf, TermApp) {
        let dir = temp_dir(tag);
        let ws = dir.join("ws");
        fs::create_dir_all(&ws).unwrap();
        for n in 0..40 {
            fs::write(ws.join(format!("f{n:02}.txt")), "").unwrap();
        }
        let editor = open_editor(&dir, &[], Some(&ws));
        let mut app = TermApp::new(editor, KeyboardMode::Enhanced, ws.clone(), None);
        draw(&mut app);
        (dir, ws, app)
    }

    #[test]
    fn a_batch_of_wheel_events_scrolls_by_all_of_them() {
        let (dir, _ws, mut app) = long_tree_app("tree-wheel-batch");
        // No render between the three events.
        for _ in 0..3 {
            app.handle(mouse(MouseEventKind::ScrollDown, 5, 10), Instant::now());
        }
        assert_eq!(app.hits.hit(5, 10), Some(Target::TreeScrolled));
        let buf = draw(&mut app);
        assert_eq!(sidebar_text(&app, &buf, tree_y(0)).trim(), "f09.txt");
        assert_eq!(app.hits.hit(5, 10), Some(Target::TreeRow(17)));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn clicks_after_a_wheel_event_in_the_same_batch_do_nothing() {
        let (dir, ws, mut app) = long_tree_app("tree-wheel-click");
        let tabs = app.editor().tabs().len();
        app.handle(mouse(MouseEventKind::Moved, 5, tree_y(1)), Instant::now());
        assert_eq!(app.hovered, Some(Target::TreeRow(1)));
        app.handle(mouse(MouseEventKind::ScrollDown, 5, 10), Instant::now());
        assert_eq!(app.hovered, None);
        assert_eq!(app.status.hint(), None);
        // The border keeps its target.
        assert_eq!(
            app.hits.hit(DEFAULT_WIDTH - 1, 10),
            Some(Target::TreeBorder)
        );

        left_click(&mut app, 5, tree_y(1));
        right_click(&mut app, 5, tree_y(1));
        right_click(&mut app, 5, tree_y(30));
        assert_eq!(app.editor().tabs().len(), tabs);
        assert_eq!(app.tree.selected(), None);
        assert!(app.menu.is_none());
        assert_eq!(app.focus, Focus::Editor);

        // After a render the click lands on the row now drawn there.
        draw(&mut app);
        left_click(&mut app, 5, tree_y(1));
        let f04 = ws.join("f04.txt");
        assert_eq!(
            app.editor().active_tab().unwrap().path.as_deref(),
            Some(f04.as_path())
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_prompt_opened_mid_drag_ends_the_drag() {
        let (dir, _ws, mut app) = tree_app("tree-drag-prompt");
        let border = DEFAULT_WIDTH - 1;
        let now = Instant::now();
        app.handle(mouse(left(MouseEventKind::Down), border, 10), now);
        app.handle(mouse(left(MouseEventKind::Drag), 34, 10), now);
        assert_eq!(app.tree.width(), 35);
        app.handle(ctrl('o'), now);
        assert!(app.prompt.is_open(&app.editor));
        assert!(app.tree.is_resizing());

        app.handle(mouse(left(MouseEventKind::Drag), 50, 10), now);
        assert_eq!(app.tree.width(), 35);
        assert!(!app.tree.is_resizing());

        app.handle(key(KeyCode::Esc, KeyModifiers::NONE), now);
        app.handle(mouse(left(MouseEventKind::Drag), 60, 10), now);
        assert_eq!(app.tree.width(), 35);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_stale_hovered_row_is_dropped_for_good_after_a_refresh() {
        let (dir, ws, mut app) = tree_app("tree-stale-hover");
        let y = tree_y(1);
        app.handle(mouse(MouseEventKind::Moved, 5, y), Instant::now());
        assert_eq!(app.hovered, Some(Target::TreeRow(1)));
        let buf = draw(&mut app);
        assert_eq!(buf[(0, y)].bg, theme::hover_bg());

        // Removing `docs` moves `Cargo.toml` into row 1.
        fs::remove_dir_all(ws.join("docs")).unwrap();
        for _ in 0..2 {
            let buf = draw(&mut app);
            assert!(sidebar_text(&app, &buf, y).starts_with("     Cargo.toml "));
            assert_ne!(buf[(0, y)].bg, theme::hover_bg());
            assert_eq!(app.hovered, None);
            assert_eq!(app.status.hint(), None);
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    /// Squiggles, the status line and the editor menu over diagnostics.
    mod proofreading {
        use frext_core::proofread::DiagnosticKind;
        use ratatui::style::{Color, Modifier};

        use super::*;
        use crate::menu::MenuEntry;

        /// Runs one proofreading pass over the active tab and waits for it:
        /// starts the editor's worker with a notify that signals a channel,
        /// queues a check, waits for the notify, then polls. Must be the
        /// first `begin_frame` on `app` (only the first notify is kept).
        ///
        /// The worker sends each result before calling notify, so once the
        /// notify arrives the result for this one request is ready to poll.
        fn proofread(app: &mut TermApp) {
            let (tx, rx) = std::sync::mpsc::channel();
            app.begin_frame(move || {
                let _ = tx.send(());
            });
            app.end_frame();
            rx.recv_timeout(Duration::from_secs(60)).unwrap();
            app.begin_frame(|| {});
        }

        const MISSPELT: &str = "This is a tset of the editor.";

        /// Wide enough that neither a temp-dir path nor a diagnostic message
        /// is truncated on the status row.
        const STATUS_WIDTH: u16 = 300;

        /// The spelling diagnostic over `tset` in the active tab.
        fn tset_diagnostic(app: &TermApp) -> Diagnostic {
            let tab = app.editor().active_tab().unwrap();
            app.editor()
                .proofread()
                .diagnostics(tab.id, &tab.text)
                .into_iter()
                .find(|d| d.kind == DiagnosticKind::Spelling && d.flagged == "tset")
                .cloned()
                .unwrap()
        }

        /// The squiggle colour of `cell`, `None` for `Color::Reset`;
        /// asserts that a coloured cell is also underlined.
        fn underline_of(cell: &ratatui::buffer::Cell) -> Option<Color> {
            let colour = Some(cell.underline_color).filter(|&c| c != Color::Reset);
            if colour.is_some() {
                assert!(
                    cell.modifier.contains(Modifier::UNDERLINED),
                    "underline colour without an underline: {cell:?}"
                );
            }
            colour
        }

        /// The status row text the path of the active tab gives.
        fn path_status(app: &TermApp) -> String {
            let tab = app.editor().active_tab().unwrap();
            format!(
                " {} ",
                app.editor().relative_to_root(tab.path.as_deref().unwrap())
            )
        }

        #[test]
        fn proofreading_results_squiggle_on_the_next_draw() {
            let dir = temp_dir("squiggles");
            let mut app = app_with_text(&dir, MISSPELT, KeyboardMode::Enhanced);
            let buf = draw(&mut app);
            let unchecked: Vec<_> = (1..23)
                .flat_map(|y| (0..80).map(move |x| (x, y)))
                .filter(|&(x, y)| underline_of(&buf[(x, y)]).is_some())
                .collect();
            assert!(
                unchecked.is_empty(),
                "underlined before checking: {unchecked:?}"
            );

            // The worker's notify fires, `begin_frame` polls the result,
            // and the next draw shows it. The waker's hop to
            // `AppEvent::Wake` is covered by the events tests.
            proofread(&mut app);
            let diagnostic = tset_diagnostic(&app);
            let buf = draw(&mut app);
            for char_idx in diagnostic.chars.clone() {
                let x = column_of_char(char_idx);
                assert_eq!(
                    underline_of(&buf[(x, 1)]),
                    Some(theme::squiggle(DiagnosticKind::Spelling)),
                    "char {char_idx} of {diagnostic:?}"
                );
            }
            fs::remove_dir_all(&dir).unwrap();
        }

        #[test]
        fn the_status_line_shows_the_diagnostic_under_the_caret() {
            let dir = temp_dir("status-diagnostic");
            let mut app = app_with_text(&dir, MISSPELT, KeyboardMode::Enhanced);
            proofread(&mut app);
            let diagnostic = tset_diagnostic(&app);
            let tab_id = app.editor().active_tab().unwrap().id;
            assert_eq!(app.buffer.caret(tab_id), Some(0));
            assert!(
                app.editor()
                    .proofread()
                    .diagnostic_at(tab_id, MISSPELT, 0)
                    .is_none()
            );

            press(
                &mut app,
                KeyCode::Right,
                KeyModifiers::NONE,
                diagnostic.chars.start + 1,
            );
            let buf = draw_sized(&mut app, STATUS_WIDTH, 24);
            assert!(
                row_text(&buf, 23)
                    .starts_with(&format!(" {} ", status_text(&diagnostic.message).unwrap())),
                "{:?}",
                row_text(&buf, 23)
            );

            app.handle(key(KeyCode::Home, KeyModifiers::NONE), Instant::now());
            assert_eq!(app.buffer.caret(tab_id), Some(0));
            let buf = draw_sized(&mut app, STATUS_WIDTH, 24);
            let status = row_text(&buf, 23);
            assert!(status.starts_with(&path_status(&app)), "{status:?}");
            fs::remove_dir_all(&dir).unwrap();
        }

        #[test]
        fn a_status_message_outranks_the_diagnostic_under_the_caret() {
            let dir = temp_dir("status-message-over-diagnostic");
            let mut app = app_with_text(&dir, MISSPELT, KeyboardMode::Enhanced);
            proofread(&mut app);
            let diagnostic = tset_diagnostic(&app);
            press(
                &mut app,
                KeyCode::Right,
                KeyModifiers::NONE,
                diagnostic.chars.start + 1,
            );
            app.status
                .set_message("Saved", MessageKind::Info, Instant::now());
            let buf = draw_sized(&mut app, STATUS_WIDTH, 24);
            let status = row_text(&buf, 23);
            assert!(status.starts_with(" Saved "), "{status:?}");
            assert!(!status.contains(&diagnostic.message), "{status:?}");
            fs::remove_dir_all(&dir).unwrap();
        }

        /// The screen column of char `char_idx` of `MISSPELT`, a one-line
        /// ASCII file drawn after its gutter.
        fn column_of_char(char_idx: usize) -> u16 {
            layout::gutter_width(line_count(MISSPELT)) + u16::try_from(char_idx).unwrap()
        }

        /// `MISSPELT`, proofread and drawn.
        fn proofread_app(dir: &Path) -> TermApp {
            let mut app = app_with_text(dir, MISSPELT, KeyboardMode::Enhanced);
            proofread(&mut app);
            draw(&mut app);
            app
        }

        /// The entries of the editor menu with no selection over `diagnostic`.
        fn menu_entries(diagnostic: Option<&Diagnostic>) -> Vec<MenuEntry<Command>> {
            editor_menu::entries(Enablement::Disabled, diagnostic)
        }

        /// Chooses the item labelled `label` from the open menu, whose entries
        /// are `entries`, with Down until it is highlighted, then Enter.
        fn choose(app: &mut TermApp, entries: &[MenuEntry<Command>], label: &str) {
            let index = entries
                .iter()
                .position(|entry| matches!(entry, MenuEntry::Item { label: l, .. } if l == label))
                .unwrap();
            for _ in 0..entries.len() {
                if app.menu.as_ref().unwrap().highlighted() == Some(index) {
                    break;
                }
                app.handle(key(KeyCode::Down, KeyModifiers::NONE), Instant::now());
            }
            assert_eq!(app.menu.as_ref().unwrap().highlighted(), Some(index));
            app.handle(key(KeyCode::Enter, KeyModifiers::NONE), Instant::now());
            assert!(app.menu.is_none());
        }

        /// Opens the editor menu over `tset` with a right-click on its second
        /// char, returning the diagnostic and the menu's entries.
        fn right_click_tset(app: &mut TermApp) -> (Diagnostic, Vec<MenuEntry<Command>>) {
            let diagnostic = tset_diagnostic(app);
            right_click(app, column_of_char(diagnostic.chars.start + 1), 1);
            let entries = menu_entries(Some(&diagnostic));
            (diagnostic, entries)
        }

        /// Asserts that no cell of `diagnostic` is underlined on a fresh draw.
        fn assert_not_underlined(app: &mut TermApp, diagnostic: &Diagnostic) {
            let buf = draw(app);
            for char_idx in diagnostic.chars.clone() {
                let x = column_of_char(char_idx);
                assert_eq!(underline_of(&buf[(x, 1)]), None, "char {char_idx}");
            }
        }

        #[test]
        fn a_right_click_on_a_diagnostic_opens_its_menu_without_moving_the_caret() {
            let dir = temp_dir("menu-diagnostic");
            let mut app = proofread_app(&dir);
            let id = active_id(&app);
            let diagnostic = tset_diagnostic(&app);
            let x = column_of_char(diagnostic.chars.start + 1);
            right_click(&mut app, x, 1);
            assert_eq!(
                app.menu,
                Some(ContextMenu::new(
                    menu_entries(Some(&diagnostic)),
                    Position::new(x, 1)
                ))
            );
            assert_eq!(app.buffer.caret(id), Some(0));
            assert_eq!(app.buffer.selection(id), Some(0..0));
            fs::remove_dir_all(&dir).unwrap();
        }

        #[test]
        fn a_right_click_on_a_clean_word_opens_the_plain_menu() {
            let dir = temp_dir("menu-clean-word");
            let mut app = proofread_app(&dir);
            let id = active_id(&app);
            // The "h" of "This".
            assert!(
                app.editor()
                    .proofread()
                    .diagnostic_at(id, MISSPELT, 1)
                    .is_none()
            );
            let x = column_of_char(1);
            right_click(&mut app, x, 1);
            assert_eq!(
                app.menu,
                Some(ContextMenu::new(menu_entries(None), Position::new(x, 1)))
            );
            assert_eq!(app.buffer.caret(id), Some(0));
            fs::remove_dir_all(&dir).unwrap();
        }

        #[test]
        fn choosing_a_suggestion_replaces_the_word_and_ctrl_z_restores() {
            let dir = temp_dir("menu-suggestion");
            let mut app = proofread_app(&dir);
            let (diagnostic, entries) = right_click_tset(&mut app);
            let suggestion = diagnostic.suggestions.first().unwrap().clone();
            assert!(!suggestion.is_empty(), "{diagnostic:?}");
            choose(&mut app, &entries, &suggestion);
            assert_eq!(text_of(&app), MISSPELT.replacen("tset", &suggestion, 1));

            app.handle(ctrl('z'), Instant::now());
            assert_eq!(text_of(&app), MISSPELT);
            fs::remove_dir_all(&dir).unwrap();
        }

        #[test]
        fn adding_to_the_dictionary_removes_the_squiggle() {
            let dir = temp_dir("menu-add-word");
            let mut app = proofread_app(&dir);
            let (diagnostic, entries) = right_click_tset(&mut app);
            choose(&mut app, &entries, "Add \u{201c}tset\u{201d} to dictionary");
            assert!(app.editor().proofread().user_words().any(|w| w == "tset"));
            // The store, and so the dictionary, lives under the temp dir.
            let dictionary = fs::read_to_string(dir.join("state").join("dictionary.txt")).unwrap();
            assert!(
                dictionary.lines().any(|line| line == "tset"),
                "{dictionary:?}"
            );
            assert_not_underlined(&mut app, &diagnostic);
            assert_eq!(text_of(&app), MISSPELT);
            fs::remove_dir_all(&dir).unwrap();
        }

        #[test]
        fn ignoring_removes_the_squiggle() {
            let dir = temp_dir("menu-ignore");
            let mut app = proofread_app(&dir);
            let (diagnostic, entries) = right_click_tset(&mut app);
            choose(&mut app, &entries, "Ignore");
            assert_not_underlined(&mut app, &diagnostic);
            assert_eq!(text_of(&app), MISSPELT);
            fs::remove_dir_all(&dir).unwrap();
        }

        /// The keys that open the editor menu at the caret.
        const EDITOR_MENU_KEYS: [(KeyCode, KeyModifiers); 3] = [
            (KeyCode::F(10), KeyModifiers::SHIFT),
            (KeyCode::F(22), KeyModifiers::NONE),
            (KeyCode::Menu, KeyModifiers::NONE),
        ];

        #[test]
        fn menu_keys_open_the_diagnostic_menu_below_the_caret() {
            let dir = temp_dir("menu-key-diagnostic");
            let mut app = proofread_app(&dir);
            let diagnostic = tset_diagnostic(&app);
            let caret = diagnostic.chars.start + 1;
            press(&mut app, KeyCode::Right, KeyModifiers::NONE, caret);
            let id = active_id(&app);
            let cell = app
                .buffer
                .caret_cell(app.editor().active_tab().unwrap())
                .unwrap();
            assert_eq!(cell, Position::new(column_of_char(caret), 1));
            for (code, modifiers) in EDITOR_MENU_KEYS {
                app.handle(key(code, modifiers), Instant::now());
                assert_eq!(
                    app.menu,
                    Some(ContextMenu::below(menu_entries(Some(&diagnostic)), cell)),
                    "{code:?} {modifiers:?}"
                );
                assert_eq!(app.buffer.caret(id), Some(caret));
                app.handle(key(KeyCode::Esc, KeyModifiers::NONE), Instant::now());
                assert!(app.menu.is_none());
            }
            fs::remove_dir_all(&dir).unwrap();
        }

        #[test]
        fn menu_keys_on_a_clean_word_open_the_plain_menu_below_the_caret() {
            let dir = temp_dir("menu-key-clean");
            let mut app = proofread_app(&dir);
            let id = active_id(&app);
            assert!(
                app.editor()
                    .proofread()
                    .diagnostic_at(id, MISSPELT, 0)
                    .is_none()
            );
            let cell = app
                .buffer
                .caret_cell(app.editor().active_tab().unwrap())
                .unwrap();
            assert_eq!(cell, Position::new(column_of_char(0), 1));
            for (code, modifiers) in EDITOR_MENU_KEYS {
                app.handle(key(code, modifiers), Instant::now());
                assert_eq!(
                    app.menu,
                    Some(ContextMenu::below(menu_entries(None), cell)),
                    "{code:?} {modifiers:?}"
                );
                app.handle(key(KeyCode::Esc, KeyModifiers::NONE), Instant::now());
            }
            fs::remove_dir_all(&dir).unwrap();
        }

        #[test]
        fn the_menu_key_opens_at_the_editor_top_left_when_the_caret_is_off_screen() {
            let dir = temp_dir("menu-key-off-screen");
            let text: String = (1..=100).map(|n| format!("line {n}\n")).collect();
            let mut app = app_with_text(&dir, &text, KeyboardMode::Enhanced);
            let id = active_id(&app);
            for _ in 0..10 {
                app.handle(mouse(MouseEventKind::ScrollDown, 10, 5), Instant::now());
            }
            draw(&mut app);
            assert_eq!(app.buffer.caret(id), Some(0));
            assert_eq!(
                app.buffer.caret_cell(app.editor().active_tab().unwrap()),
                None
            );
            let rect = app.buffer.rect();
            assert_eq!(rect.y, 1);
            app.handle(key(KeyCode::F(10), KeyModifiers::SHIFT), Instant::now());
            assert_eq!(
                app.menu,
                Some(ContextMenu::new(
                    menu_entries(None),
                    Position::new(rect.x, rect.y)
                ))
            );
            fs::remove_dir_all(&dir).unwrap();
        }

        /// Closing the find bar moves the editor up; the menu key pressed
        /// in the same batch places the menu from the old layout, and the
        /// next draw re-anchors it under the caret as drawn then.
        #[test]
        fn the_menu_key_menu_follows_the_caret_drawn_after_the_find_bar_closes() {
            let dir = temp_dir("menu-key-follows-caret");
            let mut app = app_with_text(&dir, "hello\nworld", KeyboardMode::Enhanced);
            app.handle(ctrl('f'), Instant::now());
            draw(&mut app);
            let open_rect = app.buffer.rect();
            assert!(open_rect.y > 1, "{open_rect:?}");
            // No draw from here to the menu key.
            app.handle(key(KeyCode::Esc, KeyModifiers::NONE), Instant::now());
            assert!(!app.editor().search().is_open());
            assert_eq!(app.focus, Focus::Editor);
            app.handle(key(KeyCode::F(10), KeyModifiers::SHIFT), Instant::now());
            // The initial placement is from the last draw, under the bar.
            let stale = Position::new(open_rect.x, open_rect.y);
            assert_eq!(
                app.menu,
                Some(ContextMenu::below(menu_entries(None), stale))
            );

            let buf = draw(&mut app);
            let cell = app
                .buffer
                .caret_cell(app.editor().active_tab().unwrap())
                .unwrap();
            assert_eq!(cell.y, 1, "the caret is drawn on the first text row");
            let area = buf.area;
            let rect = app.menu.as_ref().unwrap().rect(area);
            assert_eq!((rect.x, rect.y), (cell.x, cell.y + 1));
            assert_eq!(
                row_text(&buf, rect.y).chars().nth(usize::from(rect.x)),
                Some('┌')
            );
            fs::remove_dir_all(&dir).unwrap();
        }

        #[test]
        fn a_right_click_menu_stays_where_it_opened_across_draws() {
            let dir = temp_dir("right-click-menu-stays");
            let mut app = app_with_text(&dir, "hello\nworld", KeyboardMode::Enhanced);
            app.handle(ctrl('f'), Instant::now());
            draw(&mut app);
            // Past the text of the editor's second row, away from the caret.
            let y = app.buffer.rect().y + 1;
            right_click(&mut app, 20, y);
            let expected = ContextMenu::new(menu_entries(None), Position::new(20, y));
            assert_eq!(app.menu.as_ref(), Some(&expected));
            for _ in 0..2 {
                let buf = draw(&mut app);
                assert_eq!(app.menu.as_ref(), Some(&expected));
                assert_eq!(row_text(&buf, y).chars().nth(20), Some('┌'));
            }
            fs::remove_dir_all(&dir).unwrap();
        }

        #[test]
        fn menu_keys_in_the_sidebar_still_open_the_tree_menu() {
            let (dir, ws, mut app) = tree_app("tree-menu-key");
            let cargo = ws.join("Cargo.toml");
            app.handle(ctrl('e'), Instant::now());
            press(&mut app, KeyCode::Down, KeyModifiers::NONE, 3);
            let expected =
                ContextMenu::new(tree_menu::file_entries(&cargo), Position::new(5, tree_y(2)));
            for (code, modifiers) in EDITOR_MENU_KEYS {
                app.handle(key(code, modifiers), Instant::now());
                assert_eq!(app.menu.as_ref(), Some(&expected), "{code:?} {modifiers:?}");
                assert_eq!(app.focus, Focus::Sidebar);
                app.handle(key(KeyCode::Esc, KeyModifiers::NONE), Instant::now());
                assert!(app.menu.is_none());
                draw(&mut app);
            }
            fs::remove_dir_all(&dir).unwrap();
        }

        /// Asserts that a right-click at (`x`, `y`) opens the plain editor
        /// menu there, then closes it.
        fn assert_plain_menu_at(app: &mut TermApp, x: u16, y: u16) {
            right_click(app, x, y);
            assert_eq!(
                app.menu,
                Some(ContextMenu::new(menu_entries(None), Position::new(x, y))),
                "({x}, {y})"
            );
            app.handle(key(KeyCode::Esc, KeyModifiers::NONE), Instant::now());
            assert!(app.menu.is_none());
            // Clicks hit-test against the last draw.
            draw(app);
        }

        #[test]
        fn right_clicks_after_a_diagnostic_or_past_the_text_open_the_plain_menu() {
            let dir = temp_dir("menu-after-diagnostic");
            let mut app = proofread_app(&dir);
            let diagnostic = tset_diagnostic(&app);
            assert_eq!(MISSPELT.chars().nth(diagnostic.chars.end), Some(' '));
            // The space right after `tset`.
            assert_plain_menu_at(&mut app, column_of_char(diagnostic.chars.end), 1);
            // Past the end of the line's content.
            let past = column_of_char(MISSPELT.chars().count());
            assert_plain_menu_at(&mut app, past, 1);
            assert_plain_menu_at(&mut app, past + 20, 1);
            // Below the last row.
            assert_plain_menu_at(&mut app, column_of_char(diagnostic.chars.start), 5);
            fs::remove_dir_all(&dir).unwrap();
        }

        #[test]
        fn a_caret_just_after_a_diagnostic_shows_it_and_opens_its_menu() {
            let dir = temp_dir("caret-after-diagnostic");
            let mut app = proofread_app(&dir);
            let diagnostic = tset_diagnostic(&app);
            let caret = diagnostic.chars.end;
            press(&mut app, KeyCode::Right, KeyModifiers::NONE, caret);
            let buf = draw_sized(&mut app, STATUS_WIDTH, 24);
            let status = row_text(&buf, 23);
            let message = status_text(&diagnostic.message).unwrap();
            assert!(status.starts_with(&format!(" {message} ")), "{status:?}");

            let cell = app
                .buffer
                .caret_cell(app.editor().active_tab().unwrap())
                .unwrap();
            assert_eq!(cell, Position::new(column_of_char(caret), 1));
            app.handle(key(KeyCode::F(10), KeyModifiers::SHIFT), Instant::now());
            assert_eq!(
                app.menu,
                Some(ContextMenu::below(menu_entries(Some(&diagnostic)), cell))
            );
            fs::remove_dir_all(&dir).unwrap();
        }

        #[test]
        fn status_text_puts_a_message_on_one_line() {
            assert_eq!(status_text("a\nb  c").as_deref(), Some("a b c"));
            assert_eq!(status_text("\tword\r\n").as_deref(), Some("word"));
        }

        #[test]
        fn status_text_of_a_blank_message_is_none_so_the_path_shows() {
            assert_eq!(status_text("  \n"), None);
            assert_eq!(status_text(""), None);
        }

        #[test]
        fn the_status_line_shows_the_path_while_the_sidebar_has_focus() {
            let dir = temp_dir("status-sidebar-focus");
            let ws = dir.join("ws");
            fs::create_dir_all(&ws).unwrap();
            let file = ws.join("text.txt");
            fs::write(&file, MISSPELT).unwrap();
            let editor = open_editor(&dir, &[file], Some(&ws));
            let mut app = TermApp::new(editor, KeyboardMode::Enhanced, ws.clone(), None);
            draw(&mut app);
            proofread(&mut app);
            let diagnostic = tset_diagnostic(&app);
            press(
                &mut app,
                KeyCode::Right,
                KeyModifiers::NONE,
                diagnostic.chars.start + 1,
            );
            let message = status_text(&diagnostic.message).unwrap();
            let buf = draw_sized(&mut app, STATUS_WIDTH, 24);
            let status = row_text(&buf, 23);
            assert!(status.starts_with(&format!(" {message} ")), "{status:?}");

            app.handle(ctrl('e'), Instant::now());
            assert_eq!(app.focus, Focus::Sidebar);
            let buf = draw_sized(&mut app, STATUS_WIDTH, 24);
            let status = row_text(&buf, 23);
            assert!(status.starts_with(&path_status(&app)), "{status:?}");
            assert!(!status.contains(&message), "{status:?}");
            fs::remove_dir_all(&dir).unwrap();
        }

        /// Replace all is the core edit that returns no `SetSelection`, so
        /// only `record_outside_edit` re-lays the view out: the menu key
        /// still opens below the caret with no draw in between (07.7).
        #[test]
        fn the_menu_key_after_replace_all_opens_below_the_caret_without_a_draw() {
            let dir = temp_dir("menu-key-after-replace-all");
            let mut app = app_with_text(&dir, "a\na", KeyboardMode::Enhanced);
            let id = active_id(&app);
            app.handle(ctrl('h'), Instant::now());
            type_text(&mut app, "a");
            press(&mut app, KeyCode::Tab, KeyModifiers::NONE, 1);
            type_text(&mut app, "bb");
            let x = drawn_column(&mut app, 2, "[Replace all]");
            // No draw from here to the menu key.
            left_click(&mut app, x, 2);
            assert_eq!(text_of(&app), "bb\nbb");
            // The caret goes to the end of the diffed change.
            assert_eq!(app.buffer.caret(id), Some(5));
            app.handle(ctrl('e'), Instant::now());
            assert_eq!(app.focus, Focus::Editor);

            // Under the tab bar and the find and replace rows, after the
            // gutter: row 1, column 2 of the text.
            let rect = app.buffer.rect();
            assert_eq!(rect.y, 3);
            let cell = app.buffer.caret_cell(app.editor().active_tab().unwrap());
            let expected = Position::new(rect.x + 2, rect.y + 1);
            assert_eq!(cell, Some(expected));
            app.handle(key(KeyCode::F(10), KeyModifiers::SHIFT), Instant::now());
            assert_eq!(
                app.menu,
                Some(ContextMenu::below(menu_entries(None), expected))
            );

            // A real draw puts the caret on the same cell.
            app.handle(key(KeyCode::Esc, KeyModifiers::NONE), Instant::now());
            draw(&mut app);
            assert_eq!(
                app.buffer.caret_cell(app.editor().active_tab().unwrap()),
                Some(expected)
            );
            fs::remove_dir_all(&dir).unwrap();
        }
    }
}
