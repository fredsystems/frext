//! Per-session proofreading state the app drives each frame: which revision
//! of each tab was last sent to the [`Worker`], the latest results per tab,
//! the personal dictionary, and the session's ignored diagnostics. Also
//! [`diagnostic_in`], the lookup of the diagnostic over a char range.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use std::path::Path;

use super::worker::{CheckRequest, CheckResult, Worker};
use super::{Diagnostic, DiagnosticKind, IgnoreKey};

/// The lifecycle of the background worker.
enum WorkerSlot {
    /// Not spawned yet; spawned on the first frame (it needs the UI
    /// context to request repaints).
    NotStarted,
    Running(Worker),
    /// Spawning failed or the thread stopped; proofreading is off for the
    /// rest of the session.
    Unavailable,
}

/// The app-facing proofreading controller.
pub struct Proofreading {
    worker: WorkerSlot,
    /// Revision last sent to the worker, per tab id.
    requested: HashMap<u64, u64>,
    /// Latest diagnostics received, per tab id.
    results: HashMap<u64, Vec<Diagnostic>>,
    /// The personal dictionary (persisted by the caller).
    user_words: BTreeSet<String>,
    /// Diagnostics the user chose to ignore this session.
    ignored: HashSet<IgnoreKey>,
}

impl Proofreading {
    /// Create the controller with the persisted personal dictionary. The
    /// worker is not started until [`Self::ensure_started`].
    #[must_use]
    pub fn new(user_words: impl IntoIterator<Item = String>) -> Self {
        Self {
            worker: WorkerSlot::NotStarted,
            requested: HashMap::new(),
            results: HashMap::new(),
            user_words: user_words.into_iter().collect(),
            ignored: HashSet::new(),
        }
    }

    /// Spawn the worker if it has not been started. A spawn failure is
    /// logged and disables proofreading; it never interrupts editing.
    pub fn ensure_started(&mut self, notify: impl Fn() + Send + 'static) {
        if !matches!(self.worker, WorkerSlot::NotStarted) {
            return;
        }
        self.worker = match Worker::spawn(self.user_words.iter().cloned().collect(), notify) {
            Ok(worker) => WorkerSlot::Running(worker),
            Err(err) => {
                log::error!("proofreading disabled: {err}");
                WorkerSlot::Unavailable
            }
        };
    }

    /// Ask for `text` (the buffer of the file at `path`) to be checked,
    /// unless this exact (text, path) was already requested for the tab.
    /// Cheap to call every frame.
    pub fn request(&mut self, tab_id: u64, text: &str, path: Option<&Path>) {
        let revision = revision(text, path);
        if self.requested.get(&tab_id) == Some(&revision) {
            return;
        }
        let WorkerSlot::Running(worker) = &self.worker else {
            return;
        };
        let sent = worker.check(CheckRequest {
            tab_id,
            revision,
            text: text.to_owned(),
            path: path.map(Path::to_path_buf),
        });
        if sent {
            self.requested.insert(tab_id, revision);
        } else {
            self.worker_stopped();
        }
    }

    /// Collect any results the worker has finished.
    pub fn poll(&mut self) {
        let WorkerSlot::Running(worker) = &self.worker else {
            return;
        };
        let results: Vec<CheckResult> = worker.drain().collect();
        for result in results {
            self.accept(result);
        }
    }

    /// Store a result. Results for an older revision are kept too: until the
    /// newer one lands, [`Diagnostic::is_current`] filters whatever no longer
    /// matches the buffer, which beats flickering every squiggle off.
    fn accept(&mut self, mut result: CheckResult) {
        // `diagnostics` validates in one forward pass, which needs them in
        // position order.
        result
            .diagnostics
            .sort_by_key(|d| (d.bytes.start, d.bytes.end));
        self.results.insert(result.tab_id, result.diagnostics);
    }

