//! Syntax highlighting, powered by [`syntect`] with the [`two_face`] syntax
//! and theme bundles.
//!
//! `egui_extras` ships a `syntect` backend, but it can only ever use
//! syntect's default bundled syntaxes, which are missing many everyday
//! languages (TOML, Nix, Zig, Dockerfile, …). frext therefore owns its own
//! [`SyntaxSet`] sourced from [`two_face::syntax`], the curated `bat` syntax
//! collection, which *does* include those languages, and drives `syntect`
//! directly. The matching Catppuccin Mocha theme from [`two_face::theme`]
//! keeps highlighting consistent with frext's editor theme.
//!
//! The language is auto-detected from a tab's file extension. syntect matches
//! its Sublime-syntax definitions by extension, so the file suffix is handed
//! to [`SyntaxSet::find_syntax_by_extension`] directly — no per-extension
//! remapping is required, as the bundled syntaxes already declare their own
//! aliases (`yml`, `bash`, `pyw`, …).

use std::path::Path;
use std::sync::OnceLock;

use eframe::egui::{
    self, Color32, FontId, Stroke, TextStyle,
    text::{ByteIndex, LayoutJob, LayoutSection, TextFormat},
};
use two_face::re_exports::syntect::{
    easy::HighlightLines,
    highlighting::{FontStyle, Theme},
    parsing::SyntaxSet,
    util::LinesWithEndings,
};
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

/// Build a `layouter` closure suitable for [`egui::TextEdit::layouter`].
///
/// The closure highlights the buffer for `language` (a file-extension token,
/// see [`language_from_path`]) and lays the result out into a [`Galley`]. When
/// the extension does not resolve to a known syntax the text is laid out
/// unhighlighted, so the editor always renders.
///
/// [`Galley`]: egui::Galley
pub fn layouter<'a>(
    language: &'a str,
) -> impl FnMut(&egui::Ui, &dyn egui::TextBuffer, f32) -> std::sync::Arc<egui::Galley> + 'a {
    move |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
        let font_id = ui
            .style()
            .override_font_id
            .clone()
            .unwrap_or_else(|| TextStyle::Monospace.resolve(ui.style()));
        let mut job = highlight(buf.as_str(), language, font_id);
        job.wrap.max_width = wrap_width;
        ui.fonts_mut(|f| f.layout_job(job))
    }
}

/// Highlight `text` for the syntax identified by `language`, producing a
/// [`LayoutJob`]. Falls back to a single unstyled section when no syntax
/// matches or highlighting fails part-way through.
fn highlight(text: &str, language: &str, font_id: FontId) -> LayoutJob {
    highlight_impl(text, language, &font_id).unwrap_or_else(|| {
        LayoutJob::simple(text.into(), font_id, Color32::LIGHT_GRAY, f32::INFINITY)
    })
}

/// The fallible highlighting core: `None` signals "no syntax / parse error",
/// at which point the caller renders plain text.
fn highlight_impl(text: &str, language: &str, font_id: &FontId) -> Option<LayoutJob> {
    let h = highlighter();
    let syntax = h.syntaxes.find_syntax_by_extension(language)?;

    let mut lines = HighlightLines::new(syntax, &h.theme);
    let mut job = LayoutJob {
        text: text.into(),
        ..Default::default()
    };

    for line in LinesWithEndings::from(text) {
        for (style, range) in lines.highlight_line(line, &h.syntaxes).ok()? {
            let fg = style.foreground;
            let color = Color32::from_rgb(fg.r, fg.g, fg.b);
            let italics = style.font_style.contains(FontStyle::ITALIC);
            let underline = if style.font_style.contains(FontStyle::UNDERLINE) {
                Stroke::new(1.0, color)
            } else {
                Stroke::NONE
            };
            job.sections.push(LayoutSection {
                leading_space: 0.0,
                byte_range: as_byte_range(text, range),
                format: TextFormat {
                    font_id: font_id.clone(),
                    color,
                    italics,
                    underline,
                    ..Default::default()
                },
            });
        }
    }

    Some(job)
}

/// Translate a syntect string slice back into a [`ByteIndex`] range within
/// `whole`.
///
/// syntect yields sub-`&str`s that point into the line being highlighted,
/// which itself points into `whole`; pointer arithmetic recovers the offset.
/// The casts here are pointer-to-address comparisons, not lossy numeric
/// conversions, and the offset is provably in-bounds because `range` is a
/// slice of `whole`.
fn as_byte_range(whole: &str, range: &str) -> std::ops::Range<ByteIndex> {
    let whole_start = whole.as_ptr() as usize;
    let range_start = range.as_ptr() as usize;
    let offset = range_start - whole_start;
    ByteIndex(offset)..ByteIndex(offset + range.len())
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
        let h = highlighter();
        for ext in ["toml", "nix", "zig", "dockerfile"] {
            assert!(
                h.syntaxes.find_syntax_by_extension(ext).is_some(),
                "expected a syntax for .{ext}"
            );
        }
    }

    /// Common languages keep resolving, including suffix aliases that the old
    /// hand-maintained remap table used to cover.
    #[test]
    fn common_extensions_resolve_to_a_syntax() {
        let h = highlighter();
        for ext in [
            "rs", "py", "pyw", "yml", "yaml", "bash", "md", "json", "html",
        ] {
            assert!(
                h.syntaxes.find_syntax_by_extension(ext).is_some(),
                "expected a syntax for .{ext}"
            );
        }
    }

    /// A TOML buffer must produce highlighted sections rather than a single
    /// plain fallback span — this is the regression the rewrite fixes.
    #[test]
    fn toml_is_highlighted_into_multiple_sections() {
        let job = highlight(
            "[package]\nname = \"frext\"\n",
            "toml",
            FontId::monospace(12.0),
        );
        assert!(
            job.sections.len() > 1,
            "TOML should highlight into multiple styled sections, got {}",
            job.sections.len()
        );
    }

    #[test]
    fn unknown_extension_falls_back_to_plain_text() {
        let job = highlight(
            "just some text",
            "definitelynotalang",
            FontId::monospace(12.0),
        );
        // The plain fallback is a single section covering the whole buffer.
        assert_eq!(job.sections.len(), 1);
    }
}
