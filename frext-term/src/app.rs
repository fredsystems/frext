//! [`TermApp`]: the terminal front end's state, event routing and
//! rendering.
//!
//! `TermApp` turns [`AppEvent`]s into [`Command`]s for the core
//! [`Editor`] (or into local view changes), queues the [`Effect`]s the
//! editor returns, and draws the whole screen into a ratatui [`Frame`].

use std::ops::Range;
use std::path::PathBuf;
use std::time::Instant;

use crossterm::event::{Event, KeyEvent, KeyEventKind, MouseButton, MouseEvent, MouseEventKind};
use frext_core::editor::{Command, Editor, Effect};
use frext_core::lines::line_count;
use frext_core::syntax::language_from_path;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use crate::editor_view::EditorView;
use crate::events::AppEvent;
use crate::hit::{HitMap, Target};
use crate::keymap::{self, Action, Focus, KeyContext};
use crate::layout;
use crate::prompt::{PromptReply, PromptUi};
use crate::status::{MessageKind, StatusLine};
use crate::tab_bar;
use crate::terminal::KeyboardMode;
use crate::theme;

/// The sidebar's width in columns, when shown.
const SIDEBAR_WIDTH: u16 = 30;

/// How many lines one mouse-wheel notch scrolls.
const WHEEL_LINES: i32 = 3;

/// Shown on startup when the kitty keyboard protocol is unavailable.
const LEGACY_MESSAGE: &str = "Kitty keyboard protocol unavailable: use Ctrl+R for find and replace";

/// Shown when a find key is pressed before the find bar exists.
const FIND_BAR_MESSAGE: &str = "Find bar is not available yet";

