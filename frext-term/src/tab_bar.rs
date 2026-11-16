//! The tab bar at the top of the screen.
//!
//! [`items`] lays the bar out as a list of clickable [`TabBarItem`]s and
//! [`render`] draws them. The fixed buttons (`+`, `Open`, `Save`) never
//! scroll; when the tabs overflow, the tab region scrolls just enough to
//! keep the active tab and its close glyph fully visible. Each tab is
//! atomic: its title and close glyph are shown together or not at all.

use frext_core::tab::Tab;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Span;

use crate::hit::{HitMap, Target};
use crate::theme;

/// The separator drawn after the fixed group and after each tab.
const SEPARATOR: &str = "\u{2502}";

/// The close glyph label, followed by one space.
const CLOSE_LABEL: &str = "\u{d7} ";

/// One clickable item on the tab bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabBarItem {
    /// What clicking the item does.
    pub target: Target,
    /// The text drawn for the item, padding included.
    pub label: String,
    /// The column of the item, relative to the bar's left edge.
    pub x: u16,
    /// The display width of the label.
    pub width: u16,
}

/// The display width of `label`.
fn label_width(label: &str) -> usize {
    Span::raw(label).width()
}

/// An item laid out at `start` (in some coordinate space) before it is
/// clipped and placed on the bar.
struct Laid {
    target: Target,
    label: String,
    start: usize,
    width: usize,
}

impl Laid {
    fn new(target: Target, label: String, start: usize) -> Self {
        let width = label_width(&label);
        Self {
            target,
            label,
            start,
            width,
        }
    }

    const fn end(&self) -> usize {
        self.start + self.width
    }

    /// Places the item at bar column `x`, if its coordinates fit a `u16`.
    fn place(self, x: usize) -> Option<TabBarItem> {
        Some(TabBarItem {
            target: self.target,
            x: u16::try_from(x).ok()?,
            width: u16::try_from(self.width).ok()?,
            label: self.label,
        })
    }
}

/// Lays out the tab bar for `tabs` (with `active` the active tab index)
/// in a bar `width` columns wide.
///
/// Separator columns are not items, but the `x` positions leave one
/// column for each. A fixed item that does not fit entirely is omitted.
/// A tab's title and close glyph are emitted only if both fit entirely
/// in the visible tab window; otherwise neither is.
#[must_use]
pub fn items(tabs: &[Tab], active: usize, width: u16) -> Vec<TabBarItem> {
    let width = usize::from(width);
    let mut out = Vec::new();

    let mut column = 0;
    for (target, text) in [
        (Target::NewTab, "+"),
        (Target::OpenFile, "Open"),
        (Target::Save, "Save"),
    ] {
        let laid = Laid::new(target, format!(" {text} "), column);
        column = laid.end();
        let start = laid.start;
        if laid.end() <= width
            && let Some(item) = laid.place(start)
        {
            out.push(item);
        }
    }
    let tabs_start = column + label_width(SEPARATOR);
    let visible = width.saturating_sub(tabs_start);

    // Lay the tab group out in a virtual strip starting at 0, as one
    // (title, close) pair per tab.
    let mut strip = Vec::with_capacity(tabs.len());
    let mut cursor = 0;
    for (index, tab) in tabs.iter().enumerate() {
        let title = Laid::new(
            Target::SelectTab(index),
            format!(" {} ", tab.title()),
            cursor,
        );
        let close = Laid::new(Target::CloseTab(index), CLOSE_LABEL.to_owned(), title.end());
        cursor = close.end() + label_width(SEPARATOR);
        strip.push((title, close));
    }

    let offset = if cursor <= visible {
        0
    } else {
        strip.get(active).map_or(0, |(title, close)| {
            close.end().saturating_sub(visible).min(title.start)
        })
    };

    // A tab is atomic: its title and close glyph are shown together or
    // not at all, so a close glyph is never shown for a hidden title.
    for (title, close) in strip {
        if title.start < offset || close.end() > offset + visible {
            continue;
        }
        let title_x = tabs_start + title.start - offset;
        let close_x = tabs_start + close.start - offset;
        if let (Some(title), Some(close)) = (title.place(title_x), close.place(close_x)) {
            out.push(title);
            out.push(close);
        }
    }
    out
}

