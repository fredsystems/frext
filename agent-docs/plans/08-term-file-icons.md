# 08 -- TUI file icons (kitty images, Nerd Font)

- **Status:** active
- **Depends on:** 05, 06
- **Branch:** `feat/frext-term-file-icons` (one branch, one commit per
  subtask, one PR)

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
  - `AsciiIcons` (already shipped in 05) and `NoIcons`.
- Selection: `icons = auto` -> `KittyIcons` when
  `TerminalCaps::images == KittyPlaceholders`, otherwise `NerdIcons`.
  Forced modes come from plan 06's config and CLI.
- A generated `nerd_table.rs` keyed by the same icon stems as
  `file_icon_table.rs` (623), mapping to `(glyph, Rgb)`, and its
  generator.

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

## Decided at activation

- **Nerd table source and generator.** A new `xtask` workspace crate,
  run as `cargo xtask nerd-icons <nvim-web-devicons checkout>`. Each
  stem's glyph is the majority vote of `nvim-tree/nvim-web-devicons`'
  extension and filename glyphs, mapped onto frext's stems through
  `frext_core::file_icon`'s own lookups; its colour is the dominant
  colour of that stem's vendored Catppuccin Mocha SVG, so it matches
  the artwork exactly. Folder stems get the open / closed folder
  glyphs; anything unvoted gets a generic file glyph. The generated
  header records the devicons commit.
- **Table location**: `frext-term/src/icons/nerd_table.rs`. `frext-term`
  is the only consumer.
- **Cell size changes mid-session** (font zoom): keep the startup size.
  The terminal scales each image to its 2x1 placeholder slot, so icons
  stay placed correctly, only slightly soft or sharp. There is no
  mid-session probe: a `CSI 16 t` reply would arrive as garbage input.
- **Tabs**: no icons, matching the GUI.
- **`icons = "none"`**: the 2-cell slot stays, blank, so the layout,
  hit regions and menu anchors are the same in every mode.

## Further decisions (orchestrator)

These come from reading the current code: `frext-term`'s `icons.rs`,
`tree.rs` (`FileTree`, `render`, `draw_row`), `app.rs` (`TermApp::new`,
`render`, `shutdown`), `run.rs`, `main.rs`, `terminal.rs`,
`term/{caps,backend,tmux}.rs`; `frext-core`'s `file_icon.rs`,
`file_icon_table.rs`, `file_icon_bytes.rs` and `palette.rs`; the GUI's
SVG loading; ratatui-core 0.1.2's `Terminal::try_draw`; crossterm
0.29's `Colored`; `Cargo.lock`; `flake.nix`; and nvim-web-devicons'
`icons_by_file_extension.lua` / `icons_by_filename.lua`.

- **Facts from the audit.**
  - A stem is a `&'static str` (the SVG file name without `.svg`).
    `file_icon` exposes only `icon_for_file(name)`,
    `icon_for_dir(name, expanded: bool)` and `icon_bytes(stem)`; the
    tables are private. `ICON_BYTES` is sorted by stem and holds 623
    stems. The defaults are `_file`, `_folder` and `_folder_open`;
    every folder stem is `_folder`, `_folder_open`, or starts with
    `folder_` (open variants end in `_open`).
  - The existing file-icon tables have **no generator in the repo**.
  - `resvg` 0.45.1 (with `usvg` 0.45.1, `tiny-skia` 0.11.4) is pulled
    in only by `egui_extras` 0.37.0, with `default-features = false`.
    `base64` 0.22.1 is already in `Cargo.lock` (via crossterm and
    usvg). Neither is a workspace dependency yet.
  - `IconProvider::icon` takes `&self` and returns `IconCell`, which
    has only `Text`. `FileTree` owns `icons: Box<dyn IconProvider>`
    (always `AsciiIcons`); `draw_row` destructures `IconCell::Text`.
    `main.rs` computes `IconChoice::select(settings.icons,
    guard.caps())` and only logs it.
  - `Terminal::draw` gives no point between filling the buffer and
    writing the diff. ratatui-core 0.1.2 exposes the pieces:
    `autoresize()`, `get_frame()`, `backend_mut()` and
    `apply_buffer_with_cursor(Option<Position>)`. `Frame`'s cursor
    field is `pub(crate)`, so the app must return its cursor itself.
  - `FrextBackend<W>` implements `Write`; `TestBackend` does not.
    `Terminal::with_options(.., Viewport::Fixed(rect))` never calls
    `Backend::size`, so a `FrextBackend<Vec<u8>>` terminal works in
    tests.
  - crossterm drops every colour when `NO_COLOR` is set
    (`Colored::ansi_color_disabled()`), which would erase the image id
    carried in the placeholder's foreground.
  - nvim-web-devicons' data files are one entry per line:
    `["rs"] = { icon = "", color = "#DEA584", cterm_color = "216",
    name = "Rs" },`.
