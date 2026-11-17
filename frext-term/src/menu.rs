//! A right-click context menu.
//!
//! A [`ContextMenu`] is a bordered list of [`MenuEntry`]s opened at a
//! screen cell. While it is open it is modal: keys and clicks go to it
//! first, and each returns a [`MenuReply`] telling the app whether an
//! item was chosen or the menu was dismissed.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::widgets::{Block, Clear, Widget};
use unicode_width::UnicodeWidthStr;

use crate::hit::{HitMap, Target};
use crate::theme;

/// Whether a menu item can be chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Enablement {
    /// The item can be highlighted and chosen.
    Enabled,
    /// The item is drawn greyed out and cannot be chosen.
    Disabled,
}

/// One row of a context menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuEntry<T> {
    /// A choosable item.
    Item {
        /// The text shown for the item.
        label: String,
        /// Whether the item can be chosen.
        enabled: Enablement,
        /// The value returned when the item is chosen.
        value: T,
    },
    /// A horizontal rule between groups of items.
    Separator,
}

impl<T> MenuEntry<T> {
    /// Whether this entry is an enabled item.
    const fn is_enabled_item(&self) -> bool {
        matches!(
            self,
            Self::Item {
                enabled: Enablement::Enabled,
                ..
            }
        )
    }
}

/// What a menu did with an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuReply<T> {
    /// An item was chosen; the menu should close.
    Chosen(T),
    /// The menu was dismissed without a choice.
    Dismissed,
    /// The event was consumed and the menu stays open.
    Handled,
}

/// A context menu anchored at a screen cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextMenu<T> {
    entries: Vec<MenuEntry<T>>,
    anchor: Position,
    highlighted: Option<usize>,
}

impl<T: Clone> ContextMenu<T> {
    /// A menu of `entries` opened at `anchor`, with nothing highlighted.
    #[must_use]
    pub const fn new(entries: Vec<MenuEntry<T>>, anchor: Position) -> Self {
        Self {
            entries,
            anchor,
            highlighted: None,
        }
    }

    /// The entry index of the highlighted item, if any.
    #[must_use]
    pub const fn highlighted(&self) -> Option<usize> {
        self.highlighted
    }

    /// Whether entry `index` is an enabled item.
    fn is_enabled(&self, index: usize) -> bool {
        self.entries
            .get(index)
            .is_some_and(MenuEntry::is_enabled_item)
    }

    /// The value of the enabled item at `index`, if there is one.
    fn enabled_value(&self, index: usize) -> Option<T> {
        match self.entries.get(index) {
            Some(MenuEntry::Item {
                enabled: Enablement::Enabled,
                value,
                ..
            }) => Some(value.clone()),
            _ => None,
        }
    }

    /// Highlights the next enabled item, wrapping, or the first one when
    /// nothing is highlighted.
    fn highlight_next(&mut self) {
        let len = self.entries.len();
        let start = self.highlighted.map_or(0, |index| index + 1);
        self.highlighted = (0..len)
            .map(|step| (start + step) % len)
            .find(|&index| self.is_enabled(index));
    }

    /// Highlights the previous enabled item, wrapping, or the last one
    /// when nothing is highlighted.
    fn highlight_previous(&mut self) {
        let len = self.entries.len();
        let start = self.highlighted.unwrap_or(len);
        self.highlighted = (1..=len)
            .map(|step| (start + len - step) % len)
            .find(|&index| self.is_enabled(index));
    }

    /// Handles a key press. The menu is modal, so every key is consumed.
    pub fn handle_key(&mut self, key: KeyEvent) -> MenuReply<T> {
        if key.kind == KeyEventKind::Release {
            return MenuReply::Handled;
        }
        match key.code {
            KeyCode::Down => {
                self.highlight_next();
                MenuReply::Handled
            }
            KeyCode::Up => {
                self.highlight_previous();
                MenuReply::Handled
            }
            KeyCode::Enter => self
                .highlighted
                .and_then(|index| self.enabled_value(index))
                .map_or(MenuReply::Handled, MenuReply::Chosen),
            KeyCode::Esc => MenuReply::Dismissed,
            _ => MenuReply::Handled,
        }
    }

    /// Highlights the entry under the mouse when it is an enabled item;
    /// `None` clears the highlight.
    pub fn hover(&mut self, index: Option<usize>) {
        match index {
            Some(index) if self.is_enabled(index) => self.highlighted = Some(index),
            Some(_) => {}
            None => self.highlighted = None,
        }
    }

