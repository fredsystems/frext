# Attributions

frext is licensed under the [MIT License](LICENSE). It also bundles a font
asset and contains source code adapted from third parties whose licenses require
attribution. Those components and their licenses are listed below.

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
