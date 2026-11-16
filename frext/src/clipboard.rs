//! Clipboard access for the editor's cut/copy/paste menu.
//!
//! The [`Clipboard`] trait abstracts reading and writing clipboard text so
//! the editor's clipboard actions are deterministic and testable. Two
//! implementations are provided:
//!
//! - [`SystemClipboard`] talks to the OS clipboard via [`arboard`] in both
//!   directions (the GUI toolkit can *write* the clipboard but exposes no
//!   imperative *read*, which a menu-driven "Paste" needs).
//! - [`MemoryClipboard`] holds the text in process, for tests and for any
//!   front end without a system clipboard.
//!
//! Every operation is fallible and non-panicking: a clipboard that cannot be
//! opened (headless CI, no display server) yields `None`/`false` and logs,
//! leaving the editor usable.

/// A source and sink of clipboard text.
///
/// Implementations must never panic; failures are reported through the
/// return values.
pub trait Clipboard {
    /// Read the current clipboard text, or `None` when the clipboard is
    /// unavailable or holds no text.
    fn read_text(&mut self) -> Option<String>;

    /// Write `text` to the clipboard. Returns `true` on success.
    fn write_text(&mut self, text: String) -> bool;
}

/// The OS clipboard, accessed through [`arboard`].
///
/// A fresh `arboard` handle is opened for every call, so this type holds no
/// state.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClipboard;

impl Clipboard for SystemClipboard {
    fn read_text(&mut self) -> Option<String> {
        match arboard::Clipboard::new() {
            Ok(mut clipboard) => match clipboard.get_text() {
                Ok(text) => Some(text),
                Err(err) => {
                    log::debug!("clipboard read failed: {err}");
                    None
                }
            },
            Err(err) => {
                log::debug!("clipboard unavailable: {err}");
                None
            }
        }
    }

    fn write_text(&mut self, text: String) -> bool {
        match arboard::Clipboard::new() {
            Ok(mut clipboard) => match clipboard.set_text(text) {
                Ok(()) => true,
                Err(err) => {
                    log::debug!("clipboard write failed: {err}");
                    false
                }
            },
            Err(err) => {
                log::debug!("clipboard unavailable: {err}");
                false
            }
        }
    }
}

/// An in-process clipboard for tests (and any front end without a system
/// clipboard). Writes always succeed and replace the held text.
#[derive(Debug, Default, Clone)]
pub struct MemoryClipboard {
    /// The most recently written text, if any.
    text: Option<String>,
}

impl Clipboard for MemoryClipboard {
    fn read_text(&mut self) -> Option<String> {
        self.text.clone()
    }

    fn write_text(&mut self, text: String) -> bool {
        self.text = Some(text);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_clipboard_starts_empty() {
        let mut clipboard = MemoryClipboard::default();
        assert_eq!(clipboard.read_text(), None);
    }

    #[test]
    fn memory_clipboard_round_trips_written_text() {
        let mut clipboard = MemoryClipboard::default();
        assert!(clipboard.write_text("hello".to_owned()));
        assert_eq!(clipboard.read_text().as_deref(), Some("hello"));
    }

    #[test]
    fn memory_clipboard_second_write_replaces_the_first() {
        let mut clipboard = MemoryClipboard::default();
        assert!(clipboard.write_text("first".to_owned()));
        assert!(clipboard.write_text("second".to_owned()));
        assert_eq!(clipboard.read_text().as_deref(), Some("second"));
    }

    #[test]
    fn memory_clipboard_works_as_a_trait_object() {
        let mut clipboard: Box<dyn Clipboard> = Box::new(MemoryClipboard::default());
        assert!(clipboard.write_text("boxed".to_owned()));
        assert_eq!(clipboard.read_text().as_deref(), Some("boxed"));
    }
}
