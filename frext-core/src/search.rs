//! Text search over a single buffer: a small query model plus a compiled
//! matcher that drives both match highlighting and find/replace.
//!
//! Every mode (plain substring, regex, whole-word, case-(in)sensitive) is
//! expressed as a single [`regex::Regex`], so the search and replace paths
//! share one engine. Plain queries are escaped before compilation; whole-word
//! queries are wrapped in `\b…\b`; case-insensitivity is the `(?i)` flag.
//!
//! Replacement is capture-aware in regex mode (`$1`, `${name}`) and literal in
//! plain mode, so a user typing `$` into a plain replacement gets a `$`.

use std::ops::Range;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::error::SearchError;

/// A user-entered search query and its toggles.
///
/// `Serialize`/`Deserialize` so the last query can be restored across
/// sessions. The default is an empty, plain, case-insensitive query.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchQuery {
    /// The raw text the user typed.
    pub pattern: String,
    /// Match case exactly when `true`; otherwise case-insensitive.
    #[serde(default)]
    pub case_sensitive: bool,
    /// Treat `pattern` as a regular expression when `true`; otherwise as a
    /// literal substring.
    #[serde(default)]
    pub regex: bool,
    /// Require word boundaries around each match when `true`.
    #[serde(default)]
    pub whole_word: bool,
}

impl SearchQuery {
    /// Compile this query into a [`Matcher`].
    ///
    /// # Errors
    ///
    /// Returns [`SearchError::InvalidRegex`] when `regex` is set and the
    /// pattern is not a valid regular expression.
    pub fn compile(&self) -> Result<Matcher, SearchError> {
        // Build the regex source for the chosen mode.
        let core = if self.regex {
            self.pattern.clone()
        } else {
            regex::escape(&self.pattern)
        };

        let bounded = if self.whole_word {
            format!(r"\b(?:{core})\b")
        } else {
            core
        };

        let mut builder = regex::RegexBuilder::new(&bounded);
        builder.case_insensitive(!self.case_sensitive);
        // Let `.` match across the buffer's lines is undesirable for an
        // editor search; keep the default (`.` excludes `\n`).
        let regex = builder
            .build()
            .map_err(|err| SearchError::InvalidRegex(err.to_string()))?;

        Ok(Matcher {
            regex,
            literal_replacement: !self.regex,
        })
    }

    /// Whether the query is empty (nothing to search for).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pattern.is_empty()
    }
}

/// A compiled search query ready to run against buffer text.
#[derive(Debug, Clone)]
pub struct Matcher {
    regex: Regex,
    /// In plain (non-regex) mode the replacement string is treated literally
    /// (a `$` is just a `$`); in regex mode `$1` / `${name}` expand captures.
    literal_replacement: bool,
}

impl Matcher {
    /// All non-overlapping match byte ranges within `text`, in order.
    ///
    /// Zero-width matches (e.g. an anchor-only regex) are skipped so callers
    /// never see empty highlights and the iteration always terminates.
    #[must_use]
    pub fn find_matches(&self, text: &str) -> Vec<Range<usize>> {
        self.regex
            .find_iter(text)
            .map(|m| m.range())
            .filter(|r| r.start != r.end)
            .collect()
    }

    /// Match byte ranges that fall entirely within `scope` (a byte range of
    /// `text`), for search-within-selection.
    #[must_use]
    pub fn find_matches_in(&self, text: &str, scope: Range<usize>) -> Vec<Range<usize>> {
        self.find_matches(text)
            .into_iter()
            .filter(|m| m.start >= scope.start && m.end <= scope.end)
            .collect()
    }

    /// Replace every match in `text` with `replacement`, returning the new
    /// string. Capture references in `replacement` expand in regex mode and
    /// are literal in plain mode.
    #[must_use]
    pub fn replace_all(&self, text: &str, replacement: &str) -> String {
        if self.literal_replacement {
            self.regex
                .replace_all(text, regex::NoExpand(replacement))
                .into_owned()
        } else {
            self.regex.replace_all(text, replacement).into_owned()
        }
    }

