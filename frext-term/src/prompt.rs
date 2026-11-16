//! The terminal view of the editor's modal prompts.
//!
//! Core owns the [`Prompt`] itself and, for a name prompt, its text.
//! [`PromptUi`] owns only the view state: which control has focus, the
//! input's cursor and where the input was last drawn, plus the text and
//! completion state of the two path prompts (save-as, and the TUI-only
//! open-file prompt). It never applies a command itself; it returns
//! [`PromptReply::Send`] and the app sends it.

use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use frext_core::editor::{CloseChoice, Command, Editor, NameKind, Prompt, StepDirection};
use frext_core::tab::Tab;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};

use crate::complete::{Completion, CompletionView, resolve};
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

/// The width of the path popups.
const PATH_WIDTH: u16 = 70;

/// The most completion candidates a path popup lists at once.
const MAX_CANDIDATES: usize = 8;

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
    /// The TUI-only open-file prompt.
    Open,
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

/// Whether the TUI-only open-file prompt is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenState {
    Closed,
    Open,
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
    open: OpenState,
    path_text: String,
    path_error: Option<String>,
    completion: Completion,
    candidates: CompletionView,
    overwrite_armed: Option<PathBuf>,
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
            open: OpenState::Closed,
            path_text: String::new(),
            path_error: None,
            completion: Completion::default(),
            candidates: CompletionView::default(),
            overwrite_armed: None,
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

    /// The directory relative paths in a path prompt resolve against:
    /// the workspace root when there is one, else the launch directory.
    fn effective_base(&self, editor: &Editor) -> PathBuf {
        editor
            .workspace()
            .map_or_else(|| self.base.clone(), |workspace| workspace.root.clone())
    }

    /// Whether a prompt is showing: the editor's, or the local open-file
    /// prompt.
    #[must_use]
    pub fn is_open(&self, editor: &Editor) -> bool {
        editor.prompt().is_some() || self.open == OpenState::Open
    }

    /// Starts the open-file prompt with an empty field. Returns `false`,
    /// without opening it, while the editor has a prompt of its own.
    pub fn open_file(&mut self, editor: &Editor) -> bool {
        if editor.prompt().is_some() {
            return false;
        }
        self.open = OpenState::Open;
        self.reset_path();
        self.input = TextInput::new();
        self.focus = PromptFocus::Input;
        true
    }

    /// Clears the path prompts' text, error, completion and arming.
    fn reset_path(&mut self) {
        self.path_text.clear();
        self.path_error = None;
        self.completion.reset();
        self.candidates = CompletionView::default();
        self.overwrite_armed = None;
    }

    /// Resets the view state when the editor's prompt differs from the
    /// one last seen: the focus goes to the prompt's first control and
    /// the input cursor to the end of its text. A prompt from the editor
    /// closes the local open-file prompt.
    pub fn sync(&mut self, editor: &Editor) {
        let key = match editor.prompt() {
            Some(prompt) => {
                self.open = OpenState::Closed;
                Some(PromptKey::of(prompt))
            }
            None => (self.open == OpenState::Open).then_some(PromptKey::Open),
        };
        if key == self.key {
            return;
        }
        self.focus = match key {
            Some(PromptKey::Name(..) | PromptKey::SavePath(_) | PromptKey::Open) => {
                PromptFocus::Input
            }
            Some(PromptKey::ConfirmClose(_)) | None => PromptFocus::Button(0),
        };
        self.input = TextInput::new();
        if let Some(Prompt::Name(prompt)) = editor.prompt() {
            self.input.move_to_end(&prompt.name);
        }
        self.input_rect = None;
        self.reset_path();
        self.key = key;
    }

    /// Focuses the input field (for `Effect::FocusPromptField`) and moves
    /// its cursor to the end.
    pub fn focus_field(&mut self, editor: &Editor) {
        self.sync(editor);
        match (&self.key, editor.prompt()) {
            (_, Some(Prompt::Name(prompt))) => {
                self.focus = PromptFocus::Input;
                self.input.move_to_end(&prompt.name);
            }
            (Some(PromptKey::SavePath(_) | PromptKey::Open), _) => {
                self.focus = PromptFocus::Input;
                self.input.move_to_end(&self.path_text);
            }
            _ => {}
        }
    }

    /// Handles a key while a prompt is open. Keys the prompt does not
    /// bind are swallowed, since the prompt is modal.
    pub fn handle_key(&mut self, editor: &mut Editor, key: KeyEvent) -> PromptReply {
        self.sync(editor);
        match self.key {
            Some(PromptKey::ConfirmClose(tab_id)) => self.confirm_key(editor, tab_id, key),
            Some(PromptKey::Name(..)) => self.name_key(editor, key),
            Some(PromptKey::SavePath(_) | PromptKey::Open) => self.path_key(editor, key),
            None => PromptReply::Ignored,
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

    /// Handles a key in a path prompt. Focus stays on the field: Tab and
    /// Shift+Tab complete, Enter submits and Esc cancels.
    fn path_key(&mut self, editor: &Editor, key: KeyEvent) -> PromptReply {
        if key.kind == KeyEventKind::Release {
            return PromptReply::Handled;
        }
        self.focus = PromptFocus::Input;
        match key.code {
            KeyCode::Tab => {
                self.complete(editor, StepDirection::Forward);
                return PromptReply::Handled;
            }
            KeyCode::BackTab => {
                self.complete(editor, StepDirection::Backward);
                return PromptReply::Handled;
            }
            _ => {}
        }
        match self.input.handle_key(&mut self.path_text, key) {
            InputOutcome::Edited => {
                self.path_edited();
                PromptReply::Handled
            }
            InputOutcome::Moved | InputOutcome::Ignored => PromptReply::Handled,
            InputOutcome::Submit => self.path_submit(editor),
            InputOutcome::Cancel => self.path_cancel(),
        }
    }

    /// Runs one Tab-completion step on the path field.
    fn complete(&mut self, editor: &Editor, direction: StepDirection) {
        let base = self.effective_base(editor);
        self.candidates =
            self.completion
                .tab(&mut self.path_text, &base, self.home.as_deref(), direction);
        self.input.move_to_end(&self.path_text);
        self.path_error = None;
        self.overwrite_armed = None;
    }

    /// Drops the completion, error and overwrite arming after an edit.
    fn path_edited(&mut self) {
        self.completion.reset();
        self.candidates = CompletionView::default();
        self.overwrite_armed = None;
        self.path_error = None;
    }

    /// Submits the path field of the current path prompt.
    fn path_submit(&mut self, editor: &Editor) -> PromptReply {
        if self.path_text.is_empty() {
            self.path_error = Some("Enter a file name".to_string());
            return PromptReply::Handled;
        }
        let base = self.effective_base(editor);
        let path = resolve(&self.path_text, &base, self.home.as_deref());
        match self.key {
            Some(PromptKey::SavePath(_)) => self.submit_save(path),
            Some(PromptKey::Open) => self.submit_open(path),
            _ => PromptReply::Handled,
        }
    }

    /// Validates a save-as target; an existing file needs a second Enter.
    fn submit_save(&mut self, path: PathBuf) -> PromptReply {
        if path.is_dir() {
            self.path_error = Some(format!("\u{201c}{}\u{201d} is a directory", path.display()));
            return PromptReply::Handled;
        }
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
            && !parent.is_dir()
        {
            self.path_error = Some(format!(
                "Folder \u{201c}{}\u{201d} does not exist",
                parent.display()
            ));
            return PromptReply::Handled;
        }
        if path.exists() && self.overwrite_armed.as_ref() != Some(&path) {
            let name = path
                .file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
            self.path_error = Some(format!(
                "\u{201c}{name}\u{201d} exists. Press Enter again to replace it"
            ));
            self.overwrite_armed = Some(path);
            return PromptReply::Handled;
        }
        PromptReply::Send(Command::SubmitSavePath(path))
    }

    /// Opens an existing file and closes the local prompt.
    fn submit_open(&mut self, path: PathBuf) -> PromptReply {
        if path.is_dir() {
            self.path_error = Some(format!("\u{201c}{}\u{201d} is a directory", path.display()));
            return PromptReply::Handled;
        }
        if !path.exists() {
            self.path_error = Some(format!("No such file: {}", path.display()));
            return PromptReply::Handled;
        }
        self.open = OpenState::Closed;
        PromptReply::Send(Command::OpenPath(path))
    }

    /// Cancels the current path prompt: save-as asks core, open closes
    /// locally.
    fn path_cancel(&mut self) -> PromptReply {
        match self.key {
            Some(PromptKey::SavePath(_)) => PromptReply::Send(Command::CancelPrompt),
            Some(PromptKey::Open) => {
                self.open = OpenState::Closed;
                PromptReply::Handled
            }
            _ => PromptReply::Handled,
        }
    }

    /// Handles a click on `target` in a path prompt.
    fn path_click(&mut self, editor: &Editor, target: Option<Target>, column: u16) -> PromptReply {
        match target {
            Some(Target::PromptButton(0)) => self.path_submit(editor),
            Some(Target::PromptButton(1)) => self.path_cancel(),
            Some(Target::PromptInput) => {
                self.focus = PromptFocus::Input;
                if let Some(rect) = self.input_rect {
                    self.input
                        .click(&self.path_text, column.saturating_sub(rect.x));
                }
                PromptReply::Handled
            }
            Some(Target::PromptCandidate(index)) => {
                if let Some(candidate) = self.candidates.candidates.get(index).cloned() {
                    self.path_text = candidate;
                    self.path_edited();
                    self.focus = PromptFocus::Input;
                    self.input.move_to_end(&self.path_text);
                }
                PromptReply::Handled
            }
            Some(Target::PromptPopup | Target::PromptButton(_)) => PromptReply::Handled,
            _ => self.path_cancel(),
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
            Some(PromptKey::SavePath(_) | PromptKey::Open) => {
                self.path_click(editor, target, column)
            }
            None => PromptReply::Ignored,
        }
    }

    /// Inserts pasted `text` into the focused prompt's input, focusing it.
    pub fn handle_paste(&mut self, editor: &mut Editor, text: &str) {
        self.sync(editor);
        match self.key {
            Some(PromptKey::Name(..)) => {
                if let Some(name) = editor.prompt_name_mut() {
                    self.focus = PromptFocus::Input;
                    self.input.insert_str(name, text);
                }
            }
            Some(PromptKey::SavePath(_) | PromptKey::Open) => {
                self.focus = PromptFocus::Input;
                self.input.insert_str(&mut self.path_text, text);
                self.path_edited();
            }
            Some(PromptKey::ConfirmClose(_)) | None => {}
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
            Some(Prompt::SavePath { .. }) => {
                self.render_path(editor, ("Save as", "Save"), area, buf, hits)
            }
            None if self.open == OpenState::Open => {
                self.render_path(editor, ("Open file", "Open"), area, buf, hits)
            }
            None => None,
        }
    }

    /// Draws a path prompt titled `labels.0` with action button
    /// `labels.1`: the field, the base hint, any error, the candidates
    /// and the buttons.
    fn render_path(
        &mut self,
        editor: &Editor,
        (title, action): (&str, &str),
        area: Rect,
        buf: &mut Buffer,
        hits: &mut HitMap,
    ) -> Option<Position> {
        let total = self.candidates.candidates.len();
        let shown = total.min(MAX_CANDIDATES);
        let more = total > MAX_CANDIDATES;
        let rows = 5 + usize::from(self.path_error.is_some()) + shown + usize::from(more);
        let height = u16::try_from(rows).unwrap_or(u16::MAX);
        let rect = popup::centered(area, PATH_WIDTH.min(area.width), height);
        let inner = popup::render_frame(title, rect, buf, hits);
        if inner.is_empty() {
            return None;
        }
        let width = usize::from(inner.width);
        let limit = inner.bottom() - 1;
        let mut y = inner.y;

        let mut cursor = None;
        if y < limit {
            let input_rect = Rect::new(inner.x, y, inner.width, 1);
            hits.push(input_rect, Target::PromptInput);
            self.input_rect = Some(input_rect);
            let position = self.input.render(&self.path_text, input_rect, buf);
            if self.focus == PromptFocus::Input {
                cursor = position;
            }
            y += 1;
        }
        if y < limit {
            let hint = format!("relative to {}", self.effective_base(editor).display());
            buf.set_stringn(inner.x, y, hint, width, theme::popup());
            y += 1;
        }
        if let Some(error) = &self.path_error
            && y < limit
        {
            let style = theme::status_error().bg(theme::popup().bg.unwrap_or_default());
            buf.set_stringn(inner.x, y, error, width, style);
            y += 1;
        }
        let start = self
            .candidates
            .selected
            .map_or(0, |selected| selected.saturating_sub(MAX_CANDIDATES - 1));
        for index in start..start + shown {
            if y >= limit {
                break;
            }
            let Some(candidate) = self.candidates.candidates.get(index) else {
                break;
            };
            let row = Rect::new(inner.x, y, inner.width, 1);
            let style = if self.candidates.selected == Some(index) {
                theme::button_focused()
            } else {
                theme::popup()
            };
            buf.set_style(row, style);
            buf.set_stringn(inner.x, y, candidate, width, style);
            hits.push(row, Target::PromptCandidate(index));
            y += 1;
        }
        if more && y < limit {
            let text = format!("\u{2026} {} more", total - shown);
            buf.set_stringn(inner.x, y, text, width, theme::popup());
        }

        let row = Rect::new(inner.x, limit, inner.width, 1);
        popup::render_buttons(&[action, "Cancel"], usize::MAX, row, buf, hits);
        cursor
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

    /// Types `text` into the prompt one key at a time.
    fn type_text(ui: &mut PromptUi, editor: &mut Editor, text: &str) {
        for c in text.chars() {
            assert_eq!(
                ui.handle_key(editor, press(KeyCode::Char(c))),
                PromptReply::Handled
            );
        }
    }

    /// An editor with an untitled tab and a pending save-as prompt,
    /// resolving relative paths against `dir/files`.
    fn save_fixture(tag: &str) -> (PathBuf, PathBuf, Editor, PromptUi) {
        let dir = temp_dir(tag);
        let files = dir.join("files");
        fs::create_dir_all(&files).unwrap();
        let mut editor = open_editor(&dir, &[], None);
        editor.apply(Command::Save);
        assert!(matches!(editor.prompt(), Some(Prompt::SavePath { .. })));
        let ui = PromptUi::new(files.clone(), None);
        (dir, files, editor, ui)
    }

    /// An editor with no prompt and the open-file prompt showing,
    /// resolving relative paths against `dir/files`.
    fn open_fixture(tag: &str) -> (PathBuf, PathBuf, Editor, PromptUi) {
        let dir = temp_dir(tag);
        let files = dir.join("files");
        fs::create_dir_all(&files).unwrap();
        let editor = open_editor(&dir, &[], None);
        let mut ui = PromptUi::new(files.clone(), None);
        assert!(!ui.is_open(&editor));
        assert!(ui.open_file(&editor));
        assert!(ui.is_open(&editor));
        (dir, files, editor, ui)
    }

    /// The row holding `needle`, if any.
    fn find_row(buf: &Buffer, needle: &str) -> Option<u16> {
        find_row_from(buf, 0, needle)
    }

    /// The first row at or below `start` holding `needle`, if any.
    fn find_row_from(buf: &Buffer, start: u16, needle: &str) -> Option<u16> {
        (start..buf.area.height).find(|&y| row_text(buf, y).contains(needle))
    }

    /// The first 68 chars of `text`: what fits inside a 70-wide popup.
    fn fitted(text: &str) -> String {
        text.chars().take(68).collect()
    }

    #[test]
    fn save_path_to_a_new_file_submits_the_resolved_path() {
        let (dir, files, mut editor, mut ui) = save_fixture("save-new");
        type_text(&mut ui, &mut editor, "new.txt");
        assert_eq!(
            ui.handle_key(&mut editor, press(KeyCode::Enter)),
            send(Command::SubmitSavePath(files.join("new.txt")))
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn save_path_with_an_empty_field_asks_for_a_name() {
        let (dir, _, mut editor, mut ui) = save_fixture("save-empty");
        assert_eq!(
            ui.handle_key(&mut editor, press(KeyCode::Enter)),
            PromptReply::Handled
        );
        assert_eq!(ui.path_error.as_deref(), Some("Enter a file name"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn save_path_to_an_existing_file_needs_two_enters() {
        let (dir, files, mut editor, mut ui) = save_fixture("save-existing");
        let target = files.join("old.txt");
        fs::write(&target, "old").unwrap();
        type_text(&mut ui, &mut editor, "old.txt");
        assert_eq!(
            ui.handle_key(&mut editor, press(KeyCode::Enter)),
            PromptReply::Handled
        );
        assert_eq!(
            ui.path_error.as_deref(),
            Some("\u{201c}old.txt\u{201d} exists. Press Enter again to replace it")
        );
        assert_eq!(ui.overwrite_armed.as_ref(), Some(&target));
        assert_eq!(
            ui.handle_key(&mut editor, press(KeyCode::Enter)),
            send(Command::SubmitSavePath(target))
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn editing_disarms_the_overwrite() {
        let (dir, files, mut editor, mut ui) = save_fixture("save-disarm");
        fs::write(files.join("old.txt"), "old").unwrap();
        type_text(&mut ui, &mut editor, "old.txt");
        ui.handle_key(&mut editor, press(KeyCode::Enter));
        assert!(ui.overwrite_armed.is_some());
        ui.handle_key(&mut editor, press(KeyCode::Backspace));
        type_text(&mut ui, &mut editor, "t");
        assert_eq!(ui.overwrite_armed, None);
        assert_eq!(ui.path_error, None);
        assert_eq!(
            ui.handle_key(&mut editor, press(KeyCode::Enter)),
            PromptReply::Handled
        );
        assert!(ui.overwrite_armed.is_some());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn save_path_to_a_directory_or_missing_parent_shows_errors() {
        let (dir, files, mut editor, mut ui) = save_fixture("save-errors");
        fs::create_dir_all(files.join("sub")).unwrap();
        type_text(&mut ui, &mut editor, "sub");
        assert_eq!(
            ui.handle_key(&mut editor, press(KeyCode::Enter)),
            PromptReply::Handled
        );
        let expected = format!(
            "\u{201c}{}\u{201d} is a directory",
            files.join("sub").display()
        );
        assert_eq!(ui.path_error.as_deref(), Some(expected.as_str()));

        ui.handle_paste(&mut editor, "/nope/x.txt");
        assert_eq!(ui.path_error, None);
        assert_eq!(
            ui.handle_key(&mut editor, press(KeyCode::Enter)),
            PromptReply::Handled
        );
        let expected = format!(
            "Folder \u{201c}{}\u{201d} does not exist",
            files.join("sub/nope").display()
        );
        assert_eq!(ui.path_error.as_deref(), Some(expected.as_str()));

        let (buf, _, _) = render(&mut ui, &editor);
        let row = find_row(&buf, "Folder \u{201c}").unwrap();
        let col = column_of(&row_text(&buf, row), "Folder");
        assert_eq!(buf[(col, row)].style().fg, theme::status_error().fg);
        assert!(find_row(&buf, "[ Save ] [ Cancel ]").is_some());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn save_path_cancels_through_core() {
        let (dir, _, mut editor, mut ui) = save_fixture("save-cancel");
        assert_eq!(
            ui.handle_key(&mut editor, press(KeyCode::Esc)),
            send(Command::CancelPrompt)
        );
        assert_eq!(
            ui.handle_click(&mut editor, Some(Target::PromptButton(1)), 0),
            send(Command::CancelPrompt)
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
    fn save_path_button_zero_submits() {
        let (dir, files, mut editor, mut ui) = save_fixture("save-button");
        type_text(&mut ui, &mut editor, "a.txt");
        assert_eq!(
            ui.handle_click(&mut editor, Some(Target::PromptButton(0)), 0),
            send(Command::SubmitSavePath(files.join("a.txt")))
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn save_path_text_starts_empty_for_each_new_prompt() {
        let (dir, _, mut editor, mut ui) = save_fixture("save-reset");
        type_text(&mut ui, &mut editor, "draft");
        editor.apply(Command::CancelPrompt);
        ui.sync(&editor);
        editor.apply(Command::Save);
        ui.sync(&editor);
        assert_eq!(ui.path_text, "");
        assert_eq!(ui.focus, PromptFocus::Input);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn open_file_is_refused_while_a_core_prompt_is_set() {
        let (dir, mut editor, mut ui) = confirm_fixture("open-refused");
        assert!(!ui.open_file(&editor));
        assert!(ui.is_open(&editor));
        assert_eq!(ui.open, OpenState::Closed);
        // The confirmation still owns the keys.
        assert_eq!(
            ui.handle_key(&mut editor, press(KeyCode::Enter)),
            close(CloseChoice::Save)
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn open_to_a_file_sends_open_path_and_closes() {
        let (dir, files, mut editor, mut ui) = open_fixture("open-file");
        fs::write(files.join("notes.txt"), "hi").unwrap();
        type_text(&mut ui, &mut editor, "notes.txt");
        assert_eq!(
            ui.handle_key(&mut editor, press(KeyCode::Enter)),
            send(Command::OpenPath(files.join("notes.txt")))
        );
        assert!(!ui.is_open(&editor));
        assert_eq!(
            ui.handle_key(&mut editor, press(KeyCode::Enter)),
            PromptReply::Ignored
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn open_to_a_directory_or_missing_file_shows_errors() {
        let (dir, files, mut editor, mut ui) = open_fixture("open-errors");
        fs::create_dir_all(files.join("sub")).unwrap();
        assert_eq!(
            ui.handle_key(&mut editor, press(KeyCode::Enter)),
            PromptReply::Handled
        );
        assert_eq!(ui.path_error.as_deref(), Some("Enter a file name"));

        type_text(&mut ui, &mut editor, "sub");
        assert_eq!(
            ui.handle_key(&mut editor, press(KeyCode::Enter)),
            PromptReply::Handled
        );
        let expected = format!(
            "\u{201c}{}\u{201d} is a directory",
            files.join("sub").display()
        );
        assert_eq!(ui.path_error.as_deref(), Some(expected.as_str()));
        assert!(ui.is_open(&editor));
        let (buf, _, _) = render(&mut ui, &editor);
        assert!(find_row(&buf, &fitted(&expected)).is_some());

        type_text(&mut ui, &mut editor, "/x.txt");
        ui.handle_key(&mut editor, press(KeyCode::Enter));
        let expected = format!("No such file: {}", files.join("sub/x.txt").display());
        assert_eq!(ui.path_error.as_deref(), Some(expected.as_str()));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn open_cancels_locally() {
        let (dir, _, mut editor, mut ui) = open_fixture("open-cancel");
        assert_eq!(
            ui.handle_key(&mut editor, press(KeyCode::Esc)),
            PromptReply::Handled
        );
        assert!(!ui.is_open(&editor));

        assert!(ui.open_file(&editor));
        assert_eq!(
            ui.handle_click(&mut editor, Some(Target::PromptButton(1)), 0),
            PromptReply::Handled
        );
        assert!(!ui.is_open(&editor));

        assert!(ui.open_file(&editor));
        assert_eq!(
            ui.handle_click(&mut editor, Some(Target::Editor), 0),
            PromptReply::Handled
        );
        assert!(!ui.is_open(&editor));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn open_file_clears_the_previous_text() {
        let (dir, _, mut editor, mut ui) = open_fixture("open-clear");
        type_text(&mut ui, &mut editor, "abc");
        ui.handle_key(&mut editor, press(KeyCode::Enter));
        assert!(ui.path_error.is_some());
        ui.handle_key(&mut editor, press(KeyCode::Esc));
        assert!(ui.open_file(&editor));
        assert_eq!(ui.path_text, "");
        assert_eq!(ui.path_error, None);
        let (_, _, cursor) = render(&mut ui, &editor);
        assert!(cursor.is_some());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_core_prompt_closes_the_local_open_prompt() {
        let (dir, _, mut editor, mut ui) = open_fixture("open-core-wins");
        editor.apply(Command::Save);
        assert!(matches!(editor.prompt(), Some(Prompt::SavePath { .. })));
        ui.sync(&editor);
        assert_eq!(ui.open, OpenState::Closed);
        assert_eq!(ui.key, Some(PromptKey::SavePath(editor.tabs()[0].id)));
        let (buf, _, _) = render(&mut ui, &editor);
        assert!(find_row(&buf, "Save as").is_some());
        assert!(find_row(&buf, "Open file").is_none());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn tab_completes_and_the_candidates_render() {
        let (dir, files, mut editor, mut ui) = open_fixture("open-complete");
        fs::write(files.join("alpha.txt"), "").unwrap();
        fs::write(files.join("alpha.md"), "").unwrap();
        type_text(&mut ui, &mut editor, "al");
        ui.handle_key(&mut editor, press(KeyCode::Tab));
        assert_eq!(ui.path_text, "alpha.");
        assert!(ui.candidates.candidates.is_empty());

        ui.handle_key(&mut editor, press(KeyCode::Tab));
        assert_eq!(ui.path_text, "alpha.md");
        assert_eq!(ui.candidates.selected, Some(0));

        let (buf, hits, cursor) = render(&mut ui, &editor);
        let title = find_row(&buf, "Open file").unwrap();
        // Width 70 centred in 80: the border is at 5, the field at 6.
        assert_eq!(cursor, Some(Position::new(6 + 8, title + 1)));
        let hint = format!("relative to {}", files.display());
        assert!(row_text(&buf, title + 2).contains(&fitted(&hint)));
        let md = find_row_from(&buf, title + 3, "alpha.md ").unwrap();
        assert_eq!(md, title + 3);
        assert_eq!(buf[(6, md)].style().bg, theme::button_focused().bg);
        assert_eq!(hits.hit(6, md), Some(Target::PromptCandidate(0)));
        assert!(row_text(&buf, md + 1).contains("alpha.txt"));
        assert_eq!(buf[(6, md + 1)].style().bg, theme::popup().bg);
        assert_eq!(hits.hit(6, md + 1), Some(Target::PromptCandidate(1)));
        assert!(row_text(&buf, md + 2).contains("[ Open ] [ Cancel ]"));

        ui.handle_key(&mut editor, press(KeyCode::BackTab));
        assert_eq!(ui.path_text, "alpha.txt");
        ui.handle_key(&mut editor, press(KeyCode::Backspace));
        assert!(ui.candidates.candidates.is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn long_candidate_lists_show_more_and_scroll_to_the_selection() {
        let (dir, files, mut editor, mut ui) = open_fixture("open-more");
        for i in 0..10 {
            fs::write(files.join(format!("f{i:02}")), "").unwrap();
        }
        type_text(&mut ui, &mut editor, "f0");
        ui.handle_key(&mut editor, press(KeyCode::Tab));
        assert_eq!(ui.candidates.candidates.len(), 10);
        let (buf, _, _) = render(&mut ui, &editor);
        let list = find_row(&buf, "Open file").unwrap() + 3;
        assert!(row_text(&buf, list).contains("f00 "));
        assert!(row_text(&buf, list + 7).contains("f07 "));
        assert!(row_text(&buf, list + 8).contains("\u{2026} 2 more"));
        assert!(row_text(&buf, list + 9).contains("[ Open ] [ Cancel ]"));

        for _ in 0..8 {
            ui.handle_key(&mut editor, press(KeyCode::Tab));
        }
        assert_eq!(ui.candidates.selected, Some(8));
        let (buf, hits, _) = render(&mut ui, &editor);
        assert!(find_row(&buf, "f00 ").is_none());
        assert!(row_text(&buf, list).contains("f01 "));
        assert_eq!(hits.hit(6, list), Some(Target::PromptCandidate(1)));
        assert!(row_text(&buf, list + 7).contains("f08 "));
        assert_eq!(hits.hit(6, list + 7), Some(Target::PromptCandidate(8)));
        assert_eq!(buf[(6, list + 7)].style().bg, theme::button_focused().bg);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn clicking_a_candidate_fills_the_field() {
        let (dir, files, mut editor, mut ui) = open_fixture("open-click");
        fs::write(files.join("alpha.txt"), "").unwrap();
        fs::write(files.join("alpha.md"), "").unwrap();
        type_text(&mut ui, &mut editor, "alpha.");
        ui.handle_key(&mut editor, press(KeyCode::Tab));
        let (buf, hits, _) = render(&mut ui, &editor);
        let row = find_row(&buf, "alpha.txt").unwrap();
        let target = hits.hit(10, row);
        assert_eq!(target, Some(Target::PromptCandidate(1)));
        assert_eq!(
            ui.handle_click(&mut editor, target, 10),
            PromptReply::Handled
        );
        assert_eq!(ui.path_text, "alpha.txt");
        assert!(ui.candidates.candidates.is_empty());
        assert_eq!(ui.input.cursor(), "alpha.txt".len());
        assert_eq!(
            ui.handle_key(&mut editor, press(KeyCode::Enter)),
            send(Command::OpenPath(files.join("alpha.txt")))
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn clicking_the_path_field_places_the_cursor() {
        let (dir, _, mut editor, mut ui) = open_fixture("open-field-click");
        type_text(&mut ui, &mut editor, "abc");
        let (buf, hits, _) = render(&mut ui, &editor);
        let row = find_row(&buf, "Open file").unwrap() + 1;
        let target = hits.hit(7, row);
        assert_eq!(target, Some(Target::PromptInput));
        ui.handle_click(&mut editor, target, 7);
        type_text(&mut ui, &mut editor, "_");
        assert_eq!(ui.path_text, "a_bc");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn paste_inserts_into_the_path_field() {
        let (dir, files, mut editor, mut ui) = open_fixture("open-paste");
        type_text(&mut ui, &mut editor, "a");
        ui.handle_paste(&mut editor, "b/c\n");
        assert_eq!(ui.path_text, "ab/c ");
        ui.handle_key(&mut editor, press(KeyCode::Backspace));
        ui.handle_key(&mut editor, press(KeyCode::Enter));
        let expected = format!("No such file: {}", files.join("ab/c").display());
        assert_eq!(ui.path_error.as_deref(), Some(expected.as_str()));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_workspace_root_is_the_base_when_there_is_one() {
        let dir = temp_dir("open-workspace");
        let root = dir.join("root");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("w.txt"), "").unwrap();
        let mut editor = open_editor(&dir, &[], Some(&root));
        let mut ui = PromptUi::new(dir.join("elsewhere"), None);
        assert!(ui.open_file(&editor));
        let (buf, _, _) = render(&mut ui, &editor);
        let hint = format!("relative to {}", root.display());
        assert!(find_row(&buf, &fitted(&hint)).is_some());
        type_text(&mut ui, &mut editor, "w.txt");
        assert_eq!(
            ui.handle_key(&mut editor, press(KeyCode::Enter)),
            send(Command::OpenPath(root.join("w.txt")))
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn tilde_expands_to_home_in_path_prompts() {
        let (dir, files, mut editor, _) = open_fixture("open-home");
        let mut ui = PromptUi::new(dir.clone(), Some(files.clone()));
        assert!(ui.open_file(&editor));
        fs::write(files.join("h.txt"), "").unwrap();
        type_text(&mut ui, &mut editor, "~/h.txt");
        assert_eq!(
            ui.handle_key(&mut editor, press(KeyCode::Enter)),
            send(Command::OpenPath(files.join("h.txt")))
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn focus_field_moves_the_path_cursor_to_the_end() {
        let (dir, _, mut editor, mut ui) = save_fixture("save-focus");
        type_text(&mut ui, &mut editor, "abc");
        ui.handle_key(&mut editor, press(KeyCode::Home));
        ui.focus_field(&editor);
        assert_eq!(ui.focus, PromptFocus::Input);
        assert_eq!(ui.input.cursor(), 3);
        fs::remove_dir_all(&dir).unwrap();
    }
}
