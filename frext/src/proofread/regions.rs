//! Which parts of a buffer are prose worth proofreading.
//!
//! Untitled, plain-text, and Markdown buffers are checked in full, as are
//! extensionless files with no known syntax (`README`, `LICENSE`,
//! `COMMIT_EDITMSG`). Every other language syntect recognises is treated as
//! code: only its comments and string literals are checked, so identifiers
//! and keywords never collect squiggles. Files with an unrecognised
//! extension, and unrecognised dotfiles, are not checked at all: they are
//! almost always configuration, not prose.
//!
//! The comment/string split comes from syntect's scope stack, the same parse
//! the highlighter uses, so a language highlights and proofreads consistently.

use std::ffi::OsStr;
use std::ops::Range;
use std::path::Path;
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

/// How a [`Region`] relates to the region before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Boundary {
    /// A separate piece of prose (a new paragraph).
    Separate,
    /// The next line of the previous comment: the two are separated by one
    /// line break and nothing but whitespace and comment markers (`//`,
    /// `///`, `#`, …), so they belong to one paragraph or Markdown block.
    ContinuesComment,
}

/// One proofreadable slice of a code buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Region {
    /// What the slice was extracted from.
    pub kind: RegionKind,
    /// Byte range of the slice within the buffer, trimmed of surrounding
    /// whitespace and of comment/string delimiters.
    pub bytes: Range<usize>,
    /// Whether this region continues the previous comment.
    pub boundary: Boundary,
}

/// What to proofread in a buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckScope {
    /// Check the whole buffer as prose.
    Document(ProseFormat),
    /// Check only these regions (comments and strings of a code buffer),
    /// sorted and non-overlapping.
    Regions(Vec<Region>),
    /// Do not check this buffer at all.
    Skip,
}

/// Decide what to proofread in `text`, the buffer of the file at `path`
/// (`None` for an untitled buffer).
///
/// The syntax is resolved by extension, then by whole file name, so
/// `Makefile`, `Dockerfile`, and friends are recognised as code.
#[must_use]
pub fn check_scope(text: &str, path: Option<&Path>) -> CheckScope {
    let Some(path) = path else {
        return CheckScope::Document(ProseFormat::Plain);
    };
    let syntaxes = crate::highlight::syntax_set();
    let extension = path
        .extension()
        .and_then(OsStr::to_str)
        .map(str::to_ascii_lowercase);
    let file_name = path.file_name().and_then(OsStr::to_str);
    let syntax = extension
        .as_deref()
        .and_then(|ext| syntaxes.find_syntax_by_extension(ext))
        .or_else(|| file_name.and_then(|name| syntaxes.find_syntax_by_extension(name)));

    match syntax {
        Some(syntax) => match syntax.name.as_str() {
            // A commit message is prose, not code with prose comments.
            "Plain Text" | "Git Commit" => CheckScope::Document(ProseFormat::Plain),
            "Markdown" | "MultiMarkdown" => CheckScope::Document(ProseFormat::Markdown),
            _ => CheckScope::Regions(code_regions(text, syntax, syntaxes)),
        },
        None if extension.is_none() && !file_name.is_some_and(|n| n.starts_with('.')) => {
            CheckScope::Document(ProseFormat::Plain)
        }
        None => CheckScope::Skip,
    }
}

/// The scope prefixes used to classify a syntect scope stack.
struct Selectors {
    comment: Scope,
    string: Scope,
    /// Comment delimiters (`//`, `#`, `/*`). Not prose, but they do not
    /// break a comment that continues onto the next line.
    comment_marker: Scope,
    /// Other scopes nested *inside* a comment or string that are not prose:
    /// string delimiters, escapes, format placeholders, interpolations,
    /// regexes.
    excluded: Vec<Scope>,
}

/// What a run of buffer text is, for region extraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Prose(RegionKind),
    CommentMarker,
    Other,
}

/// Build the selectors once. `None` only if syntect's global scope repository
/// rejects a name, in which case proofreading code buffers is disabled rather
/// than panicking.
fn selectors() -> Option<&'static Selectors> {
    static SELECTORS: OnceLock<Option<Selectors>> = OnceLock::new();
    SELECTORS
        .get_or_init(|| {
            let excluded = [
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
                comment_marker: Scope::new("punctuation.definition.comment").ok()?,
                excluded,
            })
        })
        .as_ref()
}

