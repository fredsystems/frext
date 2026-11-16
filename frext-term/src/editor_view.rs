//! The read-only editor body and its line-number gutter.
//!
//! [`EditorView`] remembers the first visible line of every tab, scrolls
//! it, and draws the visible lines without wrapping. It has no caret,
//! selection, or highlighting.

use std::collections::HashMap;
use std::ops::Range;

use frext_core::lines::line_count;
use frext_core::tab::Tab;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use crate::hit::{HitMap, Target};
use crate::keymap::ScrollBy;
use crate::theme;

/// The number of spaces a tab character expands to.
const TAB_WIDTH: usize = 4;

/// The first visible line of each tab, by tab id.
#[derive(Debug, Default)]
pub struct EditorView {
    top: HashMap<u64, usize>,
}

/// The largest valid first visible line of `tab`.
fn max_top(tab: &Tab) -> usize {
    line_count(&tab.text).saturating_sub(1)
}

/// Converts a line index to `i64`, saturating.
fn to_i64(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

impl EditorView {
    /// The first visible line of the tab with `tab_id` (0 if unknown).
    #[must_use]
    pub fn top(&self, tab_id: u64) -> usize {
        self.top.get(&tab_id).copied().unwrap_or(0)
    }

    /// Stores `top` for `tab`, clamped to its lines.
    fn set_top(&mut self, tab: &Tab, top: usize) {
        self.top.insert(tab.id, top.min(max_top(tab)));
    }

    /// Moves the first visible line of `tab` by `delta` lines, clamped.
    fn shift(&mut self, tab: &Tab, delta: i64) {
        let current = to_i64(self.top(tab.id).min(max_top(tab)));
        let target = current.saturating_add(delta).max(0);
        self.set_top(tab, usize::try_from(target).unwrap_or(usize::MAX));
    }

    /// Scrolls `tab` in a viewport `height` rows tall. A page is
    /// `height - 1` lines (at least one).
    pub fn scroll(&mut self, tab: &Tab, by: ScrollBy, height: u16) {
        match by {
            ScrollBy::Lines(n) => self.shift(tab, i64::from(n)),
            ScrollBy::Pages(n) => {
                let page = i64::from(height.saturating_sub(1).max(1));
                self.shift(tab, i64::from(n).saturating_mul(page));
            }
            ScrollBy::Top => self.set_top(tab, 0),
            ScrollBy::Bottom => self.set_top(tab, usize::MAX),
        }
    }

    /// Scrolls `tab` by `lines` mouse-wheel lines (negative is up).
    pub fn wheel(&mut self, tab: &Tab, lines: i32) {
        self.shift(tab, i64::from(lines));
    }

    /// Scrolls `tab` so the line holding `chars.start` sits at the
    /// vertical centre of a viewport `height` rows tall.
    pub fn reveal(&mut self, tab: &Tab, chars: Range<usize>, height: u16) {
        let line = tab
            .text
            .chars()
            .take(chars.start)
            .filter(|&c| c == '\n')
            .count();
        self.set_top(tab, line.saturating_sub(usize::from(height / 2)));
    }

    /// Drops the scroll position of every tab not in `live_ids`.
    pub fn forget(&mut self, live_ids: impl IntoIterator<Item = u64>) {
        let live: Vec<u64> = live_ids.into_iter().collect();
        self.top.retain(|id, _| live.contains(id));
    }

    /// Draws the visible lines of `tab` into `editor` and their numbers
    /// into `gutter`, and records `editor` as [`Target::Editor`].
    pub fn render(
        &self,
        tab: Option<&Tab>,
        gutter: Rect,
        editor: Rect,
        buf: &mut Buffer,
        hits: &mut HitMap,
    ) {
        hits.push(editor, Target::Editor);
        let Some(tab) = tab else {
            buf.set_style(gutter, theme::base());
            buf.set_style(editor, theme::base());
            fill(buf, gutter);
            fill(buf, editor);
            return;
        };
        buf.set_style(gutter, theme::gutter());
        fill(buf, gutter);
        buf.set_style(editor, theme::base());
        fill(buf, editor);

        let top = self.top(tab.id).min(max_top(tab));
        let number_width = usize::from(gutter.width.saturating_sub(1));
        let lines = tab.text.split('\n').enumerate().skip(top);
        for ((index, line), row) in lines.zip(0..editor.height.max(gutter.height)) {
            let number = (index + 1).to_string();
            if row < gutter.height && number.len() <= number_width {
                let x = gutter.x + gutter.width - 1 - to_u16(number.len());
                buf.set_string(x, gutter.y + row, &number, theme::gutter());
            }
            if row < editor.height {
                draw_line(buf, editor.x, editor.y + row, editor.width, line);
            }
        }
    }
}

/// Converts a small width to `u16`, saturating.
fn to_u16(value: usize) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}

