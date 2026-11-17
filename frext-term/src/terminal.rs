//! Terminal lifecycle: raw mode, the alternate screen, mouse capture,
//! bracketed paste and the keyboard enhancement flags.
//!
//! [`TerminalGuard::enter`] probes the terminal's capabilities and sets
//! everything up, and [`restore`] undoes it. The unified probe
//! ([`crate::term::probe`]) replaces crossterm's keyboard enhancement
//! check. It runs inside the alternate screen, which the setup
//! sequences then clear, so a terminal that prints the queries leaves no
//! trace. It runs before any event reader exists, so no other reader
//! can swallow its replies. It reads the same tty crossterm reads:
//! stdin when that is a terminal, `/dev/tty` otherwise.
//!
//! Restoring runs on drop and from a panic hook, so the user's
//! terminal is left usable however the editor exits. The hook only
//! restores for a panic on the UI thread; a panic on a worker thread
//! (which may be caught and recovered from) is just logged.

use std::fs::File;
use std::io::{self, IsTerminal, Stdout, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Once, OnceLock};
use std::thread::ThreadId;

use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{
    Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{cursor, queue};
use frext_core::config::TermConfig;
use ratatui::Terminal;

use crate::term::backend::FrextBackend;
use crate::term::caps::{
    KeyboardProtocol, Multiplexer, TermEnv, TerminalCaps, detect_multiplexer, resolve,
};
use crate::term::probe::{PROBE_TIMEOUT, Replies, ReplyEnd, probe};

/// Whether the terminal is currently set up and needs restoring.
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// Whether the keyboard enhancement flags were pushed on setup.
static ENHANCED: AtomicBool = AtomicBool::new(false);
/// Guards the one-time panic hook installation.
static PANIC_HOOK: Once = Once::new();
/// The thread that entered the terminal: the UI thread.
static UI_THREAD: OnceLock<ThreadId> = OnceLock::new();

/// How key events are reported by the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyboardMode {
    /// The kitty keyboard protocol is active, so modified keys such as
    /// `Ctrl+Shift+Z` are reported unambiguously.
    Enhanced,
    /// Plain legacy key reporting.
    Legacy,
}

impl KeyboardMode {
    /// The keyboard mode for a detected keyboard `protocol`: the kitty
    /// protocol (with any flags) is [`KeyboardMode::Enhanced`], legacy
    /// input is [`KeyboardMode::Legacy`].
    #[must_use]
    pub const fn from_protocol(protocol: KeyboardProtocol) -> Self {
        match protocol {
            KeyboardProtocol::Kitty { .. } => Self::Enhanced,
            KeyboardProtocol::Legacy => Self::Legacy,
        }
    }
}

/// Write the sequence entering the alternate screen to `out` and flush
/// it. [`write_restore`] leaves the alternate screen again.
///
/// # Errors
///
/// Returns any I/O error from writing or flushing `out`.
pub fn write_enter(out: &mut impl Write) -> io::Result<()> {
    queue!(out, EnterAlternateScreen)?;
    out.flush()
}