/// Classify text under `stack`. The innermost relevant scope wins, so an
/// escape or interpolation nested inside a string excludes just that part,
/// while an embedded-language wrapper *outside* a comment does not hide it.
fn classify(stack: &ScopeStack, selectors: &Selectors) -> Class {
    for scope in stack.as_slice().iter().rev() {
        if selectors.comment_marker.is_prefix_of(*scope) {
            return Class::CommentMarker;
        }
        if selectors.excluded.iter().any(|e| e.is_prefix_of(*scope)) {
            return Class::Other;
        }
        if selectors.comment.is_prefix_of(*scope) {
            return Class::Prose(RegionKind::Comment);
        }
        if selectors.string.is_prefix_of(*scope) {
            return Class::Prose(RegionKind::String);
        }
    }
    Class::Other
}

/// A region as first extracted, before trimming and filtering.
struct RawRegion {
    kind: RegionKind,
    bytes: Range<usize>,
    /// Whether everything between the previous raw region and this one was
    /// whitespace or comment markers.
    clean_gap: bool,
}

/// Extract the comment and string regions of a code buffer.
///
/// A syntect parse error stops extraction early and returns what was found so
/// far: a partially-checked buffer is better than a crashed editor.
fn code_regions(text: &str, syntax: &SyntaxReference, syntaxes: &SyntaxSet) -> Vec<Region> {
    let Some(selectors) = selectors() else {
        return Vec::new();
    };

    let mut raw: Vec<RawRegion> = Vec::new();
    // Whether the text since the last raw region has been only whitespace
    // and comment markers.
    let mut gap_clean = true;
    let mut visit = |class: Class, bytes: Range<usize>| {
        if bytes.is_empty() {
            return;
        }
        match class {
            Class::Prose(kind) => {
                match raw.last_mut() {
                    Some(last) if last.kind == kind && last.bytes.end == bytes.start => {
                        last.bytes.end = bytes.end;
                    }
                    _ => raw.push(RawRegion {
                        kind,
                        bytes,
                        clean_gap: gap_clean,
                    }),
                }
                gap_clean = true;
            }
            Class::CommentMarker => {}
            Class::Other => {
                if !text.get(bytes).is_some_and(|s| s.trim().is_empty()) {
                    gap_clean = false;
                }
            }
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
            visit(
                classify(&stack, selectors),
                line_start + seg_start..line_start + offset,
            );
            if stack.apply(&op).is_err() {
                break 'lines;
            }
            seg_start = offset;
        }
        visit(
            classify(&stack, selectors),
            line_start + seg_start..line_start + line.len(),
        );
        line_start += line.len();
    }

    let mut out: Vec<Region> = Vec::with_capacity(raw.len());
    // Set when a raw region was dropped since the last kept one: whatever it
    // held (a blank comment line, a non-prose string) breaks continuity.
    let mut dropped = false;
    for raw in raw {
        let Some(bytes) = trim(text, &raw.bytes).filter(|b| is_prose(text, raw.kind, b)) else {
            dropped = true;
            continue;
        };
        let boundary = match out.last() {
            Some(prev)
                if prev.kind == RegionKind::Comment
                    && raw.kind == RegionKind::Comment
                    && raw.clean_gap
                    && !dropped
                    && text
                        .get(prev.bytes.end..bytes.start)
                        .is_some_and(|gap| gap.matches('\n').count() == 1) =>
            {
                Boundary::ContinuesComment
            }
            _ => Boundary::Separate,
        };
        dropped = false;
        out.push(Region {
            kind: raw.kind,
            bytes,
            boundary,
        });
    }
    out
}

/// Shrink a byte range to exclude surrounding whitespace; `None` if nothing
/// is left.
fn trim(text: &str, bytes: &Range<usize>) -> Option<Range<usize>> {
    let slice = text.get(bytes.clone())?;
    let leading = slice.len() - slice.trim_start().len();
    let trimmed = slice.trim();
    if trimmed.is_empty() {
        return None;
    }
    let start = bytes.start + leading;
    Some(start..start + trimmed.len())
}

