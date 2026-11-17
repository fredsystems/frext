//! kitty graphics protocol encoding.
//!
//! frext draws file icons as kitty images shown through unicode
//! placeholders: an image is uploaded once with a virtual placement
//! (`U=1`), and each cell that shows it is the placeholder character
//! [`PLACEHOLDER_CELLS`] with the image id in the foreground colour.
//!
//! Uploads use direct transfer (`t=d`), because some terminals refuse
//! file and shared-memory transfers by default. Every command carries
//! `q=2`, so the terminal sends no replies that could land in the input
//! stream as garbage key events. Image ids are 24-bit ([`ImageId`]), so
//! a whole id fits in the cell's foreground colour and the id-high-byte
//! diacritic is never needed.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ratatui::style::Color;

use super::caps::Multiplexer;
use super::tmux;

/// Opens a kitty graphics command (APC `G`).
const START: &[u8] = b"\x1b_G";

/// Closes a kitty graphics command (ST).
const END: &[u8] = b"\x1b\\";

/// The largest image id: ids must fit in a 24-bit colour.
const MAX_ID: u32 = 0xFF_FFFF;

/// A kitty image id in `1..=0xFF_FFFF`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ImageId(u32);

impl ImageId {
    /// Returns the id `id`, or `None` when it is outside
    /// `1..=0xFF_FFFF`.
    #[must_use]
    pub const fn new(id: u32) -> Option<Self> {
        if id >= 1 && id <= MAX_ID {
            Some(Self(id))
        } else {
            None
        }
    }

    /// The numeric id.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }

    /// The id after this one, or `None` when this is the largest.
    #[must_use]
    pub const fn next(self) -> Option<Self> {
        Self::new(self.0.saturating_add(1))
    }

    /// The foreground colour that tells a placeholder cell which image
    /// it shows: the id's three bytes as red, green and blue.
    #[must_use]
    pub const fn color(self) -> Color {
        let [_, r, g, b] = self.0.to_be_bytes();
        Color::Rgb(r, g, b)
    }
}

/// The two cells of a 2x1 icon, left then right.
///
/// Each is `U+10EEEE` followed by the row diacritic (row 0, `U+0305`)
/// and the column diacritic (columns 0 and 1 of kitty's
/// rowcolumn-diacritics table: `U+0305`, `U+030D`). Both diacritics are
/// written explicitly on every cell rather than inherited from the cell
/// to the left: ratatui's diff renderer may redraw only the right cell,
/// and some terminals resolve inheritance from the last placeholder
/// they processed rather than the cell to the left.
pub const PLACEHOLDER_CELLS: [&str; 2] = ["\u{10EEEE}\u{305}\u{305}", "\u{10EEEE}\u{305}\u{30D}"];

/// The number of base64 bytes sent per upload command.
pub const CHUNK: usize = 4096;

/// Encodes one graphics command: `ESC _ G <keys> ; <payload> ESC \`,
/// wrapped for `multiplexer`.
fn command(keys: &str, payload: &[u8], multiplexer: Multiplexer) -> Vec<u8> {
    let mut seq = Vec::with_capacity(START.len() + keys.len() + 1 + payload.len() + END.len());
    seq.extend_from_slice(START);
    seq.extend_from_slice(keys.as_bytes());
    seq.push(b';');
    seq.extend_from_slice(payload);
    seq.extend_from_slice(END);
    tmux::wrap_for(multiplexer, &seq)
}

/// The bytes that upload `rgba` (straight RGBA, `width` x `height`
/// pixels) as image `id` with a 2x1-cell virtual placement for unicode
/// placeholders.
///
/// The base64 payload is split every [`CHUNK`] bytes; each command is
/// wrapped for `multiplexer` separately.
#[must_use]
pub fn upload(
    id: ImageId,
    width: u32,
    height: u32,
    rgba: &[u8],
    multiplexer: Multiplexer,
) -> Vec<u8> {
    let encoded = STANDARD.encode(rgba);
    let first_keys = format!(
        "a=T,U=1,f=32,t=d,s={width},v={height},i={},c=2,r=1,q=2",
        id.get()
    );
    let mut chunks = encoded.as_bytes().chunks(CHUNK).peekable();
    let Some(first) = chunks.next() else {
        return command(&first_keys, b"", multiplexer);
    };
    if chunks.peek().is_none() {
        return command(&first_keys, first, multiplexer);
    }
    let mut out = command(&format!("{first_keys},m=1"), first, multiplexer);
    while let Some(chunk) = chunks.next() {
        let keys = if chunks.peek().is_some() {
            "m=1,q=2"
        } else {
            "m=0,q=2"
        };
        out.extend(command(keys, chunk, multiplexer));
    }
    out
}

/// The bytes that delete image `id` and its placements, wrapped for
/// `multiplexer`.
#[must_use]
pub fn delete(id: ImageId, multiplexer: Multiplexer) -> Vec<u8> {
    let mut seq = Vec::new();
    seq.extend_from_slice(START);
    seq.extend_from_slice(format!("a=d,d=I,i={},q=2", id.get()).as_bytes());
    seq.extend_from_slice(END);
    tmux::wrap_for(multiplexer, &seq)
}