/// Blanks every cell of `area`, keeping its style.
fn fill(buf: &mut Buffer, area: Rect) {
    let area = area.intersection(buf.area);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            buf[(x, y)].set_symbol(" ");
        }
    }
}

/// Draws `line` at (`x`, `y`), cut at `width` display columns.
///
/// Clipping is left to `Buffer::set_stringn`, which walks grapheme
/// clusters with their cell width and stops at the first one that does
/// not fit, so a wide char or a ZWJ sequence is never split.
fn draw_line(buf: &mut Buffer, x: u16, y: u16, width: u16, line: &str) {
    let line = line.strip_suffix('\r').unwrap_or(line);
    let columns = usize::from(width);
    // Stop building once this many chars are pushed: no realistic text
    // needs more to fill `columns` cells. A line dense with zero-width
    // marks could be cut short of the edge; that is accepted to keep a
    // huge line from being copied in full every frame.
    let max_chars = columns.saturating_mul(8).saturating_add(64);
    let mut visible = String::new();
    let mut pushed = 0_usize;
    for c in line.chars() {
        if pushed >= max_chars {
            break;
        }
        if c == '\t' {
            for _ in 0..TAB_WIDTH {
                visible.push(' ');
            }
            pushed += TAB_WIDTH;
            continue;
        }
        if c.is_control() {
            continue;
        }
        visible.push(c);
        pushed += 1;
    }
    buf.set_stringn(x, y, &visible, columns, theme::base());
}

#[cfg(test)]
mod tests {
    // Tests unwrap freely: a failure is a test failure.
    #![allow(clippy::unwrap_used)]

    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn tab(id: u64, text: &str) -> Tab {
        let mut tab = Tab::new_untitled(id);
        tab.text = text.to_owned();
        tab
    }

