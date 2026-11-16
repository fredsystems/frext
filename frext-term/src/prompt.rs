//! The terminal view of the editor's modal prompts.
//!
//! Core owns the [`Prompt`] itself and, for a name prompt, its text.
//! [`PromptUi`] owns only the view state: which control has focus, the
//! input's cursor and where the input was last drawn. It never applies a
//! command itself; it returns [`PromptReply::Send`] and the app sends it.

use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use frext_core::editor::{CloseChoice, Command, Editor, NameKind, Prompt};
use frext_core::tab::Tab;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};

use crate::hit::{HitMap, Target};
use crate::popup;
use crate::text_input::{InputOutcome, TextInput};
use crate::theme;

/// The buttons of the unsaved-changes prompt, in display order.
const CLOSE_BUTTONS: [(&str, CloseChoice); 3] = [
    ("Save and close", CloseChoice::Save),
    ("Discard changes", CloseChoice::Discard),
    ("Cancel", CloseChoice::Cancel),
];

/// The width of the unsaved-changes popup.
const CONFIRM_WIDTH: u16 = 60;

/// The width of the name popup.
const NAME_WIDTH: u16 = 50;

/// What the app should do after a prompt handled an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptReply {
    /// Send this command to the editor.
    Send(Command),
    /// The event was consumed; nothing to send.
    Handled,
    /// No prompt handled the event; the caller may bind it.
    Ignored,
}

/// Identifies a prompt, so a new prompt resets the view state.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PromptKey {
    ConfirmClose(u64),
    SavePath(u64),
    Name(NameKind, PathBuf),
}

impl PromptKey {
    fn of(prompt: &Prompt) -> Self {
        match prompt {
            Prompt::ConfirmClose { tab_id } => Self::ConfirmClose(*tab_id),
            Prompt::SavePath { tab_id, .. } => Self::SavePath(*tab_id),
            Prompt::Name(name) => Self::Name(name.kind, name.target.clone()),
        }
    }
}

/// The control inside a popup that has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromptFocus {
    Input,
    Button(usize),
}

impl PromptFocus {
    /// The next control in the cycle input, button 0, button 1.
    const fn next(self) -> Self {
        match self {
            Self::Input => Self::Button(0),
            Self::Button(0) => Self::Button(1),
            Self::Button(_) => Self::Input,
        }
    }

    /// The previous control in the cycle input, button 0, button 1.
    const fn prev(self) -> Self {
        match self {
            Self::Input => Self::Button(1),
            Self::Button(0) => Self::Input,
            Self::Button(_) => Self::Button(0),
        }
    }
}

/// The title, field label and action label of a name prompt.
const fn name_labels(kind: NameKind) -> (&'static str, &'static str, &'static str) {
    match kind {
        NameKind::Rename => ("Rename", "New name", "Rename"),
        NameKind::NewFile => ("New file", "File name", "Create"),
        NameKind::NewFolder => ("New folder", "Folder name", "Create"),
    }
}

/// The display name of the tab `tab_id`, if it is still open.
fn tab_name(editor: &Editor, tab_id: u64) -> Option<String> {
    editor
        .tabs()
        .iter()
        .find(|tab| tab.id == tab_id)
        .map(Tab::display_name)
}

/// Whether `key` types a printable char.
fn is_printable(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char(c) if !c.is_control())
        && !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
}

/// The command sent by the name prompt's button `index`.
fn name_button(index: usize) -> PromptReply {
    match index {
        0 => PromptReply::Send(Command::SubmitName),
        1 => PromptReply::Send(Command::CancelPrompt),
        _ => PromptReply::Handled,
    }
}

/// The view state of the prompt the editor is waiting on.
#[derive(Debug)]
pub struct PromptUi {
    key: Option<PromptKey>,
    focus: PromptFocus,
    input: TextInput,
    input_rect: Option<Rect>,
    base: PathBuf,
    home: Option<PathBuf>,
}

