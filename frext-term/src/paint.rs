//! Small painting helpers shared by the widgets that draw into a ratatui
//! [`Buffer`].

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

/// Blanks every cell of `area` and gives it `style`.
///
/// `area` is first cut to the buffer, so any part of it outside the
/// buffer is ignored.
pub fn fill(buf: &mut Buffer, area: Rect, style: Style) {
    let area = area.intersection(buf.area);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.reset();
                cell.set_style(style);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use ratatui::style::Color;

    use super::*;

    fn red() -> Style {
        Style::new().bg(Color::Red)
    }

    #[test]
    fn fills_the_cells_inside_the_area() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 3));
        buf.set_string(0, 0, "abcd", Style::new().fg(Color::Blue));
        fill(&mut buf, Rect::new(1, 0, 2, 2), red());
        for y in 0..3 {
            for x in 0..4 {
                let inside = (1..3).contains(&x) && y < 2;
                let cell = &buf[(x, y)];
                if inside {
                    assert_eq!(cell.symbol(), " ", "({x}, {y})");
                    assert_eq!(cell.bg, Color::Red, "({x}, {y})");
                    assert_eq!(cell.fg, Color::Reset, "({x}, {y})");
                } else {
                    assert_eq!(cell.bg, Color::Reset, "({x}, {y})");
                }
            }
        }
        assert_eq!(buf[(0, 0)].symbol(), "a");
        assert_eq!(buf[(3, 0)].symbol(), "d");
    }

    #[test]
    fn clamps_an_area_larger_than_the_buffer() {
        let area = Rect::new(2, 1, 3, 2);
        let mut buf = Buffer::empty(area);
        fill(&mut buf, Rect::new(0, 0, 40, 40), red());
        for position in area.positions() {
            assert_eq!(buf[position].bg, Color::Red, "{position:?}");
        }
    }

    #[test]
    fn an_area_outside_the_buffer_changes_nothing() {
        let area = Rect::new(0, 0, 3, 2);
        let mut buf = Buffer::empty(area);
        fill(&mut buf, Rect::new(10, 10, 5, 5), red());
        fill(&mut buf, Rect::new(u16::MAX - 1, u16::MAX - 1, 1, 1), red());
        assert_eq!(buf, Buffer::empty(area));
    }
}
