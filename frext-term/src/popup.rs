//! The shared frame of a prompt popup.
//!
//! A popup is a bordered box centred over the screen ([`centered`],
//! [`render_frame`]) with a right-aligned row of buttons
//! ([`render_buttons`]). [`wrap`] word-wraps a message to fit inside it.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::{Block, Clear, Widget};
use unicode_width::UnicodeWidthStr;

use crate::hit::{HitMap, Target};
use crate::theme;

/// A `width` x `height` rect centred in `area`, clamped to fit inside it.
#[must_use]
pub fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

/// Clears `area`, draws a bordered popup titled `title` over it, records
/// `area` as [`Target::PromptPopup`] and returns the area inside the
/// border.
pub fn render_frame(title: &str, area: Rect, buf: &mut Buffer, hits: &mut HitMap) -> Rect {
    let area = area.intersection(buf.area);
    Clear.render(area, buf);
    let block = Block::bordered()
        .title(title)
        .style(theme::popup())
        .border_style(theme::popup_border());
    let inner = block.inner(area);
    block.render(area, buf);
    hits.push(area, Target::PromptPopup);
    inner
}

/// The display width of a button drawn for `label`.
fn button_width(label: &str) -> usize {
    label.width() + 4
}

/// Draws `labels` as `[ label ]` buttons, right-aligned in the first row
/// of `row` and separated by one space, recording each as
/// [`Target::PromptButton`] with its index. The button at `focused` is
/// highlighted; an out-of-range `focused` highlights none.
pub fn render_buttons(
    labels: &[&str],
    focused: usize,
    row: Rect,
    buf: &mut Buffer,
    hits: &mut HitMap,
) {
    let row = row.intersection(buf.area);
    if row.is_empty() {
        return;
    }
    let total: usize = labels
        .iter()
        .map(|label| button_width(label))
        .sum::<usize>()
        + labels.len().saturating_sub(1);
    let width = usize::from(row.width);
    let mut x = width.saturating_sub(total);
    for (index, label) in labels.iter().enumerate() {
        if x >= width {
            break;
        }
        let room = width - x;
        let style = if index == focused {
            theme::button_focused()
        } else {
            theme::popup()
        };
        let drawn = button_width(label).min(room);
        let (Ok(col), Ok(drawn_u16)) = (u16::try_from(x), u16::try_from(drawn)) else {
            break;
        };
        buf.set_stringn(row.x + col, row.y, format!("[ {label} ]"), room, style);
        hits.push(
            Rect::new(row.x + col, row.y, drawn_u16, 1),
            Target::PromptButton(index),
        );
        x += button_width(label) + 1;
    }
}

