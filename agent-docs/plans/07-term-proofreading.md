# 07 -- TUI proofreading

- **Status:** active
- **Depends on:** 03, 06
- **Branch:** `feat/frext-term-proofreading` (one branch, one commit per
  subtask, one PR)

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

## Decided at activation

- **Squiggles and backgrounds both show.** The background keeps
  today's precedence (selection, then search match, then base) and the
  coloured underline is drawn on top of whichever wins. Where a
  spelling and a grammar diagnostic overlap, the cell takes the
  spelling colour, matching `Proofreading::diagnostic_at`'s preference.
- **Menu placement.** A right-click opens the menu at the mouse cell
  (as today). The keyboard opens it **below the caret**: it starts on
  the row under the caret's cell, and when it does not fit there it
  opens upwards so that it ends on the row above the caret. The flagged
  word stays visible either way. When the caret is scrolled off screen,
  the menu opens at the editor's top-left cell. Edges are handled by
  `ContextMenu::rect` (shift left, flip, clamp).

## Further decisions (orchestrator)

These come from reading the current code: `frext-core`'s
`proofread/{mod,state,worker}.rs`, `editor/{mod,command,editing}.rs` and
`palette.rs`; the GUI's `squiggle.rs` and `editor_context_menu`; and
`frext-term`'s `buffer/{mod,render,wrap,cells}.rs`, `menu.rs`,
`editor_menu.rs`, `popup.rs`, `status.rs`, `keymap.rs`, `tree.rs`,
`events.rs`, `run.rs`, `app.rs`, `theme.rs` and `benches/buffer.rs`.

- **Facts from the audit.**
  - `Proofreading::diagnostics(&self, tab_id, text) -> Vec<&Diagnostic>`
    takes `&self`, hides stale, ignored and newly-added-word results,
    and returns them sorted by `(bytes.start, bytes.end)` (so by
    `chars.start` too). Diagnostics **can overlap** (a grammar
    diagnostic over "the wrrod" and a spelling one over "wrrod").
  - `Proofreading::diagnostic_at(tab_id, text, char_idx)` already
    exists. Its end is **inclusive** (the caret just after a word
    counts) and it prefers a spelling diagnostic. The TUI uses it for
    both the menu and the status line; no new lookup is written.
  - `Editor::proofread() -> &Proofreading` lets the render path read
    diagnostics beside `active_tab()` without `active_view_mut()`.
  - **Repaint already works.** `run.rs` passes `events.waker()` to
    `begin_frame`, the worker calls it after each result, the `Wake`
    batch ends `next_batch`, and every batch is followed by
    `begin_frame` (which polls) and a full redraw. No new wiring is
    needed; 07.3 pins it with a test that drives the same path.
  - `ApplySuggestion` is already in `TermApp::send`'s list of edits
    recorded as one undo group. `AddToDictionary` and `Ignore` return
    no effects.
  - A `VisualRow` never contains the `'\n'` (or the `'\r'` before
    it), so no newline cell exists to underline. Whitespace at a soft
    wrap stays on the earlier row; when it is inside a diagnostic it is
    underlined there, as the GUI's galley rows do.
  - `menu.rs` has only `Item` and `Separator`; there is no text row and
    labels never wrap. `popup::wrap(text, width)` already wraps text
    by display width.
  - The editor has no keyboard menu binding. The tree's (Shift+F10, and
    `F(22)` / `Menu` plain or shifted) live in `FileTree::handle_key`,
    reached only when `keymap::action_for` returns `None`.
  - `BufferView` has no way to map a screen cell to a char without
    moving the caret (`char_at_cell` is private and used by
    `mouse_down` / `mouse_drag`), and no way to read the caret's screen
    cell outside `render`.
  - `benches/buffer.rs` calls `BufferView::render`, so it changes with
    the new parameter and squiggle drawing is on a benchmarked path.
