//! Drawing laid-out, highlighted rows into a terminal buffer.
//!
//! [`render`] fills the gutter and the editor, numbers the first visual
//! row of each logical line, draws every visible grapheme with its syntax
//! colour and background (selection over search match over base), paints
//! one selection cell after a selected line break, and returns the
//! caret's screen position. It takes the layout and the highlighting as
//! input and keeps no state.

use std::ops::Range;

use frext_core::highlight::StyledSpan;
use frext_core::palette::BASE;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use unicode_segmentation::UnicodeSegmentation;

use crate::buffer::cells::{self, Glyph};
use crate::buffer::wrap::{RowBreak, VisualRow, WrapLayout};
use crate::paint;
use crate::theme;

/// Everything [`render`] draws: a text, its layout, and what to paint on
/// it.
#[derive(Debug, Clone)]
pub struct RowsView<'a> {
    /// The whole text.
    pub text: &'a str,
    /// The text laid out at the editor's wrap width.
    pub layout: &'a WrapLayout,
    /// The first visible visual row.
    pub top_row: usize,
    /// Sorted, non-overlapping highlight spans (byte ranges into `text`)
    /// covering at least the visible rows.
    pub spans: &'a [StyledSpan],
    /// The selected chars (empty for none).
    pub selection: Range<usize>,
    /// The caret's char index.
    pub caret: usize,
}

/// Draws `view` into `editor` and its line numbers into `gutter`, and
/// returns the caret's screen position when its row is visible and its
/// column is inside `editor`.
///
/// Nothing is drawn outside `gutter` and `editor`.
pub fn render(
    view: &RowsView<'_>,
    gutter: Rect,
    editor: Rect,
    buf: &mut Buffer,
) -> Option<Position> {
    let gutter_clip = gutter.intersection(buf.area);
    let editor_clip = editor.intersection(buf.area);
    paint::fill(buf, gutter_clip, theme::gutter());
    paint::fill(buf, editor_clip, theme::base());

    let rows = view.layout.rows();
    let mut painter = Painter {
        buf,
        clip: editor_clip,
        spans: view.spans,
        span_index: 0,
    };
    for offset in 0..editor.height {
        let Some(index) = view.top_row.checked_add(usize::from(offset)) else {
            break;
        };
        let Some(row) = rows.get(index) else {
            break;
        };
        let first_of_line =
            index == 0 || rows.get(index - 1).is_none_or(|prev| prev.line != row.line);
        let y = editor.y.saturating_add(offset);
        if first_of_line {
            draw_number(
                painter.buf,
                gutter,
                gutter_clip,
                gutter.y.saturating_add(offset),
                row.line,
            );
        }
        painter.draw_row(view, row, editor.x, y);
    }

    caret_position(view, editor)
}

/// The caret's screen position, if it is on screen.
fn caret_position(view: &RowsView<'_>, editor: Rect) -> Option<Position> {
    let caret_row = view.layout.row_of_char(view.caret);
    let offset = caret_row.checked_sub(view.top_row)?;
    if offset >= usize::from(editor.height) {
        return None;
    }
    let y = editor.y.saturating_add(u16::try_from(offset).ok()?);
    let x = editor
        .x
        .saturating_add(view.layout.column_of_char(view.text, view.caret));
    (x < editor.right()).then_some(Position { x, y })
}

/// Draws the 1-based number of `line` right-aligned in `gutter` on row
/// `y`, with one trailing pad column, unless it does not fit.
fn draw_number(buf: &mut Buffer, gutter: Rect, clip: Rect, y: u16, line: usize) {
    let number = line.saturating_add(1).to_string();
    let room = usize::from(gutter.width.saturating_sub(1));
    if number.len() > room || y < clip.top() || y >= clip.bottom() {
        return;
    }
    let Ok(len) = u16::try_from(number.len()) else {
        return;
    };
    let x = gutter.right().saturating_sub(1).saturating_sub(len);
    if x < clip.left() || x >= clip.right() {
        return;
    }
    let max_width = usize::from(clip.right().saturating_sub(x));
    buf.set_stringn(x, y, &number, max_width, theme::gutter());
}

/// Draws rows into the editor, walking the spans across the frame.
struct Painter<'b, 's> {
    buf: &'b mut Buffer,
    clip: Rect,
    spans: &'s [StyledSpan],
    span_index: usize,
}

