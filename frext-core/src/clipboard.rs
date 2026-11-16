//! Clipboard access for the editor's cut/copy/paste menu.
//!
//! The [`Clipboard`] trait abstracts reading and writing clipboard text so
//! the editor's clipboard actions are deterministic and testable. Two
//! implementations are provided:
//!
//! - [`SystemClipboard`] talks to the OS clipboard via [`arboard`] in both
//!   directions (the GUI toolkit can *write* the clipboard but exposes no
//!   imperative *read*, which a menu-driven "Paste" needs). It keeps one
//!   connection open for its whole life, because on Linux dropping the last
//!   handle loses the copied text unless a clipboard manager takes it over.
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

/// What the retention logic needs from an OS clipboard connection.
trait Backend: Sized {
    /// Open a connection to the clipboard.
    fn connect() -> Result<Self, String>;

    /// Read the clipboard text through this connection.
    fn get_text(&mut self) -> Result<String, String>;

    /// Write `text` to the clipboard through this connection.
    fn set_text(&mut self, text: String) -> Result<(), String>;
}

impl Backend for arboard::Clipboard {
    fn connect() -> Result<Self, String> {
        Self::new().map_err(|err| err.to_string())
    }

    fn get_text(&mut self) -> Result<String, String> {
        Self::get_text(self).map_err(|err| err.to_string())
    }

    fn set_text(&mut self, text: String) -> Result<(), String> {
        Self::set_text(self, text).map_err(|err| err.to_string())
    }
}

/// Holds one connection for as long as it keeps working.
///
/// The connection is opened lazily on first use. A failed connect leaves no
/// connection, so the next call retries; a failed operation drops the
/// connection, so the next call reconnects.
struct Persistent<B: Backend> {
    /// The live connection, if one is open.
    handle: Option<B>,
}

impl<B: Backend> Default for Persistent<B> {
    fn default() -> Self {
        Self { handle: None }
    }
}

impl<B: Backend> Persistent<B> {
    /// Return the held connection, opening one if none is held.
    fn connection(&mut self) -> Option<&mut B> {
        if self.handle.is_none() {
            match B::connect() {
                Ok(handle) => self.handle = Some(handle),
                Err(err) => {
                    log::debug!("clipboard unavailable: {err}");
                    return None;
                }
            }
        }
        self.handle.as_mut()
    }

    /// Read the clipboard text, or `None` on any failure.
    fn read_text(&mut self) -> Option<String> {
        match self.connection()?.get_text() {
            Ok(text) => Some(text),
            Err(err) => {
                log::debug!("clipboard read failed: {err}");
                self.handle = None;
                None
            }
        }
    }

    /// Write `text` to the clipboard. Returns `true` on success.
    fn write_text(&mut self, text: String) -> bool {
        let Some(connection) = self.connection() else {
            return false;
        };
        match connection.set_text(text) {
            Ok(()) => true,
            Err(err) => {
                log::debug!("clipboard write failed: {err}");
                self.handle = None;
                false
            }
        }
    }
}

/// The OS clipboard, accessed through [`arboard`].
///
/// One `arboard` connection is opened on first use and kept for the life of
/// this value (normally the life of the editor). This matters on Linux: the
/// process that owns the clipboard serves its contents to other
/// applications, and dropping the last `arboard` handle hands the contents
/// to a clipboard manager if one is running and otherwise loses them. The
/// connection is dropped, and reopened on the next call, only after an
/// operation through it fails.
#[derive(Default)]
pub struct SystemClipboard {
    /// The retained OS clipboard connection.
    inner: Persistent<arboard::Clipboard>,
}

impl SystemClipboard {
    /// Create a system clipboard. No connection is opened until first use.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl std::fmt::Debug for SystemClipboard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SystemClipboard")
            .field("connected", &self.inner.handle.is_some())
            .finish()
    }
}

impl Clipboard for SystemClipboard {
    fn read_text(&mut self) -> Option<String> {
        self.inner.read_text()
    }

