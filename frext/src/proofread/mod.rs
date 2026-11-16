//! Spelling and grammar checking, powered by [`harper_core`].
//!
//! Harper runs entirely in-process (no server, no network) and checks both
//! spelling and grammar in one pass. This module turns its [`Lint`]s into
//! frext [`Diagnostic`]s carrying *byte* ranges (what the buffer model
//! speaks) and *char* ranges (what egui's galley speaks).
//!
//! Which parts of a buffer are checked is decided by [`regions`]: prose
//! buffers in full, code buffers only in their comments and strings.
//!
//! Checking a large buffer takes longer than a frame, so the app never calls
//! [`Proofreader`] directly: [`worker`] runs it on a background thread and
//! [`state`] tracks results per tab. [`squiggle`] paints the results.

pub mod regions;
pub mod squiggle;
pub mod state;
pub mod worker;

use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

use harper_core::{
    Dialect, DictWordMetadata, Document, Span, Token, TokenKind,
    linting::{Lint, LintGroup, LintKind, Linter, Suggestion},
    parsers::{Markdown, Parser, PlainEnglish},
    spell::{FstDictionary, MergedDictionary, MutableDictionary},
};

use self::regions::{Boundary, CheckScope, ProseFormat, Region, RegionKind};

/// Upper bound on suggestions kept per diagnostic; the context menu stays
/// short and the rest are rarely what the user meant anyway.
const MAX_SUGGESTIONS: usize = 6;

/// Harper rules that only make sense for prose documents. In code, comment
/// fragments and log strings legitimately start lowercase ("failed to open
/// …"), and runs of spaces in comments are column alignment, not typos.
const PROSE_ONLY_RULES: [&str; 2] = ["SentenceCapitalization", "Spaces"];

/// What a [`Diagnostic`] complains about. Drives the squiggle colour and
/// which context-menu actions are offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiagnosticKind {
    /// An unknown word (red squiggle; can be added to the dictionary).
    Spelling,
    /// Any other language issue Harper reports: grammar, punctuation,
    /// capitalisation, word choice (green squiggle).
    Grammar,
}

/// One proofreading result over a buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// What kind of problem this is.
    pub kind: DiagnosticKind,
    /// Byte range of the flagged text in the buffer it was computed for.
    pub bytes: Range<usize>,
    /// The same range in characters, for egui galley/cursor positions.
    pub chars: Range<usize>,
    /// The flagged text itself. Used to detect that a result has gone stale
    /// because the buffer changed underneath it.
    pub flagged: String,
    /// Harper's human-readable explanation.
    pub message: String,
    /// Replacement texts for `bytes`, best first. An empty string means
    /// "remove the flagged text".
    pub suggestions: Vec<String>,
}

impl Diagnostic {
    /// Whether this diagnostic still describes `text`: its byte range is in
    /// bounds, on char boundaries, and still holds the flagged text, and its
    /// char range still starts at the same character. Results arrive
    /// asynchronously, so a buffer edited since the check would otherwise get
    /// squiggles in the wrong place. (An edit can keep the byte offset while
    /// changing the char offset, e.g. `é` -> `ab`.)
    ///
    /// This scans `text` up to the diagnostic; to validate many sorted
    /// diagnostics in one pass, use [`state::Proofreading::diagnostics`].
    #[must_use]
    pub fn is_current(&self, text: &str) -> bool {
        self.matches_at(
            text,
            text.get(..self.bytes.start).map(|p| p.chars().count()),
        )
    }

    /// [`Self::is_current`] with the char offset of `bytes.start` already
    /// known (`None` if `bytes.start` is not a char boundary of `text`).
    fn matches_at(&self, text: &str, char_start: Option<usize>) -> bool {
        char_start == Some(self.chars.start)
            && text.get(self.bytes.clone()) == Some(self.flagged.as_str())
    }

    /// The key that "Ignore" records for this diagnostic.
    #[must_use]
    pub fn ignore_key(&self) -> IgnoreKey {
        IgnoreKey {
            kind: self.kind,
            flagged: self.flagged.clone(),
            message: self.message.clone(),
        }
    }
}

