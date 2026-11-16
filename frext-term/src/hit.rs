//! Mouse hit testing.
//!
//! Every frame, rendering records the clickable regions it drew into a
//! [`HitMap`]. Mouse events are then resolved against the previous
//! frame's map, so widgets can register regions without the app knowing
//! their geometry.

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
            Self::SelectTab(_)
            | Self::PromptPopup
            | Self::PromptInput
            | Self::PromptButton(_)
            | Self::PromptCandidate(_)
            | Self::Editor => None,
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
    }
}