    /// Replace the single match covering byte offset `at` (the start of the
    /// current match) with `replacement`, returning the new string and the
    /// byte range the replacement now occupies. Returns `None` when no match
    /// starts at `at`.
    #[must_use]
    pub fn replace_one_at(
        &self,
        text: &str,
        at: usize,
        replacement: &str,
    ) -> Option<(String, Range<usize>)> {
        let m = self.regex.find_at(text, at).filter(|m| m.start() == at)?;

        let expanded = if self.literal_replacement {
            replacement.to_owned()
        } else {
            let caps = self.regex.captures(&text[m.start()..])?;
            let mut buf = String::new();
            caps.expand(replacement, &mut buf);
            buf
        };

        let mut out = String::with_capacity(text.len());
        out.push_str(&text[..m.start()]);
        let new_start = out.len();
        out.push_str(&expanded);
        let new_end = out.len();
        out.push_str(&text[m.end()..]);

        Some((out, new_start..new_end))
    }
}

#[cfg(test)]
mod tests {
    // `unwrap`/`expect` are acceptable in tests: a panic on an unexpected
    // `Err`/`None` is exactly the failure signal we want.
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn query(pattern: &str, case_sensitive: bool, regex: bool, whole_word: bool) -> SearchQuery {
        SearchQuery {
            pattern: pattern.to_owned(),
            case_sensitive,
            regex,
            whole_word,
        }
    }

    #[test]
    fn plain_search_is_case_insensitive_by_default() {
        let m = query("foo", false, false, false).compile().unwrap();
        assert_eq!(m.find_matches("Foo foo FOO"), vec![0..3, 4..7, 8..11]);
    }

    #[test]
    fn plain_search_respects_case_sensitivity() {
        let m = query("foo", true, false, false).compile().unwrap();
        assert_eq!(m.find_matches("Foo foo FOO"), vec![4..7]);
    }

    #[test]
    fn plain_search_escapes_regex_metacharacters() {
        // The dot must match a literal dot, not "any character".
        let m = query("a.c", false, false, false).compile().unwrap();
        assert_eq!(m.find_matches("a.c abc axc"), vec![0..3]);
    }

    #[test]
    fn regex_search_matches_patterns() {
        let m = query(r"\d+", false, true, false).compile().unwrap();
        assert_eq!(m.find_matches("ab12cd345"), vec![2..4, 6..9]);
    }

    #[test]
    fn invalid_regex_is_a_typed_error() {
        let err = query("(", false, true, false).compile().unwrap_err();
        assert!(matches!(err, SearchError::InvalidRegex(_)));
    }

    #[test]
    fn whole_word_requires_boundaries() {
        let m = query("cat", false, false, true).compile().unwrap();
        // "cat" matches; "category" and "scat" do not.
        assert_eq!(m.find_matches("cat category scat cat"), vec![0..3, 18..21]);
    }

    #[test]
    fn zero_width_matches_are_skipped() {
        // `a*` can match empty; we must not emit empty ranges or loop forever.
        let m = query("a*", false, true, false).compile().unwrap();
        assert_eq!(m.find_matches("baab"), vec![1..3]);
    }

    #[test]
    fn find_matches_in_scope_filters_to_selection() {
        let m = query("a", false, false, false).compile().unwrap();
        // Full text has matches at 0, 2, 4; scope 2..5 keeps 2 and 4.
        assert_eq!(m.find_matches_in("a a a", 2..5), vec![2..3, 4..5]);
    }

    #[test]
    fn replace_all_is_literal_in_plain_mode() {
        let m = query("foo", false, false, false).compile().unwrap();
        // A `$1` in a plain replacement stays literal.
        assert_eq!(m.replace_all("foo foo", "$1"), "$1 $1");
    }

    #[test]
    fn replace_all_expands_captures_in_regex_mode() {
        let m = query(r"(\w+)@(\w+)", false, true, false).compile().unwrap();
        assert_eq!(m.replace_all("user@host", "$2.$1"), "host.user");
    }

    #[test]
    fn replace_one_at_replaces_only_the_targeted_match() {
        let m = query("foo", false, false, false).compile().unwrap();
        let (out, range) = m.replace_one_at("foo foo", 4, "bar").unwrap();
        assert_eq!(out, "foo bar");
        assert_eq!(range, 4..7);
        // No match starts at offset 1.
        assert!(m.replace_one_at("foo foo", 1, "bar").is_none());
    }
}
