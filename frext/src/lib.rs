//! frext — Fred Text, a super lightweight text editor.
//!
//! The crate is split into a thin binary (`main.rs`) over this library so the
//! persistence and buffer logic can be unit-tested without a windowing
//! backend.

pub mod app;
pub mod clipboard;
pub mod edit_ops;
pub mod editor_layout;
pub mod error;
pub mod file_icon;
mod file_icon_bytes;
mod file_icon_table;
pub mod font;
pub mod fs_ops;
pub mod highlight;
pub mod icon;
pub mod palette;
pub mod persistence;
pub mod proofread;
pub mod search;
pub mod syntax;
pub mod tab;
pub mod theme;
pub mod workspace;
