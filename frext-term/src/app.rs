//! [`TermApp`]: the terminal front end's state, event routing and
//! rendering.
//!
//! `TermApp` turns [`AppEvent`]s into [`Command`]s for the core
//! [`Editor`] (or into local view changes), queues the [`Effect`]s the
//! editor returns, and draws the whole screen into a ratatui [`Frame`].

use std::path::PathBuf;
use std::time::Instant;

use crossterm::event::{
    Event, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use frext_core::editor::{BufferChange, Command, Editor, EditorFocus, Effect};
use frext_core::lines::line_count;
use frext_core::syntax::language_from_path;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;

use crate::buffer::cursor::Selecting;
use crate::buffer::edit::{Applied, EditAction};
use crate::buffer::{BufferView, WHEEL_ROWS};
use crate::editor_menu;
use crate::events::AppEvent;
use crate::find_bar::{self, FindBar, FindField, FindReply};
use crate::hit::{HitMap, Target};
use crate::keymap::{self, Action, Focus, KeyContext};
use crate::layout;
use crate::menu::{ContextMenu, Enablement, MenuReply};
use crate::prompt::{PromptReply, PromptUi};
use crate::status::{MessageKind, StatusLine};
use crate::tab_bar;
use crate::terminal::KeyboardMode;
use crate::theme;

/// The sidebar's width in columns, when shown.
const SIDEBAR_WIDTH: u16 = 30;

/// Shown on startup when the kitty keyboard protocol is unavailable.
const LEGACY_MESSAGE: &str = "Kitty keyboard protocol unavailable: use Ctrl+R for find and replace";

/// Whether the user wants the sidebar shown (when there is a workspace).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SidebarVisibility {
    Shown,
    Hidden,
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
    find: FindBar,
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
            find: FindBar::new(),
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
                    FindReply::Focus(field) => self.set_focus(Focus::Find(field)),
                    FindReply::Handled | FindReply::Ignored => {}
                }
            }
            return;
        };
        match action {
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
            Action::ToggleFocus => {
                if self.sidebar_shown() {
                    self.set_focus(match self.focus {
                        Focus::Editor | Focus::Find(_) => Focus::Sidebar,
                        Focus::Sidebar => Focus::Editor,
                    });
                }
            }
            Action::FocusEditor => self.set_focus(Focus::Editor),
            Action::Edit(action) => self.edit(action, now),
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

    /// Records the mouse's hover target and shows its hint.
    fn hover(&mut self, target: Option<Target>) {
        self.hovered = target;
        self.status.set_hint(target.and_then(Target::hint));
    }

    /// Routes a mouse event while the editor menu is open: it captures
    /// clicks (one outside dismisses it without acting on what is under
    /// it), and the wheel does nothing.
    fn handle_menu_mouse(&mut self, mouse: MouseEvent, target: Option<Target>, now: Instant) {
        match mouse.kind {
            MouseEventKind::Moved | MouseEventKind::Drag(_) => {
                self.hover(target);
                if let Some(menu) = self.menu.as_mut() {
                    menu.hover(match target {
                        Some(Target::MenuItem(index)) => Some(index),
                        _ => None,
                    });
                }
            }
            MouseEventKind::Up(MouseButton::Left) => self.buffer.mouse_up(),
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
                self.hover(target);
                if mouse.kind == MouseEventKind::Drag(MouseButton::Left)
                    && self.buffer.is_dragging()
                    && !self.prompt.is_open(&self.editor)
                    && let Some(tab) = self.editor.active_tab()
                {
                    self.buffer.mouse_drag(tab, mouse.column, mouse.row);
                }
            }
            MouseEventKind::Up(MouseButton::Left) => self.buffer.mouse_up(),
            MouseEventKind::Down(MouseButton::Left) => {
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
                    Some(
                        Target::PromptPopup
                        | Target::PromptInput
                        | Target::PromptButton(_)
                        | Target::PromptCandidate(_)
                        | Target::MenuPopup
                        | Target::MenuItem(_)
                        | Target::FindInput
                        | Target::ReplaceInput
                        | Target::FindOption(_)
                        | Target::FindPrev
                        | Target::FindNext
                        | Target::FindReplaceRow
                        | Target::FindClose
                        | Target::ReplaceCurrent
                        | Target::ReplaceAll,
                    )
                    | None => {}
                }
            }
            MouseEventKind::Down(MouseButton::Right) => {
                if self.prompt.is_open(&self.editor) || target != Some(Target::Editor) {
                    return;
                }
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
                    self.menu = Some(ContextMenu::new(
                        editor_menu::entries(selection),
                        Position::new(mouse.column, mouse.row),
                    ));
                    // The chosen command acts on the editor's selection.
                    self.set_focus(Focus::Editor);
                }
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                if self.prompt.is_open(&self.editor) || target != Some(Target::Editor) {
                    return;
                }
                let rows = if mouse.kind == MouseEventKind::ScrollUp {
                    -WHEEL_ROWS
                } else {
                    WHEEL_ROWS
                };
                if let Some(tab) = self.editor.active_tab() {
                    self.buffer.wheel(tab, rows);
                }
            }
            _ => {}
        }
    }

    /// Applies `command` and queues the effects it returns.
    ///
    /// The editor's selection (while it has focus) is passed to core
    /// first. A text-editing command is recorded in the active tab's undo
    /// history as one group, redoing to the selection the command sets for
    /// that tab, if any.
    fn send(&mut self, command: Command, now: Instant) {
        let selection = match self.focus {
            Focus::Editor | Focus::Find(_) => self
                .editor
                .active_tab()
                .and_then(|tab| self.buffer.selection(tab.id)),
            Focus::Sidebar => None,
        };
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
    /// the next render, mouse events hit nothing. The editor menu, which
    /// is anchored to that layout, closes.
    fn forget_layout(&mut self) {
        self.hits.clear();
        self.hovered = None;
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

    /// Draws the whole screen into `frame`, rebuilding the hit map.
    pub fn render(&mut self, frame: &mut Frame) {
        self.hits.clear();
        let area = frame.area();
        let tab = self.editor.active_tab();
        let gutter_width = tab.map_or(0, |tab| layout::gutter_width(line_count(&tab.text)));
        let sidebar_width = self.sidebar_shown().then_some(SIDEBAR_WIDTH);
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
        let find_focus = match self.focus {
            Focus::Find(field) => Some(field),
            Focus::Editor | Focus::Sidebar => None,
        };
        let find_cursor = self.find.render(
            &self.editor,
            find_focus,
            self.hovered,
            areas.find_bar,
            buf,
            &mut self.hits,
        );
        fill(buf, areas.sidebar, theme::chrome());
        let caret = self.buffer.render(
            tab,
            areas.gutter,
            areas.editor,
            &self.editor.match_highlights(),
            buf,
            &mut self.hits,
        );

        let (left, right) = tab.map_or_else(
            || (String::new(), String::new()),
            |tab| {
                let left = tab.path.as_deref().map_or_else(
                    || tab.display_name(),
                    |path| self.editor.relative_to_root(path),
                );
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
        let find_cursor = find_cursor.filter(|_| find_focus.is_some() && unobscured);
        if let Some(position) = prompt_cursor.or(editor_cursor).or(find_cursor) {
            frame.set_cursor_position(position);
        }
    }
}

/// `"1 line"` for a single line, `"{n} lines"` otherwise.
fn line_count_label(n: usize) -> String {
    if n == 1 {
        "1 line".to_owned()
    } else {
        format!("{n} lines")
    }
}

/// Blanks every cell of `area` with `style`.
fn fill(buf: &mut Buffer, area: Rect, style: Style) {
    let area = area.intersection(buf.area);
    for position in area.positions() {
        buf[position].reset();
        buf[position].set_style(style);
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
    use frext_core::editor::Prompt;
    use frext_core::persistence::Store;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;

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
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
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
        assert_eq!(buf[(0, 1)].bg, theme::chrome().bg.unwrap());
        assert_eq!(buf[(29, 10)].bg, theme::chrome().bg.unwrap());

        app.handle(ctrl('e'), now);
        assert_eq!(app.focus, Focus::Sidebar);

        app.handle(ctrl('b'), now);
        assert_eq!(app.focus, Focus::Editor);
        let buf = draw(&mut app);
        assert_eq!(buf[(1, 1)].symbol(), "1");
        assert_eq!(buf[(31, 1)].symbol(), " ");

        app.handle(ctrl('e'), now);
        assert_eq!(app.focus, Focus::Editor);

        app.handle(ctrl('b'), now);
        let buf = draw(&mut app);
        assert_eq!(buf[(31, 1)].symbol(), "1");
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
    fn frame_cursor(app: &mut TermApp) -> Option<ratatui::layout::Position> {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();
        terminal
            .backend()
            .cursor_visible()
            .then(|| terminal.backend().cursor_position())
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
        assert_eq!(
            frame_cursor(&mut app),
            Some(ratatui::layout::Position { x: 5, y: 1 })
        );

        app.handle(ctrl('o'), Instant::now());
        let cursor = frame_cursor(&mut app).unwrap();
        assert_ne!(cursor, ratatui::layout::Position { x: 5, y: 1 });
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
        use frext_core::highlight::MatchBackground;

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
            for bg in [buf[(first + x, 1)].bg, buf[(second + x, 1)].bg] {
                assert_ne!(bg, current);
                assert_ne!(bg, other);
            }
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
        assert_eq!(
            frame_cursor(&mut app),
            Some(ratatui::layout::Position::new(5, 2))
        );
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
                editor_menu::entries(Enablement::Enabled),
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
                editor_menu::entries(Enablement::Disabled),
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

    #[test]
    fn ctrl_f_draws_the_bar_and_focuses_the_find_field() {
        let dir = temp_dir("find-open");
        let mut app = app_with_text(&dir, "hello world", KeyboardMode::Enhanced);
        app.handle(ctrl('f'), Instant::now());
        assert_eq!(app.focus, Focus::Find(FindField::Pattern));
        let buf = draw(&mut app);
        assert!(row_text(&buf, 1).starts_with(" Find "));
        assert!(row_text(&buf, 2).starts_with(" 1 hello world"));
        let cursor = frame_cursor(&mut app).unwrap();
        assert_eq!(cursor.y, 1);
        assert!((6..20).contains(&cursor.x), "{cursor:?}");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn typing_in_the_find_field_searches_and_highlights() {
        use frext_core::highlight::MatchBackground;

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
            Some(ratatui::layout::Position::new(9, 1))
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

        // `ToggleReplaceRow` itself (as the bar's [Replace] button sends
        // it) settles the focus on the next event.
        app.handle(ctrl('h'), Instant::now());
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE, 1);
        assert_eq!(app.focus, Focus::Find(FindField::Replacement));
        app.send(Command::ToggleReplaceRow, Instant::now());
        assert!(!app.editor().search().replace_open());
        app.handle(AppEvent::Wake, Instant::now());
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
        assert_eq!(
            frame_cursor(&mut app),
            Some(ratatui::layout::Position::new(3, 1))
        );
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
            Some(ratatui::layout::Position::new(6 + 5, 1))
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
}
