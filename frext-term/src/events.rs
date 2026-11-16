//! The terminal front end's event loop: one channel fed by an input
//! thread, a tick thread and wakers, drained in batches by the UI loop.

use std::io;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use crossterm::event::Event;

/// How often the tick thread fires by default.
pub const TICK: Duration = Duration::from_millis(500);

/// Everything that can wake the UI loop.
#[derive(Debug)]
pub enum AppEvent {
    /// A terminal input event (key, mouse, resize, paste, ...).
    Input(Event),
    /// The input reader failed; no further `Input` events will arrive.
    InputClosed,
    /// A background task asked for a redraw.
    Wake,
    /// The periodic tick.
    Tick,
}

/// The single channel every event source feeds into.
#[derive(Debug)]
pub struct EventLoop {
    tx: Sender<AppEvent>,
    rx: Receiver<AppEvent>,
}

impl Default for EventLoop {
    fn default() -> Self {
        Self::new()
    }
}

impl EventLoop {
    /// Creates an event loop with no sources attached.
    #[must_use]
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self { tx, rx }
    }

    /// Starts a thread that sends an [`AppEvent::Input`] for every event
    /// `read` returns. On a read error it logs, sends
    /// [`AppEvent::InputClosed`] and exits; it also exits once the
    /// receiver is gone.
    ///
    /// # Errors
    ///
    /// Returns the error if the thread could not be spawned.
    pub fn spawn_input(
        &self,
        mut read: impl FnMut() -> io::Result<Event> + Send + 'static,
    ) -> io::Result<()> {
        let tx = self.tx.clone();
        thread::Builder::new()
            .name("frext-term-input".to_owned())
            .spawn(move || {
                loop {
                    match read() {
                        Ok(event) => {
                            if tx.send(AppEvent::Input(event)).is_err() {
                                break;
                            }
                        }
                        Err(err) => {
                            log::error!("terminal input read failed: {err}");
                            let _ = tx.send(AppEvent::InputClosed);
                            break;
                        }
                    }
                }
            })?;
        Ok(())
    }

    /// Starts a thread that sends [`AppEvent::Tick`] every `every` until
    /// the receiver is gone.
    ///
    /// # Errors
    ///
    /// Returns the error if the thread could not be spawned.
    pub fn spawn_tick(&self, every: Duration) -> io::Result<()> {
        let tx = self.tx.clone();
        thread::Builder::new()
            .name("frext-term-tick".to_owned())
            .spawn(move || {
                loop {
                    thread::sleep(every);
                    if tx.send(AppEvent::Tick).is_err() {
                        break;
                    }
                }
            })?;
        Ok(())
    }

    /// Returns a callable that sends [`AppEvent::Wake`], ignoring a send
    /// error (the loop has shut down).
    pub fn waker(&self) -> impl Fn() + Send + 'static {
        let tx = self.tx.clone();
        move || {
            let _ = tx.send(AppEvent::Wake);
        }
    }

    /// Blocks for one event, then drains every event already queued.
    ///
    /// Returns `None` only if every sender is gone, which cannot happen
    /// while `self` holds its own sender.
    #[must_use]
    pub fn next_batch(&self) -> Option<Vec<AppEvent>> {
        let first = self.rx.recv().ok()?;
        let mut batch = vec![first];
        batch.extend(self.rx.try_iter());
        Some(batch)
    }
}

#[cfg(test)]
mod tests {
    // `unwrap` is acceptable in test code: a panic on an unexpected `Err`
    // is exactly the failure signal we want from a test.
    #![allow(clippy::unwrap_used)]

    use std::time::Instant;

    use crossterm::event::{KeyCode, KeyEvent};

    use super::*;

    fn key(c: char) -> Event {
        Event::Key(KeyEvent::from(KeyCode::Char(c)))
    }

    #[test]
    fn waker_sends_wake() {
        let events = EventLoop::new();
        let wake = events.waker();
        wake();
        let batch = events.next_batch().unwrap();
        assert_eq!(batch.len(), 1);
        assert!(matches!(batch[0], AppEvent::Wake));
    }

    #[test]
    fn input_events_arrive_in_order() {
        let events = EventLoop::new();
        let mut script =
            vec![Ok(key('a')), Ok(key('b')), Err(io::Error::other("closed"))].into_iter();
        events
            .spawn_input(move || {
                script
                    .next()
                    .unwrap_or_else(|| Err(io::Error::other("script exhausted")))
            })
            .unwrap();

        let mut received = Vec::new();
        while !received
            .last()
            .is_some_and(|event| matches!(event, AppEvent::InputClosed))
        {
            assert!(received.len() < 3, "too many events: {received:?}");
            received.extend(events.next_batch().unwrap());
        }

        assert_eq!(received.len(), 3, "{received:?}");
        assert!(matches!(&received[0], AppEvent::Input(e) if *e == key('a')));
        assert!(matches!(&received[1], AppEvent::Input(e) if *e == key('b')));
        assert!(matches!(received[2], AppEvent::InputClosed));
    }

    #[test]
    fn tick_arrives() {
        let events = EventLoop::new();
        let start = Instant::now();
        events.spawn_tick(Duration::from_millis(10)).unwrap();
        let batch = events.next_batch().unwrap();
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(!batch.is_empty());
        assert!(batch.iter().all(|event| matches!(event, AppEvent::Tick)));
    }

    #[test]
    fn next_batch_drains_queued_events() {
        let events = EventLoop::new();
        let wake = events.waker();
        wake();
        wake();
        wake();
        let batch = events.next_batch().unwrap();
        assert_eq!(batch.len(), 3);
        assert!(batch.iter().all(|event| matches!(event, AppEvent::Wake)));
    }
}
