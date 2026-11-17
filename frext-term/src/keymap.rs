//! Keybindings: a pure mapping from a terminal [`KeyEvent`] to an
//! [`Action`].
//!
//! It covers the tab, search and focus shortcuts, and, while the editor
//! has focus, the editing keys: typing, deleting, caret motion (Shift
//! extends the selection), clipboard, and undo/redo.
//!
//! While the find bar is open, Alt+C / Alt+W / Alt+R / Alt+L toggle
//! match case, whole word, regex and in-selection, whatever has focus.
//! An upper-case letter with Alt+Shift toggles too, which is how a
//! terminal reports them with Caps Lock on, or with Shift held.
//! While a find-bar field has focus only the global keys are bound; the
//! rest are left unbound so the front end can hand them to the field.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use frext_core::editor::{Command, SearchBar, SearchOption, StepDirection};

use crate::buffer::cursor::{Motion, Selecting};
use crate::buffer::edit::EditAction;
use crate::find_bar::FindField;
use crate::terminal::KeyboardMode;

/// Which pane receives keyboard input outside of a prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    /// The editor body.
    Editor,
    /// The file-tree sidebar.
    Sidebar,
    /// A field of the find bar.
    Find(FindField),
}

impl Focus {
    /// The find-bar field with focus, or `None` when focus is elsewhere.
    #[must_use]
    pub const fn find_field(self) -> Option<FindField> {
        match self {
            Self::Find(field) => Some(field),
            Self::Editor | Self::Sidebar => None,
        }
    }
}

/// What a key press asks the terminal front end to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Send this command to the editor.
    Command(Command),
    /// Show the open-file path prompt.
    OpenFilePrompt,
    /// Quit immediately.
    Quit,
    /// Show or hide the sidebar.
    ToggleSidebar,
    /// Move focus to the sidebar from the editor, back to the editor from
    /// the sidebar, and to the editor from the find bar.
    ToggleFocus,
    /// Return focus to the editor.
    FocusEditor,
    /// Apply this edit action to the active buffer.
    Edit(EditAction),
}

/// The state a key binding depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyContext {
    /// How the terminal reports keys.
    pub keyboard: KeyboardMode,
    /// Whether the find bar is open.
    pub search_open: bool,
    /// Which pane has focus.
    pub focus: Focus,
}

/// Map `key` to an [`Action`] in `ctx`, or `None` when it is unbound.
///
/// `Release` events are ignored and `Repeat` is treated as `Press`.
#[must_use]
pub fn action_for(key: KeyEvent, ctx: KeyContext) -> Option<Action> {
    if key.kind == KeyEventKind::Release {
        return None;
    }
    if let Some(letter) = ctrl_letter(key) {
        return ctrl_letter_action(letter, key, ctx);
    }
    if ctx.search_open
        && let Some(option) = search_toggle(key)
    {
        return Some(Action::Command(Command::ToggleSearchOption(option)));
    }
    let mods = key.modifiers;
    match key.code {
        KeyCode::F(3) if ctx.search_open => {
            let direction = if mods.contains(KeyModifiers::SHIFT) {
                StepDirection::Backward
            } else {
                StepDirection::Forward
            };
            Some(Action::Command(Command::StepMatch(direction)))
        }
        KeyCode::F(15) if ctx.search_open => {
            Some(Action::Command(Command::StepMatch(StepDirection::Backward)))
        }
        KeyCode::Esc if ctx.search_open => Some(Action::Command(Command::CloseSearch)),
        KeyCode::Esc if ctx.focus == Focus::Sidebar => Some(Action::FocusEditor),
        _ if ctx.focus == Focus::Editor => editing_key(key.code, mods).map(Action::Edit),
        _ => None,
    }
}