/// The style of `target` before any hover highlight.
fn item_style(target: Target, active: usize) -> Style {
    match target {
        Target::SelectTab(index) if index == active => theme::active_tab(),
        Target::CloseTab(_) => theme::close_glyph(),
        _ => theme::chrome(),
    }
}

/// Draws `items` (from [`items`]) into the first row of `area` and
/// records one hit region per item.
///
/// The tab at index `active` is highlighted, and the item whose target
/// is `hovered` gets the hover background.
pub fn render(
    items: &[TabBarItem],
    active: usize,
    hovered: Option<Target>,
    area: Rect,
    buf: &mut Buffer,
    hits: &mut HitMap,
) {
    let area = area.intersection(buf.area);
    if area.is_empty() {
        return;
    }
    let row = Rect::new(area.x, area.y, area.width, 1);
    buf.set_style(row, theme::chrome());

    for item in items {
        if item.x >= area.width {
            continue;
        }
        let x = area.x + item.x;
        let room = area.width - item.x;
        let mut style = item_style(item.target, active);
        if hovered == Some(item.target) {
            style = style.bg(theme::hover_bg());
        }
        buf.set_stringn(x, area.y, &item.label, usize::from(room), style);
        hits.push(Rect::new(x, area.y, item.width.min(room), 1), item.target);

        if matches!(item.target, Target::Save | Target::CloseTab(_)) {
            let sep = u32::from(item.x) + u32::from(item.width);
            if let Ok(sep) = u16::try_from(sep)
                && sep < area.width
            {
                buf.set_string(area.x + sep, area.y, SEPARATOR, theme::chrome());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn untitled(count: u64) -> Vec<Tab> {
        (0..count).map(Tab::new_untitled).collect()
    }

    fn targets(items: &[TabBarItem]) -> Vec<Target> {
        items.iter().map(|item| item.target).collect()
    }

    /// Asserts every emitted `CloseTab(i)` has a `SelectTab(i)` and vice
    /// versa.
    fn assert_paired(items: &[TabBarItem]) {
        let targets = targets(items);
        for target in &targets {
            match *target {
                Target::SelectTab(i) => {
                    assert!(targets.contains(&Target::CloseTab(i)), "{targets:?}");
                }
                Target::CloseTab(i) => {
                    assert!(targets.contains(&Target::SelectTab(i)), "{targets:?}");
                }
                _ => {}
            }
        }
    }

    fn row_text(buf: &Buffer) -> String {
        buf.content().iter().map(|cell| cell.symbol()).collect()
    }

    #[test]
    fn fixed_items_come_first() {
        let tabs = untitled(1);
        let items = items(&tabs, 0, 80);
        let laid: Vec<_> = items
            .iter()
            .map(|item| (item.target, item.label.as_str(), item.x, item.width))
            .collect();
        assert_eq!(
            laid,
            vec![
                (Target::NewTab, " + ", 0, 3),
                (Target::OpenFile, " Open ", 3, 6),
                (Target::Save, " Save ", 9, 6),
                (Target::SelectTab(0), " untitled ", 16, 10),
                (Target::CloseTab(0), "\u{d7} ", 26, 2),
            ]
        );
    }

    #[test]
    fn dirty_tab_shows_bullet() {
        let mut tab = Tab::new_untitled(0);
        tab.text.push('x');
        let items = items(&[tab], 0, 80);
        let title = items
            .iter()
            .find(|item| item.target == Target::SelectTab(0))
            .map(|item| (item.label.as_str(), item.width));
        assert_eq!(title, Some((" \u{2022} untitled ", 12)));
        let close = items
            .iter()
            .find(|item| item.target == Target::CloseTab(0))
            .map(|item| item.x);
        assert_eq!(close, Some(28));
    }

    #[test]
    fn tabs_after_the_first_leave_a_separator_column() {
        let tabs = untitled(2);
        let items = items(&tabs, 0, 80);
        let second = items
            .iter()
            .find(|item| item.target == Target::SelectTab(1))
            .map(|item| item.x);
        // 16 + title 10 + close 2 + separator 1.
        assert_eq!(second, Some(29));
    }

    #[test]
    fn overflow_scrolls_active_tab_into_view() {
        let tabs = untitled(10);
        let items = items(&tabs, 9, 40);
        let targets = targets(&items);
        assert_eq!(
            targets,
            vec![
                Target::NewTab,
                Target::OpenFile,
                Target::Save,
                Target::SelectTab(9),
                Target::CloseTab(9),
            ]
        );
        assert_paired(&items);
        let title = &items[3];
        let close = &items[4];
        assert_eq!((title.x, title.width), (28, 10));
        assert_eq!((close.x, close.width), (38, 2));
        assert!(close.x + close.width <= 40);
        assert!(items.iter().all(|item| item.x + item.width <= 40));
    }

    #[test]
    fn no_orphaned_close_glyphs() {
        let tabs = untitled(10);
        // " untitled " + "× ".
        let tab_width = 12;
        for width in 20..=80u16 {
            let visible = width.saturating_sub(16);
            for active in 0..tabs.len() {
                let items = items(&tabs, active, width);
                assert_paired(&items);
                assert!(items.iter().all(|item| item.x + item.width <= width));
                if tab_width <= visible {
                    let targets = targets(&items);
                    assert!(
                        targets.contains(&Target::SelectTab(active))
                            && targets.contains(&Target::CloseTab(active)),
                        "width {width}, active {active}: {targets:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn overflow_scrolls_middle_tab_into_view() {
        let tabs = untitled(10);
        let items = items(&tabs, 4, 40);
        let targets = targets(&items);
        assert!(targets.contains(&Target::SelectTab(4)));
        assert!(targets.contains(&Target::CloseTab(4)));
        assert!(!targets.contains(&Target::SelectTab(0)));
        assert!(!targets.contains(&Target::SelectTab(9)));
        assert!(items.iter().all(|item| item.x + item.width <= 40));
    }

    #[test]
    fn active_tab_wider_than_view_keeps_its_title_start() {
        let mut tab = Tab::new_untitled(0);
        tab.path = Some("a-very-long-file-name-indeed.txt".into());
        let tabs = vec![Tab::new_untitled(1), tab];
        // Visible tab region is 10 columns; tab 1 is 34 + 2 wide.
        let items = items(&tabs, 1, 26);
        assert_eq!(
            targets(&items),
            vec![Target::NewTab, Target::OpenFile, Target::Save]
        );
    }

    #[test]
    fn active_first_tab_needs_no_scroll() {
        let tabs = untitled(10);
        let items = items(&tabs, 0, 40);
        assert_eq!(
            targets(&items),
            vec![
                Target::NewTab,
                Target::OpenFile,
                Target::Save,
                Target::SelectTab(0),
                Target::CloseTab(0),
            ]
        );
        assert_eq!(items[3].x, 16);
        assert_eq!(items[4].x, 26);
        // Tab 1's title would fit, but its close glyph would end at
        // column 41, past the bar, so the whole tab is omitted.
    }

    #[test]
    fn narrow_bar_drops_fixed_items_that_do_not_fit() {
        let tabs = untitled(1);
        assert_eq!(
            targets(&items(&tabs, 0, 10)),
            vec![Target::NewTab, Target::OpenFile]
        );
        assert_eq!(targets(&items(&tabs, 0, 2)), Vec::new());
        assert_eq!(
            targets(&items(&tabs, 0, 16)),
            vec![Target::NewTab, Target::OpenFile, Target::Save]
        );
    }

    #[test]
    fn no_tabs_yields_only_fixed_items() {
        assert_eq!(
            targets(&items(&[], 0, 80)),
            vec![Target::NewTab, Target::OpenFile, Target::Save]
        );
    }

    #[test]
    fn render_draws_labels_and_separators() {
        let tabs = untitled(2);
        let area = Rect::new(0, 0, 50, 1);
        let mut buf = Buffer::empty(area);
        let mut hits = HitMap::default();
        render(&items(&tabs, 0, 50), 0, None, area, &mut buf, &mut hits);
        assert_eq!(
            row_text(&buf),
            " +  Open  Save \u{2502} untitled \u{d7} \u{2502} untitled \u{d7} \u{2502}        "
        );
        assert_eq!(buf[(0, 0)].style().bg, theme::chrome().bg);
        assert_eq!(buf[(17, 0)].style().fg, theme::active_tab().fg);
        assert_eq!(buf[(30, 0)].style().fg, theme::chrome().fg);
        assert_eq!(buf[(26, 0)].style().fg, theme::close_glyph().fg);
    }

    #[test]
    fn render_registers_hits() {
        let tabs = untitled(3);
        let area = Rect::new(2, 1, 60, 1);
        let mut buf = Buffer::empty(Rect::new(0, 0, 70, 3));
        let mut hits = HitMap::default();
        let laid = items(&tabs, 0, area.width);
        render(&laid, 0, None, area, &mut buf, &mut hits);

        let close = laid
            .iter()
            .find(|item| item.target == Target::CloseTab(1))
            .map(|item| item.x);
        assert_eq!(close, Some(39));
        assert_eq!(hits.hit(area.x + 39, 1), Some(Target::CloseTab(1)));
        assert_eq!(hits.hit(area.x + 40, 1), Some(Target::CloseTab(1)));
        assert_eq!(hits.hit(area.x, 1), Some(Target::NewTab));
        assert_eq!(hits.hit(area.x + 4, 1), Some(Target::OpenFile));
        assert_eq!(hits.hit(area.x + 10, 1), Some(Target::Save));
        assert_eq!(hits.hit(area.x + 30, 1), Some(Target::SelectTab(1)));
        // Separators are not clickable.
        assert_eq!(hits.hit(area.x + 15, 1), None);
        assert_eq!(hits.hit(area.x + 41, 1), None);
        assert_eq!(hits.hit(area.x + 39, 0), None);
    }

    #[test]
    fn render_hover_sets_background() {
        let tabs = untitled(2);
        let area = Rect::new(0, 0, 50, 1);
        let mut buf = Buffer::empty(area);
        let mut hits = HitMap::default();
        let laid = items(&tabs, 0, 50);
        render(
            &laid,
            0,
            Some(Target::CloseTab(1)),
            area,
            &mut buf,
            &mut hits,
        );
        let hover = Some(theme::hover_bg());
        assert_eq!(buf[(39, 0)].style().bg, hover);
        assert_eq!(buf[(40, 0)].style().bg, hover);
        assert_eq!(buf[(39, 0)].style().fg, theme::close_glyph().fg);
        assert_eq!(buf[(41, 0)].style().bg, theme::chrome().bg);
        assert_eq!(buf[(26, 0)].style().bg, theme::chrome().bg);

        let mut buf = Buffer::empty(area);
        render(
            &laid,
            0,
            Some(Target::SelectTab(0)),
            area,
            &mut buf,
            &mut hits,
        );
        assert_eq!(buf[(16, 0)].style().bg, hover);
        assert_eq!(buf[(16, 0)].style().fg, theme::active_tab().fg);
    }

    #[test]
    fn render_clips_to_area() {
        let tabs = untitled(1);
        let laid = items(&tabs, 0, 80);
        let area = Rect::new(0, 0, 20, 1);
        let mut buf = Buffer::empty(area);
        let mut hits = HitMap::default();
        render(&laid, 0, None, area, &mut buf, &mut hits);
        assert_eq!(row_text(&buf), " +  Open  Save \u{2502} unt");
        assert_eq!(hits.hit(19, 0), Some(Target::SelectTab(0)));
        assert_eq!(hits.hit(20, 0), None);

        let mut buf = Buffer::empty(Rect::new(0, 0, 0, 0));
        render(&laid, 0, None, Rect::new(0, 0, 0, 0), &mut buf, &mut hits);
    }
}