    /// Handles a click on `target`: an enabled item is chosen, the rest
    /// of the menu consumes the click, and anything outside dismisses it.
    pub fn click(&mut self, target: Option<Target>) -> MenuReply<T> {
        match target {
            Some(Target::MenuItem(index)) => self
                .enabled_value(index)
                .map_or(MenuReply::Handled, MenuReply::Chosen),
            Some(Target::MenuPopup) => MenuReply::Handled,
            _ => MenuReply::Dismissed,
        }
    }

    /// Where the menu is drawn inside `area`: at the anchor, moved left
    /// or opened upwards to stay on screen, then clamped into `area`.
    #[must_use]
    pub fn rect(&self, area: Rect) -> Rect {
        let longest = self
            .entries
            .iter()
            .map(|entry| match entry {
                MenuEntry::Item { label, .. } => label.width(),
                MenuEntry::Separator => 0,
            })
            .max()
            .unwrap_or(0);
        let width = u16::try_from(longest).unwrap_or(u16::MAX).saturating_add(4);
        let height = u16::try_from(self.entries.len())
            .unwrap_or(u16::MAX)
            .saturating_add(2);

        let mut x = self.anchor.x;
        let mut y = self.anchor.y;
        if x.saturating_add(width) > area.right() {
            x = area.right().saturating_sub(width);
        }
        if y.saturating_add(height) > area.bottom() {
            y = self.anchor.y.saturating_add(1).saturating_sub(height);
        }

        let width = width.min(area.width);
        let height = height.min(area.height);
        if x < area.x {
            x = area.x;
        }
        if x.saturating_add(width) > area.right() {
            x = area.right().saturating_sub(width);
        }
        if y < area.y {
            y = area.y;
        }
        if y.saturating_add(height) > area.bottom() {
            y = area.bottom().saturating_sub(height);
        }
        Rect::new(x, y, width, height)
    }

