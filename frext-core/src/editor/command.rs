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
