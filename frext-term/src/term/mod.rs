//! Terminal capability detection.
//!
//! What the hosting terminal can do: the capability types, the startup
//! probe that detects them, and the undercurl-capable backend that draws
//! with them, plus the escape-sequence encoders they enable (kitty
//! graphics, tmux passthrough). The terminal's lifecycle (raw mode,
//! alternate screen, restore) stays in [`crate::terminal`].

pub mod backend;
pub mod caps;
pub mod kitty;
pub mod late_replies;
pub mod probe;
pub mod tmux;
