//! Which parts of a buffer are prose worth proofreading.
//!
//! Plain-text and Markdown buffers are checked in full. Every other language
//! syntect recognises is treated as code: only its comments and string
//! literals are checked, so identifiers and keywords never collect squiggles.
//! The comment/string split comes from syntect's scope stack, the same parse
//! the highlighter uses, so a language highlights and proofreads consistently.

use std::ops::Range;
use std::sync::OnceLock;

use two_face::re_exports::syntect::{
    parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet},
    util::LinesWithEndings,
};

/// How a whole-buffer prose document should be parsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProseFormat {
    /// Unstructured text: every character is prose.
    Plain,
    /// Markdown: code spans/blocks and markup are skipped.
    Markdown,
}

/// The kind of code construct a [`Region`] was extracted from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionKind {
    /// A line or block comment (including doc comments). Parsed as Markdown,
    /// since doc comments routinely carry `code spans`.
    Comment,
    /// A string literal. Parsed as plain English.
    String,
}

/// One proofreadable slice of a code buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Region {
    /// What the slice was extracted from.
    pub kind: RegionKind,
    /// Byte range of the slice within the buffer, trimmed of surrounding
    /// whitespace and of comment/string delimiters.
    pub bytes: Range<usize>,
}

/// What to proofread in a buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckScope {
    /// Check the whole buffer as prose.
    Document(ProseFormat),
    /// Check only these regions (comments and strings of a code buffer),
    /// sorted and non-overlapping.
    Regions(Vec<Region>),
}

/// Decide what to proofread in `text`, given its file-extension `language`
/// token (see [`crate::highlight::language_from_path`]).
#[must_use]
pub fn check_scope(text: &str, language: &str) -> CheckScope {
    let syntaxes = crate::highlight::syntax_set();
    let Some(syntax) = syntaxes.find_syntax_by_extension(language) else {
        return CheckScope::Document(ProseFormat::Plain);
    };
    match syntax.name.as_str() {
        "Plain Text" => CheckScope::Document(ProseFormat::Plain),
        "Markdown" | "MultiMarkdown" => CheckScope::Document(ProseFormat::Markdown),
        _ => CheckScope::Regions(code_regions(text, syntax, syntaxes)),
    }
}

/// The scope prefixes used to classify a syntect scope stack.
struct Selectors {
    comment: Scope,
    string: Scope,
    /// Scopes nested *inside* a comment or string that are not prose:
    /// delimiters, escapes, format placeholders, interpolations, regexes.
    excluded: Vec<Scope>,
}

/// Build the selectors once. `None` only if syntect's global scope repository
/// rejects a name, in which case proofreading code buffers is disabled rather
/// than panicking.
fn selectors() -> Option<&'static Selectors> {
    static SELECTORS: OnceLock<Option<Selectors>> = OnceLock::new();
    SELECTORS
        .get_or_init(|| {
            let excluded = [
                "punctuation.definition.comment",
                "punctuation.definition.string",
                "punctuation.section.interpolation",
                "constant.character.escape",
                "constant.other.placeholder",
                "meta.interpolation",
                "meta.embedded",
                "storage.type.string",
                "string.regexp",
            ]
            .into_iter()
            .map(Scope::new)
            .collect::<Result<Vec<_>, _>>()
            .ok()?;
            Some(Selectors {
                comment: Scope::new("comment").ok()?,
                string: Scope::new("string").ok()?,
                excluded,
            })
        })
        .as_ref()
}

/// Classify text under `stack`. The innermost relevant scope wins, so an
/// escape or interpolation nested inside a string excludes just that part,
/// while an embedded-language wrapper *outside* a comment does not hide it.
fn classify(stack: &ScopeStack, selectors: &Selectors) -> Option<RegionKind> {
    for scope in stack.as_slice().iter().rev() {
        if selectors.excluded.iter().any(|e| e.is_prefix_of(*scope)) {
            return None;
        }
        if selectors.comment.is_prefix_of(*scope) {
            return Some(RegionKind::Comment);
        }
        if selectors.string.is_prefix_of(*scope) {
            return Some(RegionKind::String);
        }
    }
    None
}

/// Extract the comment and string regions of a code buffer.
///
/// A syntect parse error stops extraction early and returns what was found so
/// far: a partially-checked buffer is better than a crashed editor.
fn code_regions(text: &str, syntax: &SyntaxReference, syntaxes: &SyntaxSet) -> Vec<Region> {
    let Some(selectors) = selectors() else {
        return Vec::new();
    };

    let mut raw: Vec<Region> = Vec::new();
    let mut push = |kind: RegionKind, bytes: Range<usize>| {
        if bytes.is_empty() {
            return;
        }
        match raw.last_mut() {
            Some(last) if last.kind == kind && last.bytes.end == bytes.start => {
                last.bytes.end = bytes.end;
            }
            _ => raw.push(Region { kind, bytes }),
        }
    };

    let mut state = ParseState::new(syntax);
    let mut stack = ScopeStack::new();
    let mut line_start = 0;
    'lines: for line in LinesWithEndings::from(text) {
        let Ok(ops) = state.parse_line(line, syntaxes) else {
            break;
        };
        let mut seg_start = 0;
        for (offset, op) in ops {
            if let Some(kind) = classify(&stack, selectors) {
                push(kind, line_start + seg_start..line_start + offset);
            }
            if stack.apply(&op).is_err() {
                break 'lines;
            }
            seg_start = offset;
        }
        if let Some(kind) = classify(&stack, selectors) {
            push(kind, line_start + seg_start..line_start + line.len());
        }
        line_start += line.len();
    }

    raw.into_iter()
        .filter_map(|region| trim(text, region))
        .filter(|region| is_prose(text, region))
        .collect()
}

