//! How one grapheme is drawn and how many terminal cells it takes.
//!
//! A `'\t'` is [`TAB_WIDTH`] blank cells, a grapheme starting with any
//! other control char draws as [`REPLACEMENT`] in one cell, and anything
//! else is drawn as itself, at least one cell wide.

use unicode_width::UnicodeWidthStr;

/// The number of blank cells a `'\t'` takes.
pub const TAB_WIDTH: u16 = 4;

/// What a control-char grapheme draws as.
pub const REPLACEMENT: &str = "\u{FFFD}";

/// How one grapheme is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Glyph<'a> {
    /// The grapheme itself.
    Text(&'a str),
    /// This many blank cells.
    Blank(u16),
    /// [`REPLACEMENT`] in one cell.
    Replacement,
}

/// How `grapheme` is drawn.
#[must_use]
pub fn glyph(grapheme: &str) -> Glyph<'_> {
    if grapheme == "\t" {
        Glyph::Blank(TAB_WIDTH)
    } else if grapheme.chars().next().is_some_and(char::is_control) {
        Glyph::Replacement
    } else {
        Glyph::Text(grapheme)
    }
}

/// How many cells `grapheme` takes; always at least 1.
#[must_use]
pub fn width(grapheme: &str) -> u16 {
    match glyph(grapheme) {
        Glyph::Blank(n) => n,
        Glyph::Replacement => 1,
        Glyph::Text(text) => u16::try_from(text.width()).unwrap_or(u16::MAX).max(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_is_four_blank_cells() {
        assert_eq!(glyph("\t"), Glyph::Blank(TAB_WIDTH));
        assert_eq!(width("\t"), 4);
    }

    #[test]
    fn control_char_is_one_replacement_cell() {
        assert_eq!(glyph("\u{7}"), Glyph::Replacement);
        assert_eq!(glyph("\r"), Glyph::Replacement);
        assert_eq!(width("\u{7}"), 1);
    }

    #[test]
    fn cjk_is_two_cells() {
        assert_eq!(glyph("漢"), Glyph::Text("漢"));
        assert_eq!(width("漢"), 2);
    }

    #[test]
    fn zwj_emoji_is_two_cells() {
        let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
        assert_eq!(glyph(family), Glyph::Text(family));
        assert_eq!(width(family), 2);
    }

    #[test]
    fn lone_combining_mark_is_at_least_one_cell() {
        assert!(width("\u{301}") >= 1);
    }

    #[test]
    fn ascii_is_one_cell() {
        assert_eq!(glyph("a"), Glyph::Text("a"));
        assert_eq!(width("a"), 1);
        assert_eq!(width(" "), 1);
    }
}
