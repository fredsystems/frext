# 05 -- TUI file-tree sidebar

- **Status:** active
- **Depends on:** 02
- **Branch:** `feat/frext-term-file-tree` (one branch, one commit per
  subtask, one PR)

## Goal

The left file-tree sidebar, shown in the same situations as the GUI:
`frext-term <dir>` opens it rooted at `<dir>`, it persists in the TUI
session, and it can be closed. It offers the same rows, expansion
behaviour, active-file highlight, context menus and name prompts as
the GUI.

## Task summary

- A pure tree-flattening helper in `frext-core::workspace`:
  `visible_rows(&Workspace) -> Vec<TreeRow>`, with
  `TreeRow { path, depth, kind }` and
  `kind: RowKind::{Dir(FolderState), File}`, built lazily from
  `read_dir_split` over expanded folders only. It is unit-tested in core;
  the GUI may adopt it later, but this plan does not change the GUI.
- Header row: the root via `Workspace::display_root()` and a close
  button (`Command::CloseWorkspace`).
- Rows: indentation, a disclosure marker, an icon cell from the
  `IconProvider` interface (this plan ships only the ASCII provider;
  plan 08 adds Nerd Font and kitty images), and the name. The active
  tab's file is highlighted (canonicalised comparison, as in the GUI).
- Input: Up/Down/PageUp/PageDown, Left collapses or moves to the
  parent, Right expands, Enter opens (`Command::OpenPath`); mouse click
  opens a file or toggles a folder; the wheel scrolls; right-click
  opens the file/folder context menu, and so does Shift+F10 / the Menu
  key on the selected row. A right-click on empty space targets the
  root.
- Context menus send the existing commands (`BeginRename`,
  `BeginNewFile`, `BeginNewFolder`, `CopyPath`, `CopyRelativePath`,
  `Reveal`, `Trash`). Name prompts reuse plan 02's prompt framework
  over `Prompt::Name`.
- Resizable width by dragging the border with the mouse, plus a
  keyboard binding; a sensible default and minimum width.

## Durable decisions

- **`IconProvider` is the seam for plan 08.**
  `fn icon(&self, name: &str, kind: IconKind) -> IconCell`, where
  `IconCell` is `Text { text, color }` here and gains an image variant
  in plan 08. The stem lookup stays in `frext_core::file_icon`.