- **The provider seam.**

  ```rust
  pub trait IconProvider: std::fmt::Debug {
      fn icon(&mut self, name: &str, kind: IconKind) -> IconCell;
      fn take_uploads(&mut self) -> Vec<u8> { Vec::new() }
      fn release(&mut self) -> Vec<u8> { Vec::new() }
  }
  ```

  `icon` takes `&mut self` so `KittyIcons` can allocate ids and queue
  uploads without interior mutability (`FileTree::render` borrows
  `self.rows` and `self.icons` disjointly). `take_uploads` returns the
  bytes queued by `icon` since the last call; `release` returns the
  bytes that delete everything uploaded and forgets it.
  `IconCell::Image { id: ImageId }` is the new variant.
- **Kitty bytes** (`term/kitty.rs`), with `ESC _ G` ... `ESC \` around
  each command:
  - one chunk: `a=T,U=1,f=32,t=d,s=<w>,v=<h>,i=<id>,c=2,r=1,q=2;<b64>`
    (no `m` key);
  - several chunks (base64 split every 4096 bytes): the first is the
    same keys plus `,m=1`, then `m=1,q=2;<b64>` for each middle chunk
    and `m=0,q=2;<b64>` for the last;
  - delete: `a=d,d=I,i=<id>,q=2` (with `q=2`, like every command, so no
    reply reaches the input stream);
  - every command is wrapped separately with `tmux::wrap_for`.
- **Placeholder cells**: column 0 is `"\u{10EEEE}\u{305}\u{305}"`,
  column 1 is `"\u{10EEEE}\u{305}\u{30D}"` (row 0; columns 0 and 1 of
  kitty's diacritics table). Foreground
  `Color::Rgb(id >> 16, (id >> 8) & 0xff, id & 0xff)`. Ids run
  `1..=0xFF_FFFF`.
- **Rasterising**: the SVG is drawn into a `2 * cell_w` x `cell_h`
  transparent canvas, scaled to fit a centred square whose side is the
  smaller of the two, and returned as straight (not premultiplied)
  RGBA, which is what `f=32` means. A stem whose SVG does not parse is
  drawn with the Nerd glyph instead.
- **Drawing order.** `run::draw` replaces `terminal.draw`:
  `autoresize`, `get_frame`, `app.render_frame(&mut frame)` (which
  returns the cursor), `app.take_graphics()` written with
  `terminal.backend_mut().write_all`, then
  `apply_buffer_with_cursor(cursor)`. So every upload precedes, in the
  byte stream, the first frame that shows its placeholders. On exit
  `main` writes `app.release_graphics()` through the backend and
  flushes before the terminal is dropped.
- **`NO_COLOR`**: a `Kitty` choice with ANSI colours disabled builds
  `NerdIcons` instead, with a `log::warn!`.
- **Nerd fallbacks**: closed folder `U+E5FF`, open folder `U+E5FE`,
  file `U+F15B`.
- **Default members.** The workspace gains
  `default-members = ["frext-core", "frext", "frext-term"]`, so a plain
  `cargo build` (and the Nix package build) never builds or installs
  the xtask. `cargo test --all` still covers it.

## Execution model

All subtasks run **strictly in sequence** on
`feat/frext-term-file-icons`. Each subtask is one commit and leaves the
suite green (`commit-discipline`). The orchestrator reviews the full
diff of each subtask before committing it. Implementers do not commit,
merge or push.

Each subtask's verification includes the standard ritual, run inside
`nix develop`:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --all
cargo machete
```

`--workspace` is needed from 08.2 on: with `default-members` set, a
plain `cargo clippy` skips `xtask` (the pre-commit clippy hook already
passes `--workspace`).

All code follows the workspace rules:

