# 08 -- TUI file icons (kitty images, Nerd Font)

- **Status:** stub
- **Depends on:** 05, 06

## Goal

The sidebar shows the same Catppuccin file-type icons as the GUI when
the terminal supports kitty graphics with unicode placeholders, and
falls back to coloured Nerd Font glyphs (or ASCII) otherwise, selected
by `TerminalCaps` and the `icons` override from plan 06.

## Task summary

- Extend plan 05's `IconProvider` with three implementations:
  - `KittyIcons`: the vendored SVG for the icon stem (same
    `frext_core::file_icon` lookup as the GUI) rasterised and drawn as a
    kitty image in a 2x1-cell slot.
  - `NerdIcons`: a Nerd Font glyph plus a Catppuccin colour per stem.
  - `AsciiIcons` (already shipped in 05) and `None`.
- Selection: `icons = auto` -> `KittyIcons` when
  `TerminalCaps::images == KittyPlaceholders`, otherwise `NerdIcons`.
  Forced modes come from plan 06's config and CLI.
- A generated `nerd_icon_table.rs` keyed by the same icon stems as
  `file_icon_table.rs` (about 620), mapping to `(glyph, Rgb)`, with a
  generic fallback glyph for unmapped stems, and its generator.

## Durable decisions

### Kitty rendering

frext writes its own small kitty renderer (roughly 200 lines, all
unit-testable against byte output) rather than using ratatui-image's:

- **Rasterising**: `resvg` renders the SVG to RGBA at
  `2 * cell_px.width` x `cell_px.height` (about square, since cells are
  roughly 1:2). Pin `resvg` / `tiny-skia` to the versions `egui_extras`
  already pulls in, so the workspace carries one copy.
- **Upload once per stem, lazily**:
  `a=T,U=1,f=32,s=<w>,v=<h>,i=<id>,c=2,r=1,q=2`, where `s` / `v` are
  the rasterised RGBA width / height in pixels (see Rasterising); raw
  `f=32` data requires them (`c` / `r` are only the placement size in
  cells), and with `q=2` a missing size would fail silently, so they
  are spelled out. Base64 in 4096-byte chunks (`m=1` ... `m=0`), direct
  transfer `t=d`.
  `U=1` creates a virtual placement for unicode placeholders. `q=2`
  suppresses all replies so none land in the input stream as garbage
  key events. Uploads are written out-of-band through
  `terminal.backend_mut()` before the frame that first needs the icon.
  Direct transfer is used because freminal, for example, refuses
  file (`t=f`) and shared-memory (`t=s`) transfers unless the user
  enables them.
- **IDs**: sequential 24-bit image ids, so the whole id fits in the
  cell's foreground colour and the id-high-byte diacritic is never
  needed.
- **Placeholders carry explicit diacritics on every cell**: each icon
  cell is `U+10EEEE` + row diacritic + column diacritic, with fg =
  `Rgb(id)`. ratatui-image writes diacritics only on a row's first cell
  and relies on inheritance, which breaks in two ways for frext:
  ratatui's diff renderer may redraw only the second cell of an icon,
  and freminal resolves inheritance from the last placeholder it
  processed rather than the cell to its left (a deviation from the
  kitty spec). Explicit diacritics make both irrelevant. Per-cell
  grapheme: 4 + 2 + 2 bytes, within freminal's 16-byte cap.
- **Cleanup**: on exit, delete every uploaded image (`a=d,d=I,i=<id>`).
  Placements vanish with their placeholders automatically.
- **Transparency**: images keep their alpha so the active-file row
  highlight shows through.
- **tmux**: uploads are wrapped for passthrough (plan 06). Placeholders
  are plain text and pass through tmux unchanged, which is what unicode
  placeholders were designed for.

### Nerd Font fallback

- No terminal query reveals the font. Nerd Font glyphs are
  private-use codepoints with the same width whether or not the font
  has them, so a cursor-position trick cannot tell either. `nerd` is the
  `auto` fallback; users without a Nerd Font set `icons = "ascii"`.
- Glyphs are monochrome, so the colour comes from the table (Catppuccin
  Mocha colours, matching the SVG artwork's hue where practical).

## Decide at activation

- Source data for the Nerd Font table: map from `nvim-web-devicons`
  (extension/filename -> glyph + colour; Catppuccin publishes palette
  overrides for it) onto frext's stems, or hand-curate per stem. Also
  where the generator lives (`xtask` or a script) and how it is run.
- Where `nerd_icon_table.rs` lives: `frext-term` (only consumer,
  proposed) or `frext-core`.
- Behaviour when the cell size changes mid-session (font zoom):
  re-rasterise and re-upload on resize, or keep the stale size.
- Whether to also show icons on tabs (the GUI does not today, so the
  default is no).

## Follow-up outside this repo

File a cleanup entry in freminal: unicode-placeholder inheritance
should resolve from the cell to the left (same row), per the kitty
spec, rather than from the last processed placeholder, and a CR/LF
should reset it. frext does not depend on this fix.