#[cfg(test)]
mod tests {
    // Tests may unwrap: a failure here is a test failure, not a crash.
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn id(n: u32) -> ImageId {
        ImageId::new(n).unwrap()
    }

    fn count(haystack: &[u8], needle: &[u8]) -> usize {
        haystack
            .windows(needle.len())
            .filter(|w| *w == needle)
            .count()
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        count(haystack, needle) > 0
    }

    #[test]
    fn one_pixel_upload_is_exact() {
        assert_eq!(
            upload(id(7), 1, 1, &[1, 2, 3, 4], Multiplexer::None),
            b"\x1b_Ga=T,U=1,f=32,t=d,s=1,v=1,i=7,c=2,r=1,q=2;AQIDBA==\x1b\\".to_vec()
        );
    }

    #[test]
    fn empty_upload_is_one_command_with_empty_payload() {
        assert_eq!(
            upload(id(1), 0, 0, &[], Multiplexer::None),
            b"\x1b_Ga=T,U=1,f=32,t=d,s=0,v=0,i=1,c=2,r=1,q=2;\x1b\\".to_vec()
        );
    }

    #[test]
    fn exactly_one_chunk_has_no_m_key() {
        let out = upload(id(1), 32, 24, &[0; 3072], Multiplexer::None);
        assert_eq!(count(&out, b"\x1b_G"), 1);
        assert!(!contains(&out, b"m="));
        let payload = b"A".repeat(CHUNK);
        let mut expected = b"\x1b_Ga=T,U=1,f=32,t=d,s=32,v=24,i=1,c=2,r=1,q=2;".to_vec();
        expected.extend_from_slice(&payload);
        expected.extend_from_slice(b"\x1b\\");
        assert_eq!(out, expected);
    }

    #[test]
    fn one_byte_more_is_two_chunks() {
        let out = upload(id(1), 32, 24, &[0; 3073], Multiplexer::None);
        assert_eq!(count(&out, b"\x1b_G"), 2);
        let mut expected = b"\x1b_Ga=T,U=1,f=32,t=d,s=32,v=24,i=1,c=2,r=1,q=2,m=1;".to_vec();
        expected.extend_from_slice(&b"A".repeat(CHUNK));
        expected.extend_from_slice(b"\x1b\\\x1b_Gm=0,q=2;AA==\x1b\\");
        assert_eq!(out, expected);
    }

    #[test]
    fn three_chunks_have_a_middle_continuation() {
        let out = upload(id(1), 64, 48, &[0; 3072 * 2 + 1], Multiplexer::None);
        assert_eq!(count(&out, b"\x1b_G"), 3);
        assert_eq!(count(&out, b",m=1;"), 1);
        assert_eq!(count(&out, b"\x1b_Gm=1,q=2;"), 1);
        assert_eq!(count(&out, b"\x1b_Gm=0,q=2;"), 1);
        assert!(out.ends_with(b"\x1b_Gm=0,q=2;AA==\x1b\\"));
    }

    #[test]
    fn each_chunk_is_wrapped_separately_under_tmux() {
        let out = upload(id(1), 64, 48, &[0; 3072 * 2 + 1], Multiplexer::Tmux);
        assert_eq!(count(&out, b"\x1bPtmux;"), 3);
        let single = upload(id(1), 1, 1, &[1, 2, 3, 4], Multiplexer::Tmux);
        assert_eq!(
            single,
            tmux::passthrough(&upload(id(1), 1, 1, &[1, 2, 3, 4], Multiplexer::None))
        );
    }

    #[test]
    fn delete_is_exact() {
        assert_eq!(
            delete(id(42), Multiplexer::None),
            b"\x1b_Ga=d,d=I,i=42,q=2\x1b\\".to_vec()
        );
    }

    #[test]
    fn delete_under_tmux_is_wrapped() {
        assert_eq!(
            delete(id(42), Multiplexer::Tmux),
            b"\x1bPtmux;\x1b\x1b_Ga=d,d=I,i=42,q=2\x1b\x1b\\\x1b\\".to_vec()
        );
    }

    #[test]
    fn image_id_bounds() {
        assert_eq!(ImageId::new(0), None);
        assert_eq!(ImageId::new(0x100_0000), None);
        assert_eq!(ImageId::new(1).map(ImageId::get), Some(1));
        assert_eq!(ImageId::new(0xFF_FFFF).map(ImageId::get), Some(0xFF_FFFF));
    }

    #[test]
    fn next_counts_up_and_stops_at_the_top() {
        assert_eq!(id(1).next(), Some(id(2)));
        assert_eq!(id(0xFF_FFFF).next(), None);
    }

    #[test]
    fn color_spells_the_id() {
        assert_eq!(id(0x12_3456).color(), Color::Rgb(0x12, 0x34, 0x56));
    }

    #[test]
    fn placeholder_cells_are_one_column_and_eight_bytes() {
        for cell in PLACEHOLDER_CELLS {
            assert_eq!(cell.len(), 8);
            assert_eq!(unicode_width::UnicodeWidthStr::width(cell), 1);
        }
    }
}
