# 09 -- `frext-term` packaging and docs

- **Status:** stub
- **Depends on:** 02

## Goal

Ship `frext-term` alongside `frext`: a Nix package, the overlay, the
Home Manager module, CI coverage, and user documentation.

## Task summary

- `flake.nix`: `packages.<system>.frext-term`, built from the same
  workspace with `cargoBuildFlags = [ "-p" "frext-term" ]`. No
  GL/Wayland runtime libraries, no `wrapProgram` `LD_LIBRARY_PATH`, no
  desktop item and no zenity. Keep the GUI package exactly as it is.
- `nix/overlay.nix`: also expose `pkgs.frext-term`.
- `nix/home-manager-module.nix`: an option to install `frext-term`.
  Config stays the shared `frext/config.toml` with its `[term]` table
  (plan 06).
- CI: `cargo test --all` already covers every workspace member. Add a
  `nix build .#frext-term` check so the package cannot silently break.
- `README.md`: a `frext-term` section covering usage, keybindings
  (including fallbacks for terminals without the kitty keyboard
  protocol), the `[term]` config keys, icon and undercurl detection,
  and the tmux settings (`allow-passthrough on`, `usstyle`).
- `agents.md`: add `frext-term` and `xtask` (plan 08's Nerd table
  generator, `cargo xtask nerd-icons`) to the workspace layout and
  architecture, add the terminal-safety rule (always restore the
  terminal, including from the panic hook), and make the verification
  ritual's clippy `cargo clippy --workspace ...` (with
  `default-members` set, a plain `cargo clippy` skips `xtask`).
- The README's tmux section also covers kitty icons: `allow-passthrough
  all` (with `on`, uploads from a hidden pane are lost; plan 08.16) and
  a `terminal-features` entry giving the outer terminal `RGB` (the
  image id travels in a 24-bit colour).

## Durable decisions

- `frext-term` is a separate package and binary, not a `--tui` flag on
  `frext`, so terminal users never download or link the GUI stack.
- `frext-term`'s dependency closure must not contain egui, winit or
  OpenGL. Check with `nix path-info -r` or `cargo tree -p frext-term`
  as part of verification.

## Decide at activation

- Home Manager shape: `programs.frext.term.enable` inside the existing
  module (proposed, since they share config) versus a separate
  `programs.frext-term`.
- Whether `nix build .#default` stays the GUI only or becomes a
  combined package.
- Whether to ship a terminfo snippet or docs for terminals that need
  `Smulx` declared.