- Expansion state is written through `Command::SetExpanded`, the same
  persisted set the GUI uses (in the TUI's own session file).
- Reveal in file manager stays available (it runs `xdg-open`); it is
  harmless when there is no desktop session.

## Decided at activation

- **The sidebar width is not persisted.** It is TUI view state, starts
  at the default every launch, and needs no core change.
- **Focus and toggle keys are plan 02's**: Ctrl+B shows or hides the
  sidebar, Ctrl+E moves focus between the editor and the sidebar, and
  Esc in the sidebar returns focus to the editor. **Resize keys are
  `<` and `>` while the sidebar has focus**, one column per press. The
  tree has no text input, so plain keys are free there.
- **Directories are re-read on every render**, as the GUI does. The TUI
  renders only after an event batch or the 500 ms tick, so this is
  cheap and always fresh. The active-file comparison canonicalises
  only the file rows that are drawn.
- **Focus after opening a file**: Enter opens it and keeps focus in the
  tree (VS Code). A mouse click on a file opens it and moves focus to
  the editor, ready to type.
- **The ASCII icon provider** draws `/` in `palette::BLUE` for a folder
  and nothing for a file, in a 2-cell slot. The disclosure marker
  already shows whether a folder is open.

## Further decisions (orchestrator)

These come from reading the current code: `frext-core/src/workspace.rs`,
`frext-core/src/editor/{command,tree,prompt}.rs`, the GUI's `tree_dir`,
`file_context_menu` and `dir_context_menu` (`frext/src/app.rs`), and
`frext-term`'s `app.rs`, `keymap.rs`, `layout.rs`, `menu.rs`, `hit.rs`,
`prompt.rs` and `theme.rs`. Each is GUI parity unless it says otherwise.

- **Modules.**
  - `frext-core/src/workspace.rs` gains `TreeRow`, `RowKind` and
    `visible_rows`. `RowKind::Dir` carries
    `frext_core::editor::FolderState` (where it lives today).
  - `frext-term/src/icons.rs` (new): `IconKind`, `IconCell`, the
    `IconProvider` trait and `AsciiIcons`.
  - `frext-term/src/tree_menu.rs` (new): the file and folder context
    menus, mirroring `editor_menu.rs`.
  - `frext-term/src/tree.rs` (new): `FileTree`, the sidebar's view
    state (rows from the last refresh, the selected row, scroll, width,
    the border drag), its rendering and its key handling.
- **Ownership.** Core owns the workspace (root and expanded set) and
  every file-system action. `FileTree` owns only the selection, scroll,
  width and drag state. Every change to the tree goes through a
  `Command`.
- **Rows.** `visible_rows` lists the root's children at depth 0, folders
  first and then files, each sorted as `read_dir_split` sorts them, and
  descends into a folder only when `Workspace::is_expanded` says so. An
  expanded entry whose parent is collapsed is not shown. Hidden files
  are shown, as in the GUI. `read_dir_split` puts symlinks under files,
  so there is no recursion through links.
- **Refresh.** `FileTree::refresh(Option<&Workspace>) -> Refresh`
  recomputes the rows. It runs at the start of the tree's render and,
  in the app, before every key the tree handles, so a second key in
  the same event batch sees the rows the first key's command produced.
  It returns `Refresh::Changed` when the rows differ from the previous
  ones; the app then calls `forget_layout`, because the hit map's
  `TreeRow(index)` regions describe the old rows. Mouse events always
  resolve against the rows of the last render, which are the rows the
  hit map was built from.
- **Selection** is a path, so it survives rows appearing above it.
  When a refresh no longer lists the selected path, the selection moves
  to its nearest ancestor that is listed (a collapsed parent). Failing
  that it moves to the row at the old index, clamped to the last row;
  with no rows it is `None`.
- **Layout of the sidebar column** (`area` is `Areas::sidebar`):
  - The last column is the **border**: a blank cell in
    `theme::tree_border()` (bg `SURFACE0`), or
    `theme::tree_border_active()` (bg `palette::accent()`) while it is
    hovered or being dragged. It carries `Target::TreeBorder` over the
    full height. A blank cell is used rather than `│`, which is East
    Asian Ambiguous and would shift the editor's hit regions on a
    terminal that draws it wide (the 04.10 rule).
  - Everything else is filled with `theme::chrome()`.
  - **Header** (the first row): one blank cell, then `[x]` in
    `theme::close_glyph()` (bg `hover_bg()` while hovered) carrying
    `Target::TreeClose`, one blank cell, then `display_root()` in
    `theme::chrome()` + `BOLD`, cut at the border. ASCII `[x]` follows
    the find bar's 04.10 rule. Nothing is drawn in the header when the
    space left of the border is under 4 cells.
  - **Rows** fill the rest, one per row, from `top`. Each row's cells,
    left to right: one blank cell, `2 * depth` blank cells, the
    disclosure marker (`>` collapsed folder, `v` expanded folder, blank
    for a file), one blank cell, the icon slot (`ICON_WIDTH = 2` cells:
    the `IconCell` text in its colour, padded with blanks), then the
    file name (`file_name().to_string_lossy()`), cut at the border.
    So a depth-0 folder reads `" > / src"` and a file `"     main.rs"`.
  - **Row styles.** The base style is `theme::chrome()`. The active
    tab's file uses `theme::tree_active()` (accent fg, BOLD, on
    `MANTLE`, like `active_tab()`). The background is
    `theme::tree_cursor_bg()` (`selection().over(SURFACE0)`, as in
    `button_focused()`) on the selected row while the sidebar has
    focus, else `hover_bg()` on the hovered row, else `MANTLE`. The
    icon text keeps its own fg on the row's background.
  - Each drawn row carries `Target::TreeRow(index)` over the row
    (without the border), where `index` is its position in the rows.
    The rows area below the last row carries `Target::TreeEmpty`.
- **Width.** `DEFAULT_WIDTH = 30` (the current `SIDEBAR_WIDTH`),
  `MIN_WIDTH = 12`, `MIN_EDITOR_WIDTH = 20`. The width includes the
  border. `clamp_width(width, total) = width.clamp(MIN_WIDTH,
  total.saturating_sub(MIN_EDITOR_WIDTH).max(MIN_WIDTH))`. The stored
  width is always clamped against the last rendered terminal width;
  the app passes `clamp_width(stored, area.width)` to `layout::areas`,
  which still clamps to the terminal for very small terminals. Dragging
  the border sets the width so the border sits under the mouse:
  `clamp_width(column - area.x + 1, total)`.
- **Keys while the sidebar has focus.** `keymap::action_for` is
  unchanged and runs first, so the global keys and Esc keep working.
  A key it leaves unbound goes to `FileTree::handle_key`:

  | Key                           | Effect                                                                 |
  | ----------------------------- | ---------------------------------------------------------------------- |
  | Up / Down                     | select the previous / next row (stops at the ends)                     |
  | PageUp / PageDown             | move by `max(rows_height - 1, 1)` rows                                 |
  | Home / End                    | select the first / last row                                            |
  | Right on a collapsed folder   | `SetExpanded { state: Expanded }`                                      |
  | Right on an expanded folder   | select its first child, if it has one                                  |
  | Left on an expanded folder    | `SetExpanded { state: Collapsed }`                                     |
  | Left on anything else         | select the parent folder's row (nothing at depth 0)                    |
  | Enter on a folder             | `SetExpanded` with the opposite state                                  |
  | Enter on a file               | `OpenPath(file)`; focus stays in the tree                              |
  | Shift+F10, `F(22)`, Menu      | open the context menu for the selected row (the root with none)        |
  | `<` / `>`                     | narrow / widen the sidebar by one column                               |

  - With nothing selected, any of the motion keys selects the first row
    (End selects the last). Every motion scrolls the selection into
    view.
  - The keys match with no modifiers, except Shift for Shift+F10 and
    for `<` / `>` (terminals may report the Shift that typed them).
    Any key with Ctrl or Alt is ignored.
  - `F(22)` is how some xterm-style terminals report Shift+F10, as they
    report Shift+F3 as `F(15)`. The Menu key arrives only under the
    kitty protocol.
  - A keyboard-opened menu is anchored on the selected row at its name
    column; with no selection, at the first row's left edge.
