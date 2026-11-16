//! Keybindings: a pure mapping from a terminal [`KeyEvent`] to an
//! [`Action`].
//!
//! Editing keys are not bound here; this module covers tab, search,
//! focus and scrolling shortcuts only.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use frext_core::editor::{Command, SearchBar, StepDirection};

use crate::terminal::KeyboardMode;

/// Which pane receives keyboard input outside of a prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    /// The editor body.
    Editor,
    /// The file-tree sidebar.
    Sidebar,
}

/// How far to scroll the editor view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollBy {
    /// By a signed number of lines (negative is up).
    Lines(i32),
    /// By a signed number of pages (negative is up).
    Pages(i32),
    /// To the first line.
    Top,
    /// To the last line.
    Bottom,
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
    /// Move focus between the editor and the sidebar.
    ToggleFocus,
    /// Return focus to the editor.
    FocusEditor,
    /// Scroll the editor view.
    Scroll(ScrollBy),
}

/// The state a key binding depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyContext {
    /// How the terminal reports keys.
    pub keyboard: KeyboardMode,
    /// Whether the search bar is open.
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
        return ctrl_letter_action(letter, ctx);
    }
    let mods = key.modifiers;
    let plain = !mods.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
    let editor = ctx.focus == Focus::Editor;
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
        KeyCode::Up if editor && plain => Some(Action::Scroll(ScrollBy::Lines(-1))),
        KeyCode::Down if editor && plain => Some(Action::Scroll(ScrollBy::Lines(1))),
        KeyCode::PageUp if editor && plain => Some(Action::Scroll(ScrollBy::Pages(-1))),
        KeyCode::PageDown if editor && plain => Some(Action::Scroll(ScrollBy::Pages(1))),
        KeyCode::Home if editor && is_ctrl(mods) => Some(Action::Scroll(ScrollBy::Top)),
        KeyCode::End if editor && is_ctrl(mods) => Some(Action::Scroll(ScrollBy::Bottom)),
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

fn ctrl_letter_action(letter: char, ctx: KeyContext) -> Option<Action> {
    let action = match letter {
        'n' => Action::Command(Command::NewTab),
        'o' => Action::OpenFilePrompt,
        's' => Action::Command(Command::Save),
        'w' => Action::Command(Command::RequestCloseActive),
        'f' => Action::Command(Command::OpenSearch(SearchBar::Find)),
        'r' => Action::Command(Command::OpenSearch(SearchBar::FindReplace)),
        // Legacy encoding reports byte 0x08 (Backspace on some
        // terminals) as Ctrl+H, so bind it only under the kitty protocol.
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
    fn ctrl_h_is_unbound_in_legacy() {
        let legacy = KeyContext {
            keyboard: KeyboardMode::Legacy,
            ..ctx()
        };
        assert_eq!(action_for(ctrl('h'), legacy), None);
    }

    #[test]
    fn backspace_is_unbound() {
        let legacy = KeyContext {
            keyboard: KeyboardMode::Legacy,
            ..ctx()
        };
        assert_eq!(
            action_for(key(KeyCode::Backspace, KeyModifiers::CONTROL), legacy),
            None
        );
        assert_eq!(
            action_for(key(KeyCode::Backspace, KeyModifiers::NONE), legacy),
            None
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
    fn up_down_scroll_lines_in_editor() {
        assert_eq!(
            action_for(key(KeyCode::Up, KeyModifiers::NONE), ctx()),
            Some(Action::Scroll(ScrollBy::Lines(-1)))
        );
        assert_eq!(
            action_for(key(KeyCode::Down, KeyModifiers::NONE), ctx()),
            Some(Action::Scroll(ScrollBy::Lines(1)))
        );
    }

    #[test]
    fn page_up_down_scroll_pages_in_editor() {
        assert_eq!(
            action_for(key(KeyCode::PageUp, KeyModifiers::NONE), ctx()),
            Some(Action::Scroll(ScrollBy::Pages(-1)))
        );
        assert_eq!(
            action_for(key(KeyCode::PageDown, KeyModifiers::NONE), ctx()),
            Some(Action::Scroll(ScrollBy::Pages(1)))
        );
    }

    #[test]
    fn ctrl_home_end_scroll_to_top_bottom() {
        assert_eq!(
            action_for(key(KeyCode::Home, KeyModifiers::CONTROL), ctx()),
            Some(Action::Scroll(ScrollBy::Top))
        );
        assert_eq!(
            action_for(key(KeyCode::End, KeyModifiers::CONTROL), ctx()),
            Some(Action::Scroll(ScrollBy::Bottom))
        );
    }

    #[test]
    fn scroll_keys_are_unbound_in_sidebar() {
        for k in [
            key(KeyCode::Up, KeyModifiers::NONE),
            key(KeyCode::Down, KeyModifiers::NONE),
            key(KeyCode::PageUp, KeyModifiers::NONE),
            key(KeyCode::PageDown, KeyModifiers::NONE),
            key(KeyCode::Home, KeyModifiers::CONTROL),
            key(KeyCode::End, KeyModifiers::CONTROL),
        ] {
            assert_eq!(action_for(k, sidebar()), None, "{k:?}");
        }
    }

    #[test]
    fn modified_arrows_and_plain_home_are_unbound() {
        for k in [
            key(KeyCode::Up, KeyModifiers::CONTROL),
            key(KeyCode::Down, KeyModifiers::ALT),
            key(KeyCode::PageUp, KeyModifiers::CONTROL),
            key(KeyCode::Home, KeyModifiers::NONE),
            key(KeyCode::End, KeyModifiers::NONE),
        ] {
            assert_eq!(action_for(k, ctx()), None, "{k:?}");
        }
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
    fn plain_letter_is_unbound() {
        assert_eq!(
            action_for(key(KeyCode::Char('n'), KeyModifiers::NONE), ctx()),
            None
        );
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
            Some(Action::Scroll(ScrollBy::Lines(1)))
        );
    }
}
