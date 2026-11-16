//! Clipboard selection for the terminal front end.
//!
//! Under a display server (Wayland or X11) the terminal front end uses the
//! same [`SystemClipboard`] as the GUI. Without one (an SSH session, a bare
//! console) it falls back to [`Osc52Clipboard`], which asks the hosting
//! terminal emulator to set its clipboard through the OSC 52 escape.
//!
//! OSC 52 is write-only here: pasting arrives as a bracketed-paste event
//! from the terminal, so [`Osc52Clipboard::read_text`] always returns
//! `None`.

use std::ffi::OsStr;
use std::io::Write;

use crossterm::clipboard::CopyToClipboard;
use frext_core::clipboard::{Clipboard, SystemClipboard};

/// A write-only clipboard that sets the terminal emulator's clipboard by
/// emitting an OSC 52 escape to `out`.
#[derive(Debug)]
pub struct Osc52Clipboard<W: Write> {
    /// Where the escape is written (normally standard output).
    out: W,
}

impl<W: Write> Osc52Clipboard<W> {
    /// Create an OSC 52 clipboard writing its escapes to `out`.
    #[must_use]
    pub const fn new(out: W) -> Self {
        Self { out }
    }
}

impl<W: Write> Clipboard for Osc52Clipboard<W> {
    fn read_text(&mut self) -> Option<String> {
        log::debug!("OSC 52 clipboard reads are not supported; returning no text");
        None
    }

    fn write_text(&mut self, text: String) -> bool {
        let result = crossterm::queue!(self.out, CopyToClipboard::to_clipboard_from(text))
            .and_then(|()| self.out.flush());
        match result {
            Ok(()) => true,
            Err(err) => {
                log::warn!("failed to write the OSC 52 clipboard escape: {err}");
                false
            }
        }
    }
}

/// Which clipboard implementation the terminal front end uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardChoice {
    /// The OS clipboard, through a display server.
    System,
    /// The terminal emulator's clipboard, through OSC 52.
    Osc52,
}

impl ClipboardChoice {
    /// Choose from the `WAYLAND_DISPLAY` and `DISPLAY` environment values:
    /// [`System`](Self::System) if either is set and non-empty, otherwise
    /// [`Osc52`](Self::Osc52).
    #[must_use]
    pub fn from_env(wayland: Option<&OsStr>, display: Option<&OsStr>) -> Self {
        let present = |value: Option<&OsStr>| value.is_some_and(|v| !v.is_empty());
        if present(wayland) || present(display) {
            Self::System
        } else {
            Self::Osc52
        }
    }
}

/// Build the clipboard for `choice`. The OSC 52 clipboard writes to
/// standard output.
#[must_use]
pub fn make_clipboard(choice: ClipboardChoice) -> Box<dyn Clipboard> {
    match choice {
        ClipboardChoice::System => Box::new(SystemClipboard::new()),
        ClipboardChoice::Osc52 => Box::new(Osc52Clipboard::new(std::io::stdout())),
    }
}

#[cfg(test)]
mod tests {
    // `unwrap` is acceptable in test code: a panic on an unexpected `Err`
    // is exactly the failure signal we want from a test.
    #![allow(clippy::unwrap_used)]

    use std::io;

    use super::*;

    /// A writer whose every write and flush fails.
    struct FailingWriter;

    impl Write for FailingWriter {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("write refused"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("flush refused"))
        }
    }

    #[test]
    fn osc52_write_emits_the_escape() {
        let mut clipboard = Osc52Clipboard::new(Vec::new());
        assert!(clipboard.write_text("héllo".to_owned()));
        // "héllo" as UTF-8 is 68 C3 A9 6C 6C 6F, standard base64 "aMOpbGxv".
        assert_eq!(clipboard.out, b"\x1b]52;c;aMOpbGxv\x1b\\");
    }

    #[test]
    fn osc52_read_is_none() {
        let mut clipboard = Osc52Clipboard::new(Vec::new());
        assert!(clipboard.write_text("text".to_owned()));
        assert_eq!(clipboard.read_text(), None);
    }

    #[test]
    fn osc52_write_failure_returns_false() {
        let mut clipboard = Osc52Clipboard::new(FailingWriter);
        assert!(!clipboard.write_text("text".to_owned()));
    }

    #[test]
    fn from_env_both_unset_is_osc52() {
        assert_eq!(
            ClipboardChoice::from_env(None, None),
            ClipboardChoice::Osc52
        );
    }

    #[test]
    fn from_env_both_empty_is_osc52() {
        let empty = OsStr::new("");
        assert_eq!(
            ClipboardChoice::from_env(Some(empty), Some(empty)),
            ClipboardChoice::Osc52
        );
        assert_eq!(
            ClipboardChoice::from_env(Some(empty), None),
            ClipboardChoice::Osc52
        );
        assert_eq!(
            ClipboardChoice::from_env(None, Some(empty)),
            ClipboardChoice::Osc52
        );
    }

    #[test]
    fn from_env_wayland_set_is_system() {
        assert_eq!(
            ClipboardChoice::from_env(Some(OsStr::new("wayland-0")), None),
            ClipboardChoice::System
        );
        assert_eq!(
            ClipboardChoice::from_env(Some(OsStr::new("wayland-0")), Some(OsStr::new(""))),
            ClipboardChoice::System
        );
    }

    #[test]
    fn from_env_display_set_is_system() {
        assert_eq!(
            ClipboardChoice::from_env(None, Some(OsStr::new(":0"))),
            ClipboardChoice::System
        );
        assert_eq!(
            ClipboardChoice::from_env(Some(OsStr::new("")), Some(OsStr::new(":0"))),
            ClipboardChoice::System
        );
    }

    #[test]
    fn from_env_both_set_is_system() {
        assert_eq!(
            ClipboardChoice::from_env(Some(OsStr::new("wayland-0")), Some(OsStr::new(":0"))),
            ClipboardChoice::System
        );
    }

    #[test]
    fn make_clipboard_osc52_reads_nothing() {
        let mut clipboard = make_clipboard(ClipboardChoice::Osc52);
        assert_eq!(clipboard.read_text(), None);
    }
}
