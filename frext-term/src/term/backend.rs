//! [`FrextBackend`]: a crossterm backend that can draw undercurl.
//!
//! ratatui's `Modifier` has no undercurl, and the stock
//! `CrosstermBackend` only ever draws a straight underline. This backend
//! is a copy of ratatui-crossterm 0.1.2's `CrosstermBackend` (MIT
//! licensed, <https://github.com/ratatui/ratatui>): its draw loop, its
//! other `Backend` methods and its private `ModifierDiff` are copied
//! as-is, with one rule added. A cell with `Modifier::UNDERLINED` and a
//! non-`Reset` underline colour is drawn with a curly underline
//! (`CSI 4:3 m`) when the backend's [`UnderlineStyle`] is
//! [`UnderlineStyle::Curly`]; every other underline is straight. A frame
//! with no underline colour produces exactly the stock bytes.
//!
//! The upstream copyright and licence notice are kept in
//! `ATTRIBUTIONS.md` and `assets/licenses/ratatui-LICENSE` at the
//! repository root.

use std::io::{self, Write};

use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::style::{
    Attribute as CrosstermAttribute, Color as CrosstermColor, Colors as CrosstermColors, Print,
    SetAttribute, SetBackgroundColor, SetColors, SetForegroundColor, SetUnderlineColor,
};
use crossterm::terminal::{self, Clear};
use crossterm::{execute, queue};
use ratatui::backend::{Backend, ClearType, IntoCrossterm, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};
use ratatui::style::{Color, Modifier};

use super::caps::UnderlineStyle;

/// A ratatui [`Backend`] drawing through crossterm, with undercurl.
///
/// Identical to ratatui's `CrosstermBackend` except that coloured
/// underlines are drawn curly when `underline` is
/// [`UnderlineStyle::Curly`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrextBackend<W: Write> {
    /// The writer used to send commands to the terminal.
    writer: W,
    /// How a coloured underline is drawn.
    underline: UnderlineStyle,
}

impl<W> FrextBackend<W>
where
    W: Write,
{
    /// Creates a backend writing to `writer`, drawing coloured
    /// underlines in the given style.
    pub const fn new(writer: W, underline: UnderlineStyle) -> Self {
        Self { writer, underline }
    }

    /// How this backend draws a coloured underline.
    pub const fn underline(&self) -> UnderlineStyle {
        self.underline
    }

    /// Gets the writer.
    pub const fn writer(&self) -> &W {
        &self.writer
    }

    /// Gets the writer as a mutable reference.
    ///
    /// Writing to it directly can desynchronise the output from the
    /// terminal's buffer diffing.
    pub const fn writer_mut(&mut self) -> &mut W {
        &mut self.writer
    }
}

impl<W> Write for FrextBackend<W>
where
    W: Write,
{
    /// Writes a buffer of bytes to the underlying buffer.
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.writer.write(buf)
    }

    /// Flushes the underlying buffer.
    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