/// Shown when text is pasted outside a prompt.
const PASTE_MESSAGE: &str = "Editing arrives in a later version";

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
    view: EditorView,
    status: StatusLine,
    hovered: Option<Target>,
    hits: HitMap,
    last_editor_height: u16,
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
            view: EditorView::default(),
            status,
            hovered: None,
            hits: HitMap::default(),
            last_editor_height: 0,
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

    /// Returns focus to the editor when the sidebar is not shown.
    fn settle_focus(&mut self) {
        if self.focus == Focus::Sidebar && !self.sidebar_shown() {
            self.focus = Focus::Editor;
        }
    }

    /// Routes one event. Effects are queued, not applied: call
    /// [`Self::apply_effects`] after a batch of events.
    pub fn handle(&mut self, event: AppEvent, now: Instant) {
        self.prompt.sync(&self.editor);
        self.settle_focus();
        match event {
            AppEvent::Input(Event::Key(key)) => self.handle_key(key, now),
            AppEvent::Input(Event::Mouse(mouse)) => self.handle_mouse(mouse),
            AppEvent::Input(Event::Paste(text)) => {
                if self.prompt.is_open(&self.editor) {
                    self.prompt.handle_paste(&mut self.editor, &text);
                } else {
                    self.status
                        .set_message(PASTE_MESSAGE, MessageKind::Info, now);
                }
            }
            AppEvent::Input(Event::Resize(..)) => self.forget_layout(),
            AppEvent::Input(Event::FocusGained | Event::FocusLost) | AppEvent::Wake => {}
            AppEvent::Tick => {
                self.status.expire(now);
            }
            AppEvent::InputClosed => self.quit = Quit::Requested,
        }
    }

    fn handle_key(&mut self, key: KeyEvent, now: Instant) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        if self.prompt.is_open(&self.editor) {
            match self.prompt.handle_key(&mut self.editor, key) {
                PromptReply::Send(command) => self.send(command),
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
            return;
        };
        match action {
            Action::Command(command) => {
                let find = matches!(command, Command::OpenSearch(_));
                self.send(command);
                if find {
                    self.status
                        .set_message(FIND_BAR_MESSAGE, MessageKind::Info, now);
                }
            }
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
                    self.focus = match self.focus {
                        Focus::Editor => Focus::Sidebar,
                        Focus::Sidebar => Focus::Editor,
                    };
                }
            }
            Action::FocusEditor => self.focus = Focus::Editor,
            Action::Scroll(by) => {
                if let Some(tab) = self.editor.active_tab() {
                    self.view.scroll(tab, by, self.last_editor_height);
                }
            }
        }
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) {
        let target = self.hits.hit(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Moved | MouseEventKind::Drag(_) => {
                self.hovered = target;
                self.status.set_hint(target.and_then(Target::hint));
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if self.prompt.is_open(&self.editor) {
                    let reply = self
                        .prompt
                        .handle_click(&mut self.editor, target, mouse.column);
                    if let PromptReply::Send(command) = reply {
                        self.send(command);
                    }
                    return;
                }
                match target {
                    Some(Target::NewTab) => self.send(Command::NewTab),
                    Some(Target::Save) => self.send(Command::Save),
                    Some(Target::SelectTab(index)) => self.send(Command::SelectTab(index)),
                    Some(Target::CloseTab(index)) => self.send(Command::RequestClose(index)),
                    Some(Target::OpenFile) => self.open_file_prompt(),
                    Some(Target::Editor) => self.focus = Focus::Editor,
                    Some(
                        Target::PromptPopup
                        | Target::PromptInput
                        | Target::PromptButton(_)
                        | Target::PromptCandidate(_),
                    )
                    | None => {}
                }
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                if self.prompt.is_open(&self.editor) || target != Some(Target::Editor) {
                    return;
                }
                let lines = if mouse.kind == MouseEventKind::ScrollUp {
                    -WHEEL_LINES
                } else {
                    WHEEL_LINES
                };
                if let Some(tab) = self.editor.active_tab() {
                    self.view.wheel(tab, lines);
                }
            }
            _ => {}
        }
    }

    /// Applies `command` and queues the effects it returns.
    fn send(&mut self, command: Command) {
        let outcome = self.editor.apply(command);
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
    /// the next render, mouse events hit nothing.
    fn forget_layout(&mut self) {
        self.hits.clear();
        self.hovered = None;
    }

    /// Carries out the queued effects, then drops the scroll position of
    /// tabs that no longer exist.
    ///
    /// Before the editor gains a caret, `SetSelection` only supplies the
    /// range a following `RevealSelection` for the same tab centres on.
    pub fn apply_effects(&mut self) {
        let mut selection: Option<(u64, Range<usize>)> = None;
        for effect in std::mem::take(&mut self.effects) {
            match effect {
                Effect::SetSelection { tab_id, chars, .. } => {
                    log::debug!("selection {chars:?} in tab {tab_id} ignored (no caret yet)");
                    selection = Some((tab_id, chars));
                }
                Effect::RevealSelection { tab_id } => {
                    let Some((_, chars)) = selection.as_ref().filter(|(id, _)| *id == tab_id)
                    else {
                        continue;
                    };
                    if let Some(tab) = self.editor.active_tab().filter(|tab| tab.id == tab_id) {
                        self.view
                            .reveal(tab, chars.clone(), self.last_editor_height);
                    }
                }
                Effect::FocusPromptField => self.prompt.focus_field(&self.editor),
                Effect::FocusSearchField => {}
            }
        }
        self.view
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
            self.editor.search().is_open(),
            sidebar_width,
            gutter_width,
        );
        self.last_editor_height = areas.editor.height;

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
        fill(buf, areas.find_bar, theme::chrome());
        fill(buf, areas.sidebar, theme::chrome());
        self.view
            .render(tab, areas.gutter, areas.editor, buf, &mut self.hits);

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

        if let Some(position) = self.prompt.render(&self.editor, area, buf, &mut self.hits) {
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
        assert_eq!(app.last_editor_height, 22);
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
    fn paste_goes_to_the_prompt_or_sets_a_message() {
        let dir = temp_dir("paste");
        let file = dir.join("pasted.txt");
        fs::write(&file, "x").unwrap();
        let mut app = app(&dir, &[], KeyboardMode::Enhanced);
        let now = Instant::now();
        app.handle(AppEvent::Input(Event::Paste("abc".to_owned())), now);
        assert_eq!(
            app.status.message(),
            Some((PASTE_MESSAGE, MessageKind::Info))
        );

        app.handle(ctrl('o'), now);
        app.handle(AppEvent::Input(Event::Paste("pasted.txt".to_owned())), now);
        app.handle(key(KeyCode::Enter, KeyModifiers::NONE), now);
        assert_eq!(
            app.editor().active_tab().unwrap().path.as_deref(),
            Some(file.as_path())
        );
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
    fn legacy_mode_binds_ctrl_r_but_not_ctrl_h() {
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
        assert_eq!(
            app.status.message(),
            Some((FIND_BAR_MESSAGE, MessageKind::Info))
        );
        app.apply_effects();

        // The find-bar row is reserved, pushing the editor down a row.
        let buf = draw(&mut app);
        assert_eq!(row_text(&buf, 1).trim(), "");
        assert!(row_text(&buf, 2).starts_with(" 1 "));
        assert_eq!(app.last_editor_height, 21);

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
    fn keys_scroll_by_the_editor_height() {
        let dir = temp_dir("keys-scroll");
        let file = long_file(&dir);
        let mut app = app(&dir, &[file], KeyboardMode::Enhanced);
        let now = Instant::now();
        draw(&mut app);

        app.handle(key(KeyCode::PageDown, KeyModifiers::NONE), now);
        let buf = draw(&mut app);
        assert!(row_text(&buf, 1).starts_with(" 22 line 22 "));

        app.handle(key(KeyCode::Down, KeyModifiers::NONE), now);
        let buf = draw(&mut app);
        assert!(row_text(&buf, 1).starts_with(" 23 line 23 "));

        app.handle(key(KeyCode::Home, KeyModifiers::CONTROL), now);
        let buf = draw(&mut app);
        assert!(row_text(&buf, 1).starts_with("  1 line 1 "));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn reveal_selection_centres_the_last_set_selection() {
        let dir = temp_dir("reveal");
        let file = long_file(&dir);
        let mut app = app(&dir, &[file], KeyboardMode::Enhanced);
        draw(&mut app);
        let tab_id = app.editor().active_tab().unwrap().id;
        let text = app.editor().active_tab().unwrap().text.clone();
        let start = text.find("line 50\n").unwrap();
        app.effects.push(Effect::SetSelection {
            tab_id,
            chars: start..start + 4,
            focus: frext_core::editor::EditorFocus::Keep,
        });
        app.effects.push(Effect::RevealSelection { tab_id });
        app.apply_effects();
        assert!(app.effects.is_empty());
        // Line 50 (index 49) centred in 22 rows: top index 38.
        let buf = draw(&mut app);
        assert!(row_text(&buf, 1).starts_with(" 39 line 39 "));

        // A reveal with no selection in the same drain does nothing.
        app.effects.push(Effect::RevealSelection { tab_id });
        app.apply_effects();
        let buf = draw(&mut app);
        assert!(row_text(&buf, 1).starts_with(" 39 line 39 "));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_tick_after_four_seconds_clears_the_message() {
        let dir = temp_dir("tick");
        let mut app = app(&dir, &[], KeyboardMode::Enhanced);
        let start = Instant::now();
        app.handle(AppEvent::Input(Event::Paste("x".to_owned())), start);
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
}