/// Splits `text` into lines of at most `width` display columns, breaking
/// at whitespace. A word wider than `width` is split across lines.
#[must_use]
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    if width == 0 {
        return lines;
    }
    let mut line = String::new();
    let mut line_width = 0;
    for word in text.split_whitespace() {
        let word_width = word.width();
        if !line.is_empty() && line_width + 1 + word_width > width {
            lines.push(std::mem::take(&mut line));
            line_width = 0;
        }
        if word_width > width {
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
                line_width = 0;
            }
            for c in word.chars() {
                let c_width = c.to_string().width();
                if line_width + c_width > width && !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                    line_width = 0;
                }
                line.push(c);
                line_width += c_width;
            }
            continue;
        }
        if !line.is_empty() {
            line.push(' ');
            line_width += 1;
        }
        line.push_str(word);
        line_width += word_width;
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row_text(buf: &Buffer, y: u16) -> String {
        (buf.area.x..buf.area.right())
            .map(|x| buf[(x, y)].symbol())
            .collect()
    }

    #[test]
    fn centered_in_area() {
        let area = Rect::new(2, 1, 80, 24);
        assert_eq!(centered(area, 60, 6), Rect::new(12, 10, 60, 6));
    }

    #[test]
    fn centered_clamps_to_area() {
        let area = Rect::new(5, 5, 20, 4);
        assert_eq!(centered(area, 60, 10), area);
        assert_eq!(centered(area, 10, 10), Rect::new(10, 5, 10, 4));
    }

    #[test]
    fn frame_draws_border_title_and_records_popup() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 5));
        buf.set_string(0, 0, "xxxxxxxxxxxxxxxxxxxx", theme::base());
        let mut hits = HitMap::default();
        let area = Rect::new(2, 1, 12, 4);
        let inner = render_frame("Title", area, &mut buf, &mut hits);
        assert_eq!(inner, Rect::new(3, 2, 10, 2));
        assert!(row_text(&buf, 1).contains("Title"));
        assert_eq!(buf[(2, 1)].symbol(), "\u{250c}");
        assert_eq!(buf[(2, 1)].style().fg, theme::popup_border().fg);
        assert_eq!(buf[(5, 2)].style().bg, theme::popup().bg);
        assert_eq!(buf[(5, 2)].symbol(), " ");
        assert_eq!(hits.hit(3, 2), Some(Target::PromptPopup));
        assert_eq!(hits.hit(2, 1), Some(Target::PromptPopup));
        assert_eq!(hits.hit(1, 1), None);
    }

    #[test]
    fn buttons_are_right_aligned_and_hit_tested() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 1));
        let mut hits = HitMap::default();
        render_buttons(&["Ok", "No"], 1, buf.area, &mut buf, &mut hits);
        // "[ Ok ] [ No ]" is 13 wide, so it starts at column 7.
        assert_eq!(row_text(&buf, 0), "       [ Ok ] [ No ]");
        assert_eq!(hits.hit(7, 0), Some(Target::PromptButton(0)));
        assert_eq!(hits.hit(12, 0), Some(Target::PromptButton(0)));
        assert_eq!(hits.hit(13, 0), None);
        assert_eq!(hits.hit(14, 0), Some(Target::PromptButton(1)));
        assert_eq!(hits.hit(19, 0), Some(Target::PromptButton(1)));
        assert_eq!(buf[(14, 0)].style().bg, theme::button_focused().bg);
        assert_eq!(buf[(7, 0)].style().bg, theme::popup().bg);
    }

    #[test]
    fn out_of_range_focus_highlights_none() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 1));
        let mut hits = HitMap::default();
        render_buttons(&["Ok"], 5, buf.area, &mut buf, &mut hits);
        assert_eq!(buf[(9, 0)].style().bg, theme::popup().bg);
    }

    #[test]
    fn buttons_clip_in_a_narrow_row() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 8, 1));
        let mut hits = HitMap::default();
        render_buttons(&["Save", "Cancel"], 0, buf.area, &mut buf, &mut hits);
        assert_eq!(row_text(&buf, 0), "[ Save ]");
        assert_eq!(hits.hit(0, 0), Some(Target::PromptButton(0)));
        assert_eq!(hits.hit(7, 0), Some(Target::PromptButton(0)));
    }

    #[test]
    fn wrap_breaks_at_whitespace() {
        assert_eq!(
            wrap("the quick brown fox", 10),
            vec!["the quick", "brown fox"]
        );
        assert_eq!(wrap("a  b\nc", 80), vec!["a b c"]);
        assert_eq!(wrap("exactly10!", 10), vec!["exactly10!"]);
    }

    #[test]
    fn wrap_splits_long_words() {
        assert_eq!(wrap("ab abcdefgh", 4), vec!["ab", "abcd", "efgh"]);
        assert_eq!(wrap("abcdef x", 4), vec!["abcd", "ef x"]);
    }

    #[test]
    fn wrap_uses_display_width() {
        assert_eq!(wrap("日本 語", 4), vec!["日本", "語"]);
        assert_eq!(wrap("日本語", 4), vec!["日本", "語"]);
    }

    #[test]
    fn wrap_empty_and_zero_width() {
        assert!(wrap("", 10).is_empty());
        assert!(wrap("   ", 10).is_empty());
        assert!(wrap("text", 0).is_empty());
    }
}