    /// The diagnostics to show for `tab_id` whose buffer is now `text`:
    /// still current (see [`Diagnostic::is_current`]), not ignored, and not
    /// a word just added to the dictionary (which the worker has yet to
    /// re-check).
    ///
    /// Called every frame, so currency is validated in one forward pass over
    /// `text` rather than rescanning the buffer prefix per diagnostic.
    #[must_use]
    pub fn diagnostics<'a>(&'a self, tab_id: u64, text: &str) -> Vec<&'a Diagnostic> {
        let Some(all) = self.results.get(&tab_id) else {
            return Vec::new();
        };
        let mut shown = Vec::new();
        // Char offset of byte `byte` in `text`, advanced monotonically.
        let mut byte = 0;
        let mut chars = 0;
        for diagnostic in all {
            let char_start = match text.get(byte..diagnostic.bytes.start) {
                Some(between) => {
                    chars += between.chars().count();
                    byte = diagnostic.bytes.start;
                    Some(chars)
                }
                // Out of range or not on a char boundary: stale.
                None => None,
            };
            if diagnostic.matches_at(text, char_start)
                && !self.ignored.contains(&diagnostic.ignore_key())
                && !(diagnostic.kind == DiagnosticKind::Spelling
                    && self.knows_word(&diagnostic.flagged))
            {
                shown.push(diagnostic);
            }
        }
        shown
    }

    /// The diagnostic under the cursor position `char_idx`, preferring a
    /// spelling one when several overlap (its fixes are the most specific).
    ///
    /// This is [`diagnostic_in`] over [`Self::diagnostics`] with the
    /// caret-equivalent range `char_idx - 1 .. char_idx + 1` (saturating).
    /// Its intersection test is an end-inclusive caret test, so a click on
    /// the right half of a word's last glyph resolves to the cursor position
    /// just *after* it. At `char_idx == 0` the range is `0..1`, which meets
    /// exactly the diagnostics starting at 0; diagnostics are never empty,
    /// so that is the same as `start <= 0 <= end`.
    #[must_use]
    pub fn diagnostic_at<'a>(
        &'a self,
        tab_id: u64,
        text: &'a str,
        char_idx: usize,
    ) -> Option<&'a Diagnostic> {
        diagnostic_in(
            &self.diagnostics(tab_id, text),
            char_idx.saturating_sub(1)..char_idx.saturating_add(1),
        )
    }

    /// Add `word` to the personal dictionary. Returns `true` if it was new,
    /// in which case the caller should persist [`Self::user_words`]. Every
    /// tab is re-checked against the new dictionary.
    pub fn add_word(&mut self, word: &str) -> bool {
        let word = word.trim();
        if word.is_empty() || !self.user_words.insert(word.to_owned()) {
            return false;
        }
        if let WorkerSlot::Running(worker) = &self.worker
            && !worker.set_user_words(self.user_words.iter().cloned().collect())
        {
            self.worker_stopped();
        }
        self.requested.clear();
        true
    }

    /// The personal dictionary, sorted.
    pub fn user_words(&self) -> impl Iterator<Item = &str> {
        self.user_words.iter().map(String::as_str)
    }

    /// Hide diagnostics matching `key` for the rest of the session.
    pub fn ignore(&mut self, key: IgnoreKey) {
        self.ignored.insert(key);
    }

    /// Drop everything held for a closed tab.
    pub fn forget_tab(&mut self, tab_id: u64) {
        self.requested.remove(&tab_id);
        self.results.remove(&tab_id);
    }

    fn knows_word(&self, word: &str) -> bool {
        self.user_words
            .iter()
            .any(|known| known.eq_ignore_ascii_case(word))
    }

    fn worker_stopped(&mut self) {
        log::error!("proofreading worker stopped; proofreading disabled");
        self.worker = WorkerSlot::Unavailable;
    }
}

/// The diagnostic among `diagnostics` over the chars `chars`.
///
/// Of those whose `chars` intersect `chars` (half-open on both sides, the
/// renderer's rule), the first is returned, unless it is not a spelling
/// diagnostic and a later one is, in which case the first spelling one wins
/// (its fixes are the most specific).
///
/// A drawn cell passes its grapheme's char range. A caret passes the
/// graphemes either side of it: from the start of the grapheme ending at the
/// caret to the end of the grapheme starting there. Over single-char
/// graphemes that is `c - 1 .. c + 1`, whose intersection is exactly an
/// end-inclusive caret test (`start <= c <= end`).
#[must_use]
pub fn diagnostic_in<'a>(
    diagnostics: &[&'a Diagnostic],
    chars: Range<usize>,
) -> Option<&'a Diagnostic> {
    let mut hits = diagnostics
        .iter()
        .copied()
        .filter(|d| d.chars.start < chars.end && chars.start < d.chars.end);
    let first = hits.next()?;
    if first.kind == DiagnosticKind::Spelling {
        return Some(first);
    }
    Some(
        hits.find(|d| d.kind == DiagnosticKind::Spelling)
            .unwrap_or(first),
    )
}

