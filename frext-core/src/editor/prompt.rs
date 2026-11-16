//! The single modal prompt the editor may be waiting on.
//!
//! Holding at most one [`Prompt`] makes two simultaneous modals
//! unrepresentable. Prompts refer to tabs by **id**, not index, so a
//! reordered or closed tab can never be confused with another.

use std::path::PathBuf;

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
    /// A name is being entered for a rename, new file or new folder.
    /// Answered with [`Command::SubmitName`](super::Command::SubmitName) or
    /// [`Command::CancelPrompt`](super::Command::CancelPrompt).
    Name(NamePrompt),
}

/// A pending filesystem name-entry prompt (rename / new file / new folder).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamePrompt {
    /// Which operation the entered name applies to.
    pub kind: NameKind,
    /// The path the operation targets: the entry being renamed, or the
    /// directory a new file/folder is created inside.
    pub target: PathBuf,
    /// The live text of the name field.
    pub name: String,
    /// An error message from the previous attempt, shown inline.
    pub error: Option<String>,
}

/// The operation a [`NamePrompt`] collects a name for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameKind {
    /// Rename `target` to the entered name.
    Rename,
    /// Create a new file named by the entry inside `target` (a directory).
    NewFile,
    /// Create a new folder named by the entry inside `target` (a directory).
    NewFolder,
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