/// Identifies an ignored diagnostic independent of its position, so ignoring
/// one occurrence also hides identical complaints elsewhere in the session.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IgnoreKey {
    kind: DiagnosticKind,
    flagged: String,
    message: String,
}

/// A synchronous Harper checker with the user's personal dictionary loaded.
pub struct Proofreader {
    dictionary: Arc<MergedDictionary>,
    linter: LintGroup,
}

impl Proofreader {
    /// Build a checker using Harper's curated American-English dictionary
    /// plus `user_words`. Construction loads the dictionary and rule set, so
    /// it is done once (on the worker thread) and reused.
    #[must_use]
    pub fn new(user_words: &[String]) -> Self {
        let mut user = MutableDictionary::new();
        for word in user_words {
            user.append_word_str(word, DictWordMetadata::default());
        }
        let mut merged = MergedDictionary::new();
        merged.add_dictionary(FstDictionary::curated());
        merged.add_dictionary(Arc::new(user));
        let dictionary = Arc::new(merged);
        let linter = LintGroup::new_curated(Arc::clone(&dictionary), Dialect::American);
        Self { dictionary, linter }
    }

    /// Check `text`, the buffer of the file at `path` (`None` for an
    /// untitled buffer). Diagnostics are sorted by position.
    pub fn check(&mut self, text: &str, path: Option<&Path>) -> Vec<Diagnostic> {
        let scope = regions::check_scope(text, path);
        self.check_scope(text, scope)
    }

    fn check_scope(&mut self, text: &str, scope: CheckScope) -> Vec<Diagnostic> {
        let prose = matches!(scope, CheckScope::Document(_));
        for rule in PROSE_ONLY_RULES {
            self.linter.config.set_rule_enabled(rule, prose);
        }
        match scope {
            CheckScope::Document(ProseFormat::Plain) => self.lint_with(text, &PlainEnglish),
            CheckScope::Document(ProseFormat::Markdown) => {
                self.lint_with(text, &Markdown::default())
            }
            CheckScope::Skip => Vec::new(),
            CheckScope::Regions(regions) if regions.is_empty() => Vec::new(),
            CheckScope::Regions(regions) => {
                let parser = RegionParser::new(text, &regions);
                let mut diagnostics = self.lint_with(text, &parser);
                // A lint across two regions (say, a repeated word over a line
                // break) also covers the comment markers or code between
                // them. Show it, but offer no one-click fix that could
                // rewrite that code.
                for diagnostic in &mut diagnostics {
                    if !within_one_region(&regions, &diagnostic.bytes) {
                        diagnostic.suggestions.clear();
                    }
                }
                diagnostics
            }
        }
    }

    fn lint_with(&mut self, text: &str, parser: &impl Parser) -> Vec<Diagnostic> {
        let document = Document::new(text, parser, self.dictionary.as_ref());
        let offsets = char_byte_offsets(text);
        let mut out: Vec<Diagnostic> = self
            .linter
            .lint(&document)
            .iter()
            .filter_map(|lint| to_diagnostic(lint, text, &offsets))
            .collect();
        out.sort_by_key(|d| (d.bytes.start, d.bytes.end));
        out
    }
}

/// Whether `bytes` lies entirely inside one of the sorted `regions`.
fn within_one_region(regions: &[Region], bytes: &Range<usize>) -> bool {
    let after = regions.partition_point(|r| r.bytes.start <= bytes.start);
    after
        .checked_sub(1)
        .and_then(|i| regions.get(i))
        .is_some_and(|r| bytes.end <= r.bytes.end)
}

/// `offsets[i]` is the byte offset of char `i`; the final entry is
/// `text.len()`, so an exclusive char end maps cleanly too.
fn char_byte_offsets(text: &str) -> Vec<usize> {
    text.char_indices()
        .map(|(byte, _)| byte)
        .chain(std::iter::once(text.len()))
        .collect()
}

