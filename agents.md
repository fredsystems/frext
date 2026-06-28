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
persistence, and (stretch goal) syntax highlighting.

### Workspace layout

```text
frext (binary + library)
  src/
    main.rs         -- thin eframe entry point
    lib.rs          -- module root (so logic is unit-testable)
    app.rs          -- FrextApp: tab bar, editor surface, action wiring
    tab.rs          -- per-tab buffer model (content, dirty state, title)
    persistence.rs  -- swap-file autosave + session index (Store)
    theme.rs        -- Catppuccin Mocha applied to egui::Visuals
    error.rs        -- typed error enums

nix/
  overlay.nix              -- adds pkgs.frext
  home-manager-module.nix  -- programs.frext options
```

### Architecture, in one paragraph

frext is a single egui/eframe app. `FrextApp` owns the open `Tab`s, the
active index, and a `persistence::Store`. The egui `App::ui` callback
renders a top `Panel` (tab bar + new/open/save buttons) and a
`CentralPanel` containing a multiline `TextEdit` bound to the active
tab's text. Edits are written to the active tab's swap file immediately
so unsaved work is crash-safe; the session index (`session.json`) is
rewritten whenever the tab set, ordering, or active tab changes. On
launch the swap file is the source of truth for a tab's content, so
unsaved edits always win over what is on disk. UI actions are collected
into a `MenuAction` enum and applied after the `self.tabs` borrow ends,
to keep the borrow checker happy.

---

## Non-negotiable rules

- No unsafe code unless explicitly requested.
- Prefer clarity over cleverness.
- No public APIs without tests.
- All observable behavior must be testable -- this is why the logic
  lives in `lib.rs` modules rather than inline in `main.rs`.
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
- The Catppuccin theme is implemented inline in `theme.rs` rather than
  via a theme crate, specifically to avoid lagging egui releases. Keep
  it that way unless there is a strong reason not to.

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
