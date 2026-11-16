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
    /// Answer a pending [`Prompt::Name`](super::Prompt::Name) with the
    /// name currently entered. A failure keeps the prompt open with its
    /// error set.
    SubmitName,
    // Workspace.
    /// Record a sidebar folder as expanded or collapsed.
    SetExpanded {
        /// The folder whose state changed.
        dir: PathBuf,
        /// Its new state.
        state: FolderState,
    },
    /// Close the sidebar workspace and drop it from the session.
    CloseWorkspace,
    /// Copy this path, absolute, to the clipboard.
    CopyPath(PathBuf),
    /// Copy this path, relative to the workspace root, to the clipboard.
    CopyRelativePath(PathBuf),
    /// Open this path (or its containing directory) in the OS file manager.
    Reveal(PathBuf),
    /// Begin renaming this path: raises [`Prompt::Name`](super::Prompt::Name).
    BeginRename(PathBuf),
    /// Begin creating a new file inside this directory: raises
    /// [`Prompt::Name`](super::Prompt::Name).
    BeginNewFile(PathBuf),
    /// Begin creating a new folder inside this directory: raises
    /// [`Prompt::Name`](super::Prompt::Name).
    BeginNewFolder(PathBuf),
    /// Move this path to the OS trash, closing any tab on a file at or
    /// under it.
    Trash(PathBuf),
    // Search.
    /// Show the find bar (seeding the pattern from a single-line selection)
    /// and focus its find field.
    OpenSearch(SearchBar),
    /// Hide the find bar, clearing its matches, and persist the query.
    CloseSearch,
    /// Focus the next or previous match, wrapping around, and select it.
    StepMatch(StepDirection),
    /// Re-run the search after the pattern was edited.
    RecomputeMatches,
    /// Flip one search toggle and re-run the search.
    ToggleSearchOption(SearchOption),
    /// Show or hide the replace row.
    ToggleReplaceRow,
    /// Replace the focused match with the replacement text.
    ReplaceCurrent,
    /// Replace every match in the active buffer.
    ReplaceAll,
}

/// A sidebar folder's open/closed state, set by [`Command::SetExpanded`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderState {
    /// The folder's contents are shown.
    Expanded,
    /// The folder's contents are hidden.
    Collapsed,
}

/// Which rows of the search bar [`Command::OpenSearch`] shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchBar {
    /// The find row only.
    Find,
    /// The find row and the replace row.
    FindReplace,
}

/// The direction [`Command::StepMatch`] moves in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepDirection {
    /// To the next match.
    Forward,
    /// To the previous match.
    Backward,
}

/// A search toggle flipped by [`Command::ToggleSearchOption`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchOption {
    /// Match case exactly.
    CaseSensitive,
    /// Require word boundaries around each match.
    WholeWord,
    /// Treat the pattern as a regular expression.
    Regex,
    /// Restrict matches to the editor's current selection.
    InSelection,
}
