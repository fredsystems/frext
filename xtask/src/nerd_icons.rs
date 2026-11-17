//! Generate the Nerd Font icon table from an `nvim-web-devicons` checkout.
//!
//! Each frext icon stem's glyph is the majority vote of `nvim-web-devicons`'
//! extension and filename glyphs, mapped onto frext's stems through
//! [`frext_core::file_icon`]'s own lookups. Its colour is the dominant colour
//! of the stem's vendored Catppuccin Mocha SVG, preferring accents over
//! neutrals. Folder stems get the open / closed folder glyphs; unvoted stems
//! fall back to a devicons entry whose key or name matches the stem's name,
//! else a generic file glyph.

use std::{collections::BTreeMap, fmt::Write as _, sync::LazyLock};

use frext_core::{
    file_icon::{icon_bytes, icon_for_file, stems},
    palette::{Rgb, TEXT},
};
use regex::Regex;

/// Nerd Font closed folder glyph.
const FOLDER_CLOSED: char = '\u{e5ff}';
/// Nerd Font open folder glyph.
const FOLDER_OPEN: char = '\u{e5fe}';
/// Nerd Font generic file glyph.
const FILE: char = '\u{f15b}';
/// The default file stem; votes for it carry no information.
const DEFAULT_FILE_STEM: &str = "_file";

/// The Catppuccin Mocha neutrals: Text, Subtext 1/0, Overlay 2/1/0,
/// Surface 2/1/0, Base, Mantle and Crust.
///
/// Most icon artwork pairs a neutral outline with one accent; the accent is
/// the colour that identifies the icon, so neutrals only win when an SVG has
/// no accent at all.
const NEUTRALS: [Rgb; 12] = [
    Rgb::new(0xcd, 0xd6, 0xf4),
    Rgb::new(0xba, 0xc2, 0xde),
    Rgb::new(0xa6, 0xad, 0xc8),
    Rgb::new(0x93, 0x99, 0xb2),
    Rgb::new(0x7f, 0x84, 0x9c),
    Rgb::new(0x6c, 0x70, 0x86),
    Rgb::new(0x58, 0x5b, 0x70),
    Rgb::new(0x45, 0x47, 0x5a),
    Rgb::new(0x31, 0x32, 0x44),
    Rgb::new(0x1e, 0x1e, 0x2e),
    Rgb::new(0x18, 0x18, 0x25),
    Rgb::new(0x11, 0x11, 0x1b),
];

/// rustfmt's default `fn_call_width`: a tuple whose fields are wider than
/// this is laid out one field per line, so the table must be too.
const RUSTFMT_FN_CALL_WIDTH: usize = 60;

/// One `nvim-web-devicons` entry line: `["<key>"] = { icon = "<glyph>", ...`.
static ENTRY_LINE: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r#"^\s*\["([^"]+)"\]\s*=\s*\{\s*icon\s*=\s*"([^"]*)""#).ok());

/// The `name = "<name>"` field of a devicons entry line.
static NAME_FIELD: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r#"\bname\s*=\s*"([^"]*)""#).ok());

/// A `#`-prefixed hex run, ending at a word boundary.
static HEX_COLOUR: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"#([0-9a-fA-F]+)\b").ok());

/// One `nvim-web-devicons` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevIcon {
    /// The extension or file name the entry is keyed by.
    pub key: String,
    /// The entry's glyph, exactly one `char`.
    pub glyph: String,
    /// The entry's `name` field, empty when the line has none.
    pub name: String,
}

/// Every `["<key>"] = { icon = "<glyph>", ...` line of a devicons data file
/// as a [`DevIcon`].
///
/// Entries whose glyph is not exactly one `char` are skipped, as are lines of
/// any other shape (comments, `return {`, closing braces). An entry line
/// without a `name = "..."` field is kept with an empty name.
#[must_use]
pub fn parse_devicons(lua: &str) -> Vec<DevIcon> {
    let Some(pattern) = ENTRY_LINE.as_ref() else {
        return Vec::new();
    };
    lua.lines()
        .filter_map(|line| {
            let captures = pattern.captures(line)?;
            let key = captures.get(1)?.as_str();
            let glyph = captures.get(2)?.as_str();
            let mut chars = glyph.chars();
            if !matches!((chars.next(), chars.next()), (Some(_), None)) {
                return None;
            }
            let name = NAME_FIELD
                .as_ref()
                .and_then(|name| name.captures(line))
                .and_then(|captures| captures.get(1))
                .map_or("", |name| name.as_str());
            Some(DevIcon {
                key: key.to_owned(),
                glyph: glyph.to_owned(),
                name: name.to_owned(),
            })
        })
        .collect()
}

