//! Terminal capability detection.
//!
//! What the hosting terminal can do: the capability types, the startup
//! probe that detects them, and the undercurl-capable backend that draws
//! with them. The terminal's lifecycle (raw mode, alternate screen,
//! restore) stays in [`crate::terminal`].

pub mod backend;
pub mod caps;
pub mod late_replies;
pub mod probe;
pub mod tmux;
