//! The editable text buffer view.
//!
//! [`cells`] decides how one grapheme is drawn and how many terminal
//! cells it takes, and [`wrap`] lays a text out into soft-wrapped visual
//! rows and maps a caret between char indices and (row, column).
//! [`words`] finds word and line boundaries for word motions and
//! double/triple-click selection.

pub mod cells;
pub mod words;
pub mod wrap;