/// Each stem's glyph, by majority vote of devicons' entries.
///
/// Each extension key `k` votes its glyph for `icon_for_file("x.k")`, each
/// filename key for `icon_for_file(k)`. Votes for the default `_file` stem
/// are dropped. Ties are broken by the smallest glyph.
#[must_use]
pub fn votes(extensions: &[DevIcon], filenames: &[DevIcon]) -> BTreeMap<&'static str, String> {
    let mut tally: BTreeMap<&'static str, BTreeMap<&str, usize>> = BTreeMap::new();
    let extension_votes = extensions.iter().map(|icon| {
        (
            icon_for_file(&format!("x.{}", icon.key)),
            icon.glyph.as_str(),
        )
    });
    let filename_votes = filenames
        .iter()
        .map(|icon| (icon_for_file(&icon.key), icon.glyph.as_str()));
    for (stem, glyph) in extension_votes.chain(filename_votes) {
        if stem == DEFAULT_FILE_STEM {
            continue;
        }
        *tally.entry(stem).or_default().entry(glyph).or_default() += 1;
    }

    tally
        .into_iter()
        .filter_map(|(stem, counts)| {
            let mut best: Option<(&str, usize)> = None;
            // Ascending glyph order: only a strictly larger count replaces the
            // best, so ties keep the smallest glyph.
            for (glyph, count) in counts {
                if best.is_none_or(|(_, best_count)| count > best_count) {
                    best = Some((glyph, count));
                }
            }
            best.map(|(glyph, _)| (stem, glyph.to_owned()))
        })
        .collect()
}

/// The most frequent accent `#rrggbb` / `#rgb` colour in `svg`.
///
/// Accents are colours not in [`NEUTRALS`]; an SVG with no accent at all
/// takes its most frequent neutral instead. Matching is case-insensitive and
/// `#rgb` is expanded to `#rrggbb`. Ties are broken by first occurrence.
/// Returns [`TEXT`] when there is no colour.
#[must_use]
pub fn dominant_colour(svg: &str) -> Rgb {
    let Some(pattern) = HEX_COLOUR.as_ref() else {
        return TEXT;
    };
    // In first-occurrence order.
    let mut counts: Vec<(Rgb, usize)> = Vec::new();
    for captures in pattern.captures_iter(svg) {
        let Some(colour) = captures.get(1).and_then(|hex| parse_hex(hex.as_str())) else {
            continue;
        };
        match counts.iter_mut().find(|(seen, _)| *seen == colour) {
            Some((_, count)) => *count += 1,
            None => counts.push((colour, 1)),
        }
    }

    let (neutrals, accents): (Vec<_>, Vec<_>) = counts
        .into_iter()
        .partition(|(colour, _)| NEUTRALS.contains(colour));
    most_frequent(&accents)
        .or_else(|| most_frequent(&neutrals))
        .unwrap_or(TEXT)
}

/// The colour with the highest count, ties broken by first position.
fn most_frequent(counts: &[(Rgb, usize)]) -> Option<Rgb> {
    let mut best: Option<(Rgb, usize)> = None;
    for &(colour, count) in counts {
        if best.is_none_or(|(_, best_count)| count > best_count) {
            best = Some((colour, count));
        }
    }
    best.map(|(colour, _)| colour)
}

/// The glyph of the devicons entry whose key or name matches `stem`'s name.
///
/// Candidates, in order: the stem, the stem with every `-` and `_` removed,
/// and the stem up to its first `-` or `_` (only if it has one), each
/// lowercased. Each candidate is looked up among the entries' keys
/// (`extensions` then `filenames`, lowercased), then among their `name`
/// fields (likewise); the first matching entry in file order wins. Empty
/// candidates are skipped.
#[must_use]
pub fn name_glyph(stem: &str, extensions: &[DevIcon], filenames: &[DevIcon]) -> Option<char> {
    let lower = stem.to_lowercase();
    let mut candidates = vec![lower.clone(), lower.replace(['-', '_'], "")];
    if let Some((prefix, _)) = lower.split_once(['-', '_']) {
        candidates.push(prefix.to_owned());
    }
    let all = || extensions.iter().chain(filenames);
    candidates
        .iter()
        .filter(|candidate| !candidate.is_empty())
        .find_map(|candidate| {
            all()
                .find(|icon| icon.key.to_lowercase() == *candidate)
                .or_else(|| all().find(|icon| icon.name.to_lowercase() == *candidate))
        })
        .and_then(|icon| icon.glyph.chars().next())
}