impl Painter<'_, '_> {
    /// The span covering byte `byte`, advancing the moving index.
    fn span_at(&mut self, byte: usize) -> Option<&StyledSpan> {
        while self
            .spans
            .get(self.span_index)
            .is_some_and(|span| span.bytes.end <= byte)
        {
            self.span_index += 1;
        }
        self.spans
            .get(self.span_index)
            .filter(|span| span.bytes.start <= byte)
    }

    /// Draws `row` starting at (`x`, `y`).
    fn draw_row(&mut self, view: &RowsView<'_>, row: &VisualRow, x: u16, y: u16) {
        if y < self.clip.top() || y >= self.clip.bottom() {
            return;
        }
        let right = self.clip.right();
        let content = view.text.get(row.bytes.clone()).unwrap_or("");
        let mut x = x;
        let mut char_idx = row.chars.start;
        for (offset, grapheme) in content.grapheme_indices(true) {
            if x >= right {
                return;
            }
            let selected = view.selection.contains(&char_idx);
            let (fg_style, match_role) = match self.span_at(row.bytes.start.saturating_add(offset))
            {
                Some(span) => (theme::span(span), span.background),
                None => (theme::base(), None),
            };
            let bg = if selected {
                theme::selection_bg()
            } else {
                match_role.map_or(theme::color(BASE), theme::match_bg)
            };
            let width = cells::width(grapheme);
            let style = match cells::glyph(grapheme) {
                Glyph::Text(text) => {
                    self.write(x, y, text, fg_style.bg(bg));
                    fg_style.bg(bg)
                }
                Glyph::Blank(n) => {
                    let spaces = " ".repeat(usize::from(n));
                    self.write(x, y, &spaces, fg_style.bg(bg));
                    fg_style.bg(bg)
                }
                Glyph::Replacement => {
                    let style = theme::replacement().bg(bg);
                    self.write(x, y, cells::REPLACEMENT, style);
                    style
                }
            };
            self.style_cells(x, y, width, style);
            x = x.saturating_add(width);
            char_idx = char_idx.saturating_add(grapheme.chars().count());
        }
        if row.break_after == RowBreak::Newline
            && view.selection.contains(&row.chars.end)
            && x < right
            && x >= self.clip.left()
            && let Some(cell) = self.buf.cell_mut((x, y))
        {
            cell.set_bg(theme::selection_bg());
        }
    }

    /// Writes `text` at (`x`, `y`), clipped to the editor row.
    fn write(&mut self, x: u16, y: u16, text: &str, style: Style) {
        if x < self.clip.left() || x >= self.clip.right() {
            return;
        }
        let max_width = usize::from(self.clip.right().saturating_sub(x));
        self.buf.set_stringn(x, y, text, max_width, style);
    }

