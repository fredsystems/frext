//! A single-line text input over an external string.
//!
//! The text itself is not owned here, because the text of a name prompt
//! lives in core. [`TextInput`] only holds the view state: the cursor and
//! the horizontal scroll, both as char indices.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use unicode_width::UnicodeWidthChar;

use crate::theme;

/// What a key did to a [`TextInput`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputOutcome {
    /// The text changed.
    Edited,
    /// Only the cursor moved (or nothing changed).
    Moved,
    /// Enter was pressed.
    Submit,
    /// Esc was pressed.
    Cancel,
    /// The key is not handled by the input; the caller may bind it.
    Ignored,
}

/// The cursor and horizontal scroll of a single-line input.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TextInput {
    cursor: usize,
    scroll: usize,
}

/// The display width of a char, zero for non-printing chars.
fn char_width(c: char) -> usize {
    UnicodeWidthChar::width(c).unwrap_or(0)
}

/// The byte offset of char index `index` in `text`, or `text.len()` past
/// the end.
fn byte_offset(text: &str, index: usize) -> usize {
    text.char_indices()
        .nth(index)
        .map_or(text.len(), |(byte, _)| byte)
}

/// The key code `key` is handled as.
///
/// Ctrl+H (without Alt) becomes [`KeyCode::Backspace`]: legacy terminals
/// send 0x08 for the Backspace key, which crossterm reports as Ctrl+H.
fn normalise(key: KeyEvent) -> KeyCode {
    match key.code {
        KeyCode::Char('h' | 'H')
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT) =>
        {
            KeyCode::Backspace
        }
        code => code,
    }
}

impl TextInput {
    /// An input with the cursor at the start.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The cursor, as a char index.
    #[must_use]
    pub const fn cursor(&self) -> usize {
        self.cursor
    }

    /// The first visible char, as a char index.
    #[must_use]
    pub const fn scroll(&self) -> usize {
        self.scroll
    }

    /// Clamps the cursor and scroll to `text`, which may have changed
    /// since the last call.
    fn clamp(&mut self, text: &str) -> usize {
        let len = text.chars().count();
        self.cursor = self.cursor.min(len);
        self.scroll = self.scroll.min(self.cursor);
        len
    }

    /// Handles one key event against `text`.
    pub fn handle_key(&mut self, text: &mut String, key: KeyEvent) -> InputOutcome {
        let len = self.clamp(text);
        if key.kind == KeyEventKind::Release {
            return InputOutcome::Ignored;
        }
        match normalise(key) {
            KeyCode::Char(c) => {
                if key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    || c.is_control()
                {
                    return InputOutcome::Ignored;
                }
                text.insert(byte_offset(text, self.cursor), c);
                self.cursor += 1;
                InputOutcome::Edited
            }
            KeyCode::Backspace => {
                if self.cursor == 0 {
                    return InputOutcome::Moved;
                }
                self.cursor -= 1;
                text.remove(byte_offset(text, self.cursor));
                InputOutcome::Edited
            }
            KeyCode::Delete => {
                if self.cursor >= len {
                    return InputOutcome::Moved;
                }
                text.remove(byte_offset(text, self.cursor));
                InputOutcome::Edited
            }
            KeyCode::Left => {
                self.cursor = self.cursor.saturating_sub(1);
                InputOutcome::Moved
            }
            KeyCode::Right => {
                self.cursor = (self.cursor + 1).min(len);
                InputOutcome::Moved
            }
            KeyCode::Home => {
                self.cursor = 0;
                InputOutcome::Moved
            }
            KeyCode::End => {
                self.cursor = len;
                InputOutcome::Moved
            }
            KeyCode::Enter => InputOutcome::Submit,
            KeyCode::Esc => InputOutcome::Cancel,
            _ => InputOutcome::Ignored,
        }
    }

    /// Inserts `s` at the cursor (for paste), with each line break
    /// replaced by a space.
    pub fn insert_str(&mut self, text: &mut String, s: &str) {
        self.clamp(text);
        let cleaned: String = s
            .replace("\r\n", " ")
            .chars()
            .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
            .collect();
        text.insert_str(byte_offset(text, self.cursor), &cleaned);
        self.cursor += cleaned.chars().count();
    }

    /// Moves the cursor past the last char of `text`.
    pub fn move_to_end(&mut self, text: &str) {
        self.cursor = self.clamp(text);
    }

