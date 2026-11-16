# 02 -- `frext-term` shell

- **Status:** stub
- **Depends on:** 01

## Goal

Create the `frext-term` binary crate: a ratatui + crossterm application
over `frext_core::Editor` with the full screen layout, terminal
lifecycle, event loop, tab bar, status line, prompt/modal framework and
keybindings. The editor surface in this plan is a read-only plain-text
view. Real editing arrives in plan 03, the sidebar in 05.

## Task summary

- New workspace member `frext-term/` (package `frext-term`, binary
  `frext-term`), with a `lib.rs` so view logic is unit-testable, and a
  thin `main.rs`.
- CLI argument handling identical to `frext`: files open in tabs; the
  first directory becomes the workspace.
- Terminal lifecycle: raw mode, alternate screen, mouse capture,
  bracketed paste, kitty keyboard enhancement where supported; restore
  everything on exit **and** from a panic hook (panics are forbidden in
  production code, but a dependency can still panic, and a wrecked
  terminal is the worst failure mode a TUI has).
- Event loop: one channel that merges crossterm input events, a
  proofread-worker wake-up (the `notify` closure passed to
  `Editor::begin_frame` sends on it), and a periodic tick (about 500 ms)
  for the on-disk reload check. Redraw only on an event.
- Layout: tab bar row (`+`, Open, Save, then tabs with a dirty `•` and
  a close `×`), a find-bar area (empty until plan 04), sidebar area
  (empty until plan 05), the editor area with line-number gutter, and a
  one-line status bar.
- Prompt/modal framework: a centred popup widget used for
  `Prompt::ConfirmClose`, `Prompt::SavePath`, `Prompt::Name`, and the
  TUI-only open-file path prompt. Includes a reusable single-line text
  input widget (also used by plan 04's find bar).
- Logging goes to a file (`$XDG_STATE_HOME/frext/term/frext-term.log`);
  `env_logger` to stderr would corrupt the screen.
- Palette -> ratatui `Color::Rgb` conversion, plus a `Tint` blend-over
  helper (`Tint::over(base: Rgb) -> Rgb`) added to `frext-core::palette`
  beside its first user.
- Clipboard: `frext-term` provides an OSC 52 `Clipboard`
  implementation (write-only) and picks it over `SystemClipboard` when
  no display server is available (`WAYLAND_DISPLAY` and `DISPLAY`
  unset, e.g. over SSH). Paste from the terminal arrives as a bracketed
  paste event and is inserted directly. OSC 52 reads are not used:
  terminals commonly disable them (freminal does by default).

## Durable decisions

- **Separate binary, shared core.** `frext-term` depends only on
  `frext-core` (plus ratatui, crossterm and its own small deps), never
  on the `frext` GUI crate. Its build must not pull in egui, winit or
  OpenGL.
- **ratatui (with its crossterm backend) is the TUI library.** Pin both
  to full semver at activation, verified against the live registry.
  ratatui moves its API between minors as egui does; fix drift, don't
  assume old patterns.
- **Separate sessions, shared dictionary.** The GUI and TUI may run
  side by side, so they must not share `session.json` or swap files
  (two processes writing the same index would clobber each other's
  tabs). `Store` gains a profile:
  - GUI: unchanged, `$XDG_STATE_HOME/frext/{session.json,swap/}`, so
    existing sessions survive.
  - TUI: `$XDG_STATE_HOME/frext/term/{session.json,swap/}`.
  - Both: `$XDG_STATE_HOME/frext/dictionary.txt`. Adding a word becomes
    "re-read, union, write atomically (temp file + rename)", with that
    whole sequence run under an exclusive advisory lock on a sidecar
    `dictionary.txt.lock` (`flock`-style, via `std::fs::File::lock`,
    stable since Rust 1.89 and below the workspace MSRV of 1.95, so no
    new dependency). Without the lock both processes could read before
    either renames and one word would be lost. The lock is held only
    for that sequence, and the rename stays atomic so readers never see
    a partial file. This is a change to `frext-core::persistence` and
    lands in this plan with tests, including two writers interleaving
    (each adds a different word; both words survive).
  - Two instances of the **same** front end sharing one session is an
    existing limitation and stays out of scope.
- **Non-modal, GUI-parity keybindings.** Ctrl+N / Ctrl+O / Ctrl+S /
  Ctrl+W / Ctrl+F / Ctrl+H / F3 / Shift+F3 / Esc mean what they mean in
  the GUI. Ctrl+H is indistinguishable from Backspace in legacy
  terminal encoding, so the kitty keyboard protocol is requested
  (crossterm `PushKeyboardEnhancementFlags`) when the terminal supports
  it, and a fallback binding is provided when it does not. Plan 06
  replaces the interim crossterm support check with the unified
  capability probe.
- **Mouse is first class.** Clicking tabs, `×`, toolbar buttons, tree
  rows and menu items, the wheel, and right-click context menus all
  work, as they do in the GUI. Shift+drag is left to the terminal for
  native selection.
- **Tooltips become status-bar text.** Hover hints (shortcut names) and
  transient messages (save failed, regex error) are shown on the status
  line.

## Decide at activation

- Open-file UX: a path prompt with Tab completion (proposed) versus a
  fuzzy finder over the workspace.
- Keyboard focus model between editor, sidebar and find bar (for
  example Ctrl+E / Ctrl+B to focus or toggle the sidebar, Tab cycling
  within the find bar).
- The fallback binding for find-and-replace when Ctrl+H cannot be
  distinguished from Backspace.
- Whether keybindings are defined once in `frext-core` as
  frontend-neutral data (a `KeyChord -> Command` table both front ends
  translate into) or per front end.
- How the editor area renders before plan 03 lands (read-only
  `Paragraph` versus a placeholder).