impl PromptUi {
    /// A prompt view resolving relative paths against `base`, with `~`
    /// expanding to `home`.
    #[must_use]
    pub fn new(base: PathBuf, home: Option<PathBuf>) -> Self {
        Self {
            key: None,
            focus: PromptFocus::Button(0),
            input: TextInput::new(),
            input_rect: None,
            base,
            home,
        }
    }

    /// The directory relative paths resolve against.
    #[must_use]
    pub fn base(&self) -> &Path {
        &self.base
    }

    /// The directory `~` expands to, if known.
    #[must_use]
    pub fn home(&self) -> Option<&Path> {
        self.home.as_deref()
    }

    /// Resets the view state when the editor's prompt differs from the
    /// one last seen: the focus goes to the prompt's first control and
    /// the input cursor to the end of its text.
    pub fn sync(&mut self, editor: &Editor) {
        let key = editor.prompt().map(PromptKey::of);
        if key == self.key {
            return;
        }
        self.focus = match key {
            Some(PromptKey::Name(..)) => PromptFocus::Input,
            _ => PromptFocus::Button(0),
        };
        self.input = TextInput::new();
        if let Some(Prompt::Name(prompt)) = editor.prompt() {
            self.input.move_to_end(&prompt.name);
        }
        self.input_rect = None;
        self.key = key;
    }

    /// Focuses the input field (for `Effect::FocusPromptField`) and moves
    /// its cursor to the end.
    pub fn focus_field(&mut self, editor: &Editor) {
        self.sync(editor);
        if let Some(Prompt::Name(prompt)) = editor.prompt() {
            self.focus = PromptFocus::Input;
            self.input.move_to_end(&prompt.name);
        }
    }

    /// Handles a key while a prompt is open. Keys the prompt does not
    /// bind are swallowed, since the prompt is modal.
    pub fn handle_key(&mut self, editor: &mut Editor, key: KeyEvent) -> PromptReply {
        self.sync(editor);
        match self.key {
            Some(PromptKey::ConfirmClose(tab_id)) => self.confirm_key(editor, tab_id, key),
            Some(PromptKey::Name(..)) => self.name_key(editor, key),
            Some(PromptKey::SavePath(_)) | None => PromptReply::Ignored,
        }
    }

    fn confirm_key(&mut self, editor: &Editor, tab_id: u64, key: KeyEvent) -> PromptReply {
        if key.kind == KeyEventKind::Release {
            return PromptReply::Handled;
        }
        if tab_name(editor, tab_id).is_none() {
            return PromptReply::Send(Command::CancelPrompt);
        }
        let count = CLOSE_BUTTONS.len();
        let index = match self.focus {
            PromptFocus::Button(index) => index.min(count - 1),
            PromptFocus::Input => 0,
        };
        match key.code {
            KeyCode::Left | KeyCode::BackTab => {
                self.focus = PromptFocus::Button((index + count - 1) % count);
                PromptReply::Handled
            }
            KeyCode::Right | KeyCode::Tab => {
                self.focus = PromptFocus::Button((index + 1) % count);
                PromptReply::Handled
            }
            KeyCode::Enter => CLOSE_BUTTONS.get(index).map_or(PromptReply::Handled, |b| {
                PromptReply::Send(Command::ResolveClose(b.1))
            }),
            KeyCode::Esc => PromptReply::Send(Command::ResolveClose(CloseChoice::Cancel)),
            _ => PromptReply::Handled,
        }
    }