    /// Draws the menu over `area` and records its hit regions: the whole
    /// menu as [`Target::MenuPopup`], then each item row as
    /// [`Target::MenuItem`] with its entry index.
    pub fn render(&self, area: Rect, buf: &mut Buffer, hits: &mut HitMap) {
        let r = self.rect(area.intersection(buf.area));
        Clear.render(r, buf);
        let block = Block::bordered()
            .style(theme::popup())
            .border_style(theme::popup_border());
        let inner = block.inner(r);
        block.render(r, buf);
        hits.push(r, Target::MenuPopup);

        for (index, entry) in self.entries.iter().enumerate() {
            let Ok(offset) = u16::try_from(index) else {
                break;
            };
            if offset >= inner.height {
                break;
            }
            let y = inner.y + offset;
            match entry {
                MenuEntry::Item { label, enabled, .. } => {
                    let style = if self.highlighted == Some(index) {
                        theme::button_focused()
                    } else if *enabled == Enablement::Disabled {
                        theme::disabled()
                    } else {
                        theme::popup()
                    };
                    let row = Rect::new(inner.x, y, inner.width, 1);
                    buf.set_style(row, style);
                    if inner.width > 1 {
                        buf.set_stringn(inner.x + 1, y, label, usize::from(inner.width - 1), style);
                    }
                    hits.push(row, Target::MenuItem(index));
                }
                MenuEntry::Separator => {
                    if r.width >= 2 {
                        let rule = format!("├{}┤", "─".repeat(usize::from(r.width - 2)));
                        buf.set_stringn(r.x, y, rule, usize::from(r.width), theme::popup_border());
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    // `unwrap` is acceptable in test code: a panic on an unexpected `Err`
    // is exactly the failure signal we want from a test.
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crossterm::event::KeyModifiers;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn item(label: &str, enabled: Enablement, value: u8) -> MenuEntry<u8> {
        MenuEntry::Item {
            label: label.to_owned(),
            enabled,
            value,
        }
    }

    /// Cut (disabled), Copy, ---, Paste, Save (disabled), Select all.
    fn sample(anchor: Position) -> ContextMenu<u8> {
        ContextMenu::new(
            vec![
                item("Cut", Enablement::Disabled, 0),
                item("Copy", Enablement::Enabled, 1),
                MenuEntry::Separator,
                item("Paste", Enablement::Enabled, 2),
                item("Save", Enablement::Disabled, 3),
                item("Select all", Enablement::Enabled, 4),
            ],
            anchor,
        )
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn row_text(buf: &Buffer, y: u16) -> String {
        (buf.area.x..buf.area.right())
            .map(|x| buf[(x, y)].symbol())
            .collect()
    }

    #[test]
    fn new_highlights_nothing() {
        assert_eq!(sample(Position::new(0, 0)).highlighted(), None);
    }

    #[test]
    fn down_skips_separators_and_disabled_items_and_wraps() {
        let mut menu = sample(Position::new(0, 0));
        let mut seen = Vec::new();
        for _ in 0..4 {
            assert_eq!(menu.handle_key(press(KeyCode::Down)), MenuReply::Handled);
            seen.push(menu.highlighted());
        }
        assert_eq!(seen, vec![Some(1), Some(3), Some(5), Some(1)]);
    }

    #[test]
    fn up_skips_separators_and_disabled_items_and_wraps() {
        let mut menu = sample(Position::new(0, 0));
        let mut seen = Vec::new();
        for _ in 0..4 {
            assert_eq!(menu.handle_key(press(KeyCode::Up)), MenuReply::Handled);
            seen.push(menu.highlighted());
        }
        assert_eq!(seen, vec![Some(5), Some(3), Some(1), Some(5)]);
    }

    #[test]
    fn up_from_nothing_goes_to_the_last_enabled_item() {
        let mut menu = ContextMenu::new(
            vec![
                item("A", Enablement::Enabled, 0),
                item("B", Enablement::Enabled, 1),
                MenuEntry::Separator,
                item("C", Enablement::Disabled, 2),
            ],
            Position::new(0, 0),
        );
        menu.handle_key(press(KeyCode::Up));
        assert_eq!(menu.highlighted(), Some(1));
    }

    #[test]
    fn navigation_without_enabled_items_highlights_nothing() {
        let mut menu = ContextMenu::new(
            vec![item("A", Enablement::Disabled, 0), MenuEntry::Separator],
            Position::new(0, 0),
        );
        menu.handle_key(press(KeyCode::Down));
        assert_eq!(menu.highlighted(), None);
        menu.handle_key(press(KeyCode::Up));
        assert_eq!(menu.highlighted(), None);
        let mut empty: ContextMenu<u8> = ContextMenu::new(Vec::new(), Position::new(0, 0));
        empty.handle_key(press(KeyCode::Down));
        empty.handle_key(press(KeyCode::Up));
        assert_eq!(empty.highlighted(), None);
    }

    #[test]
    fn enter_chooses_the_highlighted_item() {
        let mut menu = sample(Position::new(0, 0));
        menu.handle_key(press(KeyCode::Down));
        menu.handle_key(press(KeyCode::Down));
        assert_eq!(menu.handle_key(press(KeyCode::Enter)), MenuReply::Chosen(2));
    }

    #[test]
    fn enter_with_nothing_highlighted_does_nothing() {
        let mut menu = sample(Position::new(0, 0));
        assert_eq!(menu.handle_key(press(KeyCode::Enter)), MenuReply::Handled);
        assert_eq!(menu.highlighted(), None);
    }

    #[test]
    fn esc_dismisses() {
        let mut menu = sample(Position::new(0, 0));
        assert_eq!(menu.handle_key(press(KeyCode::Esc)), MenuReply::Dismissed);
    }

    #[test]
    fn other_keys_and_releases_are_handled() {
        let mut menu = sample(Position::new(0, 0));
        assert_eq!(
            menu.handle_key(press(KeyCode::Char('x'))),
            MenuReply::Handled
        );
        let mut release = press(KeyCode::Esc);
        release.kind = KeyEventKind::Release;
        assert_eq!(menu.handle_key(release), MenuReply::Handled);
        let mut release = press(KeyCode::Down);
        release.kind = KeyEventKind::Release;
        menu.handle_key(release);
        assert_eq!(menu.highlighted(), None);
    }

    #[test]
    fn hover_highlights_enabled_items_and_clears() {
        let mut menu = sample(Position::new(0, 0));
        menu.hover(Some(3));
        assert_eq!(menu.highlighted(), Some(3));
        menu.hover(None);
        assert_eq!(menu.highlighted(), None);
    }

    #[test]
    fn hover_over_a_disabled_item_or_separator_does_nothing() {
        let mut menu = sample(Position::new(0, 0));
        menu.hover(Some(1));
        menu.hover(Some(0));
        assert_eq!(menu.highlighted(), Some(1));
        menu.hover(Some(2));
        assert_eq!(menu.highlighted(), Some(1));
        menu.hover(Some(99));
        assert_eq!(menu.highlighted(), Some(1));
    }

    #[test]
    fn clicks() {
        let mut menu = sample(Position::new(0, 0));
        assert_eq!(menu.click(Some(Target::MenuItem(3))), MenuReply::Chosen(2));
        assert_eq!(menu.click(Some(Target::MenuItem(0))), MenuReply::Handled);
        assert_eq!(menu.click(Some(Target::MenuItem(2))), MenuReply::Handled);
        assert_eq!(menu.click(Some(Target::MenuPopup)), MenuReply::Handled);
        assert_eq!(menu.click(Some(Target::Editor)), MenuReply::Dismissed);
        assert_eq!(menu.click(None), MenuReply::Dismissed);
    }

    #[test]
    fn rect_opens_at_the_anchor() {
        let menu = sample(Position::new(5, 3));
        // "Select all" is 10 wide, so the menu is 14 x 8.
        assert_eq!(menu.rect(Rect::new(0, 0, 80, 24)), Rect::new(5, 3, 14, 8));
    }

    #[test]
    fn rect_at_the_bottom_right_flips_up_and_left() {
        let area = Rect::new(0, 0, 80, 24);
        let menu = sample(Position::new(79, 23));
        assert_eq!(menu.rect(area), Rect::new(66, 16, 14, 8));
    }

    #[test]
    fn rect_larger_than_the_area_is_clamped() {
        let area = Rect::new(2, 1, 6, 4);
        for anchor in [
            Position::new(0, 0),
            Position::new(4, 2),
            Position::new(7, 4),
            Position::new(u16::MAX, u16::MAX),
        ] {
            let rect = sample(anchor).rect(area);
            assert_eq!(rect, area, "anchor {anchor:?}");
        }
        let empty: ContextMenu<u8> = ContextMenu::new(Vec::new(), Position::new(0, 0));
        assert_eq!(empty.rect(Rect::new(0, 0, 0, 0)), Rect::new(0, 0, 0, 0));
    }

    #[test]
    fn rendering_a_clamped_menu_does_not_panic() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 3, 2));
        let mut hits = HitMap::default();
        sample(Position::new(1, 1)).render(buf.area, &mut buf, &mut hits);
        assert_eq!(hits.hit(0, 0), Some(Target::MenuPopup));
        let mut buf = Buffer::empty(Rect::new(0, 0, 0, 0));
        sample(Position::new(0, 0)).render(buf.area, &mut buf, &mut hits);
    }

    #[test]
    fn render_shows_labels_separator_and_highlight() {
        let mut terminal = Terminal::new(TestBackend::new(20, 10)).unwrap();
        let mut menu = sample(Position::new(1, 1));
        menu.hover(Some(3));
        let mut hits = HitMap::default();
        terminal
            .draw(|frame| menu.render(frame.area(), frame.buffer_mut(), &mut hits))
            .unwrap();
        let buf = terminal.backend().buffer();
        assert_eq!(row_text(buf, 1), " ┌────────────┐     ");
        assert_eq!(row_text(buf, 2), " │ Cut        │     ");
        assert_eq!(row_text(buf, 3), " │ Copy       │     ");
        assert_eq!(row_text(buf, 4), " ├────────────┤     ");
        assert_eq!(row_text(buf, 5), " │ Paste      │     ");
        assert_eq!(row_text(buf, 6), " │ Save       │     ");
        assert_eq!(row_text(buf, 7), " │ Select all │     ");
        assert_eq!(row_text(buf, 8), " └────────────┘     ");

        assert_eq!(buf[(3, 3)].style().fg, theme::popup().fg);
        assert_eq!(buf[(3, 3)].style().bg, theme::popup().bg);
        assert_eq!(buf[(3, 2)].style().fg, theme::disabled().fg);
        assert_eq!(buf[(3, 6)].style().fg, theme::disabled().fg);
        for x in 2..14 {
            assert_eq!(buf[(x, 5)].style().bg, theme::button_focused().bg);
        }
        assert_eq!(buf[(1, 4)].style().fg, theme::popup_border().fg);
        assert_eq!(buf[(5, 4)].style().fg, theme::popup_border().fg);
    }

    #[test]
    fn hit_regions_resolve_to_menu_items() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 10));
        let mut hits = HitMap::default();
        hits.push(buf.area, Target::Editor);
        sample(Position::new(1, 1)).render(buf.area, &mut buf, &mut hits);
        assert_eq!(hits.hit(1, 1), Some(Target::MenuPopup));
        assert_eq!(hits.hit(1, 2), Some(Target::MenuPopup));
        assert_eq!(hits.hit(2, 2), Some(Target::MenuItem(0)));
        assert_eq!(hits.hit(13, 3), Some(Target::MenuItem(1)));
        assert_eq!(hits.hit(5, 4), Some(Target::MenuPopup));
        assert_eq!(hits.hit(5, 5), Some(Target::MenuItem(3)));
        assert_eq!(hits.hit(5, 6), Some(Target::MenuItem(4)));
        assert_eq!(hits.hit(5, 7), Some(Target::MenuItem(5)));
        assert_eq!(hits.hit(14, 7), Some(Target::MenuPopup));
        assert_eq!(hits.hit(15, 7), Some(Target::Editor));
        assert_eq!(hits.hit(5, 8), Some(Target::MenuPopup));
        assert_eq!(hits.hit(5, 9), Some(Target::Editor));
    }
}
