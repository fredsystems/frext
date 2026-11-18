# Attributions

frext is licensed under the [MIT License](LICENSE). It also bundles a font
asset and file-icon artwork, and contains icon data and source code adapted from
third parties whose licenses require attribution. Those components and their
licenses are listed below.

This document is the single source of truth for attribution: the README points
here rather than duplicating license text.

## Bundled fonts

The following font is embedded into the frext binary at compile time. The full
upstream license text is bundled under [`assets/fonts/`](assets/fonts/).

### CaskaydiaCove Nerd Font

- **Upstream:** Nerd Fonts — <https://github.com/ryanoasis/nerd-fonts> (Cascadia
  Code patched and renamed to "CaskaydiaCove"); base typeface Cascadia Code by
  Microsoft — <https://github.com/microsoft/cascadia-code>
- **License:** SIL Open Font License 1.1. The base typeface reserves the name
  "Cascadia Code" under the OFL Reserved Font Name clause; the Nerd Fonts patch
  renames the bundled face to "CaskaydiaCove" to comply.
- **License text:**
  [`assets/fonts/CaskaydiaCove-NerdFont-LICENSE.md`](assets/fonts/CaskaydiaCove-NerdFont-LICENSE.md)
- **Files:** `assets/fonts/CaskaydiaCoveNerdFont-Regular.ttf`

frext bundles only the regular face: the editor renders with a single font and
synthesises slant and weight, so dedicated bold/italic faces are not embedded.
The `Cove`/Code variant is used because it carries the coding ligatures
(`->`, `=>`, `!=`, …) that frext's editor relies on; the `CaskaydiaMono` variant
strips them.

## Icon data

The following file-type icon artwork and icon mapping data are bundled with or
derived into frext. The full upstream license texts are bundled alongside the
icons under [`assets/file-icons/`](assets/file-icons/) and under
[`assets/licenses/`](assets/licenses/).

### Catppuccin file icons

- **Upstream:** Catppuccin Icons for Zed —
  <https://github.com/catppuccin/zed-icons> (Mocha flavour), vendored at commit `6d953c8c7566b9e721d15b54e802dd77ae26fa9c`
- **License:** MIT License; copyright (c) 2021 Catppuccin.
- **License text:**
  [`assets/file-icons/catppuccin-icons-LICENSE.md`](assets/file-icons/catppuccin-icons-LICENSE.md)
- **Files:** `assets/file-icons/mocha/` — the SVGs are embedded in both the
  `frext` and `frext-term` binaries via `frext-core/src/file_icon_bytes.rs`; the
  lookup tables in `frext-core/src/file_icon_table.rs` are generated from the
  upstream icon-theme manifest at the same commit.

### nvim-web-devicons

- **Upstream:** nvim-web-devicons —
  <https://github.com/nvim-tree/nvim-web-devicons>, at commit `0dc0cfb0c6858041174a53c7aa4adaf65e994b3c`
- **License:** MIT License; copyright (c) 2023 nvim-tree.
- **License text:**
  [`assets/licenses/nvim-web-devicons-LICENSE`](assets/licenses/nvim-web-devicons-LICENSE)
- **Files:** `frext-term/src/icons/nerd_table.rs` — the Nerd Font glyph for each
  icon stem is derived from the upstream extension and filename tables by
  `cargo xtask nerd-icons`. The glyph colours come from the Catppuccin SVGs, not
  from nvim-web-devicons.

## Adapted source code

The following source code is adapted from a third-party project. The full
upstream license text is bundled under [`assets/licenses/`](assets/licenses/).

### ratatui-crossterm

- **Upstream:** ratatui — <https://github.com/ratatui/ratatui> (crate
  `ratatui-crossterm` 0.1.2)
- **License:** MIT License; copyright Florian Dehau and The Ratatui Developers.
- **License text:**
  [`assets/licenses/ratatui-LICENSE`](assets/licenses/ratatui-LICENSE)
- **Files:** `frext-term/src/term/backend.rs` — its `FrextBackend` draw loop,
  `Backend` methods and `ModifierDiff` are adapted from `CrosstermBackend`.
