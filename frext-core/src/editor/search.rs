//! Find/replace over the active buffer: the live search state, and the
//! [`Editor`] methods that open, step, replace and close a search.

use std::ops::Range;

use super::{
    BufferChange, Editor, EditorFocus, Effect, Outcome, SearchBar, SearchOption, StepDirection,
};
use crate::{
    edit_ops,
    highlight::{MatchHighlights, MatchHighlightsRef},
    offsets,
    search::SearchQuery,
};

/// Live state for the find/replace bar. Only the query itself survives
/// across sessions.
///
/// Search-within-selection works on a scope captured from the editor's
/// selection when the option is turned on (or the bar is reopened with it
/// on), not on the live selection: stepping to a match selects it, and that
/// must not shrink the scope. Edits to the buffer move the scope with the
/// text it covers.
#[derive(Debug, Default)]
pub struct SearchState {
    /// The search query and its toggles (persisted in the session).
    query: SearchQuery,
    /// Whether the find/replace bar is currently shown.
    open: bool,
    /// Whether the replace row is shown.
    replace_open: bool,
    /// The replacement text for find/replace.
    replacement: String,
    /// Restrict matches to the captured selection `scope` when `true`.
    in_selection: bool,
    /// The selection search-within-selection is restricted to, captured when
    /// the option was turned on; `None` when there was no selection to
    /// capture.
    scope: Option<SearchScope>,
    /// Cached match byte ranges for the active buffer (recomputed whenever
    /// the query, the toggles or the buffer change).
    matches: Vec<Range<usize>>,
    /// Index into `matches` of the focused match, if any.
    current: Option<usize>,
    /// The most recent regex-compile error message, shown inline.
    error: Option<String>,
}

/// A captured search-within-selection scope.
#[derive(Debug)]
struct SearchScope {
    /// The id of the tab the scope belongs to.
    tab_id: u64,
    /// The scope as a char range of `base`.
    chars: Range<usize>,
    /// The tab's text `chars` was last mapped against; when the buffer no
    /// longer matches it, `chars` is moved through the edit.
    base: String,
}

impl SearchState {
    /// A closed search bar holding a query restored from the session.
    pub(super) fn restored(query: SearchQuery) -> Self {
        Self {
            query,
            ..Self::default()
        }
    }

    /// Whether the find/replace bar is currently shown.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Whether the replace row is shown.
    #[must_use]
    pub fn replace_open(&self) -> bool {
        self.replace_open
    }

    /// Whether matches are restricted to the selection captured when the
    /// option was turned on.
    #[must_use]
    pub fn in_selection(&self) -> bool {
        self.in_selection
    }

    /// Whether the search option `option` is on: the case, whole-word and
    /// regex toggles of the query, or search-within-selection.
    #[must_use]
    pub fn option(&self, option: SearchOption) -> bool {
        match option {
            SearchOption::CaseSensitive => self.query.case_sensitive,
            SearchOption::WholeWord => self.query.whole_word,
            SearchOption::Regex => self.query.regex,
            SearchOption::InSelection => self.in_selection,
        }
    }

    /// The search query and its toggles.
    #[must_use]
    pub fn query(&self) -> &SearchQuery {
        &self.query
    }

    /// The replacement text for find/replace.
    #[must_use]
    pub fn replacement(&self) -> &str {
        &self.replacement
    }

    /// The match byte ranges in the active buffer, in order.
    #[must_use]
    pub fn matches(&self) -> &[Range<usize>] {
        &self.matches
    }

    /// Index into [`Self::matches`] of the focused match, if any.
    #[must_use]
    pub fn current(&self) -> Option<usize> {
        self.current
    }

    /// The most recent regex-compile error message, if the query is invalid.
    #[must_use]
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// A short "current/total" (or "no results") label for the find bar;
    /// empty while the query is empty.
    #[must_use]
    pub fn count_label(&self) -> String {
        if self.query.is_empty() {
            String::new()
        } else if self.matches.is_empty() {
            "no results".to_owned()
        } else {
            let current = self.current.map_or(0, |i| i + 1);
            format!("{current}/{}", self.matches.len())
        }
    }
}

impl Editor {
    /// Open the find bar, seeding the pattern from the editor's current
    /// selection when there is a non-empty, single-line one, and request
    /// keyboard focus for the find field. While search-within-selection is
    /// on, a non-empty selection also becomes the new scope.
    pub(super) fn open_search(&mut self, bar: SearchBar) -> Outcome {
        if let Some(selected) = self.active_selection_text()
            && !selected.is_empty()
            && !selected.contains('\n')
        {
            // A single-line selection is a sensible search seed; multi-line
            // selections are left alone (they are usually a scope, not a
            // pattern).
            self.search.query.pattern = selected;
            // Plain mode for a seeded literal, so regex metacharacters in
            // the selected text match literally.
            self.search.query.regex = false;
        }
        if self.search.in_selection
            && let Some(scope) = self.capture_scope()
        {
            self.search.scope = Some(scope);
        }
        self.search.open = true;
        self.search.replace_open = bar == SearchBar::FindReplace;
        self.recompute_matches();
        Outcome {
            buffer: BufferChange::Unchanged,
            effects: vec![Effect::FocusSearchField],
        }
    }

    /// The current selection as a byte range of the active buffer.
    fn selection_bytes(&self) -> Option<Range<usize>> {
        let chars = self.active_selection()?;
        let text = &self.tabs.get(self.active)?.text;
        Some(offsets::byte_of_char(text, chars.start)..offsets::byte_of_char(text, chars.end))
    }

    /// The active buffer's currently-selected text.
    fn active_selection_text(&self) -> Option<String> {
        let range = self.selection_bytes()?;
        if range.start >= range.end {
            return None;
        }
        let tab = self.tabs.get(self.active)?;
        tab.text.get(range).map(str::to_owned)
    }

