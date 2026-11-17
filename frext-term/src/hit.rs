//! Mouse hit testing.
//!
//! Every frame, rendering records the clickable regions it drew into a
//! [`HitMap`]. Mouse events are then resolved against the previous
//! frame's map, so widgets can register regions without the app knowing
//! their geometry.

use frext_core::editor::SearchOption;
use ratatui::layout::{Position, Rect};

/// Something on screen that a mouse event can land on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// The "new tab" button.
    NewTab,
    /// The "open file" button.
    OpenFile,
    /// The "save" button.
    Save,
    /// The title of the tab at this index.
    SelectTab(usize),
    /// The close glyph of the tab at this index.
    CloseTab(usize),
    /// The body of a prompt popup.
    PromptPopup,
    /// The text input of a prompt popup.
    PromptInput,
    /// The prompt button at this index.
    PromptButton(usize),
    /// The completion candidate at this index.
    PromptCandidate(usize),
    /// The editor body.
    Editor,
    /// The body of a context menu.
    MenuPopup,
    /// The context-menu entry at this index.
    MenuItem(usize),
    /// The find field of the find bar.
    FindPattern,
    /// The "With" field of the find bar.
    FindReplacement,
    /// A search-option toggle on the find bar.
    FindOption(SearchOption),
    /// The find bar's "previous match" button.
    FindPrev,
    /// The find bar's "next match" button.
    FindNext,
    /// The find bar's replace-row toggle.
    FindReplaceRow,
    /// The find bar's close button.
    FindClose,
    /// The find bar's "replace the current match" button.
    FindReplaceCurrent,
    /// The find bar's "replace all matches" button.
    FindReplaceAll,
}

impl Target {
    /// The status-line hint shown while the mouse hovers this target.
    #[must_use]
    pub const fn hint(self) -> Option<&'static str> {
        match self {
            Self::NewTab => Some("New tab (Ctrl+N)"),
            Self::OpenFile => Some("Open file (Ctrl+O)"),
            Self::Save => Some("Save (Ctrl+S)"),
            Self::CloseTab(_) => Some("Close tab (Ctrl+W)"),
            Self::FindOption(SearchOption::CaseSensitive) => Some("Match case (Alt+C)"),
            Self::FindOption(SearchOption::WholeWord) => Some("Whole word (Alt+W)"),
            Self::FindOption(SearchOption::Regex) => Some("Regular expression (Alt+R)"),
            Self::FindOption(SearchOption::InSelection) => {
                Some("Search within the current selection (Alt+L)")
            }
            Self::FindPrev => Some("Previous match (Shift+F3)"),
            Self::FindNext => Some("Next match (F3 / Enter)"),
            Self::FindReplaceRow => Some("Toggle replace"),
            Self::FindClose => Some("Close (Esc)"),
            Self::FindReplaceCurrent => Some("Replace the current match (Enter in With)"),
            Self::FindReplaceAll => Some("Replace all matches"),
            Self::SelectTab(_)
            | Self::PromptPopup
            | Self::PromptInput
            | Self::PromptButton(_)
            | Self::PromptCandidate(_)
            | Self::Editor
            | Self::MenuPopup
            | Self::MenuItem(_)
            | Self::FindPattern
            | Self::FindReplacement => None,
        }
    }
}

/// The clickable regions recorded during one frame.
#[derive(Debug, Default)]
pub struct HitMap {
    regions: Vec<(Rect, Target)>,
}

impl HitMap {
    /// Removes every recorded region.
    pub fn clear(&mut self) {
        self.regions.clear();
    }

    /// Records a region. Later regions take priority over earlier ones.
    pub fn push(&mut self, rect: Rect, target: Target) {
        self.regions.push((rect, target));
    }

