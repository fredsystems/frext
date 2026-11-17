//! The editor's right-click menu.
//!
//! [`entries`] mirrors the GUI's `editor_context_menu` in `frext`: the
//! same items, labels and order. With a proofreading diagnostic under
//! the click, the menu leads with its message, its fixes, "Add to
//! dictionary" (spelling only) and "Ignore", then the editing items.

use frext_core::editor::{Command, SearchBar};
use frext_core::proofread::{Diagnostic, DiagnosticKind};

use crate::menu::{Enablement, MenuEntry};
use crate::popup;

/// The widest a line of the diagnostic message is in the menu, in
/// display columns. Longer messages wrap onto several rows.
pub const MESSAGE_WIDTH: usize = 40;

/// The editor menu. With `diagnostic`, it starts with the diagnostic's
/// message (wrapped to [`MESSAGE_WIDTH`]), one item per suggestion (or a
/// disabled "No suggestions"), a separator, "Add to dictionary" for a
/// spelling diagnostic, "Ignore", and a separator. Then, always: Cut and
/// Copy (enabled per `selection`), Paste, Select all, a separator, Find…
/// and Find and replace…, a separator, then Save.
#[must_use]
pub fn entries(selection: Enablement, diagnostic: Option<&Diagnostic>) -> Vec<MenuEntry<Command>> {
    let mut entries = diagnostic.map(diagnostic_entries).unwrap_or_default();
    entries.extend(editing_entries(selection));
    entries
}

/// The proofreading items for `diagnostic`, ending with a separator.
fn diagnostic_entries(diagnostic: &Diagnostic) -> Vec<MenuEntry<Command>> {
    let mut entries: Vec<MenuEntry<Command>> = popup::wrap(&diagnostic.message, MESSAGE_WIDTH)
        .into_iter()
        .map(MenuEntry::Note)
        .collect();
    if diagnostic.suggestions.is_empty() {
        entries.push(item(
            "No suggestions",
            Enablement::Disabled,
            Command::Ignore(diagnostic.ignore_key()),
        ));
    } else {
        entries.extend(diagnostic.suggestions.iter().map(|suggestion| {
            let label = if suggestion.is_empty() {
                format!("Remove \u{201c}{}\u{201d}", diagnostic.flagged)
            } else {
                suggestion.clone()
            };
            item(
                &label,
                Enablement::Enabled,
                Command::ApplySuggestion {
                    bytes: diagnostic.bytes.clone(),
                    flagged: diagnostic.flagged.clone(),
                    replacement: suggestion.clone(),
                },
            )
        }));
    }
    entries.push(MenuEntry::Separator);
    if diagnostic.kind == DiagnosticKind::Spelling {
        entries.push(item(
            &format!("Add \u{201c}{}\u{201d} to dictionary", diagnostic.flagged),
            Enablement::Enabled,
            Command::AddToDictionary(diagnostic.flagged.clone()),
        ));
    }
    entries.push(item(
        "Ignore",
        Enablement::Enabled,
        Command::Ignore(diagnostic.ignore_key()),
    ));
    entries.push(MenuEntry::Separator);
    entries
}

