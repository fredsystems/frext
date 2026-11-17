//! The UI loop: draw, wait for a batch of events, route them, repeat.

use std::io::{self, Write};
use std::time::Instant;

use ratatui::Terminal;
use ratatui::backend::Backend;

use crate::app::TermApp;
use crate::events::EventLoop;

/// Draws one frame of `app` on `terminal`, writing the app's queued
/// graphics (icon uploads) between filling the buffer and writing its
/// diff.
///
/// This replaces [`Terminal::draw`]: an upload must precede, in the byte
/// stream, the first frame that shows its placeholders, and
/// `Terminal::draw` offers no point between filling the buffer and
/// writing the diff. The app returns its cursor from
/// [`TermApp::render_frame`] because a `Frame`'s cursor is private.
///
/// # Errors
///
/// Returns the backend's error, converted to an [`io::Error`], or the
/// error writing the graphics.
pub fn draw<B>(app: &mut TermApp, terminal: &mut Terminal<B>) -> io::Result<()>
where
    B: Backend + Write,
    B::Error: Send + Sync + 'static,
{
    terminal.autoresize().map_err(io::Error::other)?;
    let cursor = {
        let mut frame = terminal.get_frame();
        app.render_frame(&mut frame)
    };
    let graphics = app.take_graphics();
    if !graphics.is_empty() {
        terminal.backend_mut().write_all(&graphics)?;
    }
    terminal
        .apply_buffer_with_cursor(cursor)
        .map_err(io::Error::other)?;
    Ok(())
}

/// Runs `app` on `terminal` until it asks to quit or `events` runs dry.
///
/// Each frame applies queued effects, draws with [`draw`], and ends the
/// frame; then it
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
    B: Backend + Write,
    B::Error: Send + Sync + 'static,
{
    app.begin_frame(events.waker());
    loop {
        app.apply_effects();
        draw(app, terminal)?;
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
    use crossterm::style::Colored;
    use frext_core::clipboard::MemoryClipboard;
    use frext_core::editor::Editor;
    use frext_core::persistence::Store;
    use ratatui::layout::Rect;
    use ratatui::{TerminalOptions, Viewport};

    use super::*;
    use crate::events::AppEvent;
    use crate::icons::kitty::KittyIcons;
    use crate::term::backend::FrextBackend;
    use crate::term::caps::{CellPx, Multiplexer, UnderlineStyle};
    use crate::term::kitty::PLACEHOLDER_CELLS;
    use crate::terminal::KeyboardMode;

    type TestTerminal = Terminal<FrextBackend<Vec<u8>>>;

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

    /// An 80 x 24 terminal writing to memory; a fixed viewport never asks
    /// the backend for its size.
    fn terminal() -> TestTerminal {
        Terminal::with_options(
            FrextBackend::new(Vec::new(), UnderlineStyle::Straight),
            TerminalOptions {
                viewport: Viewport::Fixed(Rect::new(0, 0, 80, 24)),
            },
        )
        .unwrap()
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

    /// An app with the workspace `dir/ws`, which holds `main.rs`, and
    /// its state in `dir/<state>`.
    fn workspace_app(dir: &Path, state: &str) -> TermApp {
        let ws = dir.join("ws");
        fs::create_dir_all(&ws).unwrap();
        fs::write(ws.join("main.rs"), "fn main() {}\n").unwrap();
        let editor = Editor::open(
            Store::at(dir.join(state)).unwrap(),
            Box::new(MemoryClipboard::default()),
            &[],
            Some(&ws),
        );
        TermApp::new(editor, KeyboardMode::Enhanced, ws, None)
    }

    fn ctrl(c: char) -> AppEvent {
        AppEvent::Input(Event::Key(KeyEvent::new(
            KeyCode::Char(c),
            KeyModifiers::CONTROL,
        )))
    }

    fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
    }

    #[test]
    fn run_routes_queued_events_until_quit() {
        let dir = temp_dir("quit");
        let mut app = app(&dir);
        let mut terminal = terminal();
        let events = EventLoop::new();
        let tx = events.sender();
        tx.send(ctrl('n')).unwrap();
        tx.send(ctrl('q')).unwrap();

        run(&mut app, &mut terminal, &events, Instant::now).unwrap();

        assert!(app.should_quit());
        assert_eq!(app.editor().tabs().len(), 2);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn run_stops_when_input_closes() {
        let dir = temp_dir("closed");
        let mut app = app(&dir);
        let mut terminal = terminal();
        let events = EventLoop::new();
        events.sender().send(AppEvent::InputClosed).unwrap();

        run(&mut app, &mut terminal, &events, Instant::now).unwrap();

        assert!(app.should_quit());
        assert_eq!(app.editor().tabs().len(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn draw_writes_uploads_before_the_frame_that_shows_them() {
        Colored::set_ansi_color_disabled(false);
        let dir = temp_dir("uploads");
        let mut app = workspace_app(&dir, "state");
        app.set_icons(Box::new(KittyIcons::new(
            CellPx {
                width: 10,
                height: 20,
            },
            Multiplexer::None,
        )));
        let mut terminal = terminal();

        draw(&mut app, &mut terminal).unwrap();
        let output = terminal.backend().writer();
        let upload = find(output, b"\x1b_Ga=T").unwrap();
        let placeholder = find(output, PLACEHOLDER_CELLS[0].as_bytes()).unwrap();
        assert!(upload < placeholder, "{upload} >= {placeholder}");

        terminal.backend_mut().writer_mut().clear();
        draw(&mut app, &mut terminal).unwrap();
        assert_eq!(find(terminal.backend().writer(), b"\x1b_G"), None);

        let release = String::from_utf8(app.release_graphics()).unwrap();
        assert!(release.contains("a=d,d=I,i=1,q=2"), "{release:?}");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn draw_without_graphics_matches_terminal_draw() {
        Colored::set_ansi_color_disabled(false);
        let dir = temp_dir("plain");
        let mut drawn = workspace_app(&dir, "state-a");
        let mut expected = workspace_app(&dir, "state-b");
        let mut ours = terminal();
        let mut theirs = terminal();

        draw(&mut drawn, &mut ours).unwrap();
        theirs.draw(|frame| expected.render(frame)).unwrap();

        assert!(!ours.backend().writer().is_empty());
        assert_eq!(ours.backend().writer(), theirs.backend().writer());
        fs::remove_dir_all(&dir).unwrap();
    }
}
