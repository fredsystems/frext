//! The editable text buffer view.
//!
//! [`cells`] decides how one grapheme is drawn and how many terminal
//! cells it takes, and [`wrap`] lays a text out into soft-wrapped visual
//! rows and maps a caret between char indices and (row, column).

pub mod cells;
pub mod wrap;
