//! A right-click context menu.
//!
//! A [`ContextMenu`] is a bordered list of [`MenuEntry`]s placed by a
//! [`MenuAnchor`]: at a screen cell (a right-click) or on the row below
//! one (opened from the keyboard at the caret). While it is open it is
//! modal: keys and clicks go to it first, and each returns a
//! [`MenuReply`] telling the app whether an item was chosen or the menu
//! was dismissed.

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
    /// One row of text that cannot be chosen: drawn greyed out, never
    /// highlighted, and skipped by keys and hover.
    Note(String),
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

/// Where a context menu opens relative to a screen cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAnchor {
    /// Opens at the cell, moved left or upwards to stay on screen.
    At(Position),
    /// Never covers the cell's row when the area has room on either side
    /// of it: opens on the row under the cell if the menu fits below, else
    /// ends on the row above it if it fits above, else opens on the side
    /// with more room (below on a tie), cut to that room, and scrolls.
    /// A cut keeps at least three rows (both borders and one item), so
    /// only an area without three rows on either side of the cell
    /// overlaps it.
    Below(Position),
}

/// A context menu anchored at a screen cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextMenu<T> {
    entries: Vec<MenuEntry<T>>,
    anchor: MenuAnchor,
    highlighted: Option<usize>,
}

impl<T: Clone> ContextMenu<T> {
    /// A menu of `entries` placed by `anchor`, with nothing highlighted.
    #[must_use]
    pub const fn anchored(entries: Vec<MenuEntry<T>>, anchor: MenuAnchor) -> Self {
        Self {
            entries,
            anchor,
            highlighted: None,
        }
    }

    /// A menu of `entries` opened at `anchor` ([`MenuAnchor::At`]), with
    /// nothing highlighted.
    #[must_use]
    pub const fn new(entries: Vec<MenuEntry<T>>, anchor: Position) -> Self {
        Self::anchored(entries, MenuAnchor::At(anchor))
    }

    /// A menu of `entries` placed around `cell` by [`MenuAnchor::Below`]:
    /// below the cell's row, else above it, else on the side with more
    /// room and cut to it when that room is at least three rows, with
    /// nothing highlighted. Used when the menu is opened from the keyboard
    /// at the caret, so the menu does not cover the caret's row unless
    /// the area has fewer than three rows on either side of it.
    #[must_use]
    pub const fn below(entries: Vec<MenuEntry<T>>, cell: Position) -> Self {
        Self::anchored(entries, MenuAnchor::Below(cell))
    }