    fn name_key(&mut self, editor: &mut Editor, key: KeyEvent) -> PromptReply {
        if key.kind == KeyEventKind::Release {
            return PromptReply::Handled;
        }
        if key.code == KeyCode::Esc {
            return PromptReply::Send(Command::CancelPrompt);
        }
        match (self.focus, key.code) {
            (PromptFocus::Input, KeyCode::Tab) => {
                self.focus = self.focus.next();
                PromptReply::Handled
            }
            (PromptFocus::Input, KeyCode::BackTab) => {
                self.focus = self.focus.prev();
                PromptReply::Handled
            }
            (PromptFocus::Input, _) => self.input_key(editor, key),
            (PromptFocus::Button(_), KeyCode::Tab | KeyCode::Right) => {
                self.focus = self.focus.next();
                PromptReply::Handled
            }
            (PromptFocus::Button(_), KeyCode::BackTab | KeyCode::Left) => {
                self.focus = self.focus.prev();
                PromptReply::Handled
            }
            (PromptFocus::Button(index), KeyCode::Enter) => name_button(index),
            (PromptFocus::Button(_), _) if is_printable(key) => {
                self.focus = PromptFocus::Input;
                self.input_key(editor, key)
            }
            (PromptFocus::Button(_), _) => PromptReply::Handled,
        }
    }

    /// Passes `key` to the name input.
    fn input_key(&mut self, editor: &mut Editor, key: KeyEvent) -> PromptReply {
        let Some(name) = editor.prompt_name_mut() else {
            return PromptReply::Ignored;
        };
        match self.input.handle_key(name, key) {
            InputOutcome::Submit => PromptReply::Send(Command::SubmitName),
            InputOutcome::Cancel => PromptReply::Send(Command::CancelPrompt),
            InputOutcome::Edited | InputOutcome::Moved | InputOutcome::Ignored => {
                PromptReply::Handled
            }
        }
    }

    /// Handles a click at `column` on `target` (the hit-map result) while
    /// a prompt is open. A click outside the popup cancels the prompt.
    pub fn handle_click(
        &mut self,
        editor: &mut Editor,
        target: Option<Target>,
        column: u16,
    ) -> PromptReply {
        self.sync(editor);
        match self.key {
            Some(PromptKey::ConfirmClose(tab_id)) => {
                if tab_name(editor, tab_id).is_none() {
                    return PromptReply::Send(Command::CancelPrompt);
                }
                match target {
                    Some(Target::PromptButton(index)) => {
                        CLOSE_BUTTONS.get(index).map_or(PromptReply::Handled, |b| {
                            PromptReply::Send(Command::ResolveClose(b.1))
                        })
                    }
                    Some(
                        Target::PromptPopup | Target::PromptInput | Target::PromptCandidate(_),
                    ) => PromptReply::Handled,
                    _ => PromptReply::Send(Command::ResolveClose(CloseChoice::Cancel)),
                }
            }
            Some(PromptKey::Name(..)) => match target {
                Some(Target::PromptButton(index)) => name_button(index),
                Some(Target::PromptInput) => {
                    self.focus = PromptFocus::Input;
                    if let (Some(name), Some(rect)) = (editor.prompt_name_mut(), self.input_rect) {
                        self.input.click(name, column.saturating_sub(rect.x));
                    }
                    PromptReply::Handled
                }
                Some(Target::PromptPopup | Target::PromptCandidate(_)) => PromptReply::Handled,
                _ => PromptReply::Send(Command::CancelPrompt),
            },
            Some(PromptKey::SavePath(_)) | None => PromptReply::Ignored,
        }
    }

    /// Inserts pasted `text` into the focused prompt's input, focusing it.
    pub fn handle_paste(&mut self, editor: &mut Editor, text: &str) {
        self.sync(editor);
        if let Some(PromptKey::Name(..)) = self.key
            && let Some(name) = editor.prompt_name_mut()
        {
            self.focus = PromptFocus::Input;
            self.input.insert_str(name, text);
        }
    }