    /// Places the cursor for a click at `column`, relative to the field's
    /// left edge as last rendered: before the char whose cells contain
    /// the column, or at the end when the click is past the text.
    pub fn click(&mut self, text: &str, column: u16) {
        let len = self.clamp(text);
        let column = usize::from(column);
        let mut acc = 0;
        for (i, c) in text.chars().enumerate().skip(self.scroll) {
            acc += char_width(c);
            if acc > column {
                self.cursor = i;
                return;
            }
        }
        self.cursor = len;
    }

    /// Draws `text` into the first row of `area` with [`theme::input`],
    /// scrolling horizontally so the cursor stays inside the row, and
    /// returns the cursor's cell (`None` if `area` is empty).
    pub fn render(&mut self, text: &str, area: Rect, buf: &mut Buffer) -> Option<Position> {
        self.clamp(text);
        let area = area.intersection(buf.area);
        if area.is_empty() {
            return None;
        }
        let row = Rect::new(area.x, area.y, area.width, 1);
        buf.set_style(row, theme::input());

        let width = usize::from(area.width);
        let widths: Vec<usize> = text.chars().map(char_width).collect();
        // Reserve the last column for a cursor that sits past the text.
        while self.scroll < self.cursor
            && widths[self.scroll..self.cursor].iter().sum::<usize>() >= width
        {
            self.scroll += 1;
        }
        let cursor_col: usize = widths[self.scroll..self.cursor].iter().sum();

        let mut visible = String::new();
        let mut used = 0;
        for (c, w) in text.chars().zip(&widths).skip(self.scroll) {
            if used + w > width {
                break;
            }
            used += w;
            visible.push(c);
        }
        buf.set_stringn(area.x, area.y, &visible, width, theme::input());

        let col = u16::try_from(cursor_col).ok()?;
        Some(Position::new(area.x + col, area.y))
    }
}

#[cfg(test)]
mod tests {
    // `unwrap` is acceptable in test code: a panic on an unexpected `Err`
    // is exactly the failure signal we want from a test.
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn type_str(input: &mut TextInput, text: &mut String, s: &str) {
        for c in s.chars() {
            assert_eq!(
                input.handle_key(text, press(KeyCode::Char(c))),
                InputOutcome::Edited
            );
        }
    }

    fn row_text(buf: &Buffer) -> String {
        buf.content().iter().map(|cell| cell.symbol()).collect()
    }

    #[test]
    fn insert_at_start_middle_and_end_multibyte() {
        let mut input = TextInput::new();
        let mut text = String::new();
        type_str(&mut input, &mut text, "é日");
        assert_eq!(text, "é日");
        assert_eq!(input.cursor(), 2);

        input.handle_key(&mut text, press(KeyCode::Home));
        type_str(&mut input, &mut text, "a");
        assert_eq!(text, "aé日");

        input.handle_key(&mut text, press(KeyCode::Right));
        type_str(&mut input, &mut text, "本");
        assert_eq!(text, "aé本日");
        assert_eq!(input.cursor(), 3);

        input.handle_key(&mut text, press(KeyCode::End));
        type_str(&mut input, &mut text, "z");
        assert_eq!(text, "aé本日z");
        assert_eq!(input.cursor(), 5);
    }

