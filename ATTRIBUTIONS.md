# Attributions

frext is licensed under the [MIT License](LICENSE). It also bundles a font
asset from a third party whose license requires attribution. That component and
its license are listed below.

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