/// Parse a 3- or 6-digit hex colour (without the `#`).
fn parse_hex(hex: &str) -> Option<Rgb> {
    let digits = hex
        .chars()
        .map(|c| c.to_digit(16).and_then(|d| u8::try_from(d).ok()))
        .collect::<Option<Vec<u8>>>()?;
    match *digits.as_slice() {
        [r, g, b] => Some(Rgb::new(r * 0x11, g * 0x11, b * 0x11)),
        [r1, r2, g1, g2, b1, b2] => Some(Rgb::new(r1 * 16 + r2, g1 * 16 + g2, b1 * 16 + b2)),
        _ => None,
    }
}

/// Render the generated table file for devicons commit `commit`.
///
/// `entries` are written in the given order (callers pass them in stem
/// order), each glyph as a lowercase `\u{..}` escape and each colour as
/// `Rgb::new(0x.., 0x.., 0x..)`.
#[must_use]
pub fn render_table(commit: &str, entries: &[(&str, char, Rgb)]) -> String {
    let mut out = String::new();
    out.push_str("//! Generated Nerd Font icon table: icon stem to glyph and colour.\n");
    out.push_str("//!\n");
    // `write!` into a `String` cannot fail.
    let _ = writeln!(
        out,
        "//! Glyphs from `nvim-tree/nvim-web-devicons` commit `{commit}`,"
    );
    out.push_str("//! mapped onto frext's icon stems; colours from each stem's Catppuccin\n");
    out.push_str("//! Mocha SVG. Generated by `cargo xtask nerd-icons <checkout>`; do\n");
    out.push_str("//! not edit by hand.\n");
    out.push('\n');
    out.push_str("use frext_core::palette::Rgb;\n");
    out.push('\n');
    out.push_str("/// Every icon stem as `(stem, glyph, colour)`, sorted by stem.\n");
    out.push_str("pub const NERD_ICONS: &[(&str, &str, Rgb)] = &[\n");
    for &(stem, glyph, colour) in entries {
        let fields = [
            format!("\"{stem}\""),
            format!("\"\\u{{{:x}}}\"", u32::from(glyph)),
            format!(
                "Rgb::new(0x{:02x}, 0x{:02x}, 0x{:02x})",
                colour.r, colour.g, colour.b
            ),
        ];
        let one_line = fields.join(", ");
        if one_line.chars().count() <= RUSTFMT_FN_CALL_WIDTH {
            let _ = writeln!(out, "    ({one_line}),");
        } else {
            out.push_str("    (\n");
            for field in &fields {
                let _ = writeln!(out, "        {field},");
            }
            out.push_str("    ),\n");
        }
    }
    out.push_str("];\n");
    out
}

/// One `(stem, glyph, colour)` entry per bundled stem, in stem order.
///
/// Folder stems get the closed / open folder glyphs; other stems their vote
/// from `votes`, else their [`name_glyph`] in `extensions` / `filenames`,
/// else the generic file glyph. The default `_file` stem always gets the
/// generic file glyph. The colour is the dominant colour of the stem's SVG.
#[must_use]
pub fn entries(
    votes: &BTreeMap<&'static str, String>,
    extensions: &[DevIcon],
    filenames: &[DevIcon],
) -> Vec<(&'static str, char, Rgb)> {
    stems()
        .map(|stem| {
            let glyph = folder_glyph(stem)
                .or_else(|| votes.get(stem).and_then(|glyph| glyph.chars().next()))
                .or_else(|| {
                    (stem != DEFAULT_FILE_STEM)
                        .then(|| name_glyph(stem, extensions, filenames))
                        .flatten()
                })
                .unwrap_or(FILE);
            let colour = icon_bytes(stem).map_or(TEXT, |bytes| {
                dominant_colour(&String::from_utf8_lossy(bytes))
            });
            (stem, glyph, colour)
        })
        .collect()
}

