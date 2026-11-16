# 07 -- TUI proofreading

- **Status:** stub
- **Depends on:** 03, 06

## Goal

Spelling and grammar squiggles in the TUI buffer view, with the same
fix-it menu as the GUI: Harper's message, suggested replacements,
"Add to dictionary" for spelling, and "Ignore".

## Task summary

- Underlines: for each diagnostic from
  `ActiveView::proofread.diagnostics(tab.id, &tab.text)`, cells covering
  its char range get `Modifier::UNDERLINED` plus `underline_color` from
  `palette::spelling_squiggle()` (red) or `grammar_squiggle()` (green).
  `FrextBackend` (plan 06) turns that into undercurl, or a straight
  coloured underline when undercurl is off or unsupported. A diagnostic
  that crosses a soft wrap is underlined on every visual row it covers;
  the newline itself is never underlined (matching
  `squiggle::segments` in the GUI).
- Context menu: a right-click on a diagnostic, or Shift+F10 / the Menu
  key with the caret inside one, opens the editor context menu with the
  diagnostic items first, then Cut / Copy / Paste / Select all / Find /
  Find and replace / Save, mirroring the GUI's `editor_context_menu`.
  Choices send `ApplySuggestion`, `AddToDictionary` and `Ignore`.
- Status bar: when the caret is inside a diagnostic, show its message.
- Repaint: the proofread worker's `notify` (plan 02's event channel)
  triggers a redraw when results land, so squiggles appear without
  further input.

## Durable decisions

- No new proofreading logic in `frext-term`. Worker, regions, results,
  dictionary and ignore set are all `frext-core` (plan 01), and the
  dictionary file is shared with the GUI (plan 02's merge-on-write).
- Underline styling is expressed only through ratatui's
  `UNDERLINED` + `underline_color`; the curly-versus-straight decision
  belongs to the backend and `TerminalCaps`, never to this plan's
  rendering code.

## Decide at activation

- Precedence when a selection or search-match background overlaps a
  squiggle (underlines are independent of background, so both can show;
  confirm the visual).
- Whether the context menu opens at the mouse position or at the caret
  for keyboard invocation, and how it is placed near the screen edges.