    /// The target of the last pushed region containing the cell at
    /// (`column`, `row`), if any.
    #[must_use]
    pub fn hit(&self, column: u16, row: u16) -> Option<Target> {
        let point = Position { x: column, y: row };
        self.regions
            .iter()
            .rev()
            .find(|(rect, _)| rect.contains(point))
            .map(|&(_, target)| target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_returns_containing_region() {
        let mut map = HitMap::default();
        map.push(Rect::new(0, 0, 3, 1), Target::NewTab);
        map.push(Rect::new(3, 0, 6, 1), Target::OpenFile);
        assert_eq!(map.hit(0, 0), Some(Target::NewTab));
        assert_eq!(map.hit(2, 0), Some(Target::NewTab));
        assert_eq!(map.hit(3, 0), Some(Target::OpenFile));
    }

    #[test]
    fn overlap_resolves_to_last_pushed() {
        let mut map = HitMap::default();
        map.push(Rect::new(0, 0, 80, 24), Target::Editor);
        map.push(Rect::new(10, 5, 20, 5), Target::PromptPopup);
        map.push(Rect::new(12, 7, 10, 1), Target::PromptInput);
        assert_eq!(map.hit(12, 7), Some(Target::PromptInput));
        assert_eq!(map.hit(11, 6), Some(Target::PromptPopup));
        assert_eq!(map.hit(0, 0), Some(Target::Editor));
    }

    #[test]
    fn miss_is_none() {
        let mut map = HitMap::default();
        assert_eq!(map.hit(0, 0), None);
        map.push(Rect::new(5, 5, 2, 2), Target::Save);
        assert_eq!(map.hit(7, 5), None);
        assert_eq!(map.hit(4, 5), None);
        assert_eq!(map.hit(5, 7), None);
    }

    #[test]
    fn clear_removes_regions() {
        let mut map = HitMap::default();
        map.push(Rect::new(0, 0, 1, 1), Target::Save);
        map.clear();
        assert_eq!(map.hit(0, 0), None);
    }

    #[test]
    fn hints() {
        assert_eq!(Target::NewTab.hint(), Some("New tab (Ctrl+N)"));
        assert_eq!(Target::OpenFile.hint(), Some("Open file (Ctrl+O)"));
        assert_eq!(Target::Save.hint(), Some("Save (Ctrl+S)"));
        assert_eq!(Target::CloseTab(3).hint(), Some("Close tab (Ctrl+W)"));
        assert_eq!(Target::SelectTab(0).hint(), None);
        assert_eq!(Target::PromptPopup.hint(), None);
        assert_eq!(Target::PromptInput.hint(), None);
        assert_eq!(Target::PromptButton(1).hint(), None);
        assert_eq!(Target::PromptCandidate(2).hint(), None);
        assert_eq!(Target::Editor.hint(), None);
        assert_eq!(Target::MenuPopup.hint(), None);
        assert_eq!(Target::MenuItem(4).hint(), None);
    }

    #[test]
    fn find_bar_hints() {
        assert_eq!(Target::FindPattern.hint(), None);
        assert_eq!(Target::FindReplacement.hint(), None);
        assert_eq!(
            Target::FindOption(SearchOption::CaseSensitive).hint(),
            Some("Match case (Alt+C)")
        );
        assert_eq!(
            Target::FindOption(SearchOption::WholeWord).hint(),
            Some("Whole word (Alt+W)")
        );
        assert_eq!(
            Target::FindOption(SearchOption::Regex).hint(),
            Some("Regular expression (Alt+R)")
        );
        assert_eq!(
            Target::FindOption(SearchOption::InSelection).hint(),
            Some("Search within the current selection (Alt+L)")
        );
        assert_eq!(Target::FindPrev.hint(), Some("Previous match (Shift+F3)"));
        assert_eq!(Target::FindNext.hint(), Some("Next match (F3 / Enter)"));
        assert_eq!(Target::FindReplaceRow.hint(), Some("Toggle replace"));
        assert_eq!(Target::FindClose.hint(), Some("Close (Esc)"));
        assert_eq!(
            Target::FindReplaceCurrent.hint(),
            Some("Replace the current match (Enter in With)")
        );
        assert_eq!(Target::FindReplaceAll.hint(), Some("Replace all matches"));
    }

    #[test]
    fn find_option_region_resolves() {
        let mut map = HitMap::default();
        map.push(Rect::new(0, 0, 80, 24), Target::Editor);
        map.push(
            Rect::new(40, 0, 3, 1),
            Target::FindOption(SearchOption::Regex),
        );
        assert_eq!(
            map.hit(41, 0),
            Some(Target::FindOption(SearchOption::Regex))
        );
        assert_eq!(map.hit(43, 0), Some(Target::Editor));
    }
}