    #[test]
    fn shift_char_inserts_but_ctrl_alt_and_release_are_ignored() {
        let mut input = TextInput::new();
        let mut text = String::new();
        let shifted = KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT);
        assert_eq!(input.handle_key(&mut text, shifted), InputOutcome::Edited);
        let ctrl = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
        assert_eq!(input.handle_key(&mut text, ctrl), InputOutcome::Ignored);
        let alt = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT);
        assert_eq!(input.handle_key(&mut text, alt), InputOutcome::Ignored);
        let mut release = press(KeyCode::Char('r'));
        release.kind = KeyEventKind::Release;
        assert_eq!(input.handle_key(&mut text, release), InputOutcome::Ignored);
        assert_eq!(
            input.handle_key(&mut text, press(KeyCode::Char('\u{7}'))),
            InputOutcome::Ignored
        );
        assert_eq!(text, "A");
    }

    #[test]
    fn backspace_and_delete_at_start_middle_and_end() {
        let mut input = TextInput::new();
        let mut text = String::from("é日本");

        // Start.
        assert_eq!(
            input.handle_key(&mut text, press(KeyCode::Backspace)),
            InputOutcome::Moved
        );
        assert_eq!(
            input.handle_key(&mut text, press(KeyCode::Delete)),
            InputOutcome::Edited
        );
        assert_eq!(text, "日本");

        // Middle.
        input.handle_key(&mut text, press(KeyCode::Right));
        assert_eq!(
            input.handle_key(&mut text, press(KeyCode::Backspace)),
            InputOutcome::Edited
        );
        assert_eq!(text, "本");
        assert_eq!(input.cursor(), 0);
        text = String::from("é日本");
        input.handle_key(&mut text, press(KeyCode::Right));
        assert_eq!(
            input.handle_key(&mut text, press(KeyCode::Delete)),
            InputOutcome::Edited
        );
        assert_eq!(text, "é本");

        // End.
        input.handle_key(&mut text, press(KeyCode::End));
        assert_eq!(
            input.handle_key(&mut text, press(KeyCode::Delete)),
            InputOutcome::Moved
        );
        assert_eq!(
            input.handle_key(&mut text, press(KeyCode::Backspace)),
            InputOutcome::Edited
        );
        assert_eq!(text, "é");
        assert_eq!(input.cursor(), 1);
    }

    #[test]
    fn ctrl_h_deletes_backward_like_backspace() {
        let mut input = TextInput::new();
        let mut text = String::from("abc");
        input.handle_key(&mut text, press(KeyCode::Right));
        input.handle_key(&mut text, press(KeyCode::Right));
        let ctrl_h = KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL);
        assert_eq!(input.handle_key(&mut text, ctrl_h), InputOutcome::Edited);
        assert_eq!(text, "ac");
        assert_eq!(input.cursor(), 1);

        let ctrl_shift_h = KeyEvent::new(
            KeyCode::Char('H'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );
        assert_eq!(
            input.handle_key(&mut text, ctrl_shift_h),
            InputOutcome::Edited
        );
        assert_eq!(text, "c");
        assert_eq!(input.cursor(), 0);

        // Start.
        assert_eq!(input.handle_key(&mut text, ctrl_h), InputOutcome::Moved);
        assert_eq!(text, "c");
        assert_eq!(input.cursor(), 0);
    }

    #[test]
    fn ctrl_alt_h_is_ignored() {
        let mut input = TextInput::new();
        let mut text = String::from("abc");
        input.move_to_end(&text);
        let ctrl_alt_h = KeyEvent::new(
            KeyCode::Char('h'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        );
        assert_eq!(
            input.handle_key(&mut text, ctrl_alt_h),
            InputOutcome::Ignored
        );
        assert_eq!(text, "abc");
        assert_eq!(input.cursor(), 3);
    }

    #[test]
    fn home_end_left_right() {
        let mut input = TextInput::new();
        let mut text = String::from("abc");
        assert_eq!(
            input.handle_key(&mut text, press(KeyCode::End)),
            InputOutcome::Moved
        );
        assert_eq!(input.cursor(), 3);
        input.handle_key(&mut text, press(KeyCode::Right));
        assert_eq!(input.cursor(), 3);
        input.handle_key(&mut text, press(KeyCode::Left));
        assert_eq!(input.cursor(), 2);
        let shift_home = KeyEvent::new(KeyCode::Home, KeyModifiers::SHIFT);
        assert_eq!(input.handle_key(&mut text, shift_home), InputOutcome::Moved);
        assert_eq!(input.cursor(), 0);
        input.handle_key(&mut text, press(KeyCode::Left));
        assert_eq!(input.cursor(), 0);
    }

    #[test]
    fn enter_esc_tab_and_backtab() {
        let mut input = TextInput::new();
        let mut text = String::from("x");
        assert_eq!(
            input.handle_key(&mut text, press(KeyCode::Enter)),
            InputOutcome::Submit
        );
        assert_eq!(
            input.handle_key(&mut text, press(KeyCode::Esc)),
            InputOutcome::Cancel
        );
        assert_eq!(
            input.handle_key(&mut text, press(KeyCode::Tab)),
            InputOutcome::Ignored
        );
        assert_eq!(
            input.handle_key(&mut text, press(KeyCode::BackTab)),
            InputOutcome::Ignored
        );
        assert_eq!(text, "x");
    }

    #[test]
    fn clamps_after_external_shrink() {
        let mut input = TextInput::new();
        let mut text = String::from("hello");
        input.move_to_end(&text);
        assert_eq!(input.cursor(), 5);

        text = String::from("hé");
        type_str(&mut input, &mut text, "!");
        assert_eq!(text, "hé!");
        assert_eq!(input.cursor(), 3);

        text.clear();
        assert_eq!(
            input.handle_key(&mut text, press(KeyCode::Backspace)),
            InputOutcome::Moved
        );
        assert_eq!(input.cursor(), 0);
    }

    #[test]
    fn insert_str_replaces_newlines() {
        let mut input = TextInput::new();
        let mut text = String::from("ab");
        input.handle_key(&mut text, press(KeyCode::Right));
        input.insert_str(&mut text, "x\ny\r\nz\r日");
        assert_eq!(text, "ax y z 日b");
        assert_eq!(input.cursor(), 8);
    }

    #[test]
    fn click_over_wide_chars() {
        let mut input = TextInput::new();
        let text = "a日本b";
        // Cells: a=0, 日=1..2, 本=3..4, b=5.
        input.click(text, 0);
        assert_eq!(input.cursor(), 0);
        input.click(text, 1);
        assert_eq!(input.cursor(), 1);
        input.click(text, 2);
        assert_eq!(input.cursor(), 1);
        input.click(text, 3);
        assert_eq!(input.cursor(), 2);
        input.click(text, 5);
        assert_eq!(input.cursor(), 3);
        input.click(text, 6);
        assert_eq!(input.cursor(), 4);
        input.click(text, 40);
        assert_eq!(input.cursor(), 4);
    }

    #[test]
    fn click_is_relative_to_scroll() {
        let mut input = TextInput::new();
        let text = "0123456789abcdef";
        input.move_to_end(text);
        let area = Rect::new(0, 0, 10, 1);
        let mut buf = Buffer::empty(area);
        input.render(text, area, &mut buf);
        assert_eq!(input.scroll(), 7);
        input.click(text, 0);
        assert_eq!(input.cursor(), 7);
    }

    #[test]
    fn render_short_text_from_start() {
        let mut input = TextInput::new();
        let area = Rect::new(2, 1, 10, 1);
        let mut buf = Buffer::empty(Rect::new(0, 0, 14, 3));
        let pos = input.render("hi", area, &mut buf);
        assert_eq!(pos, Some(Position::new(2, 1)));
        assert_eq!(buf[(2, 1)].symbol(), "h");
        assert_eq!(buf[(3, 1)].symbol(), "i");
        assert_eq!(buf[(11, 1)].style().bg, theme::input().bg);
    }

    #[test]
    fn render_scrolls_to_show_tail() {
        let mut input = TextInput::new();
        let text = "0123456789abcdef";
        input.move_to_end(text);
        let area = Rect::new(0, 0, 10, 1);
        let mut buf = Buffer::empty(area);
        let pos = input.render(text, area, &mut buf).unwrap();
        assert!(area.contains(pos));
        assert_eq!(pos, Position::new(9, 0));
        assert_eq!(row_text(&buf), "789abcdef ");

        let mut expected = Buffer::with_lines(["789abcdef "]);
        expected.set_style(area, theme::input());
        assert_eq!(buf, expected);

        // Moving home scrolls back.
        let mut owned = text.to_owned();
        input.handle_key(&mut owned, press(KeyCode::Home));
        let mut buf = Buffer::empty(area);
        let pos = input.render(text, area, &mut buf);
        assert_eq!(pos, Some(Position::new(0, 0)));
        assert_eq!(row_text(&buf), "0123456789");
    }

    #[test]
    fn render_omits_wide_char_at_right_edge() {
        let mut input = TextInput::new();
        let text = "abcd日";
        let area = Rect::new(0, 0, 5, 1);
        let mut buf = Buffer::empty(area);
        let pos = input.render(text, area, &mut buf);
        assert_eq!(pos, Some(Position::new(0, 0)));
        assert_eq!(row_text(&buf), "abcd ");
    }

    #[test]
    fn render_wide_chars_with_cursor_at_end() {
        let mut input = TextInput::new();
        let text = "日本語";
        input.move_to_end(text);
        let area = Rect::new(0, 0, 5, 1);
        let mut buf = Buffer::empty(area);
        let pos = input.render(text, area, &mut buf).unwrap();
        assert!(area.contains(pos));
        assert_eq!(pos, Position::new(4, 0));
        assert_eq!(buf[(0, 0)].symbol(), "本");
        assert_eq!(buf[(2, 0)].symbol(), "語");
    }

    #[test]
    fn render_empty_area_returns_none() {
        let mut input = TextInput::new();
        let mut buf = Buffer::empty(Rect::new(0, 0, 5, 1));
        assert_eq!(input.render("x", Rect::new(0, 0, 0, 1), &mut buf), None);
        assert_eq!(input.render("x", Rect::new(0, 0, 5, 0), &mut buf), None);
    }
}
