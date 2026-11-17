//! Terminal lifecycle: raw mode, the alternate screen, mouse capture,
//! bracketed paste and the keyboard enhancement flags.
//!
//! [`TerminalGuard::enter`] probes the terminal's capabilities and sets
//! everything up, and [`restore`] undoes it. The unified probe
//! ([`crate::term::probe`]) replaces crossterm's keyboard enhancement
//! check and runs right after raw mode is enabled, before any event
//! reader exists, so no other reader can swallow its replies.
//!
//! Restoring runs on drop and from a panic hook, so the user's
//! terminal is left usable however the editor exits. The hook only
//! restores for a panic on the UI thread; a panic on a worker thread
//! (which may be caught and recovered from) is just logged.

use std::io::{self, Stdout, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Once, OnceLock};
use std::thread::ThreadId;

use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{cursor, queue};
use frext_core::config::TermConfig;
use ratatui::Terminal;

use crate::term::backend::FrextBackend;
use crate::term::caps::{KeyboardProtocol, TermEnv, TerminalCaps, detect_multiplexer, resolve};
use crate::term::probe::{PROBE_TIMEOUT, Replies, probe};

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

/// Write the setup sequences for `mode` to `out` and flush it.
///
/// Enters the alternate screen and enables mouse capture and bracketed
/// paste; in [`KeyboardMode::Enhanced`] it also pushes the
/// disambiguate-escape-codes keyboard flag.
///
/// # Errors
///
/// Returns any I/O error from writing or flushing `out`.
pub fn write_setup(out: &mut impl Write, mode: KeyboardMode) -> io::Result<()> {
    queue!(
        out,
        EnterAlternateScreen,
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

/// Write the inverse of [`write_setup`] for `mode` to `out`, in reverse
/// order, then show the cursor and flush.
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
}

impl TerminalGuard {
    /// Put the terminal into editor mode and build the ratatui
    /// [`Terminal`].
    ///
    /// Enables raw mode, then runs the unified terminal probe and
    /// resolves the [`TerminalCaps`] from its replies, the environment
    /// and `settings`; the probe replaces crossterm's keyboard
    /// enhancement check, and the keyboard mode follows the detected
    /// protocol. The probe runs before the alternate screen and before
    /// any event reader exists, so its replies are not swallowed by
    /// another reader; a probe I/O error is logged and treated as no
    /// replies. It then writes the setup sequences, records the calling
    /// thread as the UI thread, and installs a panic hook that restores
    /// the terminal when the UI thread panics. If any setup step fails,
    /// what was done is undone.
    ///
    /// # Errors
    ///
    /// Returns any I/O error from enabling raw mode, writing the setup
    /// sequences, or creating the terminal.
    pub fn enter(settings: TermConfig) -> io::Result<(Self, Terminal<FrextBackend<Stdout>>)> {
        enable_raw_mode()?;
        let env = TermEnv::from_process();
        let multiplexer = detect_multiplexer(&env);
        let replies = match probe(
            &mut io::stdout(),
            rustix::stdio::stdin(),
            multiplexer,
            PROBE_TIMEOUT,
        ) {
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
            if let Err(undo) = write_restore(&mut io::stdout(), mode) {
                log::error!("failed to undo terminal setup: {undo}");
            }
            if let Err(undo) = disable_raw_mode() {
                log::error!("failed to disable raw mode: {undo}");
            }
            return Err(err);
        }
        ENHANCED.store(mode == KeyboardMode::Enhanced, Ordering::SeqCst);
        ACTIVE.store(true, Ordering::SeqCst);
        // Already set when entered before: the UI thread does not change.
        let _ = UI_THREAD.set(std::thread::current().id());
        install_panic_hook();
        // From here on, an early return drops the guard, which restores.
        let underline = caps.underline;
        let guard = Self { mode, caps };
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

    #[test]
    fn setup_bytes_enhanced() {
        let expected = format!("\x1b[?1049h{MOUSE_ON}\x1b[?2004h\x1b[>1u");
        assert_eq!(setup(KeyboardMode::Enhanced), expected);
    }

    #[test]
    fn setup_bytes_legacy() {
        let bytes = setup(KeyboardMode::Legacy);
        assert_eq!(bytes, format!("\x1b[?1049h{MOUSE_ON}\x1b[?2004h"));
        assert!(!bytes.contains("\x1b[>"));
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
        };
        assert_eq!(guard.mode(), KeyboardMode::Legacy);
        assert_eq!(guard.caps(), &caps);
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