/// Convert a Harper lint (char span) into a [`Diagnostic`]. Zero-width or
/// out-of-range spans are dropped: there is nothing to underline.
fn to_diagnostic(lint: &Lint, text: &str, offsets: &[usize]) -> Option<Diagnostic> {
    let chars = lint.span.start..lint.span.end;
    let start = *offsets.get(chars.start)?;
    let end = *offsets.get(chars.end)?;
    if start >= end {
        return None;
    }
    let flagged = text.get(start..end)?.to_owned();

    let mut suggestions: Vec<String> = Vec::new();
    for suggestion in &lint.suggestions {
        let replacement = match suggestion {
            Suggestion::ReplaceWith(with) => with.iter().collect(),
            Suggestion::InsertAfter(insert) => {
                let mut s = flagged.clone();
                s.extend(insert.iter());
                s
            }
            Suggestion::Remove => String::new(),
        };
        if replacement != flagged && !suggestions.contains(&replacement) {
            suggestions.push(replacement);
        }
        if suggestions.len() == MAX_SUGGESTIONS {
            break;
        }
    }

    let kind = if lint.lint_kind == LintKind::Spelling {
        DiagnosticKind::Spelling
    } else {
        DiagnosticKind::Grammar
    };

    Some(Diagnostic {
        kind,
        bytes: start..end,
        chars,
        flagged,
        message: lint.message.clone(),
        suggestions,
    })
}

/// A region converted to the char-indexed span Harper works in.
struct CharRegion {
    kind: RegionKind,
    start: usize,
    end: usize,
    boundary: Boundary,
}

/// A Harper [`Parser`] that tokenises only the given regions of a code
/// buffer, leaving everything else out of the document. Token spans stay
/// relative to the whole buffer, so lint positions need no remapping.
///
/// This is Harper's `Mask` parser plus one refinement: a run of comment lines
/// that continue one another (`/// a\n/// b`) is joined with plain newlines
/// and parsed as *one* Markdown document, then mapped back onto the buffer.
/// A sentence wrapped across lines reads as one sentence, and Markdown
/// spanning lines (fenced code examples in doc comments) is recognised as
/// such instead of being checked as prose.
struct RegionParser {
    regions: Vec<CharRegion>,
}

impl RegionParser {
    fn new(text: &str, regions: &[Region]) -> Self {
        let offsets = char_byte_offsets(text);
        let to_char = |byte: usize| offsets.partition_point(|&b| b < byte);
        let regions = regions
            .iter()
            .map(|region| CharRegion {
                kind: region.kind,
                start: to_char(region.bytes.start),
                end: to_char(region.bytes.end),
                boundary: region.boundary,
            })
            .filter(|region| region.start < region.end)
            .collect();
        Self { regions }
    }

    /// Split the regions into runs: a region plus every following one that
    /// continues it.
    fn runs(&self) -> impl Iterator<Item = &[CharRegion]> {
        self.regions
            .chunk_by(|_, next| next.boundary == Boundary::ContinuesComment)
    }
}

impl Parser for RegionParser {
    fn parse(&self, source: &[char]) -> Vec<Token> {
        let mut tokens = Vec::new();
        let mut previous_end: Option<usize> = None;

        for run in self.runs() {
            let (Some(first), Some(last)) = (run.first(), run.last()) else {
                continue;
            };
            if let Some(end) = previous_end
                && end < first.start
            {
                tokens.push(Token::new(
                    Span::new(end, first.start),
                    TokenKind::ParagraphBreak,
                ));
            }
            tokens.extend(parse_run(source, run));
            previous_end = Some(last.end);
        }

        tokens
    }
}

