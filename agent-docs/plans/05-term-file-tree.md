# 05 -- TUI file-tree sidebar

- **Status:** stub
- **Depends on:** 02

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
- Header row: the root via `Workspace::display_root()` and a close `×`
  (`Command::CloseWorkspace`).
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

## Decide at activation

- Whether the sidebar width persists in the TUI session (the GUI's
  width is remembered by eframe, not by frext's session).
- Keybinding to focus or toggle the sidebar (coordinate with plan 02's
  focus model).
- Whether to re-read directories every frame (as the GUI does) or cache
  listings and refresh on a tick.
