//! Keeps late probe replies out of the editor.
//!
//! A reply that arrives after the probe gave up reaches crossterm's
//! reader, which turns a DCS / APC / OSC reply into key events: `Alt+P`,
//! `Alt+_` or `Alt+]`, then one printable press per payload byte, then
//! `Alt+\` (or `Ctrl+G` for a BEL terminator), all back to back. Late CSI
//! replies are never delivered as events, so they need nothing.
//! [`LateReplyFilter`] recognises such a burst for a short while after
//! startup and drops it, and [`filtered`] wraps an event reader with it.

use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::term::caps::Multiplexer;
use crate::term::probe::ReplyEnd;

/// How long after startup a late reply is still looked for.
pub const LATE_REPLY_WINDOW: Duration = Duration::from_secs(5);
/// The longest pause between two events of one reply; a longer pause is a
/// person typing, not a burst.
pub const REPLY_GAP: Duration = Duration::from_millis(50);
/// The most events one reply may span before the filter gives up on it.
pub const MAX_REPLY_EVENTS: usize = 512;

/// What to do with an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Hand the event to the editor.
    Pass,
    /// Discard the event: it is part of a late terminal reply.
    Drop,
}

/// Where the filter is in a possible late reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Not inside a reply.
    Idle,
    /// Inside a reply: the time of its last event and how many events it
    /// has spanned so far.
    InReply { last: Instant, events: usize },
}

/// When the filter may start a reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Arming {
    /// Never: no reply can come late.
    Never,
    /// For events before this time.
    Until(Instant),
    /// Always: the deadline overflowed.
    NoEnd,
}

/// Recognises the key events of a late probe reply and drops them.
#[derive(Debug, Clone)]
pub struct LateReplyFilter {
    /// When the filter may start a reply.
    arming: Arming,
    /// Where the filter is in a possible late reply.
    state: State,
}

impl LateReplyFilter {
    /// A filter for a probe that ended with `end` under `multiplexer`,
    /// created at `now`.
    ///
    /// It is armed until `now + LATE_REPLY_WINDOW` when the probe ended
    /// without DA1 or runs under tmux (the only cases where a reply can
    /// come late); an overflowing deadline means armed with no end.
    /// Otherwise it is never armed.
    #[must_use]
    pub fn new(end: ReplyEnd, multiplexer: Multiplexer, now: Instant) -> Self {
        let arming = if end == ReplyEnd::Incomplete || multiplexer == Multiplexer::Tmux {
            now.checked_add(LATE_REPLY_WINDOW)
                .map_or(Arming::NoEnd, Arming::Until)
        } else {
            Arming::Never
        };
        Self {
            arming,
            state: State::Idle,
        }
    }

    /// Judge `event`, which arrived at `now`, and update the filter.
    pub fn verdict(&mut self, event: &Event, now: Instant) -> Verdict {
        let press = key_press(event);
        if let State::InReply { last, events } = self.state {
            if now.saturating_duration_since(last) > REPLY_GAP || events >= MAX_REPLY_EVENTS {
                self.state = State::Idle;
                return self.idle_verdict(press, now);
            }
            if press.is_some_and(is_terminator) {
                self.state = State::Idle;
                log::info!(
                    "dropped a late terminal reply of {} events",
                    events.saturating_add(1)
                );
                return Verdict::Drop;
            }
            if press.is_some_and(is_payload) {
                self.state = State::InReply {
                    last: now,
                    events: events.saturating_add(1),
                };
                return Verdict::Drop;
            }
            self.state = State::Idle;
            return Verdict::Pass;
        }
        self.idle_verdict(press, now)
    }

    /// Judge a key press (`None` for any other event) while not inside a
    /// reply.
    fn idle_verdict(&mut self, press: Option<&KeyEvent>, now: Instant) -> Verdict {
        let armed = match self.arming {
            Arming::Never => false,
            Arming::Until(until) => now < until,
            Arming::NoEnd => true,
        };
        if armed && press.is_some_and(is_introducer) {
            self.state = State::InReply {
                last: now,
                events: 1,
            };
            Verdict::Drop
        } else {
            Verdict::Pass
        }
    }
}

/// The key event in `event` when it is a press.
fn key_press(event: &Event) -> Option<&KeyEvent> {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => Some(key),
        _ => None,
    }
}

/// `Alt+P`, `Alt+_` or `Alt+]`: how `ESC P`, `ESC _` and `ESC ]` arrive.
fn is_introducer(key: &KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::ALT) && matches!(key.code, KeyCode::Char('P' | '_' | ']'))
}