impl<W> Backend for FrextBackend<W>
where
    W: Write,
{
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        let mut fg = Color::Reset;
        let mut bg = Color::Reset;
        let mut underline_color = Color::Reset;
        let mut modifier = Modifier::empty();
        let mut underline = UnderlineDraw::Off;
        let mut last_pos: Option<Position> = None;
        for (x, y, cell) in content {
            // Move the cursor if the previous location was not (x - 1, y)
            if !matches!(last_pos, Some(p) if x == p.x + 1 && y == p.y) {
                queue!(self.writer, MoveTo(x, y))?;
            }
            last_pos = Some(Position { x, y });
            let cell_underline = UnderlineDraw::of(cell, self.underline);
            if cell.modifier != modifier || cell_underline != underline {
                let diff = ModifierDiff {
                    from: modifier,
                    to: cell.modifier,
                    from_underline: underline,
                    to_underline: cell_underline,
                };
                diff.queue(&mut self.writer)?;
                modifier = cell.modifier;
                underline = cell_underline;
            }
            if cell.fg != fg || cell.bg != bg {
                queue!(
                    self.writer,
                    SetColors(CrosstermColors::new(
                        cell.fg.into_crossterm(),
                        cell.bg.into_crossterm(),
                    ))
                )?;
                fg = cell.fg;
                bg = cell.bg;
            }
            if cell.underline_color != underline_color {
                let color = cell.underline_color.into_crossterm();
                queue!(self.writer, SetUnderlineColor(color))?;
                underline_color = cell.underline_color;
            }

            queue!(self.writer, Print(cell.symbol()))?;
        }

        queue!(
            self.writer,
            SetForegroundColor(CrosstermColor::Reset),
            SetBackgroundColor(CrosstermColor::Reset),
            SetUnderlineColor(CrosstermColor::Reset),
            SetAttribute(CrosstermAttribute::Reset),
        )
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        execute!(self.writer, Hide)
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        execute!(self.writer, Show)
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        crossterm::cursor::position()
            .map(|(x, y)| Position { x, y })
            .map_err(io::Error::other)
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        let Position { x, y } = position.into();
        execute!(self.writer, MoveTo(x, y))
    }

    fn clear(&mut self) -> io::Result<()> {
        self.clear_region(ClearType::All)
    }

    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        execute!(
            self.writer,
            Clear(match clear_type {
                ClearType::All => crossterm::terminal::ClearType::All,
                ClearType::AfterCursor => crossterm::terminal::ClearType::FromCursorDown,
                ClearType::BeforeCursor => crossterm::terminal::ClearType::FromCursorUp,
                ClearType::CurrentLine => crossterm::terminal::ClearType::CurrentLine,
                ClearType::UntilNewLine => crossterm::terminal::ClearType::UntilNewLine,
            })
        )
    }

    fn append_lines(&mut self, n: u16) -> io::Result<()> {
        for _ in 0..n {
            queue!(self.writer, Print("\n"))?;
        }
        self.writer.flush()
    }

    fn size(&self) -> io::Result<Size> {
        let (width, height) = terminal::size()?;
        Ok(Size { width, height })
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        let crossterm::terminal::WindowSize {
            columns,
            rows,
            width,
            height,
        } = terminal::window_size()?;
        Ok(WindowSize {
            columns_rows: Size {
                width: columns,
                height: rows,
            },
            pixels: Size { width, height },
        })
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

/// How a cell's underline is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnderlineDraw {
    /// No underline.
    Off,
    /// A straight underline (`CSI 4 m`).
    Straight,
    /// A curly underline (`CSI 4:3 m`).
    Curly,
}

impl UnderlineDraw {
    /// The underline `cell` is drawn with by a backend whose coloured
    /// underlines use `style`.
    fn of(cell: &Cell, style: UnderlineStyle) -> Self {
        if !cell.modifier.contains(Modifier::UNDERLINED) {
            Self::Off
        } else if cell.underline_color != Color::Reset && style == UnderlineStyle::Curly {
            Self::Curly
        } else {
            Self::Straight
        }
    }
}

/// The `ModifierDiff` struct is used to calculate the difference between two `Modifier`
/// values. This is useful when updating the terminal display, as it allows for more
/// efficient updates by only sending the necessary changes.
///
/// `UNDERLINED` is handled only through `from_underline` / `to_underline`.
struct ModifierDiff {
    from: Modifier,
    to: Modifier,
    from_underline: UnderlineDraw,
    to_underline: UnderlineDraw,
}

