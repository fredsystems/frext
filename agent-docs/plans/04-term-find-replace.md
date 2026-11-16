# 04 -- TUI find and replace

- **Status:** stub
- **Depends on:** 03

## Goal

The find/replace bar under the tab bar, matching the GUI: a find
field, toggles for match case / whole word / regex / in-selection,
Prev / Next, a match counter, a Replace toggle, an optional replace
row with Replace / Replace all, and an inline regex error. Matches are
highlighted in the buffer view, with the current match distinct.

## Task summary

- Render the bar from `Editor::search()`; bind the two fields with
  plan 02's single-line input to `search_pattern_mut` /
  `search_replacement_mut`, sending `RecomputeMatches` on change.
- Toggles are clickable (`[Aa] [W] [.*] [sel]`) and also have keyboard
  bindings.
- Enter / Shift+Enter in the find field and F3 / Shift+F3 anywhere send
  `StepMatch`; Esc sends `CloseSearch`.
- Ctrl+F / Ctrl+H send `OpenSearch`. Seeding from a single-line
  selection already happens in the core.
- Buffer view: `Editor::match_highlights()` -> the
  `MatchBackground::{Other, Current}` roles, composed per cell by plan
  03's rendering pass.
- `Effect::FocusSearchField` moves keyboard focus to the find field.

## Durable decisions

- All search semantics already live in `frext-core` (plan 01). This plan
  is presentation and input only; it must not add search logic to
  `frext-term`.

## Decide at activation

- Keyboard bindings for the four toggles (Alt+C / Alt+W / Alt+R / Alt+S
  proposed; check for collisions with the kitty keyboard protocol and
  common terminal shortcuts).
- Focus traversal inside the bar (Tab between find and replace fields).