/// The edit action for a non-Ctrl+letter key in the editor, if any.
fn editing_key(code: KeyCode, mods: KeyModifiers) -> Option<EditAction> {
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    let alt = mods.contains(KeyModifiers::ALT);
    let shift = mods.contains(KeyModifiers::SHIFT);
    let other = mods.intersects(KeyModifiers::SUPER | KeyModifiers::HYPER | KeyModifiers::META);
    let selecting = if shift {
        Selecting::Extend
    } else {
        Selecting::Move
    };
    let motion = |plain: Motion, with_ctrl: Motion| {
        let motion = if ctrl { with_ctrl } else { plain };
        (!alt).then_some(EditAction::Move(motion, selecting))
    };
    match code {
        KeyCode::Char(c) if !ctrl && !alt && !other => Some(EditAction::Type(c)),
        KeyCode::Enter if !ctrl && !alt => Some(EditAction::Newline),
        KeyCode::Tab if !ctrl && !alt && !shift => Some(EditAction::Tab),
        KeyCode::Tab if shift => Some(EditAction::Outdent),
        KeyCode::BackTab => Some(EditAction::Outdent),
        KeyCode::Backspace if ctrl || alt => Some(EditAction::WordBackspace),
        KeyCode::Backspace => Some(EditAction::Backspace),
        KeyCode::Delete if ctrl || alt => Some(EditAction::WordDelete),
        KeyCode::Delete => Some(EditAction::Delete),
        KeyCode::Left => motion(Motion::Left, Motion::WordLeft),
        KeyCode::Right => motion(Motion::Right, Motion::WordRight),
        KeyCode::Up => motion(Motion::Up, Motion::DocStart),
        KeyCode::Down => motion(Motion::Down, Motion::DocEnd),
        KeyCode::Home => motion(Motion::RowStart, Motion::DocStart),
        KeyCode::End => motion(Motion::RowEnd, Motion::DocEnd),
        KeyCode::PageUp => motion(Motion::PageUp, Motion::PageUp),
        KeyCode::PageDown => motion(Motion::PageDown, Motion::PageDown),
        _ => None,
    }
}

/// The search option an Alt letter toggles, if `key` is one.
///
/// The modifiers must be exactly Alt, or exactly Alt+Shift with an
/// ASCII upper-case letter: legacy encoding reports `ESC C` (Caps Lock
/// on) that way, and kitty reports Alt+Shift+C so. The letter is matched
/// case-insensitively.
fn search_toggle(key: KeyEvent) -> Option<SearchOption> {
    let KeyCode::Char(c) = key.code else {
        return None;
    };
    let alt_shift_upper =
        key.modifiers == KeyModifiers::ALT | KeyModifiers::SHIFT && c.is_ascii_uppercase();
    if key.modifiers != KeyModifiers::ALT && !alt_shift_upper {
        return None;
    }
    match c.to_ascii_lowercase() {
        'c' => Some(SearchOption::CaseSensitive),
        'w' => Some(SearchOption::WholeWord),
        'r' => Some(SearchOption::Regex),
        'l' => Some(SearchOption::InSelection),
        _ => None,
    }
}

/// Whether `mods` is Control, optionally with Shift, and nothing else.
fn is_ctrl(mods: KeyModifiers) -> bool {
    mods.contains(KeyModifiers::CONTROL)
        && (mods - KeyModifiers::CONTROL - KeyModifiers::SHIFT).is_empty()
}

/// The lower-case letter of a Ctrl+letter chord, if `key` is one.
fn ctrl_letter(key: KeyEvent) -> Option<char> {
    match key.code {
        KeyCode::Char(c) if c.is_ascii_alphabetic() && is_ctrl(key.modifiers) => {
            Some(c.to_ascii_lowercase())
        }
        _ => None,
    }
}

fn ctrl_letter_action(letter: char, key: KeyEvent, ctx: KeyContext) -> Option<Action> {
    if ctx.focus == Focus::Editor
        && let Some(action) = editing_ctrl_letter(letter, key, ctx.keyboard)
    {
        return Some(action);
    }
    let action = match letter {
        'n' => Action::Command(Command::NewTab),
        'o' => Action::OpenFilePrompt,
        's' => Action::Command(Command::Save),
        'w' => Action::Command(Command::RequestCloseActive),
        'f' => Action::Command(Command::OpenSearch(SearchBar::Find)),
        'r' => Action::Command(Command::OpenSearch(SearchBar::FindReplace)),
        // Legacy encoding reports byte 0x08 (Backspace on some
        // terminals) as Ctrl+H, so bind it only under the kitty protocol;
        // in the editor it is Backspace under legacy encoding.
        'h' if ctx.keyboard == KeyboardMode::Enhanced => {
            Action::Command(Command::OpenSearch(SearchBar::FindReplace))
        }
        'q' => Action::Quit,
        'b' => Action::ToggleSidebar,
        'e' => Action::ToggleFocus,
        _ => return None,
    };
    Some(action)
}

