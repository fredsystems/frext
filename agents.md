# AGENTS.md -- frext workspace

This document is the always-on orientation for AI coding agents working
in the frext workspace. Operational procedures live as opencode skills
(sourced from fred's shared skill set) and are loaded on demand. This
document gives you the map; the skills give you the moves.

---

## Project overview

frext (Fred Text) is a super lightweight GUI text editor written in Rust
(Edition 2024). It is built on egui/eframe and targets a small, fast
binary with a pleasant daily-driver feature set: tabs, session
persistence, and syntax highlighting.

### Workspace layout

```text
Cargo.toml              -- workspace: members frext-core, frext

frext-core/             -- frontend-independent core (library, no UI toolkit)
  src/
    lib.rs              -- module root
    editor/             -- the Editor controller
      mod.rs            -- Editor: owns all non-view state; apply(),
                           begin_frame/end_frame/shutdown, active_view_mut,
                           commit_edit, set_selection
      command.rs        -- Command: every user intent a front end can send
      effect.rs         -- Outcome, BufferChange, Effect, EditorFocus
      prompt.rs         -- Prompt: the single pending modal (tab ids, not
                           indices)
      tabs.rs           -- tab lifecycle: open/switch/save/close + prompts
      search.rs         -- find/replace state and Editor search methods
      tree.rs           -- sidebar workspace + filesystem actions
      editing.rs        -- cut/copy/paste, select all, proofreading fixes
    tab.rs              -- per-tab buffer model (content, dirty, disk reload)
    persistence.rs      -- swap-file autosave + session index (Store)
    workspace.rs        -- file-tree sidebar model (root + expanded folders)
    search.rs           -- search query model + compiled regex matcher
    syntax.rs           -- syntect + two-face syntax/theme sets, extension
                           language detection
    highlight.rs        -- highlighting into StyledSpans + search-match
                           overlay (MatchBackground roles)
    palette.rs          -- Catppuccin Mocha as plain Rgb + semantic roles
    lines.rs            -- line counting + line-number gutter logic
    offsets.rs          -- char index <-> byte offset conversion
    clipboard.rs        -- Clipboard trait; SystemClipboard (arboard),
                           MemoryClipboard (in-process)
    edit_ops.rs         -- pure cut/paste splices over char ranges
    fs_ops.rs           -- rename / create / trash for the sidebar
    file_icon.rs        -- file-type icon lookup for the sidebar
    file_icon_table.rs  -- generated icon lookup tables (do not edit)
    file_icon_bytes.rs  -- generated embedded SVG bytes (do not edit)
    proofread/          -- Harper spelling/grammar
      mod.rs            -- checker: Lints -> Diagnostics (byte + char ranges)
      regions.rs        -- which parts of a buffer are prose to check
      state.rs          -- per-tab results, dictionary, ignores, worker handle
      worker.rs         -- background proofreading thread
    error.rs            -- typed error enums

frext/                  -- egui/eframe GUI front end (binary + library)
  src/
    main.rs             -- thin eframe entry point; CLI arg parsing
    lib.rs              -- module root for the GUI modules
    app.rs              -- FrextApp: renders tab bar, sidebar, editor,
                           prompts; sends Commands, applies Effects
    editor_layout.rs    -- StyledSpans -> egui LayoutJob; TextEdit layouter
    font.rs             -- bundled CaskaydiaCove font + emoji fallback
    icon.rs             -- embedded window icon
    squiggle.rs         -- wavy underlines under proofreading diagnostics
    theme.rs            -- palette.rs colours -> egui::Visuals
  benches/
    highlight.rs        -- criterion bench for editor_layout::layout_job

nix/
  overlay.nix              -- adds pkgs.frext
  home-manager-module.nix  -- programs.frext options
```

### Architecture

frext is a two-crate workspace. `frext-core` holds the frontend-neutral
`editor::Editor`, which owns all non-view state: the open `Tab`s, the
active index, the `persistence::Store`, the optional
`workspace::Workspace`, find/replace state, the single pending `Prompt`,
proofreading, the clipboard, and the char selection last reported by
the view (`Editor::set_selection`). A front end sends every user intent
as a `Command` through `Editor::apply`, which returns an `Outcome`: a
`BufferChange` plus a list of `Effect`s (set selection, reveal
selection, focus a field) that only the front end can carry out. The
front end binds its text widget to the active buffer through
`Editor::active_view_mut()` and calls `Editor::commit_edit()` after the
widget edits it. `begin_frame` (proofread polling, disk-reload check),
`end_frame` (queue a proofread) and `shutdown` (final flush) bracket the
frame loop.

The GUI's `FrextApp` holds only `editor`, `menu_diagnostic` (the
diagnostic under a right-click) and the queued `effects`. Its `send`
helper applies a `Command` and queues the returned effects;
`apply_effects` drains them into a `FrameEffects` at the start of the
next frame, before anything renders. Highlighting in core produces
`StyledSpan`s (plain `Rgb` + search-match roles) that `editor_layout.rs`
converts to egui `LayoutJob`s; `palette.rs` colours are converted to
egui colours by `theme.rs`.