    /// The active tab's non-empty selection as a search-within-selection
    /// scope, or `None` without one.
    fn capture_scope(&self) -> Option<SearchScope> {
        let chars = self.active_selection()?;
        let chars = chars.start.min(chars.end)..chars.start.max(chars.end);
        if chars.is_empty() {
            return None;
        }
        let tab = self.tabs.get(self.active)?;
        Some(SearchScope {
            tab_id: tab.id,
            chars,
            base: tab.text.clone(),
        })
    }

    /// The captured search-within-selection scope as a byte range of the
    /// active buffer, after moving it through any edit made since it was
    /// last mapped. `None` (search the whole buffer) when the option is off,
    /// nothing was captured, the scope belongs to another tab, or it is
    /// empty.
    fn scope_bytes(&mut self) -> Option<Range<usize>> {
        if !self.search.in_selection {
            return None;
        }
        let tab = self.tabs.get(self.active)?;
        let scope = self.search.scope.as_mut()?;
        if scope.tab_id != tab.id {
            return None;
        }
        if scope.base != tab.text {
            scope.chars = edit_ops::track_range(scope.chars.clone(), &scope.base, &tab.text);
            scope.base.clone_from(&tab.text);
        }
        let start = offsets::byte_of_char(&tab.text, scope.chars.start);
        let end = offsets::byte_of_char(&tab.text, scope.chars.end);
        (start < end).then_some(start..end)
    }

    /// Close the find bar, clearing the live match cache so highlights
    /// vanish, and dropping the search-within-selection scope.
    pub(super) fn close_search(&mut self) {
        self.search.open = false;
        self.search.matches.clear();
        self.search.current = None;
        self.search.scope = None;
    }

    /// Recompute the match set for the active buffer from the current query.
    /// Updates the inline error on an invalid regex and clamps `current`.
    pub(super) fn recompute_matches(&mut self) {
        self.search.error = None;
        self.search.matches.clear();

        if self.search.query.is_empty() {
            self.search.current = None;
            return;
        }

        let scope = self.scope_bytes();
        let Some(tab) = self.tabs.get(self.active) else {
            self.search.current = None;
            return;
        };

        let matcher = match self.search.query.compile() {
            Ok(matcher) => matcher,
            Err(err) => {
                self.search.error = Some(err.to_string());
                self.search.current = None;
                return;
            }
        };

        // Search-within-selection restricts to the captured scope's bytes.
        let matches = match scope {
            Some(scope) => matcher.find_matches_in(&tab.text, scope),
            None => matcher.find_matches(&tab.text),
        };

        self.search.matches = matches;
        self.search.current = match self.search.current {
            Some(i) if i < self.search.matches.len() => Some(i),
            _ if self.search.matches.is_empty() => None,
            _ => Some(0),
        };
    }

    /// Move to the next or previous match, wrapping around, and select it in
    /// the editor (scrolled into view).
    pub(super) fn step_match(&mut self, direction: StepDirection) -> Outcome {
        let len = self.search.matches.len();
        if len == 0 {
            return Outcome::default();
        }
        let next = match (self.search.current, direction) {
            (Some(i), StepDirection::Forward) => (i + 1) % len,
            (Some(i), StepDirection::Backward) => (i + len - 1) % len,
            (None, _) => 0,
        };
        self.search.current = Some(next);
        Outcome {
            buffer: BufferChange::Unchanged,
            effects: self.select_match_effects(next),
        }
    }

    /// The effects that select match `index` in the active buffer (as a char
    /// range, keeping focus where it is) and scroll it into view; empty when
    /// there is no such match.
    fn select_match_effects(&self, index: usize) -> Vec<Effect> {
        let (Some(m), Some(tab)) = (self.search.matches.get(index), self.tabs.get(self.active))
        else {
            return Vec::new();
        };
        let chars =
            offsets::char_of_byte(&tab.text, m.start)..offsets::char_of_byte(&tab.text, m.end);
        vec![
            Effect::SetSelection {
                tab_id: tab.id,
                chars,
                focus: EditorFocus::Keep,
            },
            Effect::RevealSelection { tab_id: tab.id },
        ]
    }

    /// Flip one search toggle, then re-find. Turning search-within-selection
    /// on captures the active selection as its scope; turning it off drops
    /// the scope.
    pub(super) fn toggle_search_option(&mut self, option: SearchOption) {
        let flag = match option {
            SearchOption::CaseSensitive => &mut self.search.query.case_sensitive,
            SearchOption::WholeWord => &mut self.search.query.whole_word,
            SearchOption::Regex => &mut self.search.query.regex,
            SearchOption::InSelection => &mut self.search.in_selection,
        };
        *flag = !*flag;
        if option == SearchOption::InSelection {
            self.search.scope = if self.search.in_selection {
                self.capture_scope()
            } else {
                None
            };
        }
        self.recompute_matches();
    }

    /// Show or hide the replace row.
    pub(super) fn toggle_replace_row(&mut self) {
        self.search.replace_open = !self.search.replace_open;
    }

    /// Replace the currently-focused match with the replacement text, then
    /// re-find so the highlights and indices stay correct. When a match
    /// remains, the next one (at or after the replaced position) is selected
    /// in the editor and scrolled into view.
    pub(super) fn replace_current(&mut self) -> Outcome {
        let Some(index) = self.search.current else {
            return Outcome::default();
        };
        let Some(range) = self.search.matches.get(index).cloned() else {
            return Outcome::default();
        };
        let matcher = match self.search.query.compile() {
            Ok(matcher) => matcher,
            Err(err) => {
                self.search.error = Some(err.to_string());
                return Outcome::default();
            }
        };
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return Outcome::default();
        };