/// The editing action of a Ctrl+letter chord in the editor, if any.
/// Ctrl+Z is Redo when Shift is held, or the terminal reports the
/// letter upper-cased.
fn editing_ctrl_letter(letter: char, key: KeyEvent, keyboard: KeyboardMode) -> Option<Action> {
    let shifted = key.modifiers.contains(KeyModifiers::SHIFT) || key.code == KeyCode::Char('Z');
    let action = match letter {
        'a' => Action::Command(Command::SelectAll),
        'c' => Action::Command(Command::Copy),
        'x' => Action::Command(Command::Cut),
        'v' => Action::Command(Command::Paste),
        'z' if shifted => Action::Edit(EditAction::Redo),
        'z' => Action::Edit(EditAction::Undo),
        'y' => Action::Edit(EditAction::Redo),
        'h' if keyboard == KeyboardMode::Legacy => Action::Edit(EditAction::Backspace),
        _ => return None,
    };
    Some(action)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventState;

    fn ctx() -> KeyContext {
        KeyContext {
            keyboard: KeyboardMode::Enhanced,
            search_open: false,
            focus: Focus::Editor,
        }
    }

    fn searching() -> KeyContext {
        KeyContext {
            search_open: true,
            ..ctx()
        }
    }

    fn sidebar() -> KeyContext {
        KeyContext {
            focus: Focus::Sidebar,
            ..ctx()
        }
    }

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn ctrl(c: char) -> KeyEvent {
        key(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn cmd(command: Command) -> Option<Action> {
        Some(Action::Command(command))
    }

    fn edit(action: EditAction) -> Option<Action> {
        Some(Action::Edit(action))
    }

    fn legacy() -> KeyContext {
        KeyContext {
            keyboard: KeyboardMode::Legacy,
            ..ctx()
        }
    }

    fn plain(code: KeyCode) -> KeyEvent {
        key(code, KeyModifiers::NONE)
    }

    /// `key` with `mods` maps to `motion`, and with Shift added extends.
    fn assert_motion(code: KeyCode, mods: KeyModifiers, motion: Motion) {
        assert_eq!(
            action_for(key(code, mods), ctx()),
            edit(EditAction::Move(motion, Selecting::Move)),
            "{code:?} {mods:?}"
        );
        assert_eq!(
            action_for(key(code, mods | KeyModifiers::SHIFT), ctx()),
            edit(EditAction::Move(motion, Selecting::Extend)),
            "{code:?} {mods:?} + Shift"
        );
    }

    #[test]
    fn ctrl_n_opens_new_tab() {
        assert_eq!(action_for(ctrl('n'), ctx()), cmd(Command::NewTab));
    }

    #[test]
    fn ctrl_o_opens_file_prompt() {
        assert_eq!(action_for(ctrl('o'), ctx()), Some(Action::OpenFilePrompt));
    }

    #[test]
    fn ctrl_s_saves() {
        assert_eq!(action_for(ctrl('s'), ctx()), cmd(Command::Save));
    }

    #[test]
    fn ctrl_w_requests_close() {
        assert_eq!(
            action_for(ctrl('w'), ctx()),
            cmd(Command::RequestCloseActive)
        );
    }

    #[test]
    fn ctrl_f_opens_find() {
        assert_eq!(
            action_for(ctrl('f'), ctx()),
            cmd(Command::OpenSearch(SearchBar::Find))
        );
    }

    #[test]
    fn ctrl_r_opens_find_replace_in_both_modes() {
        let legacy = KeyContext {
            keyboard: KeyboardMode::Legacy,
            ..ctx()
        };
        for c in [ctx(), legacy] {
            assert_eq!(
                action_for(ctrl('r'), c),
                cmd(Command::OpenSearch(SearchBar::FindReplace))
            );
        }
    }

    #[test]
    fn ctrl_h_opens_find_replace_when_enhanced() {
        assert_eq!(
            action_for(ctrl('h'), ctx()),
            cmd(Command::OpenSearch(SearchBar::FindReplace))
        );
    }

    #[test]
    fn f3_steps_forward_when_search_open() {
        assert_eq!(
            action_for(key(KeyCode::F(3), KeyModifiers::NONE), searching()),
            cmd(Command::StepMatch(StepDirection::Forward))
        );
    }

    #[test]
    fn shift_f3_steps_backward_when_search_open() {
        assert_eq!(
            action_for(key(KeyCode::F(3), KeyModifiers::SHIFT), searching()),
            cmd(Command::StepMatch(StepDirection::Backward))
        );
    }

    #[test]
    fn f15_steps_backward_when_search_open() {
        assert_eq!(
            action_for(key(KeyCode::F(15), KeyModifiers::NONE), searching()),
            cmd(Command::StepMatch(StepDirection::Backward))
        );
    }

    #[test]
    fn f3_is_unbound_when_search_closed() {
        assert_eq!(
            action_for(key(KeyCode::F(3), KeyModifiers::NONE), ctx()),
            None
        );
        assert_eq!(
            action_for(key(KeyCode::F(3), KeyModifiers::SHIFT), ctx()),
            None
        );
        assert_eq!(
            action_for(key(KeyCode::F(15), KeyModifiers::NONE), ctx()),
            None
        );
    }

    #[test]
    fn esc_closes_search_when_open() {
        let esc = key(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(action_for(esc, searching()), cmd(Command::CloseSearch));
        let sidebar_searching = KeyContext {
            search_open: true,
            ..sidebar()
        };
        assert_eq!(
            action_for(esc, sidebar_searching),
            cmd(Command::CloseSearch)
        );
    }

    #[test]
    fn esc_from_sidebar_focuses_editor() {
        assert_eq!(
            action_for(key(KeyCode::Esc, KeyModifiers::NONE), sidebar()),
            Some(Action::FocusEditor)
        );
    }

    #[test]
    fn esc_is_unbound_in_editor_with_search_closed() {
        assert_eq!(
            action_for(key(KeyCode::Esc, KeyModifiers::NONE), ctx()),
            None
        );
    }

    #[test]
    fn ctrl_q_quits() {
        assert_eq!(action_for(ctrl('q'), ctx()), Some(Action::Quit));
    }

    #[test]
    fn ctrl_b_toggles_sidebar() {
        assert_eq!(action_for(ctrl('b'), ctx()), Some(Action::ToggleSidebar));
    }

    #[test]
    fn ctrl_e_toggles_focus() {
        assert_eq!(action_for(ctrl('e'), ctx()), Some(Action::ToggleFocus));
    }

    #[test]
    fn ctrl_letters_apply_regardless_of_focus() {
        assert_eq!(action_for(ctrl('q'), sidebar()), Some(Action::Quit));
        assert_eq!(
            action_for(ctrl('b'), sidebar()),
            Some(Action::ToggleSidebar)
        );
        assert_eq!(action_for(ctrl('e'), sidebar()), Some(Action::ToggleFocus));
        assert_eq!(action_for(ctrl('n'), sidebar()), cmd(Command::NewTab));
    }

    #[test]
    fn ctrl_letter_is_case_insensitive_and_accepts_shift() {
        let shifted = key(
            KeyCode::Char('N'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );
        assert_eq!(action_for(shifted, ctx()), cmd(Command::NewTab));
        assert_eq!(
            action_for(key(KeyCode::Char('N'), KeyModifiers::CONTROL), ctx()),
            cmd(Command::NewTab)
        );
    }

    #[test]
    fn ctrl_letter_with_alt_or_super_is_unbound() {
        for mods in [
            KeyModifiers::CONTROL | KeyModifiers::ALT,
            KeyModifiers::CONTROL | KeyModifiers::SUPER,
        ] {
            assert_eq!(action_for(key(KeyCode::Char('n'), mods), ctx()), None);
        }
    }

    #[test]
    fn release_is_ignored() {
        let release = KeyEvent::new_with_kind(
            KeyCode::Char('q'),
            KeyModifiers::CONTROL,
            KeyEventKind::Release,
        );
        assert_eq!(action_for(release, ctx()), None);
    }

    #[test]
    fn repeat_is_treated_as_press() {
        let repeat = KeyEvent::new_with_kind_and_state(
            KeyCode::Down,
            KeyModifiers::NONE,
            KeyEventKind::Repeat,
            KeyEventState::NONE,
        );
        assert_eq!(
            action_for(repeat, ctx()),
            edit(EditAction::Move(Motion::Down, Selecting::Move))
        );
    }

    #[test]
    fn ctrl_h_is_backspace_in_legacy() {
        assert_eq!(action_for(ctrl('h'), legacy()), edit(EditAction::Backspace));
        let legacy_sidebar = KeyContext {
            focus: Focus::Sidebar,
            ..legacy()
        };
        assert_eq!(action_for(ctrl('h'), legacy_sidebar), None);
    }

    #[test]
    fn ctrl_h_opens_find_replace_when_enhanced_in_any_focus() {
        assert_eq!(
            action_for(ctrl('h'), sidebar()),
            cmd(Command::OpenSearch(SearchBar::FindReplace))
        );
    }

    #[test]
    fn plain_and_shifted_chars_type() {
        assert_eq!(
            action_for(plain(KeyCode::Char('n')), ctx()),
            edit(EditAction::Type('n'))
        );
        assert_eq!(
            action_for(key(KeyCode::Char('N'), KeyModifiers::SHIFT), ctx()),
            edit(EditAction::Type('N'))
        );
        assert_eq!(
            action_for(plain(KeyCode::Char('é')), ctx()),
            edit(EditAction::Type('é'))
        );
    }

    #[test]
    fn alt_or_super_chars_do_not_type() {
        for mods in [
            KeyModifiers::ALT,
            KeyModifiers::SUPER,
            KeyModifiers::HYPER,
            KeyModifiers::META,
            KeyModifiers::CONTROL,
        ] {
            assert_eq!(action_for(key(KeyCode::Char('1'), mods), ctx()), None);
        }
        assert_eq!(
            action_for(key(KeyCode::Char('n'), KeyModifiers::ALT), ctx()),
            None
        );
    }

    #[test]
    fn enter_inserts_a_newline() {
        assert_eq!(
            action_for(plain(KeyCode::Enter), ctx()),
            edit(EditAction::Newline)
        );
        assert_eq!(
            action_for(key(KeyCode::Enter, KeyModifiers::SHIFT), ctx()),
            edit(EditAction::Newline)
        );
        assert_eq!(
            action_for(key(KeyCode::Enter, KeyModifiers::CONTROL), ctx()),
            None
        );
        assert_eq!(
            action_for(key(KeyCode::Enter, KeyModifiers::ALT), ctx()),
            None
        );
    }

    #[test]
    fn tab_indents_and_back_tab_outdents() {
        assert_eq!(
            action_for(plain(KeyCode::Tab), ctx()),
            edit(EditAction::Tab)
        );
        assert_eq!(
            action_for(key(KeyCode::Tab, KeyModifiers::SHIFT), ctx()),
            edit(EditAction::Outdent)
        );
        assert_eq!(
            action_for(key(KeyCode::BackTab, KeyModifiers::SHIFT), ctx()),
            edit(EditAction::Outdent)
        );
        assert_eq!(
            action_for(plain(KeyCode::BackTab), ctx()),
            edit(EditAction::Outdent)
        );
        assert_eq!(
            action_for(key(KeyCode::Tab, KeyModifiers::CONTROL), ctx()),
            None
        );
        assert_eq!(
            action_for(key(KeyCode::Tab, KeyModifiers::ALT), ctx()),
            None
        );
    }

    #[test]
    fn backspace_and_delete() {
        for c in [ctx(), legacy()] {
            assert_eq!(
                action_for(plain(KeyCode::Backspace), c),
                edit(EditAction::Backspace)
            );
            assert_eq!(
                action_for(plain(KeyCode::Delete), c),
                edit(EditAction::Delete)
            );
        }
        for mods in [KeyModifiers::CONTROL, KeyModifiers::ALT] {
            assert_eq!(
                action_for(key(KeyCode::Backspace, mods), ctx()),
                edit(EditAction::WordBackspace)
            );
            assert_eq!(
                action_for(key(KeyCode::Delete, mods), ctx()),
                edit(EditAction::WordDelete)
            );
        }
    }

    #[test]
    fn horizontal_motions() {
        assert_motion(KeyCode::Left, KeyModifiers::NONE, Motion::Left);
        assert_motion(KeyCode::Right, KeyModifiers::NONE, Motion::Right);
        assert_motion(KeyCode::Left, KeyModifiers::CONTROL, Motion::WordLeft);
        assert_motion(KeyCode::Right, KeyModifiers::CONTROL, Motion::WordRight);
    }

    #[test]
    fn vertical_motions() {
        assert_motion(KeyCode::Up, KeyModifiers::NONE, Motion::Up);
        assert_motion(KeyCode::Down, KeyModifiers::NONE, Motion::Down);
        assert_motion(KeyCode::Up, KeyModifiers::CONTROL, Motion::DocStart);
        assert_motion(KeyCode::Down, KeyModifiers::CONTROL, Motion::DocEnd);
        assert_motion(KeyCode::PageUp, KeyModifiers::NONE, Motion::PageUp);
        assert_motion(KeyCode::PageDown, KeyModifiers::NONE, Motion::PageDown);
    }

    #[test]
    fn home_and_end_motions() {
        assert_motion(KeyCode::Home, KeyModifiers::NONE, Motion::RowStart);
        assert_motion(KeyCode::End, KeyModifiers::NONE, Motion::RowEnd);
        assert_motion(KeyCode::Home, KeyModifiers::CONTROL, Motion::DocStart);
        assert_motion(KeyCode::End, KeyModifiers::CONTROL, Motion::DocEnd);
    }

    #[test]
    fn alt_motions_are_unbound() {
        for code in [
            KeyCode::Left,
            KeyCode::Right,
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::Home,
            KeyCode::End,
            KeyCode::PageUp,
            KeyCode::PageDown,
        ] {
            assert_eq!(action_for(key(code, KeyModifiers::ALT), ctx()), None);
            assert_eq!(
                action_for(key(code, KeyModifiers::ALT | KeyModifiers::SHIFT), ctx()),
                None
            );
        }
    }

    #[test]
    fn clipboard_ctrl_letters() {
        assert_eq!(action_for(ctrl('a'), ctx()), cmd(Command::SelectAll));
        assert_eq!(action_for(ctrl('c'), ctx()), cmd(Command::Copy));
        assert_eq!(action_for(ctrl('x'), ctx()), cmd(Command::Cut));
        assert_eq!(action_for(ctrl('v'), ctx()), cmd(Command::Paste));
    }

    #[test]
    fn undo_and_redo() {
        assert_eq!(action_for(ctrl('z'), ctx()), edit(EditAction::Undo));
        assert_eq!(action_for(ctrl('y'), ctx()), edit(EditAction::Redo));
        assert_eq!(
            action_for(
                key(
                    KeyCode::Char('z'),
                    KeyModifiers::CONTROL | KeyModifiers::SHIFT
                ),
                ctx()
            ),
            edit(EditAction::Redo)
        );
        assert_eq!(
            action_for(
                key(
                    KeyCode::Char('Z'),
                    KeyModifiers::CONTROL | KeyModifiers::SHIFT
                ),
                ctx()
            ),
            edit(EditAction::Redo)
        );
        assert_eq!(
            action_for(key(KeyCode::Char('Z'), KeyModifiers::CONTROL), ctx()),
            edit(EditAction::Redo)
        );
    }

    #[test]
    fn editing_keys_are_unbound_in_sidebar() {
        for c in ['a', 'c', 'x', 'v', 'z', 'y'] {
            assert_eq!(action_for(ctrl(c), sidebar()), None, "Ctrl+{c}");
        }
        for k in [
            plain(KeyCode::Char('n')),
            plain(KeyCode::Enter),
            plain(KeyCode::Tab),
            plain(KeyCode::BackTab),
            plain(KeyCode::Backspace),
            plain(KeyCode::Delete),
            plain(KeyCode::Left),
            plain(KeyCode::Right),
            plain(KeyCode::Up),
            plain(KeyCode::Down),
            plain(KeyCode::Home),
            plain(KeyCode::End),
            plain(KeyCode::PageUp),
            plain(KeyCode::PageDown),
            key(KeyCode::Home, KeyModifiers::CONTROL),
            key(KeyCode::Right, KeyModifiers::SHIFT),
        ] {
            assert_eq!(action_for(k, sidebar()), None, "{k:?}");
        }
    }

    #[test]
    fn non_editing_ctrl_letters_still_work_in_the_editor() {
        assert_eq!(
            action_for(ctrl('w'), ctx()),
            cmd(Command::RequestCloseActive)
        );
        assert_eq!(
            action_for(ctrl('h'), ctx()),
            cmd(Command::OpenSearch(SearchBar::FindReplace))
        );
    }

    fn finding() -> KeyContext {
        KeyContext {
            focus: Focus::Find(FindField::Pattern),
            ..searching()
        }
    }

    fn alt(c: char) -> KeyEvent {
        key(KeyCode::Char(c), KeyModifiers::ALT)
    }

    fn toggle(option: SearchOption) -> Option<Action> {
        cmd(Command::ToggleSearchOption(option))
    }

    const TOGGLES: [(char, SearchOption); 4] = [
        ('c', SearchOption::CaseSensitive),
        ('w', SearchOption::WholeWord),
        ('r', SearchOption::Regex),
        ('l', SearchOption::InSelection),
    ];

    #[test]
    fn alt_letters_toggle_search_options_when_search_open() {
        for (c, option) in TOGGLES {
            assert_eq!(action_for(alt(c), searching()), toggle(option), "Alt+{c}");
        }
    }

    #[test]
    fn alt_toggles_are_unbound_when_search_closed() {
        for (c, _) in TOGGLES {
            assert_eq!(action_for(alt(c), ctx()), None, "Alt+{c}");
        }
    }

    #[test]
    fn alt_toggle_matches_an_upper_case_letter() {
        assert_eq!(
            action_for(alt('C'), searching()),
            toggle(SearchOption::CaseSensitive)
        );
    }

    #[test]
    fn alt_shift_toggles_only_an_upper_case_letter() {
        let alt_shift = KeyModifiers::ALT | KeyModifiers::SHIFT;
        assert_eq!(
            action_for(key(KeyCode::Char('C'), alt_shift), searching()),
            toggle(SearchOption::CaseSensitive)
        );
        assert_eq!(
            action_for(key(KeyCode::Char('c'), alt_shift), searching()),
            None
        );
        assert_eq!(
            action_for(
                key(
                    KeyCode::Char('c'),
                    KeyModifiers::CONTROL | KeyModifiers::ALT
                ),
                searching()
            ),
            None
        );
    }

    #[test]
    fn an_unbound_alt_letter_falls_through() {
        assert_eq!(action_for(alt('x'), searching()), None);
    }

    #[test]
    fn find_field_projects_the_find_focus() {
        assert_eq!(Focus::Editor.find_field(), None);
        assert_eq!(Focus::Sidebar.find_field(), None);
        for field in [FindField::Pattern, FindField::Replacement] {
            assert_eq!(Focus::Find(field).find_field(), Some(field));
        }
    }

    #[test]
    fn alt_toggles_work_in_any_focus() {
        let sidebar_searching = KeyContext {
            search_open: true,
            ..sidebar()
        };
        let replacing = KeyContext {
            focus: Focus::Find(FindField::Replacement),
            ..searching()
        };
        for ctx in [sidebar_searching, finding(), replacing] {
            for (letter, option) in TOGGLES {
                assert_eq!(action_for(alt(letter), ctx), toggle(option), "Alt+{letter}");
            }
        }
    }

    #[test]
    fn editing_keys_are_unbound_in_find_focus() {
        for k in [
            plain(KeyCode::Char('a')),
            plain(KeyCode::Enter),
            plain(KeyCode::Tab),
            plain(KeyCode::Backspace),
            plain(KeyCode::Left),
            ctrl('a'),
            ctrl('v'),
            ctrl('z'),
        ] {
            assert_eq!(action_for(k, finding()), None, "{k:?}");
        }
    }

    #[test]
    fn global_keys_work_in_find_focus() {
        assert_eq!(
            action_for(ctrl('f'), finding()),
            cmd(Command::OpenSearch(SearchBar::Find))
        );
        assert_eq!(action_for(ctrl('s'), finding()), cmd(Command::Save));
        assert_eq!(
            action_for(plain(KeyCode::F(3)), finding()),
            cmd(Command::StepMatch(StepDirection::Forward))
        );
        assert_eq!(
            action_for(key(KeyCode::F(3), KeyModifiers::SHIFT), finding()),
            cmd(Command::StepMatch(StepDirection::Backward))
        );
        assert_eq!(
            action_for(plain(KeyCode::Esc), finding()),
            cmd(Command::CloseSearch)
        );
    }

    #[test]
    fn legacy_ctrl_h_is_unbound_in_find_focus() {
        let legacy_finding = KeyContext {
            keyboard: KeyboardMode::Legacy,
            ..finding()
        };
        assert_eq!(action_for(ctrl('h'), legacy_finding), None);
    }
}
