//! A thin wrapper over the OS clipboard for the editor's cut/copy/paste menu.
//!
//! egui can *write* the clipboard (via `Context::copy_text`) but exposes no
//! imperative *read*, which a menu-driven "Paste" needs. This module uses
//! [`arboard`] for both directions so the editor clipboard actions are
//! deterministic and testable, rather than depending on egui's event timing.
//!
//! Every operation is fallible and non-panicking: a clipboard that cannot be
//! opened (headless CI, no display server) yields `None`/`false` and logs,
//! leaving the editor usable.

/// Read the current clipboard text, or `None` when the clipboard is
/// unavailable or holds no text.
#[must_use]
pub fn read_text() -> Option<String> {
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

/// Write `text` to the clipboard. Returns `true` on success.
pub fn write_text(text: String) -> bool {
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