Every edit is written to the active tab's swap file immediately, so
unsaved work is crash-safe; the session index (`session.json`) is
rewritten whenever the tab set, ordering, or active tab changes. On
launch the swap file is the source of truth for a tab's content, so
unsaved edits always win over what is on disk. A directory argument
opens a `Workspace` (root + expanded folder set, persisted in
`session.json`) shown as a left-panel file tree; the editor body is a
`TextEdit` wrapped in a `ScrollArea` (the wrap is required -- a bare
`add_sized` clamps the editor to the viewport and breaks scrolling).
Each frame `begin_frame` calls `Tab::reload_if_changed_on_disk` on the
active tab, which reloads a _clean_ buffer when its backing file's
on-disk size changed; a dirty buffer is never clobbered.

---

## Non-negotiable rules

- No unsafe code unless explicitly requested.
- Prefer clarity over cleverness.
- No public APIs without tests.
- All observable behavior must be testable -- this is why editor logic
  lives in `frext-core` behind `Editor`, and the GUI's own modules are
  in `frext`'s `lib.rs` rather than inline in `main.rs`.
- **`frext-core` never depends on a UI toolkit.** No egui, eframe, rfd,
  ratatui, crossterm, or similar in its `Cargo.toml`; the crate boundary
  enforces it. Toolkit-specific conversion lives in the front end.
- Correctness > performance.
- **Panic-free production code**: `unwrap()` / `expect()` are forbidden
  outside `#[cfg(test)]`. Enforced via `[workspace.lints.clippy]`
  (`unwrap_used` / `expect_used` = `deny`). Test modules opt out with a
  module-level `#![allow(clippy::unwrap_used)]` and an explanatory
  comment.
- **Errors are explicit and typed.** Domain errors are enums in
  `error.rs` (`thiserror`). No `anyhow` in the library.
- Persistence must never crash the editor: I/O failures in autosave are
  logged, not propagated, from the UI loop.

The `rust-best-practices` skill expands the panic/lint/dependency rules.

---

## Dependency policy

- Pin every dependency to full `major.minor.patch` semver, alphabetically
  sorted within each table (`rust-best-practices`).
- egui and eframe move fast and break their API between minor versions.
  **When bumping them, verify against the live registry and fix the API
  drift** -- do not assume an old pattern still compiles. (Example: egui
  0.35 replaced `TopBottomPanel` with `Panel::top`, and `eframe::App`'s
  `update(&Context)` became `ui(&mut Ui)`.)
- The Catppuccin theme is implemented inline (palette in
  `frext-core/src/palette.rs`, egui conversion in `frext/src/theme.rs`)
  rather than via a theme crate, specifically to avoid lagging egui
  releases. Keep it that way unless there is a strong reason not to.

---

## Verification ritual

Run inside the Nix dev shell (`nix develop`) so the egui runtime
libraries are present:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
cargo machete
```

All four must be green before a change is considered done
(`testing-mandate`). New code needs new tests; bug fixes need regression
tests.

---

## Nix / flake

- The flake exposes `packages.<system>.frext`, an overlay, a Home Manager
  module, a `pre-commit-check`, and `default` / `ci` dev shells.
- System tools (compilers, egui runtime libs) come from the dev shell.
  If a build needs a tool that is missing, add it to `flake.nix` and ask
  the user to re-enter the shell -- do not work around it
  (`flake-dev-shell-discipline`).
- `.nix` files are linted with nixfmt + statix + deadnix on commit
  (`nix-best-practices`).

---

## Skills you will likely need here

- `rust-best-practices` -- lint/panic/dependency policy.
- `nix-best-practices` -- editing the flake or nix modules.
- `flake-dev-shell-discipline` -- missing system tools.
- `testing-mandate` -- before declaring a task done.
- `commit-discipline` / `precommit-fix-loop` -- committing.
- `markdown-lint-discipline` -- editing this file or the README.