/// A revision id for a (text, path) pair. Only ever compared for equality
/// with the previous request, so a collision merely skips one re-check.
fn revision(text: &str, path: Option<&Path>) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    path.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    // `expect` is acceptable in test code: a panic on an unexpected `None`
    // is exactly the failure signal we want from a test.
    #![allow(clippy::expect_used)]

    use super::*;

    fn diagnostic(kind: DiagnosticKind, text: &str, flagged: &str) -> Diagnostic {
        let start = text.find(flagged).expect("flagged text is in the buffer");
        let char_start = text[..start].chars().count();
        Diagnostic {
            kind,
            bytes: start..start + flagged.len(),
            chars: char_start..char_start + flagged.chars().count(),
            flagged: flagged.to_owned(),
            message: format!("about {flagged}"),
            suggestions: Vec::new(),
        }
    }

    fn with_results(tab_id: u64, diagnostics: Vec<Diagnostic>) -> Proofreading {
        let mut state = Proofreading::new(Vec::new());
        state.accept(CheckResult {
            tab_id,
            revision: 0,
            diagnostics,
        });
        state
    }

    fn flagged<'a>(state: &'a Proofreading, tab_id: u64, text: &'a str) -> Vec<&'a str> {
        state
            .diagnostics(tab_id, text)
            .into_iter()
            .map(|d| d.flagged.as_str())
            .collect()
    }

    #[test]
    fn results_are_per_tab() {
        let text = "a wrrod";
        let state = with_results(1, vec![diagnostic(DiagnosticKind::Spelling, text, "wrrod")]);
        assert_eq!(flagged(&state, 1, text), vec!["wrrod"]);
        assert!(flagged(&state, 2, text).is_empty());
    }

    #[test]
    fn stale_results_are_hidden_after_an_edit() {
        let text = "a wrrod";
        let state = with_results(1, vec![diagnostic(DiagnosticKind::Spelling, text, "wrrod")]);
        assert!(flagged(&state, 1, "a word").is_empty());
        // An edit earlier in the buffer shifts the range: also stale.
        assert!(flagged(&state, 1, "xa wrrod").is_empty());
    }

    #[test]
    fn a_char_shift_with_the_same_byte_offset_hides_results() {
        // `é` -> `ab` keeps "wrrod" at the same bytes but one char later;
        // the old char range would put the squiggle under the wrong glyphs.
        let text = "é wrrod";
        let state = with_results(1, vec![diagnostic(DiagnosticKind::Spelling, text, "wrrod")]);
        assert_eq!(flagged(&state, 1, text), vec!["wrrod"]);
        assert!(flagged(&state, 1, "ab wrrod").is_empty());
    }

    #[test]
    fn every_current_diagnostic_survives_the_single_pass() {
        let text = "café wrrod, naïve wrrod and more wrrod";
        let mut all = Vec::new();
        let mut from = 0;
        while let Some(at) = text[from..].find("wrrod") {
            let start = from + at;
            let char_start = text[..start].chars().count();
            all.push(Diagnostic {
                kind: DiagnosticKind::Spelling,
                bytes: start..start + 5,
                chars: char_start..char_start + 5,
                flagged: "wrrod".to_owned(),
                message: String::new(),
                suggestions: Vec::new(),
            });
            from = start + 5;
        }
        // Accepted out of order: `accept` sorts them for the single pass.
        all.reverse();
        let state = with_results(1, all);
        assert_eq!(flagged(&state, 1, text).len(), 3);
    }

    #[test]
    fn ignoring_hides_every_matching_diagnostic() {
        let text = "wrrod and wrrod";
        let first = diagnostic(DiagnosticKind::Spelling, text, "wrrod");
        let mut second = first.clone();
        second.bytes = 10..15;
        second.chars = 10..15;
        let key = first.ignore_key();
        let mut state = with_results(1, vec![first, second]);
        assert_eq!(flagged(&state, 1, text).len(), 2);
        state.ignore(key);
        assert!(flagged(&state, 1, text).is_empty());
    }

    #[test]
    fn added_word_hides_spelling_but_not_grammar_until_recheck() {
        let text = "wrrod";
        let mut state = with_results(
            1,
            vec![
                diagnostic(DiagnosticKind::Spelling, text, "wrrod"),
                diagnostic(DiagnosticKind::Grammar, text, "wrrod"),
            ],
        );
        assert!(state.add_word("Wrrod"));
        assert!(!state.add_word("Wrrod"), "adding twice is a no-op");
        let kinds: Vec<_> = state.diagnostics(1, text).iter().map(|d| d.kind).collect();
        assert_eq!(kinds, vec![DiagnosticKind::Grammar]);
        assert_eq!(state.user_words().collect::<Vec<_>>(), vec!["Wrrod"]);
    }

    #[test]
    fn blank_words_are_not_added() {
        let mut state = Proofreading::new(Vec::new());
        assert!(!state.add_word("   "));
        assert_eq!(state.user_words().count(), 0);
    }

    #[test]
    fn diagnostic_at_prefers_spelling_when_overlapping() {
        let text = "the wrrod here";
        let state = with_results(
            1,
            vec![
                diagnostic(DiagnosticKind::Grammar, text, "the wrrod"),
                diagnostic(DiagnosticKind::Spelling, text, "wrrod"),
            ],
        );
        let at = |idx| state.diagnostic_at(1, text, idx).map(|d| d.kind);
        assert_eq!(at(1), Some(DiagnosticKind::Grammar));
        assert_eq!(at(5), Some(DiagnosticKind::Spelling));
        // Just past the end of "wrrod" (cursor after its last glyph).
        assert_eq!(at(9), Some(DiagnosticKind::Spelling));
        assert_eq!(at(10), None);
    }

    #[test]
    fn diagnostic_at_the_start_of_the_text() {
        let text = "wrrod here";
        let state = with_results(1, vec![diagnostic(DiagnosticKind::Spelling, text, "wrrod")]);
        assert!(state.diagnostic_at(1, text, 0).is_some());
        let later = "a wrrod";
        let state = with_results(
            1,
            vec![diagnostic(DiagnosticKind::Spelling, later, "wrrod")],
        );
        assert!(state.diagnostic_at(1, later, 0).is_none());
    }

    #[test]
    fn cell_ranges_match_half_open() {
        let text = "a wrrod here";
        let wrrod = diagnostic(DiagnosticKind::Spelling, text, "wrrod");
        assert_eq!(wrrod.chars, 2..7);
        let all = [&wrrod];
        let at = |chars| diagnostic_in(&all, chars).map(|d| d.flagged.as_str());
        assert_eq!(at(1..2), None, "before start");
        assert_eq!(at(2..3), Some("wrrod"), "at start");
        assert_eq!(at(4..5), Some("wrrod"), "inside");
        assert_eq!(at(6..7), Some("wrrod"), "on last char");
        assert_eq!(at(7..8), None, "just past the end");
    }

    #[test]
    fn caret_ranges_include_the_end() {
        let text = "a wrrod here";
        let wrrod = diagnostic(DiagnosticKind::Spelling, text, "wrrod");
        let all = [&wrrod];
        let caret = |c: usize| diagnostic_in(&all, c - 1..c + 1).map(|d| d.flagged.as_str());
        assert_eq!(caret(1), None, "before start");
        assert_eq!(caret(2), Some("wrrod"), "at start");
        assert_eq!(caret(7), Some("wrrod"), "just past the end");
        assert_eq!(caret(8), None, "one further");
    }

    #[test]
    fn a_range_ending_where_a_diagnostic_starts_does_not_match() {
        let text = "a wrrod here";
        let wrrod = diagnostic(DiagnosticKind::Spelling, text, "wrrod");
        let all = [&wrrod];
        assert!(diagnostic_in(&all, 0..2).is_none());
        assert!(diagnostic_in(&all, 7..9).is_none());
    }

    #[test]
    fn diagnostic_in_prefers_overlapping_spelling() {
        let text = "the wrrod here";
        let grammar = diagnostic(DiagnosticKind::Grammar, text, "the wrrod");
        let spelling = diagnostic(DiagnosticKind::Spelling, text, "wrrod");
        let all = [&grammar, &spelling];
        let kind = |chars| diagnostic_in(&all, chars).map(|d| d.kind);
        assert_eq!(kind(5..6), Some(DiagnosticKind::Spelling));
        assert_eq!(kind(4..6), Some(DiagnosticKind::Spelling));
        assert_eq!(kind(1..2), Some(DiagnosticKind::Grammar));
    }

    #[test]
    fn grammar_starting_at_a_spelling_end_wins_only_for_a_cell() {
        let text = "wrrod, here";
        let spelling = diagnostic(DiagnosticKind::Spelling, text, "wrrod");
        let grammar = diagnostic(DiagnosticKind::Grammar, text, ", here");
        assert_eq!(spelling.chars.end, grammar.chars.start);
        let all = [&spelling, &grammar];
        let kind = |chars| diagnostic_in(&all, chars).map(|d| d.kind);
        assert_eq!(kind(5..6), Some(DiagnosticKind::Grammar), "cell");
        assert_eq!(kind(4..6), Some(DiagnosticKind::Spelling), "caret at 5");
    }

    #[test]
    fn diagnostic_in_an_empty_slice_is_none() {
        assert!(diagnostic_in(&[], 0..1).is_none());
        assert!(diagnostic_in(&[], 0..0).is_none());
    }

    #[test]
    fn forget_tab_drops_its_results() {
        let text = "a wrrod";
        let mut state = with_results(1, vec![diagnostic(DiagnosticKind::Spelling, text, "wrrod")]);
        state.forget_tab(1);
        assert!(flagged(&state, 1, text).is_empty());
    }

    #[test]
    fn revision_tracks_text_and_path() {
        let rs = Some(Path::new("a.rs"));
        assert_eq!(revision("a", rs), revision("a", rs));
        assert_ne!(revision("a", rs), revision("b", rs));
        assert_ne!(revision("a", rs), revision("a", Some(Path::new("a.md"))));
        assert_ne!(revision("a", rs), revision("a", None));
    }

    #[test]
    fn requests_without_a_worker_are_harmless() {
        let mut state = Proofreading::new(Vec::new());
        state.request(1, "text", None);
        state.poll();
        assert!(state.diagnostics(1, "text").is_empty());
    }
}