- **Mouse on the sidebar** (no prompt or menu open):
  - Left click on `TreeClose` sends `CloseWorkspace`.
  - Left click on `TreeRow(i)` selects the row. A file sends
    `OpenPath` and focuses the editor. A folder sends `SetExpanded`
    with the opposite state and focuses the sidebar.
  - Left click on `TreeEmpty` focuses the sidebar.
  - Left press on `TreeBorder` starts a drag; `Drag(Left)` resizes;
    `Up(Left)` ends it.
  - Right click on `TreeRow(i)` selects the row and opens its menu at
    the mouse. Right click on `TreeEmpty` opens the folder menu for the
    workspace root. Either focuses the sidebar. The header has no menu,
    as in the GUI.
  - The wheel over `TreeRow`, `TreeEmpty` or `TreeBorder` scrolls the
    tree by `WHEEL_ROWS` rows, clamped so the last row stays at the
    bottom at most.
- **Menus** (exact GUI labels and order; `…` is U+2026, as in the
  editor menu):
  - File: `Open` (`OpenPath`), separator, `Copy path`,
    `Copy relative path`, `Reveal in file manager`, separator,
    `Rename…`, `Delete (move to trash)`.
  - Folder (and the root): `New file…` (`BeginNewFile`), `New folder…`
    (`BeginNewFolder`), separator, `Copy path`, `Copy relative path`,
    `Reveal in file manager`, separator, `Rename…`,
    `Delete (move to trash)`.
  - Every item is enabled. Delete has no confirmation, as in the GUI.
  - They reuse `ContextMenu<Command>` and `TermApp::menu`; a choice is
    sent as any other menu choice is.
- **Hints** (status-line text on hover):

  | Target         | Hint                                    |
  | -------------- | --------------------------------------- |
  | `TreeClose`    | `Close file tree`                       |
  | `TreeBorder`   | `Drag to resize (< / > in the tree)`    |
  | `TreeRow(_)`   | none                                    |
  | `TreeEmpty`    | none                                    |