    /// Draws the open prompt centred in `area`, recording its hit regions,
    /// and returns the cursor's cell when the input has focus.
    pub fn render(
        &mut self,
        editor: &Editor,
        area: Rect,
        buf: &mut Buffer,
        hits: &mut HitMap,
    ) -> Option<Position> {
        self.sync(editor);
        match editor.prompt() {
            Some(&Prompt::ConfirmClose { tab_id }) => {
                let name = tab_name(editor, tab_id)?;
                self.render_confirm(&name, area, buf, hits);
                None
            }
            Some(Prompt::Name(prompt)) => {
                let (title, label, action) = name_labels(prompt.kind);
                let height = if prompt.error.is_some() { 7 } else { 6 };
                let rect = popup::centered(area, NAME_WIDTH, height);
                let inner = popup::render_frame(title, rect, buf, hits);
                if inner.is_empty() {
                    return None;
                }
                let width = usize::from(inner.width);
                buf.set_stringn(inner.x, inner.y, label, width, theme::popup());

                let mut cursor = None;
                if inner.height > 1 {
                    let input_rect = Rect::new(inner.x, inner.y + 1, inner.width, 1);
                    hits.push(input_rect, Target::PromptInput);
                    self.input_rect = Some(input_rect);
                    let position = self.input.render(&prompt.name, input_rect, buf);
                    if self.focus == PromptFocus::Input {
                        cursor = position;
                    }
                }
                if let Some(error) = &prompt.error
                    && inner.height > 2
                {
                    let style = theme::status_error().bg(theme::popup().bg.unwrap_or_default());
                    buf.set_stringn(inner.x, inner.y + 2, error, width, style);
                }
                let focused = match self.focus {
                    PromptFocus::Button(index) => index,
                    PromptFocus::Input => usize::MAX,
                };
                let row = Rect::new(inner.x, inner.bottom() - 1, inner.width, 1);
                popup::render_buttons(&[action, "Cancel"], focused, row, buf, hits);
                cursor
            }
            Some(Prompt::SavePath { .. }) | None => None,
        }
    }

    fn render_confirm(&self, name: &str, area: Rect, buf: &mut Buffer, hits: &mut HitMap) {
        let message =
            format!("\u{201c}{name}\u{201d} has unsaved changes. What would you like to do?");
        let width = CONFIRM_WIDTH.min(area.width);
        let lines = popup::wrap(&message, usize::from(width.saturating_sub(2)));
        let height = u16::try_from(lines.len())
            .unwrap_or(u16::MAX)
            .saturating_add(4);
        let rect = popup::centered(area, width, height);
        let inner = popup::render_frame("Unsaved changes", rect, buf, hits);
        if inner.is_empty() {
            return;
        }
        let message_rows = inner.height.saturating_sub(2);
        for (y, line) in (inner.y..inner.y + message_rows).zip(&lines) {
            buf.set_stringn(inner.x, y, line, usize::from(inner.width), theme::popup());
        }
        let focused = match self.focus {
            PromptFocus::Button(index) => index,
            PromptFocus::Input => 0,
        };
        let labels = CLOSE_BUTTONS.map(|(label, _)| label);
        let row = Rect::new(inner.x, inner.bottom() - 1, inner.width, 1);
        popup::render_buttons(&labels, focused, row, buf, hits);
    }
}

#[cfg(test)]
mod tests {
    // `unwrap` is acceptable in test code: a panic on an unexpected `Err`
    // is exactly the failure signal we want from a test.
    #![allow(clippy::unwrap_used)]

    use std::fs;

    use frext_core::clipboard::MemoryClipboard;
    use frext_core::persistence::Store;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "frext-term-prompt-test-{tag}-{}",
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

    /// An editor over `notes.txt` with a dirty buffer and a pending
    /// close confirmation.
    fn confirm_fixture(tag: &str) -> (PathBuf, Editor, PromptUi) {
        let dir = temp_dir(tag);
        let file = dir.join("notes.txt");
        fs::write(&file, "hello").unwrap();
        let mut editor = open_editor(&dir, &[file], None);
        editor.active_view_mut().unwrap().tab.text.push('x');
        editor.commit_edit();
        editor.apply(Command::RequestCloseActive);
        assert!(matches!(editor.prompt(), Some(Prompt::ConfirmClose { .. })));
        let ui = PromptUi::new(dir.clone(), None);
        (dir, editor, ui)
    }

