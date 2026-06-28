//! Syntax highlighting, powered by `egui_extras`' `syntect` backend.
//!
//! The language is auto-detected from a tab's file extension. syntect matches
//! its bundled Sublime-syntax definitions by extension token (e.g. `rs`,
//! `py`, `toml`), so for most files the extension *is* the language token.
//! A few common extensions whose token differs from the file suffix are
//! remapped explicitly.

use std::path::Path;

use eframe::egui;
use egui_extras::syntax_highlighting::{CodeTheme, highlight};

/// Map a file path to a syntect language token, using its extension.
///
/// Returns an empty string when there is no usable extension; syntect treats
/// that as "plain text", which is exactly the desired fallback for untitled
/// or extension-less buffers.
#[must_use]
pub fn language_from_path(path: Option<&Path>) -> String {
    let Some(ext) = path.and_then(Path::extension).and_then(|ext| ext.to_str()) else {
        return String::new();
    };

    // syntect resolves most languages directly from the lowercase extension.
    // Only extensions whose syntect token differs from the suffix need a
    // remap here; everything else is handed to syntect verbatim, which
    // matches it against a bundled syntax or falls back to plain text.
    let lower = ext.to_ascii_lowercase();
    let token = match lower.as_str() {
        "pyw" => "py",
        "mjs" | "cjs" => "js",
        "markdown" => "md",
        "yml" => "yaml",
        "bash" | "zsh" => "sh",
        "h" => "c",
        "cc" | "cxx" | "hpp" | "hh" => "cpp",
        "htm" => "html",
        other => other,
    };
    token.to_owned()
}

/// Build a `layouter` closure suitable for `egui::TextEdit::layouter`.
///
/// The closure highlights the buffer for `language` using the egui-managed
/// `CodeTheme` (read from / stored in egui memory). Highlighting is memoized
/// per frame by `egui_extras`, so this is cheap to call every frame.
pub fn layouter<'a>(
    ctx: &'a egui::Context,
    language: &'a str,
) -> impl FnMut(&egui::Ui, &dyn egui::TextBuffer, f32) -> std::sync::Arc<egui::Galley> + 'a {
    move |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
        let theme = CodeTheme::from_memory(ui.ctx(), ui.style());
        let mut job = highlight(ctx, ui.style(), &theme, buf.as_str(), language);
        job.wrap.max_width = wrap_width;
        ui.fonts_mut(|f| f.layout_job(job))
    }
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
    fn known_extension_maps_to_itself() {
        assert_eq!(language_from_path(Some(Path::new("main.rs"))), "rs");
        assert_eq!(language_from_path(Some(Path::new("a.toml"))), "toml");
        assert_eq!(language_from_path(Some(Path::new("a.nix"))), "nix");
    }

    #[test]
    fn aliased_extensions_remap_to_canonical_token() {
        assert_eq!(language_from_path(Some(Path::new("a.yml"))), "yaml");
        assert_eq!(language_from_path(Some(Path::new("a.markdown"))), "md");
        assert_eq!(language_from_path(Some(Path::new("a.htm"))), "html");
        assert_eq!(language_from_path(Some(Path::new("a.hpp"))), "cpp");
        assert_eq!(language_from_path(Some(Path::new("run.bash"))), "sh");
    }

    #[test]
    fn extension_matching_is_case_insensitive() {
        assert_eq!(language_from_path(Some(Path::new("MAIN.RS"))), "rs");
        assert_eq!(language_from_path(Some(Path::new("Config.YML"))), "yaml");
    }

    #[test]
    fn unknown_extension_is_passed_through_verbatim() {
        // syntect will resolve or fall back to plain text; we just forward it.
        assert_eq!(language_from_path(Some(Path::new("a.zig"))), "zig");
    }
}
