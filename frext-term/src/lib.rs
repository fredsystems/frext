//! frext-term — the terminal front end for Fred Text.
//!
//! This crate is a thin binary (`main.rs`) over this library of terminal
//! modules. The frontend-independent editor logic lives in the
//! [`frext_core`] crate.

pub mod cli;
pub mod clipboard;
pub mod complete;
pub mod events;
pub mod hit;
pub mod layout;
pub mod logging;
pub mod popup;
pub mod prompt;
pub mod status;
pub mod tab_bar;
pub mod terminal;
pub mod text_input;
pub mod theme;
