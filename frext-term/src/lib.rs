//! frext-term — the terminal front end for Fred Text.
//!
//! This crate is a thin binary (`main.rs`) over this library of terminal
//! modules. The frontend-independent editor logic lives in the
//! [`frext_core`] crate.

pub mod app;
pub mod buffer;
pub mod cli;
pub mod clipboard;
pub mod complete;
pub mod editor_menu;
pub mod events;
pub mod find_bar;
pub mod hit;
pub mod icons;
pub mod keymap;
pub mod layout;
pub mod logging;
pub mod menu;
pub mod paint;
pub mod popup;
pub mod prompt;
pub mod run;
pub mod status;
pub mod tab_bar;
pub mod terminal;
pub mod text_input;
pub mod theme;
