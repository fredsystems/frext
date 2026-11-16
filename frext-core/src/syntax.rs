//! Syntax and theme resources, powered by [`syntect`] with the [`two_face`]
//! syntax and theme bundles, plus language detection from file extensions.
//!
//! A stock `syntect` integration can only use syntect's default bundled
//! syntaxes, which are missing many everyday languages (TOML, Nix, Zig,
//! Dockerfile, …). frext therefore owns its own [`SyntaxSet`] sourced from
//! [`two_face::syntax`], the curated `bat` syntax collection, which *does*
//! include those languages, and drives `syntect` directly. The matching
//! Catppuccin Mocha theme from [`two_face::theme`] keeps highlighting
//! consistent with frext's editor theme.
//!
//! The language is auto-detected from a tab's file extension. syntect matches
//! its Sublime-syntax definitions by extension, so the file suffix is handed
//! to [`SyntaxSet::find_syntax_by_extension`] directly — no per-extension
//! remapping is required, as the bundled syntaxes already declare their own
//! aliases (`yml`, `bash`, `pyw`, …).
//!
//! [`syntect`]: two_face::re_exports::syntect

use std::path::Path;
use std::sync::OnceLock;

use two_face::re_exports::syntect::{highlighting::Theme, parsing::SyntaxSet};
use two_face::theme::EmbeddedThemeName;

/// The owned highlighting resources: a syntax set with the extra `bat`
/// languages and the Catppuccin Mocha theme that matches frext's UI.
struct Highlighter {
    syntaxes: SyntaxSet,
    theme: Theme,
}

/// Lazily build (and cache) the highlighter. Loading the bundled dumps is not
/// free, so it is done once and reused for the lifetime of the process.
fn highlighter() -> &'static Highlighter {
    static HIGHLIGHTER: OnceLock<Highlighter> = OnceLock::new();
    HIGHLIGHTER.get_or_init(|| Highlighter {
        syntaxes: two_face::syntax::extra_newlines(),
        // `two_face::theme::extra()` returns a lazy set; clone the one theme
        // we use out of it so we own a plain `Theme` for `HighlightLines`.
        theme: two_face::theme::extra()
            .get(EmbeddedThemeName::CatppuccinMocha)
            .clone(),
    })
}

/// The shared syntax set, so other modules (the highlighter, the
/// proofreader's comment/string extraction) resolve languages identically.
pub(crate) fn syntax_set() -> &'static SyntaxSet {
    &highlighter().syntaxes
}

/// The Catppuccin Mocha syntect theme used for highlighting.
pub(crate) fn theme() -> &'static Theme {
    &highlighter().theme
}

/// Map a file path to the file-extension token used to select a syntax.
///
/// Returns an empty string when there is no usable extension; that selects the
/// plain-text fallback, which is exactly right for untitled or extension-less
/// buffers. The extension is forwarded to syntect verbatim (lowercased);
/// syntect's bundled syntaxes already declare their own aliases.
#[must_use]
pub fn language_from_path(path: Option<&Path>) -> String {
    path.and_then(Path::extension)
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_path_yields_plain_text() {
        assert_eq!(language_from_path(None), "");
    }

    #[test]
    fn extensionless_path_yields_plain_text() {
        assert_eq!(language_from_path(Some(Path::new("/etc/hostname"))), "");
    }

    #[test]
    fn extension_is_lowercased_token() {
        assert_eq!(language_from_path(Some(Path::new("main.rs"))), "rs");
        assert_eq!(language_from_path(Some(Path::new("a.toml"))), "toml");
        assert_eq!(language_from_path(Some(Path::new("a.nix"))), "nix");
    }

    #[test]
    fn extension_matching_is_case_insensitive() {
        // The token is the lowercased suffix; syntect resolves `yml` to YAML
        // by its own extension aliases, so no remap to `yaml` is needed here.
        assert_eq!(language_from_path(Some(Path::new("MAIN.RS"))), "rs");
        assert_eq!(language_from_path(Some(Path::new("Config.YML"))), "yml");
    }

    /// The whole point of owning a `two_face` syntax set: extensions that
    /// syntect's bundled defaults lack must now resolve to a real syntax.
    #[test]
    fn extra_languages_resolve_to_a_syntax() {
        let syntaxes = syntax_set();
        for ext in ["toml", "nix", "zig", "dockerfile"] {
            assert!(
                syntaxes.find_syntax_by_extension(ext).is_some(),
                "expected a syntax for .{ext}"
            );
        }
    }

    /// Common languages keep resolving, including suffix aliases that the old
    /// hand-maintained remap table used to cover.
    #[test]
    fn common_extensions_resolve_to_a_syntax() {
        let syntaxes = syntax_set();
        for ext in [
            "rs", "py", "pyw", "yml", "yaml", "bash", "md", "json", "html",
        ] {
            assert!(
                syntaxes.find_syntax_by_extension(ext).is_some(),
                "expected a syntax for .{ext}"
            );
        }
    }
}
