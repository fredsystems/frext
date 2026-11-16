//! frext-core — the frontend-independent core of frext.
//!
//! Buffers, persistence, the workspace tree, search, proofreading,
//! highlighting spans and the colour palette live here so they can be
//! unit-tested and shared by any front end. This crate never depends on a
//! UI toolkit.

pub mod clipboard;
pub mod edit_ops;
pub mod error;
pub mod file_icon;
mod file_icon_bytes;
mod file_icon_table;
pub mod fs_ops;
pub mod highlight;
pub mod lines;
pub mod offsets;
pub mod palette;
pub mod persistence;
pub mod proofread;
pub mod search;
pub mod syntax;
pub mod tab;
pub mod workspace;