/// `Alt+\` (`ESC \`) or `Ctrl+G` (BEL).
fn is_terminator(key: &KeyEvent) -> bool {
    (key.modifiers.contains(KeyModifiers::ALT) && key.code == KeyCode::Char('\\'))
        || (key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char('g'))
}

/// A printable ASCII character with no modifiers, or only Shift.
fn is_payload(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char(' '..='~'))
        && (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT)
}

/// Wrap `read` so late probe replies are dropped: each call reads until
/// an event `filter` passes (judged at the time `now` returns) and
/// returns it. Errors from `read` are returned as they come.
pub fn filtered(
    mut read: impl FnMut() -> io::Result<Event>,
    mut filter: LateReplyFilter,
    now: impl Fn() -> Instant,
) -> impl FnMut() -> io::Result<Event> {
    move || {
        loop {
            let event = read()?;
            if filter.verdict(&event, now()) == Verdict::Pass {
                return Ok(event);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    // Tests may unwrap: a failure is a test failure.
    #![allow(clippy::unwrap_used)]

    use std::collections::VecDeque;

    use crossterm::event::{MouseEvent, MouseEventKind};

    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> Event {
        Event::Key(KeyEvent::new(code, modifiers))
    }

    fn alt(c: char) -> Event {
        let modifiers = if c.is_ascii_uppercase() {
            KeyModifiers::ALT | KeyModifiers::SHIFT
        } else {
            KeyModifiers::ALT
        };
        key(KeyCode::Char(c), modifiers)
    }

    fn plain(c: char) -> Event {
        let modifiers = if c.is_ascii_uppercase() {
            KeyModifiers::SHIFT
        } else {
            KeyModifiers::NONE
        };
        key(KeyCode::Char(c), modifiers)
    }

    fn ctrl_g() -> Event {
        key(KeyCode::Char('g'), KeyModifiers::CONTROL)
    }

    /// The events crossterm makes of `ESC <intro> <payload> <end>`.
    fn reply(intro: char, payload: &str, end: Event) -> Vec<Event> {
        let mut events = vec![alt(intro)];
        events.extend(payload.chars().map(plain));
        events.push(end);
        events
    }

    fn at(base: Instant, micros: u64) -> Instant {
        base.checked_add(Duration::from_micros(micros)).unwrap()
    }

    /// Judge `events` one microsecond apart, starting at `start`.
    fn judge(
        filter: &mut LateReplyFilter,
        events: &[Event],
        base: Instant,
        start: u64,
    ) -> Vec<Verdict> {
        events
            .iter()
            .zip(start..)
            .map(|(event, micros)| filter.verdict(event, at(base, micros)))
            .collect()
    }

    #[test]
    fn not_armed_passes_everything() {
        let base = Instant::now();
        let mut filter = LateReplyFilter::new(ReplyEnd::Da1, Multiplexer::None, base);
        let events = reply('P', ">|tmux 3.5a", alt('\\'));
        let verdicts = judge(&mut filter, &events, base, 1);
        assert!(verdicts.iter().all(|v| *v == Verdict::Pass));
    }

    #[test]
    fn not_armed_passes_alt_p_before_its_creation() {
        let base = Instant::now();
        let Some(earlier) = base.checked_sub(Duration::from_millis(1)) else {
            // The clock cannot go back from here: nothing to check.
            return;
        };
        let mut filter = LateReplyFilter::new(ReplyEnd::Da1, Multiplexer::None, base);
        assert_eq!(filter.verdict(&alt('P'), earlier), Verdict::Pass);
    }

    #[test]
    fn armed_incomplete_drops_a_dcs_reply_then_passes_typing() {
        let base = Instant::now();
        let mut filter = LateReplyFilter::new(ReplyEnd::Incomplete, Multiplexer::None, base);
        let events = reply('P', ">|tmux 3.5a", alt('\\'));
        let verdicts = judge(&mut filter, &events, base, 1);
        assert_eq!(verdicts.len(), events.len());
        assert!(verdicts.iter().all(|v| *v == Verdict::Drop));
        assert_eq!(filter.verdict(&plain('x'), at(base, 1_000)), Verdict::Pass);
    }

    #[test]
    fn armed_under_tmux_drops_a_kitty_reply() {
        let base = Instant::now();
        let mut filter = LateReplyFilter::new(ReplyEnd::Da1, Multiplexer::Tmux, base);
        let events = reply('_', "Gi=31;OK", alt('\\'));
        let verdicts = judge(&mut filter, &events, base, 1);
        assert!(verdicts.iter().all(|v| *v == Verdict::Drop));
    }

    #[test]
    fn a_bel_terminated_reply_is_dropped() {
        let base = Instant::now();
        let mut filter = LateReplyFilter::new(ReplyEnd::Incomplete, Multiplexer::None, base);
        let events = reply(']', "11;rgb:1e1e/1e1e/2e2e", ctrl_g());
        let verdicts = judge(&mut filter, &events, base, 1);
        assert!(verdicts.iter().all(|v| *v == Verdict::Drop));
        assert_eq!(filter.verdict(&plain('x'), at(base, 100)), Verdict::Pass);
    }

    #[test]
    fn after_the_window_alt_p_passes() {
        let base = Instant::now();
        let mut filter = LateReplyFilter::new(ReplyEnd::Incomplete, Multiplexer::None, base);
        let later = base.checked_add(LATE_REPLY_WINDOW).unwrap();
        assert_eq!(filter.verdict(&alt('P'), later), Verdict::Pass);
        assert_eq!(filter.verdict(&plain('x'), at(later, 1)), Verdict::Pass);
    }

    #[test]
    fn a_gap_ends_the_reply() {
        let base = Instant::now();
        let mut filter = LateReplyFilter::new(ReplyEnd::Incomplete, Multiplexer::None, base);
        assert_eq!(filter.verdict(&alt('P'), at(base, 1)), Verdict::Drop);
        assert_eq!(
            filter.verdict(&plain('x'), at(base, 200_001)),
            Verdict::Pass
        );
    }

    #[test]
    fn a_non_printable_key_ends_the_reply() {
        let base = Instant::now();
        let mut filter = LateReplyFilter::new(ReplyEnd::Incomplete, Multiplexer::None, base);
        assert_eq!(filter.verdict(&alt('P'), at(base, 1)), Verdict::Drop);
        assert_eq!(
            filter.verdict(&key(KeyCode::Left, KeyModifiers::NONE), at(base, 2)),
            Verdict::Pass
        );
        assert_eq!(filter.verdict(&plain('y'), at(base, 3)), Verdict::Pass);
    }

    #[test]
    fn a_reply_longer_than_the_limit_lets_the_rest_pass() {
        let base = Instant::now();
        let mut filter = LateReplyFilter::new(ReplyEnd::Incomplete, Multiplexer::None, base);
        assert_eq!(filter.verdict(&alt('P'), at(base, 1)), Verdict::Drop);
        let payload = vec![plain('a'); MAX_REPLY_EVENTS + 50];
        let verdicts = judge(&mut filter, &payload, base, 2);
        let dropped = MAX_REPLY_EVENTS - 1;
        assert!(verdicts[..dropped].iter().all(|v| *v == Verdict::Drop));
        assert!(verdicts[dropped..].iter().all(|v| *v == Verdict::Pass));
    }

    #[test]
    fn a_key_release_inside_a_reply_passes_and_ends_it() {
        let base = Instant::now();
        let mut filter = LateReplyFilter::new(ReplyEnd::Incomplete, Multiplexer::None, base);
        assert_eq!(filter.verdict(&alt('P'), at(base, 1)), Verdict::Drop);
        let mut release = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        assert_eq!(
            filter.verdict(&Event::Key(release), at(base, 2)),
            Verdict::Pass
        );
        assert_eq!(filter.verdict(&plain('a'), at(base, 3)), Verdict::Pass);
    }

    #[test]
    fn a_mouse_event_inside_a_reply_passes_and_ends_it() {
        let base = Instant::now();
        let mut filter = LateReplyFilter::new(ReplyEnd::Incomplete, Multiplexer::None, base);
        assert_eq!(filter.verdict(&alt('_'), at(base, 1)), Verdict::Drop);
        let mouse = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 3,
            row: 4,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(filter.verdict(&mouse, at(base, 2)), Verdict::Pass);
        assert_eq!(filter.verdict(&plain('a'), at(base, 3)), Verdict::Pass);
    }

    #[test]
    fn filtered_skips_a_late_reply_and_returns_errors() {
        let base = Instant::now();
        let mut script: VecDeque<io::Result<Event>> = reply('P', ">|tmux 3.5a", alt('\\'))
            .into_iter()
            .map(Ok)
            .collect();
        script.push_back(Ok(plain('q')));
        script.push_back(Err(io::Error::other("closed")));
        let read = move || {
            script
                .pop_front()
                .unwrap_or_else(|| Err(io::Error::other("exhausted")))
        };
        let filter = LateReplyFilter::new(ReplyEnd::Incomplete, Multiplexer::None, base);
        let mut next = filtered(read, filter, move || base);
        assert_eq!(next().unwrap(), plain('q'));
        assert_eq!(next().unwrap_err().to_string(), "closed");
    }
}
