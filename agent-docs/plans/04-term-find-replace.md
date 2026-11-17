# 04 -- TUI find and replace

- **Status:** active
- **Depends on:** 03
- **Branch:** `feat/frext-term-find-replace` (one branch, one commit per
  subtask, one PR)

## Goal

The find/replace bar under the tab bar, matching the GUI: a find
field, toggles for match case / whole word / regex / in-selection,
Prev / Next, a match counter, a Replace toggle, an optional replace
row with Replace / Replace all, and an inline regex error. Matches are
highlighted in the buffer view, with the current match distinct.

## Task summary

- Render the bar from `Editor::search()`. Bind its two fields with
  plan 02's single-line input to `search_pattern_mut` and
  `search_replacement_mut`, and send `RecomputeMatches` when the
  pattern changes.
- The toggles (`[Aa] [W] [.*] [sel]`) can be clicked and also have
  key bindings.
- Enter / Shift+Enter in the find field and F3 / Shift+F3 anywhere send
  `StepMatch`. Esc sends `CloseSearch`.
- Ctrl+F / Ctrl+H send `OpenSearch`. The core already seeds the
  pattern from a single-line selection.
- Buffer view: `Editor::match_highlights()` gives the
  `MatchBackground::{Other, Current}` roles, which plan 03's rendering
  pass composes per cell.
- `Effect::FocusSearchField` moves keyboard focus to the find field.

## Durable decisions

- All search semantics already live in `frext-core` (plan 01). This plan
  is presentation and input only. It must not add search logic to
  `frext-term`.

## Decided at activation

- **Toggle keys are Alt+C / Alt+W / Alt+R / Alt+L** (match case, whole
  word, regex, in selection), the same keys VS Code uses. They work
  while the bar is open, whatever has focus. The modifiers must be
  exactly Alt, and the letter matches either case. Alt+letter is
  otherwise unbound in `frext-term`. Under legacy encoding a terminal
  sends Alt+c as `ESC c`, which crossterm reports as Alt+c. macOS
  terminals send these keys only when Option acts as Meta.
- **Tab / Shift+Tab (`BackTab`) cycle the bar's fields only.** They
  switch between Find and With while the replace row is shown, and do
  nothing otherwise. To leave the bar, press Esc (which closes it) or
  click the editor.
- **Enter in the With field sends `ReplaceCurrent`**, and Shift+Enter
  there does nothing. This differs from the GUI on purpose (the GUI
  ignores Enter there). It matches VS Code.

## Further decisions (orchestrator)

These come from reading the current code and the GUI's
`search_bar_ui` (`frext/src/app.rs`). Each is GUI parity unless it
says otherwise.

- **Module.** The bar is `frext-term/src/find_bar.rs`: the
  `FindField` enum, the `FindBar` view state (two `TextInput`s and
  their last rects), `height`, rendering, and field input. The core
  owns the text of both fields. The bar owns only their cursors and
  scroll.
- **Focus.** `keymap::Focus` gains `Find(FindField)`, with
  `find_bar::FindField { Pattern, Replacement }`.
  - `settle_focus` moves `Find(_)` back to `Editor` when the bar is
    closed, and `Find(Replacement)` to `Find(Pattern)` when the replace
    row is hidden. `TermApp::handle` calls it before routing an event
    and again after `apply_effects`.
  - Ctrl+E (`ToggleFocus`) treats `Find(_)` like `Editor`: it goes to
    the sidebar when the sidebar is shown.
- **Selection to core.** While focus is `Find(_)`, `send` passes the
  editor's selection, as it does for `Editor`. This is GUI parity: egui
  keeps reporting the editor's last cursor range while the find field
  has focus. In-selection and Replace all need it. Only `Sidebar`
  sends `None`.