- **Name prompts** already work (plan 02's `PromptUi` over
  `Prompt::Name`, with the GUI's labels). This plan only reaches them
  through the menus.
- **No terminal cursor** is drawn while the sidebar has focus (as
  today); the selected row's background shows where the keyboard is.
- **Not in this plan:**
  - Changing the GUI (see the cleanup entry 05.9).
  - Nerd Font or kitty icons (plan 08).
  - Shift+F10 / Menu for the editor menu.
  - Persisting the width.
  - Type-to-find in the tree.
  - Docs (plan 09).

## Execution model

All subtasks run **strictly in sequence** on
`feat/frext-term-file-tree`. Each subtask is one commit and leaves the
suite green (`commit-discipline`). The orchestrator reviews the full
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
- no bare `bool` parameters or fields for states (`state-representation`)

Benchmarks: no subtask touches a benchmarked path (`buffer`, `wrap`,
the GUI's `highlight`). The tree's per-render directory reads are a
deliberate, GUI-matching choice (see "Decided at activation").

## Subtasks

### 05.1 -- Core: `visible_rows`

Scope: `frext-core/src/workspace.rs`.

What:

- `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum RowKind {
  Dir(FolderState), File }`, with `FolderState` from
  `crate::editor::FolderState`.
- `#[derive(Debug, Clone, PartialEq, Eq)] pub struct TreeRow { pub
  path: PathBuf, pub depth: usize, pub kind: RowKind }`.
- `#[must_use] pub fn visible_rows(workspace: &Workspace) ->
  Vec<TreeRow>`: the rule under "Further decisions" (Rows). For each
  directory, `read_dir_split`; every sub-folder is a row with
  `Dir(Expanded)` when `workspace.is_expanded(&path)`, else
  `Dir(Collapsed)`, immediately followed by its own rows at `depth + 1`
  when expanded; then the files. An unreadable root gives no rows.

Deliverable: tests in `workspace.rs` over a temp directory (the
existing `read_dir_split` test's naming pattern):

- nothing expanded: the root's folders then files, all depth 0, folders
  `Dir(Collapsed)`
- one folder expanded: its children follow it at depth 1, before the
  next sibling, and it is `Dir(Expanded)`
- two levels expanded: depth 2 rows appear
- an expanded folder inside a collapsed one is not listed
- a missing root gives an empty list
- an empty expanded folder gives just its own row

Verification: the standard ritual.

Prohibitions: do NOT change `read_dir_split`, `Workspace` or the
editor; do NOT touch `frext` or `frext-term`; do NOT proceed to 05.2.

Stop: report files changed and verification; await review.

### 05.2 -- Icons: the `IconProvider` seam

Scope: `frext-term/src/icons.rs` (new), `frext-term/src/lib.rs`.

What:

- `lib.rs`: `pub mod icons;`. The module doc says the provider is the
  seam plan 08 extends with Nerd Font and kitty-image providers.
- `pub const ICON_WIDTH: u16 = 2;` (the slot, in cells).
- `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum IconKind {
  File, Dir(FolderState) }` (`frext_core::editor::FolderState`).
- `#[derive(Debug, Clone, PartialEq, Eq)] pub enum IconCell { Text {
  text: &'static str, color: Rgb } }` (`frext_core::palette::Rgb`). The
  doc says plan 08 adds an image variant, and that `text` is at most
  `ICON_WIDTH` cells wide.
- `pub trait IconProvider { fn icon(&self, name: &str, kind: IconKind)
  -> IconCell; }`
- `#[derive(Debug, Clone, Copy, Default)] pub struct AsciiIcons;`
  implementing it: any `Dir(_)` gives `Text { text: "/", color:
  palette::BLUE }`; `File` gives `Text { text: "", color:
  palette::TEXT }`. `name` is unused.

Deliverable: tests:

- `AsciiIcons` for a collapsed and an expanded folder, and for files
  named `main.rs` and `README`
- every `AsciiIcons` text fits in `ICON_WIDTH` (by `UnicodeWidthStr`)
- `AsciiIcons` used through `&dyn IconProvider`

Verification: the standard ritual.

Prohibitions: do NOT add Nerd Font or kitty providers; do NOT touch
`file_icon`; do NOT proceed to 05.3.

Stop: report and await review.

### 05.3 -- Hit targets and theme styles for the tree

Scope: `frext-term/src/hit.rs`, `frext-term/src/theme.rs`,
`frext-term/src/app.rs` (only to add the new targets to the existing
ignored arm of `handle_mouse`'s left-click match).

What:

- `hit::Target` gains `TreeClose`, `TreeRow(usize)`, `TreeEmpty` and
  `TreeBorder`. `Target::hint` returns the exact hints in the table
  under "Further decisions".
- `theme.rs`:
  - `#[must_use] pub const fn tree_active() -> Style`: fg
    `palette::accent()`, bg `MANTLE`, BOLD
  - `#[must_use] pub fn tree_cursor_bg() -> Color`:
    `palette::selection().over(SURFACE0)`
  - `#[must_use] pub const fn tree_border() -> Style`: bg `SURFACE0`
  - `#[must_use] pub const fn tree_border_active() -> Style`: bg
    `palette::accent()`
- `app.rs`: the four targets join the ignored arm (they are wired in
  05.7 and 05.8).

Deliverable: tests:

- each new target's hint (exact strings, `None` for the row and the
  empty area)
- a `HitMap` region for `TreeRow(3)` resolves
- the colours of the four theme helpers

Verification: the standard ritual.

Prohibitions: do NOT render anything; do NOT change `keymap.rs`; do
NOT proceed to 05.4.

Stop: report and await review.

### 05.4 -- Tree context menus

Scope: `frext-term/src/tree_menu.rs` (new), `frext-term/src/lib.rs`.

What:

- `lib.rs`: `pub mod tree_menu;`. The module doc says it mirrors the
  GUI's `file_context_menu` and `dir_context_menu` in `frext`.
- `#[must_use] pub fn file_entries(path: &Path) ->
  Vec<MenuEntry<Command>>` and `#[must_use] pub fn dir_entries(path:
  &Path) -> Vec<MenuEntry<Command>>`: exactly the items, labels, order,
  separators and commands under "Further decisions" (Menus), each
  command carrying `path.to_path_buf()`, every item
  `Enablement::Enabled`.
- `#[must_use] pub fn entries(path: &Path, kind: RowKind) ->
  Vec<MenuEntry<Command>>`: `dir_entries` for `RowKind::Dir(_)`,
  `file_entries` for `RowKind::File`.
- A private `item` helper as in `editor_menu.rs`.

Deliverable: tests that compare each list against a literal expected
`Vec` (as `editor_menu.rs` does), and that `entries` picks the right
list for `Dir(Expanded)`, `Dir(Collapsed)` and `File`.

Verification: the standard ritual.

Prohibitions: do NOT change `menu.rs` or `editor_menu.rs`; do NOT wire
the menus into the app; do NOT proceed to 05.5.

Stop: report and await review.

### 05.5 -- `FileTree`: state, refresh, width and rendering

Scope: `frext-term/src/tree.rs` (new), `frext-term/src/lib.rs`,
`frext-term/src/icons.rs` (the `Debug` supertrait only, added during
05.5).

What:

- `icons.rs`: `pub trait IconProvider: std::fmt::Debug`, so `FileTree`
  can own a provider and still derive `Debug`. Found during 05.5: with
  the provider as a `render` parameter, `render` had 8 arguments and
  failed `clippy::too_many_arguments`; owning it keeps the provider
  choice in one place for plan 08.
- `lib.rs`: `pub mod tree;`. The module doc describes the sidebar and
  the split of ownership (core owns the workspace, the tree owns the
  selection, scroll, width and drag).
- Constants: `pub const DEFAULT_WIDTH: u16 = 30;`,
  `pub const MIN_WIDTH: u16 = 12;`,
  `pub const MIN_EDITOR_WIDTH: u16 = 20;`,
  `pub const CLOSE_LABEL: &str = "[x]";`.
- `#[must_use] pub fn clamp_width(width: u16, total: u16) -> u16`
  (the rule under "Further decisions").
- `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum TreeFocus {
  Focused, Unfocused }`.
- `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum Refresh {
  Unchanged, Changed }`.
- A private `#[derive(Debug, Clone, Copy, PartialEq, Eq)] enum Resize {
  Idle, Dragging }`.
- `#[derive(Debug)] pub struct FileTree` with private fields:
  - `rows: Vec<TreeRow>`
  - `root: Option<PathBuf>` (the workspace root at the last refresh)
  - `selected: Option<PathBuf>`
  - `selected_index: usize` (the selection's last known index)
  - `top: usize`
  - `width: u16`
  - `total: u16` (the terminal width at the last render)
  - `area: Rect` (the sidebar at the last render)
  - `resize: Resize`
  - `icons: Box<dyn IconProvider>`
- Methods (with `Default` delegating to `new`):
  - `#[must_use] pub fn new() -> Self`: no rows, nothing selected,
    `width: DEFAULT_WIDTH`, `total: 0`, `Rect::default()`, `Idle`,
    `Box::new(AsciiIcons)`. Plan 08 adds a way to choose the provider.
  - `#[must_use] pub const fn width(&self) -> u16`
  - `#[must_use] pub fn effective_width(&self, total: u16) -> u16`
    returns `clamp_width(self.width, total)`.
  - `#[must_use] pub fn rows(&self) -> &[TreeRow]`,
    `#[must_use] pub fn row(&self, index: usize) -> Option<&TreeRow>`,
    `#[must_use] pub fn selected(&self) -> Option<&Path>`,
    `#[must_use] pub fn root(&self) -> Option<&Path>`.
  - `pub fn select(&mut self, index: usize)`: selects `rows[index]`
    when it exists, otherwise does nothing.
  - `pub fn refresh(&mut self, workspace: Option<&Workspace>) ->
    Refresh`: sets `root`; computes `visible_rows` (no rows without a
    workspace); returns `Unchanged` if they equal the old rows.
    Otherwise it stores them, repairs the selection (the rule under
    "Further decisions", Selection), clamps `top` to
    `rows.len().saturating_sub(1)`, and returns `Changed`.
  - `pub fn render(&mut self, editor: &Editor, focus: TreeFocus,
    hovered: Option<Target>, area: Rect, buf: &mut Buffer, hits: &mut
    HitMap)`, using `self.icons`:
    - Stores `area` and `total = buf.area.width`, then calls
      `refresh(editor.workspace())`.
    - Without a workspace, or with an empty `area`, draws nothing.
    - Otherwise draws the border, the header, the rows and the empty
      area, and pushes their targets, exactly as under "Further
      decisions" (Layout of the sidebar column, Row styles).
      `tree_border_active()` applies when `hovered ==
      Some(TreeBorder)` or the resize is `Dragging`.
    - Before drawing rows it clamps `top` to
      `rows.len().saturating_sub(rows_height)`, where `rows_height =
      area.height - 1`.
    - The active file: canonicalise the active tab's path once; a file
      row is active when its canonical path equals it. Only drawn rows
      are canonicalised.
    - Icon text is drawn with `theme::color(icon_color)` as fg over the
      row's background. Use `buf.set_stringn` with display widths so
      wide names are cut, never split.

Deliverable: `Buffer` tests over a real `Editor` with a workspace (copy
`prompt.rs`'s `temp_dir` / `open_editor` helpers; build a tree of
folders `src/` (with `main.rs`), `docs/`, and files `Cargo.toml`,
`README.md`):

- `clamp_width` at totals 0, 20, 32, 80, 200 and widths 0, 12, 30, 500
- header text `" [x] <display_root>"` and the `TreeClose` region over
  exactly the three `[x]` cells
- collapsed rows `" > / docs"`, `" > / src"`, `"     Cargo.toml"`,
  `"     README.md"`; after `editor.apply(SetExpanded { src, Expanded
  })` and a re-render, `" v / src"` followed by `"       main.rs"`
- the border column has `tree_border()` bg and a `TreeBorder` region on
  every row; with `hovered: Some(TreeBorder)` it has the active bg
- `TreeRow(i)` regions per drawn row and a `TreeEmpty` region below
- the active file's row uses `tree_active()` fg; the selected row's bg
  is `tree_cursor_bg()` only with `TreeFocus::Focused`; a hovered row
  has `hover_bg()`
- a name longer than the width is cut at the border, which stays drawn
- a 3-row-high sidebar with `top` scrolled shows rows from `top`
- `refresh`: `Unchanged` on a second call; `Changed` after a new file
  appears; the selection follows its path when a folder above it
  expands; moves to the collapsed parent when its folder collapses;
  falls back to the clamped index when its file is deleted; becomes
  `None` without a workspace
- no workspace or a zero-size area draws nothing and pushes nothing

Verification: the standard ritual.

Prohibitions: do NOT handle keys or the mouse (05.6); do NOT wire the
tree into the app; do NOT change core; do NOT proceed to 05.6.

Stop: report and await review.

### 05.6 -- `FileTree`: keys, wheel, resize and menu anchor

Scope: `frext-term/src/tree.rs`.

What:

- `#[derive(Debug, Clone, PartialEq, Eq)] pub enum TreeReply {
  Send(Command), OpenMenu { path: PathBuf, kind: RowKind, anchor:
  Position }, Resized, Handled, Ignored }`.
- `pub fn handle_key(&mut self, key: KeyEvent) -> TreeReply`: the key
  table and rules under "Further decisions" (Keys while the sidebar has
  focus), over the rows of the last refresh. Release events give
  `Ignored`. Motion keys give `Handled` (even with no rows). Right /
  Left / Enter give `Send(..)` when they produce a command, else
  `Handled`. `<` / `>` change the width by one column through
  `clamp_width(.., self.total)` and give `Resized` (also when the
  clamp leaves it unchanged). The menu keys give `OpenMenu` for the
  selected row, or, with no selection, for `root` with kind
  `Dir(Expanded)`; with neither, `Ignored`. Anything else is
  `Ignored`.
- `rows_height` for paging and scrolling is `area.height - 1` from the
  last render (at least 1 for the arithmetic).
- `#[must_use] pub fn menu_anchor(&self) -> Position`: for a selected
  row that is drawn, its name column, that is
  `x = area.x + 1 + 2 * depth + 2 + ICON_WIDTH` and
  `y = area.y + 1 + (index - top)`; otherwise
  `(area.x + 1, area.y + 1)`. `OpenMenu` uses it.
- `pub fn wheel(&mut self, rows: i32)`: moves `top` by `rows`, clamped
  to `0..=rows.len().saturating_sub(rows_height)`. The selection does
  not move.
- `pub fn begin_resize(&mut self)`, `pub fn end_resize(&mut self)`,
  `#[must_use] pub fn is_resizing(&self) -> bool` (an accessor over the
  private `Resize`, not stored state), and `pub fn drag_to(&mut self,
  column: u16)`: while `Dragging`, sets the width to
  `clamp_width(column.saturating_sub(area.x) + 1, total)`; otherwise
  does nothing.

Deliverable: tests over the 05.5 fixture (render once into a `Buffer`
to set `area` / `total`, then drive `handle_key`):

- Down from no selection selects row 0; Down / Up step and stop at the
  ends; Home / End; PageDown / PageUp by `rows_height - 1`; the
  selection scrolls into view in a short sidebar
- Right on collapsed `src` sends `SetExpanded { src, Expanded }`; after
  applying it and refreshing, Right selects `main.rs`; Right on a file
  is `Handled` and sends nothing
- Left on expanded `src` sends `Collapsed`; Left on `main.rs` selects
  `src`; Left at depth 0 is `Handled` and changes nothing
- Enter on a folder toggles; Enter on a file sends `OpenPath`
- Shift+F10, `F(22)` and Menu give `OpenMenu` with the row's path,
  kind and `menu_anchor()`; with no selection, the root as
  `Dir(Expanded)`
- `<` / `>` change `width()` by one and stop at `MIN_WIDTH` and at
  `total - MIN_EDITOR_WIDTH`; `<` with Shift works
- Ctrl+Down, Alt+Right and a plain `x` are `Ignored`; a Release is
  `Ignored`
- `wheel` clamps at both ends and leaves the selection
- `drag_to` only while resizing, clamped, and relative to `area.x`

Verification: the standard ritual.

Prohibitions: do NOT change rendering beyond what the anchor needs; do
NOT wire the tree into the app; do NOT proceed to 05.7.

Stop: report and await review.

### 05.7 -- App: render the tree and route its keys

Scope: `frext-term/src/app.rs`.

What:

- `TermApp` gains `tree: FileTree` (which owns the icon provider; see
  05.5). `SIDEBAR_WIDTH` is removed.
- `render`: `sidebar_width` is
  `self.sidebar_shown().then(|| self.tree.effective_width(area.width))`.
  The `paint::fill` of the sidebar is replaced, while the sidebar is
  shown, by `self.tree.render(&self.editor, focus, self.hovered,
  areas.sidebar, buf, &mut self.hits)` with
  `TreeFocus::Focused` iff `self.focus == Focus::Sidebar`.
- `handle_key`: when `action_for` returns `None` and focus is
  `Focus::Sidebar`:
  1. `if self.tree.refresh(self.editor.workspace()) == Refresh::Changed
     { self.forget_layout() }`
  2. match `self.tree.handle_key(key)`:
     - `Send(c)` goes to `send(c)`
     - `OpenMenu { path, kind, anchor }` sets `self.menu =
       Some(ContextMenu::new(tree_menu::entries(&path, kind), anchor))`
     - `Resized` calls `forget_layout()`
     - `Handled` / `Ignored` do nothing
- The module doc mentions the sidebar.

Deliverable: tests (update the two that assumed an empty sidebar;
build the workspace fixture as in 05.5 under the app tests' `ws`):

- `ctrl_b_hides_and_shows_the_sidebar` and
  `editing_keys_do_nothing_while_the_sidebar_has_focus` are updated to
  the drawn tree, keeping their intent (the column is there / gone; no
  text edit happens)
- the first frame shows the header and the root's rows
- Ctrl+E, Down, Down (onto `src`), Right: `src` is expanded in
  `editor().workspace()` and `main.rs` is drawn under it; Left
  collapses it
- Enter on `Cargo.toml` opens a tab for it, focus stays
  `Focus::Sidebar`, and its row is drawn with `tree_active()` fg
- Shift+F10 on `Cargo.toml` opens a menu whose first item is `Open`;
  choosing `Rename…` opens the rename prompt; typing a new name and
  Enter renames the file on disk and in the tree
- `>` widens the sidebar (the gutter's first column moves right by one)
  and `<` narrows it; neither goes below `MIN_WIDTH`
- Esc returns focus to the editor (unchanged)

Verification: the standard ritual.

Prohibitions: do NOT add mouse handling (05.8); do NOT change
`keymap.rs`, `tree.rs` or core; do NOT proceed to 05.8.

Stop: report and await review.

### 05.8 -- App: mouse on the tree

Scope: `frext-term/src/app.rs`.

What: the rules under "Further decisions" (Mouse on the sidebar):

- `Down(Left)` (no prompt): `TreeClose` sends `CloseWorkspace`.
  `TreeRow(i)`: take `self.tree.row(i).cloned()`, `self.tree.select(i)`;
  a file sends `OpenPath(path)` then `set_focus(Focus::Editor)`; a
  folder sends `SetExpanded` with the opposite state then
  `set_focus(Focus::Sidebar)`. `TreeEmpty` sets `Focus::Sidebar`.
  `TreeBorder` calls `self.tree.begin_resize()`. These replace the
  targets' place in the ignored arm.
- `Drag(Left)` while `self.tree.is_resizing()`: `drag_to(column)` then
  `forget_layout()` (after the existing hover update).
- `Up(Left)`: also `self.tree.end_resize()`.
- `Down(Right)` (no prompt): `Editor` is unchanged. `TreeRow(i)`
  selects the row and opens `tree_menu::entries(&row.path, row.kind)`
  at the mouse. `TreeEmpty` opens `tree_menu::dir_entries(root)` at the
  mouse, with the root from `editor.workspace()`. Both set
  `Focus::Sidebar`. Anything else does nothing.
- `ScrollUp` / `ScrollDown` (no prompt) over `TreeRow(_)`,
  `TreeEmpty` or `TreeBorder`: `self.tree.wheel(∓WHEEL_ROWS)` then
  `forget_layout()`.

Deliverable: tests (`left_click`, `right_click`, `mouse` helpers):

- clicking `[x]` closes the workspace and the sidebar disappears
- clicking a folder row expands it (persisted in the workspace) and
  again collapses it; focus is `Focus::Sidebar`
- clicking a file row opens it and focuses the editor
- right-clicking a file row opens the file menu at the mouse; a folder
  row the folder menu; the empty area the folder menu for the root;
  choosing `New file…` from the root menu opens the new-file prompt,
  and submitting a name creates and opens the file
- dragging the border from its column to column 39 makes the sidebar
  40 wide (the border sits under the mouse; the gutter moves), and a
  drag past the limits clamps
- the wheel over a long tree (40 files in a 24-row terminal) scrolls
  it, and stops at the end
- hovering `[x]` shows `Close file tree` on the status line
- with a prompt open, clicks on the tree do nothing

Verification: the standard ritual.

Prohibitions: do NOT change `tree.rs`, `keymap.rs` or core; do NOT
proceed past 05.8.

Stop: report and await review.

### 05.9 -- Cleanup (deferred): GUI disclosure-triangle click

Surfaced during activation (READ-ONLY audit of `frext/src/app.rs`
`tree_dir`, on `main` at `315c9f7`). Not in this plan's scope: this
plan does not change the GUI.

- **Surface point:** `tree_dir` sends `SetExpanded` only when
  `header.response.clicked()`, and discards the `_toggle` response of
  `CollapsingState::show_header`. If a click on egui's triangle toggles
  the live `CollapsingState` without the header response reporting a
  click, the expansion is not persisted and the GUI and
  `Workspace::expanded` disagree until restart.
- **Impact:** a folder opened with the triangle may reopen collapsed
  after a restart. Unverified.
- **Scope of fix:** `frext/src/app.rs` `tree_dir`.
- **Suggested approach:** first verify by hand (expand with the
  triangle, restart, check). If confirmed, send `SetExpanded` when the
  toggle response is clicked too, or derive the command from the
  change in `state.is_open()` across the frame; longer term, adopt
  `visible_rows` in the GUI.
- **Verification:** a regression test if egui's test harness allows,
  otherwise a manual check recorded here.
- **Scheduling:** any GUI-touching branch; independent of the TUI
  plans.

### 05.10 -- Review fixes (adversarial branch review)

Surfaced by a CODE-REVIEW pass over `main...18932b2`.

Scope: `frext-term/src/tree.rs`, `frext-term/src/app.rs`.

The fixes:

- **A -- the stored width follows the terminal** (bug + spec
  deviation). `render` stored `total` but never clamped `width`. After
  the terminal shrank, `<` started from the old, wider width, so the
  first press changed nothing on screen. Now `render` sets
  `self.width = clamp_width(self.width, self.total)` right after
  storing `total`, as "Width" requires. Test: drag to 60 in 80
  columns, render at 40 columns, then `<` once narrows the drawn
  sidebar by one column.
- **B -- a lost release cannot leave a drag stuck** (risk). The drag
  ended only on `Up(Left)`. Now `Moved` (sent only with no button
  held) and any `Down(Left)` not on `TreeBorder` also call
  `end_resize()`. Tests: start a drag, send `Moved`, and a later
  `Drag(Left)` does not resize; the same after a `Down(Left)` on the
  editor.
- **C -- hover and scroll after a refresh** (risk).
  - When the refresh inside `render` returns `Changed`, a hovered
    `TreeRow(_)` is ignored for that frame, because its index refers to
    the old rows.
  - `refresh` clamps `top` to `rows.len().saturating_sub(rows_height)`
    (the "at least 1" height from the last render) instead of
    `rows.len() - 1`. This keeps `menu_anchor` right for a menu opened
    in the same batch as the refresh.

  Tests: a hovered row index past a removed row does not paint
  `hover_bg()` on the row that moved into its place; after a collapse
  in a short sidebar, `top` is clamped by `refresh` alone.
- **D -- tests for paths nothing guarded**:
  - the `Refresh::Changed` branch of `TermApp::tree_key`: create a file
    on disk, press Down without rendering, and the hit map is empty
  - clicking `[x]` while the tree has focus returns focus to the editor
  - the Shift+F10 app test asserts a literal anchor position, not
    `menu_anchor()` read back

Not changed (recorded so they are not re-litigated):

- Renaming the selected entry moves the selection to the row now at
  the old index. That is the Selection rule; re-selecting the new path
  would need the rename's result, which core does not return.
- With the find bar open, Esc in the tree closes the search first, and
  the Alt toggles apply. That is the keymap's order from plan 04, not
  this plan's.
- `F(20)` (the Linux console's Shift+F10) is not bound; the key list is
  as decided.
- `menu_anchor` does not clamp a very deep row's indent; `ContextMenu`
  clamps the menu onto the screen.
- `total` and `area` go stale while the sidebar is hidden; the next
  render corrects them, and A clamps the width then.

Verification: the standard ritual.

## Manual smoke checklist

Run in a real terminal (kitty or freminal, then a legacy terminal):

1. `frext-term <dir>` shows the header with the root and `[x]`, and
   the root's folders then files.
2. Ctrl+E focuses the tree, and the selected row is highlighted.
   Up/Down/PageUp/PageDown/Home/End move it, and the view follows.
3. Right / Left / Enter expand and collapse; expansion survives a
   restart.
4. Enter on a file opens it and keeps focus in the tree; a click on a
   file opens it and focuses the editor. The active file is
   highlighted.
5. Right-click (and Shift+F10 / Menu) opens the right menu; rename,
   new file, new folder, copy path and trash all work, and the prompt
   reports errors (an existing name).
6. Dragging the border and `<` / `>` resize the sidebar within its
   limits.
7. The wheel scrolls a long tree. `[x]` closes the tree, and Ctrl+B
   hides and shows it.
