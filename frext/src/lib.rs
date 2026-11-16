//! frext — Fred Text, a super lightweight text editor.
//!
//! This crate is the egui/eframe front end: a thin binary (`main.rs`) over
//! this library of GUI modules. The frontend-independent buffer,
//! persistence, search, proofreading and highlighting logic lives in the
//! [`frext_core`] crate.

pub mod app;
pub mod editor_layout;
pub mod font;
pub mod icon;
pub mod squiggle;
pub mod theme;