/// Whether a region reads as prose. Comments always do. A string literal
/// with no whitespace in it is almost always a key, path, identifier, or
/// format token (`"frext_editor"`, `"utf-8"`, `"%Y-%m-%d"`) rather than a
/// sentence, so it is skipped to keep code buffers free of false positives.
fn is_prose(text: &str, kind: RegionKind, bytes: &Range<usize>) -> bool {
    match kind {
        RegionKind::Comment => true,
        RegionKind::String => text
            .get(bytes.clone())
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
            other => panic!("expected regions, got {other:?}"),
        }
    }

    fn file(name: &str) -> Option<&Path> {
        Some(Path::new(name))
    }

    /// Each region's text and boundary, for continuation assertions.
    fn boundaries<'a>(text: &'a str, scope: &CheckScope) -> Vec<(&'a str, Boundary)> {
        match scope {
            CheckScope::Regions(regions) => regions
                .iter()
                .map(|r| (&text[r.bytes.clone()], r.boundary))
                .collect(),
            other => panic!("expected regions, got {other:?}"),
        }
    }

    #[test]
    fn untitled_and_plain_text_buffers_are_plain_prose() {
        let plain = CheckScope::Document(ProseFormat::Plain);
        assert_eq!(check_scope("hello", None), plain);
        assert_eq!(check_scope("hello", file("notes.txt")), plain);
    }

    #[test]
    fn extensionless_files_without_a_syntax_are_prose() {
        let plain = CheckScope::Document(ProseFormat::Plain);
        assert_eq!(check_scope("hello", file("/repo/README")), plain);
        assert_eq!(
            check_scope("hello", file("/repo/.git/COMMIT_EDITMSG")),
            plain
        );
    }

    #[test]
    fn unknown_extensions_and_dotfiles_are_skipped() {
        assert_eq!(
            check_scope("key = value", file("a.definitelynotalang")),
            CheckScope::Skip
        );
        assert_eq!(
            check_scope("key = value", file("/home/me/.definitelynotarc")),
            CheckScope::Skip
        );
    }

    #[test]
    fn well_known_file_names_resolve_to_their_syntax() {
        for name in ["Makefile", "Dockerfile"] {
            assert!(
                matches!(
                    check_scope("# a comment\n", file(name)),
                    CheckScope::Regions(_)
                ),
                "{name} should be treated as code"
            );
        }
    }

    #[test]
    fn consecutive_line_comments_continue() {
        let text = "/// First line\n/// second line.\n///\n/// New paragraph.\n";
        let scope = check_scope(text, file("x.rs"));
        assert_eq!(
            boundaries(text, &scope),
            vec![
                ("First line", Boundary::Separate),
                ("second line.", Boundary::ContinuesComment),
                // The blank `///` line between them is a paragraph break.
                ("New paragraph.", Boundary::Separate),
            ]
        );
    }

    #[test]
    fn code_between_comments_breaks_continuation() {
        // The gap `\n} ` holds no letters or digits but is still code.
        let text = "fn f() {\n    let x = 1; // one\n} // two\n";
        let scope = check_scope(text, file("x.rs"));
        assert_eq!(
            boundaries(text, &scope),
            vec![("one", Boundary::Separate), ("two", Boundary::Separate)]
        );
    }

    #[test]
    fn markdown_is_checked_as_a_markdown_document() {
        assert_eq!(
            check_scope("# Title", file("x.md")),
            CheckScope::Document(ProseFormat::Markdown)
        );
    }

    #[test]
    fn rust_comments_are_extracted_without_their_markers() {
        let text = "// first comment\nfn main() {} /* block comment */\n";
        let scope = check_scope(text, file("x.rs"));
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
        assert_eq!(
            check_scope(text, file("x.rs")),
            CheckScope::Regions(Vec::new())
        );
    }

    #[test]
    fn sentence_strings_are_extracted_without_quotes() {
        let text = "let msg = \"failed to open the file\";\n";
        let scope = check_scope(text, file("x.rs"));
        assert_eq!(
            region_texts(text, &scope),
            vec![(RegionKind::String, "failed to open the file")]
        );
    }

    #[test]
    fn single_token_strings_are_skipped() {
        let text = "let id = \"frext_editor\"; let enc = \"utf-8\";\n";
        assert_eq!(
            check_scope(text, file("x.rs")),
            CheckScope::Regions(Vec::new())
        );
    }

    #[test]
    fn escapes_inside_strings_split_the_region() {
        let text = "let s = \"first line\\nsecond line\";\n";
        let scope = check_scope(text, file("x.rs"));
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
        let scope = check_scope(text, file("x.py"));
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
        let CheckScope::Regions(regions) = check_scope(text, file("x.rs")) else {
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
