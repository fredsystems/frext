//! The single modal prompt the editor may be waiting on.
//!
//! Holding at most one [`Prompt`] makes two simultaneous modals
//! unrepresentable. Prompts refer to tabs by **id**, not index, so a
//! reordered or closed tab can never be confused with another.

/// A question the editor is waiting for the user to answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prompt {
    /// The tab `tab_id` has unsaved changes and a close was requested:
    /// save, discard, or cancel? Answered with
    /// [`Command::ResolveClose`](super::Command::ResolveClose).
    ConfirmClose {
        /// The id of the tab being closed.
        tab_id: u64,
    },
    /// The untitled tab `tab_id` is being saved and needs a path. Answered
    /// with [`Command::SubmitSavePath`](super::Command::SubmitSavePath) or
    /// [`Command::CancelPrompt`](super::Command::CancelPrompt).
    SavePath {
        /// The id of the tab being saved.
        tab_id: u64,
        /// What to do once the save succeeds.
        then: AfterSave,
    },
}

/// What happens after a [`Prompt::SavePath`] save succeeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AfterSave {
    /// Keep the tab open.
    Nothing,
    /// Close the tab (the "Save and close" path).
    CloseTab,
}

/// The user's answer to a [`Prompt::ConfirmClose`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseChoice {
    /// Save the buffer, then close the tab.
    Save,
    /// Close the tab without saving.
    Discard,
    /// Keep the tab open.
    Cancel,
}