/// Parse one run of regions as a single document. The run's contents are
/// concatenated (joined by `\n`) with a map from each concatenated char back
/// to its buffer position; token spans are translated through that map.
fn parse_run(source: &[char], run: &[CharRegion]) -> Vec<Token> {
    let mut joined: Vec<char> = Vec::new();
    let mut origin: Vec<usize> = Vec::new();
    let mut previous: Option<&CharRegion> = None;
    for region in run {
        let Some(content) = source.get(region.start..region.end) else {
            continue;
        };
        if let Some(prev) = previous {
            // The joiner stands for the real line break between the two.
            let newline = source
                .get(prev.end..region.start)
                .and_then(|gap| gap.iter().position(|&c| c == '\n'))
                .map_or(prev.end, |at| prev.end + at);
            joined.push('\n');
            origin.push(newline);
        }
        joined.extend_from_slice(content);
        origin.extend(region.start..region.end);
        previous = Some(region);
    }

    let parsed = match run.first().map(|r| r.kind) {
        Some(RegionKind::Comment) => Markdown::default().parse(&joined),
        Some(RegionKind::String) => PlainEnglish.parse(&joined),
        None => Vec::new(),
    };

    parsed
        .into_iter()
        .filter_map(|mut token| {
            let start = *origin.get(token.span.start)?;
            let end = if token.span.end > token.span.start {
                *origin.get(token.span.end - 1)? + 1
            } else {
                start
            };
            if end < start {
                return None;
            }
            token.span = Span::new(start, end);
            Some(token)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    // `expect` is acceptable in test code: a panic on an unexpected `None`
    // is exactly the failure signal we want from a test.
    #![allow(clippy::expect_used)]

    use super::*;

    /// Check `text` as the file `name` (`""` for an untitled buffer).
    fn check(text: &str, name: &str) -> Vec<Diagnostic> {
        let path = (!name.is_empty()).then(|| Path::new(name));
        Proofreader::new(&[]).check(text, path)
    }

    fn spelling(diagnostics: &[Diagnostic]) -> Vec<&str> {
        diagnostics
            .iter()
            .filter(|d| d.kind == DiagnosticKind::Spelling)
            .map(|d| d.flagged.as_str())
            .collect()
    }

    #[test]
    fn misspelled_word_is_a_spelling_diagnostic_with_suggestions() {
        let text = "This sentennce has a typo.";
        let diagnostics = check(text, "");
        let typo = diagnostics
            .iter()
            .find(|d| d.flagged == "sentennce")
            .expect("the typo is flagged");
        assert_eq!(typo.kind, DiagnosticKind::Spelling);
        assert_eq!(&text[typo.bytes.clone()], "sentennce");
        assert!(
            typo.suggestions.iter().any(|s| s == "sentence"),
            "suggestions were {:?}",
            typo.suggestions
        );
    }

    #[test]
    fn clean_prose_has_no_diagnostics() {
        assert!(check("The quick brown fox jumps over the lazy dog.", "").is_empty());
    }

    #[test]
    fn grammar_issue_is_a_grammar_diagnostic() {
        let diagnostics = check("I ate a apple yesterday.", "");
        assert!(
            diagnostics
                .iter()
                .any(|d| d.kind == DiagnosticKind::Grammar),
            "expected a grammar diagnostic, got {diagnostics:?}"
        );
    }

    #[test]
    fn user_words_are_accepted() {
        let text = "I love frext a lot.";
        assert_eq!(spelling(&check(text, "")), vec!["frext"]);
        let with_word = Proofreader::new(&["frext".to_owned()]).check(text, None);
        assert!(spelling(&with_word).is_empty());
    }

    #[test]
    fn byte_and_char_ranges_agree_after_multibyte_text() {
        let text = "Café crème is nice, but this wrrod is wrong.";
        let diagnostics = check(text, "");
        let typo = diagnostics
            .iter()
            .find(|d| d.flagged == "wrrod")
            .expect("the typo is flagged");
        assert_eq!(&text[typo.bytes.clone()], "wrrod");
        let by_chars: String = text
            .chars()
            .skip(typo.chars.start)
            .take(typo.chars.len())
            .collect();
        assert_eq!(by_chars, "wrrod");
    }

    #[test]
    fn code_is_checked_only_in_comments_and_strings() {
        let text = "// This kommentt has a typo.\n\
                    fn mispelld_fn(arguumnt: u32) {\n\
                    \x20   println!(\"an erorrr in a string\");\n\
                    }\n";
        let diagnostics = check(text, "x.rs");
        let flagged = spelling(&diagnostics);
        assert!(flagged.contains(&"kommentt"), "flagged: {flagged:?}");
        assert!(flagged.contains(&"erorrr"), "flagged: {flagged:?}");
        assert!(
            !flagged
                .iter()
                .any(|w| w.contains("mispelld") || w.contains("arguumnt")),
            "identifiers must not be checked: {flagged:?}"
        );
    }

    #[test]
    fn wrapped_line_comments_have_no_whitespace_complaints() {
        let text =
            "fn f() {\n    /// This sentence is wrapped across\n    /// two comment lines.\n}\n";
        let diagnostics = check(text, "x.rs");
        assert!(diagnostics.is_empty(), "got {diagnostics:?}");
    }

    #[test]
    fn lowercase_sentences_are_fine_in_code_but_not_in_prose() {
        let code = "log::error!(\"failed to open the file\");\n";
        assert!(
            check(code, "x.rs").is_empty(),
            "got {:?}",
            check(code, "x.rs")
        );
        let prose = check("This is fine. failed to open the file.", "");
        assert!(
            prose.iter().any(|d| d.kind == DiagnosticKind::Grammar),
            "prose keeps the capitalisation rule: {prose:?}"
        );
    }

    #[test]
    fn code_spans_in_doc_comments_are_not_checked() {
        let text = "/// Returns `xyzzyfoo` when ready.\nfn f() {}\n";
        assert!(spelling(&check(text, "x.rs")).is_empty());
    }

    #[test]
    fn markdown_code_is_not_checked() {
        let text = "# Notes\n\nRun `cargo xyzzyfoo` first.\n\n```\nqwzrtx\n```\n";
        assert!(spelling(&check(text, "x.md")).is_empty());
    }

    #[test]
    fn stale_diagnostics_are_detected() {
        let text = "a wrrod here";
        let diagnostic = check(text, "")
            .into_iter()
            .find(|d| d.flagged == "wrrod")
            .expect("the typo is flagged");
        assert!(diagnostic.is_current(text));
        assert!(!diagnostic.is_current("a word here"));
        assert!(!diagnostic.is_current("short"));
    }

    #[test]
    fn a_char_shift_with_the_same_byte_offset_is_stale() {
        // `é` (2 bytes, 1 char) -> `ab` (2 bytes, 2 chars): the flagged word
        // keeps its byte range but moves one character to the right.
        let text = "é wrrod";
        let diagnostic = check(text, "")
            .into_iter()
            .find(|d| d.flagged == "wrrod")
            .expect("the typo is flagged");
        assert!(diagnostic.is_current(text));
        assert!(!diagnostic.is_current("ab wrrod"));
    }

    #[test]
    fn fenced_code_in_a_doc_comment_is_not_checked() {
        let text = "/// Example:\n///\n/// ```\n/// let qwzrtx = zorbify();\n/// ```\nfn f() {}\n";
        let diagnostics = check(text, "x.rs");
        assert!(
            spelling(&diagnostics).is_empty(),
            "code inside the fence was checked: {diagnostics:?}"
        );
    }

    #[test]
    fn a_typo_on_a_continued_comment_line_maps_back_to_the_buffer() {
        let text = "/// The first line and\n/// a wrrod on the second.\nfn f() {}\n";
        let typo = check(text, "x.rs")
            .into_iter()
            .find(|d| d.flagged == "wrrod")
            .expect("the typo is flagged");
        assert_eq!(&text[typo.bytes.clone()], "wrrod");
        assert!(typo.is_current(text));
    }

    #[test]
    fn a_lint_spanning_comment_lines_offers_no_fixes() {
        // Harper flags the repeated "the" across the line break. Its range
        // covers the `///` marker between them, so applying a fix could
        // rewrite comment syntax: it is shown, but with no fixes.
        let text = "/// Please read the\n/// the docs first.\nfn f() {}\n";
        let repeated = check(text, "x.rs")
            .into_iter()
            .find(|d| d.flagged.contains("///"))
            .expect("the repeated word across lines is flagged");
        assert!(repeated.suggestions.is_empty(), "{repeated:?}");
    }

    #[test]
    fn within_one_region_requires_full_containment() {
        let region = |bytes: Range<usize>| Region {
            kind: RegionKind::Comment,
            bytes,
            boundary: Boundary::Separate,
        };
        let regions = [region(0..10), region(20..30)];
        assert!(within_one_region(&regions, &(2..8)));
        assert!(within_one_region(&regions, &(20..30)));
        assert!(!within_one_region(&regions, &(8..22)));
        assert!(!within_one_region(&regions, &(12..14)));
    }

    #[test]
    fn skipped_files_have_no_diagnostics() {
        assert!(check("A wrrod here.", "a.definitelynotalang").is_empty());
    }
}