- no `unwrap` / `expect` outside tests
- no `unsafe`
- no indexing that can panic (use `get`, or ranges proven in bounds)
- every public item tested
- doc comments on every public item
- no bare `bool` parameters or fields for states (`state-representation`)
- no dependency beyond those this plan names

Benchmarks: no subtask touches a benchmarked path (`benches/buffer.rs`
and `benches/wrap.rs` cover the buffer view only).

Tests that check escape bytes call crossterm's
`Colored::set_ansi_color_disabled(false)` first, as
`term/backend.rs`'s tests do, so `NO_COLOR` in the environment cannot
break them.

## Subtasks

### 08.1 -- Core: list every icon stem

Scope: `frext-core/src/file_icon.rs`.

What: `#[must_use] pub fn stems() -> impl Iterator<Item = &'static
str>`, documented: every bundled icon stem, sorted, each resolvable by
`icon_bytes`. It iterates `ICON_BYTES`.

Deliverable: tests: `stems()` yields 623 stems, strictly increasing;
each has `icon_bytes(stem).is_some()`; it contains `_file`, `_folder`
and `_folder_open`; every stem `icon_for_file` / `icon_for_dir` returns
for a sample of names (`main.rs`, `Cargo.toml`, `README.md`, `x.unknown`,
`src` open and closed, `.cargo` open and closed) is among them.

Verification: the standard ritual.

Prohibitions: do NOT touch the generated tables or any other file;
do NOT proceed to 08.2.

Stop: report files changed and verification; await review.

### 08.2 -- xtask: the Nerd table generator

Scope: `Cargo.toml` (workspace `members`, new `default-members`),
`.cargo/config.toml` (new), `xtask/Cargo.toml` (new),
`xtask/src/main.rs` (new), `xtask/src/nerd_icons.rs` (new),
`Cargo.lock`, and `.gitignore` (`!.cargo/config.toml`, since a bare
`config.toml` pattern ignored it).

What:

- Workspace: `members` gains `"xtask"`;
  `default-members = ["frext-core", "frext", "frext-term"]`.
- `.cargo/config.toml`: `[alias] xtask = "run --package xtask --"`.
- `xtask/Cargo.toml`: `name = "xtask"`, `publish = false`, the same
  `*.workspace = true` package fields and `[lints] workspace = true` as
  `frext-term/Cargo.toml`; dependencies `frext-core`, `regex`,
  `thiserror` (all `workspace = true`), alphabetical.