- **Key routing while focus is `Find(field)`.** `keymap::action_for`
  runs first and keeps the global keys:
  - the global Ctrl letters
  - F3 / Shift+F3 / F15
  - Esc (`CloseSearch`)
  - the Alt toggles

  The editing keys and the editing Ctrl letters stay `Editor`-only. A
  key that `action_for` does not bind goes to `FindBar::handle_key`:
  - Tab / `BackTab` / Shift+Tab: switch fields (see above).
  - **Find:** an edit sends `RecomputeMatches`. Enter sends
    `StepMatch(Forward)`, and Shift+Enter sends `StepMatch(Backward)`.
  - **With:** an edit sends nothing (no recompute, as in the GUI). Enter
    sends `ReplaceCurrent`.
  - Either field: `TextInput`'s `Cancel` (Esc, if it ever gets there)
    sends `CloseSearch`.
  - Under legacy encoding, Ctrl+H is unbound outside the editor, so it
    reaches the field, where `TextInput` treats it as Backspace.
- **`FocusSearchField`** (only while the bar is open) focuses
  `Find(Pattern)` and moves that field's cursor to the end of the
  pattern, as `FocusPromptField` does for prompts.
- **Layout.** `layout::areas` takes `find_rows: u16` instead of
  `find_open: bool`. `find_bar::height(search)` is 0 while the bar is
  closed. Otherwise it is 1 (the find row), plus the error rows, plus
  1 when the replace row is shown, in that order top to bottom (the
  GUI's order). Rows are taken in this order: tab bar, status, find
  bar, then the body gets the rest.
- **Regex error.** The text is `format!("regex: {err}")`, as in the GUI.
  `regex`'s syntax errors are several lines long: a header, the
  pattern, a `^` caret line, and `error: …`. In a monospace terminal
  the caret lines up with the pattern, so the bar shows every line, up
  to `MAX_ERROR_ROWS = 4`. With more lines it shows the first 3 and
  then the last one. Each line starts at column 1 in
  `theme::status_error()` and is cut to the row width minus 2.
- **Find row**, left to right, every item separated by one blank cell:
  - `" Find "`, a 6-cell label in `theme::chrome()`
  - the find field, `field_width(area.width)` cells
  - `[Aa]` `[W]` `[.*]` `[sel]`
  - `[Prev]` `[Next]`
  - the count, in a slot of `COUNT_WIDTH = 11` cells
    (`SearchState::count_label()`, left aligned, in `theme::status()`)
  - `[Replace]` (the replace-row toggle)
  - `[×]`
- **Replace row:**
  - `" With "`
  - the replace field (the same width, so the fields line up)
  - `[Replace]` (`ReplaceCurrent`)
  - `[Replace all]`
- **Field width.**
  `field_width(width) = width.saturating_sub(FIXED_WIDTH).clamp(MIN_FIELD_WIDTH, MAX_FIELD_WIDTH)`.
  - `FIXED_WIDTH = 66` is every cell of the find row except the field.
  - `MIN_FIELD_WIDTH = 10`, `MAX_FIELD_WIDTH = 40`.
  - So an 80-column terminal gets a 14-cell field. GUI parity is a
    fixed width (220 px).
- **Clipping.** Items are laid out left to right. The first item that
  does not fit whole in the row is not drawn and gets no hit region,
  and neither does anything after it. The field is cut to the space
  left.
- **Placeholders.** While a field is empty, its hint is drawn over it
  in `theme::input_hint()`:
  - Find: `search…`
  - With: `replacement…`, or `replacement ($1 for groups)…` when regex
    is on

  The field's cursor stays at its first cell. `TextInput` is not
  changed.
- **Styles.** The bar fills with `theme::chrome()`.
  - A toggle that is on uses `theme::toggle_on()`. This covers the four
    option toggles, which follow `SearchQuery` /
    `SearchState::in_selection`, and `[Replace]`, which follows
    `replace_open`.
  - A toggle that is off, and every button, uses `theme::chrome()`.
    While hovered it uses `theme::chrome().bg(theme::hover_bg())`.
  - A toggle that is on keeps `toggle_on()` while hovered.
- **Hit targets.** These are new `hit::Target` variants. Their hints are
  shown on the status line, since tooltips become status-line text.

  | Target                                   | Hint                                           |
  | ---------------------------------------- | ---------------------------------------------- |
  | `FindInput`, `ReplaceInput`              | none                                           |
  | `FindOption(SearchOption::CaseSensitive)` | `Match case (Alt+C)`                           |
  | `FindOption(SearchOption::WholeWord)`    | `Whole word (Alt+W)`                           |
  | `FindOption(SearchOption::Regex)`        | `Regular expression (Alt+R)`                   |
  | `FindOption(SearchOption::InSelection)`  | `Search within the current selection (Alt+L)`  |
  | `FindPrev`                               | `Previous match (Shift+F3)`                    |
  | `FindNext`                               | `Next match (F3 / Enter)`                      |
  | `FindReplaceRow`                         | `Toggle replace`                               |
  | `FindClose`                              | `Close (Esc)`                                  |
  | `ReplaceCurrent`                         | `Replace the current match (Enter in With)`    |
  | `ReplaceAll`                             | `Replace all matches`                          |

- **Mouse on the bar** (with no prompt or menu open):
  - A left click on a field focuses it and places its cursor
    (`TextInput::click`, relative to the field's last rect).
  - A left click on any other target sends its command:
    `ToggleSearchOption(o)`, `StepMatch(Backward / Forward)`,
    `ToggleReplaceRow`, `CloseSearch`, `ReplaceCurrent`, `ReplaceAll`.
    It leaves the focus where it was.
  - Right clicks and the wheel over the bar do nothing.
- **Paste** while focus is `Find(field)` goes into that field
  (`TextInput::insert_str`, which turns line breaks into spaces). A
  paste into Find sends `RecomputeMatches`.
- **The terminal cursor** is drawn by the focused find field while
  focus is `Find(_)` and no prompt or menu is open. The order of
  precedence is the prompt, then the editor, then the find field. These
  are mutually exclusive in practice.
- **`FIND_BAR_MESSAGE` and `send_command` are removed.** Every caller
  calls `send`.
- **Match highlights are windowed in core.** `overlay_matches` tests
  every match for every byte. Feeding it a whole buffer's matches every
  frame would cost O(viewport bytes × matches). The new
  `MatchHighlights::within(bytes)` in `frext-core` uses a binary search
  to keep only the matches that overlap a byte window, and remaps
  `current`. This is rendering support, not search semantics. The
  `BufferView` windows the highlights to the visible lines' spans before
  overlaying them.
- **Not in this plan:**
  - Ctrl+A / C / X / V / Z inside the fields. They stay editor-only,
    as in prompts.
  - Persisting `replacement` or `in_selection`, which is core
    behaviour.
  - Hint text inside `TextInput` itself.
  - Docs (plan 09).

## Execution model

All subtasks run **strictly in sequence** on
`feat/frext-term-find-replace`. Each subtask is one commit and leaves
the suite green (`commit-discipline`). The orchestrator reviews the full
diff of each subtask before committing it. Implementers do not commit,
merge or push.

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

Benchmarks (`performance-benchmarks`): 04.2 touches the buffer render
path. It records a `before04` baseline of `frext-term`'s `buffer` bench
before changing any code. It then reports the deltas and a new
with-matches row. A regression over 15% must be justified or fixed.

## Subtasks

### 04.1 -- Core: windowed match highlights

Scope: `frext-core/src/highlight/mod.rs`.

What:

- `MatchHighlights` also derives `PartialEq, Eq`.
- `#[must_use] pub fn within(&self, bytes: Range<usize>) -> Self`. The
  doc states the precondition, which `SearchState` guarantees: `ranges`
  are sorted by start and do not overlap.
  - An empty `bytes` gives `Self::default()`.
  - Otherwise, `lo = ranges.partition_point(|r| r.end <= bytes.start)`
    and `hi = ranges.partition_point(|r| r.start < bytes.end)`, with
    `hi` taken as at least `lo`. The result keeps `ranges[lo..hi]`
    (through `get`) and `current - lo` when `current` is in `lo..hi`,
    otherwise `None`.

Deliverable: tests in `highlight/mod.rs`:

- `within` keeps only the overlapping matches, including a match that
  crosses each edge of the window
- `current` inside the window is remapped
- `current` outside the window becomes `None`
- an empty window, or no matches, is empty
- a window past the end is empty
- `overlay_matches(spans, &m.within(w))` equals
  `overlay_matches(spans, &m)` for spans inside `w`

Verification: the standard ritual.

Prohibitions: do NOT change `overlay_matches`, `highlight` or any
search code; do NOT touch `frext-term` or the GUI; do NOT proceed to
04.2.

Stop: report files changed and verification; await review.

### 04.2 -- Buffer view: paint match highlights

Scope: `frext-term/src/buffer/mod.rs`, `frext-term/src/app.rs` (the
one `buffer.render` call and one new test), `frext-term/benches/buffer.rs`.

First, before editing anything, run `cargo bench -p frext-term --bench
buffer -- --save-baseline before04`.

What:

- `BufferView::render` gains a parameter `matches: &MatchHighlights`,
  after `editor`. Once the spans are computed, if they are non-empty,
  take `window = first.bytes.start..last.bytes.end` and use
  `overlay_matches(spans, &matches.within(window))`. Pass the result to
  `render::render`. Nothing else changes.
- `app.rs`: `render` passes `&self.editor.match_highlights()`.
- The `draw` test helper in `buffer/mod.rs` passes
  `&MatchHighlights::default()`, and a `draw_with` variant takes the
  matches.
- `benches/buffer.rs`: the two existing rows pass
  `&MatchHighlights::default()`. A new row,
  `render_with_matches_rust_5k_lines`, renders the warm view at line
  2,500 with `MatchHighlights { ranges: <every match_indices("self")
  as a byte range>, current: Some(<the first match at or after line
  2,500>) }`.

Deliverable: tests:

- `buffer/mod.rs`:
  - a match on a visible row paints `match_bg(Other)` on its cells
    only, and the current match paints `match_bg(Current)`
  - a match on a row scrolled out of view does not panic, and leaves
    the visible cells on `BASE`
  - a selected cell inside a match keeps the selection colour
- `app.rs`: with the text `"hello world hello"`, select the first
  `hello`, press Ctrl+F, and draw. The first `hello` has the
  current-match bg, and the second has the other-match bg. After Esc
  and a redraw, neither has a match bg.

Verification: the standard ritual, plus `cargo bench -p frext-term
--bench buffer -- --baseline before04`. Report the table of the
`before04` → after deltas and the new row.

Prohibitions: do NOT change `render.rs`, `theme.rs` or core; do NOT
add search logic; do NOT render the find bar; do NOT proceed to 04.3.

Stop: report and await review.

### 04.3 -- Layout: a multi-row find bar

Scope: `frext-term/src/layout.rs`, `frext-term/src/app.rs` (the one
`layout::areas` call).

What:

- `areas(area: Rect, find_rows: u16, sidebar_width: Option<u16>,
  gutter_width: u16) -> Areas`. The find bar takes
  `take(find_rows)` (still after the tab bar and status). The doc
  comments of `areas` and `Areas::find_bar` are updated.
- `app.rs` passes `u16::from(self.editor.search().is_open())` for now,
  so there is no behaviour change.

Deliverable: the existing layout tests are updated to pass `0` / `1`,
plus:

- a 3-row find bar pushes the editor down 3 rows
- a find bar taller than the space left takes what is left, and leaves
  the body 0 rows, without panicking
- the `every_area_within_bounds` sum check covers `find_rows` up to 6

Verification: the standard ritual.

Prohibitions: do NOT add `find_bar.rs`; do NOT proceed to 04.4.

Stop: report and await review.

### 04.4 -- Hit targets and theme styles for the bar

Scope: `frext-term/src/hit.rs`, `frext-term/src/theme.rs`,
`frext-term/src/app.rs` (only to add the new targets to the existing
"ignored" arm of `handle_mouse`).

What:

- `hit::Target` gains `FindInput`, `ReplaceInput`,
  `FindOption(SearchOption)`, `FindPrev`, `FindNext`,
  `FindReplaceRow`, `FindClose`, `ReplaceCurrent` and `ReplaceAll`.
  `SearchOption` is `frext_core::editor::SearchOption`. `Target::hint`
  returns the exact hints in the table under "Further decisions".
- `theme.rs`:
  - `#[must_use] pub fn toggle_on() -> Style`: fg
    `palette::accent()`, bg `palette::selection().over(MANTLE)`, BOLD
  - `#[must_use] pub const fn input_hint() -> Style`: fg `OVERLAY0`, bg
    `SURFACE1`
- `app.rs`: the new targets join the `Some(Target::PromptPopup | …) |
  None => {}` arm (they are wired in 04.9).

Deliverable: tests:

- every new target's hint (exact strings, and `None` for the two
  inputs)
- a `HitMap` region for `FindOption(Regex)` resolves
- the colours of both theme helpers

Verification: the standard ritual.

Prohibitions: do NOT render anything; do NOT change `keymap.rs`; do
NOT proceed to 04.5.

Stop: report and await review.

### 04.5 -- Find bar: height and rendering

Scope: `frext-term/src/find_bar.rs` (new), `frext-term/src/lib.rs`.

What:

- `lib.rs`: `pub mod find_bar;`. The module doc describes the bar and
  the split of ownership (core owns the text, the bar owns cursors).
- `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum FindField {
  Pattern, Replacement }`
- Constants: `pub const MAX_ERROR_ROWS: u16 = 4;`,
  `pub const COUNT_WIDTH: u16 = 11;`, `pub const FIXED_WIDTH: u16 = 66;`,
  `pub const MIN_FIELD_WIDTH: u16 = 10;`,
  `pub const MAX_FIELD_WIDTH: u16 = 40;`,
  `pub const PATTERN_HINT: &str = "search…";`,
  `pub const REPLACEMENT_HINT: &str = "replacement…";`,
  `pub const REGEX_REPLACEMENT_HINT: &str = "replacement ($1 for groups)…";`
- `#[must_use] pub fn field_width(width: u16) -> u16` (the rule under
  "Further decisions").
- `#[must_use] pub fn error_lines(search: &SearchState) -> Vec<String>`:
  no lines without an error; otherwise the lines of
  `format!("regex: {err}")`, with the first-3-plus-last rule when there
  are more than `MAX_ERROR_ROWS`.
- `#[must_use] pub fn height(search: &SearchState) -> u16` (the rule
  under "Further decisions", using `error_lines`).
- `#[derive(Debug, Default)] pub struct FindBar { pattern: TextInput,
  replacement: TextInput, pattern_rect: Option<Rect>, replacement_rect:
  Option<Rect> }`, with `#[must_use] pub fn new() -> Self`.
- `pub fn render(&mut self, editor: &Editor, focus: Option<FindField>,
  hovered: Option<Target>, area: Rect, buf: &mut Buffer, hits: &mut
  HitMap) -> Option<Position>`:
  - It clears both rects. With the bar closed, or an empty `area`, it
    returns `None` and draws nothing.
  - Otherwise it fills `area` with `theme::chrome()`. Then it draws the
    find row, the error lines and the replace row, in that order, from
    `area.y` down, skipping any row past `area.bottom()`.
  - Item order, gaps, clipping, placeholders and styles are exactly as
    under "Further decisions". Each item draws its bracketed label and
    pushes its `Target` over exactly its cells. Each field pushes
    `FindInput` / `ReplaceInput` over its rect and stores the rect.
  - It returns the cursor position from `TextInput::render` of the
    field named by `focus` (`None` when `focus` is `None` or that
    field's row is not drawn).

Deliverable: `TestBackend` / `Buffer` tests over a real `Editor` (copy
`prompt.rs`'s `temp_dir` / `open_editor` helpers; drive the state with
`editor.apply(..)` and `search_pattern_mut`):

- `field_width` at widths 0, 76, 80, 106 and 200
- `height`:
  - closed: 0
  - open: 1
  - with replace: 2
  - with a regex error from pattern `(`: `1 + error_lines.len()`
- `error_lines`:
  - for `(` it starts with `regex: invalid regular expression:`
  - a synthetic error of more than 4 lines keeps the first 3 and the
    last. Build a `SearchState` through the editor where you can.
    Otherwise test the line-selection helper directly as a private
    `fn cap_lines(lines: Vec<String>) -> Vec<String>`.
- at 80 columns the find row reads exactly
  `" Find " + 14-cell field + " [Aa] [W] [.*] [sel] [Prev] [Next] " +
  count slot + " [Replace] [×]"` (assert the label columns)
- the placeholder `search…` shows while the pattern is empty, and the
  pattern text shows otherwise
- the count shows `1/2` for `hello` in `"hello hello"` after
  `RecomputeMatches`
- `[Aa]` uses `toggle_on()` after `ToggleSearchOption(CaseSensitive)`,
  and `chrome()` before
- the hovered `[Next]` has the hover bg
- the replace row shows with `OpenSearch(FindReplace)`, its field has
  the same x and width as the find field, and the regex placeholder is
  used when regex is on
- the regex error rows sit between the find and replace rows, in the
  error style
- hit regions: every item resolves to its `Target` at its first and
  last cell, and the gap cells between items resolve to `None`
- at width 40 the trailing items are not drawn and have no hit region,
  and nothing panics at widths 1 and 0
- the returned cursor is in the focused field, and `None` without focus

Verification: the standard ritual.

Prohibitions: do NOT handle input (04.6); do NOT touch `app.rs`,
`keymap.rs` or `text_input.rs`; do NOT proceed to 04.6.

Stop: report and await review.

### 04.6 -- Find bar: field input

Scope: `frext-term/src/find_bar.rs`.

What:

- `#[derive(Debug, Clone, PartialEq, Eq)] pub enum FindReply {
  Send(Command), Focus(FindField), Handled, Ignored }`
- `pub fn handle_key(&mut self, editor: &mut Editor, field: FindField,
  key: KeyEvent) -> FindReply`:
  1. A `Release` is `Ignored`.
  2. Tab without modifiers, `BackTab`, or Shift+Tab: with the replace
     row shown, `Focus(<the other field>)`. Otherwise `Handled`.
  3. Otherwise `TextInput::handle_key` on that field's text
     (`search_pattern_mut` / `search_replacement_mut`):
     - Find: `Edited` gives `Send(RecomputeMatches)`. `Submit` gives
       `Send(StepMatch(Backward))` with Shift held, else
       `Send(StepMatch(Forward))`.
     - With: `Edited` gives `Handled`. `Submit` with Shift is
       `Handled`, and without it is `Send(ReplaceCurrent)`.
     - Both: `Moved` is `Handled`, `Cancel` is `Send(CloseSearch)`, and
       `Ignored` is `Ignored`.
- `pub fn focus_field(&mut self, editor: &Editor, field: FindField)`:
  moves that field's cursor to the end of its text.
- `pub fn click(&mut self, editor: &Editor, field: FindField, column:
  u16)`: `TextInput::click` with `column` relative to the field's last
  rect (does nothing without a rect).
- `pub fn paste(&mut self, editor: &mut Editor, field: FindField, text:
  &str) -> FindReply`: `TextInput::insert_str`. Find gives
  `Send(RecomputeMatches)`, and With gives `Handled`.

Deliverable: tests:

- typing `ab` into Find edits the pattern and replies
  `Send(RecomputeMatches)` each time
- Backspace likewise
- Left / Home are `Handled`
- Enter and Shift+Enter in Find step forward and backward
- Enter in With is `ReplaceCurrent`, and Shift+Enter there is `Handled`
- typing into With edits the replacement and is `Handled`
- Tab / BackTab / Shift+Tab switch fields with the replace row shown,
  and are `Handled` without it
- Esc is `Send(CloseSearch)`
- an Alt+char and a Release are `Ignored`
- `focus_field` puts the cursor at the end (it renders at the end of
  the pattern)
- `click` after a render places the cursor at the clicked grapheme
- `paste` of `"a\nb"` into Find gives the pattern `a b` and
  `Send(RecomputeMatches)`, and into With gives `Handled`

Verification: the standard ritual.

Prohibitions: do NOT touch `app.rs`, `keymap.rs` or `text_input.rs`;
do NOT proceed to 04.7.

Stop: report and await review.

### 04.7 -- Keymap: find focus and the toggle keys

Scope: `frext-term/src/keymap.rs`, `frext-term/src/app.rs` (only the
two exhaustive `Focus` matches, in `send` and `Action::ToggleFocus`).

What:

- `Focus` gains `Find(FindField)` (`crate::find_bar::FindField`), with a
  doc comment.
- In `action_for`, after the Ctrl-letter check: a `KeyCode::Char(c)`
  whose modifiers are exactly `ALT`, with `ctx.search_open`, maps
  `c.to_ascii_lowercase()` to a toggle. `c` is
  `CaseSensitive`, `w` is `WholeWord`, `r` is `Regex` and `l` is
  `InSelection`, each as
  `Action::Command(Command::ToggleSearchOption(..))`. Any other Alt
  char falls through to the existing arms.
- Nothing else changes: the editing keys and editing Ctrl letters
  already require `Focus::Editor`. The module doc now mentions the
  find-field focus and the Alt toggles.
- `app.rs`:
  - `send`: `Focus::Editor | Focus::Find(_)` pass the editor's
    selection, and `Focus::Sidebar` passes `None`
  - `ToggleFocus`: `Focus::Editor | Focus::Find(_) => Focus::Sidebar`

Deliverable: keymap tests:

- Alt+C / W / R / L with search open map to the four toggles, and with
  it closed map to `None`
- Alt+Shift+C and Ctrl+Alt+C are `None`
- the toggles also work in `Sidebar` and `Find(_)` focus
- in `Find(Pattern)` focus:
  - a plain char, Enter, Tab, Backspace, Left, Ctrl+A, Ctrl+V and
    Ctrl+Z are `None`
  - Ctrl+F, Ctrl+S, F3, Shift+F3 and Esc keep their actions
  - Legacy Ctrl+H is `None`

Verification: the standard ritual.

Prohibitions: do NOT route keys to the find bar or change any other
app behaviour (04.8); do NOT proceed to 04.8.

Stop: report and await review.

### 04.8 -- App: render the bar, keyboard, focus and effects

Scope: `frext-term/src/app.rs`.

What:

- `TermApp` gains `find: FindBar`.
- `render`:
  - `layout::areas` gets `find_bar::height(self.editor.search())`.
  - The `fill(buf, areas.find_bar, ..)` becomes `self.find.render(&self.editor,
    <Some(f) iff focus is Find(f)>, self.hovered, areas.find_bar, buf,
    &mut self.hits)`, drawn at the same point in the order.
  - The frame cursor is `prompt_cursor.or(editor_cursor).or(find_cursor)`,
    where `find_cursor` is used only while focus is `Find(_)`, no menu
    is open and no prompt is open.
- `handle_key`: when `keymap::action_for` returns `None` and focus is
  `Find(field)`, call `self.find.handle_key(&mut self.editor, field,
  key)`:
  - `Send(c)` goes to `self.send(c, now)`.
  - `Focus(f)` goes to `self.set_focus(Focus::Find(f))`.
  - `Handled` and `Ignored` do nothing.
- `settle_focus` adds the two find rules under "Further decisions".
  `handle` also calls it after `apply_effects`.
- `apply_effects`: `FocusSearchField`, while the bar is open, calls
  `self.find.focus_field(&self.editor, FindField::Pattern)` and then
  `self.set_focus(Focus::Find(FindField::Pattern))`.
- `FIND_BAR_MESSAGE` and `send_command` are removed. Their callers use
  `send`.
- The tests that asserted `FIND_BAR_MESSAGE` or a blank find row are
  rewritten:
  - `legacy_mode_binds_ctrl_r_and_ctrl_h_is_not_replace` asserts the
    bar's find and replace rows are drawn.
  - `a_tick_after_four_seconds_clears_the_message` uses the legacy
    startup message.
  - `choosing_find_opens_the_search` asserts focus `Find(Pattern)` and
    the drawn bar.

Deliverable: app tests (`TestBackend`, real `Editor`):

- Ctrl+F draws the bar on row 1, focus becomes `Find(Pattern)`, and the
  frame cursor is in the find field
- typing `wor` into the field sets the pattern, the count shows `1/1`
  for `"hello world"`, and the match is highlighted on the next draw
- Enter selects the next match and keeps focus in the field, Shift+Enter
  goes back, and F3 works from the field
- Alt+C toggles match case (the count changes for `"Hello hello"` with
  pattern `hello`)
- Ctrl+H (Enhanced) shows the replace row. Tab moves to With, typing
  sets the replacement, and Enter replaces the current match. Ctrl+Z
  (after a click into the editor) restores it, which proves the outside
  edit was recorded.
- `ToggleReplaceRow` while focus is in With moves focus to Find
- Esc closes the bar, focus returns to the editor, the bar row is gone,
  and the editor caret is the frame cursor
- a seeded pattern (a selection plus Ctrl+F) puts the field cursor at
  the end of the pattern
- in-selection: select the first line of `"a a\na a"`, press Ctrl+F,
  type `a`, then Alt+L. The count is `1/2`, which proves the selection
  reached core while focus was in the field.
- Legacy Ctrl+H in the field deletes a char from the pattern
- a regex error (`(` with Alt+R) draws the error rows, and the editor
  moves down by them

Verification: the standard ritual, plus `cargo bench -p frext-term
--bench buffer`.

Prohibitions: do NOT add mouse or paste handling for the bar (04.9); do
NOT change core, `find_bar.rs` or `keymap.rs`; do NOT proceed to 04.9.

Stop: report and await review.

### 04.9 -- App: mouse and paste on the bar

Scope: `frext-term/src/app.rs`.

What:

- `handle_mouse`, `Down(Left)` with no prompt open:
  - `FindInput` calls `self.find.click(&self.editor, Pattern, column)`
    and then `set_focus(Focus::Find(Pattern))`. `ReplaceInput` does the
    same for `Replacement`.
  - The other find targets send their commands as listed under
    "Further decisions", without changing focus.
- Paste while focus is `Find(field)` (no menu, no prompt):
  `self.find.paste(&mut self.editor, field, &text)`, and a `Send(c)`
  reply goes to `send`.

Deliverable: app tests:

- clicking the find field focuses it, and the cursor lands at the
  clicked column
- clicking `[Aa]` toggles case and keeps focus where it was
- `[Next]` / `[Prev]` step
- `[Replace]` shows the replace row
- `[×]` closes the bar
- With and `[Replace all]` replace every match
- hovering `[.*]` shows `Regular expression (Alt+R)` on the status line
- a bracketed paste into the find field sets the pattern and recomputes
  the count
- a paste while focus is in the editor still goes to the buffer
- clicks on the bar while a prompt is open do nothing to the bar

Verification: the standard ritual, plus the manual smoke checklist
below, run by the orchestrator with the user.

Prohibitions: do NOT change any other file; do NOT proceed further.

Stop: report and await review.

## Manual smoke checklist

Run in a real terminal (kitty or freminal, then a legacy terminal):

1. Ctrl+F opens the bar with the cursor in Find. Typing highlights
   matches live, and the current match is distinct.
2. Enter / Shift+Enter / F3 / Shift+F3 step, with wraparound, and the
   view follows.
3. Alt+C / Alt+W / Alt+R / Alt+L toggle, and the buttons show it. An
   invalid regex shows the error rows.
4. Ctrl+H (kitty) or Ctrl+R (legacy) shows the replace row. Tab
   switches fields. Enter in With replaces one, and Replace all replaces
   every match. Ctrl+Z in the editor undoes them.
5. A one-line selection seeds Find. In-selection limits matches to the
   selection.
6. The mouse works on every button and field, and hovering shows the
   hint.
7. Esc closes the bar and returns to the editor.
8. Legacy terminal: Backspace in the field deletes.
9. An 80-column terminal shows the whole find row, and a narrow one
   clips it cleanly.