        let Some((new_text, _)) =
            matcher.replace_one_at(&tab.text, range.start, &self.search.replacement)
        else {
            return Outcome::default();
        };
        tab.text = new_text;
        self.persist_active_swap();
        // Keep the cursor near where we just replaced.
        self.recompute_matches();
        // Focus the match at (or after) the replaced position.
        if let Some(pos) = self
            .search
            .matches
            .iter()
            .position(|m| m.start >= range.start)
        {
            self.search.current = Some(pos);
        }
        let effects = self
            .search
            .current
            .map(|current| self.select_match_effects(current))
            .unwrap_or_default();
        Outcome {
            buffer: BufferChange::Edited,
            effects,
        }
    }

    /// Replace every match in the active buffer (or, with
    /// search-within-selection on, in its captured scope) with the
    /// replacement text.
    pub(super) fn replace_all(&mut self) -> BufferChange {
        if self.search.query.is_empty() {
            return BufferChange::Unchanged;
        }
        let matcher = match self.search.query.compile() {
            Ok(matcher) => matcher,
            Err(err) => {
                self.search.error = Some(err.to_string());
                return BufferChange::Unchanged;
            }
        };
        // The same scope `recompute_matches` searches within.
        let scope = self.scope_bytes();
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return BufferChange::Unchanged;
        };

        let new_text = match scope {
            Some(scope) => matcher.replace_all_in(&tab.text, scope, &self.search.replacement),
            None => matcher.replace_all(&tab.text, &self.search.replacement),
        };
        let change = if new_text == tab.text {
            BufferChange::Unchanged
        } else {
            tab.text = new_text;
            self.persist_active_swap();
            BufferChange::Edited
        };
        self.search.current = None;
        self.recompute_matches();
        change
    }

    /// The match highlights to paint over the active buffer: empty while the
    /// find bar is closed.
    #[must_use]
    pub fn match_highlights(&self) -> MatchHighlights {
        if self.search.open {
            MatchHighlights {
                ranges: self.search.matches.clone(),
                current: self.search.current,
            }
        } else {
            MatchHighlights::default()
        }
    }

    /// The match highlights to paint over the active buffer, borrowed rather
    /// than cloned: empty while the find bar is closed.
    #[must_use]
    pub fn match_highlights_ref(&self) -> MatchHighlightsRef<'_> {
        if self.search.open {
            MatchHighlightsRef {
                ranges: &self.search.matches,
                current: self.search.current,
            }
        } else {
            MatchHighlightsRef::default()
        }
    }

    /// The live search state.
    #[must_use]
    pub fn search(&self) -> &SearchState {
        &self.search
    }

    /// The search pattern, for the front end's find field to edit in place.
    /// Send [`Command::RecomputeMatches`](super::Command::RecomputeMatches)
    /// after changing it.
    pub fn search_pattern_mut(&mut self) -> &mut String {
        &mut self.search.query.pattern
    }

    /// The replacement text, for the front end's replace field to edit in
    /// place.
    pub fn search_replacement_mut(&mut self) -> &mut String {
        &mut self.search.replacement
    }
}

#[cfg(test)]
mod tests {
    // `unwrap`/`expect` are acceptable in test code: a panic on an unexpected
    // `Err`/`None` is exactly the failure signal we want from a test.
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::fs;
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::clipboard::MemoryClipboard;
    use crate::editor::Command;
    use crate::persistence::Store;

    /// Create an isolated temp directory unique to this test.
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "frext-search-test-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A fresh editor backed by a store rooted in `dir/state`, with `files`
    /// open.
    fn editor_with_files(dir: &Path, files: &[PathBuf]) -> Editor {
        let store = Store::at(dir.join("state")).unwrap();
        Editor::open(store, Box::new(MemoryClipboard::default()), files, None)
    }