/// Shrink a region to exclude surrounding whitespace; `None` if nothing is
/// left.
fn trim(text: &str, region: Region) -> Option<Region> {
    let slice = text.get(region.bytes.clone())?;
    let leading = slice.len() - slice.trim_start().len();
    let trimmed = slice.trim();
    if trimmed.is_empty() {
        return None;
    }
    let start = region.bytes.start + leading;
    Some(Region {
        kind: region.kind,
        bytes: start..start + trimmed.len(),
    })
}

/// Whether a region reads as prose. Comments always do. A string literal
/// with no whitespace in it is almost always a key, path, identifier, or
/// format token (`"frext_editor"`, `"utf-8"`, `"%Y-%m-%d"`) rather than a
/// sentence, so it is skipped to keep code buffers free of false positives.
fn is_prose(text: &str, region: &Region) -> bool {
    match region.kind {
        RegionKind::Comment => true,
        RegionKind::String => text
            .get(region.bytes.clone())
            .is_some_and(|s| s.contains(char::is_whitespace)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The text of every region, for readable assertions.
    fn region_texts<'a>(text: &'a str, scope: &CheckScope) -> Vec<(RegionKind, &'a str)> {
        match scope {
            CheckScope::Regions(regions) => regions
                .iter()
                .map(|r| (r.kind, &text[r.bytes.clone()]))
                .collect(),
            CheckScope::Document(format) => panic!("expected regions, got {format:?}"),
        }
    }

    #[test]
    fn untitled_and_unknown_buffers_are_plain_prose() {
        assert_eq!(
            check_scope("hello", ""),
            CheckScope::Document(ProseFormat::Plain)
        );
        assert_eq!(
            check_scope("hello", "definitelynotalang"),
            CheckScope::Document(ProseFormat::Plain)
        );
        assert_eq!(
            check_scope("hello", "txt"),
            CheckScope::Document(ProseFormat::Plain)
        );
    }

    #[test]
    fn markdown_is_checked_as_a_markdown_document() {
        assert_eq!(
            check_scope("# Title", "md"),
            CheckScope::Document(ProseFormat::Markdown)
        );
    }

    #[test]
    fn rust_comments_are_extracted_without_their_markers() {
        let text = "// first comment\nfn main() {} /* block comment */\n";
        let scope = check_scope(text, "rs");
        assert_eq!(
            region_texts(text, &scope),
            vec![
                (RegionKind::Comment, "first comment"),
                (RegionKind::Comment, "block comment"),
            ]
        );
    }

    #[test]
    fn identifiers_and_keywords_are_never_extracted() {
        let text = "fn spelt_wrogn(arguumnt: u32) -> u32 { arguumnt }\n";
        assert_eq!(check_scope(text, "rs"), CheckScope::Regions(Vec::new()));
    }

    #[test]
    fn sentence_strings_are_extracted_without_quotes() {
        let text = "let msg = \"failed to open the file\";\n";
        let scope = check_scope(text, "rs");
        assert_eq!(
            region_texts(text, &scope),
            vec![(RegionKind::String, "failed to open the file")]
        );
    }

    #[test]
    fn single_token_strings_are_skipped() {
        let text = "let id = \"frext_editor\"; let enc = \"utf-8\";\n";
        assert_eq!(check_scope(text, "rs"), CheckScope::Regions(Vec::new()));
    }

    #[test]
    fn escapes_inside_strings_split_the_region() {
        let text = "let s = \"first line\\nsecond line\";\n";
        let scope = check_scope(text, "rs");
        assert_eq!(
            region_texts(text, &scope),
            vec![
                (RegionKind::String, "first line"),
                (RegionKind::String, "second line"),
            ]
        );
    }

    #[test]
    fn python_hash_comments_and_docstrings_are_extracted() {
        let text = "# a hash comment\ndef f():\n    \"\"\"A docstring here.\"\"\"\n";
        let scope = check_scope(text, "py");
        let texts = region_texts(text, &scope);
        assert!(texts.contains(&(RegionKind::Comment, "a hash comment")));
        assert!(
            texts.iter().any(|(_, t)| t.contains("A docstring here.")),
            "docstring missing from {texts:?}"
        );
    }

    #[test]
    fn regions_are_sorted_and_disjoint_with_multibyte_text() {
        let text = "// café naïve\nlet s = \"über cool text\"; // ünïcödé comment\n";
        let CheckScope::Regions(regions) = check_scope(text, "rs") else {
            panic!("expected regions");
        };
        assert!(!regions.is_empty());
        for pair in regions.windows(2) {
            assert!(pair[0].bytes.end <= pair[1].bytes.start);
        }
        for region in &regions {
            assert!(
                text.get(region.bytes.clone()).is_some(),
                "not on a char boundary"
            );
        }
    }
}