/// Write the setup sequences for `mode` to `out` and flush it.
///
/// Clears the screen (wiping anything a terminal printed in response to
/// the probe's queries), then enables mouse capture and bracketed
/// paste; in [`KeyboardMode::Enhanced`] it also pushes the
/// disambiguate-escape-codes keyboard flag. The alternate screen is
/// entered separately, by [`write_enter`].
///
/// # Errors
///
/// Returns any I/O error from writing or flushing `out`.
pub fn write_setup(out: &mut impl Write, mode: KeyboardMode) -> io::Result<()> {
    queue!(
        out,
        Clear(ClearType::All),
        EnableMouseCapture,
        EnableBracketedPaste
    )?;
    if mode == KeyboardMode::Enhanced {
        queue!(
            out,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
    }
    out.flush()
}

/// Write the inverse of [`write_enter`] followed by [`write_setup`] for
/// `mode` to `out`, in reverse order (leaving the alternate screen
/// last), then show the cursor and flush.
///
/// # Errors
///
/// Returns any I/O error from writing or flushing `out`.
pub fn write_restore(out: &mut impl Write, mode: KeyboardMode) -> io::Result<()> {
    if mode == KeyboardMode::Enhanced {
        queue!(out, PopKeyboardEnhancementFlags)?;
    }
    queue!(
        out,
        DisableBracketedPaste,
        DisableMouseCapture,
        LeaveAlternateScreen,
        cursor::Show
    )?;
    out.flush()
}

/// Owns the terminal setup; dropping it calls [`restore`].
#[derive(Debug)]
pub struct TerminalGuard {
    mode: KeyboardMode,
    caps: TerminalCaps,
    probe_end: ReplyEnd,
}

impl TerminalGuard {
    /// Put the terminal into editor mode and build the ratatui
    /// [`Terminal`].
    ///
    /// Enables raw mode and enters the alternate screen, then runs the
    /// unified terminal probe and resolves the [`TerminalCaps`] from its
    /// replies, the environment and `settings`; the probe replaces
    /// crossterm's keyboard enhancement check, and the keyboard mode
    /// follows the detected protocol. The probe reads the tty crossterm
    /// reads (stdin when it is a terminal, `/dev/tty` otherwise) before
    /// any event reader exists, so its replies are not swallowed by
    /// another reader; a probe I/O error is logged and treated as no
    /// replies. It then writes the setup sequences (which first clear the
    /// screen, wiping anything a terminal printed in response to the
    /// queries), records the calling thread as the UI thread, and
    /// installs a panic hook that restores the terminal when the UI
    /// thread panics. If any setup step fails, what was done is undone.
    ///
    /// # Errors
    ///
    /// Returns any I/O error from enabling raw mode, writing the setup
    /// sequences, or creating the terminal.
    pub fn enter(settings: TermConfig) -> io::Result<(Self, Terminal<FrextBackend<Stdout>>)> {
        enable_raw_mode()?;
        if let Err(err) = write_enter(&mut io::stdout()) {
            undo_setup(KeyboardMode::Legacy);
            return Err(err);
        }
        let env = TermEnv::from_process();
        let multiplexer = detect_multiplexer(&env);
        let replies = match run_probe(multiplexer) {
            Ok(replies) => replies,
            Err(err) => {
                log::warn!("terminal probe failed: {err}");
                Replies::default()
            }
        };
        let caps = resolve(&replies, &env, settings);
        log::info!("terminal caps: {caps:?}");
        let mode = KeyboardMode::from_protocol(caps.keyboard);
        if let Err(err) = write_setup(&mut io::stdout(), mode) {
            undo_setup(mode);
            return Err(err);
        }
        ENHANCED.store(mode == KeyboardMode::Enhanced, Ordering::SeqCst);
        ACTIVE.store(true, Ordering::SeqCst);
        // Already set when entered before: the UI thread does not change.
        let _ = UI_THREAD.set(std::thread::current().id());
        install_panic_hook();
        // From here on, an early return drops the guard, which restores.
        let underline = caps.underline;
        let guard = Self {
            mode,
            caps,
            probe_end: replies.end,
        };
        let terminal = Terminal::new(FrextBackend::new(io::stdout(), underline))?;
        Ok((guard, terminal))
    }

    /// The keyboard mode detected by [`TerminalGuard::enter`].
    #[must_use]
    pub const fn mode(&self) -> KeyboardMode {
        self.mode
    }

    /// The terminal capabilities resolved by [`TerminalGuard::enter`].
    #[must_use]
    pub const fn caps(&self) -> &TerminalCaps {
        &self.caps
    }

    /// How the startup probe's replies ended; [`ReplyEnd::Incomplete`]
    /// after a probe error.
    #[must_use]
    pub const fn probe_end(&self) -> ReplyEnd {
        self.probe_end
    }
}

/// Undo a partial setup for `mode` after a setup step failed: write the
/// restore sequences and disable raw mode, logging any error.
fn undo_setup(mode: KeyboardMode) {
    if let Err(undo) = write_restore(&mut io::stdout(), mode) {
        log::error!("failed to undo terminal setup: {undo}");
    }
    if let Err(undo) = disable_raw_mode() {
        log::error!("failed to disable raw mode: {undo}");
    }
}

/// Probe the terminal on the tty crossterm reads: stdin when it is a
/// terminal, `/dev/tty` otherwise. The queries go to stdout, which
/// crossterm and ratatui write to.
///
/// # Errors
///
/// Returns the error from opening `/dev/tty` or from the probe.
fn run_probe(multiplexer: Multiplexer) -> io::Result<Replies> {
    if io::stdin().is_terminal() {
        probe(
            &mut io::stdout(),
            rustix::stdio::stdin(),
            multiplexer,
            PROBE_TIMEOUT,
        )
    } else {
        let tty = File::open("/dev/tty")?;
        probe(&mut io::stdout(), &tty, multiplexer, PROBE_TIMEOUT)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore();
    }
}

/// Restore the terminal to the state it was in before
/// [`TerminalGuard::enter`].
///
/// Idempotent: does nothing unless the terminal is currently set up.
/// Errors are logged, not returned, so this is safe to call from a
/// panic hook.
pub fn restore() {
    if restore_to(&mut io::stdout())
        && let Err(err) = disable_raw_mode()
    {
        log::error!("failed to disable raw mode: {err}");
    }
}

/// Write the restore sequences to `out` if the terminal is set up, and
/// mark it as no longer set up. Returns whether it was set up.
fn restore_to(out: &mut impl Write) -> bool {
    if !ACTIVE.swap(false, Ordering::SeqCst) {
        return false;
    }
    let mode = if ENHANCED.load(Ordering::SeqCst) {
        KeyboardMode::Enhanced
    } else {
        KeyboardMode::Legacy
    };
    if let Err(err) = write_restore(out, mode) {
        log::error!("failed to restore terminal: {err}");
    }
    true
}

/// What the panic hook does for a panic on a given thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PanicResponse {
    /// Restore the terminal, log, and run the previous hook.
    RestoreAndReport,
    /// Only log: the panic may be caught, and restoring or printing
    /// would break the still-running UI.
    LogOnly,
}