    #[test]
    fn replacement_reads_back_what_was_written() {
        let dir = temp_dir("replacement-getter");
        let mut editor = editor_with_files(&dir, &[]);
        assert_eq!(editor.search().replacement(), "");
        "bye".clone_into(editor.search_replacement_mut());
        assert_eq!(editor.search().replacement(), "bye");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn option_reads_each_toggle() {
        let dir = temp_dir("option-getter");
        let mut editor = editor_with_files(&dir, &[]);
        let options = [
            SearchOption::CaseSensitive,
            SearchOption::WholeWord,
            SearchOption::Regex,
            SearchOption::InSelection,
        ];
        for option in options {
            assert!(!editor.search().option(option), "{option:?}");
        }
        for (turned_on, option) in options.into_iter().enumerate() {
            editor.apply(Command::ToggleSearchOption(option));
            for (index, other) in options.into_iter().enumerate() {
                assert_eq!(
                    editor.search().option(other),
                    index <= turned_on,
                    "{other:?} after turning on {option:?}"
                );
            }
        }
        let search = editor.search();
        assert_eq!(
            search.option(SearchOption::CaseSensitive),
            search.query().case_sensitive
        );
        assert_eq!(
            search.option(SearchOption::InSelection),
            search.in_selection()
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    /// An editor with two file-backed tabs with distinct contents, focused on
    /// the one holding `first`. (A fresh editor always starts with an empty
    /// untitled tab, so the file tabs land after it.)
    fn editor_with_two_tabs(dir: &Path, first: &str, second: &str) -> Editor {
        let a = dir.join("a.txt");
        let b = dir.join("b.txt");
        fs::write(&a, first).unwrap();
        fs::write(&b, second).unwrap();
        let mut editor = editor_with_files(dir, &[a, b]);
        let index = editor
            .tabs()
            .iter()
            .position(|t| t.text == first)
            .expect("a tab holding the first content");
        editor.apply(Command::SelectTab(index));
        editor
    }

    /// Set the search pattern and re-find, as the find field does.
    fn find(editor: &mut Editor, pattern: &str) {
        *editor.search_pattern_mut() = pattern.to_owned();
        editor.apply(Command::RecomputeMatches);
    }

    #[test]
    fn search_targets_only_the_active_tab() {
        let dir = temp_dir("search-active");
        let mut editor = editor_with_two_tabs(&dir, "foo here foo", "foo and foo and foo");

        find(&mut editor, "foo");

        // The active tab ("foo here foo") has two "foo"s; the other tab's
        // three are never considered.
        assert_eq!(editor.search().matches(), [0..3, 9..12]);

        // Switching the active tab re-scopes the search to that buffer.
        let index = editor
            .tabs()
            .iter()
            .position(|t| t.text == "foo and foo and foo")
            .unwrap();
        editor.apply(Command::SelectTab(index));
        editor.apply(Command::RecomputeMatches);
        assert_eq!(editor.search().matches().len(), 3);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn stepping_matches_wraps_around() {
        let dir = temp_dir("search-step");
        let mut editor = editor_with_two_tabs(&dir, "x x x", "");
        find(&mut editor, "x");
        assert_eq!(editor.search().current(), Some(0));

        editor.apply(Command::StepMatch(StepDirection::Forward));
        assert_eq!(editor.search().current(), Some(1));
        editor.apply(Command::StepMatch(StepDirection::Forward));
        assert_eq!(editor.search().current(), Some(2));
        // Wrap forward to the first.
        editor.apply(Command::StepMatch(StepDirection::Forward));
        assert_eq!(editor.search().current(), Some(0));
        // Wrap backward to the last.
        editor.apply(Command::StepMatch(StepDirection::Backward));
        assert_eq!(editor.search().current(), Some(2));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn replace_all_only_changes_the_active_tab() {
        let dir = temp_dir("search-replace-all");
        // Distinct contents so we can tell the two file tabs apart.
        let mut editor = editor_with_two_tabs(&dir, "cat cat", "cat in tab two");
        let active = editor.active_index();
        let other = editor
            .tabs()
            .iter()
            .position(|t| t.text == "cat in tab two")
            .unwrap();

        *editor.search_replacement_mut() = "dog".to_owned();
        find(&mut editor, "cat");
        editor.apply(Command::ReplaceAll);

        assert_eq!(editor.tabs()[active].text, "dog dog");
        // The inactive tab is untouched.
        assert_eq!(editor.tabs()[other].text, "cat in tab two");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn replace_current_replaces_one_match_and_advances() {
        let dir = temp_dir("search-replace-one");
        let mut editor = editor_with_two_tabs(&dir, "cat cat cat", "");
        *editor.search_replacement_mut() = "dog".to_owned();
        find(&mut editor, "cat");

        // Replace the first match only.
        assert_eq!(editor.search().current(), Some(0));
        editor.apply(Command::ReplaceCurrent);
        let active = editor.active_index();
        assert_eq!(editor.tabs()[active].text, "dog cat cat");
        // Two "cat" matches remain.
        assert_eq!(editor.search().matches().len(), 2);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn regex_replace_expands_capture_groups() {
        let dir = temp_dir("search-regex-replace");
        let mut editor = editor_with_two_tabs(&dir, "user@host", "");
        editor.apply(Command::ToggleSearchOption(SearchOption::Regex));
        *editor.search_replacement_mut() = "$2.$1".to_owned();
        find(&mut editor, r"(\w+)@(\w+)");
        editor.apply(Command::ReplaceAll);

        let active = editor.active_index();
        assert_eq!(editor.tabs()[active].text, "host.user");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn matches_refresh_after_the_buffer_is_edited() {
        let dir = temp_dir("search-live-edit");
        let mut editor = editor_with_two_tabs(&dir, "foo", "");
        editor.apply(Command::OpenSearch(SearchBar::Find));
        find(&mut editor, "foo");
        assert_eq!(editor.search().matches().len(), 1);

        // A view edit that adds two more occurrences.
        editor.active_view_mut().unwrap().tab.text = "foo foo foo".to_owned();
        editor.commit_edit();

        assert_eq!(editor.search().matches(), [0..3, 4..7, 8..11]);

        // And removing all occurrences clears the count.
        editor.active_view_mut().unwrap().tab.text = "nothing here".to_owned();
        editor.commit_edit();
        assert!(editor.search().matches().is_empty());
        assert_eq!(editor.search().current(), None);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn invalid_regex_sets_inline_error_and_no_matches() {
        let dir = temp_dir("search-bad-regex");
        let mut editor = editor_with_two_tabs(&dir, "anything", "");
        editor.apply(Command::ToggleSearchOption(SearchOption::Regex));
        find(&mut editor, "(");

        assert!(editor.search().error().is_some());
        assert!(editor.search().matches().is_empty());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn opening_search_seeds_from_a_single_line_selection() {
        let dir = temp_dir("search-seed");
        let mut editor = editor_with_two_tabs(&dir, "café a.b café", "");
        editor.apply(Command::ToggleSearchOption(SearchOption::Regex));
        assert!(editor.search().query().regex);

        // Select "a.b" (chars 5..8; bytes 6..9 after the two-byte "é").
        editor.set_selection(Some(5..8));
        editor.apply(Command::OpenSearch(SearchBar::Find));

        assert!(editor.search().is_open());
        assert!(!editor.search().replace_open());
        assert_eq!(editor.search().query().pattern, "a.b");
        assert!(!editor.search().query().regex, "a seeded literal is plain");
        assert_eq!(editor.search().matches(), std::slice::from_ref(&(6..9)));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_multi_line_selection_does_not_seed_the_pattern() {
        let dir = temp_dir("search-no-seed");
        let mut editor = editor_with_two_tabs(&dir, "one\ntwo", "");
        *editor.search_pattern_mut() = "kept".to_owned();

        editor.set_selection(Some(0..7));
        editor.apply(Command::OpenSearch(SearchBar::FindReplace));

        assert!(editor.search().replace_open());
        assert_eq!(editor.search().query().pattern, "kept");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn opening_search_requests_focus_on_the_find_field() {
        let dir = temp_dir("search-focus");
        let mut editor = editor_with_files(&dir, &[]);

        let outcome = editor.apply(Command::OpenSearch(SearchBar::Find));

        assert_eq!(outcome.buffer, BufferChange::Unchanged);
        assert_eq!(outcome.effects, vec![Effect::FocusSearchField]);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn stepping_selects_the_match_as_a_char_range_and_reveals_it() {
        let dir = temp_dir("search-step-select");
        let mut editor = editor_with_two_tabs(&dir, "café foo café foo", "");
        find(&mut editor, "foo");
        // Byte ranges: "é" is two bytes, so the second "foo" sits at 16..19
        // (chars 14..17).
        assert_eq!(editor.search().matches(), [6..9, 16..19]);

        let outcome = editor.apply(Command::StepMatch(StepDirection::Forward));

        assert_eq!(editor.search().current(), Some(1));
        let tab_id = editor.active_tab().unwrap().id;
        assert_eq!(
            outcome.effects,
            vec![
                Effect::SetSelection {
                    tab_id,
                    chars: 14..17,
                    focus: EditorFocus::Keep,
                },
                Effect::RevealSelection { tab_id },
            ]
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn closing_search_clears_matches_and_persists_the_query() {
        let dir = temp_dir("search-close");
        let mut editor = editor_with_two_tabs(&dir, "foo foo", "");
        editor.apply(Command::OpenSearch(SearchBar::Find));
        find(&mut editor, "foo");
        editor.apply(Command::ToggleSearchOption(SearchOption::WholeWord));
        assert_eq!(editor.search().matches().len(), 2);

        editor.apply(Command::CloseSearch);

        assert!(!editor.search().is_open());
        assert!(editor.search().matches().is_empty());
        assert_eq!(editor.search().current(), None);

        let restored = Store::at(dir.join("state")).unwrap().load().unwrap();
        assert_eq!(restored.search.pattern, "foo");
        assert!(restored.search.whole_word);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn search_in_selection_is_limited_to_the_selected_chars() {
        let dir = temp_dir("search-in-selection");
        let mut editor = editor_with_two_tabs(&dir, "é foo é foo é foo", "");
        find(&mut editor, "foo");
        assert_eq!(editor.search().matches().len(), 3);

        // Select "o é foo" (chars 4..11, starting at the last "o" of the first
        // "foo"): only the middle "foo" (chars 8..11, bytes 10..13) lies wholly
        // inside it; the first "foo" (chars 2..5) is only partly inside.
        editor.set_selection(Some(4..11));
        editor.apply(Command::ToggleSearchOption(SearchOption::InSelection));

        assert!(editor.search().in_selection());
        assert_eq!(editor.search().matches(), std::slice::from_ref(&(10..13)));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn toggling_an_option_recomputes_matches() {
        let dir = temp_dir("search-toggle");
        let mut editor = editor_with_two_tabs(&dir, "Foo foo", "");
        find(&mut editor, "foo");
        assert_eq!(editor.search().matches().len(), 2);

        editor.apply(Command::ToggleSearchOption(SearchOption::CaseSensitive));

        assert!(editor.search().query().case_sensitive);
        assert_eq!(editor.search().matches(), std::slice::from_ref(&(4..7)));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn toggling_the_replace_row_does_not_recompute() {
        let dir = temp_dir("search-toggle-replace");
        let mut editor = editor_with_two_tabs(&dir, "foo foo", "");
        find(&mut editor, "foo");
        // Change the pattern without re-finding: a recompute would drop the
        // two stale matches.
        *editor.search_pattern_mut() = "absent".to_owned();

        editor.apply(Command::ToggleReplaceRow);

        assert!(editor.search().replace_open());
        assert_eq!(editor.search().matches().len(), 2);

        editor.apply(Command::ToggleReplaceRow);
        assert!(!editor.search().replace_open());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn match_highlights_are_empty_while_closed() {
        let dir = temp_dir("search-highlights");
        let mut editor = editor_with_two_tabs(&dir, "foo foo", "");
        find(&mut editor, "foo");

        let closed = editor.match_highlights();
        assert!(closed.ranges.is_empty());
        assert_eq!(closed.current, None);

        editor.apply(Command::OpenSearch(SearchBar::Find));
        let open = editor.match_highlights();
        assert_eq!(open.ranges, vec![0..3, 4..7]);
        assert_eq!(open.current, Some(0));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn borrowed_match_highlights_are_empty_while_closed() {
        let dir = temp_dir("search-highlights-ref");
        let mut editor = editor_with_two_tabs(&dir, "foo foo", "");
        find(&mut editor, "foo");

        assert_eq!(editor.match_highlights_ref(), MatchHighlightsRef::default());

        editor.apply(Command::OpenSearch(SearchBar::Find));
        let open = editor.match_highlights_ref();
        assert_eq!(open.ranges, &[0..3, 4..7]);
        assert_eq!(open.current, Some(0));
        assert_eq!(open, editor.match_highlights().as_ref());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn count_label_reports_position_and_total() {
        let dir = temp_dir("search-count");
        let mut editor = editor_with_two_tabs(&dir, "foo foo", "");
        assert_eq!(editor.search().count_label(), "");

        find(&mut editor, "absent");
        assert_eq!(editor.search().count_label(), "no results");

        find(&mut editor, "foo");
        assert_eq!(editor.search().count_label(), "1/2");
        editor.apply(Command::StepMatch(StepDirection::Forward));
        assert_eq!(editor.search().count_label(), "2/2");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn selecting_a_tab_recomputes_matches_for_it() {
        let dir = temp_dir("search-select-tab");
        let mut editor = editor_with_two_tabs(&dir, "foo", "foo foo foo");
        find(&mut editor, "foo");
        assert_eq!(editor.search().matches().len(), 1);

        let index = editor
            .tabs()
            .iter()
            .position(|t| t.text == "foo foo foo")
            .unwrap();
        editor.apply(Command::SelectTab(index));

        assert_eq!(editor.search().matches(), [0..3, 4..7, 8..11]);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn replace_current_reports_an_edit() {
        let dir = temp_dir("search-replace-edit");
        let mut editor = editor_with_two_tabs(&dir, "cat", "");
        *editor.search_replacement_mut() = "dog".to_owned();
        find(&mut editor, "cat");

        let outcome = editor.apply(Command::ReplaceCurrent);

        assert_eq!(outcome.buffer, BufferChange::Edited);
        assert!(outcome.effects.is_empty());
        assert_eq!(editor.active_tab().unwrap().text, "dog");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn replace_current_selects_and_reveals_the_next_match() {
        let dir = temp_dir("search-replace-select-next");
        let mut editor = editor_with_two_tabs(&dir, "hello hello hello", "");
        *editor.search_replacement_mut() = "bye".to_owned();
        find(&mut editor, "hello");
        let tab_id = editor.active_tab().unwrap().id;

        let outcome = editor.apply(Command::ReplaceCurrent);

        assert_eq!(editor.active_tab().unwrap().text, "bye hello hello");
        assert_eq!(outcome.buffer, BufferChange::Edited);
        assert_eq!(editor.search().current(), Some(0));
        assert_eq!(
            outcome.effects,
            vec![
                Effect::SetSelection {
                    tab_id,
                    chars: 4..9,
                    focus: EditorFocus::Keep,
                },
                Effect::RevealSelection { tab_id },
            ]
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn replacing_the_final_match_wraps_to_the_first() {
        let dir = temp_dir("search-replace-wrap");
        let mut editor = editor_with_two_tabs(&dir, "hello hello", "");
        *editor.search_replacement_mut() = "bye".to_owned();
        find(&mut editor, "hello");
        editor.apply(Command::StepMatch(StepDirection::Forward));
        assert_eq!(editor.search().current(), Some(1));
        let tab_id = editor.active_tab().unwrap().id;

        let outcome = editor.apply(Command::ReplaceCurrent);

        assert_eq!(editor.active_tab().unwrap().text, "hello bye");
        assert_eq!(outcome.buffer, BufferChange::Edited);
        assert_eq!(editor.search().current(), Some(0));
        assert_eq!(
            outcome.effects,
            vec![
                Effect::SetSelection {
                    tab_id,
                    chars: 0..5,
                    focus: EditorFocus::Keep,
                },
                Effect::RevealSelection { tab_id },
            ]
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn replacing_the_last_match_selects_nothing() {
        let dir = temp_dir("search-replace-last");
        let mut editor = editor_with_two_tabs(&dir, "say hello", "");
        *editor.search_replacement_mut() = "bye".to_owned();
        find(&mut editor, "hello");

        let outcome = editor.apply(Command::ReplaceCurrent);

        assert_eq!(editor.active_tab().unwrap().text, "say bye");
        assert_eq!(outcome.buffer, BufferChange::Edited);
        assert_eq!(editor.search().current(), None);
        assert!(outcome.effects.is_empty());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn replace_current_without_a_current_match_is_a_no_op() {
        let dir = temp_dir("search-replace-no-current");
        let mut editor = editor_with_two_tabs(&dir, "cat", "");
        *editor.search_replacement_mut() = "dog".to_owned();
        find(&mut editor, "absent");
        assert_eq!(editor.search().current(), None);

        let outcome = editor.apply(Command::ReplaceCurrent);

        assert_eq!(outcome, Outcome::default());
        assert_eq!(editor.active_tab().unwrap().text, "cat");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn replace_all_without_matches_reports_no_edit() {
        let dir = temp_dir("search-replace-none");
        let mut editor = editor_with_two_tabs(&dir, "cat", "");
        *editor.search_replacement_mut() = "dog".to_owned();
        find(&mut editor, "absent");

        let outcome = editor.apply(Command::ReplaceAll);

        assert_eq!(outcome.buffer, BufferChange::Unchanged);
        assert_eq!(editor.active_tab().unwrap().text, "cat");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn find_action_opens_the_search_bar() {
        let dir = temp_dir("search-find-action");
        let mut editor = editor_with_files(&dir, &[]);
        assert!(!editor.search().is_open());

        let outcome = editor.apply(Command::OpenSearch(SearchBar::Find));

        assert_eq!(outcome.buffer, BufferChange::Unchanged);
        assert!(editor.search().is_open());
        assert!(!editor.search().replace_open());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn find_replace_action_opens_the_replace_row() {
        let dir = temp_dir("search-replace-action");
        let mut editor = editor_with_files(&dir, &[]);

        editor.apply(Command::OpenSearch(SearchBar::FindReplace));

        assert!(editor.search().is_open());
        assert!(editor.search().replace_open());

        fs::remove_dir_all(&dir).unwrap();
    }

    /// Write `text` to `dir/name`, returning the path.
    fn file(dir: &Path, name: &str, text: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, text).unwrap();
        path
    }

    /// An editor with `files` open (the last one active), the find bar open
    /// and "foo" searched in the active buffer.
    fn searching_foo(dir: &Path, files: &[PathBuf]) -> Editor {
        let store = Store::at(dir.join("state")).unwrap();
        let mut editor = Editor::open(store, Box::new(MemoryClipboard::default()), files, None);
        editor.apply(Command::OpenSearch(SearchBar::Find));
        *editor.search_pattern_mut() = "foo".to_owned();
        editor.apply(Command::RecomputeMatches);
        editor
    }

    #[test]
    fn opening_a_path_refreshes_matches_for_the_new_buffer() {
        let dir = temp_dir("open-path");
        let a = file(&dir, "a.txt", "foo xxxx");
        let b = file(&dir, "b.txt", "xxxx foo");
        let mut editor = searching_foo(&dir, &[a]);
        assert_eq!(editor.search().matches(), [Range { start: 0, end: 3 }]);

        editor.apply(Command::OpenPath(b));

        assert_eq!(editor.active_tab().unwrap().text, "xxxx foo");
        assert_eq!(editor.search().matches(), [Range { start: 5, end: 8 }]);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn focusing_an_already_open_path_refreshes_matches() {
        let dir = temp_dir("refocus-path");
        let a = file(&dir, "a.txt", "foo xxxx");
        let b = file(&dir, "b.txt", "xxxx foo");
        let mut editor = searching_foo(&dir, &[b.clone(), a]);
        assert_eq!(editor.active_tab().unwrap().text, "foo xxxx");
        assert_eq!(editor.search().matches(), [Range { start: 0, end: 3 }]);

        editor.apply(Command::OpenPath(b));

        assert_eq!(editor.active_tab().unwrap().text, "xxxx foo");
        assert_eq!(editor.search().matches(), [Range { start: 5, end: 8 }]);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_new_tab_has_no_stale_matches() {
        let dir = temp_dir("new-tab");
        let a = file(&dir, "a.txt", "foo foo");
        let mut editor = searching_foo(&dir, &[a]);
        assert_eq!(editor.search().matches(), [0..3, 4..7]);

        editor.apply(Command::NewTab);

        assert!(editor.search().matches().is_empty());
        assert_eq!(editor.search().current(), None);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn closing_a_tab_refreshes_matches_for_the_next_active_buffer() {
        let dir = temp_dir("close-tab");
        let a = file(&dir, "a.txt", "foo xxxx");
        let b = file(&dir, "b.txt", "xxxx foo");
        let mut editor = searching_foo(&dir, &[a, b]);
        let a_index = editor
            .tabs()
            .iter()
            .position(|t| t.text == "foo xxxx")
            .unwrap();
        editor.apply(Command::SelectTab(a_index));
        assert_eq!(editor.search().matches(), [Range { start: 0, end: 3 }]);

        // Closing the clean active tab focuses "xxxx foo" in its slot.
        editor.apply(Command::RequestClose(a_index));

        assert_eq!(editor.active_tab().unwrap().text, "xxxx foo");
        assert_eq!(editor.search().matches(), [Range { start: 5, end: 8 }]);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_external_reload_refreshes_matches() {
        let dir = temp_dir("reload");
        let a = file(&dir, "a.txt", "foo x");
        let mut editor = searching_foo(&dir, std::slice::from_ref(&a));
        assert_eq!(editor.search().matches(), [Range { start: 0, end: 3 }]);

        // An external writer changes the file's length.
        fs::write(&a, "xx foo").unwrap();
        editor.begin_frame(|| {});

        assert_eq!(editor.active_tab().unwrap().text, "xx foo");
        assert_eq!(editor.search().matches(), [Range { start: 3, end: 6 }]);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn stale_matches_never_split_a_multi_byte_char() {
        let dir = temp_dir("multi-byte");
        let a = file(&dir, "a.txt", "foo");
        let b = file(&dir, "b.txt", "éé");
        let mut editor = searching_foo(&dir, &[a]);
        assert_eq!(editor.search().matches(), [Range { start: 0, end: 3 }]);

        editor.apply(Command::OpenPath(b));

        let text = &editor.active_tab().unwrap().text;
        assert_eq!(text, "éé");
        for range in editor.search().matches() {
            assert!(text.is_char_boundary(range.start), "{range:?}");
            assert!(text.is_char_boundary(range.end), "{range:?}");
        }
        assert!(editor.search().matches().is_empty());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn in_selection_ignores_another_tabs_selection() {
        let dir = temp_dir("in-selection-other-tab");
        let a = file(&dir, "a.txt", "foo foo foo");
        let b = file(&dir, "b.txt", "foo foo foo");
        let mut editor = searching_foo(&dir, &[a.clone(), b.clone()]);
        let index_of = |editor: &Editor, path: &Path| {
            editor
                .tabs()
                .iter()
                .position(|t| t.path.as_deref() == Some(path))
                .unwrap()
        };
        editor.apply(Command::SelectTab(index_of(&editor, &a)));
        editor.set_selection(Some(0..3));
        editor.apply(Command::ToggleSearchOption(SearchOption::InSelection));
        assert_eq!(editor.search().matches(), [Range { start: 0, end: 3 }]);

        // Tab B never reported a selection: A's range must not scope it.
        editor.apply(Command::SelectTab(index_of(&editor, &b)));

        assert_eq!(editor.search().matches(), [0..3, 4..7, 8..11]);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn replace_all_in_selection_only_replaces_inside_the_selection() {
        let dir = temp_dir("replace-all-in-selection");
        let mut editor = editor_with_two_tabs(&dir, "cat cat", "");
        *editor.search_replacement_mut() = "dog".to_owned();
        find(&mut editor, "cat");
        editor.set_selection(Some(0..3));
        editor.apply(Command::ToggleSearchOption(SearchOption::InSelection));

        let outcome = editor.apply(Command::ReplaceAll);

        assert_eq!(outcome.buffer, BufferChange::Edited);
        assert_eq!(editor.active_tab().unwrap().text, "dog cat");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn replace_all_without_in_selection_still_replaces_everything() {
        let dir = temp_dir("replace-all-no-in-selection");
        let mut editor = editor_with_two_tabs(&dir, "cat cat", "");
        *editor.search_replacement_mut() = "dog".to_owned();
        find(&mut editor, "cat");
        editor.set_selection(Some(0..3));

        let outcome = editor.apply(Command::ReplaceAll);

        assert_eq!(outcome.buffer, BufferChange::Edited);
        assert_eq!(editor.active_tab().unwrap().text, "dog dog");

        fs::remove_dir_all(&dir).unwrap();
    }

    /// An editor on "a a\na a" (plus `other` in a second tab) with "a"
    /// searched in the open bar and search-within-selection captured over
    /// line 1 (chars 0..3).
    fn in_selection_over_line_one(dir: &Path, other: &str) -> Editor {
        let mut editor = editor_with_two_tabs(dir, "a a\na a", other);
        editor.apply(Command::OpenSearch(SearchBar::Find));
        find(&mut editor, "a");
        editor.set_selection(Some(0..3));
        editor.apply(Command::ToggleSearchOption(SearchOption::InSelection));
        assert_eq!(editor.search().matches(), [0..1, 2..3]);
        editor
    }

    #[test]
    fn replace_all_after_a_step_replaces_the_whole_captured_scope() {
        let dir = temp_dir("scope-step-replace-all");
        let mut editor = in_selection_over_line_one(&dir, "");
        *editor.search_replacement_mut() = "x".to_owned();

        // Stepping selects the second match, as the front ends report.
        editor.apply(Command::StepMatch(StepDirection::Forward));
        editor.set_selection(Some(2..3));
        assert_eq!(editor.search().matches(), [0..1, 2..3]);

        let outcome = editor.apply(Command::ReplaceAll);

        assert_eq!(outcome.buffer, BufferChange::Edited);
        assert_eq!(editor.active_tab().unwrap().text, "x x\na a");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_edit_before_the_scope_shifts_it() {
        let dir = temp_dir("scope-shift");
        let mut editor = in_selection_over_line_one(&dir, "");

        editor.active_view_mut().unwrap().tab.text = "zza a\na a".to_owned();
        editor.commit_edit();

        assert_eq!(editor.search().matches(), [2..3, 4..5]);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_scope_is_ignored_on_another_tab() {
        let dir = temp_dir("scope-other-tab");
        let mut editor = in_selection_over_line_one(&dir, "a a a");
        let first = editor.active_index();
        let other = editor
            .tabs()
            .iter()
            .position(|t| t.text == "a a a")
            .unwrap();

        editor.apply(Command::SelectTab(other));
        assert_eq!(editor.search().matches(), [0..1, 2..3, 4..5]);

        // Back on its own tab, the scope applies again.
        editor.apply(Command::SelectTab(first));
        assert_eq!(editor.search().matches(), [0..1, 2..3]);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn toggling_in_selection_off_drops_the_scope() {
        let dir = temp_dir("scope-toggle-off");
        let mut editor = in_selection_over_line_one(&dir, "");

        editor.apply(Command::ToggleSearchOption(SearchOption::InSelection));
        assert_eq!(editor.search().matches().len(), 4);

        // Turning it back on without a selection does not revive the old
        // scope.
        editor.set_selection(None);
        editor.apply(Command::ToggleSearchOption(SearchOption::InSelection));
        assert!(editor.search().in_selection());
        assert_eq!(editor.search().matches().len(), 4);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn closing_search_drops_the_scope() {
        let dir = temp_dir("scope-close");
        let mut editor = in_selection_over_line_one(&dir, "");

        editor.apply(Command::CloseSearch);
        editor.set_selection(None);
        editor.apply(Command::OpenSearch(SearchBar::Find));

        assert!(editor.search().in_selection());
        assert_eq!(editor.search().matches().len(), 4);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn opening_search_with_in_selection_on_recaptures_the_scope() {
        let dir = temp_dir("scope-reopen");
        let mut editor = in_selection_over_line_one(&dir, "");

        // Select line 2 and reopen the bar (which also seeds "a a").
        editor.set_selection(Some(4..7));
        editor.apply(Command::OpenSearch(SearchBar::Find));
        find(&mut editor, "a");

        assert_eq!(editor.search().matches(), [4..5, 6..7]);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn selection_effects_name_the_tab_they_target() {
        let dir = temp_dir("effects-name-tab");
        let mut editor = editor_with_two_tabs(&dir, "foo foo", "other");
        let first = editor.active_tab().unwrap().id;
        find(&mut editor, "foo");

        let stepped = editor.apply(Command::StepMatch(StepDirection::Forward));
        editor.set_selection(Some(0..4));
        let cut = editor.apply(Command::Cut);

        assert_eq!(
            stepped.effects,
            vec![
                Effect::SetSelection {
                    tab_id: first,
                    chars: 4..7,
                    focus: EditorFocus::Keep,
                },
                Effect::RevealSelection { tab_id: first },
            ]
        );
        assert_eq!(
            cut.effects,
            vec![Effect::SetSelection {
                tab_id: first,
                chars: 0..0,
                focus: EditorFocus::Focus,
            }]
        );

        // After a switch the queued effects still name the original tab, not
        // the newly active one.
        let other = editor
            .tabs()
            .iter()
            .position(|t| t.text == "other")
            .unwrap();
        editor.apply(Command::SelectTab(other));
        let now_active = editor.active_tab().unwrap().id;
        assert_ne!(now_active, first);
        for effect in stepped.effects.iter().chain(&cut.effects) {
            let (Effect::SetSelection { tab_id, .. } | Effect::RevealSelection { tab_id }) = effect
            else {
                panic!("unexpected effect {effect:?}");
            };
            assert_eq!(*tab_id, first);
        }

        fs::remove_dir_all(&dir).unwrap();
    }
}