    /// Applies `style` to the `width` cells from (`x`, `y`) inside the
    /// editor, so a wide char's hidden cells keep the background.
    fn style_cells(&mut self, x: u16, y: u16, width: u16, style: Style) {
        let end = x.saturating_add(width).min(self.clip.right());
        for cx in x.max(self.clip.left())..end {
            if let Some(cell) = self.buf.cell_mut((cx, y)) {
                cell.set_style(style);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    // Tests unwrap freely: a failure is a test failure.
    #![allow(clippy::unwrap_used)]

    use super::*;
    use frext_core::highlight::{MatchBackground, MatchHighlights, highlight, overlay_matches};
    use frext_core::palette;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;

    const GUTTER: u16 = 4;

    /// Renders `text` laid out at `editor_width - 1` into a terminal
    /// `GUTTER + editor_width` wide and `height` tall.
    fn draw(
        text: &str,
        spans: &[StyledSpan],
        top_row: usize,
        selection: Range<usize>,
        caret: usize,
        editor_width: u16,
        height: u16,
    ) -> (Buffer, Option<Position>) {
        let layout = WrapLayout::new(text, editor_width.saturating_sub(1));
        let view = RowsView {
            text,
            layout: &layout,
            top_row,
            spans,
            selection,
            caret,
        };
        let mut terminal = Terminal::new(TestBackend::new(GUTTER + editor_width, height)).unwrap();
        let mut caret_at = None;
        terminal
            .draw(|frame| {
                let gutter = Rect::new(0, 0, GUTTER, height);
                let editor = Rect::new(GUTTER, 0, editor_width, height);
                caret_at = render(&view, gutter, editor, frame.buffer_mut());
            })
            .unwrap();
        (terminal.backend().buffer().clone(), caret_at)
    }

    fn plain(text: &str) -> Vec<StyledSpan> {
        highlight(text, "")
    }

    fn row_text(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
    }

    fn bg(buf: &Buffer, x: u16, y: u16) -> Option<Color> {
        buf[(x, y)].style().bg
    }

    fn base_bg() -> Option<Color> {
        Some(theme::color(BASE))
    }

    #[test]
    fn gutter_numbers_only_first_row_of_wrapped_line() {
        let text = "abcdefgh\nxy";
        // Wrap width 4: "abcd", "efgh", "xy".
        let (buf, _) = draw(text, &plain(text), 0, 0..0, 0, 5, 4);
        assert_eq!(row_text(&buf, 0), "  1 abcd ");
        assert_eq!(row_text(&buf, 1), "    efgh ");
        assert_eq!(row_text(&buf, 2), "  2 xy   ");
        assert_eq!(row_text(&buf, 3), "         ");
        assert_eq!(buf[(2, 0)].style().fg, theme::gutter().fg);
        assert_eq!(bg(&buf, 0, 3), theme::gutter().bg);
    }

    #[test]
    fn gutter_numbers_first_visible_row_only_if_line_starts_there() {
        let text = "abcdefgh\nxy";
        let (buf, _) = draw(text, &plain(text), 1, 0..0, 0, 5, 2);
        assert_eq!(row_text(&buf, 0), "    efgh ");
        assert_eq!(row_text(&buf, 1), "  2 xy   ");
    }

    #[test]
    fn gutter_skips_numbers_too_wide() {
        let text: String = (1..=1_000).map(|i| format!("{i}\n")).collect();
        let (buf, _) = draw(&text, &plain(&text), 998, 0..0, 0, 6, 2);
        assert_eq!(row_text(&buf, 0), "999 999   ");
        assert_eq!(row_text(&buf, 1), "    1000  ");
    }

    #[test]
    fn syntax_colours_keyword() {
        let text = "fn main() {}\n";
        let spans = highlight(text, "rs");
        let keyword = spans.iter().find(|s| s.bytes.start == 0).unwrap();
        let ident = spans.iter().find(|s| s.bytes.contains(&3)).unwrap();
        assert_ne!(keyword.fg, ident.fg, "fixture needs distinct colours");
        let (buf, _) = draw(text, &spans, 0, 0..0, 0, 20, 2);
        assert_eq!(buf[(GUTTER, 0)].symbol(), "f");
        assert_eq!(buf[(GUTTER, 0)].style().fg, Some(theme::color(keyword.fg)));
        assert_eq!(
            buf[(GUTTER + 1, 0)].style().fg,
            Some(theme::color(keyword.fg))
        );
        assert_eq!(
            buf[(GUTTER + 3, 0)].style().fg,
            Some(theme::color(ident.fg))
        );
        assert_eq!(bg(&buf, GUTTER, 0), base_bg());
    }

    #[test]
    fn uncovered_grapheme_uses_base() {
        let text = "ab";
        let spans = vec![StyledSpan {
            bytes: 0..1,
            fg: palette::RED,
            background: None,
            italic: true,
            underline: false,
        }];
        let (buf, _) = draw(text, &spans, 0, 0..0, 0, 10, 1);
        assert_eq!(
            buf[(GUTTER, 0)].style().fg,
            Some(theme::color(palette::RED))
        );
        assert_eq!(buf[(GUTTER + 1, 0)].style().fg, theme::base().fg);
    }

    #[test]
    fn selection_bg_on_selected_cells_and_newline() {
        let text = "abc\ndef";
        // Select "bc\nd": chars 1..5.
        let (buf, _) = draw(text, &plain(text), 0, 1..5, 0, 10, 2);
        let sel = Some(theme::selection_bg());
        assert_eq!(bg(&buf, GUTTER, 0), base_bg());
        assert_eq!(bg(&buf, GUTTER + 1, 0), sel);
        assert_eq!(bg(&buf, GUTTER + 2, 0), sel);
        // The newline cell right after "abc".
        assert_eq!(bg(&buf, GUTTER + 3, 0), sel);
        assert_eq!(bg(&buf, GUTTER + 4, 0), base_bg());
        assert_eq!(bg(&buf, GUTTER, 1), sel);
        assert_eq!(bg(&buf, GUTTER + 1, 1), base_bg());
        // The end of the text has no newline cell.
        let (buf, _) = draw(text, &plain(text), 0, 0..7, 0, 10, 2);
        assert_eq!(bg(&buf, GUTTER + 2, 1), sel);
        assert_eq!(bg(&buf, GUTTER + 3, 1), base_bg());
    }

    #[test]
    fn crlf_newline_cell_and_no_carriage_return_drawn() {
        let text = "ab\r\ncd";
        let (buf, _) = draw(text, &plain(text), 0, 2..3, 0, 10, 2);
        assert_eq!(row_text(&buf, 0), "  1 ab        ");
        assert_eq!(bg(&buf, GUTTER + 2, 0), Some(theme::selection_bg()));
        assert_eq!(bg(&buf, GUTTER + 3, 0), base_bg());
    }

    #[test]
    fn newline_cell_unselected_when_selection_ends_before_it() {
        let text = "abc\ndef";
        let (buf, _) = draw(text, &plain(text), 0, 0..3, 0, 10, 2);
        assert_eq!(bg(&buf, GUTTER + 2, 0), Some(theme::selection_bg()));
        assert_eq!(bg(&buf, GUTTER + 3, 0), base_bg());
    }

    #[test]
    fn selection_wins_over_match_and_match_roles_paint() {
        let text = "foo foo foo";
        let matches = MatchHighlights {
            ranges: vec![0..3, 4..7, 8..11],
            current: Some(1),
        };
        let spans = overlay_matches(plain(text), &matches);
        // Select the first match's middle char and the third match.
        let (buf, _) = draw(text, &spans, 0, 1..2, 0, 20, 1);
        let other = Some(theme::match_bg(MatchBackground::Other));
        let current = Some(theme::match_bg(MatchBackground::Current));
        assert_eq!(bg(&buf, GUTTER, 0), other);
        assert_eq!(bg(&buf, GUTTER + 1, 0), Some(theme::selection_bg()));
        assert_eq!(bg(&buf, GUTTER + 2, 0), other);
        assert_eq!(bg(&buf, GUTTER + 3, 0), base_bg());
        assert_eq!(bg(&buf, GUTTER + 4, 0), current);
        assert_eq!(bg(&buf, GUTTER + 8, 0), other);
    }

    #[test]
    fn tab_control_and_wide_cells() {
        let text = "\tx\u{7}y漢z";
        let sel = 0..text.chars().count();
        let (buf, _) = draw(text, &plain(text), 0, sel, 0, 20, 1);
        for dx in 0..4 {
            assert_eq!(buf[(GUTTER + dx, 0)].symbol(), " ");
            assert_eq!(bg(&buf, GUTTER + dx, 0), Some(theme::selection_bg()));
        }
        assert_eq!(buf[(GUTTER + 4, 0)].symbol(), "x");
        assert_eq!(buf[(GUTTER + 5, 0)].symbol(), cells::REPLACEMENT);
        assert_eq!(buf[(GUTTER + 5, 0)].style().fg, theme::replacement().fg);
        assert_eq!(bg(&buf, GUTTER + 5, 0), Some(theme::selection_bg()));
        assert_eq!(buf[(GUTTER + 6, 0)].symbol(), "y");
        assert_eq!(buf[(GUTTER + 7, 0)].symbol(), "漢");
        assert_eq!(buf[(GUTTER + 9, 0)].symbol(), "z");
    }

    #[test]
    fn wide_char_hidden_cell_keeps_background() {
        // The terminal diff skips a wide char's hidden cell, so check the
        // frame buffer itself.
        let text = "漢z";
        let layout = WrapLayout::new(text, 9);
        let spans = plain(text);
        let view = RowsView {
            text,
            layout: &layout,
            top_row: 0,
            spans: &spans,
            selection: 0..2,
            caret: 0,
        };
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 1));
        render(
            &view,
            Rect::new(0, 0, 0, 1),
            Rect::new(0, 0, 10, 1),
            &mut buf,
        );
        assert_eq!(buf[(0, 0)].symbol(), "漢");
        assert_eq!(bg(&buf, 1, 0), Some(theme::selection_bg()));
        assert_eq!(buf[(2, 0)].symbol(), "z");
    }

    #[test]
    fn caret_positions() {
        let text = "abcdefgh\nxy";
        // Wrap width 4: rows "abcd", "efgh", "xy".
        let spans = plain(text);
        let at = |caret: usize| draw(text, &spans, 0, 0..0, caret, 5, 3).1;
        assert_eq!(at(0), Some(Position { x: GUTTER, y: 0 }));
        assert_eq!(
            at(2),
            Some(Position {
                x: GUTTER + 2,
                y: 0
            })
        );
        // The soft-wrap boundary is the start of the next row.
        assert_eq!(at(4), Some(Position { x: GUTTER, y: 1 }));
        // The end of the wrapped line, before the newline.
        assert_eq!(
            at(8),
            Some(Position {
                x: GUTTER + 4,
                y: 1
            })
        );
        // After the last line.
        assert_eq!(
            at(11),
            Some(Position {
                x: GUTTER + 2,
                y: 2
            })
        );
    }

    #[test]
    fn caret_none_when_scrolled_away() {
        let text = "a\nb\nc\nd";
        let spans = plain(text);
        assert_eq!(draw(text, &spans, 2, 0..0, 0, 5, 2).1, None);
        assert_eq!(draw(text, &spans, 0, 0..0, 6, 5, 2).1, None);
        assert_eq!(
            draw(text, &spans, 2, 0..0, 6, 5, 2).1,
            Some(Position { x: GUTTER, y: 1 })
        );
    }

    #[test]
    fn top_row_past_end_draws_only_fills() {
        let text = "a\nb";
        let (buf, caret) = draw(text, &plain(text), 50, 0..3, 0, 5, 2);
        assert_eq!(row_text(&buf, 0), "         ");
        assert_eq!(row_text(&buf, 1), "         ");
        assert_eq!(bg(&buf, GUTTER, 0), base_bg());
        assert_eq!(caret, None);
        let _ = draw(text, &plain(text), usize::MAX, 0..3, 0, 5, 2);
    }

    #[test]
    fn zero_size_rects_do_not_panic() {
        let text = "hello\nworld";
        let layout = WrapLayout::new(text, 1);
        let spans = plain(text);
        let view = RowsView {
            text,
            layout: &layout,
            top_row: 0,
            spans: &spans,
            selection: 0..11,
            caret: 3,
        };
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 2));
        let caret = render(
            &view,
            Rect::new(0, 0, 0, 2),
            Rect::new(0, 0, 0, 2),
            &mut buf,
        );
        assert_eq!(caret, None);
        let caret = render(
            &view,
            Rect::new(0, 0, 1, 0),
            Rect::new(1, 0, 3, 0),
            &mut buf,
        );
        assert_eq!(caret, None);
        // Rects outside the buffer are clipped, not indexed.
        let _ = render(
            &view,
            Rect::new(10, 10, 4, 4),
            Rect::new(14, 10, 4, 4),
            &mut buf,
        );
    }

    #[test]
    fn nothing_drawn_outside_rects() {
        let text = "abcdefgh漢\nxy";
        let layout = WrapLayout::new(text, 40);
        let spans = plain(text);
        let view = RowsView {
            text,
            layout: &layout,
            top_row: 0,
            spans: &spans,
            selection: 0..20,
            caret: 0,
        };
        let mut buf = Buffer::empty(Rect::new(0, 0, 12, 3));
        let marker = Style::new().bg(Color::Red);
        buf.set_style(buf.area, marker);
        render(
            &view,
            Rect::new(0, 0, 3, 1),
            Rect::new(3, 0, 5, 1),
            &mut buf,
        );
        for x in 8..12 {
            assert_eq!(bg(&buf, x, 0), Some(Color::Red), "x {x}");
        }
        for x in 0..12 {
            assert_eq!(bg(&buf, x, 1), Some(Color::Red), "x {x}");
        }
        assert_eq!(row_text(&buf, 0), " 1 abcde    ");
    }
}