/// Decide the panic response for a panic on thread `current`, given the
/// UI thread `ui` (`None` before the terminal was ever entered).
fn panic_response(current: ThreadId, ui: Option<ThreadId>) -> PanicResponse {
    match ui {
        Some(ui) if ui != current => PanicResponse::LogOnly,
        _ => PanicResponse::RestoreAndReport,
    }
}

/// Install, once, a panic hook. A panic on the UI thread restores the
/// terminal before the previous hook prints the panic message; a panic
/// on any other thread is only logged, leaving the terminal and the
/// screen untouched.
fn install_panic_hook() {
    PANIC_HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let current = std::thread::current();
            match panic_response(current.id(), UI_THREAD.get().copied()) {
                PanicResponse::RestoreAndReport => {
                    restore();
                    log::error!("panic: {info}");
                    previous(info);
                }
                PanicResponse::LogOnly => {
                    let name = current.name().unwrap_or("<unnamed>");
                    log::error!("panic on thread {name}: {info}");
                }
            }
        }));
    });
}

#[cfg(test)]
mod tests {
    // Tests may unwrap: a failure is a test failure.
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::term::caps::UnderlineStyle;

    const MOUSE_ON: &str = "\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1015h\x1b[?1006h";
    const MOUSE_OFF: &str = "\x1b[?1006l\x1b[?1015l\x1b[?1003l\x1b[?1002l\x1b[?1000l";

