# 03 -- TUI buffer view (editor widget)

- **Status:** stub
- **Depends on:** 02

## Goal

A custom ratatui editing widget for the active tab, matching the GUI
editor: soft wrap, a line-number gutter that numbers only the first
visual row of each logical line, syntax highlighting, selection, mouse
and keyboard editing, undo/redo, and clipboard integration. Search-match
backgrounds and proofreading underlines are rendered by later plans
through the same per-cell styling pass.

## Task summary

- Text model: the buffer stays `Tab::text: String`, edited through
  `Editor::active_view_mut` and `commit_edit`, exactly as the GUI does.
  The widget owns only view state: caret and anchor (char indices),
  preferred column for vertical motion, scroll offset, and undo
  history.
- Layout: visual rows computed from the buffer and the editor width:
  grapheme clusters (`unicode-segmentation`), display width
  (`unicode-width`), tab expansion, wrapping at whitespace with a
  fallback to grapheme boundaries. The gutter reuses
  `frext_core::lines::line_numbers_for_rows`.
- Rendering writes straight into the ratatui `Buffer` cell by cell (not
  through `Paragraph`), merging per cell: the syntax span fg/italic,
  the selection background, and (plans 04/07) the match background and
  diagnostic underline. Selection and match colours come from
  `palette::Tint::over(BASE)`.
- Editing: insert, delete, Backspace, Enter (keeping the GUI's
  `code_editor()` indentation behaviour, to be verified at activation),
  Tab, word and line motions, Home/End, PageUp/PageDown,
  Ctrl+Home/End, Shift-extended selections, Ctrl+A, mouse click /
  double-click (word) / drag to select, wheel scroll.
- Undo/redo: an `EditHistory` with coalesced typing runs; Ctrl+Z,
  Ctrl+Shift+Z and Ctrl+Y.
- Core commands: Cut / Copy / Paste / SelectAll go through
  `Editor::apply` with `set_selection`, and the widget applies the
  returned `Effect`s (caret, reveal).
- Scroll-to-caret, and `Effect::RevealSelection` centring.

## Durable decisions

- **Custom widget, not `tui-textarea` / `ratatui-textarea` / `edtui`.**
  Those own their own line storage and either lack arbitrary per-range
  styling or impose modal (Vim) editing. frext needs per-cell
  composition of syntax, selection, match and diagnostic styles, and
  non-modal GUI-parity editing.
- **Char indices are the caret currency**, matching `frext-core`
  (`set_selection`, `Effect::SetSelection`) and the GUI.
- **Undo lives in the TUI for now.** The GUI keeps egui's built-in
  undo. If parity problems show up, promoting `EditHistory` into
  `frext-core` for both front ends is a later, separate plan.
- **`String` buffer, not a rope.** Consistent with the GUI, swap
  persistence and proofreading. Revisit only with benchmark evidence.

## Decide at activation

- Highlighting cost: today the GUI re-highlights the whole buffer on
  every layout. A TUI redraw per keystroke on a large file needs an
  incremental highlighter (syntect parse-state checkpoints every N
  lines, invalidated from the first edited line, highlighting only up
  to the viewport). Decide whether it lives in `frext-core::highlight`
  (so the GUI can adopt it) or in the TUI. Either way it is a
  benchmark-mandated area: extend `frext/benches/highlight.rs` or add a
  `frext-core` bench before and after.
- Tab width (match whatever egui's `code_editor()` renders) and whether
  to show whitespace.
- Word-boundary rules for double-click and Ctrl+Left/Right (Unicode
  word segmentation versus code-identifier rules).
- Whether the widget's layout pass is cached between frames when
  neither text nor width changed.
