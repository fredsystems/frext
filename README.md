# frext

**Fred Text** — a super lightweight text editor written in Rust.

frext is a small GUI text editor built on [egui]/[eframe]. It aims to be
minimal and fast while still offering the conveniences that make a daily
driver pleasant: tabs, session persistence, and (as a stretch goal)
syntax highlighting.

## Features

- **Tabs.** Edit multiple buffers in one window.
- **Session persistence.** Unsaved work survives quitting and reopening.
  Every keystroke is written to a per-tab swap file under the platform
  state directory, so an unsaved buffer is exactly as you left it on the
  next launch — even after a crash.
- **Catppuccin Mocha theme.** The palette is applied directly to egui's
  visuals, so there is no theme-crate dependency to drift behind egui
  releases.
- **Bundled coding font with ligatures.** frext embeds the CaskaydiaCove
  Nerd Font (Cascadia Code) and installs it as the editor's monospace
  face, so coding ligatures (`->`, `=>`, `!=`, …) and a broad glyph set
  render out of the box, independent of what fonts the host has
  installed.
- **Native file dialogs** for open and save.
- **Open from the command line.** `frext file.txt` opens the file on
  launch (alongside any restored session). A file that is already open is
  focused rather than opened twice.
- **Syntax highlighting.** Powered by syntect (via `egui_extras`), with
  the language auto-detected from the file extension. Untitled or
  extension-less buffers render as plain text.
- **File-tree sidebar.** Launch `frext path/to/dir` to open a directory
  in a resizable sidebar. Click a file to open it; the active file is
  highlighted in the tree. The workspace root and which folders you have
  expanded persist across sessions.
- **External-change reload.** If a file's size changes on disk while it
  is open, frext reloads it automatically — unless the buffer has unsaved
  edits, which are never overwritten.

## Usage

```sh
frext                  # restore the previous session
frext notes.txt a.rs   # open files (added to the restored session)
frext path/to/project  # open a directory in the file-tree sidebar
```

## Keyboard shortcuts

| Shortcut | Action          |
| -------- | --------------- |
| Ctrl+N   | New tab         |
| Ctrl+O   | Open file       |
| Ctrl+S   | Save active tab |
| Ctrl+W   | Close tab       |

## Persistence layout

State lives under the platform state directory (on Linux,
`$XDG_STATE_HOME/frext`, typically `~/.local/state/frext`):

```text
frext/
  session.json     # tab order, ids, paths, active tab, workspace
  swap/
    <id>.swp       # full text of each tab's buffer
```

On launch, a tab's swap file is the source of truth for its content, so
unsaved edits always win over what is currently on disk.

## Building

frext uses a Nix flake for a reproducible dev environment.

```sh
nix develop          # enter the dev shell (or use direnv: `direnv allow`)
cargo run            # build and run
```

Without Nix you will need the usual egui runtime libraries on Linux
(`libxkbcommon`, `wayland`, `libGL`) plus `pkg-config`.

## Installing via Nix flake (Home Manager)

Add frext to your flake inputs and import the Home Manager module:

```nix
{
  inputs.frext.url = "github:fredsystems/frext";

  # In your Home Manager configuration:
  imports = [ inputs.frext.homeManagerModules.default ];

  programs.frext.enable = true;
}
```

The package is also exposed as an overlay (`frext.overlays.default`,
adding `pkgs.frext`) and directly as
`frext.packages.${system}.frext`.

## Development

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
cargo machete
```

## License

MIT. See [LICENSE](LICENSE).

frext bundles the CaskaydiaCove Nerd Font under the SIL Open Font
License 1.1. See [ATTRIBUTIONS.md](ATTRIBUTIONS.md) for the full
attribution and license details.

[egui]: https://github.com/emilk/egui
[eframe]: https://github.com/emilk/egui/tree/master/crates/eframe
