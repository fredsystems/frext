# 03 -- TUI buffer view (editor widget)

- **Status:** active
- **Depends on:** 02
- **Branch:** `feat/frext-term-buffer-view` (one branch, one commit per
  subtask, one PR)

## Goal

A custom ratatui editing widget for the active tab, matching the GUI
editor: soft wrap, a line-number gutter that numbers only the first
visual row of each logical line, syntax highlighting, selection, mouse
and keyboard editing, undo/redo, clipboard integration, and the editor
right-click menu. Search-match backgrounds and proofreading underlines
are rendered by later plans through the same per-cell styling pass.

## Task summary

- Text model: the buffer stays `Tab::text: String`, edited through
  `Editor::active_view_mut` and `commit_edit`, exactly as the GUI does.
  The widget owns only view state: caret and anchor (char indices),
  preferred column for vertical motion, scroll offset, and undo
  history.
- Layout: visual rows computed from the buffer and the editor width:
  grapheme clusters (`unicode-segmentation`), display width
  (`unicode-width`), tab expansion, wrapping at whitespace with a
  fallback to grapheme boundaries. The gutter numbers the first row of
  each logical line, which is what
  `frext_core::lines::line_numbers_for_rows` computes.
- Rendering writes straight into the ratatui `Buffer` cell by cell (not
  through `Paragraph`), merging per cell: the syntax span fg/italic,
  the selection background, and (plans 04/07) the match background and
  diagnostic underline. Selection and match colours come from
  `palette::Tint::over(BASE)`.
- Editing: insert, delete, Backspace, Enter, Tab, word and line
  motions, Home/End, PageUp/PageDown, Ctrl+Home/End, Shift-extended
  selections, Ctrl+A, mouse click / double-click (word) / triple-click
  (line) / drag to select, wheel scroll.
- Undo/redo: an `EditHistory` with coalesced typing runs; Ctrl+Z,
  Ctrl+Shift+Z and Ctrl+Y.
- Core commands: Cut / Copy / Paste / SelectAll go through
  `Editor::apply` with `set_selection`, and the widget applies the
  returned `Effect`s (caret, reveal).
- Scroll-to-caret, and `Effect::RevealSelection` centring.
- The editor context menu (right-click), with a reusable menu widget.

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

## Decided at activation

- **Incremental highlighter in `frext-core`**
  (`frext_core::highlight::incremental`), so the GUI can adopt it
  later. The GUI is not changed by this plan. A new `frext-core`
  criterion bench captures the cost before and after.
- **Tabs render as a fixed 4 cells** (no tab stops), matching egui,
  which advances a `'\t'` by 4 space widths wherever it is.
- **No whitespace markers**, as in the GUI.
- **Word boundaries are egui's** (`next_word_boundary_char_index`,
  ported): Unicode word bounds, a word char is alphanumeric or `_`,
  `.` is always a stop, whitespace runs are skipped. Double-click
  selects the word clipped to its line; triple-click selects the line
  without its line break. The **line** rule intentionally differs from
  egui's `select_line_at`, which over-selects on empty and one-char
  lines (an empty line selects the whole text): `line_at` is exactly
  the logical line's content, excluding its `'\n'` and a `'\r'` before
  it (decided during 03.4).
- **The wrap layout is cached** per tab, keyed on a hash of the text
  and the wrap width.