- **The menu text row.** `MenuEntry::Note(String)`: one row of
  non-choosable text, drawn in `theme::disabled()` (the TUI's stand-in
  for the GUI's weak label), padded like an item, counted in the
  menu's width, never highlighted, and with no `MenuItem` hit region
  (a click on it hits `MenuPopup` and is consumed).
- **The keyboard anchor.** `menu::MenuAnchor { At(Position),
  Below(Position) }`. `ContextMenu::new(entries, at)` keeps meaning
  `At`; `ContextMenu::below(entries, cell)` is new.
- **Diagnostic menu items** (`editor_menu::entries(selection,
  diagnostic: Option<&Diagnostic>)`), exactly the GUI's order and
  labels:
  1. The message, wrapped by `popup::wrap(&diagnostic.message,
     MESSAGE_WIDTH)` with `pub const MESSAGE_WIDTH: usize = 40`, one
     `Note` per line.
  2. With no suggestions, a disabled item `No suggestions` (its value
     is `Command::Ignore(diagnostic.ignore_key())`, never sent because
     it is disabled). Otherwise one enabled item per suggestion,
     labelled with the suggestion, or `Remove “<flagged>”` for an empty
     one, sending `ApplySuggestion { bytes, flagged, replacement }`.
     No bold: the menu has no per-item emphasis, and adding it is out
     of scope.
  3. A separator.
  4. For `DiagnosticKind::Spelling` only, `Add “<flagged>” to
     dictionary`, sending `AddToDictionary(flagged)`.
  5. `Ignore`, sending `Ignore(diagnostic.ignore_key())`.
  6. A separator, then today's nine entries unchanged.
- **Squiggle colour.** `theme::squiggle(kind: DiagnosticKind) -> Color`:
  `color(palette::spelling_squiggle())` or
  `color(palette::grammar_squiggle())`.
- **Squiggle drawing.** `RowsView` gains `diagnostics: &'a [&'a
  Diagnostic]` (the slice `Proofreading::diagnostics` returns, sorted,
  possibly overlapping). `render` first keeps only the diagnostics
  whose `chars` intersect, as half-open ranges, the visible chars
  (from the first visible row's `chars.start` to the last visible
  row's `chars.end`). For each grapheme, with
  `char_idx` its first char, the underline is:
  - none when no kept diagnostic has `chars.contains(&char_idx)`;
  - else `theme::squiggle(Spelling)` when any containing one is
    spelling, otherwise `theme::squiggle(Grammar)`.

  It is applied as `style.add_modifier(Modifier::UNDERLINED)
  .underline_color(colour)` to the `style` passed to `style_cells`, so
  every cell of a tab or wide char is underlined too. The selection
  newline cell is never underlined.
- **Where diagnostics come from.** `BufferView::render`'s `matches`
  parameter becomes `overlays: Overlays<'_>`, a new
  `#[derive(Debug, Clone, Copy, Default)] pub struct Overlays<'a> {
  pub matches: MatchHighlightsRef<'a>, pub diagnostics: &'a [&'a
  Diagnostic] }` in `buffer/mod.rs`, so the signature stays at seven
  arguments and needs no `too_many_arguments` suppression (07.2
  review). `TermApp::render` computes
  `self.editor.proofread().diagnostics(tab.id, &tab.text)` for the
  active tab (empty without one).
- **The status line.** The left side's default text (shown when there
  is no transient message and no hover hint) becomes the message of
  `diagnostic_at(tab.id, &tab.text, caret)` when the caret is in a
  diagnostic, else the path as today. `StatusLine` is unchanged.
- **New `BufferView` queries** (both read-only, over the last
  rendered rect and layout):
  - `pub fn char_at_cell(&self, tab: &Tab, column: u16, row: u16) ->
    Option<usize>`: `None` when the cell is outside the editor rect
    or the view has no layout for `tab`; else the same char
    `char_at_cell` gives a click there.
  - `pub fn caret_cell(&self, tab: &Tab) -> Option<Position>`: the
    caret's screen cell, computed by the same function `render` uses
    (so `None` when scrolled off or past the right edge, or when the
    view does not know `tab`).
  - `pub const fn rect(&self) -> Rect`: the editor rect of the last
    render (the keyboard menu's fallback anchor is its top-left).
- **The keyboard binding.** `keymap::Action::OpenEditorMenu`, returned
  by `action_for` only when `ctx.focus == Focus::Editor`, for
  `F(10)` with exactly Shift, and `F(22)` / `Menu` with no modifiers
  or exactly Shift. The sidebar keeps its own handling unchanged.
- **No new dependency.**

## Execution model

All subtasks run **strictly in sequence** on
`feat/frext-term-proofreading`. Each subtask is one commit and leaves
the suite green (`commit-discipline`). The orchestrator reviews the
full diff of each subtask before committing it. Implementers do not
commit, merge or push.

Each subtask's verification includes the standard ritual, run inside
`nix develop`:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
cargo machete
```

All code follows the workspace rules:

- no `unwrap` / `expect` outside tests
- no `unsafe`
- no indexing that can panic (use `get`, or ranges proven in bounds)
- every public item tested
- doc comments on every public item
- no new dependency
- no bare `bool` parameters or fields for states (`state-representation`)

Benchmarks: 07.2 changes `buffer::render`, which `benches/buffer.rs`
measures. It captures a `before` baseline of the `buffer` bench before
editing, compares after, adds a squiggle bench, and reports the table
(`performance-benchmarks`). No other subtask touches a benchmarked
path.

Tests that need real diagnostics run Harper through the editor's own
worker: a helper starts it with a notify closure that sends on an
`mpsc` channel, queues a check with `end_frame`, waits with
`recv_timeout(60 s)` (event-driven, the same pattern as
`proofread/worker.rs`'s tests), then calls `begin_frame` to poll. They
never sleep. Expectations are taken from the diagnostics that came
back (for example "a spelling diagnostic whose `flagged` is `tset`"),
not from Harper's exact wording.

## Subtasks

### 07.1 -- Menu: text rows and the below-the-caret anchor

Scope: `frext-term/src/menu.rs`.

What:

- `MenuEntry::Note(String)`, documented as one row of text that
  cannot be chosen. `is_enabled_item` stays false for it, so keys and
  hover skip it.
- `rect`: a note's width is its text's display width, like an item's
  label.
- `render`: a note row is styled `theme::disabled()` across the inner
  width with its text at `inner.x + 1`, truncated like a label, and
  pushes **no** hit region.
- `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum MenuAnchor {
  At(Position), Below(Position) }`, documented: `At` opens at the cell
  (moved left or upwards to stay on screen); `Below` opens on the row
  under the cell, or upwards ending on the row above it.
- `ContextMenu.anchor: MenuAnchor`. `new(entries, anchor: Position)`
  stores `At(anchor)`, unchanged for callers. New `pub const fn
  below(entries: Vec<MenuEntry<T>>, cell: Position) -> Self` stores
  `Below(cell)`.
- `rect` for `Below(cell)`: `x = cell.x`, `y = cell.y + 1` (saturating);
  when `y + height > area.bottom()`, `y = cell.y.saturating_sub(height)`.
  The horizontal shift and the final clamp are the same as `At`.
  `At` is byte-for-byte today's behaviour.

Deliverable: tests:

- a note is drawn in `theme::disabled()`, widens the menu, and is
  skipped by Down/Up and by `hover(Some(i))`
- a click on a note row is `MenuReply::Handled` (it hits
  `MenuPopup`), and no `MenuItem` hit lies on it
- `below` opens under the cell (e.g. cell `(5, 3)` gives `y == 4`)
- `below` near the bottom opens upwards ending on `cell.y - 1`
- `below` near the right edge shifts left
- `below` on a screen too small either way is clamped into the area
- every existing test passes unchanged

Verification: the standard ritual.

Prohibitions: do NOT touch any other file; do NOT change `At`
placement; do NOT add wrapping inside the menu; do NOT proceed to 07.2.

Stop: report files changed and verification; await review.

### 07.2 -- Buffer: draw squiggles

Scope: `frext-term/src/theme.rs`, `frext-term/src/buffer/render.rs`,
`frext-term/src/buffer/mod.rs`, `frext-term/src/app.rs` (the one
`buffer.render` call only), `frext-term/benches/buffer.rs`.

What:

- **First**, before editing anything:
  `cargo bench -p frext-term --bench buffer -- --save-baseline before`.
- `theme.rs`: `#[must_use] pub const fn squiggle(kind: DiagnosticKind)
  -> Color` as decided.
- `render.rs`:
  - `RowsView` gains `pub diagnostics: &'a [&'a Diagnostic]`,
    documented as the proofreading diagnostics of `text`, sorted by
    start, possibly overlapping.
  - `render` keeps the diagnostics intersecting the visible rows'
    chars (a `Vec<&Diagnostic>` built once per call) and hands them to
    the `Painter` (a new field).
  - `draw_row` applies the underline rule decided above to `style`
    before `style_cells`. The newline selection cell is unchanged.
  - The module doc mentions the diagnostic underline.
- `buffer/mod.rs`: `Overlays` as decided under "Where diagnostics
  come from"; `BufferView::render` takes it in place of `matches`,
  documented, and passes `overlays.diagnostics` into `RowsView`. Its
  tests pass `Overlays { matches, ..Overlays::default() }` or
  `Overlays::default()` where they do not test squiggles.
- `app.rs`: the existing `self.buffer.render(..)` call passes
  `Overlays { matches: self.editor.match_highlights_ref(),
  diagnostics: &[] }`.
  (07.3 passes real diagnostics.)
- `benches/buffer.rs`: existing benches pass no diagnostics. Add
  `buffer/render_with_squiggles_rust_5k_lines`: the same text and
  view, with one synthetic `Diagnostic` (alternating `Spelling` and
  `Grammar`, `suggestions` empty) over every occurrence of `self`,
  built once outside the timed loop with correct byte and char
  ranges.
- **Last**: `cargo bench -p frext-term --bench buffer -- --baseline
  before`.

Deliverable: tests in `render.rs` (build `Diagnostic` values
directly; their fields are public):

- a spelling diagnostic underlines exactly its cells: each has
  `Modifier::UNDERLINED` and `underline_color ==
  Some(theme::squiggle(Spelling))`; the cells before and after do not
- grammar uses `theme::squiggle(Grammar)`
- a diagnostic across a soft wrap is underlined on both rows, and the
  cell after the last char of a line (the newline cell) is not, even
  when selected
- overlapping grammar + spelling: the overlap is spelling-coloured,
  the grammar-only part grammar-coloured
- a selected and a search-matched underlined cell keep their
  selection / match background and the underline
- a tab inside a diagnostic underlines all four cells; a wide char
  both cells (frame-buffer check, as `wide_char_hidden_cell_keeps_background`)
- diagnostics entirely above or below the visible rows draw nothing
  and do not panic; an empty slice draws no underline anywhere

Verification: the standard ritual, plus the bench comparison. Report
the before/after table for every `buffer/*` benchmark and the new
squiggle bench's time. A regression over 15% must be justified or the
change revised.

Prohibitions: do NOT read diagnostics from the editor (07.3); do NOT
touch menus, keymap or status; do NOT change the background
precedence; do NOT proceed to 07.3.

Stop: report files changed, verification and the bench table; await
review.

### 07.3 -- App: squiggles from the editor, and the status message

Scope: `frext-term/src/app.rs`.

What:

- `render` passes
  `self.editor.proofread().diagnostics(tab.id, &tab.text)` for the
  active tab (an empty `Vec` with no tab) to `self.buffer.render`.
- The status line's left default is the message of
  `self.editor.proofread().diagnostic_at(tab.id, &tab.text, caret)`
  when `self.buffer.caret(tab.id)` is `Some(caret)` and a diagnostic
  is there; else today's path text.
- Test helper in the test module, documented:
  `fn proofread(app: &mut TermApp)` as described under "Execution
  model" (it must be the first `begin_frame` on that app).

Deliverable: tests (each over `app_with_text` with a sentence
containing a misspelling such as `"This is a tset of the editor."`):

- before proofreading, no editor cell is underlined; after
  `proofread` and a draw, the cells of the `tset` spelling diagnostic
  are underlined in `theme::squiggle(Spelling)` (this is the repaint
  path: notify fired, `begin_frame` polled, the next draw shows it)
- with the caret moved into `tset`, the status row shows the
  diagnostic's message; moved to the start of the text (outside every
  diagnostic), it shows the path again
- a transient status message still takes precedence over the
  diagnostic message

Verification: the standard ritual.

Prohibitions: do NOT touch menus or keys; do NOT change `StatusLine`;
do NOT proceed to 07.4.

Stop: report files changed and verification; await review.

### 07.4 -- Editor menu: the diagnostic items

Scope: `frext-term/src/editor_menu.rs`, `frext-term/src/app.rs` (the
`editor_menu::entries` call sites only, passing `None`).

What:

- `pub const MESSAGE_WIDTH: usize = 40;`, documented.
- `pub fn entries(selection: Enablement, diagnostic:
  Option<&Diagnostic>) -> Vec<MenuEntry<Command>>`: with a diagnostic,
  the items decided under "Diagnostic menu items", then a separator,
  then today's entries; without one, exactly today's entries.
- The module doc drops "Plan 07 prepends …" and describes the
  diagnostic items.
- `app.rs`: `open_editor_menu` and the tests that build
  `editor_menu::entries(..)` pass `None`. No behaviour change.

Deliverable: tests in `editor_menu.rs` with hand-built diagnostics:

- spelling with two suggestions: the exact full entry list
- a suggestion of `""` is labelled `Remove “<flagged>”` and sends an
  empty replacement
- grammar has no "Add to dictionary"
- no suggestions gives one disabled `No suggestions`
- a message longer than `MESSAGE_WIDTH` becomes several `Note`s, each
  at most `MESSAGE_WIDTH` wide, joining back to the message's words
- `None` is unchanged from today

Verification: the standard ritual.

Prohibitions: do NOT look up diagnostics in `app.rs`; do NOT change
the existing nine entries; do NOT proceed to 07.5.

Stop: report files changed and verification; await review.

### 07.5 -- Buffer: read-only cell and caret queries

Scope: `frext-term/src/buffer/mod.rs`,
`frext-term/src/buffer/render.rs`.

What:

- `render.rs`: replace the private `caret_position(view, editor)` with
  `pub(crate) fn caret_cell(layout: &WrapLayout, text: &str, top_row:
  usize, caret: usize, editor: Rect) -> Option<Position>`, the same
  arithmetic, documented; `render` calls it. No behaviour change.
- `buffer/mod.rs`: `BufferView::char_at_cell`, `BufferView::caret_cell`
  and `BufferView::rect` as decided under "New `BufferView` queries".
  `char_at_cell` reuses the private `char_at_cell` function after
  checking the cell is inside `self.rect`; `caret_cell` calls
  `render::caret_cell` with the view's layout, `top_row(layout,
  view.top)`, the cursor's caret and `self.rect`.

Deliverable: tests:

- after a render, `char_at_cell` on a cell of the second wrapped row
  gives the same char a click there would, and does not move the
  caret or selection
- outside the editor rect, and for a tab the view does not know,
  `char_at_cell` is `None`
- `caret_cell` matches the position `render` returned, follows a
  scroll, and is `None` when the caret row is scrolled off
- `rect` is the editor rect passed to the last `render`
- every existing `render.rs` caret test passes unchanged

Verification: the standard ritual.

Prohibitions: do NOT change `mouse_down` / `mouse_drag` behaviour or
any drawing; do NOT touch other files; do NOT proceed to 07.6.

Stop: report files changed and verification; await review.

### 07.6 -- App: open the menu on diagnostics, from mouse and keyboard

Scope: `frext-term/src/keymap.rs`, `frext-term/src/app.rs`.

What:

- `keymap.rs`: `Action::OpenEditorMenu`, documented, bound as decided
  under "The keyboard binding". The module doc mentions it.
- `app.rs`:
  - `open_editor_menu` takes the diagnostic to show and a
    `MenuAnchor`, and builds the menu with `ContextMenu::new` for `At`
    and `ContextMenu::below` for `Below`.
  - Right-click on `Target::Editor`: the diagnostic is
    `buffer.char_at_cell(tab, column, row)` then `diagnostic_at`,
    cloned; anchor `At(mouse cell)`. The caret still does not move.
  - `Action::OpenEditorMenu`: the diagnostic at the caret; anchor
    `Below(caret_cell)`, or `At` the top-left of `self.buffer.rect()`
    when `caret_cell` is `None`. Nothing new is stored on `TermApp`.
  - Choosing an item needs no new code: `menu_reply` sends the
    command.

Deliverable: tests (real diagnostics via the 07.3 helper):

- right-click on `tset` opens a menu whose entries are
  `editor_menu::entries(selection, Some(diagnostic))` at the mouse
  cell; right-click on a clean word opens today's menu
- choosing the first suggestion replaces `tset` in the text, and
  Ctrl+Z restores it
- "Add to dictionary" removes the squiggle on the next draw and adds
  the word to `user_words`; "Ignore" removes it too
- with the caret inside `tset`, Shift+F10, `F(22)` and `Menu` each
  open the diagnostic menu `below` the caret cell; with the caret on
  a clean word they open today's menu below the caret
- with the caret scrolled off screen the keyboard menu opens at the
  editor's top-left
- Shift+F10 with the sidebar focused still opens the tree menu
  (regression)
- `keymap` unit tests for the bindings, including that Ctrl+Shift+F10
  and a focused sidebar or find bar get no `OpenEditorMenu`

Verification: the standard ritual.

Prohibitions: do NOT touch files outside scope; do NOT move the caret
on right-click; do NOT proceed further.

Stop: report files changed and verification; await review.

### 07.7 -- Cleanup (deferred): keyboard menu after an unrendered edit

Surface point: 07.6 (`8090026`), reported by its implementer.

Impact: `BufferView::caret_cell` returns `None` when the tab's stored
layout is stale (its text changed since the last render). If an edit
and Shift+F10 / Menu arrive in the same input batch, there has been no
render in between, so the keyboard menu opens at the editor's top-left
instead of below the caret. The diagnostic is still found correctly
(`diagnostic_at` reads the live text). A render follows every batch,
so this needs typing and the menu key within one read.

Scope of fix: `frext-term/src/buffer/mod.rs` (and a test in
`frext-term/src/app.rs`).

Suggested approach: give `BufferView` a `&mut self` variant that runs
`prepare` (re-layout and keep the caret visible, as `render` does)
before computing the cell, and call it from
`TermApp::open_editor_menu_at_caret`.

Verification: an app test that types a char and presses Shift+F10 in
one batch, with no draw in between, gets `ContextMenu::below` at the
caret's cell; the standard ritual.

Scheduling: independent of everything else in this plan; may land
after 07 merges, under a later plan's cleanup.

## Manual smoke checklist

Run in a real terminal (kitty or freminal, then a terminal without
undercurl, then tmux):

1. Open a `.md` file with a misspelling and a grammar slip. Red and
   green squiggles appear within a second, without pressing a key.
2. In kitty/freminal they are curly; with `--undercurl off`, or in a
   terminal without `Smulx`, they are straight and still coloured.
3. A squiggle crossing a soft wrap shows on both rows; selecting it or
   matching it in the find bar keeps both background and squiggle.
4. Moving the caret into a flagged word shows Harper's message in the
   status line.
5. Right-click on it: message, suggestions, Add to dictionary,
   Ignore, then the usual items. Each works; a suggestion is undone by
   Ctrl+Z.
6. Shift+F10 / Menu with the caret in a flagged word opens the same
   menu below the caret, and above it near the bottom of the screen.
7. A word added in the TUI is not flagged in the GUI after a restart
   (shared dictionary).
