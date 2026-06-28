//! frext — Fred Text, a super lightweight text editor.
//!
//! The crate is split into a thin binary (`main.rs`) over this library so the
//! persistence and buffer logic can be unit-tested without a windowing
//! backend.

pub mod app;
pub mod error;
pub mod highlight;
pub mod persistence;
pub mod tab;
pub mod theme;
pub mod workspace;