/// The editing items: Cut and Copy (enabled per `selection`), Paste,
/// Select all, a separator, Find… and Find and replace…, a separator,
/// then Save.
fn editing_entries(selection: Enablement) -> Vec<MenuEntry<Command>> {
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
    use unicode_width::UnicodeWidthStr;

    use super::*;

    fn test_item(label: &str, enabled: Enablement, value: Command) -> MenuEntry<Command> {
        MenuEntry::Item {
            label: label.to_owned(),
            enabled,
            value,
        }
    }

    fn expected(selection: Enablement) -> Vec<MenuEntry<Command>> {
        let item = test_item;
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

    fn diagnostic(kind: DiagnosticKind, message: &str, suggestions: &[&str]) -> Diagnostic {
        Diagnostic {
            kind,
            bytes: 4..9,
            chars: 4..9,
            flagged: "wrrod".to_owned(),
            message: message.to_owned(),
            suggestions: suggestions.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    fn apply(diagnostic: &Diagnostic, replacement: &str) -> Command {
        Command::ApplySuggestion {
            bytes: diagnostic.bytes.clone(),
            flagged: diagnostic.flagged.clone(),
            replacement: replacement.to_owned(),
        }
    }

    fn items(entries: &[MenuEntry<Command>]) -> Vec<(&str, Enablement, &Command)> {
        entries
            .iter()
            .filter_map(|entry| match entry {
                MenuEntry::Item {
                    label,
                    enabled,
                    value,
                } => Some((label.as_str(), *enabled, value)),
                _ => None,
            })
            .collect()
    }

    fn notes(entries: &[MenuEntry<Command>]) -> Vec<&str> {
        entries
            .iter()
            .filter_map(|entry| match entry {
                MenuEntry::Note(text) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn with_a_selection_every_item_is_enabled() {
        assert_eq!(
            entries(Enablement::Enabled, None),
            expected(Enablement::Enabled)
        );
    }

    #[test]
    fn without_a_selection_cut_and_copy_are_disabled() {
        let entries = entries(Enablement::Disabled, None);
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

    #[test]
    fn a_spelling_diagnostic_leads_the_menu() {
        let d = diagnostic(DiagnosticKind::Spelling, "Did you mean?", &["the", "tea"]);
        let mut want = vec![
            MenuEntry::Note("Did you mean?".to_owned()),
            test_item("the", Enablement::Enabled, apply(&d, "the")),
            test_item("tea", Enablement::Enabled, apply(&d, "tea")),
            MenuEntry::Separator,
            test_item(
                "Add \u{201c}wrrod\u{201d} to dictionary",
                Enablement::Enabled,
                Command::AddToDictionary("wrrod".to_owned()),
            ),
            test_item(
                "Ignore",
                Enablement::Enabled,
                Command::Ignore(d.ignore_key()),
            ),
            MenuEntry::Separator,
        ];
        want.extend(expected(Enablement::Disabled));
        assert_eq!(entries(Enablement::Disabled, Some(&d)), want);
    }

    #[test]
    fn an_empty_suggestion_is_labelled_remove() {
        let d = diagnostic(DiagnosticKind::Grammar, "Remove this.", &[""]);
        let entries = entries(Enablement::Enabled, Some(&d));
        let remove = apply(&d, "");
        assert!(items(&entries).contains(&(
            "Remove \u{201c}wrrod\u{201d}",
            Enablement::Enabled,
            &remove
        )));
    }

    #[test]
    fn grammar_has_no_add_to_dictionary() {
        let d = diagnostic(DiagnosticKind::Grammar, "Odd phrasing.", &["the"]);
        let entries = entries(Enablement::Enabled, Some(&d));
        let items = items(&entries);
        assert!(
            !items
                .iter()
                .any(|(_, _, value)| matches!(value, Command::AddToDictionary(_)))
        );
        assert!(!items.iter().any(|(label, ..)| label.starts_with("Add ")));
        assert!(items.iter().any(|(label, ..)| *label == "Ignore"));
    }

    #[test]
    fn no_suggestions_gives_one_disabled_item() {
        let d = diagnostic(DiagnosticKind::Spelling, "Unknown word.", &[]);
        let entries = entries(Enablement::Enabled, Some(&d));
        let items = items(&entries);
        let no_suggestions: Vec<_> = items
            .iter()
            .filter(|(label, ..)| *label == "No suggestions")
            .collect();
        assert_eq!(no_suggestions.len(), 1);
        assert_eq!(no_suggestions[0].1, Enablement::Disabled);
        assert!(
            !items
                .iter()
                .any(|(_, _, value)| matches!(value, Command::ApplySuggestion { .. }))
        );
    }

    #[test]
    fn a_long_message_wraps_into_several_notes() {
        let message = "This sentence is rather long and keeps going well past the \
                       width that the menu allows for a single line of text.";
        assert!(message.width() > MESSAGE_WIDTH);
        let d = diagnostic(DiagnosticKind::Grammar, message, &["x"]);
        let entries = entries(Enablement::Enabled, Some(&d));
        let notes = notes(&entries);
        assert!(notes.len() > 1);
        assert!(notes.iter().all(|line| line.width() <= MESSAGE_WIDTH));
        let joined: Vec<&str> = notes.iter().flat_map(|l| l.split_whitespace()).collect();
        let words: Vec<&str> = message.split_whitespace().collect();
        assert_eq!(joined.join(" "), words.join(" "));
        assert!(
            entries[..notes.len()]
                .iter()
                .all(|entry| matches!(entry, MenuEntry::Note(_)))
        );
    }
}
