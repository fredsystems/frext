//! What applying a [`Command`](super::Command) did: whether the active
//! buffer changed, and the view-state effects the front end must apply.

use std::ops::Range;

/// The result of [`Editor::apply`](super::Editor::apply).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    /// Whether the command edited the active buffer.
    pub buffer: BufferChange,
    /// View-state changes the front end should apply on its next frame, in
    /// order.
    pub effects: Vec<Effect>,
}

/// Whether a command edited the active buffer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BufferChange {
    /// The buffer was not touched.
    #[default]
    Unchanged,
    /// The buffer's text was modified.
    Edited,
}

/// A change to view state that only the front end can make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Set the editor's selection (a char range; empty = caret).
    SetSelection {
        /// The tab whose editor the effect applies to; a front end ignores
        /// it for any other tab.
        tab_id: u64,
        /// The selected char range; an empty range is a collapsed caret.
        chars: Range<usize>,
        /// Whether the editor should also take keyboard focus.
        focus: EditorFocus,
    },
    /// Scroll the editor so its selection is visible, centred.
    RevealSelection {
        /// The tab whose editor the effect applies to; a front end ignores
        /// it for any other tab.
        tab_id: u64,
    },
    /// Give keyboard focus to the find field.
    FocusSearchField,
    /// Give keyboard focus to the active prompt's text field.
    FocusPromptField,
}

/// Whether an [`Effect::SetSelection`] also focuses the editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorFocus {
    /// Give the editor keyboard focus.
    Focus,
    /// Leave keyboard focus where it is.
    Keep,
}
