//! The capability types.
//!
//! [`TerminalCaps`] records what the hosting terminal supports. All
//! rendering reads it; [`TerminalCaps::default`] describes a terminal
//! that answered nothing.

/// The size of one terminal cell, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellPx {
    /// The width of one cell, in pixels.
    pub width: u16,
    /// The height of one cell, in pixels.
    pub height: u16,
}

/// How the terminal can draw images.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ImageSupport {
    /// The kitty graphics protocol with unicode placeholders.
    KittyPlaceholders {
        /// The cell size, used to size rasterised images.
        cell_px: CellPx,
    },
    /// No usable image support.
    #[default]
    None,
}

/// How a coloured underline is drawn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum UnderlineStyle {
    /// A curly underline ("undercurl", `CSI 4:3 m`).
    Curly,
    /// A plain straight underline.
    #[default]
    Straight,
}

/// Which keyboard protocol the terminal speaks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum KeyboardProtocol {
    /// The kitty keyboard protocol.
    Kitty {
        /// The progressive-enhancement flags the terminal reported.
        flags: u16,
    },
    /// Legacy keyboard input only.
    #[default]
    Legacy,
}

/// The terminal multiplexer frext runs inside, if any.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Multiplexer {
    /// No multiplexer.
    #[default]
    None,
    /// tmux.
    Tmux,
}

/// Everything frext knows about the hosting terminal.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TerminalCaps {
    /// How images can be drawn.
    pub images: ImageSupport,
    /// How a coloured underline is drawn.
    pub underline: UnderlineStyle,
    /// Which keyboard protocol the terminal speaks.
    pub keyboard: KeyboardProtocol,
    /// The multiplexer frext runs inside.
    pub multiplexer: Multiplexer,
    /// The raw XTVERSION reply, when the terminal sent one.
    pub version: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_caps_describe_a_terminal_that_answered_nothing() {
        let caps = TerminalCaps::default();
        assert_eq!(caps.images, ImageSupport::None);
        assert_eq!(caps.underline, UnderlineStyle::Straight);
        assert_eq!(caps.keyboard, KeyboardProtocol::Legacy);
        assert_eq!(caps.multiplexer, Multiplexer::None);
        assert_eq!(caps.version, None);
    }
}