impl ModifierDiff {
    fn queue<W>(self, mut w: W) -> io::Result<()>
    where
        W: io::Write,
    {
        let removed = self.from - self.to;
        if removed.contains(Modifier::REVERSED) {
            queue!(w, SetAttribute(CrosstermAttribute::NoReverse))?;
        }

        let reset_intensity = removed.contains(Modifier::BOLD) || removed.contains(Modifier::DIM);
        if reset_intensity {
            // Bold and Dim are both reset by applying the Normal intensity
            queue!(w, SetAttribute(CrosstermAttribute::NormalIntensity))?;

            // The remaining Bold and Dim attributes must be
            // reapplied after the intensity reset above.
            if self.to.contains(Modifier::DIM) {
                queue!(w, SetAttribute(CrosstermAttribute::Dim))?;
            }

            if self.to.contains(Modifier::BOLD) {
                queue!(w, SetAttribute(CrosstermAttribute::Bold))?;
            }
        }

        if removed.contains(Modifier::ITALIC) {
            queue!(w, SetAttribute(CrosstermAttribute::NoItalic))?;
        }
        if self.to_underline == UnderlineDraw::Off && self.from_underline != UnderlineDraw::Off {
            queue!(w, SetAttribute(CrosstermAttribute::NoUnderline))?;
        }
        if removed.contains(Modifier::CROSSED_OUT) {
            queue!(w, SetAttribute(CrosstermAttribute::NotCrossedOut))?;
        }
        if removed.contains(Modifier::HIDDEN) {
            queue!(w, SetAttribute(CrosstermAttribute::NoHidden))?;
        }
        if removed.contains(Modifier::SLOW_BLINK) || removed.contains(Modifier::RAPID_BLINK) {
            queue!(w, SetAttribute(CrosstermAttribute::NoBlink))?;
        }

        let added = self.to - self.from;
        if added.contains(Modifier::REVERSED) {
            queue!(w, SetAttribute(CrosstermAttribute::Reverse))?;
        }
        if added.contains(Modifier::BOLD) && !reset_intensity {
            queue!(w, SetAttribute(CrosstermAttribute::Bold))?;
        }
        if added.contains(Modifier::ITALIC) {
            queue!(w, SetAttribute(CrosstermAttribute::Italic))?;
        }
        if self.to_underline != self.from_underline {
            match self.to_underline {
                UnderlineDraw::Off => {}
                UnderlineDraw::Straight => {
                    queue!(w, SetAttribute(CrosstermAttribute::Underlined))?;
                }
                UnderlineDraw::Curly => {
                    queue!(w, SetAttribute(CrosstermAttribute::Undercurled))?;
                }
            }
        }
        if added.contains(Modifier::DIM) && !reset_intensity {
            queue!(w, SetAttribute(CrosstermAttribute::Dim))?;
        }
        if added.contains(Modifier::CROSSED_OUT) {
            queue!(w, SetAttribute(CrosstermAttribute::CrossedOut))?;
        }
        if added.contains(Modifier::HIDDEN) {
            queue!(w, SetAttribute(CrosstermAttribute::Hidden))?;
        }
        if added.contains(Modifier::SLOW_BLINK) {
            queue!(w, SetAttribute(CrosstermAttribute::SlowBlink))?;
        }
        if added.contains(Modifier::RAPID_BLINK) {
            queue!(w, SetAttribute(CrosstermAttribute::RapidBlink))?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    // `unwrap` is acceptable in test code: a panic on an unexpected `Err`
    // is exactly the failure signal we want from a test.
    #![allow(clippy::unwrap_used)]

    use ratatui::backend::CrosstermBackend;
    use ratatui::style::Style;

    use super::*;

    /// The proofreading underline colour used by the tests.
    const RED: Color = Color::Rgb(243, 139, 168);

    /// Turns colour output on even when `NO_COLOR` is set.
    fn enable_colour() {
        crossterm::style::Colored::set_ansi_color_disabled(false);
    }

    /// A cell holding `symbol` drawn in `style`.
    fn cell(symbol: &'static str, style: Style) -> Cell {
        let mut cell = Cell::new(symbol);
        cell.set_style(style);
        cell
    }

    /// The bytes `FrextBackend` writes drawing `cells`.
    fn draw_frext(cells: &[(u16, u16, Cell)], underline: UnderlineStyle) -> String {
        let mut backend = FrextBackend::new(Vec::new(), underline);
        backend
            .draw(cells.iter().map(|(x, y, c)| (*x, *y, c)))
            .unwrap();
        String::from_utf8(backend.writer().clone()).unwrap()
    }

    /// The bytes ratatui's stock `CrosstermBackend` writes drawing `cells`.
    fn draw_stock(cells: &[(u16, u16, Cell)]) -> String {
        let mut bytes = Vec::new();
        CrosstermBackend::new(&mut bytes)
            .draw(cells.iter().map(|(x, y, c)| (*x, *y, c)))
            .unwrap();
        String::from_utf8(bytes).unwrap()
    }

    /// The text between the first `a` and the first `b` in `out`.
    fn between_a_and_b(out: &str) -> &str {
        let start = out.find('a').unwrap() + 1;
        let end = out.find('b').unwrap();
        out.get(start..end).unwrap()
    }

    fn underlined() -> Style {
        Style::new().add_modifier(Modifier::UNDERLINED)
    }

    fn curly() -> Style {
        underlined().underline_color(RED)
    }

    #[test]
    fn matches_stock_backend_without_underline_colour() {
        enable_colour();
        let row = [
            Style::new().add_modifier(Modifier::BOLD),
            Style::new().add_modifier(Modifier::DIM),
            Style::new().add_modifier(Modifier::ITALIC),
            Style::new().add_modifier(Modifier::REVERSED),
            Style::new().fg(Color::Red).bg(Color::Rgb(1, 2, 3)),
            Style::new().add_modifier(Modifier::BOLD),
            Style::new().add_modifier(Modifier::DIM),
            underlined(),
            Style::new(),
        ];
        let cells: Vec<(u16, u16, Cell)> = row
            .iter()
            .zip(0u16..)
            .map(|(style, x)| (x, 0, cell("x", *style)))
            .collect();
        let stock = draw_stock(&cells);
        assert_eq!(draw_frext(&cells, UnderlineStyle::Curly), stock);
        assert_eq!(draw_frext(&cells, UnderlineStyle::Straight), stock);
    }

    #[test]
    fn coloured_underline_under_curly_is_undercurl() {
        enable_colour();
        let cells = [(0, 0, cell("a", curly()))];
        assert_eq!(
            draw_frext(&cells, UnderlineStyle::Curly),
            "\x1b[1;1H\x1b[4:3m\x1b[58;2;243;139;168ma\x1b[39m\x1b[49m\x1b[59m\x1b[0m"
        );
    }

    #[test]
    fn coloured_underline_under_straight_is_straight() {
        enable_colour();
        let cells = [(0, 0, cell("a", curly()))];
        assert_eq!(
            draw_frext(&cells, UnderlineStyle::Straight),
            "\x1b[1;1H\x1b[4m\x1b[58;2;243;139;168ma\x1b[39m\x1b[49m\x1b[59m\x1b[0m"
        );
    }

    #[test]
    fn underline_without_colour_stays_straight_under_curly() {
        enable_colour();
        let cells = [(0, 0, cell("a", underlined()))];
        let out = draw_frext(&cells, UnderlineStyle::Curly);
        assert!(out.contains("\x1b[4m"));
        assert!(!out.contains("4:3"));
    }

    #[test]
    fn underline_colour_without_underline_draws_no_underline() {
        enable_colour();
        let cells = [(0, 0, cell("a", Style::new().underline_color(RED)))];
        let out = draw_frext(&cells, UnderlineStyle::Curly);
        assert!(out.contains("\x1b[58;2;243;139;168m"));
        assert!(!out.contains("\x1b[4m"));
        assert!(!out.contains("4:3"));
    }

    #[test]
    fn curly_then_straight_switches_to_straight() {
        enable_colour();
        let cells = [(0, 0, cell("a", curly())), (1, 0, cell("b", underlined()))];
        let out = draw_frext(&cells, UnderlineStyle::Curly);
        let between = between_a_and_b(&out);
        assert!(between.contains("\x1b[4m"));
        assert!(!between.contains("\x1b[24m"));
    }

    #[test]
    fn curly_then_plain_turns_underline_off() {
        enable_colour();
        let cells = [(0, 0, cell("a", curly())), (1, 0, cell("b", Style::new()))];
        let out = draw_frext(&cells, UnderlineStyle::Curly);
        let between = between_a_and_b(&out);
        let off = between.find("\x1b[24m").unwrap();
        let reset = between.find("\x1b[59m").unwrap();
        assert!(off < reset);
    }

    #[test]
    fn straight_then_curly_switches_to_curly() {
        enable_colour();
        let cells = [(0, 0, cell("a", underlined())), (1, 0, cell("b", curly()))];
        let out = draw_frext(&cells, UnderlineStyle::Curly);
        assert!(between_a_and_b(&out).contains("\x1b[4:3m"));
    }

    #[test]
    fn non_adjacent_cells_move_the_cursor() {
        enable_colour();
        let cells = [
            (0, 0, cell("a", Style::new())),
            (3, 2, cell("b", Style::new())),
        ];
        let out = draw_frext(&cells, UnderlineStyle::Curly);
        assert_eq!(between_a_and_b(&out), "\x1b[3;4H");
        assert_eq!(out, draw_stock(&cells));
    }

    #[test]
    fn exposes_underline_style_and_writer() {
        enable_colour();
        let mut backend = FrextBackend::new(Vec::new(), UnderlineStyle::Curly);
        assert_eq!(backend.underline(), UnderlineStyle::Curly);
        assert_eq!(
            FrextBackend::new(Vec::<u8>::new(), UnderlineStyle::Straight).underline(),
            UnderlineStyle::Straight
        );
        backend.write_all(b"hello").unwrap();
        Write::flush(&mut backend).unwrap();
        assert_eq!(backend.writer(), b"hello");
        backend.writer_mut().clear();
        assert!(backend.writer().is_empty());
    }
}