    fn write_text(&mut self, text: String) -> bool {
        self.inner.write_text(text)
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

    use std::cell::Cell;

    thread_local! {
        static CONNECTS: Cell<usize> = const { Cell::new(0) };
        static FAIL_CONNECT: Cell<bool> = const { Cell::new(false) };
        static FAIL_GET: Cell<bool> = const { Cell::new(false) };
        static FAIL_SET: Cell<bool> = const { Cell::new(false) };
    }

    /// A fake OS clipboard connection; each test thread has its own counters
    /// and failure switches.
    struct FakeBackend {
        text: String,
    }

    impl Backend for FakeBackend {
        fn connect() -> Result<Self, String> {
            CONNECTS.with(|c| c.set(c.get() + 1));
            if FAIL_CONNECT.with(Cell::get) {
                return Err("no display".to_owned());
            }
            Ok(Self {
                text: String::new(),
            })
        }

        fn get_text(&mut self) -> Result<String, String> {
            if FAIL_GET.with(Cell::get) {
                return Err("read error".to_owned());
            }
            Ok(self.text.clone())
        }

        fn set_text(&mut self, text: String) -> Result<(), String> {
            if FAIL_SET.with(Cell::get) {
                return Err("write error".to_owned());
            }
            self.text = text;
            Ok(())
        }
    }

    fn connects() -> usize {
        CONNECTS.with(Cell::get)
    }

    #[test]
    fn the_connection_is_opened_lazily_and_reused() {
        let mut clipboard = Persistent::<FakeBackend>::default();
        assert_eq!(connects(), 0);
        assert!(clipboard.write_text("one".to_owned()));
        assert!(clipboard.write_text("two".to_owned()));
        assert_eq!(clipboard.read_text().as_deref(), Some("two"));
        assert!(clipboard.write_text("three".to_owned()));
        assert_eq!(clipboard.read_text().as_deref(), Some("three"));
        assert_eq!(connects(), 1);
    }

    #[test]
    fn a_failed_connect_is_retried_on_the_next_call() {
        let mut clipboard = Persistent::<FakeBackend>::default();
        FAIL_CONNECT.with(|f| f.set(true));
        assert!(!clipboard.write_text("lost".to_owned()));
        assert_eq!(clipboard.read_text(), None);
        assert!(clipboard.handle.is_none());
        FAIL_CONNECT.with(|f| f.set(false));
        assert!(clipboard.write_text("kept".to_owned()));
        assert_eq!(clipboard.read_text().as_deref(), Some("kept"));
        assert_eq!(connects(), 3);
    }

    #[test]
    fn a_failed_operation_drops_the_connection_and_reconnects() {
        let mut clipboard = Persistent::<FakeBackend>::default();
        assert!(clipboard.write_text("first".to_owned()));
        assert_eq!(connects(), 1);
        FAIL_SET.with(|f| f.set(true));
        assert!(!clipboard.write_text("fails".to_owned()));
        assert!(clipboard.handle.is_none());
        FAIL_SET.with(|f| f.set(false));
        assert!(clipboard.write_text("second".to_owned()));
        assert_eq!(connects(), 2);
        assert_eq!(clipboard.read_text().as_deref(), Some("second"));
        assert_eq!(connects(), 2);
    }

    #[test]
    fn a_failed_read_drops_the_connection_and_reconnects() {
        let mut clipboard = Persistent::<FakeBackend>::default();
        assert!(clipboard.write_text("first".to_owned()));
        FAIL_GET.with(|f| f.set(true));
        assert_eq!(clipboard.read_text(), None);
        assert!(clipboard.handle.is_none());
        FAIL_GET.with(|f| f.set(false));
        assert!(clipboard.write_text("second".to_owned()));
        assert_eq!(clipboard.read_text().as_deref(), Some("second"));
        assert_eq!(connects(), 2);
    }

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
