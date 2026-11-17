//! tmux passthrough wrapping.
//!
//! tmux swallows escape sequences it does not understand, such as the
//! kitty graphics protocol. Wrapping one in `ESC P tmux; ... ESC \`
//! (with every inner `ESC` doubled) asks tmux to pass it through to the
//! outer terminal. tmux only honours this when the user has set
//! `set -g allow-passthrough on`.

use super::caps::Multiplexer;

/// The escape byte.
const ESC: u8 = 0x1b;

/// Opens a tmux passthrough sequence.
const PREFIX: &[u8] = b"\x1bPtmux;";

/// Closes a tmux passthrough sequence (ST).
const SUFFIX: &[u8] = b"\x1b\\";

/// Wraps `seq` for tmux passthrough: `ESC P tmux ;`, then `seq` with
/// every `ESC` doubled, then `ESC \`.
#[must_use]
pub fn passthrough(seq: &[u8]) -> Vec<u8> {
    let escapes = seq.iter().filter(|&&b| b == ESC).count();
    let mut out = Vec::with_capacity(PREFIX.len() + seq.len() + escapes + SUFFIX.len());
    out.extend_from_slice(PREFIX);
    for &byte in seq {
        if byte == ESC {
            out.push(ESC);
        }
        out.push(byte);
    }
    out.extend_from_slice(SUFFIX);
    out
}

/// Wraps `seq` for `multiplexer`: [`passthrough`] under tmux, otherwise
/// `seq` unchanged.
#[must_use]
pub fn wrap_for(multiplexer: Multiplexer, seq: &[u8]) -> Vec<u8> {
    match multiplexer {
        Multiplexer::Tmux => passthrough(seq),
        Multiplexer::None => seq.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_doubles_inner_escapes() {
        assert_eq!(
            passthrough(b"\x1b_Gq\x1b\\"),
            b"\x1bPtmux;\x1b\x1b_Gq\x1b\x1b\\\x1b\\".to_vec()
        );
    }

    #[test]
    fn passthrough_of_nothing_is_just_the_wrapper() {
        assert_eq!(passthrough(b""), b"\x1bPtmux;\x1b\\".to_vec());
    }

    #[test]
    fn wrap_for_no_multiplexer_is_unchanged() {
        let seq = b"\x1b_Gq\x1b\\";
        assert_eq!(wrap_for(Multiplexer::None, seq), seq.to_vec());
    }

    #[test]
    fn wrap_for_tmux_is_passthrough() {
        let seq = b"\x1b_Gq\x1b\\";
        assert_eq!(wrap_for(Multiplexer::Tmux, seq), passthrough(seq));
    }
}