    /// Re-places the menu by `anchor`. The entries and the highlight are
    /// kept, so a menu re-anchored while open keeps its keyboard state.
    pub const fn set_anchor(&mut self, anchor: MenuAnchor) {
        self.anchor = anchor;
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

    /// Where the menu is drawn inside `area`: placed by its
    /// [`MenuAnchor`], moved left or opened upwards to stay on screen,
    /// then clamped into `area`. A [`MenuAnchor::Below`] menu that fits
    /// on neither side of its cell's row is cut to the larger side when
    /// that side has at least three rows (else it is clamped, and may
    /// cover the row), so the rect can be shorter than the entries;
    /// [`ContextMenu::render`] then scrolls.
    #[must_use]
    pub fn rect(&self, area: Rect) -> Rect {
        let longest = self
            .entries
            .iter()
            .map(|entry| match entry {
                MenuEntry::Item { label, .. } => label.width(),
                MenuEntry::Note(text) => text.width(),
                MenuEntry::Separator => 0,
            })
            .max()
            .unwrap_or(0);
        let width = u16::try_from(longest).unwrap_or(u16::MAX).saturating_add(4);
        let height = u16::try_from(self.entries.len())
            .unwrap_or(u16::MAX)
            .saturating_add(2);

        let (mut x, mut y, height) = match self.anchor {
            MenuAnchor::At(cell) => {
                let mut y = cell.y;
                if y.saturating_add(height) > area.bottom() {
                    y = cell.y.saturating_add(1).saturating_sub(height);
                }
                (cell.x, y, height)
            }
            MenuAnchor::Below(cell) => {
                let (y, height) = below_placement(cell.y, height, area);
                (cell.x, y, height)
            }
        };
        if x.saturating_add(width) > area.right() {
            x = area.right().saturating_sub(width);
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
    /// [`Target::MenuItem`] with its entry index. Note rows get no region
    /// of their own, so a click on one hits [`Target::MenuPopup`].
    ///
    /// When the menu is clamped shorter than its entries, only a window of
    /// them is drawn: from the first entry, or scrolled down just enough
    /// that the highlighted item is on the last row.
    pub fn render(&self, area: Rect, buf: &mut Buffer, hits: &mut HitMap) {
        let r = self.rect(area.intersection(buf.area));
        Clear.render(r, buf);
        let block = Block::bordered()
            .style(theme::popup())
            .border_style(theme::popup_border());
        let inner = block.inner(r);
        block.render(r, buf);
        hits.push(r, Target::MenuPopup);

        let rows = usize::from(inner.height);
        let start = match self.highlighted {
            Some(highlighted) if rows > 0 && highlighted + 1 > rows => highlighted + 1 - rows,
            _ => 0,
        };
        let visible = self.entries.iter().enumerate().skip(start).take(rows);
        for (offset, (index, entry)) in (0..inner.height).zip(visible) {
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
                MenuEntry::Note(text) => {
                    let style = theme::disabled();
                    buf.set_style(Rect::new(inner.x, y, inner.width, 1), style);
                    if inner.width > 1 {
                        buf.set_stringn(inner.x + 1, y, text, usize::from(inner.width - 1), style);
                    }
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

/// The fewest rows a [`MenuAnchor::Below`] menu is cut to: a border on
/// each side plus one item row. A shorter cut would show no item at all,
/// which is worse than covering the cell's row.
const MIN_CUT_HEIGHT: u16 = 3;

/// The top row and height of a [`MenuAnchor::Below`] menu `height` rows
/// tall around the row `cell_y`, inside `area`.
///
/// A `cell_y` outside `area` is first clamped onto its nearest row, so a
/// caret reported past the edge places the menu as if it were on the edge
/// row. The menu opens under that row if it fits, else ends just above it
/// if it fits there, else takes the side with more room (below on a tie)
/// with its height cut to that room, provided that room is at least
/// [`MIN_CUT_HEIGHT`] rows. When neither side has that many rows, the old
/// "below, else above" position is returned uncut and the caller's clamp
/// settles it, which may cover the cell's row.
fn below_placement(cell_y: u16, height: u16, area: Rect) -> (u16, u16) {
    let row = cell_y.min(area.bottom().saturating_sub(1)).max(area.y);
    let below = area.bottom().saturating_sub(row.saturating_add(1));
    let above = row.saturating_sub(area.y);
    if height <= below {
        (row.saturating_add(1), height)
    } else if height <= above {
        (row.saturating_sub(height), height)
    } else if below.max(above) >= MIN_CUT_HEIGHT {
        if below >= above {
            (row.saturating_add(1), below)
        } else {
            (area.y, above)
        }
    } else {
        let mut y = cell_y.saturating_add(1);
        if y.saturating_add(height) > area.bottom() {
            y = cell_y.saturating_sub(height);
        }
        (y, height)
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

    /// Nine entries, the size of the editor menu: Cut, Copy, Paste, ---,
    /// Select all, ---, Undo, Redo, Last.
    fn tall(anchor: Position) -> ContextMenu<u8> {
        ContextMenu::new(
            vec![
                item("Cut", Enablement::Enabled, 0),
                item("Copy", Enablement::Enabled, 1),
                item("Paste", Enablement::Enabled, 2),
                MenuEntry::Separator,
                item("Select all", Enablement::Enabled, 3),
                MenuEntry::Separator,
                item("Undo", Enablement::Enabled, 4),
                item("Redo", Enablement::Enabled, 5),
                item("Last", Enablement::Enabled, 6),
            ],
            anchor,
        )
    }

    fn render_into(menu: &ContextMenu<u8>, area: Rect) -> (Buffer, HitMap) {
        let mut buf = Buffer::empty(area);
        let mut hits = HitMap::default();
        menu.render(area, &mut buf, &mut hits);
        (buf, hits)
    }

    #[test]
    fn a_clamped_menu_scrolls_to_show_the_highlighted_item() {
        // 8 rows tall: the 11-row menu is clamped to 6 inner rows.
        let area = Rect::new(0, 0, 20, 8);
        let mut menu = tall(Position::new(0, 0));
        menu.handle_key(press(KeyCode::Up));
        assert_eq!(menu.highlighted(), Some(8));
        let (buf, hits) = render_into(&menu, area);

        // Entries 3..9 are drawn on rows 1..7.
        assert_eq!(row_text(&buf, 1), "├────────────┤      ");
        assert_eq!(row_text(&buf, 2), "│ Select all │      ");
        assert_eq!(row_text(&buf, 5), "│ Redo       │      ");
        assert_eq!(row_text(&buf, 6), "│ Last       │      ");
        for x in 1..13 {
            assert_eq!(buf[(x, 6)].style().bg, theme::button_focused().bg);
        }
        assert_eq!(hits.hit(2, 6), Some(Target::MenuItem(8)));
        assert_eq!(hits.hit(2, 2), Some(Target::MenuItem(4)));
        for y in 0..8 {
            let row = row_text(&buf, y);
            assert!(!row.contains("Cut") && !row.contains("Copy"), "{row:?}");
        }
    }

    #[test]
    fn a_clamped_menu_with_nothing_highlighted_starts_at_the_first_entry() {
        let area = Rect::new(0, 0, 20, 8);
        let (buf, hits) = render_into(&tall(Position::new(0, 0)), area);
        assert_eq!(row_text(&buf, 1), "│ Cut        │      ");
        assert_eq!(row_text(&buf, 5), "│ Select all │      ");
        assert_eq!(row_text(&buf, 6), "├────────────┤      ");
        assert_eq!(hits.hit(2, 1), Some(Target::MenuItem(0)));
        assert_eq!(hits.hit(2, 5), Some(Target::MenuItem(4)));
        assert_eq!(hits.hit(2, 6), Some(Target::MenuPopup));
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

    /// A long note, A, ---, B: the note is entry 0.
    fn noted(anchor: Position) -> ContextMenu<u8> {
        ContextMenu::new(
            vec![
                MenuEntry::Note("A long note text".to_owned()),
                item("A", Enablement::Enabled, 0),
                MenuEntry::Separator,
                item("B", Enablement::Enabled, 1),
            ],
            anchor,
        )
    }

    #[test]
    fn a_note_widens_the_menu_and_is_drawn_greyed_out() {
        let menu = noted(Position::new(1, 1));
        // "A long note text" is 16 wide.
        assert_eq!(menu.rect(Rect::new(0, 0, 40, 10)), Rect::new(1, 1, 20, 6));
        let (buf, _) = render_into(&menu, Rect::new(0, 0, 40, 10));
        assert_eq!(
            row_text(&buf, 2),
            " │ A long note text │                   "
        );
        for x in 2..20 {
            assert_eq!(buf[(x, 2)].style().fg, theme::disabled().fg, "x {x}");
        }
        assert_eq!(buf[(3, 3)].style().fg, theme::popup().fg);
    }

    #[test]
    fn a_note_is_skipped_by_keys_and_hover() {
        let mut menu = noted(Position::new(0, 0));
        let mut seen = Vec::new();
        for _ in 0..3 {
            menu.handle_key(press(KeyCode::Down));
            seen.push(menu.highlighted());
        }
        assert_eq!(seen, vec![Some(1), Some(3), Some(1)]);
        let mut seen = Vec::new();
        for _ in 0..3 {
            menu.handle_key(press(KeyCode::Up));
            seen.push(menu.highlighted());
        }
        assert_eq!(seen, vec![Some(3), Some(1), Some(3)]);
        menu.hover(Some(0));
        assert_eq!(menu.highlighted(), Some(3));
        menu.hover(None);
        menu.hover(Some(0));
        assert_eq!(menu.highlighted(), None);
    }

    #[test]
    fn a_click_on_a_note_is_consumed() {
        let mut menu = noted(Position::new(1, 1));
        let (_, hits) = render_into(&menu, Rect::new(0, 0, 40, 10));
        for x in 2..20 {
            assert_eq!(hits.hit(x, 2), Some(Target::MenuPopup), "x {x}");
        }
        assert_eq!(hits.hit(3, 3), Some(Target::MenuItem(1)));
        assert_eq!(menu.click(hits.hit(5, 2)), MenuReply::Handled);
        assert_eq!(menu.highlighted(), None);
    }

    fn sample_below(cell: Position) -> ContextMenu<u8> {
        let mut menu = sample(cell);
        menu.anchor = MenuAnchor::Below(cell);
        assert_eq!(
            menu,
            ContextMenu::below(menu.entries.clone(), cell),
            "below stores a Below anchor"
        );
        menu
    }

    #[test]
    fn below_opens_under_the_cell() {
        let menu = sample_below(Position::new(5, 3));
        assert_eq!(menu.rect(Rect::new(0, 0, 80, 24)), Rect::new(5, 4, 14, 8));
    }

    #[test]
    fn below_near_the_bottom_opens_upwards_above_the_cell() {
        let area = Rect::new(0, 0, 80, 24);
        for y in [17, 20, 23] {
            let rect = sample_below(Position::new(5, y)).rect(area);
            assert_eq!(rect.bottom(), y, "cell y {y}");
            assert_eq!(rect.height, 8);
            assert_eq!(rect.x, 5);
        }
        // The last row that still fits below.
        assert_eq!(
            sample_below(Position::new(5, 15)).rect(area),
            Rect::new(5, 16, 14, 8)
        );
    }

    #[test]
    fn below_near_the_right_edge_shifts_left() {
        let area = Rect::new(0, 0, 80, 24);
        let rect = sample_below(Position::new(75, 3)).rect(area);
        assert_eq!(rect.right(), area.right());
        assert_eq!(rect, Rect::new(66, 4, 14, 8));
    }

    #[test]
    fn below_in_a_small_area_is_cut_and_clamped() {
        // The 8-row menu fits on neither side of any row of this 4-row
        // area (rows 1..5). On the edge rows the larger side has 3 rows,
        // enough for a cut, so it is cut to that side instead of being
        // clamped over the whole area (and the cell's row). On the middle
        // rows neither side has 3 rows, so it is clamped over the whole
        // area. A cell outside the area is treated as being on its
        // nearest row.
        let area = Rect::new(2, 1, 6, 4);
        for (cell, want) in [
            // Row 1 (clamped from 0): 0 above, 3 below: cut below.
            (Position::new(0, 0), Rect::new(2, 2, 6, 3)),
            // Row 2: 1 above, 2 below: clamped.
            (Position::new(4, 2), area),
            // Row 3: 2 above, 1 below: clamped.
            (Position::new(4, 3), area),
            // Row 4: 3 above, 0 below: cut above.
            (Position::new(7, 4), Rect::new(2, 1, 6, 3)),
            (Position::new(u16::MAX, u16::MAX), Rect::new(2, 1, 6, 3)),
        ] {
            assert_eq!(sample_below(cell).rect(area), want, "cell {cell:?}");
        }
        let area = Rect::new(0, 0, 80, 10);
        let rect = sample_below(Position::new(5, 4)).rect(area);
        assert!(
            rect.y >= area.y && rect.bottom() <= area.bottom(),
            "{rect:?}"
        );
        let empty: ContextMenu<u8> = ContextMenu::below(Vec::new(), Position::new(u16::MAX, 0));
        assert_eq!(empty.rect(Rect::new(0, 0, 0, 0)), Rect::new(0, 0, 0, 0));
    }

    /// Fifteen enabled items "Item 0" to "Item 14": 17 rows, 11 columns.
    fn seventeen(cell: Position) -> ContextMenu<u8> {
        let entries = (0..15)
            .map(|i| item(&format!("Item {i}"), Enablement::Enabled, i))
            .collect();
        ContextMenu::below(entries, cell)
    }

    #[test]
    fn below_never_covers_the_cells_row() {
        let area = Rect::new(0, 0, 80, 24);
        for y in 0..24 {
            let rect = seventeen(Position::new(3, y)).rect(area);
            assert!(!(rect.y <= y && y < rect.bottom()), "y {y}: {rect:?}");
            assert_eq!(rect.intersection(area), rect, "y {y}: {rect:?}");
            let below = 23 - y;
            let above = y;
            if below >= 17 {
                assert_eq!((rect.y, rect.height), (y + 1, 17), "y {y}");
            } else if above >= 17 {
                assert_eq!((rect.y, rect.height), (y - 17, 17), "y {y}");
            } else if below >= above {
                assert_eq!((rect.y, rect.height), (y + 1, below), "y {y}");
            } else {
                assert_eq!((rect.y, rect.height), (0, above), "y {y}");
            }
        }
    }

    #[test]
    fn below_takes_the_side_with_more_room_and_cuts_to_it() {
        let area = Rect::new(0, 0, 80, 24);
        // 10 rows above, 13 below.
        assert_eq!(
            seventeen(Position::new(3, 10)).rect(area),
            Rect::new(3, 11, 11, 13)
        );
        // 14 rows above, 9 below.
        assert_eq!(
            seventeen(Position::new(3, 14)).rect(area),
            Rect::new(3, 0, 11, 14)
        );
    }

    #[test]
    fn below_on_a_tie_opens_below() {
        // 12 rows on each side of row 12.
        let area = Rect::new(0, 0, 80, 25);
        assert_eq!(
            seventeen(Position::new(3, 12)).rect(area),
            Rect::new(3, 13, 11, 12)
        );
    }

    #[test]
    fn below_with_under_three_rows_on_each_side_is_clamped() {
        // Row 2 of a 5-row area has 2 rows above and 2 below: a cut to
        // either would show no item, so the 8-row menu is clamped over
        // the whole area, covering the cell's row.
        let area = Rect::new(0, 0, 80, 5);
        assert_eq!(
            sample_below(Position::new(5, 2)).rect(area),
            Rect::new(5, 0, 14, 5)
        );
    }

    #[test]
    fn below_in_a_one_row_area_falls_back_to_the_clamp() {
        let area = Rect::new(0, 5, 80, 1);
        for y in [0, 5, 6, u16::MAX] {
            assert_eq!(
                sample_below(Position::new(5, y)).rect(area),
                Rect::new(5, 5, 14, 1),
                "y {y}"
            );
        }
    }

    #[test]
    fn anchored_matches_new_and_below() {
        let entries = sample(Position::new(0, 0)).entries;
        let p = Position::new(4, 7);
        assert_eq!(
            ContextMenu::anchored(entries.clone(), MenuAnchor::At(p)),
            ContextMenu::new(entries.clone(), p)
        );
        assert_eq!(
            ContextMenu::anchored(entries.clone(), MenuAnchor::Below(p)),
            ContextMenu::below(entries, p)
        );
    }

    #[test]
    fn set_anchor_moves_the_rect_and_keeps_the_highlight() {
        let area = Rect::new(0, 0, 80, 24);
        let mut menu = sample_below(Position::new(5, 3));
        menu.handle_key(press(KeyCode::Down));
        menu.handle_key(press(KeyCode::Down));
        assert_eq!(menu.highlighted(), Some(3));

        menu.set_anchor(MenuAnchor::Below(Position::new(10, 7)));
        assert_eq!(menu.rect(area), Rect::new(10, 8, 14, 8));
        assert_eq!(menu.highlighted(), Some(3));

        menu.set_anchor(MenuAnchor::At(Position::new(2, 1)));
        assert_eq!(menu.rect(area), Rect::new(2, 1, 14, 8));
        assert_eq!(menu.highlighted(), Some(3));

        let before = menu.clone();
        menu.set_anchor(MenuAnchor::At(Position::new(2, 1)));
        assert_eq!(menu, before);
    }

    #[test]
    fn a_cut_below_menu_draws_inside_its_rect_and_scrolls() {
        let area = Rect::new(0, 0, 20, 24);
        let mut menu = seventeen(Position::new(0, 10));
        menu.handle_key(press(KeyCode::Up));
        assert_eq!(menu.highlighted(), Some(14));
        let rect = menu.rect(area);
        assert_eq!(rect, Rect::new(0, 11, 11, 13));
        let (buf, hits) = render_into(&menu, area);

        for y in 0..11 {
            assert_eq!(row_text(&buf, y), " ".repeat(20), "row {y}");
        }
        for y in 11..24 {
            let outside: String = row_text(&buf, y).chars().skip(11).collect();
            assert_eq!(outside, " ".repeat(9), "row {y}");
        }
        // 11 inner rows: entries 4..15 on rows 12..23.
        assert_eq!(row_text(&buf, 11), "┌─────────┐         ");
        assert_eq!(row_text(&buf, 12), "│ Item 4  │         ");
        assert_eq!(row_text(&buf, 22), "│ Item 14 │         ");
        assert_eq!(row_text(&buf, 23), "└─────────┘         ");
        for x in 1..10 {
            assert_eq!(buf[(x, 22)].style().bg, theme::button_focused().bg);
        }
        assert_eq!(hits.hit(2, 22), Some(Target::MenuItem(14)));
        assert_eq!(hits.hit(2, 12), Some(Target::MenuItem(4)));
        assert_eq!(hits.hit(2, 10), None);
        for y in 0..24 {
            assert!(!row_text(&buf, y).contains("Item 3"), "row {y}");
        }
    }
}