/// The folder glyph for a folder stem, `None` for any other stem.
fn folder_glyph(stem: &str) -> Option<char> {
    let is_open = stem.ends_with("_open");
    if stem == "_folder" || stem == "_folder_open" || stem.starts_with("folder_") {
        Some(if is_open { FOLDER_OPEN } else { FOLDER_CLOSED })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    // `unwrap` is acceptable in test code: a panic on an unexpected `None`
    // is exactly the failure signal we want from a test.
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn pairs(items: &[(&str, &str)]) -> Vec<DevIcon> {
        items
            .iter()
            .map(|&(key, glyph)| DevIcon {
                key: key.to_owned(),
                glyph: glyph.to_owned(),
                name: String::new(),
            })
            .collect()
    }

    fn named(items: &[(&str, &str, &str)]) -> Vec<DevIcon> {
        items
            .iter()
            .map(|&(key, glyph, name)| DevIcon {
                key: key.to_owned(),
                glyph: glyph.to_owned(),
                name: name.to_owned(),
            })
            .collect()
    }

    #[test]
    fn parses_entry_lines_and_skips_other_lines() {
        let lua = concat!(
            "return {\n",
            "  -- a comment\n",
            "  [\"rs\"]             = { icon = \"\u{e7a8}\", color = \"#DEA584\", ",
            "cterm_color = \"216\", name = \"Rs\"                         },\n",
            "  [\"toml\"]           = { icon = \"\u{e6b2}\", color = \"#9C4221\", ",
            "cterm_color = \"124\", name = \"Toml\"                       },\n",
            "}\n",
        );
        assert_eq!(
            parse_devicons(lua),
            named(&[("rs", "\u{e7a8}", "Rs"), ("toml", "\u{e6b2}", "Toml")])
        );
    }

    #[test]
    fn skips_multi_char_and_empty_glyphs() {
        let lua = concat!(
            "  [\"a\"] = { icon = \"ab\", color = \"#000000\" },\n",
            "  [\"b\"] = { icon = \"\", color = \"#000000\" },\n",
            "  [\"c\"] = { icon = \"\u{f15b}\", color = \"#000000\" },\n",
        );
        assert_eq!(parse_devicons(lua), pairs(&[("c", "\u{f15b}")]));
    }

    #[test]
    fn extension_rs_votes_for_the_rust_stem() {
        let result = votes(&pairs(&[("rs", "R")]), &[]);
        assert_eq!(icon_for_file("x.rs"), "rust");
        assert_eq!(result.get("rust").map(String::as_str), Some("R"));
    }

    #[test]
    fn majority_wins() {
        let result = votes(
            &pairs(&[("rs", "B"), ("rs", "A")]),
            &pairs(&[("main.rs", "B")]),
        );
        assert_eq!(result.get("rust").map(String::as_str), Some("B"));
    }

    #[test]
    fn tie_keeps_the_smallest_glyph() {
        let result = votes(&pairs(&[("rs", "Z"), ("rs", "A")]), &[]);
        assert_eq!(result.get("rust").map(String::as_str), Some("A"));
    }

    #[test]
    fn default_file_votes_are_dropped() {
        assert_eq!(icon_for_file("x.unknownextension"), DEFAULT_FILE_STEM);
        let result = votes(
            &pairs(&[("unknownextension", "U")]),
            &pairs(&[("unknownfilename", "U")]),
        );
        assert!(!result.contains_key(DEFAULT_FILE_STEM));
        assert!(result.is_empty());
    }

    #[test]
    fn dominant_colour_expands_short_form() {
        assert_eq!(
            dominant_colour(r##"<path fill="#abc"/>"##),
            Rgb::new(0xaa, 0xbb, 0xcc)
        );
    }

    #[test]
    fn dominant_colour_is_case_insensitive() {
        let svg = r##"<a fill="#A6E3A1"/><b fill="#89b4fa"/><c fill="#a6e3a1"/>"##;
        assert_eq!(dominant_colour(svg), Rgb::new(0xa6, 0xe3, 0xa1));
    }

    #[test]
    fn dominant_colour_tie_keeps_first_occurrence() {
        let svg = r##"<a fill="#89b4fa"/><b fill="#a6e3a1"/>"##;
        assert_eq!(dominant_colour(svg), Rgb::new(0x89, 0xb4, 0xfa));
    }

    #[test]
    fn dominant_colour_ignores_non_colour_hex_runs() {
        let svg = r##"<a fill="url(#a)"/><b fill="#abcdef12"/><c fill="#f00"/>"##;
        assert_eq!(dominant_colour(svg), Rgb::new(0xff, 0x00, 0x00));
    }

    #[test]
    fn dominant_colour_without_colours_is_text() {
        assert_eq!(dominant_colour("<svg><path d=\"M0 0\"/></svg>"), TEXT);
    }

    #[test]
    fn dominant_colour_prefers_an_accent_over_a_first_neutral() {
        let svg = r##"<a stroke="#cdd6f4"/><b fill="#fab387"/>"##;
        assert_eq!(dominant_colour(svg), Rgb::new(0xfa, 0xb3, 0x87));
    }

    #[test]
    fn dominant_colour_prefers_an_accent_over_a_more_frequent_neutral() {
        let svg = r##"<a stroke="#6c7086"/><b fill="#6c7086"/><c fill="#a6e3a1"/>"##;
        assert_eq!(dominant_colour(svg), Rgb::new(0xa6, 0xe3, 0xa1));
    }

    #[test]
    fn dominant_colour_of_neutrals_only_is_the_most_frequent_neutral() {
        let svg = r##"<a fill="#cdd6f4"/><b fill="#7f849c"/><c fill="#7f849c"/>"##;
        assert_eq!(dominant_colour(svg), Rgb::new(0x7f, 0x84, 0x9c));
    }

    #[test]
    fn dominant_colour_neutral_tie_keeps_first_occurrence() {
        let svg = r##"<a fill="#7f849c"/><b fill="#cdd6f4"/>"##;
        assert_eq!(dominant_colour(svg), Rgb::new(0x7f, 0x84, 0x9c));
    }

    #[test]
    fn neutrals_are_the_twelve_mocha_neutrals() {
        assert!(NEUTRALS.contains(&TEXT));
        for (i, colour) in NEUTRALS.iter().enumerate() {
            assert!(!NEUTRALS[..i].contains(colour), "{colour:?}");
        }
    }

    #[test]
    fn name_glyph_hits_an_exact_key() {
        let extensions = pairs(&[("toml", "T"), ("astro", "A")]);
        assert_eq!(name_glyph("astro", &extensions, &[]), Some('A'));
    }

    #[test]
    fn name_glyph_matches_keys_case_insensitively() {
        let filenames = pairs(&[("Dockerfile", "D")]);
        assert_eq!(name_glyph("dockerfile", &[], &filenames), Some('D'));
    }

    #[test]
    fn name_glyph_hits_a_name_with_separators_removed() {
        let filenames = named(&[("go.mod", "G", "GoMod")]);
        assert_eq!(name_glyph("go-mod", &[], &filenames), Some('G'));
    }

    #[test]
    fn name_glyph_hits_the_prefix() {
        let extensions = pairs(&[("astro", "A")]);
        assert_eq!(name_glyph("astro-config", &extensions, &[]), Some('A'));
    }

    #[test]
    fn name_glyph_prefers_a_key_over_a_name() {
        let extensions = named(&[("other", "N", "Lua")]);
        let filenames = pairs(&[("lua", "K")]);
        assert_eq!(name_glyph("lua", &extensions, &filenames), Some('K'));
    }

    #[test]
    fn name_glyph_prefers_extensions_and_file_order() {
        let extensions = pairs(&[("lua", "1"), ("lua", "2")]);
        let filenames = pairs(&[("lua", "3")]);
        assert_eq!(name_glyph("lua", &extensions, &filenames), Some('1'));
    }

    #[test]
    fn name_glyph_prefers_an_earlier_candidate() {
        // The full stem matches a name; the prefix matches a key. The full
        // stem is the earlier candidate, so its name hit wins.
        let extensions = named(&[("astro", "A", ""), ("x", "C", "astro-config")]);
        assert_eq!(name_glyph("astro-config", &extensions, &[]), Some('C'));
    }

    #[test]
    fn name_glyph_without_a_hit_is_none() {
        let extensions = named(&[("rs", "R", "Rs")]);
        assert_eq!(name_glyph("zzz-unknown", &extensions, &[]), None);
        // A leading separator yields an empty prefix, which matches nothing,
        // not even entries with an empty name.
        assert_eq!(name_glyph("_zzz", &pairs(&[("q", "Q")]), &[]), None);
    }

    #[test]
    fn votes_beat_the_name_fallback() {
        let votes = BTreeMap::from([("rust", "V".to_owned())]);
        let extensions = pairs(&[("rust", "N")]);
        let all = entries(&votes, &extensions, &[]);
        let rust = all.iter().find(|entry| entry.0 == "rust").unwrap();
        assert_eq!(rust.1, 'V');
    }

    #[test]
    fn unvoted_stems_use_the_name_fallback() {
        let extensions = pairs(&[("astro", "A")]);
        let all = entries(&BTreeMap::new(), &extensions, &[]);
        let glyph = |stem: &str| all.iter().find(|entry| entry.0 == stem).unwrap().1;
        assert_eq!(glyph("astro-config"), 'A');
        assert_eq!(glyph("_file"), FILE);
    }

    #[test]
    fn folder_stems_get_folder_glyphs() {
        let votes = BTreeMap::from([("_folder", "X".to_owned()), ("rust", "R".to_owned())]);
        let all = entries(&votes, &[], &[]);
        let glyph = |stem: &str| all.iter().find(|entry| entry.0 == stem).unwrap().1;
        assert_eq!(glyph("_folder"), FOLDER_CLOSED);
        assert_eq!(glyph("_folder_open"), FOLDER_OPEN);
        for &(stem, glyph, _) in all.iter().filter(|entry| entry.0.starts_with("folder_")) {
            let expected = if stem.ends_with("_open") {
                FOLDER_OPEN
            } else {
                FOLDER_CLOSED
            };
            assert_eq!(glyph, expected, "{stem}");
        }
        assert_eq!(glyph("rust"), 'R');
        assert_eq!(glyph("_file"), FILE);
    }

    #[test]
    fn entries_cover_every_stem_in_order() {
        let all = entries(&BTreeMap::new(), &[], &[]);
        let expected: Vec<&str> = stems().collect();
        let actual: Vec<&str> = all.iter().map(|entry| entry.0).collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn renders_the_table_exactly() {
        let table = render_table(
            "0123abcd",
            &[
                ("3d", '\u{f01a7}', Rgb::new(0xa6, 0xe3, 0xa1)),
                ("rust", '\u{e7a8}', Rgb::new(0xfa, 0xb3, 0x87)),
            ],
        );
        let expected = concat!(
            "//! Generated Nerd Font icon table: icon stem to glyph and colour.\n",
            "//!\n",
            "//! Glyphs from `nvim-tree/nvim-web-devicons` commit `0123abcd`,\n",
            "//! mapped onto frext's icon stems; colours from each stem's Catppuccin\n",
            "//! Mocha SVG. Generated by `cargo xtask nerd-icons <checkout>`; do\n",
            "//! not edit by hand.\n",
            "\n",
            "use frext_core::palette::Rgb;\n",
            "\n",
            "/// Every icon stem as `(stem, glyph, colour)`, sorted by stem.\n",
            "pub const NERD_ICONS: &[(&str, &str, Rgb)] = &[\n",
            "    (\"3d\", \"\\u{f01a7}\", Rgb::new(0xa6, 0xe3, 0xa1)),\n",
            "    (\"rust\", \"\\u{e7a8}\", Rgb::new(0xfa, 0xb3, 0x87)),\n",
            "];\n",
        );
        assert_eq!(table, expected);
    }

    #[test]
    fn wide_entries_use_the_rustfmt_vertical_layout() {
        let colour = Rgb::new(0xcd, 0xd6, 0xf4);
        // Fields exactly 60 chars wide stay on one line; 61 wrap.
        let table = render_table(
            "c",
            &[
                ("folder_controllers", '\u{e5ff}', colour),
                ("folder_android_open", '\u{e5fe}', colour),
            ],
        );
        let body: Vec<&str> = table
            .lines()
            .skip_while(|line| !line.starts_with("pub const"))
            .collect();
        assert_eq!(
            body,
            [
                "pub const NERD_ICONS: &[(&str, &str, Rgb)] = &[",
                "    (\"folder_controllers\", \"\\u{e5ff}\", Rgb::new(0xcd, 0xd6, 0xf4)),",
                "    (",
                "        \"folder_android_open\",",
                "        \"\\u{e5fe}\",",
                "        Rgb::new(0xcd, 0xd6, 0xf4),",
                "    ),",
                "];",
            ]
        );
    }

    #[test]
    fn every_rendered_line_fits_in_100_chars() {
        let commit = "0123456789abcdef0123456789abcdef01234567";
        // The widest possible glyph escape, so no real table can be wider.
        let widest: Vec<_> = entries(&BTreeMap::new(), &[], &[])
            .into_iter()
            .map(|(stem, _, colour)| (stem, '\u{10ffff}', colour))
            .collect();
        let table = render_table(commit, &widest);
        for line in table.lines() {
            assert!(line.chars().count() <= 100, "{line}");
        }
    }
}