    /// An editor over a workspace holding `a.txt` and `b.txt`, renaming
    /// `a.txt`.
    fn rename_fixture(tag: &str) -> (PathBuf, Editor, PromptUi) {
        let dir = temp_dir(tag);
        let root = dir.join("root");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("a.txt"), "").unwrap();
        fs::write(root.join("b.txt"), "").unwrap();
        let mut editor = open_editor(&dir, &[], Some(&root));
        editor.apply(Command::BeginRename(root.join("a.txt")));
        let ui = PromptUi::new(dir.clone(), None);
        (dir, editor, ui)
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn send(command: Command) -> PromptReply {
        PromptReply::Send(command)
    }

    fn close(choice: CloseChoice) -> PromptReply {
        send(Command::ResolveClose(choice))
    }

    fn name_text(editor: &Editor) -> String {
        match editor.prompt() {
            Some(Prompt::Name(prompt)) => prompt.name.clone(),
            other => panic!("expected a name prompt, got {other:?}"),
        }
    }

    fn render(ui: &mut PromptUi, editor: &Editor) -> (Buffer, HitMap, Option<Position>) {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut hits = HitMap::default();
        let mut cursor = None;
        terminal
            .draw(|frame| {
                let area = frame.area();
                cursor = ui.render(editor, area, frame.buffer_mut(), &mut hits);
            })
            .unwrap();
        (terminal.backend().buffer().clone(), hits, cursor)
    }

    fn row_text(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
    }

    /// The column of `needle` in a row of single-width cells.
    fn column_of(row: &str, needle: &str) -> u16 {
        let byte = row.find(needle).unwrap();
        u16::try_from(row[..byte].chars().count()).unwrap()
    }