    /// Text with lines "1" through "n".
    fn numbered(n: usize) -> String {
        (1..=n)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn render(view: &EditorView, tab: Option<&Tab>, width: u16, height: u16) -> (Buffer, HitMap) {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut hits = HitMap::default();
        terminal
            .draw(|frame| {
                let gutter = Rect::new(0, 0, 4, height);
                let editor = Rect::new(4, 0, width - 4, height);
                view.render(tab, gutter, editor, frame.buffer_mut(), &mut hits);
            })
            .unwrap();
        (terminal.backend().buffer().clone(), hits)
    }

    fn colours_of(
        style: ratatui::style::Style,
    ) -> (Option<ratatui::style::Color>, Option<ratatui::style::Color>) {
        (style.fg, style.bg)
    }

    fn colours(
        buf: &Buffer,
        x: u16,
        y: u16,
    ) -> (Option<ratatui::style::Color>, Option<ratatui::style::Color>) {
        colours_of(buf[(x, y)].style())
    }

    fn row_text(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
    }

    #[test]
    fn top_defaults_to_zero() {
        assert_eq!(EditorView::default().top(7), 0);
    }

    #[test]
    fn scroll_clamps_at_top() {
        let tab = tab(1, &numbered(10));
        let mut view = EditorView::default();
        view.scroll(&tab, ScrollBy::Lines(-3), 5);
        assert_eq!(view.top(1), 0);
        view.scroll(&tab, ScrollBy::Lines(3), 5);
        assert_eq!(view.top(1), 3);
        view.scroll(&tab, ScrollBy::Lines(i32::MIN), 5);
        assert_eq!(view.top(1), 0);
        view.scroll(&tab, ScrollBy::Top, 5);
        assert_eq!(view.top(1), 0);
    }

    #[test]
    fn scroll_clamps_at_bottom() {
        let tab = tab(1, &numbered(10));
        let mut view = EditorView::default();
        view.scroll(&tab, ScrollBy::Lines(100), 5);
        assert_eq!(view.top(1), 9);
        view.scroll(&tab, ScrollBy::Lines(i32::MAX), 5);
        assert_eq!(view.top(1), 9);
        view.scroll(&tab, ScrollBy::Top, 5);
        view.scroll(&tab, ScrollBy::Bottom, 5);
        assert_eq!(view.top(1), 9);
    }

    #[test]
    fn scroll_by_pages() {
        let tab = tab(1, &numbered(100));
        let mut view = EditorView::default();
        view.scroll(&tab, ScrollBy::Pages(2), 11);
        assert_eq!(view.top(1), 20);
        view.scroll(&tab, ScrollBy::Pages(-1), 11);
        assert_eq!(view.top(1), 10);
        view.scroll(&tab, ScrollBy::Pages(1), 1);
        assert_eq!(view.top(1), 11);
        view.scroll(&tab, ScrollBy::Pages(1), 0);
        assert_eq!(view.top(1), 12);
        view.scroll(&tab, ScrollBy::Pages(i32::MAX), u16::MAX);
        assert_eq!(view.top(1), 99);
        view.scroll(&tab, ScrollBy::Pages(i32::MIN), u16::MAX);
        assert_eq!(view.top(1), 0);
    }

    #[test]
    fn wheel_scrolls_and_clamps() {
        let tab = tab(1, &numbered(10));
        let mut view = EditorView::default();
        view.wheel(&tab, 3);
        assert_eq!(view.top(1), 3);
        view.wheel(&tab, -1);
        assert_eq!(view.top(1), 2);
        view.wheel(&tab, -10);
        assert_eq!(view.top(1), 0);
        view.wheel(&tab, 50);
        assert_eq!(view.top(1), 9);
    }

    #[test]
    fn scroll_is_per_tab() {
        let first = tab(1, &numbered(10));
        let second = tab(2, &numbered(10));
        let mut view = EditorView::default();
        view.scroll(&first, ScrollBy::Lines(4), 5);
        assert_eq!(view.top(1), 4);
        assert_eq!(view.top(2), 0);
        view.scroll(&second, ScrollBy::Lines(1), 5);
        assert_eq!(view.top(1), 4);
        assert_eq!(view.top(2), 1);
    }

    #[test]
    fn reveal_centres_line() {
        let text = numbered(100);
        let tab = tab(1, &text);
        // Char offset of the start of line index 50 ("51").
        let start = text
            .split('\n')
            .take(50)
            .map(|l| l.len() + 1)
            .sum::<usize>();
        let mut view = EditorView::default();
        view.reveal(&tab, start..start + 2, 10);
        assert_eq!(view.top(1), 45);
    }

    #[test]
    fn reveal_clamps_at_start() {
        let tab = tab(1, &numbered(100));
        let mut view = EditorView::default();
        view.scroll(&tab, ScrollBy::Lines(30), 10);
        view.reveal(&tab, 2..3, 10);
        assert_eq!(view.top(1), 0);
    }

    #[test]
    fn reveal_past_end_clamps_to_last_line() {
        let tab = tab(1, "a\nb\nc");
        let mut view = EditorView::default();
        view.reveal(&tab, 1_000..1_001, 0);
        assert_eq!(view.top(1), 2);
    }

    #[test]
    fn reveal_counts_chars_not_bytes() {
        let tab = tab(1, "éé\nb\nc");
        let mut view = EditorView::default();
        // Char 3 is 'b' on line 1; byte 3 would still be on line 0.
        view.reveal(&tab, 3..4, 0);
        assert_eq!(view.top(1), 1);
    }

    #[test]
    fn forget_drops_closed_tabs() {
        let first = tab(1, &numbered(10));
        let second = tab(2, &numbered(10));
        let mut view = EditorView::default();
        view.scroll(&first, ScrollBy::Lines(3), 5);
        view.scroll(&second, ScrollBy::Lines(4), 5);
        view.forget([2]);
        assert_eq!(view.top(1), 0);
        assert_eq!(view.top(2), 4);
        view.forget(std::iter::empty());
        assert_eq!(view.top(2), 0);
    }

    #[test]
    fn render_right_aligns_line_numbers() {
        let tab = tab(1, &numbered(12));
        let mut view = EditorView::default();
        view.scroll(&tab, ScrollBy::Lines(8), 4);
        let (buf, _) = render(&view, Some(&tab), 12, 4);
        assert_eq!(row_text(&buf, 0), "  9 9       ");
        assert_eq!(row_text(&buf, 1), " 10 10      ");
        assert_eq!(row_text(&buf, 3), " 12 12      ");
        assert_eq!(buf[(1, 1)].style().fg, theme::gutter().fg);
        assert_eq!(buf[(3, 1)].style().bg, theme::gutter().bg);
        assert_eq!(colours(&buf, 4, 1), colours_of(theme::base()));
    }

    #[test]
    fn render_skips_numbers_wider_than_gutter() {
        let tab = tab(1, &numbered(1_000));
        let mut view = EditorView::default();
        view.scroll(&tab, ScrollBy::Lines(998), 2);
        let (buf, _) = render(&view, Some(&tab), 12, 2);
        assert_eq!(row_text(&buf, 0), "999 999     ");
        assert_eq!(row_text(&buf, 1), "    1000    ");
    }

    #[test]
    fn render_expands_tabs() {
        let tab = tab(1, "\tx\ty");
        let (buf, _) = render(&EditorView::default(), Some(&tab), 20, 1);
        assert_eq!(row_text(&buf, 0), "  1     x    y      ");
    }

    #[test]
    fn render_truncates_long_line() {
        let tab = tab(1, "abcdefghijklmnop\nxy");
        let (buf, _) = render(&EditorView::default(), Some(&tab), 10, 2);
        assert_eq!(row_text(&buf, 0), "  1 abcdef");
        assert_eq!(row_text(&buf, 1), "  2 xy    ");
    }

    #[test]
    fn render_truncates_tab_at_edge() {
        let tab = tab(1, "abcd\tz");
        let (buf, _) = render(&EditorView::default(), Some(&tab), 10, 1);
        assert_eq!(row_text(&buf, 0), "  1 abcd  ");
    }

    #[test]
    fn render_scrolled_view_starts_at_line_5() {
        let tab = tab(1, &numbered(20));
        let mut view = EditorView::default();
        view.scroll(&tab, ScrollBy::Lines(4), 3);
        let (buf, _) = render(&view, Some(&tab), 10, 3);
        assert_eq!(row_text(&buf, 0), "  5 5     ");
        assert_eq!(row_text(&buf, 1), "  6 6     ");
        assert_eq!(row_text(&buf, 2), "  7 7     ");
    }

    #[test]
    fn render_past_last_line_is_blank() {
        let tab = tab(1, "a\nb");
        let (buf, _) = render(&EditorView::default(), Some(&tab), 8, 3);
        assert_eq!(row_text(&buf, 1), "  2 b   ");
        assert_eq!(row_text(&buf, 2), "        ");
        assert_eq!(buf[(0, 2)].style().bg, theme::base().bg);
    }

    #[test]
    fn render_empty_buffer_shows_line_1() {
        let tab = tab(1, "");
        let (buf, hits) = render(&EditorView::default(), Some(&tab), 8, 2);
        assert_eq!(row_text(&buf, 0), "  1     ");
        assert_eq!(row_text(&buf, 1), "        ");
        assert_eq!(hits.hit(4, 0), Some(Target::Editor));
        assert_eq!(hits.hit(0, 0), None);
    }

    #[test]
    fn render_strips_carriage_return_and_control_chars() {
        let tab = tab(1, "a\u{1}b\r\nc\r");
        let (buf, _) = render(&EditorView::default(), Some(&tab), 8, 2);
        assert_eq!(row_text(&buf, 0), "  1 ab  ");
        assert_eq!(row_text(&buf, 1), "  2 c   ");
    }

    #[test]
    fn render_multi_byte_line() {
        let tab = tab(1, "héllo 世界!");
        let (buf, _) = render(&EditorView::default(), Some(&tab), 20, 1);
        assert_eq!(buf[(5, 0)].symbol(), "é");
        assert_eq!(buf[(10, 0)].symbol(), "世");
        assert_eq!(buf[(12, 0)].symbol(), "界");
        assert_eq!(buf[(14, 0)].symbol(), "!");
    }

    #[test]
    fn render_never_splits_wide_char_at_edge() {
        let tab = tab(1, "a世界");
        // Editor is 4 columns: "a" + "世" fill 3, "界" would need 5.
        let (buf, _) = render(&EditorView::default(), Some(&tab), 8, 1);
        assert_eq!(buf[(5, 0)].symbol(), "世");
        assert_eq!(buf[(7, 0)].symbol(), " ");
    }

    #[test]
    fn render_keeps_zwj_sequence_whole() {
        let tab = tab(1, "👩‍💻x");
        // Editor is 2 columns: the emoji fills both, "x" does not fit.
        let (buf, _) = render(&EditorView::default(), Some(&tab), 6, 1);
        assert_eq!(buf[(4, 0)].symbol(), "👩‍💻");
        assert_ne!(buf[(5, 0)].symbol(), "x");
        assert!(!row_text(&buf, 0).contains('x'));
    }

    #[test]
    fn render_zwj_sequence_does_not_truncate_following_text() {
        let tab = tab(1, "👩‍💻x");
        let (buf, _) = render(&EditorView::default(), Some(&tab), 7, 1);
        assert_eq!(buf[(4, 0)].symbol(), "👩‍💻");
        assert_eq!(buf[(6, 0)].symbol(), "x");
    }

    #[test]
    fn render_without_tab_fills_base() {
        let (buf, hits) = render(&EditorView::default(), None, 8, 2);
        assert_eq!(row_text(&buf, 0), "        ");
        assert_eq!(row_text(&buf, 1), "        ");
        assert_eq!(colours(&buf, 0, 0), colours_of(theme::base()));
        assert_eq!(colours(&buf, 5, 1), colours_of(theme::base()));
        assert_eq!(hits.hit(5, 1), Some(Target::Editor));
    }

    #[test]
    fn render_clamps_stale_top_after_text_shrinks() {
        let mut tab = tab(1, &numbered(20));
        let mut view = EditorView::default();
        view.scroll(&tab, ScrollBy::Bottom, 3);
        tab.text = "a\nb".to_owned();
        let (buf, _) = render(&view, Some(&tab), 8, 2);
        assert_eq!(row_text(&buf, 0), "  2 b   ");
    }
}