- **The editor context menu lands here**, with a reusable
  `ContextMenu` widget (plan 05's tree menu reuses it). Plan 07 puts
  its diagnostic items first, and owns the keyboard invocation
  (Shift+F10 / Menu).
- **Enter / Tab are exact GUI parity** (verified against egui 0.37.0's
  `code_editor()`):
  - Enter inserts `\n` with no auto-indent.
  - Tab inserts a literal `\t`, replacing any selection. There is no
    block indent.
  - Shift+Tab (`BackTab`) first deletes any selection, then removes
    one leading `\t`, or 4 leading spaces, from the caret's logical
    line (egui's `decrease_indentation`, including its quirk: if fewer
    than 4 chars remain on the line and they are all spaces, they are
    removed too; it never panics). The caret moves back by the removed
    count unless it was at the line start.

## Further decisions (orchestrator)

These come from reading egui 0.37.0 and the current code; each is GUI
parity unless it says otherwise.

- **Module layout.** The widget is `frext-term/src/buffer/`:

  ```text
  buffer/
    mod.rs       BufferView: per-tab view state, the app-facing API
    cells.rs     how one grapheme is drawn and how wide it is
    wrap.rs      WrapLayout: visual rows + caret <-> (row, column)
    words.rs     egui's word / line boundaries
    cursor.rs    Cursor (anchor, caret, preferred column) + motions
    history.rs   EditHistory: grouped, coalesced undo/redo
    edit.rs      EditAction and applying it to the text
    render.rs    the per-cell rendering pass
  menu.rs        ContextMenu widget (generic; top level, reused by 05)
  editor_menu.rs the editor's right-click menu entries
  ```

  `editor_view.rs` (plan 02's read-only view) is deleted when the app
  switches to `BufferView` (03.10).
- **Grapheme motion is over the whole text.** Left/Right, Backspace and
  Delete step by extended grapheme cluster of the full buffer
  (`unicode_segmentation::GraphemeCursor`), so a caret never rests
  inside `"\r\n"` or a ZWJ sequence. `Cursor::clamp` snaps a foreign
  position (from core, or after an outside edit) down to a boundary.
- **Cells.** A `'\t'` is 4 blank cells. A grapheme whose first char is
  a control char (other than `'\t'`) draws as `U+FFFD` in one cell. Any
  other grapheme is `UnicodeWidthStr::width`, with a minimum of 1. A
  logical line's drawn content excludes its `'\n'` and a `'\r'` right
  before it.
- **Wrap width is `editor.width - 1`** (minimum 1). The last column is
  always free, so a caret at the end of a full row is still on screen.
  Greedy wrap: a row breaks after the last whitespace grapheme that
  fits (the whitespace stays on the earlier row; U+00A0, U+2007 and
  U+202F are not break opportunities), otherwise before the grapheme
  that would overflow. A row always takes at least one grapheme.
- **Soft-wrap caret rule.** The char index at a soft-wrap boundary is
  the start of the next row. "End of a soft-wrapped row" (End, a click
  past the row's content, Up/Down onto it) is the boundary before that
  row's last grapheme. On the last row of a logical line it is the row
  end.
- **Vertical motion** is by visual row, keeping a preferred column in
  cells (egui keeps a pixel x). Up on the first row goes to 0, and Down
  on the last row goes to the end. PageUp/PageDown move by
  `max(height - 1, 1)` rows with the same rule. Any other motion or
  edit drops the preferred column.
- **Home/End go to the visual row**, with no smart home.
  Ctrl+Home/End and Ctrl+Up/Down go to the start or end of the buffer.
- **Plain Left/Right with a selection collapse it** to its start or
  end without moving. Word motions move from the caret.
- **Scroll position** is a char index: the start of the top visual
  row. It survives width changes and edits, and is clamped and snapped
  to a row start on every use. The caret is kept visible with minimal
  scrolling after every motion or edit. `RevealSelection` centres the
  caret row. The wheel scrolls 3 rows without moving the caret.
- **Undo groups.** An edit is `{ at, removed, inserted, before, after }`
  (chars, with cursors). A new edit joins the open group only if all
  of these hold:
  - it is the same kind as the last edit, and that kind is `Typing` or
    `Deleting`
  - it comes within 1 s of the previous edit
  - it is contiguous with the previous edit
  - no break has happened since (a caret move, a click, a core command,
    or a focus change)

  Otherwise it starts a new group. At most 100 groups are kept. A new
  edit clears redo. Undo restores the `before` cursor of the group's
  first edit, and redo restores the `after` cursor of the group's last
  edit. An undo or redo whose recorded text no longer matches the
  buffer clears the history and does nothing.
- **Outside edits.** A text-editing core command sent by the app
  (`Cut`, `Paste`, `ApplySuggestion`, `ReplaceCurrent`, `ReplaceAll`)
  is recorded as one group. The app snapshots the text before `apply`.
  If the outcome is `Edited`, `BufferView::record_outside_edit` stores
  the common-prefix/suffix difference. Any other change the view did
  not make or record (a disk reload) is detected by a text-hash
  mismatch. It clears that tab's history and clamps its cursor.
- **Selection to core.** `TermApp::send` first calls
  `editor.set_selection(..)` with the active tab's selection while
  focus is `Editor`, and with `None` otherwise (the GUI sends `None`
  when its editor is unfocused).
- **Effects apply at the end of every `handle`**, not just before the
  draw, so the next event in the same batch sees them (Ctrl+A then
  Backspace in one batch empties the buffer).
  - `SetSelection` sets the anchor to `chars.start` and the caret to
    `chars.end` on that tab. With `EditorFocus::Focus`, it also moves
    focus to the editor.
  - `RevealSelection` centres the caret row.
  - `FocusSearchField` stays a no-op until plan 04.
- **Bracketed paste outside a prompt** inserts at the caret as one
  undo group, with `\r\n` and lone `\r` turned into `\n`.
- **Mouse.**
  - A left press places the caret, and Shift+press extends from the
    anchor.
  - A second or third press at the same cell within 500 ms selects the
    word or the line. crossterm reports no click counts, so the view
    counts them.
  - A drag extends from the anchor by the press's granularity, keeping
    the original word or line selected (egui's
    `extend_granular_select`). A drag above or below the editor
    clamps to the edge row and scrolls one row per event.
  - A drag that started in the editor keeps routing to it wherever the
    pointer goes, until the button is released.
  - A right-click opens the editor menu without moving the caret (egui
    acts only on the primary button).
- **Keys.** While focus is `Editor` and no prompt or menu is open:

  | Key                                     | Action                          |
  | --------------------------------------- | ------------------------------- |
  | printable char (no Ctrl/Alt)            | `Edit(Type(c))`                 |
  | Enter                                   | `Edit(Newline)`                 |
  | Tab / BackTab                           | `Edit(Tab)` / `Edit(Outdent)`   |
  | Backspace / Delete                      | `Edit(Backspace)` / `Delete`    |
  | Ctrl or Alt + Backspace / Delete        | `WordBackspace` / `WordDelete`  |
  | Left / Right (+Ctrl: word)              | `Move(..)`                      |
  | Up / Down / Home / End / PageUp / PageDown | `Move(..)`                   |
  | Ctrl+Home / Ctrl+End / Ctrl+Up / Ctrl+Down | `Move(DocStart / DocEnd)`    |
  | any motion + Shift                      | the same `Move`, `Extend`       |
  | Ctrl+A / Ctrl+C / Ctrl+X / Ctrl+V       | `Command(SelectAll/Copy/Cut/Paste)` |
  | Ctrl+Z                                  | `Edit(Undo)`                    |
  | Ctrl+Shift+Z / Ctrl+Y                   | `Edit(Redo)`                    |
  | Ctrl+H (`Legacy` only)                  | `Edit(Backspace)`               |

  The Ctrl+letter editing bindings apply only while focus is `Editor`.
  Ctrl+W stays "close tab" and Ctrl+H (`Enhanced`) stays "replace", as
  in the GUI. egui's Ctrl+K / Ctrl+U paragraph deletes are not bound.
  `Action::Scroll` and `ScrollBy` are removed: the arrows move the
  caret now.
- **Terminal cursor.** The caret is the terminal's own cursor
  (`Frame::set_cursor_position`), shown only while focus is `Editor`
  with no prompt or menu open. The cursor shape is left alone.
- **Selected line breaks** paint one selection-coloured cell after the
  row's content. Selection wins over a match background.
- **The editor menu** matches the GUI's `editor_context_menu`, in this
  order: `Cut` and `Copy` (disabled without a selection), `Paste`,
  `Select all`, a separator, `Find…` and `Find and replace…`, a
  separator, `Save`. It opens at the mouse cell and flips left or up
  to stay on screen. While it is open it captures the keyboard: Up and
  Down move over the enabled items, Enter chooses, Esc dismisses. A
  click outside dismisses it, and so does any core command that
  changes the layout.
- **Not in this plan:** match backgrounds fed from `match_highlights`
  (plan 04; the render pass already maps `MatchBackground` roles),
  diagnostic underlines and menu items (07), a keyboard way to open
  the menu (07), horizontal scrolling (the view always wraps), a
  current-line highlight, and docs (plan 09).

## Execution model

All subtasks run **strictly in sequence** on
`feat/frext-term-buffer-view`. Each subtask is one commit, leaves the
suite green (`commit-discipline`), and gets a full diff review by the
orchestrator before it is committed. Implementers do not commit, merge
or push.

Each subtask's verification includes the standard ritual, run inside
`nix develop`:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
cargo machete
```

All code follows the workspace rules: no `unwrap` / `expect` outside
tests, no `unsafe`, no indexing that can panic (use `get`, or ranges
proven in bounds), every public item tested, and no new dependency
beyond those named here. `criterion` is already a workspace dependency
(0.8.2).

Benchmarks (`performance-benchmarks`): 03.1 records the `frext-core`
baseline before the incremental highlighter. 03.2 reports the
before/after table. 03.3 and 03.9 add `frext-term` benches with their
code, and later subtasks that touch those paths re-run them and report
the deltas (a regression over 15% must be justified or fixed).

## Subtasks

### 03.1 -- Core highlight benchmark (baseline)

Scope: `frext-core/Cargo.toml`, `frext-core/benches/highlight.rs`
(new), `Cargo.lock`.

What: add `criterion = { workspace = true }` under
`[dev-dependencies]` and a `[[bench]] name = "highlight" harness =
false` to `frext-core`. The bench reuses the 50-line Rust `TEMPLATE`
and `REPEATS = 100` from `frext/benches/highlight.rs` (copied
verbatim, so both benches measure the same 5,000-line source). It
benchmarks group `"core_highlight"`:

- `full_rust_5k_lines`: `highlight(&source, "rs")`
- `full_plain_5k_lines`: `highlight(&source, "")`

Deliverable: the bench, compiling and running. Capture the baseline
with `cargo bench -p frext-core --bench highlight -- --save-baseline
before` and report the numbers.

Verification: the standard ritual, plus `cargo bench -p frext-core
--bench highlight --no-run`.

Prohibitions: do NOT change any `src/` file; do NOT touch the GUI
bench; do NOT proceed to 03.2.

Stop: report files changed, the baseline numbers and verification;
await review.

### 03.2 -- Incremental highlighter

Scope: `frext-core/src/highlight.rs` -> `frext-core/src/highlight/mod.rs`
(`git mv`, content unchanged except the `mod incremental;` /
`pub use` lines), `frext-core/src/highlight/incremental.rs` (new),
`frext-core/benches/highlight.rs`.

What:

- `pub mod incremental;` in `highlight/mod.rs`. Export only the type:
  callers write `frext_core::highlight::incremental::IncrementalHighlighter`.
- `pub const CHECKPOINT_LINES: usize = 64;`
- `pub struct IncrementalHighlighter`, with private fields:
  - `language: String`
  - `text: String`, a copy of the text last highlighted
  - `line_starts: Vec<usize>`, the byte start of each line of `text`
  - `checkpoints: Vec<Checkpoint>`, where `checkpoints[k]` is the
    state before line `k * CHECKPOINT_LINES`, kept as a valid prefix

  A syntect `Highlighter::new(theme())` is built per call (it is cheap
  and borrows the static theme). A private `struct Checkpoint { parse: ParseState, highlight:
  HighlightState }`.
- `#[must_use] pub fn new() -> Self` (and `Default`).
- `pub fn spans(&mut self, text: &str, language: &str, lines:
  Range<usize>) -> Vec<StyledSpan>`:
  1. If `language` differs from the stored one, reset everything.
  2. If `text` differs from the stored copy, find the first differing
     byte (common-prefix scan), map it to its line in the **old**
     `line_starts`, drop every checkpoint after that line, store the
     new text and recompute `line_starts`.
  3. Clamp `lines` to the line count (line = a `split_inclusive('\n')`
     piece; an empty text has no lines and returns `vec![]`).
  4. With no syntax for `language`, return one span covering the bytes
     of the clamped lines with `highlight`'s plain fallback style (make
     `PLAIN_FG` / the fallback span constructor `pub(crate)` in
     `mod.rs` for this).
  5. Otherwise resume from the last checkpoint at or before
     `lines.start` (the initial state when there is none:
     `ParseState::new(syntax)`, `HighlightState::new(&highlighter,
     ScopeStack::new())`). Walk lines up to `lines.end`, pushing a
     checkpoint whenever the next line index is
     `checkpoints.len() * CHECKPOINT_LINES`. For each line in the
     range, run `parse_line` and `RangedHighlightIterator`, and emit
     spans with absolute byte ranges and the same style mapping as
     `highlight_impl`.
  6. On a parse error, drop the checkpoints from the failing line on
     and return the plain fallback span over the requested lines.

  The returned spans tile exactly the bytes of the clamped lines.
- Doc comments on the module and every public item.

Deliverable: tests in `incremental.rs`, over the bench's template
(make a small `fn source(lines)` helper in the test module):

- `matches_full_highlight_on_every_window`: for windows 0..50,
  1000..1050 and the last 50 lines of a 5,000-line source, the spans
  equal the `highlight(text, "rs")` spans whose bytes lie inside those
  lines (syntect spans never cross a line, so this is an exact
  comparison), in that order and requested in that order
- `edit_invalidates_from_the_changed_line`: highlight line 3000,
  insert `/*` at line 10, and the spans for line 3000 now equal the
  full highlight of the edited text (everything is a comment)
- `edit_after_the_window_keeps_earlier_checkpoints`: the private
  checkpoint count is unchanged for checkpoints before an edit at line
  4000
- `language_change_resets`
- `unknown_language_is_one_plain_span`
- `empty_text_has_no_spans`
- `window_past_the_end_is_clamped`
- `crlf_text_matches_full_highlight`

Bench additions to group `"core_highlight"`:

- `incremental_viewport_after_edit_rust_5k_lines`: per iteration,
  insert then remove one char at line 2,500 and request lines
  2,480..2,530 (a warm highlighter, as the TUI uses it per keystroke)
- `incremental_cold_viewport_rust_5k_lines`: a fresh highlighter,
  lines 4,950..5,000

Report a table comparing `full_rust_5k_lines` (03.1 baseline, the cost
the TUI would pay per keystroke without this subtask) to the
incremental rows, and confirm that `full_*` did not change
(`--baseline before`).

Verification: the standard ritual, plus `cargo bench -p frext-core
--bench highlight -- --baseline before`.

Prohibitions: do NOT change `highlight()` or `overlay_matches`
behaviour; do NOT touch the GUI; do NOT add a dependency; do NOT
proceed to 03.3.

Stop: report and await review.

### 03.3 -- Cell rules and wrap layout

Scope: `frext-term/src/buffer/{mod,cells,wrap}.rs` (new),
`frext-term/src/lib.rs`, `frext-term/Cargo.toml`,
`frext-term/benches/wrap.rs` (new), `Cargo.lock`.

What:

- `lib.rs`: `pub mod buffer;`. `buffer/mod.rs`: a module doc and
  `pub mod cells; pub mod wrap;` (later subtasks add the rest).
- `cells.rs`:
  - `pub const TAB_WIDTH: u16 = 4;`
  - `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum Glyph<'a> {
    Text(&'a str), Blank(u16), Replacement }`
  - `#[must_use] pub fn glyph(grapheme: &str) -> Glyph<'_>`: `"\t"`
    gives `Blank(TAB_WIDTH)`, a grapheme whose first char is a control
    char gives `Replacement`, anything else gives `Text`
  - `#[must_use] pub fn width(grapheme: &str) -> u16`: `Blank(n)` is
    `n`, `Replacement` is 1, and `Text` is `UnicodeWidthStr::width`,
    at least 1, saturating into `u16`
  - `pub const REPLACEMENT: &str = "\u{FFFD}";`
- `wrap.rs`:
  - `#[derive(Debug, Clone, PartialEq, Eq)] pub struct VisualRow {
    pub chars: Range<usize>, pub bytes: Range<usize>, pub line: usize,
    pub break_after: RowBreak }` and `#[derive(Debug, Clone, Copy,
    PartialEq, Eq)] pub enum RowBreak { Newline, Wrap, EndOfText }`.
    `chars` and `bytes` cover the drawn content (no `'\n'`, no `'\r'`
    before it).
  - `#[derive(Debug, Clone, PartialEq, Eq)] pub struct WrapLayout {
    rows: Vec<VisualRow>, width: u16 }`
  - `#[must_use] pub fn new(text: &str, width: u16) -> Self`, using the
    wrap rule under "Further decisions" (`width` 0 is treated as 1).
    An empty text, or a text ending in `'\n'`, has a final empty row.
  - `pub fn rows(&self) -> &[VisualRow]`, `pub const fn width(&self)
    -> u16`
  - `#[must_use] pub fn row_of_char(&self, char_idx: usize) -> usize`:
    the last row whose `chars.start <= char_idx`, which gives the
    soft-wrap caret rule
  - `#[must_use] pub fn column_of_char(&self, text: &str, char_idx:
    usize) -> u16`: cells from that row's start to `char_idx`
  - `#[must_use] pub fn char_at(&self, text: &str, row: usize, column:
    u16) -> usize`: the caret position before the grapheme whose cells
    contain `column`, or the row end (soft-wrap rule) past the
    content. A row past the end is the last row.
  - `#[must_use] pub fn row_end(&self, text: &str, row: usize) ->
    usize`: the soft-wrap "end of row" rule, shared with `char_at` and
    the End key
- `frext-term/Cargo.toml`: `criterion = { workspace = true }` under
  `[dev-dependencies]` and `[[bench]] name = "wrap" harness = false`.
  `benches/wrap.rs` copies the template (5,000 lines) and benchmarks
  group `"wrap"`: `wrap_rust_5k_lines_80` (width 79) and
  `wrap_rust_5k_lines_40` (width 39).

Deliverable: tests:

- `cells`: tab, control char, CJK width 2, a ZWJ emoji, a combining
  mark alone (width at least 1), plain ASCII
- `wrap`:
  - a short line is one row
  - an empty text is one empty row, and `"a\n"` is two rows
  - a break after a space keeps the space on the first row
  - a long word with no spaces breaks at the width
  - a wide char that does not fit moves to the next row
  - a tab counts as 4 cells
  - CRLF: the `'\r'` is not in the row content
  - NBSP is not a break opportunity
  - every row is within the width (property over the template at
    widths 1, 7, 39, 79)
  - the first-row-of-line numbering agrees with
    `frext_core::lines::line_numbers_for_rows` over `break_after ==
    Newline`
  - `row_of_char` at a soft-wrap boundary is the next row
  - `column_of_char` and `char_at` round-trip on every grapheme
    boundary of a mixed-width text
  - `char_at` past the content (hard and soft rows)
  - width 0 does not panic

Verification: the standard ritual, plus `cargo bench -p frext-term
--bench wrap` (report the numbers).

Prohibitions: do NOT render anything; do NOT touch `text_input.rs` or
`editor_view.rs`; do NOT proceed to 03.4.

Stop: report and await review.

### 03.4 -- Word and line boundaries

Scope: `frext-term/src/buffer/words.rs` (new),
`frext-term/src/buffer/mod.rs`.

What: a faithful, panic-free port of egui 0.37.0's
`text_cursor_state.rs` rules, over char indices:

- `#[must_use] pub fn is_word_char(c: char) -> bool`
- `#[must_use] pub fn next_word_boundary(text: &str, char_idx: usize)
  -> usize` (egui's `next_word_boundary_char_index`)
- `#[must_use] pub fn previous_word_boundary(text: &str, char_idx:
  usize) -> usize` (egui's `ccursor_previous_word`: run the forward
  rule over the grapheme-reversed text)
- `#[must_use] pub fn word_at(text: &str, char_idx: usize) ->
  Range<usize>` (egui's `select_word_at`, clipped to the line, sorted)
- `#[must_use] pub fn line_at(text: &str, char_idx: usize) ->
  Range<usize>` (egui's `select_line_at`, sorted)

egui's `slice_char_range` asserts; the port must not. Use
`frext_core::offsets::byte_of_char` for conversions.

Deliverable: egui's own test vectors, ported verbatim
(`"abc d3f g_h i-j"`, `"abc.def.ghi"`, the emoji/Czech line, `"abc def
ghi"` forward and backward, the empty string, `"hello world"` for
`word_at`), plus `line_at` on a three-line text (each line without its
`'\n'`), at index 0, at the end, and on an empty line.

Verification: the standard ritual.

Prohibitions: do NOT invent different word rules; do NOT proceed to
03.5.

Stop: report and await review.

### 03.5 -- Cursor and motions

Scope: `frext-term/src/buffer/cursor.rs` (new),
`frext-term/src/buffer/mod.rs`.

What:

- `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum Motion {
  Left, Right, WordLeft, WordRight, Up, Down, RowStart, RowEnd,
  PageUp, PageDown, DocStart, DocEnd }`
- `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum Selecting {
  Move, Extend }`
- `#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)] pub struct
  Cursor { anchor: usize, caret: usize, preferred_column: Option<u16> }`
  with:
  - `pub const fn caret(&self)`, `pub const fn anchor(&self)`
  - `pub fn selection(&self) -> Range<usize>` (ordered)
  - `pub const fn has_selection(&self) -> bool`
  - `pub const fn at(char_idx) -> Self`
  - `pub const fn spanning(anchor, caret) -> Self`
  - `pub fn clamp(&mut self, text: &str)`: clamp both ends to the char
    count and snap them down to full-text grapheme boundaries
  - `pub fn apply(&mut self, text: &str, layout: &WrapLayout, motion:
    Motion, selecting: Selecting, page_rows: u16)`
- Grapheme helpers in this file (private, or `pub(crate)` for 03.7):
  `next_grapheme(text, char_idx) -> usize` and `previous_grapheme(..)`
  over the full text with `GraphemeCursor`, and `snap_down(..)`.
- Motion semantics exactly as in "Further decisions" (collapse on
  plain Left/Right with a selection, word motion from the caret,
  vertical motion with the preferred column and the first/last-row
  rule, PageUp/PageDown by `max(page_rows - 1, 1)`, Home/End on the
  visual row via `WrapLayout::row_end`, DocStart/DocEnd). `Move` sets
  the anchor to the caret. `Extend` keeps it. Only vertical motions
  keep `preferred_column`.

Deliverable: one test per motion (with and without `Extend`), plus:

- Left/Right step over `"\r\n"`, `é` written as `e` + combining acute,
  and a ZWJ emoji as one unit
- Up/Down keep the preferred column across a shorter row, and drop it
  after a Left
- Down from the last row goes to the end, and Up from the first row
  goes to 0
- End on a soft-wrapped row, and Down onto a soft-wrapped row
- `clamp` after the text shrinks, and inside `"\r\n"`

Verification: the standard ritual.

Prohibitions: do NOT edit text; do NOT proceed to 03.6.

Stop: report and await review.

### 03.6 -- Edit history

Scope: `frext-term/src/buffer/history.rs` (new),
`frext-term/src/buffer/mod.rs`.

What:

- `#[derive(Debug, Clone, PartialEq, Eq)] pub struct Edit { pub at:
  usize, pub removed: String, pub inserted: String, pub before:
  Cursor, pub after: Cursor }` (char index `at`)
- `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum EditKind {
  Typing, Deleting, Other }`
- `pub const MAX_GROUPS: usize = 100;` and `pub const COALESCE: Duration
  = Duration::from_secs(1);`
- `#[derive(Debug, Default)] pub struct EditHistory`, private fields:
  `undo: Vec<Vec<Edit>>`, `redo: Vec<Vec<Edit>>`, and `open:
  Option<(EditKind, Instant)>` for the group that may still coalesce.
- Methods:
  - `pub fn record(&mut self, edit: Edit, kind: EditKind, now: Instant)`,
    with the coalescing rule under "Further decisions". Contiguity:
    `Typing` joins when `edit.at` equals the previous edit's
    `at + inserted.chars().count()`. `Deleting` joins when it is a
    Backspace run (`edit.at + removed.chars().count()` equals the
    previous `at`) or a Delete run (`edit.at` equals the previous
    `at`), with `inserted` empty in both.
  - `pub fn record_group(&mut self, edits: Vec<Edit>)`: closes the open
    group and pushes `edits` as one new, non-coalescing group (an empty
    `edits` does nothing). It is used by Outdent, paste and outside
    edits.
  - `pub fn break_group(&mut self)`
  - `pub fn undo(&mut self, text: &mut String) -> Option<Cursor>`
  - `pub fn redo(&mut self, text: &mut String) -> Option<Cursor>`
  - `pub fn clear(&mut self)`
  - `pub fn can_undo(&self) -> bool` and `pub fn can_redo(&self) ->
    bool`

  Undo applies the group's edits in reverse: it checks that the text
  at `at` is `inserted` (otherwise it clears and returns `None`), then
  replaces it with `removed`. Redo applies them forward with the
  mirror check. Use `frext_core::edit_ops::replace_range` and
  `selected_text`.

Deliverable: tests:

- typing `abc` within 1 s is one group, and undo restores the empty
  text and the cursor
- a pause of more than 1 s splits the group
- `break_group` splits it
- `record_group` of two edits undoes in one step, and the next typed
  char does not join it
- a Backspace run coalesces and a Delete run coalesces
- typing then deleting are two groups
- a non-contiguous insert splits
- redo after undo, and a new edit clears redo
- the 101st group drops the oldest
- an undo after an outside change is refused and clears the history
- multi-byte text (`日本`) round-trips

Verification: the standard ritual.

Prohibitions: do NOT touch `Tab` or core; do NOT proceed to 03.7.

Stop: report and await review.

### 03.7 -- Edit actions

Scope: `frext-term/src/buffer/edit.rs` (new),
`frext-term/src/buffer/mod.rs`, `frext-term/src/buffer/cursor.rs`
(only to make the grapheme helpers `pub(crate)` if they are not yet).

What:

- `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum EditAction {
  Move(Motion, Selecting), Type(char), Newline, Tab, Outdent,
  Backspace, Delete, WordBackspace, WordDelete, Undo, Redo }`
- `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum Applied {
  Unchanged, Moved, Edited }`
- `pub struct EditTarget<'a> { pub text: &'a mut String, pub cursor:
  &'a mut Cursor, pub history: &'a mut EditHistory }`
- `pub fn apply(target: EditTarget<'_>, layout: &WrapLayout, action:
  EditAction, page_rows: u16, now: Instant) -> Applied`:
  - `Move`: `Cursor::apply` and `history.break_group()`. The result is
    `Moved` if the cursor changed, else `Unchanged`.
  - `Type(c)` (a control char gives `Unchanged`), `Tab` (`"\t"`) and
    `Newline` (`"\n"`) replace the selection. The kind is `Typing`
    for `Type`/`Tab` with no selection, and `Other` for `Newline` or
    any replacement of a selection.
  - `Backspace` / `Delete` delete the selection (`Other`), or else the
    previous / next grapheme (`Deleting`). `WordBackspace` /
    `WordDelete` delete the selection, or else up to
    `previous_word_boundary` / `next_word_boundary` (`Other`). At a
    buffer edge with nothing to delete, the result is `Unchanged`.
  - `Outdent`: the egui sequence under "Decided at activation": the
    selection delete (if any), then the indent removal (if any),
    recorded together through `history.record_group`. `Unchanged` if
    neither happened.
  - `Undo` / `Redo`: restore the returned cursor (clamped). `Edited`
    if anything changed.
  - Every edit drops the preferred column and records an `Edit` with
    the before and after cursors.
- `pub fn insert_text(target: EditTarget<'_>, text: &str, now: Instant)
  -> Applied`: the paste path. It normalises `\r\n` and lone `\r` to
  `\n`, replaces the selection, and records it with
  `history.record_group` (an empty `text` with no selection is
  `Unchanged`).

Deliverable: one test per action, plus:

- typing over a selection, then undo restores the selection text and
  the selection
- Backspace over a ZWJ emoji removes it whole
- WordBackspace in `"foo.bar baz"` from the end removes `baz`
- Outdent: a `\t` line, a 4-space line, a 2-space line (the egui
  quirk: removed), a line starting `x` (nothing), and the caret at the
  line start
- Tab with a selection replaces it
- `insert_text` CRLF normalisation and a single undo
- Redo after Undo

Verification: the standard ritual.

Prohibitions: do NOT talk to `Editor` or core commands; do NOT add
key handling; do NOT proceed to 03.8.

Stop: report and await review.

### 03.8 -- Rendering pass

Scope: `frext-term/src/buffer/render.rs` (new),
`frext-term/src/buffer/mod.rs`, `frext-term/src/theme.rs`.

What:

- `theme.rs` gains:
  - `#[must_use] pub fn selection_bg() -> Color`:
    `palette::selection().over(BASE)`
  - `#[must_use] pub fn match_bg(role: MatchBackground) -> Color`:
    `Other` is `palette::search_match()`, `Current` is
    `palette::search_current_match().over(BASE)`
  - `#[must_use] pub fn span(span: &StyledSpan) -> Style`: fg
    `color(span.fg)`, bg `BASE`, plus `ITALIC` / `UNDERLINED` from the
    flags
  - `#[must_use] pub const fn replacement() -> Style`: fg
    `palette::gutter()`, bg `BASE`
- `render.rs`:
  - `pub struct RowsView<'a> { pub text: &'a str, pub layout: &'a
    WrapLayout, pub top_row: usize, pub spans: &'a [StyledSpan], pub
    selection: Range<usize>, pub caret: usize }`
  - `pub fn render(view: &RowsView<'_>, gutter: Rect, editor: Rect,
    buf: &mut Buffer) -> Option<Position>`:
    1. Fill `gutter` with `theme::gutter()` and `editor` with
       `theme::base()`.
    2. For each visible row (`top_row..top_row + editor.height`):
       - on the first row of its logical line, the 1-based line
         number right-aligned in `gutter` with one trailing space
         column (skipped when too wide, as today)
       - each grapheme of the row drawn at its column through
         `buf.set_stringn` (so wide chars are handled), styled by the
         span covering its first byte (`theme::span`, walking the
         sorted spans with a moving index, `theme::base()` where no
         span covers it), then the background: the selection if its
         char index is in `selection`, else its span's match role,
         else `BASE`
       - `Blank(n)` fills `n` cells, `Replacement` draws
         `cells::REPLACEMENT` in `theme::replacement()` plus the
         background
       - if the row ends in `Newline` and the newline's char index is
         in `selection`, one selection cell after the content
    3. Return the caret's screen position when its row is visible.
  - Nothing is drawn outside `gutter` and `editor`.

Deliverable: `TestBackend` / `Buffer` tests:

- the gutter numbers only the first row of a wrapped line
- the syntax colours of a small Rust snippet (a keyword cell has the
  keyword fg from `highlight`)
- the selection bg on selected cells only, including the newline cell
- selection over a match keeps the selection colour, and `match_bg`
  for both roles
- a tab is 4 cells, a control char is the replacement glyph, and a
  wide char takes 2 cells
- the caret position at the start, mid-row, the end of a wrapped row,
  and after the last line; `None` when scrolled away
- a `top_row` past the end does not panic
- the `theme` helpers' colours

Verification: the standard ritual.

Prohibitions: do NOT add state; do NOT compute layout or highlighting
here (take them as input); do NOT proceed to 03.9.

Stop: report and await review.

### 03.9 -- `BufferView`

Scope: `frext-term/src/buffer/mod.rs`, `frext-term/Cargo.toml` (one
`[[bench]]` entry), `frext-term/benches/buffer.rs` (new).

What: `#[derive(Default)] pub struct BufferView`:

- Fields: `tabs: HashMap<u64, TabView>`, `rect: Rect` (the last editor
  rect), `press: Option<Press>`, `drag: Option<Drag>`.
- Private `struct TabView { cursor: Cursor, history: EditHistory,
  highlighter: IncrementalHighlighter, layout: Option<(u64, WrapLayout)>
  (text hash, layout), top: usize (char index), known: u64 (text hash
  after the view's last change) }`.
- Private `enum Granularity { Char, Word, Line }`, `struct Press {
  column: u16, row: u16, at: Instant, granularity: Granularity }` and
  `struct Drag { granularity: Granularity, origin: Range<usize> }`.
- `pub const DOUBLE_CLICK: Duration = Duration::from_millis(500);` and
  `pub const WHEEL_ROWS: i32 = 3;`
- A private `fn prepare(&mut self, tab: &Tab) -> &mut TabView` that
  creates the state, detects outside changes by text hash (clamp the
  cursor, clear the history, update `known`), and refreshes the
  layout for `wrap width = rect.width - 1`.
- Public methods (all take the active `Tab`; `now` is injected):
  - `pub fn handle(&mut self, tab: &mut Tab, action: EditAction, now:
    Instant) -> Applied`: `edit::apply` with `page_rows = rect.height`,
    then keep the caret visible.
  - `pub fn insert_text(&mut self, tab: &mut Tab, text: &str, now:
    Instant) -> Applied`
  - `pub fn record_outside_edit(&mut self, tab: &Tab, before: &str,
    now: Instant)`: the common-prefix/suffix difference against
    `tab.text`, recorded with `history.record_group`. The `before`
    cursor is the view's cursor, and `after` is a caret at the end of
    the inserted text. The real cursor is left to the effects. Update
    `known`.
  - `pub fn selection(&self, tab_id: u64) -> Option<Range<usize>>`
  - `pub fn set_selection(&mut self, tab: &Tab, chars: Range<usize>)`:
    anchor at `chars.start`, caret at `chars.end`, clamped, then
    `break_group`
  - `pub fn reveal(&mut self, tab: &Tab)`: centre the caret row
  - `pub fn wheel(&mut self, tab: &Tab, rows: i32)`
  - `pub fn mouse_down(&mut self, tab: &Tab, column: u16, row: u16,
    selecting: Selecting, now: Instant)`, `pub fn mouse_drag(&mut
    self, tab: &Tab, column: u16, row: u16)`, `pub fn mouse_up(&mut
    self)`, and `pub const fn is_dragging(&self) -> bool`. Absolute
    screen coordinates are mapped through `rect` and `top`. The
    click-count and drag rules are under "Further decisions".
  - `pub fn forget(&mut self, live_ids: impl IntoIterator<Item = u64>)`
  - `pub fn render(&mut self, tab: Option<&Tab>, gutter: Rect, editor:
    Rect, buf: &mut Buffer, hits: &mut HitMap) -> Option<Position>`:
    store `rect`, `prepare`, clamp `top`, ask the highlighter for the
    visible logical lines (`language_from_path(tab.path)`), call
    `render::render`, and push `Target::Editor`. With no tab, fill
    both rects with `BASE` and return `None`.
- Text hash: `std::hash::DefaultHasher` over the `str`.
- `benches/buffer.rs`, group `"buffer"`: `type_and_render_rust_5k_lines`
  types one char at line 2,500 and renders an 80x24 `Buffer` per
  iteration (a warm view), and `render_only_rust_5k_lines` renders
  with no change.

Deliverable: tests over a real `Tab` (`Tab::new_untitled` + text):

- typing and rendering show the text and the caret
- the caret is kept visible when typing past the bottom
- `reveal` centres
- the wheel scrolls without moving the caret
- a resize (a different `editor` rect) keeps the top char
- an outside text change clears undo, and an undo after it does
  nothing
- `record_outside_edit` makes the change undoable as one step
- single, double (word) and triple (line) clicks, Shift+click extend,
  and a press more than 500 ms later counts as single
- drag by char, and by word after a double-click (the original word
  stays selected)
- a drag below the editor scrolls
- `set_selection` clamps a range past the end
- `forget` drops closed tabs
- with no tab, it renders `BASE`

Verification: the standard ritual, plus `cargo bench -p frext-term
--bench buffer` and `--bench wrap` (report the numbers).

Prohibitions: do NOT touch `app.rs`, `keymap.rs` or `editor_view.rs`;
do NOT proceed to 03.10.

Stop: report and await review.

### 03.10 -- App: keyboard editing

Scope: `frext-term/src/app.rs`, `frext-term/src/keymap.rs`,
`frext-term/src/lib.rs`, `frext-term/src/editor_view.rs` (deleted),
`frext-term/src/run.rs` (only if a test needs adjusting).

What:

- `keymap.rs`:
  - `Action` gains `Edit(EditAction)`. `Action::Scroll` and `ScrollBy`
    are removed.
  - The editing bindings in the table under "Further decisions",
    focus `Editor` only, using the existing `is_ctrl` /
    `ctrl_letter` helpers. Ctrl+Z needs the Shift bit (Ctrl+Shift+Z is
    Redo). In `Legacy` mode Ctrl+H is `Edit(Backspace)`. The module
    doc is updated (editing keys are bound now).
- `app.rs`:
  - `view: EditorView` becomes `buffer: BufferView`. `editor_view.rs`
    and its `lib.rs` entry are deleted.
  - `Action::Edit(a)`: through `editor.active_view_mut()`, call
    `buffer.handle(view.tab, a, now)`. If `Edited`, call
    `editor.commit_edit()`.
  - `send(command)`:
    1. `editor.set_selection(..)` per "Selection to core"
    2. for `Cut`, `Paste`, `ApplySuggestion { .. }`, `ReplaceCurrent`
       or `ReplaceAll`, snapshot the active tab's text
    3. apply
    4. if `Edited` and a snapshot was taken (and the active tab id is
       unchanged), `buffer.record_outside_edit`
    5. the existing queueing, `prompt.sync` and `forget_layout`
  - `handle` ends with `self.apply_effects()`. `apply_effects` handles
    `SetSelection` (the tab must exist; `Focus` moves focus to the
    editor) and `RevealSelection` (only for the active tab) through
    `BufferView`, then `forget` as today.
  - Paste outside a prompt goes to `buffer.insert_text`, then
    `commit_edit` if `Edited`. `PASTE_MESSAGE` is removed.
  - `render` uses `buffer.render(..)`. When it returns a caret
    position, focus is `Editor` and no prompt is open, the caret
    becomes the frame cursor. The prompt's cursor still wins when a
    prompt is open.
  - `last_editor_height` is removed if nothing uses it any more.
  - The existing scroll tests are rewritten for caret motion (arrows
    move the caret, and the view follows).

Deliverable: app tests (`TestBackend`, real `Editor`):

- typing `hi` shows it, the tab becomes dirty, and the swap file holds
  `hi` (`commit_edit` ran)
- Ctrl+A then Backspace in **one batch** (two `handle` calls with no
  render between) empties the buffer
- Ctrl+X then Ctrl+Z restores the text (outside edit recorded)
- Ctrl+C copies the selection (a `MemoryClipboard`)
- Ctrl+V pastes from it
- bracketed paste inserts at the caret
- Ctrl+Z / Ctrl+Shift+Z / Ctrl+Y
- the caret is the frame cursor in the editor, and not while a prompt
  is open or the sidebar has focus
- `SetSelection` with `Focus` moves focus from the sidebar
- `RevealSelection` centres the caret
- `Legacy` Ctrl+H deletes a char and `Enhanced` Ctrl+H opens search
- the editing Ctrl letters do nothing while the sidebar has focus
- `send` passes the selection to core (Ctrl+F with a one-line
  selection seeds the search pattern)

Keymap tests: one per new binding, `Shift` extending, the removed
scroll actions gone, and the sidebar-focus exclusions.

Verification: the standard ritual, plus `cargo bench -p frext-term
--bench buffer`.

Prohibitions: do NOT add mouse selection (03.11) or the menu (03.12+);
do NOT change core; do NOT proceed to 03.11.

Stop: report and await review.

### 03.11 -- App: mouse selection

Scope: `frext-term/src/app.rs`.

What: in `handle_mouse`, with no prompt open:

- `Down(Left)` on `Target::Editor` calls `buffer.mouse_down` (`Extend`
  when the event has `SHIFT`), focuses the editor, and starts a drag.
- `Drag(Left)` while `buffer.is_dragging()` calls `buffer.mouse_drag`,
  whatever the target. Hover is still updated.
- `Up(Left)` calls `buffer.mouse_up`.
- The wheel calls `buffer.wheel(±WHEEL_ROWS)`.
- After each, `editor.set_selection` is **not** called (send does it).

Deliverable: app tests:

- a click places the caret (frame cursor at the cell)
- a double-click selects a word, and Ctrl+C copies it
- a drag selects across two lines
- a drag that leaves the editor keeps selecting
- Shift+click extends
- the wheel scrolls without moving the caret
- a click while a prompt is open does not touch the buffer

Verification: the standard ritual.

Prohibitions: do NOT add the right-click menu; do NOT proceed to 03.12.

Stop: report and await review.

### 03.12 -- Context menu widget

Scope: `frext-term/src/menu.rs` (new), `frext-term/src/hit.rs`,
`frext-term/src/lib.rs`.

What:

- `hit.rs`: `Target` gains `MenuPopup` and `MenuItem(usize)` (hint
  `None`), with tests.
- `menu.rs`:
  - `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum Enablement
    { Enabled, Disabled }`
  - `#[derive(Debug, Clone, PartialEq, Eq)] pub enum MenuEntry<T> {
    Item { label: String, enabled: Enablement, value: T }, Separator }`
  - `#[derive(Debug, Clone, PartialEq, Eq)] pub enum MenuReply<T> {
    Chosen(T), Dismissed, Handled }`
  - `pub struct ContextMenu<T> { entries: Vec<MenuEntry<T>>, anchor:
    Position, highlighted: Option<usize> }` (`T: Clone`):
    - `pub fn new(entries, anchor: Position) -> Self` (nothing
      highlighted)
    - `pub fn handle_key(&mut self, key: KeyEvent) -> MenuReply<T>`:
      Up / Down move over enabled items (wrapping, starting from the
      first or last), Enter chooses the highlighted item (`Handled` if
      none), Esc dismisses, `Release` is `Handled`, and everything else
      is `Handled` (the menu is modal)
    - `pub fn hover(&mut self, index: Option<usize>)` (enabled items
      only)
    - `pub fn click(&mut self, target: Option<Target>) -> MenuReply<T>`:
      an enabled `MenuItem(i)` chooses it, a disabled item or a
      separator is `Handled`, `MenuPopup` is `Handled`, and anything
      else dismisses
    - `pub fn rect(&self, area: Rect) -> Rect`: the width is the
      longest label + 4 (a border and one space of padding each side),
      the height is entries + 2. It is placed at the anchor, moved
      left when it would cross the right edge, and opened upwards
      (ending at the anchor row) when it would cross the bottom, then
      clamped into `area`.
    - `pub fn render(&self, area: Rect, buf: &mut Buffer, hits: &mut
      HitMap)`: `Clear`, a bordered block in `theme::popup()` /
      `theme::popup_border()`, items in `theme::popup()`, disabled
      items with fg `OVERLAY0` (add `theme::disabled()`), the
      highlighted item in `theme::button_focused()`, separators as a
      `─` row joined to the border (`├…┤`). It pushes `MenuPopup` for
      the rect, then one `MenuItem(i)` per item row (`i` is the entry
      index).

Deliverable: tests:

- keyboard navigation skips separators and disabled items and wraps
- Enter with nothing highlighted does nothing
- Esc dismisses
- clicks on an item, a disabled item, the popup and outside
- `rect` at the bottom-right corner flips up and left, and a menu
  larger than the area is clamped without panicking
- a `TestBackend` render shows labels, the separator and the
  highlight
- the hit regions resolve to the right `MenuItem`

Verification: the standard ritual.

Prohibitions: do NOT wire it into the app (03.13); do NOT proceed to
03.13.

Stop: report and await review.

### 03.13 -- Editor context menu

Scope: `frext-term/src/editor_menu.rs` (new), `frext-term/src/app.rs`,
`frext-term/src/lib.rs`.

What:

- `editor_menu.rs`: `#[must_use] pub fn entries(has_selection:
  Enablement) -> Vec<MenuEntry<Command>>`, in the order under
  "Further decisions" (labels exactly `Cut`, `Copy`, `Paste`,
  `Select all`, `Find…`, `Find and replace…`, `Save`).
- `app.rs`:
  - `menu: Option<ContextMenu<Command>>`
  - `Down(Right)` on `Target::Editor` with no prompt open opens it at
    the mouse cell (`Enabled` iff the active tab has a selection)
  - while it is open, keys go to `handle_key`, and `Moved` / `Drag`
    over a `MenuItem(i)` calls `hover`
  - `Down(Left)` and `Down(Right)` go to `click`
  - `Chosen(c)` closes the menu and calls `send(c)`; `Dismissed`
    closes it
  - while the menu is open the terminal cursor is hidden, and
    `forget_layout` closes the menu
  - `render` draws it after the status line and before the prompt

Deliverable: tests for `entries` (both enablements, the exact order),
plus app tests:

- right-click opens the menu without moving the caret
- choosing Copy copies the selection
- choosing Cut with the keyboard (Down to Cut, Enter) cuts, and Ctrl+Z
  restores it
- `Find…` opens the search
- Esc and an outside click dismiss
- disabled Cut does nothing on click
- the menu is drawn on screen

Verification: the standard ritual, plus `cargo bench -p frext-term
--bench buffer`, and the manual smoke checklist below, run by the
orchestrator with the user.

Prohibitions: do NOT add diagnostic items or keyboard invocation (plan
07); do NOT proceed further.

Stop: report and await review.

## Manual smoke checklist

Run in a real terminal (kitty or freminal, then a legacy terminal):

1. Open a Rust file: it is syntax highlighted, long lines wrap, and
   wrapped rows have no line number.
2. Type, Backspace, Delete, Enter and Tab. Shift+Tab outdents. The tab
   turns dirty, and killing the process keeps the edit (the swap file).
3. Arrows, Ctrl+arrows, Home/End, PageUp/PageDown and Ctrl+Home/End
   move as in the GUI. Shift extends the selection.
4. Ctrl+Z, Ctrl+Shift+Z (kitty) and Ctrl+Y.
5. Ctrl+A, Ctrl+C, Ctrl+X and Ctrl+V. Bracketed paste
   (Ctrl+Shift+V) inserts multi-line text.
6. Click, double-click, triple-click, drag, drag past the bottom, and
   Shift+click. The wheel scrolls.
7. Right-click: the menu opens, flips near the screen edges, and Cut,
   Copy, Paste, Select all, Find…, and Save work.
8. Legacy terminal: Backspace deletes, and Ctrl+R still opens replace.
9. A 5,000-line file stays responsive while typing at the bottom.
