//! Every user intent a front end can send to the [`Editor`](super::Editor).

use std::path::PathBuf;

use super::prompt::CloseChoice;

/// A request to change editor state, applied with
/// [`Editor::apply`](super::Editor::apply).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    // Tabs.
    /// Open a new, empty, untitled tab and focus it.
    NewTab,
    /// Focus the tab at this index (ignored when out of range).
    SelectTab(usize),
    /// Close the tab at this index; a dirty tab raises
    /// [`Prompt::ConfirmClose`](super::Prompt::ConfirmClose) instead.
    RequestClose(usize),
    /// [`Command::RequestClose`] for the active tab.
    RequestCloseActive,
    /// Open this file in a tab (or focus the tab already on it).
    OpenPath(PathBuf),
    /// Save the active tab; an untitled tab raises
    /// [`Prompt::SavePath`](super::Prompt::SavePath).
    Save,
    // Prompts.
    /// Answer a pending [`Prompt::ConfirmClose`](super::Prompt::ConfirmClose).
    ResolveClose(CloseChoice),
    /// Answer a pending [`Prompt::SavePath`](super::Prompt::SavePath) with
    /// the chosen path.
    SubmitSavePath(PathBuf),
    /// Dismiss whatever prompt is pending.
    CancelPrompt,
}