    fn setup(mode: KeyboardMode) -> String {
        let mut out = Vec::new();
        write_setup(&mut out, mode).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn restored(mode: KeyboardMode) -> String {
        let mut out = Vec::new();
        write_restore(&mut out, mode).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn entered() -> String {
        let mut out = Vec::new();
        write_enter(&mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn enter_bytes_enter_the_alternate_screen() {
        assert_eq!(entered(), "\x1b[?1049h");
    }

    #[test]
    fn setup_bytes_enhanced() {
        let bytes = setup(KeyboardMode::Enhanced);
        assert_eq!(bytes, format!("\x1b[2J{MOUSE_ON}\x1b[?2004h\x1b[>1u"));
        assert!(!bytes.contains("\x1b[?1049h"));
    }

    #[test]
    fn setup_bytes_legacy() {
        let bytes = setup(KeyboardMode::Legacy);
        assert_eq!(bytes, format!("\x1b[2J{MOUSE_ON}\x1b[?2004h"));
        assert!(!bytes.contains("\x1b[>"));
        assert!(!bytes.contains("\x1b[?1049h"));
    }

    #[test]
    fn enter_then_setup_is_the_full_setup() {
        assert_eq!(
            entered() + &setup(KeyboardMode::Enhanced),
            format!("\x1b[?1049h\x1b[2J{MOUSE_ON}\x1b[?2004h\x1b[>1u")
        );
        assert_eq!(
            entered() + &setup(KeyboardMode::Legacy),
            format!("\x1b[?1049h\x1b[2J{MOUSE_ON}\x1b[?2004h")
        );
    }

    #[test]
    fn restore_bytes_are_the_inverse() {
        assert_eq!(
            restored(KeyboardMode::Enhanced),
            format!("\x1b[<1u\x1b[?2004l{MOUSE_OFF}\x1b[?1049l\x1b[?25h")
        );
        let legacy = restored(KeyboardMode::Legacy);
        assert_eq!(
            legacy,
            format!("\x1b[?2004l{MOUSE_OFF}\x1b[?1049l\x1b[?25h")
        );
        assert!(!legacy.contains("\x1b[<"));
    }

    #[test]
    fn restore_is_idempotent_when_inactive() {
        // No test ever sets ACTIVE, so it is always false here.
        let mut out = Vec::new();
        assert!(!restore_to(&mut out));
        assert!(!restore_to(&mut out));
        assert!(out.is_empty());
        restore();
        restore();
    }

    #[test]
    fn mode_reports_the_detected_mode() {
        // Built directly: entering for real would touch the test
        // runner's terminal. ACTIVE stays false, so the drop is a no-op.
        let caps = TerminalCaps {
            underline: UnderlineStyle::Curly,
            keyboard: KeyboardProtocol::Legacy,
            version: Some("test".to_owned()),
            ..TerminalCaps::default()
        };
        let guard = TerminalGuard {
            mode: KeyboardMode::Legacy,
            caps: caps.clone(),
            probe_end: ReplyEnd::Da1,
        };
        assert_eq!(guard.mode(), KeyboardMode::Legacy);
        assert_eq!(guard.caps(), &caps);
    }

    #[test]
    fn probe_end_reports_what_the_guard_was_built_with() {
        // Built directly, as above: ACTIVE stays false.
        for end in [ReplyEnd::Da1, ReplyEnd::Incomplete] {
            let guard = TerminalGuard {
                mode: KeyboardMode::Legacy,
                caps: TerminalCaps::default(),
                probe_end: end,
            };
            assert_eq!(guard.probe_end(), end);
        }
    }

    #[test]
    fn from_protocol_maps_kitty_to_enhanced() {
        assert_eq!(
            KeyboardMode::from_protocol(KeyboardProtocol::Kitty { flags: 0 }),
            KeyboardMode::Enhanced
        );
        assert_eq!(
            KeyboardMode::from_protocol(KeyboardProtocol::Kitty { flags: 31 }),
            KeyboardMode::Enhanced
        );
    }

    #[test]
    fn from_protocol_maps_legacy_to_legacy() {
        assert_eq!(
            KeyboardMode::from_protocol(KeyboardProtocol::Legacy),
            KeyboardMode::Legacy
        );
    }

    fn other_thread_id() -> ThreadId {
        std::thread::spawn(|| std::thread::current().id())
            .join()
            .unwrap()
    }

    #[test]
    fn panic_response_on_the_ui_thread_restores() {
        let ui = std::thread::current().id();
        assert_eq!(
            panic_response(ui, Some(ui)),
            PanicResponse::RestoreAndReport
        );
    }

    #[test]
    fn panic_response_on_another_thread_only_logs() {
        let ui = std::thread::current().id();
        let worker = other_thread_id();
        assert_ne!(worker, ui);
        assert_eq!(panic_response(worker, Some(ui)), PanicResponse::LogOnly);
    }

    #[test]
    fn panic_response_before_enter_restores() {
        let current = std::thread::current().id();
        assert_eq!(
            panic_response(current, None),
            PanicResponse::RestoreAndReport
        );
        assert_eq!(
            panic_response(other_thread_id(), None),
            PanicResponse::RestoreAndReport
        );
    }
}
