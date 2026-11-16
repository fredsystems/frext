//! The UI loop: draw, wait for a batch of events, route them, repeat.

use std::io;
use std::time::Instant;

use ratatui::Terminal;
use ratatui::backend::Backend;

use crate::app::TermApp;
use crate::events::EventLoop;

/// Runs `app` on `terminal` until it asks to quit or `events` runs dry.
///
/// Each frame applies queued effects, draws, and ends the frame; then it
/// blocks for the next batch of events and routes each one, stamped with
/// `now()`. Quitting is checked after the draw, so the last frame is
/// shown.
///
/// # Errors
///
/// Returns the backend's draw error, converted to an [`io::Error`].
pub fn run<B>(
    app: &mut TermApp,
    terminal: &mut Terminal<B>,
    events: &EventLoop,
    now: impl Fn() -> Instant,
) -> io::Result<()>
where
    B: Backend,
    B::Error: Send + Sync + 'static,
{
    app.begin_frame(events.waker());
    loop {
        app.apply_effects();
        terminal
            .draw(|frame| app.render(frame))
            .map_err(io::Error::other)?;
        app.end_frame();
        if app.should_quit() {
            break;
        }
        let Some(batch) = events.next_batch() else {
            break;
        };
        app.begin_frame(events.waker());
        for event in batch {
            app.handle(event, now());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    // `unwrap` is acceptable in test code: a panic on an unexpected `Err`
    // is exactly the failure signal we want from a test.
    #![allow(clippy::unwrap_used)]

    use std::fs;
    use std::path::{Path, PathBuf};

    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use frext_core::clipboard::MemoryClipboard;
    use frext_core::editor::Editor;
    use frext_core::persistence::Store;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::events::AppEvent;
    use crate::terminal::KeyboardMode;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "frext-term-run-test-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn app(dir: &Path) -> TermApp {
        let editor = Editor::open(
            Store::at(dir.join("state")).unwrap(),
            Box::new(MemoryClipboard::default()),
            &[],
            None,
        );
        TermApp::new(editor, KeyboardMode::Enhanced, dir.to_path_buf(), None)
    }

    fn ctrl(c: char) -> AppEvent {
        AppEvent::Input(Event::Key(KeyEvent::new(
            KeyCode::Char(c),
            KeyModifiers::CONTROL,
        )))
    }

    #[test]
    fn run_routes_queued_events_until_quit() {
        let dir = temp_dir("quit");
        let mut app = app(&dir);
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let events = EventLoop::new();
        let tx = events.sender();
        tx.send(ctrl('n')).unwrap();
        tx.send(ctrl('q')).unwrap();

        run(&mut app, &mut terminal, &events, Instant::now).unwrap();

        assert!(app.should_quit());
        assert_eq!(app.editor().tabs().len(), 2);
    }

    #[test]
    fn run_stops_when_input_closes() {
        let dir = temp_dir("closed");
        let mut app = app(&dir);
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let events = EventLoop::new();
        events.sender().send(AppEvent::InputClosed).unwrap();

        run(&mut app, &mut terminal, &events, Instant::now).unwrap();

        assert!(app.should_quit());
        assert_eq!(app.editor().tabs().len(), 1);
    }
}
