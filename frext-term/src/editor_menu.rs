//! The editor's right-click menu.
//!
//! [`entries`] mirrors the GUI's `editor_context_menu` in `frext`: the
//! same items, labels and order. Plan 07 prepends the proofreading
//! diagnostic items to it.

use frext_core::editor::{Command, SearchBar};

use crate::menu::{Enablement, MenuEntry};

/// The editor menu: Cut and Copy (enabled per `selection`), Paste,
/// Select all, a separator, Find… and Find and replace…, a separator,
/// then Save.
#[must_use]
pub fn entries(selection: Enablement) -> Vec<MenuEntry<Command>> {
    vec![
        item("Cut", selection, Command::Cut),
        item("Copy", selection, Command::Copy),
        item("Paste", Enablement::Enabled, Command::Paste),
        item("Select all", Enablement::Enabled, Command::SelectAll),
        MenuEntry::Separator,
        item(
            "Find\u{2026}",
            Enablement::Enabled,
            Command::OpenSearch(SearchBar::Find),
        ),
        item(
            "Find and replace\u{2026}",
            Enablement::Enabled,
            Command::OpenSearch(SearchBar::FindReplace),
        ),
        MenuEntry::Separator,
        item("Save", Enablement::Enabled, Command::Save),
    ]
}

/// A menu item labelled `label`.
fn item(label: &str, enabled: Enablement, value: Command) -> MenuEntry<Command> {
    MenuEntry::Item {
        label: label.to_owned(),
        enabled,
        value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expected(selection: Enablement) -> Vec<MenuEntry<Command>> {
        let item = |label: &str, enabled, value| MenuEntry::Item {
            label: label.to_owned(),
            enabled,
            value,
        };
        vec![
            item("Cut", selection, Command::Cut),
            item("Copy", selection, Command::Copy),
            item("Paste", Enablement::Enabled, Command::Paste),
            item("Select all", Enablement::Enabled, Command::SelectAll),
            MenuEntry::Separator,
            item(
                "Find…",
                Enablement::Enabled,
                Command::OpenSearch(SearchBar::Find),
            ),
            item(
                "Find and replace…",
                Enablement::Enabled,
                Command::OpenSearch(SearchBar::FindReplace),
            ),
            MenuEntry::Separator,
            item("Save", Enablement::Enabled, Command::Save),
        ]
    }

    #[test]
    fn with_a_selection_every_item_is_enabled() {
        assert_eq!(entries(Enablement::Enabled), expected(Enablement::Enabled));
    }

    #[test]
    fn without_a_selection_cut_and_copy_are_disabled() {
        let entries = entries(Enablement::Disabled);
        assert_eq!(entries, expected(Enablement::Disabled));
        let disabled: Vec<&str> = entries
            .iter()
            .filter_map(|entry| match entry {
                MenuEntry::Item {
                    label,
                    enabled: Enablement::Disabled,
                    ..
                } => Some(label.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(disabled, vec!["Cut", "Copy"]);
    }
}