- `xtask/src/main.rs`: parses `nerd-icons <devicons-dir> [<out>]`;
  `<out>` defaults to `frext-term/src/icons/nerd_table.rs` under the
  workspace root (`CARGO_MANIFEST_DIR`'s parent). Reads
  `<devicons-dir>/lua/nvim-web-devicons/default/icons_by_file_extension.lua`
  and `icons_by_filename.lua`, gets the commit with
  `git -C <devicons-dir> rev-parse HEAD`, writes the table, and prints
  a usage message and exits non-zero on a bad command line or any
  error. Errors are a `thiserror` enum; no `unwrap` / `expect`.
- `xtask/src/nerd_icons.rs`, pure and unit-tested:
  - `pub fn parse_devicons(lua: &str) -> Vec<(String, String)>`: every
    `["<key>"] = { icon = "<glyph>", ...` line as `(key, glyph)`;
    entries whose glyph is not exactly one `char` are skipped.
  - `pub fn votes(extensions, filenames) -> BTreeMap<&'static str,
    String>`: each extension key `k` votes its glyph for
    `icon_for_file(&format!("x.{k}"))`, each filename key for
    `icon_for_file(k)`; votes for `_file` are dropped. A stem's glyph
    is the one with the most votes, ties broken by the smallest glyph.
  - `pub fn dominant_colour(svg: &str) -> Rgb`: the most frequent
    `#rrggbb` or `#rgb` colour (case-insensitive, `#rgb` expanded),
    ties broken by first occurrence; `palette::TEXT` when there is
    none.
  - `pub fn render_table(commit: &str, entries: &[(&str, char, Rgb)])
    -> String`: the file below, one entry per stem in `stems()` order,
    each glyph written as a `\u{..}` escape (lowercase hex) and each
    colour as `Rgb::new(0x.., 0x.., 0x..)`.
  - `pub fn entries(...)`: for every stem of `file_icon::stems()`:
    `_folder` and closed `folder_*` stems get `U+E5FF`, `_folder_open`
    and `folder_*_open` stems `U+E5FE`; others their vote, else
    `U+F15B`; the colour is `dominant_colour` of the stem's SVG
    (`icon_bytes`, as UTF-8).

  The generated file:

  ```rust
  //! Generated Nerd Font icon table: icon stem to glyph and colour.
  //!
  //! Glyphs from `nvim-tree/nvim-web-devicons` commit `<sha>`,
  //! mapped onto frext's icon stems; colours from each stem's Catppuccin
  //! Mocha SVG. Generated by `cargo xtask nerd-icons <checkout>`; do
  //! not edit by hand.

  use frext_core::palette::Rgb;

  /// Every icon stem as `(stem, glyph, colour)`, sorted by stem.
  pub const NERD_ICONS: &[(&str, &str, Rgb)] = &[
      ("3d", "\u{f01a7}", Rgb::new(0xa6, 0xe3, 0xa1)),
  ];
  ```

  An entry whose three fields are wider than 60 characters (rustfmt's
  `fn_call_width`) is written one field per line, as rustfmt lays it
  out, so the generated file is `cargo fmt` clean as written.

Deliverable: unit tests: parsing two real-format lines plus a comment
and a `return {` line; a multi-char glyph skipped; a majority and a
tie; `_file` votes dropped; `rs` reaching the `rust` stem;
`dominant_colour` on `#rgb`, mixed case, a tie and no colour; folder
stem glyphs; `render_table` output for two entries exactly; every
rendered line at most 100 characters; argument parsing (missing dir,
unknown command, default out path).

Verification: the standard ritual; `cargo build` at the root does not
build `xtask` (check `cargo build -v` lists no `xtask` unit, or
`cargo metadata` default members).

Prohibitions: do NOT generate the table or create
`frext-term/src/icons/` (08.3); do NOT touch `frext-term` or
`flake.nix`; do NOT proceed to 08.3.

Stop: report files changed and verification; await review.

### 08.3 -- Icons: module split and the generated table

Scope: `frext-term/src/icons.rs` moved (`git mv`) to
`frext-term/src/icons/mod.rs`, `frext-term/src/icons/nerd_table.rs`
(new, generated), `flake.nix` (`extraExcludes` only).

What:

- Clone `https://github.com/nvim-tree/nvim-web-devicons` into
  `/tmp/opencode/nvim-web-devicons` and check out
  `0dc0cfb0c6858041174a53c7aa4adaf65e994b3c`. Run
  `cargo xtask nerd-icons /tmp/opencode/nvim-web-devicons`. The table
  is committed exactly as generated.
- `icons/mod.rs`: `pub mod nerd_table;` and nothing else new.
- `flake.nix`: add `"^frext-term/src/icons/nerd_table\\.rs$"` to
  `extraExcludes` beside the file-icon tables, under their comment
  (generated, so not ours to lint).

Deliverable: tests in `icons/mod.rs`: `NERD_ICONS` stems equal
`file_icon::stems()` exactly and in order; every glyph is one `char`;
`rust`, `_folder`, `_folder_open` and `_file` have the expected glyphs
(`_folder` is `U+E5FF`, `_folder_open` `U+E5FE`, `_file` `U+F15B`).

Verification: the standard ritual; `nix flake check` evaluation is not
required, but `nixfmt --check flake.nix` and `statix check` must pass.

Prohibitions: do NOT hand-edit the generated table; do NOT add
providers (08.6); do NOT proceed to 08.4.

Stop: report files changed, the devicons commit used, verification;
await review.

### 08.4 -- Term: kitty graphics encoding

Scope: `frext-term/src/term/kitty.rs` (new), `frext-term/src/term/mod.rs`,
`Cargo.toml` (workspace `base64 = "0.22.1"`), `frext-term/Cargo.toml`,
`Cargo.lock`.

What:

- `#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)] pub struct
  ImageId(u32)` with `pub const fn new(id: u32) -> Option<Self>`
  (`None` outside `1..=0xFF_FFFF`), `pub const fn get(self) -> u32`,
  `pub const fn next(self) -> Option<Self>`, and `#[must_use] pub const
  fn color(self) -> ratatui::style::Color` as decided.
- `pub const PLACEHOLDER_CELLS: [&str; 2]` as decided, documented.
- `pub const CHUNK: usize = 4096;`
- `#[must_use] pub fn upload(id: ImageId, width: u32, height: u32,
  rgba: &[u8], multiplexer: Multiplexer) -> Vec<u8>` and
  `#[must_use] pub fn delete(id: ImageId, multiplexer: Multiplexer) ->
  Vec<u8>`, producing the bytes decided under "Kitty bytes".
- `term/mod.rs`: `pub mod kitty;` and a module-doc mention.

Deliverable: tests: exact bytes of a 1x1 upload; a payload of exactly
4096 base64 bytes is one chunk; one byte more is two (`m=1` with all
keys, then `m=0,q=2`); three chunks have a middle `m=1,q=2`; each chunk
is wrapped separately under tmux; exact delete bytes, plain and tmux;
`ImageId` bounds, `next` at the top, and `color` for `0x123456`; each
placeholder cell is 8 bytes and `unicode_width` 1.

Verification: the standard ritual.

Prohibitions: do NOT touch icons or the tree; do NOT proceed to 08.5.

Stop: report files changed and verification; await review.

### 08.5 -- Icons: SVG rasteriser

Scope: `frext-term/src/icons/raster.rs` (new),
`frext-term/src/icons/mod.rs` (`pub mod raster;` only), `Cargo.toml`
(workspace `resvg = { version = "0.45.1", default-features = false }`),
`frext-term/Cargo.toml`, `Cargo.lock`.

What: `#[must_use] pub fn rasterize(svg: &[u8], width: u32, height:
u32) -> Option<Vec<u8>>`, documented: straight RGBA, `width * height *
4` bytes, the icon fitted to the centred square as decided; `None` when
either side is zero or the SVG does not parse. Uses `resvg::usvg` and
`resvg::tiny_skia` (no separate dependency); demultiplies with
`PremultipliedColorU8::demultiply`. `Cargo.lock` must still hold a
single `resvg`, `usvg` and `tiny-skia`.

Deliverable: tests: the `rust` icon at 20x20 has the right length,
transparent corners and an opaque pixel of `#fab387`; at 40x20 the
left and right 10 columns are fully transparent; a synthetic SVG with a
`#ff0000` rect at `fill-opacity="0.5"` gives `(255, 0, 0, ~128)`
(straight alpha); garbage bytes and a zero side give `None`.

Verification: the standard ritual; `grep -c 'name = "resvg"'
Cargo.lock` is 1.

Prohibitions: do NOT add providers; do NOT proceed to 08.6.

Stop: report files changed and verification; await review.

### 08.6 -- Icons: the seam, `NerdIcons`, `NoIcons`, image drawing

Scope: `frext-term/src/icons/mod.rs`, `frext-term/src/icons/nerd.rs`
(new), `frext-term/src/tree.rs`.

What:

- `icons/mod.rs`: the trait as decided under "The provider seam";
  `IconCell::Image { id: ImageId }` (documented: drawn as
  `PLACEHOLDER_CELLS` across the slot); `#[derive(Debug, Clone, Copy,
  Default)] pub struct NoIcons;` (empty text in `palette::TEXT` for
  every kind); `AsciiIcons` adapted to `&mut self`. The module doc
  lists the four providers, and the "plan 08 adds / extends" wording
  in it and on `IconCell` goes. `pub mod nerd;`.
- `icons/nerd.rs`: `#[derive(Debug, Clone, Copy, Default)] pub struct
  NerdIcons;` looking up `icon_for_file(name)` or
  `icon_for_dir(name, state == Expanded)` in `NERD_ICONS` by binary
  search, giving `Text { glyph, colour }`; an unlisted stem gets the
  decided fallbacks in `palette::TEXT` (file) or `palette::BLUE`
  (folders). `#[must_use] pub fn nerd_icon(stem: &str, kind: IconKind)
  -> IconCell` is the shared lookup (08.7 reuses it).
- `tree.rs`: `draw_row` takes `&mut dyn IconProvider`; for `Image {
  id }` it writes `PLACEHOLDER_CELLS[i]` into each slot cell `i` that
  fits (`Buffer::cell_mut`, no indexing), styled `style.fg(id.color())`
  so the row background stays. `FileTree` gains `pub fn
  set_icons(&mut self, icons: Box<dyn IconProvider>)`, `pub fn
  take_uploads(&mut self) -> Vec<u8>` and `pub fn release_icons(&mut
  self) -> Vec<u8>`, each documented.

Deliverable: tests: `NerdIcons` for `main.rs`, `Cargo.toml`, a closed
and an open folder, and an unknown name (`_file`'s glyph); `nerd_icon`
for a stem not in the table; `NoIcons` for every kind; the existing
`AsciiIcons` tests; a tree test with a fake provider returning
`Image { id }` for `0x010203`: both slot cells hold the placeholder
symbols, fg `Rgb(1, 2, 3)`, and the selected row keeps its background;
with the sidebar one cell too narrow only the first placeholder is
drawn; `take_uploads` / `release_icons` pass through the provider's
bytes.

Verification: the standard ritual.

Prohibitions: do NOT add `KittyIcons` or change app / run / main; do
NOT proceed to 08.7.

Stop: report files changed and verification; await review.

### 08.7 -- Icons: `KittyIcons`

Scope: `frext-term/src/icons/kitty.rs` (new),
`frext-term/src/icons/mod.rs` (`pub mod kitty;` only).

What: `#[derive(Debug)] pub struct KittyIcons` holding the cell size,
the multiplexer, the uploaded stems
(`HashMap<&'static str, Upload>` with a private `enum Upload {
Image(ImageId), Failed }`), the next id, the queued bytes and a
`Rasterizer` (`pub(crate) type Rasterizer = fn(&[u8], u32, u32) ->
Option<Vec<u8>>`).

- `#[must_use] pub fn new(cell_px: CellPx, multiplexer: Multiplexer)
  -> Self` uses `raster::rasterize`; `pub(crate) fn with_rasterizer(..)`
  takes one, for tests.
- `icon`: the stem as `NerdIcons` finds it; a known `Image` gives
  `IconCell::Image`; `Failed` gives `nerd_icon(stem, kind)`; an unseen
  stem is rasterised at `2 * width` x `height` and, on success, given
  the next id with its `kitty::upload` bytes queued; on failure (or ids
  exhausted, or no bytes for the stem) it is marked `Failed` with one
  `log::warn!`.
- `take_uploads`: drains the queue. `release`: `kitty::delete` for
  every uploaded id in id order, then forgets every stem and empties
  the queue.

Deliverable: tests (decoding nothing; assert on the bytes'
structure): the first `main.rs` queues one upload with `i=1`, `s=20`,
`v=20` for a 10x20 cell; a second `main.rs` and `lib.rs` (same stem)
queue nothing; `Cargo.toml` gets `i=2`; `take_uploads` then returns
empty; under tmux the upload is wrapped; a failing rasterizer gives
the Nerd glyph, warns once and never queues; `release` deletes `i=1`
then `i=2`, a second `release` is empty, and after it the same stem is
uploaded again with a fresh id.

Verification: the standard ritual.

Prohibitions: do NOT wire the provider into the app; do NOT proceed
to 08.8.

Stop: report files changed and verification; await review.

### 08.8 -- Wiring: choose the provider, write uploads before the frame

Scope: `frext-term/src/icons/mod.rs`, `frext-term/src/app.rs`,
`frext-term/src/run.rs`, `frext-term/src/main.rs`.

What:

- `icons/mod.rs`: `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub
  enum AnsiColours { Enabled, Disabled }` with `#[must_use] pub fn
  from_env() -> Self` (crossterm's `Colored::ansi_color_disabled()`),
  and `#[must_use] pub fn provider(choice: IconChoice, multiplexer:
  Multiplexer, colours: AnsiColours) -> Box<dyn IconProvider>`:
  `Kitty` gives `KittyIcons`, or `NerdIcons` with a `log::warn!` under
  `Disabled`; `Nerd`, `Ascii` and `None` give `NerdIcons`,
  `AsciiIcons` and `NoIcons`.
- `app.rs`: `pub fn render_frame(&mut self, frame: &mut Frame) ->
  Option<Position>` is today's `render` returning the cursor instead
  of setting it; `render` calls it and sets the cursor (unchanged for
  callers). `pub fn set_icons`, `pub fn take_graphics(&mut self) ->
  Vec<u8>` and `pub fn release_graphics(&mut self) -> Vec<u8>` pass
  through to the tree.
- `run.rs`: `pub fn draw<B: Backend + Write>(app: &mut TermApp,
  terminal: &mut Terminal<B>) -> io::Result<()>` as decided under
  "Drawing order"; `run` takes `B: Backend + Write` and calls it.
  `run`'s tests move to `Terminal::with_options(FrextBackend::new(Vec::new(),
  UnderlineStyle::Straight), TerminalOptions { viewport:
  Viewport::Fixed(Rect::new(0, 0, 80, 24)) })`.
- `main.rs`: builds the provider from `IconChoice::select(..)`,
  `guard.caps().multiplexer` and `AnsiColours::from_env()`, logs the
  choice as today, calls `app.set_icons`; after `run`, writes
  `app.release_graphics()` and flushes through `terminal.backend_mut()`
  (an error is logged, not returned) before dropping the terminal.

Deliverable: tests: `provider` for each choice (its `Debug` output
names the type) and the `Disabled` kitty fallback; `render_frame`
returns the cursor `render` sets; `draw` with a workspace holding
`main.rs` and `KittyIcons`: the upload's bytes come before the first
placeholder's bytes in the backend's output, a second `draw` uploads
nothing, and `release_graphics` then holds the delete for `i=1`; with
`AsciiIcons` `draw`'s output equals `terminal.draw`'s for the same
frame; the existing `run` tests pass on the new backend.

Verification: the standard ritual.

Prohibitions: do NOT change the probe, caps or `IconChoice::select`; do
NOT touch the tab bar; do NOT proceed to 08.9.

Stop: report files changed and verification; await review.

### 08.9 -- Docs: attributions

Scope: `ATTRIBUTIONS.md`, `assets/licenses/nvim-web-devicons-LICENSE`
(new).

What:

- An "Icon data" section: the Nerd Font glyph mapping in
  `frext-term/src/icons/nerd_table.rs` is derived from
  nvim-web-devicons (MIT, copyright and URL as upstream states them),
  with the licence text copied verbatim from upstream at the pinned
  commit into `assets/licenses/nvim-web-devicons-LICENSE`.
- The same section lists the Catppuccin file icons
  (`assets/file-icons/mocha/`, licence already at
  `assets/file-icons/catppuccin-icons-LICENSE.md`), which
  `ATTRIBUTIONS.md` never listed (cleanup 08.10).

Deliverable: the doc, markdownlint-clean.

Verification: the standard ritual; `markdownlint-cli2 ATTRIBUTIONS.md`.

Prohibitions: do NOT edit README or `agents.md` (plan 09); do NOT
proceed further.

Stop: report files changed and verification; await review.

### 08.10 -- Cleanup: Catppuccin icons missing from `ATTRIBUTIONS.md`

Surface point: the 08 activation audit. Impact: the vendored
Catppuccin file icons (embedded in both binaries) are not listed in
`ATTRIBUTIONS.md`, which calls itself the single source of truth for
attribution, though their licence file is bundled. Scope of fix: one
section of `ATTRIBUTIONS.md`. Resolution: in 08.9.

### 08.11 -- Follow-up outside this repo: freminal placeholder inheritance

File a cleanup entry in freminal: unicode-placeholder inheritance
should resolve from the cell to the left (same row), per the kitty
spec, rather than from the last processed placeholder, and a CR/LF
should reset it. frext does not depend on this fix. Done by the
orchestrator with the user's confirmation, after 08.9 lands.

## Manual smoke checklist

Run in kitty, freminal, a terminal without kitty graphics (foot or
alacritty), and tmux with `allow-passthrough on`:

1. `frext-term <dir>` in kitty / freminal: the sidebar shows the same
   coloured icons as the GUI, sharp at the current font size.
2. Selecting, hovering and the active-file row keep their background
   behind the icon (transparency).
3. A context menu or prompt covering half an icon leaves the other
   half drawn correctly; closing it redraws the icon whole.
4. Scrolling a long tree shows each icon type uploaded once
   (`frext-term.log` at debug level shows no repeated uploads).
5. Quitting leaves no images in the terminal (kitty: no memory growth
   over repeated runs).
6. In foot / alacritty: coloured Nerd Font glyphs; `--icons ascii`
   gives the ASCII slash; `--icons none` a blank slot.
7. `NO_COLOR=1 frext-term --icons kitty <dir>` in kitty: Nerd glyphs,
   and a warning in the log.
8. In tmux (`allow-passthrough on`) inside kitty: kitty icons; with
   passthrough off: Nerd glyphs.