    fn screen_text(buf: &Buffer) -> String {
        (0..buf.area.height)
            .map(|y| row_text(buf, y))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn new_stores_base_and_home() {
        let ui = PromptUi::new(PathBuf::from("/work"), Some(PathBuf::from("/home/me")));
        assert_eq!(ui.base(), Path::new("/work"));
        assert_eq!(ui.home(), Some(Path::new("/home/me")));
        assert_eq!(PromptUi::new(PathBuf::from("/w"), None).home(), None);
    }

    #[test]
    fn no_prompt_ignores_everything_and_draws_nothing() {
        let dir = temp_dir("none");
        let mut editor = open_editor(&dir, &[], None);
        let mut ui = PromptUi::new(dir.clone(), None);
        assert_eq!(
            ui.handle_key(&mut editor, press(KeyCode::Enter)),
            PromptReply::Ignored
        );
        assert_eq!(ui.handle_click(&mut editor, None, 0), PromptReply::Ignored);
        ui.handle_paste(&mut editor, "x");
        let (buf, hits, cursor) = render(&mut ui, &editor);
        assert_eq!(cursor, None);
        assert_eq!(hits.hit(40, 12), None);
        assert!(screen_text(&buf).trim().is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn confirm_enter_on_default_button_saves() {
        let (dir, mut editor, mut ui) = confirm_fixture("confirm-enter");
        assert_eq!(
            ui.handle_key(&mut editor, press(KeyCode::Enter)),
            close(CloseChoice::Save)
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn confirm_tab_tab_enter_cancels() {
        let (dir, mut editor, mut ui) = confirm_fixture("confirm-tab");
        assert_eq!(
            ui.handle_key(&mut editor, press(KeyCode::Tab)),
            PromptReply::Handled
        );
        assert_eq!(
            ui.handle_key(&mut editor, press(KeyCode::Tab)),
            PromptReply::Handled
        );
        assert_eq!(
            ui.handle_key(&mut editor, press(KeyCode::Enter)),
            close(CloseChoice::Cancel)
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn confirm_focus_wraps_both_ways() {
        let (dir, mut editor, mut ui) = confirm_fixture("confirm-wrap");
        ui.handle_key(&mut editor, press(KeyCode::Left));
        assert_eq!(
            ui.handle_key(&mut editor, press(KeyCode::Enter)),
            close(CloseChoice::Cancel)
        );
        ui.handle_key(&mut editor, press(KeyCode::Right));
        assert_eq!(
            ui.handle_key(&mut editor, press(KeyCode::Enter)),
            close(CloseChoice::Save)
        );
        ui.handle_key(&mut editor, press(KeyCode::BackTab));
        ui.handle_key(&mut editor, press(KeyCode::BackTab));
        assert_eq!(
            ui.handle_key(&mut editor, press(KeyCode::Enter)),
            close(CloseChoice::Discard)
        );
        assert_eq!(
            ui.handle_key(&mut editor, press(KeyCode::Char('q'))),
            PromptReply::Handled
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn confirm_esc_cancels() {
        let (dir, mut editor, mut ui) = confirm_fixture("confirm-esc");
        assert_eq!(
            ui.handle_key(&mut editor, press(KeyCode::Esc)),
            close(CloseChoice::Cancel)
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn confirm_click_outside_cancels() {
        let (dir, mut editor, mut ui) = confirm_fixture("confirm-outside");
        assert_eq!(
            ui.handle_click(&mut editor, None, 0),
            close(CloseChoice::Cancel)
        );
        assert_eq!(
            ui.handle_click(&mut editor, Some(Target::Editor), 0),
            close(CloseChoice::Cancel)
        );
        assert_eq!(
            ui.handle_click(&mut editor, Some(Target::PromptPopup), 0),
            PromptReply::Handled
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn confirm_click_on_button_one_discards() {
        let (dir, mut editor, mut ui) = confirm_fixture("confirm-click");
        assert_eq!(
            ui.handle_click(&mut editor, Some(Target::PromptButton(1)), 0),
            close(CloseChoice::Discard)
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn confirm_click_through_hit_map_discards() {
        let (dir, mut editor, mut ui) = confirm_fixture("confirm-hitmap");
        let (buf, hits, _) = render(&mut ui, &editor);
        let row = (0..24)
            .find(|&y| row_text(&buf, y).contains("[ Discard changes ]"))
            .unwrap();
        let col = column_of(&row_text(&buf, row), "[ Discard");
        let target = hits.hit(col + 2, row);
        assert_eq!(target, Some(Target::PromptButton(1)));
        assert_eq!(
            ui.handle_click(&mut editor, target, col + 2),
            close(CloseChoice::Discard)
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn tab_name_finds_open_tabs_by_id_only() {
        let (dir, editor, _) = confirm_fixture("tab-name");
        let Some(&Prompt::ConfirmClose { tab_id }) = editor.prompt() else {
            panic!("expected a close confirmation");
        };
        assert_eq!(tab_name(&editor, tab_id).as_deref(), Some("notes.txt"));
        assert_eq!(tab_name(&editor, u64::MAX), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn confirm_render_shows_title_message_and_centred_buttons() {
        let (dir, editor, mut ui) = confirm_fixture("confirm-render");
        let (buf, hits, cursor) = render(&mut ui, &editor);
        assert_eq!(cursor, None);

        // 60 wide, centred in 80: the border starts at column 10. The
        // message wraps onto two lines, so the popup is 6 tall, rows 9..15.
        let top = row_text(&buf, 9);
        assert_eq!(&top[..], {
            let mut s = " ".repeat(10);
            s.push('\u{250c}');
            s.push_str("Unsaved changes");
            s.push_str(&"\u{2500}".repeat(43));
            s.push('\u{2510}');
            s.push_str(&" ".repeat(10));
            s
        });
        assert!(
            row_text(&buf, 10)
                .contains("\u{201c}notes.txt\u{201d} has unsaved changes. What would you like to")
        );
        assert!(row_text(&buf, 11).contains("do?"));
        let buttons = row_text(&buf, 13);
        assert!(buttons.contains("[ Save and close ] [ Discard changes ] [ Cancel ]\u{2502}"));
        assert!(row_text(&buf, 14).contains('\u{2518}'));

        // The default button is highlighted.
        let save = column_of(&buttons, "[ Save");
        assert_eq!(buf[(save, 13)].style().bg, theme::button_focused().bg);
        assert_eq!(hits.hit(40, 10), Some(Target::PromptPopup));
        assert_eq!(hits.hit(0, 0), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn name_typing_edits_core_text_and_enter_submits() {
        let (dir, mut editor, mut ui) = rename_fixture("name-type");
        assert_eq!(name_text(&editor), "a.txt");
        ui.handle_key(&mut editor, press(KeyCode::Backspace));
        ui.handle_key(&mut editor, press(KeyCode::Char('m')));
        assert_eq!(name_text(&editor), "a.txm");
        ui.handle_key(&mut editor, press(KeyCode::Home));
        ui.handle_key(&mut editor, press(KeyCode::Char('x')));
        assert_eq!(name_text(&editor), "xa.txm");
        assert_eq!(
            ui.handle_key(&mut editor, press(KeyCode::Enter)),
            send(Command::SubmitName)
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn name_esc_and_cancel_button_cancel() {
        let (dir, mut editor, mut ui) = rename_fixture("name-cancel");
        assert_eq!(
            ui.handle_key(&mut editor, press(KeyCode::Esc)),
            send(Command::CancelPrompt)
        );
        assert_eq!(
            ui.handle_click(&mut editor, Some(Target::PromptButton(1)), 0),
            send(Command::CancelPrompt)
        );
        assert_eq!(
            ui.handle_click(&mut editor, Some(Target::PromptButton(0)), 0),
            send(Command::SubmitName)
        );
        assert_eq!(
            ui.handle_click(&mut editor, None, 0),
            send(Command::CancelPrompt)
        );
        assert_eq!(
            ui.handle_click(&mut editor, Some(Target::PromptPopup), 0),
            PromptReply::Handled
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn name_tab_cycles_through_input_and_buttons() {
        let (dir, mut editor, mut ui) = rename_fixture("name-tab");
        ui.handle_key(&mut editor, press(KeyCode::Tab));
        assert_eq!(ui.focus, PromptFocus::Button(0));
        assert_eq!(
            ui.handle_key(&mut editor, press(KeyCode::Enter)),
            send(Command::SubmitName)
        );
        ui.handle_key(&mut editor, press(KeyCode::Right));
        assert_eq!(
            ui.handle_key(&mut editor, press(KeyCode::Enter)),
            send(Command::CancelPrompt)
        );
        ui.handle_key(&mut editor, press(KeyCode::Tab));
        assert_eq!(ui.focus, PromptFocus::Input);
        ui.handle_key(&mut editor, press(KeyCode::BackTab));
        assert_eq!(ui.focus, PromptFocus::Button(1));
        ui.handle_key(&mut editor, press(KeyCode::Left));
        assert_eq!(ui.focus, PromptFocus::Button(0));
        ui.handle_key(&mut editor, press(KeyCode::BackTab));
        assert_eq!(ui.focus, PromptFocus::Input);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn name_typing_on_a_button_moves_focus_to_input() {
        let (dir, mut editor, mut ui) = rename_fixture("name-button-type");
        ui.handle_key(&mut editor, press(KeyCode::Tab));
        assert_eq!(
            ui.handle_key(&mut editor, press(KeyCode::Backspace)),
            PromptReply::Handled
        );
        assert_eq!(name_text(&editor), "a.txt");
        ui.handle_key(&mut editor, press(KeyCode::Char('z')));
        assert_eq!(ui.focus, PromptFocus::Input);
        assert_eq!(name_text(&editor), "a.txtz");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn name_paste_inserts_into_the_field() {
        let (dir, mut editor, mut ui) = rename_fixture("name-paste");
        ui.handle_key(&mut editor, press(KeyCode::Tab));
        ui.handle_paste(&mut editor, "-copy\n");
        assert_eq!(ui.focus, PromptFocus::Input);
        assert_eq!(name_text(&editor), "a.txt-copy ");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn name_click_on_input_focuses_and_places_cursor() {
        let (dir, mut editor, mut ui) = rename_fixture("name-click");
        let (buf, hits, cursor) = render(&mut ui, &editor);
        // Width 50 centred in 80: border at 15, input starts at 16.
        assert_eq!(cursor, Some(Position::new(16 + 5, 11)));
        assert!(row_text(&buf, 9).contains("Rename"));
        assert!(row_text(&buf, 10).contains("New name"));
        assert!(row_text(&buf, 13).contains("[ Rename ] [ Cancel ]"));
        assert_eq!(hits.hit(16, 11), Some(Target::PromptInput));

        ui.handle_key(&mut editor, press(KeyCode::Tab));
        let (_, _, cursor) = render(&mut ui, &editor);
        assert_eq!(cursor, None);

        let target = hits.hit(17, 11);
        assert_eq!(
            ui.handle_click(&mut editor, target, 17),
            PromptReply::Handled
        );
        assert_eq!(ui.focus, PromptFocus::Input);
        ui.handle_key(&mut editor, press(KeyCode::Char('_')));
        assert_eq!(name_text(&editor), "a_.txt");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn name_failed_submit_renders_the_error() {
        let (dir, mut editor, mut ui) = rename_fixture("name-error");
        let name = editor.prompt_name_mut().unwrap();
        name.clear();
        name.push_str("b.txt");
        assert_eq!(
            ui.handle_key(&mut editor, press(KeyCode::Enter)),
            send(Command::SubmitName)
        );
        editor.apply(Command::SubmitName);
        let error = match editor.prompt() {
            Some(Prompt::Name(prompt)) => prompt.error.clone().unwrap(),
            other => panic!("expected a name prompt, got {other:?}"),
        };

        let (buf, _, cursor) = render(&mut ui, &editor);
        // Height 7 centred in 24: rows 8..15, the error on row 11.
        let error_row = row_text(&buf, 11);
        let shown: String = error.chars().take(48).collect();
        assert!(error_row.contains(&shown), "{error_row:?} lacks {shown:?}");
        assert_eq!(buf[(16, 11)].style().fg, theme::status_error().fg);
        assert_eq!(buf[(16, 11)].style().bg, theme::popup().bg);
        assert!(row_text(&buf, 13).contains("[ Rename ] [ Cancel ]"));
        // The same prompt kept its view state.
        assert_eq!(cursor, Some(Position::new(16 + 5, 10)));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sync_resets_focus_when_a_new_prompt_replaces_the_old() {
        let (dir, mut editor, mut ui) = rename_fixture("sync");
        ui.handle_key(&mut editor, press(KeyCode::Tab));
        assert_eq!(ui.focus, PromptFocus::Button(0));
        // Re-syncing the same prompt keeps the focus.
        ui.sync(&editor);
        assert_eq!(ui.focus, PromptFocus::Button(0));

        let root = dir.join("root");
        editor.apply(Command::BeginRename(root.join("b.txt")));
        ui.sync(&editor);
        assert_eq!(ui.focus, PromptFocus::Input);
        assert_eq!(ui.input.cursor(), "b.txt".len());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn focus_field_focuses_the_input_at_the_end() {
        let (dir, mut editor, mut ui) = rename_fixture("focus-field");
        ui.handle_key(&mut editor, press(KeyCode::Home));
        ui.handle_key(&mut editor, press(KeyCode::Tab));
        ui.focus_field(&editor);
        assert_eq!(ui.focus, PromptFocus::Input);
        assert_eq!(ui.input.cursor(), "a.txt".len());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn name_labels_match_the_gui() {
        assert_eq!(
            name_labels(NameKind::Rename),
            ("Rename", "New name", "Rename")
        );
        assert_eq!(
            name_labels(NameKind::NewFile),
            ("New file", "File name", "Create")
        );
        assert_eq!(
            name_labels(NameKind::NewFolder),
            ("New folder", "Folder name", "Create")
        );
    }
}
