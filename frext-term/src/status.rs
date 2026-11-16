//! The status line at the bottom of the screen.

use std::time::{Duration, Instant};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Span;

use crate::theme;

/// How long a status message stays on screen.
pub const MESSAGE_TTL: Duration = Duration::from_secs(4);

/// The kind of a status message, which decides its colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageKind {
    /// An ordinary message.
    Info,
    /// An error.
    Error,
}

/// The status line: a transient message, a hover hint, and a right-hand
/// summary.
#[derive(Debug, Default)]
pub struct StatusLine {
    message: Option<(String, MessageKind, Instant)>,
    hint: Option<&'static str>,
}

impl StatusLine {
    /// An empty status line.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Shows `text` as a message set at `now`, replacing any previous
    /// message.
    pub fn set_message(&mut self, text: impl Into<String>, kind: MessageKind, now: Instant) {
        self.message = Some((text.into(), kind, now));
    }

    /// Sets (or clears) the hover hint.
    pub fn set_hint(&mut self, hint: Option<&'static str>) {
        self.hint = hint;
    }

    /// Drops the message once it is at least [`MESSAGE_TTL`] old.
    /// Returns whether anything changed.
    pub fn expire(&mut self, now: Instant) -> bool {
        let expired = self
            .message
            .as_ref()
            .is_some_and(|(_, _, set_at)| now.duration_since(*set_at) >= MESSAGE_TTL);
        if expired {
            self.message = None;
        }
        expired
    }

    /// The current message and its kind, if any.
    #[must_use]
    pub fn message(&self) -> Option<(&str, MessageKind)> {
        self.message
            .as_ref()
            .map(|(text, kind, _)| (text.as_str(), *kind))
    }

    /// The current hover hint, if any.
    #[must_use]
    pub const fn hint(&self) -> Option<&'static str> {
        self.hint
    }

    /// Draws the status line into the first row of `area`.
    ///
    /// The left side shows the message, else the hint, else
    /// `left_default`, starting one column in and truncated to fit. The
    /// right side shows `right`, ending one column before the right
    /// edge, and is omitted when it would not fit beside the left side.
    pub fn render(&self, area: Rect, buf: &mut Buffer, left_default: &str, right: &str) {
        let area = area.intersection(buf.area);
        if area.is_empty() {
            return;
        }
        let row = Rect::new(area.x, area.y, area.width, 1);
        buf.set_style(row, theme::status());

        let (left, left_style) = match (&self.message, self.hint) {
            (Some((text, MessageKind::Error, _)), _) => (text.as_str(), theme::status_error()),
            (Some((text, MessageKind::Info, _)), _) => (text.as_str(), theme::status()),
            (None, Some(hint)) => (hint, theme::status()),
            (None, None) => (left_default, theme::status()),
        };

        let max_left = usize::from(area.width.saturating_sub(2));
        if max_left > 0 {
            buf.set_stringn(area.x + 1, area.y, left, max_left, left_style);
        }

        let left_width = Span::raw(left).width();
        let right_width = Span::raw(right).width();
        if right_width == 0 || left_width + right_width + 3 > usize::from(area.width) {
            return;
        }
        let Ok(right_width) = u16::try_from(right_width) else {
            return;
        };
        let x = area.right() - 1 - right_width;
        buf.set_string(x, area.y, right, theme::status());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row_text(buf: &Buffer) -> String {
        buf.content().iter().map(|cell| cell.symbol()).collect()
    }

    #[test]
    fn new_is_empty() {
        let status = StatusLine::new();
        assert_eq!(status.message(), None);
        assert_eq!(status.hint(), None);
    }

    #[test]
    fn priority_message_then_hint_then_default() {
        let now = Instant::now();
        let area = Rect::new(0, 0, 20, 1);
        let mut status = StatusLine::new();

        let mut buf = Buffer::empty(area);
        status.render(area, &mut buf, "default", "");
        assert_eq!(row_text(&buf), " default            ");

        status.set_hint(Some("hint"));
        assert_eq!(status.hint(), Some("hint"));
        let mut buf = Buffer::empty(area);
        status.render(area, &mut buf, "default", "");
        assert_eq!(row_text(&buf), " hint               ");

        status.set_message("msg", MessageKind::Info, now);
        assert_eq!(status.message(), Some(("msg", MessageKind::Info)));
        let mut buf = Buffer::empty(area);
        status.render(area, &mut buf, "default", "");
        assert_eq!(row_text(&buf), " msg                ");

        status.set_hint(None);
        assert_eq!(status.hint(), None);
    }

    #[test]
    fn expiry_boundary() {
        let start = Instant::now();
        let mut status = StatusLine::new();
        assert!(!status.expire(start));

        status.set_message("saved", MessageKind::Info, start);
        let almost = start + MESSAGE_TTL - Duration::from_millis(1);
        assert!(!status.expire(almost));
        assert_eq!(status.message(), Some(("saved", MessageKind::Info)));

        assert!(status.expire(start + MESSAGE_TTL));
        assert_eq!(status.message(), None);
        assert!(!status.expire(start + MESSAGE_TTL));
    }

    #[test]
    fn renders_error_message_with_right_side() {
        let area = Rect::new(0, 0, 20, 1);
        let mut buf = Buffer::empty(area);
        let mut status = StatusLine::new();
        status.set_message("Oops", MessageKind::Error, Instant::now());
        status.render(area, &mut buf, "default", "Ln 1");

        let mut expected = Buffer::with_lines([" Oops          Ln 1 "]);
        expected.set_style(area, theme::status());
        expected.set_style(Rect::new(1, 0, 4, 1), theme::status_error());
        assert_eq!(buf, expected);
    }

    #[test]
    fn truncates_left_and_drops_right() {
        let area = Rect::new(0, 0, 10, 1);
        let mut buf = Buffer::empty(area);
        let status = StatusLine::new();
        status.render(area, &mut buf, "a long default text", "Ln 1");

        let mut expected = Buffer::with_lines([" a long d "]);
        expected.set_style(area, theme::status());
        assert_eq!(buf, expected);
    }

    #[test]
    fn right_dropped_exactly_when_it_would_overlap() {
        // left 3 + right 4 + 3 = 10 fits in 10 columns.
        let area = Rect::new(0, 0, 10, 1);
        let mut buf = Buffer::empty(area);
        StatusLine::new().render(area, &mut buf, "abc", "Ln 1");
        assert_eq!(row_text(&buf), " abc Ln 1 ");

        let area = Rect::new(0, 0, 9, 1);
        let mut buf = Buffer::empty(area);
        StatusLine::new().render(area, &mut buf, "abc", "Ln 1");
        assert_eq!(row_text(&buf), " abc     ");
    }

    #[test]
    fn tiny_areas_do_not_panic() {
        for width in 0..4 {
            let area = Rect::new(0, 0, width, 1);
            let mut buf = Buffer::empty(area);
            StatusLine::new().render(area, &mut buf, "default", "right");
        }
        let mut buf = Buffer::empty(Rect::new(0, 0, 0, 0));
        StatusLine::new().render(Rect::new(0, 0, 0, 0), &mut buf, "x", "y");
    }
}
