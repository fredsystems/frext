# 02 -- `frext-term` shell

- **Status:** active
- **Depends on:** 01
- **Branch:** `feat/frext-term-shell` (one branch, one commit per
  subtask, one PR)

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

## Decided at activation

- **Open-file UX: a path prompt with Tab completion.** A fuzzy finder
  can be a later plan.
- **Focus model: a TUI-local `Focus` enum.** This plan defines
  `Focus::{Editor, Sidebar}`. Plan 04 adds the find-bar fields. Ctrl+B
  shows or hides the sidebar, and Ctrl+E moves focus between the editor
  and the sidebar (only while the sidebar is shown). Esc from the
  sidebar returns focus to the editor. An open prompt always captures
  the keyboard. Tab / Shift+Tab cycling inside the find bar is plan
  04's.
- **Find-and-replace fallback: Ctrl+R.** It is always bound. Ctrl+H is
  bound **only** when the kitty keyboard protocol is active. In legacy
  encoding, crossterm reports byte `0x08` as Ctrl+H, and some terminals
  send that byte for Backspace, so binding Ctrl+H there would turn
  Backspace into "open replace".
- **Keybindings live in `frext-term`** (`keymap.rs`), as a pure
  `KeyEvent -> Action` function. The GUI is not touched.
- **Before plan 03, the editor area is a read-only `Paragraph`** of the
  active buffer: plain text, no wrap, a line-number gutter, and
  scrolling by keyboard and wheel. Bracketed paste outside a prompt is
  a no-op with a status message.
- **Quit: Ctrl+Q, immediately.** Unsaved work is already in swap files,
  which is what closing the GUI window does today.
- **Logging: a small in-crate `log::Log`** that writes to the log file.
  No new dependency.
- **OSC 52: crossterm's own `clipboard::CopyToClipboard`** (the
  `osc52` feature, which uses the `base64` crate internally) instead of
  a direct `base64` dependency. This is the same encoder with one less
  manifest entry, and the escape is a tested crossterm `Command`.

## Further decisions (orchestrator)

- **Dependencies** (checked against crates.io on 2026-11-16):
  `ratatui = { version = "0.30.2", default-features = false, features
  = ["crossterm", "layout-cache", "underline-color"] }` and
  `crossterm = { version = "0.29.0", features = ["osc52"] }`, the same
  crossterm version that ratatui 0.30.2 uses through
  `ratatui-crossterm`, so the tree resolves to one crossterm. Both MSRVs
  (1.88 and 1.63) are below the workspace's 1.95. Add `unicode-width`
  at the version already in `Cargo.lock` through ratatui (0.2.3).
  Each dependency enters both `[workspace.dependencies]` and
  `frext-term/Cargo.toml` in the subtask that first uses it (`ratatui`
  in 02.5, `crossterm` in 02.6, `unicode-width` in 02.11), so
  `cargo machete` and cargo's unused-workspace-dependency check stay
  quiet. Those subtasks' scopes include the manifests and `Cargo.lock`.
- **No `ratatui::init`.** Its panic hook restores only raw mode and the
  alternate screen, not mouse capture, bracketed paste or the keyboard
  flags. `frext-term` owns the whole lifecycle in `terminal.rs`.
- **Store profiles.** `persistence::Profile { Gui, Term }`.
  `Store::new(profile)` replaces `Store::new()`. `Store::at(root)` keeps
  the dictionary in `root` (existing tests unchanged).
  `Store::at_with_dictionary_dir(root, dir)` is the general form, and
  `Store::root()` exposes the state root (the log file lives there).
- **Dictionary merge.** `Store::merge_dictionary(words)` replaces
  `save_dictionary`. Under an exclusive `File::lock` on
  `dictionary.txt.lock`, it re-reads the file, unions it with `words`,
  writes `dictionary.txt.tmp` and renames it. If the re-read fails for
  any reason other than "not found", it returns the error and writes
  nothing. The editor's `DictionaryFile::Unreadable` guard stays.
- **`Tint::over(base)`** is a per-channel sRGB blend,
  `round(base * (1 - a) + color * a)`, with `a = opacity` clamped to
  `0.0..=1.0`. Its first user is the TUI hover highlight
  (`theme::hover_bg`).
- **Hit testing.** Every frame, rendering records a `HitMap` of
  `(Rect, Target)` regions. Mouse events are resolved against the
  previous frame's map, last-registered region first. This lets
  widgets register clickable regions without the app knowing their
  geometry.
- **Prompt state split.** Core owns `Prompt` and, for `Prompt::Name`,
  the text (`prompt_name_mut`). The TUI owns only view state: cursor,
  focused button, completion cycling, and the text of the two path
  prompts (`SavePath`, and Open, which is TUI-only). The TUI's open
  prompt can only open while `editor.prompt()` is `None`, which keeps
  "one modal at a time" true.
- **Path resolution for the path prompts.** A leading `~/` expands to
  `$HOME`. A relative path joins onto the workspace root if there is
  one, otherwise onto the working directory captured at launch.
- **Find keys before plan 04.** Ctrl+F / Ctrl+R / Ctrl+H, F3,
  Shift+F3 and Esc are mapped and send their `Command`s, so the core
  search state is real. The find-bar row is reserved (one blank row
  while `search().is_open()`) and the status line says
  `Find bar is not available yet`. Plan 04 renders the row.
- **Effects before plan 03.** `RevealSelection` scrolls the read-only
  view to centre the selection's line. `SetSelection` is ignored (no
  caret yet). `FocusPromptField` moves the prompt cursor to the end.
  `FocusSearchField` is a no-op until plan 04.
- **Not in this plan:** a context-menu widget (it lands with its first
  user, plan 03's editor menu or plan 05's tree menu), save-failure
  messages (core does not report them in an `Outcome` yet), SIGTERM /
  SIGHUP handling (swap files are already written on every edit), and
  docs (`agents.md` / `README.md` are plan 09's).

## Target layout

```text
frext-term/
  Cargo.toml            package and binary `frext-term`
  src/
    main.rs             thin: args, Store, logger, clipboard, run()
    lib.rs              module root
    cli.rs              Args { files, dir }: the GUI's argument split
    logging.rs          FileLogger (log::Log) + level parsing
    clipboard.rs        Osc52Clipboard, ClipboardChoice::from_env
    theme.rs            palette -> ratatui Color/Style; hover_bg
    terminal.rs         setup/restore, KeyboardMode, panic hook
    events.rs           AppEvent, EventLoop (input/wake/tick threads)
    hit.rs              HitMap, Target, Target::hint
    layout.rs           Areas: tab bar, find bar, sidebar, gutter,
                        editor, status
    status.rs           StatusLine: messages, hints, default text
    tab_bar.rs          tab bar items, scrolling, render
    text_input.rs       single-line input over an external String
    complete.rs         path Tab completion (pure)
    popup.rs            centred popup frame + button row
    prompt.rs           PromptUi: ConfirmClose, Name, SavePath, Open
    keymap.rs           Action, KeyContext, action_for()
    editor_view.rs      read-only Paragraph view + per-tab scroll
    app.rs              TermApp: event handling, render, effects
    run.rs              the event loop over a real terminal
```

## Execution model

All subtasks run **strictly in sequence** on `feat/frext-term-shell`.
Each subtask is one commit, leaves the suite green
(`commit-discipline`), and gets a full diff review by the orchestrator
before it is committed. Implementers do not commit, merge or push.

Each subtask's verification includes the standard ritual, run inside
`nix develop`:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
cargo machete
```

All `frext-term` code follows the workspace rules: no `unwrap` /
`expect` outside tests, no `unsafe`, `[lints] workspace = true`, and
every public item tested. Tests that need a directory use a
`temp_dir(tag)` helper like the one in `persistence.rs`, so no
`tempfile` dependency is added.

## Subtasks

### 02.1 -- Store profiles

Scope: `frext-core/src/persistence.rs`, `frext/src/main.rs`.

What: Add `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum
Profile { Gui, Term }`. `Store` gains a `dictionary_dir: PathBuf`
field. `Store::new(profile: Profile)` resolves the same `ProjectDirs`
base as today. `Gui` uses root = base, and `Term` uses root =
`base/term`. Both use dictionary dir = base. Add
`Store::at_with_dictionary_dir(root, dictionary_dir)`, which creates
`root/swap` and `dictionary_dir`. `Store::at(root)` delegates with
`dictionary_dir = root.clone()`. `dictionary_path()` uses
`dictionary_dir`. Add `#[must_use] pub fn root(&self) -> &Path`. Update
the module doc's layout block for both profiles. `frext/src/main.rs`
calls `Store::new(Profile::Gui)`.

Deliverable: tests `term_profile_layout_is_under_term` (exercised
through a private `fn for_profile(base: PathBuf, profile) -> Result`
that `new` calls, so the test never touches the real XDG dir),
`gui_profile_layout_is_unchanged`,
`profiles_share_one_dictionary` (a word saved through one store loads
through the other), and `root_reports_the_state_root`.

Verification: the standard ritual.

Prohibitions: do NOT change the session or swap format; do NOT add
locking (02.2); do NOT proceed to 02.2.

Stop: report files changed + verification; await review.

### 02.2 -- Locked dictionary merge

Scope: `frext-core/src/persistence.rs`,
`frext-core/src/editor/editing.rs`.

What: Replace `save_dictionary` with `pub fn merge_dictionary<'a>(&self,
words: impl IntoIterator<Item = &'a str>) -> Result<(), PersistenceError>`.
It opens (create, write) `dictionary_dir/dictionary.txt.lock` and calls
`File::lock()`, held for the whole sequence and released on drop. It
then re-reads the file with the existing `load_dictionary` logic
(NotFound means empty, any other error is returned with no write),
builds a `BTreeSet` union of the file's words and the trimmed,
non-empty `words`, writes `dictionary.txt.tmp`, and renames it over
`dictionary.txt` (keeping today's temp cleanup on a failed write). Lock
I/O errors map to `PersistenceError::Io` with the lock path.
`add_to_dictionary` calls `merge_dictionary`.

Deliverable: the existing dictionary tests ported to
`merge_dictionary`, plus:

- `merge_keeps_words_written_by_another_store`: two stores on one
  dictionary dir, both loaded empty. A merges `alpha`, B merges `beta`,
  and the file holds both.
- `concurrent_merges_lose_no_words`: 8 threads, each with its own
  `Store` and a distinct word, start together on a `Barrier`, and all
  8 words survive.
- `merge_does_not_write_when_the_file_is_unreadable`: the dictionary
  path is a directory, so the merge returns `Err` and nothing is
  renamed.
- the editor tests in `editing.rs` stay green unchanged.

Verification: the standard ritual, plus
`cargo test -p frext-core concurrent_merges -- --test-threads=1` run
10 times in a loop (all pass).

Prohibitions: do NOT add a dependency; do NOT make proofreading pick up
words from the other process; do NOT proceed to 02.3.

Stop: report and await review.

### 02.3 -- `Tint::over`

Scope: `frext-core/src/palette.rs`.

What: `#[must_use] pub fn over(self, base: Rgb) -> Rgb` on `Tint`,
using the formula under "Further decisions", with doc comment.

Deliverable: tests `over_at_zero_opacity_is_base`,
`over_at_full_opacity_is_the_tint`, `over_blends_and_rounds` (a
hand-computed value, e.g. `selection().over(BASE)`), and
`over_clamps_opacity`.

Verification: the standard ritual.

Prohibitions: do NOT change any existing role or constant; do NOT
proceed to 02.4.

Stop: report and await review.

### 02.4 -- `frext-term` crate skeleton: CLI and file logger

Scope: `Cargo.toml`, `Cargo.lock`, `frext-term/Cargo.toml` (new),
`frext-term/src/{main,lib,cli,logging}.rs` (new).

What:

- Add `frext-term` to the workspace members. `frext-term/Cargo.toml`
  inherits the `workspace.package` fields, sets
  `[lints] workspace = true`, and depends only on `frext-core` and
  `log` (the TUI dependencies arrive with their first users). Not
  depending on `directories` is fine, because the path comes from
  `Store::root()`. The global logger is installed with
  `log::set_logger(Box::leak(..))`, because `set_boxed_logger` needs
  `log`'s `std` feature.
- `cli.rs`: `#[derive(Debug, Default, PartialEq, Eq)] pub struct Args
  { pub files: Vec<PathBuf>, pub dir: Option<PathBuf> }` and
  `pub fn parse(args: impl IntoIterator<Item = OsString>) -> Args`
  (program name already skipped). This is the GUI's rule: the first
  directory becomes `dir`, later directories are ignored, and
  everything else is a file.
- `logging.rs`: `pub fn parse_level(value: Option<&str>) -> LevelFilter`
  (case-insensitive `off|error|warn|info|debug|trace`, otherwise
  `Info`). `pub fn init(path: &Path, level: LevelFilter) ->
  io::Result<()>` creates the parent directory, renames an existing
  log to `frext-term.log.old`, opens a fresh file, and installs a
  `FileLogger { file: Mutex<File>, level }`. The logger writes
  `{unix_secs}.{millis:03} {LEVEL:<5} {target}: {args}\n` and flushes
  each record, and a poisoned mutex skips the record. A
  `FileLogger::new` / `format_record` seam makes the format testable
  without the global logger.
- `main.rs`: `fn main() -> ExitCode`. It parses args, builds
  `Store::new(Profile::Term)` (on error: `eprintln!` and exit 1),
  initialises logging at `store.root().join("frext-term.log")` with
  level `parse_level(env RUST_LOG)` (on error: `eprintln!` a warning
  and continue), opens `Editor::open(store, SystemClipboard, ..)`,
  calls `shutdown()`, and exits 0. Later subtasks replace the middle.

Deliverable: tests for `parse` (first dir wins, files keep order, a
second dir is ignored, no args), `parse_level` (each level, mixed
case, garbage, `None`), `format_record`, and `init` rotation (an
existing log becomes `.old`).

Verification: the standard ritual, plus
`cargo tree -p frext-term -e normal | rg -i 'egui|eframe|winit|glow|wgpu'`
prints nothing.

Prohibitions: do NOT touch `frext/` or `flake.nix`; do NOT enter raw
mode yet; do NOT proceed to 02.5.

Stop: report and await review.

### 02.5 -- Theme conversion

Scope: `Cargo.toml`, `Cargo.lock`, `frext-term/Cargo.toml`, `frext-term/src/theme.rs` (new), `frext-term/src/lib.rs`.

What: `pub const fn color(rgb: Rgb) -> Color` (`Color::Rgb`) and
`#[must_use]` style helpers:

| Fn                 | Style                                      |
| ------------------ | ------------------------------------------ |
| `base()`           | fg `TEXT`, bg `BASE`                       |
| `chrome()`         | fg `TEXT`, bg `MANTLE` (tab/status bar)    |
| `active_tab()`     | `chrome()` + fg `accent()` + bold          |
| `close_glyph()`    | `chrome()` + fg `OVERLAY1`                 |
| `gutter()`         | fg `palette::gutter()`, bg `BASE`          |
| `status()`         | fg `SUBTEXT0`, bg `MANTLE`                 |
| `status_error()`   | fg `palette::error()`, bg `MANTLE`         |
| `popup()`          | fg `TEXT`, bg `SURFACE0`                   |
| `popup_border()`   | fg `accent()`, bg `SURFACE0`               |
| `input()`          | fg `TEXT`, bg `SURFACE1`                   |
| `button_focused()` | fg `TEXT`, bg `selection().over(SURFACE0)` |
| `hover_bg()`       | `Color`: `selection().over(MANTLE)`        |

Deliverable: tests `color_maps_rgb_exactly` and one assertion per
helper's colours.

Verification: the standard ritual.

Adds `ratatui` (pinned as under "Further decisions") to the workspace
and to `frext-term`. Verification also runs `cargo tree -p frext-term
-i crossterm`, which must show exactly one crossterm (0.29.0).

Prohibitions: do NOT add colours to `frext-core`; do NOT proceed to
02.6.

Stop: report and await review.

### 02.6 -- OSC 52 clipboard and clipboard choice

Scope: `Cargo.toml`, `Cargo.lock`, `frext-term/Cargo.toml`, `frext-term/src/clipboard.rs` (new), `frext-term/src/lib.rs`.

What: `pub struct Osc52Clipboard<W: Write> { out: W }` with
`pub fn new(out: W) -> Self`, implementing `frext_core::clipboard::
Clipboard`. `write_text` queues
`crossterm::clipboard::CopyToClipboard::to_clipboard_from(text)` to
`out` and flushes, returning `true` on success and logging and
returning `false` on error. `read_text` always returns `None`, with a
`log::debug!`. Add `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub
enum ClipboardChoice { System, Osc52 }` with `pub fn from_env(wayland:
Option<&OsStr>, display: Option<&OsStr>) -> Self`: `System` if either
is set and non-empty, else `Osc52`. Add `pub fn
make_clipboard(choice) -> Box<dyn Clipboard>` (`SystemClipboard::new()`
or `Osc52Clipboard::new(std::io::stdout())`).

Deliverable: tests `osc52_write_emits_the_escape` (into a `Vec<u8>`:
exactly `ESC ] 52 ; c ; <base64> ESC \` as crossterm encodes it, with
the base64 for `"héllo"` asserted literally), `osc52_read_is_none`,
`osc52_write_failure_returns_false` (a writer that errors), and
`from_env` for all four set/unset/empty combinations.

Verification: the standard ritual.

Adds `crossterm` (pinned as under "Further decisions") to the workspace
and to `frext-term`. `cargo tree -p frext-term -i crossterm` must still
show exactly one version.

Prohibitions: do NOT add tmux passthrough (plan 06); do NOT add a
`base64` dependency; do NOT proceed to 02.7.

Stop: report and await review.

### 02.7 -- Terminal lifecycle and panic hook

Scope: `frext-term/src/terminal.rs` (new), `frext-term/src/lib.rs`.

What:

- `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum KeyboardMode
  { Enhanced, Legacy }`.
- `pub fn write_setup(out: &mut impl Write, mode: KeyboardMode) ->
  io::Result<()>` queues `EnterAlternateScreen`, `EnableMouseCapture`,
  `EnableBracketedPaste`, and, when `Enhanced`,
  `PushKeyboardEnhancementFlags(DISAMBIGUATE_ESCAPE_CODES)`, then
  flushes.
- `pub fn write_restore(out, mode)` queues the inverse in reverse
  order (`PopKeyboardEnhancementFlags` if `Enhanced`,
  `DisableBracketedPaste`, `DisableMouseCapture`,
  `LeaveAlternateScreen`, `cursor::Show`), then flushes.
- `pub struct TerminalGuard { mode: KeyboardMode }`.
  `TerminalGuard::enter() -> io::Result<(Self, Terminal<CrosstermBackend<Stdout>>)>`:
  1. `enable_raw_mode`.
  2. `mode` = `Enhanced` iff
     `crossterm::terminal::supports_keyboard_enhancement()` is
     `Ok(true)`. This is called here, before any event reader exists.
  3. `write_setup` to stdout.
  4. Record the mode in private `static`s (`ACTIVE: AtomicBool`,
     `ENHANCED: AtomicBool`).
  5. Install the panic hook.
  6. Build the `Terminal`.

  If any step fails, it restores what it did and returns the error.
  `pub fn mode(&self) -> KeyboardMode`.
- `pub fn restore()`. It is idempotent: it runs only if
  `ACTIVE.swap(false)`. It calls `write_restore` to stdout and then
  `disable_raw_mode`, ignoring and logging errors. `Drop for
  TerminalGuard` calls `restore()`.
- `fn install_panic_hook()` wraps `std::panic::take_hook()`. The new
  hook calls `restore()`, `log::error!`s the panic info, and then calls
  the previous hook (so the message prints on the restored screen). It
  is installed once (a `std::sync::Once`).

Deliverable: tests `setup_bytes_enhanced` and `setup_bytes_legacy`
(assert the exact escape sequences, e.g. `\x1b[?1049h`, `\x1b[?2004h`,
`\x1b[>1u`, and that the legacy setup has no `\x1b[>`),
`restore_bytes_are_the_inverse` (both modes), and
`restore_is_idempotent_when_inactive` (calling `restore()` with
`ACTIVE` false writes nothing and does not panic). Raw mode itself is
not unit-tested.

Verification: the standard ritual.

Prohibitions: do NOT use `ratatui::init` / `restore`; do NOT start any
event thread; do NOT proceed to 02.8.

Stop: report and await review.

### 02.8 -- Event loop

Scope: `frext-term/src/events.rs` (new), `frext-term/src/lib.rs`.

What: `#[derive(Debug)] pub enum AppEvent { Input(crossterm::event::Event),
InputClosed, Wake, Tick }`. `pub struct EventLoop { tx: Sender<AppEvent>,
rx: Receiver<AppEvent> }` (std `mpsc`):

- `pub fn new() -> Self`.
- `pub fn spawn_input(&self, read: impl FnMut() -> io::Result<Event> +
  Send + 'static)` starts a thread that sends `Input` per event. On a
  read error it logs, sends `InputClosed` and exits. It also exits when
  the send fails.
- `pub fn spawn_tick(&self, every: Duration)` sends `Tick` on that
  interval until a send fails.
- Both spawn functions use `thread::Builder` and return
  `io::Result<()>`, so a failed spawn is an error, not a panic.
- `pub fn waker(&self) -> impl Fn() + Send + 'static` sends `Wake`,
  ignoring a send error.
- `pub fn next_batch(&self) -> Option<Vec<AppEvent>>` blocks for one
  event, then drains everything already queued with `try_recv`. It
  returns `None` only if every sender is gone (unreachable while `self`
  holds `tx`, but handled).
- `pub const TICK: Duration = Duration::from_millis(500)`.

Deliverable: tests `waker_sends_wake`,
`input_events_arrive_in_order` (a scripted reader yielding two keys
and then an error, giving `Input, Input, InputClosed`),
`tick_arrives` (a 10 ms interval, `next_batch` contains a `Tick`
within 1 s), and `next_batch_drains_queued_events` (three `Wake`s sent
before the call come back as one batch of three).

Verification: the standard ritual. Run `cargo test -p frext-term
events -- --test-threads=8` 10 times; it must never fail
(`flaky-tests-are-bugs`).

Prohibitions: do NOT call `crossterm::event::read` in tests; do NOT
proceed to 02.9.

Stop: report and await review.

### 02.9 -- Hit map, screen layout and status line

Scope: `frext-term/src/{hit,layout,status}.rs` (new),
`frext-term/src/lib.rs`.

What:

- `hit.rs`: `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum
  Target { NewTab, OpenFile, Save, SelectTab(usize), CloseTab(usize),
  PromptPopup, PromptInput, PromptButton(usize),
  PromptCandidate(usize), Editor }`. `#[derive(Default)] pub struct
  HitMap` with `clear()`, `push(Rect, Target)`, and `hit(column, row)
  -> Option<Target>` (the last pushed region containing the point).
  `Target::hint(self) -> Option<&'static str>`:

  | Target        | Hint                   |
  | ------------- | ---------------------- |
  | `NewTab`      | `New tab (Ctrl+N)`     |
  | `OpenFile`    | `Open file (Ctrl+O)`   |
  | `Save`        | `Save (Ctrl+S)`        |
  | `CloseTab(_)` | `Close tab (Ctrl+W)`   |
  | anything else | `None`                 |

- `layout.rs`: `pub struct Areas { pub tab_bar, pub find_bar, pub
  sidebar, pub gutter, pub editor, pub status: Rect }` and
  `pub fn areas(area: Rect, find_open: bool, sidebar_width: Option<u16>,
  gutter_width: u16) -> Areas`. The rows are: tab bar 1, find bar 1 if
  open (else 0), body (fill), status 1. The body splits horizontally
  into sidebar (`sidebar_width`, or 0), gutter (`gutter_width`) and
  editor (fill). Areas that do not fit come out as zero-size rects (no
  panic on a 1x1 terminal). `pub fn gutter_width(line_count: usize) ->
  u16` is `max(digits, 2) + 1`.
- `status.rs`: `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum
  MessageKind { Info, Error }`. `pub struct StatusLine` holds
  `message: Option<(String, MessageKind, Instant)>` and
  `hint: Option<&'static str>`. Methods:
  - `set_message(text, kind, now)`
  - `set_hint(Option<&'static str>)`
  - `expire(now) -> bool` (it drops a message older than
    `MESSAGE_TTL = 4 s` and returns whether anything changed)
  - `render(&self, area, buf, left_default: &str, right: &str)`

  The left side shows the message (in `theme::status_error()` for
  errors), else the hint, else `left_default`, and is truncated to fit.
  The right side shows `right`, right-aligned, and is dropped when it
  would overlap the left.

Deliverable: tests for `HitMap` (overlap resolves to the last pushed,
miss is `None`), each `hint`, `areas` (find bar on and off, sidebar on
and off, a 1x1 area, and every area within bounds), `gutter_width` (1,
9, 10, 100000 lines), and `StatusLine` (priority order, expiry
boundary, and a rendered `Buffer` for an error message and for
truncation).

Verification: the standard ritual.

Prohibitions: do NOT render the tab bar or editor; do NOT proceed to
02.10.

Stop: report and await review.

### 02.10 -- Tab bar

Scope: `frext-term/src/tab_bar.rs` (new), `frext-term/src/lib.rs`.

What: `pub struct TabBarItem { pub target: Target, pub label: String,
pub x: u16, pub width: u16 }` and `pub fn items(tabs: &[Tab], active:
usize, width: u16) -> Vec<TabBarItem>`. The fixed items come first,
each label padded with one space on both sides: `+` (`NewTab`), `Open`
(`OpenFile`) and `Save` (`Save`), then a `│` separator column. Each tab
follows as its `tab.title()` padded the same way (`SelectTab(i)`), then
`×` followed by one space (`CloseTab(i)`), then a `│`. Widths use `unicode-width` (via ratatui's `Span::width`, so no
direct dependency is needed yet). When the tabs overflow, the tab
region scrolls just enough for the active tab and its `×` to be fully
visible. The fixed items never scroll. A tab is atomic: its title and
`×` are shown only if both fit fully inside the visible window, so a
close glyph is never shown for a hidden title. `pub fn render(items, active, hovered: Option<Target>, area,
buf, hits: &mut HitMap)` fills the row with `theme::chrome()`, draws
the active tab with `theme::active_tab()` and `×` with
`theme::close_glyph()`, sets `theme::hover_bg()` as the background of
the hovered item, and pushes one hit region per item.

Deliverable: tests `fixed_items_come_first`, `dirty_tab_shows_bullet`,
`overflow_scrolls_active_tab_into_view` (10 tabs in 40 columns, active
= 9), `active_first_tab_needs_no_scroll`, `render_registers_hits`
(clicking the `×` column of tab 1 gives `CloseTab(1)`), and
`render_hover_sets_background`.

Verification: the standard ritual.

Prohibitions: do NOT handle mouse events here (the app does); do NOT
proceed to 02.11.

Stop: report and await review.

### 02.11 -- Single-line text input

Scope: `Cargo.toml`, `Cargo.lock`, `frext-term/Cargo.toml` (adds
`unicode-width`),
`frext-term/src/text_input.rs` (new), `frext-term/src/lib.rs`.

What: `#[derive(Debug, Default, Clone, PartialEq, Eq)] pub struct
TextInput { cursor: usize, scroll: usize }` (char indices) operates on
an external `&mut String`, because the text of a name prompt lives in
core. `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum
InputOutcome { Edited, Moved, Submit, Cancel, Ignored }`. Methods:

- `handle_key(&mut self, text: &mut String, key: KeyEvent) ->
  InputOutcome`:
  - a printable char with no Ctrl/Alt inserts it (`Edited`)
  - `Backspace` / `Delete` (`Edited` if anything changed, else
    `Moved`)
  - `Left` / `Right` / `Home` / `End` (`Moved`)
  - `Enter` gives `Submit`, `Esc` gives `Cancel`
  - everything else, including `Tab` and `BackTab`, gives `Ignored`,
    so callers can bind them
- `insert_str(&mut self, text: &mut String, s: &str)`, for paste, with
  newlines replaced by spaces
- `move_to_end(&mut self, text: &str)`
- `click(&mut self, text: &str, column: u16)` (a column relative to the
  field, mapped through display widths)
- `render(&mut self, text: &str, area: Rect, buf: &mut Buffer) ->
  Option<Position>`, which draws with `theme::input()`, scrolls
  horizontally so the cursor stays inside `area`, and returns the
  terminal cursor position
- the cursor is clamped to `text.chars().count()` at the start of every
  method, because core may have changed the text

Deliverable: tests for insert and delete at start, middle and end with
multi-byte chars (`é`, `日本`), Home/End, clamping after an external
shrink, `insert_str` newline handling, `click` over wide chars,
`render` scroll (a long text with the cursor at the end in a 10-wide
area shows the tail, and the cursor position is inside the area), and
`Tab` giving `Ignored`.

Verification: the standard ritual.

Prohibitions: do NOT add selection or word motions (plan 03 owns
editing semantics); do NOT proceed to 02.12.

Stop: report and await review.

### 02.12 -- Path completion

Scope: `frext-term/src/complete.rs` (new), `frext-term/src/lib.rs`.

What:

- `pub fn resolve(input: &str, base: &Path, home: Option<&Path>) ->
  PathBuf`: `~` or `~/…` expands to `home` when given, absolute paths
  stay as they are, and relative paths join onto `base`.
- `pub fn candidates(input: &str, base: &Path, home: Option<&Path>) ->
  Vec<String>`. The input splits at its last `/` into a dir part and a
  name prefix. The resolved dir is listed with `std::fs::read_dir`
  (errors give an empty list). Entries are filtered by a case-sensitive
  prefix match, and dotfiles are included only when the prefix starts
  with `.`. Each candidate is the **full input text** with the
  completed name, plus a trailing `/` for directories. The list is
  sorted with directories first, then by name.
- `pub fn longest_common_prefix(items: &[String]) -> String`
  (char-boundary safe).
- `#[derive(Debug, Default)] pub struct Completion { cycle:
  Option<(Vec<String>, usize)> }` holds the Tab state:
  - `tab(&mut self, text: &mut String, base, home, direction:
    StepDirection) -> CompletionView`
  - `reset(&mut self)`, called by the prompt on any edit
  - `pub struct CompletionView { pub candidates: Vec<String>, pub
    selected: Option<usize> }` (empty means draw no list)

  Behaviour on the first Tab:
  - no candidates: nothing changes
  - one candidate: replace the text with it (no list)
  - several: replace the text with their longest common prefix if that
    is longer than the text (no list yet), else start cycling, which
    sets the text to the first candidate (or the last for `Backward`)
    and returns the list with that one selected

  Later Tabs while cycling step the selection (wrapping) and set the
  text to it.

Deliverable: tests over a temp dir (`src/`, `src/main.rs`, `Cargo.toml`,
`Cargo.lock`, `.hidden`):

- `~` expansion
- relative and absolute resolution
- dir-first ordering
- dotfile filtering
- a single match completes `src` to `src/`
- the common prefix extends `Ca` to `Cargo.`
- a second Tab starts cycling and `Backward` wraps
- `reset` stops cycling
- an unreadable dir gives no candidates
- `longest_common_prefix` with multi-byte strings

Verification: the standard ritual.

Prohibitions: do NOT render anything; do NOT read `$HOME` inside these
functions (the caller passes it); do NOT proceed to 02.13.

Stop: report and await review.

### 02.13 -- Popup frame and core prompts (ConfirmClose, Name)

Scope: `frext-term/src/{popup,prompt}.rs` (new), `frext-term/src/lib.rs`.

What:

- `popup.rs`:
  - `pub fn centered(area: Rect, width: u16, height: u16) -> Rect`,
    clamped to `area`.
  - `pub fn render_frame(title: &str, area, buf, hits) -> Rect`. It
    draws `Clear`, then a bordered `Block` with `theme::popup()` /
    `theme::popup_border()` and the title, pushes
    `Target::PromptPopup`, and returns the inner rect.
  - `pub fn render_buttons(labels: &[&str], focused: usize, row: Rect,
    buf, hits)`. The buttons are right-aligned, each drawn as
    `[ label ]`, with the focused one in `theme::button_focused()`,
    each pushing `PromptButton(i)`.
- `prompt.rs`: `pub struct PromptUi` holds the TUI view state for the
  current prompt:
  - `key: Option<PromptKey>`, a private enum that identifies the
    prompt: `ConfirmClose(tab_id)`, `SavePath(tab_id)`,
    `Name(kind, target)`, `Open`
  - `focus: PromptFocus`, a private enum `{ Input, Button(usize) }`
  - `input: TextInput`

  `pub fn sync(&mut self, editor: &Editor)` resets the state when the
  current prompt's key differs from `key` (this is the "first time
  seen" reset; the cursor goes to the end of the text). Then:
  - `pub fn handle_key(&mut self, editor: &mut Editor, key: KeyEvent)
    -> PromptReply`
  - `pub fn handle_click(&mut self, editor: &mut Editor, target:
    Option<Target>, column: u16) -> PromptReply`
  - `pub fn handle_paste(&mut self, editor: &mut Editor, text: &str)`
  - `pub fn render(&mut self, editor: &Editor, area, buf, hits) ->
    Option<Position>`

  `pub enum PromptReply { Send(Command), Handled, Ignored }` (the app
  sends the command). This subtask handles:
  - **ConfirmClose**: title `Unsaved changes`, message
    `“{display_name}” has unsaved changes. What would you like to do?`
    (wrapped, 60 columns wide). Buttons `Save and close`,
    `Discard changes` and `Cancel` map to
    `ResolveClose(Save|Discard|Cancel)`. Left/Right/Tab/BackTab move
    the focus (wrapping), Enter activates the focused button, Esc
    cancels, a click on a button activates it, and a click outside
    the popup (`target` not `PromptPopup` and not any `Prompt*`)
    cancels. A missing tab sends `CancelPrompt`, as in the GUI.
  - **Name**: the title, field label and action label per `NameKind`
    match the GUI (`Rename` / `New name` / `Rename`, `New file` /
    `File name` / `Create`, `New folder` / `Folder name` / `Create`).
    The input is bound to `editor.prompt_name_mut()`. The error line
    is shown in `theme::status_error()` colours on the popup
    background. Enter or the action button sends `SubmitName`, and Esc,
    `Cancel` or a click outside sends `CancelPrompt`. Tab/BackTab moves
    between the input and the two buttons, and while a button is
    focused, Enter activates it.
  - `SavePath` / Open are stubbed to `Ignored` / not rendered until
    02.14.
  - `pub fn focus_field(&mut self, editor: &Editor)` (for
    `Effect::FocusPromptField`) focuses the input and moves the cursor
    to the end.
  - `PromptUi` remembers the input field's rect from the last render,
    so a click on `PromptInput` focuses the field and calls
    `TextInput::click` with the column relative to that rect.

Deliverable: tests that drive a real `Editor` (`Store::at(temp)`,
`MemoryClipboard`):

- dirty tab + `RequestCloseActive` + Enter on the default button gives
  `Send(ResolveClose(Save))`
- Tab, Tab, Enter gives `Cancel`
- Esc gives `Cancel`
- a click outside gives `Cancel`
- a click on button 1 gives `Discard`
- Name: typing edits `prompt_name_mut`, Enter gives `SubmitName`, and
  an error from a failed submit renders
- `sync` resets the focus when a new prompt replaces the old one
- a `TestBackend` render of ConfirmClose shows the title, message and
  three buttons centred

Verification: the standard ritual.

Prohibitions: do NOT implement SavePath / Open (02.14); do NOT call
`editor.apply` from `prompt.rs` (return `Send`); do NOT proceed to
02.14.

Stop: report and await review.

### 02.14 -- Path prompts (SavePath and Open)

Scope: `frext-term/src/prompt.rs`.

What: `PromptUi` gains `path_text: String`, `completion: Completion`,
`candidates: CompletionView`, `overwrite_armed: Option<PathBuf>`,
`base: PathBuf` and `home: Option<PathBuf>` (set by
`PromptUi::new(base, home)`), plus
`pub fn open_file(&mut self, editor: &Editor) -> bool`. That method
starts the TUI-only Open prompt and returns `false`, without opening
it, if `editor.prompt()` is `Some`. `pub fn is_open(&self, editor)
-> bool` is true for a core prompt or a local Open prompt. Both
prompts:

- show the title, then the field, then a hint line
  `relative to {base}` (or `{workspace root}`; the app passes
  `editor.workspace()` root as `base` when there is one), then up to 8
  candidate rows (each a `PromptCandidate(i)` hit, the selected one in
  `theme::button_focused()`, plus `… N more` if there are more), then
  the buttons
- Tab / BackTab run `Completion::tab`. Any `Edited` input calls
  `completion.reset()`, clears the candidates and clears
  `overwrite_armed`. A click on a candidate sets the text to it, resets
  the completion and focuses the input.
- give an inline error line using the 02.13 Name styling
- keep keyboard focus on the field, because Tab is taken by
  completion: Enter always submits, and the buttons are mouse-only
  (Esc still cancels)

Per prompt:

- **SavePath** (title `Save as`, buttons `Save` / `Cancel`):
  - Enter resolves the path. An existing directory gives the error
    `“{p}” is a directory`. A missing parent gives the error
    `Folder “{parent}” does not exist`. An existing file the first time
    sets `overwrite_armed` and the error
    `“{name}” exists. Press Enter again to replace it`, and a second
    Enter on the same path sends `SubmitSavePath(path)`. Otherwise it
    sends `SubmitSavePath(path)`.
  - Esc, `Cancel` or a click outside sends `CancelPrompt`.
- **Open** (title `Open file`, buttons `Open` / `Cancel`):
  - Enter resolves the path. A file sends `OpenPath(path)` and closes
    the local prompt. A directory gives the error
    `“{p}” is a directory`. A missing path gives the error
    `No such file: {p}`.
  - Esc, `Cancel` or a click outside closes it locally and returns
    `Handled`.

Deliverable: tests over a temp dir:

- SavePath to a new file gives `Send(SubmitSavePath)` with the
  resolved path
- SavePath to an existing file needs two Enters
- SavePath to a directory and to a missing parent show the errors
- `open_file` is refused while a `ConfirmClose` prompt is set
- Open to a file gives `OpenPath` and the prompt closes
- Open to a directory shows the error
- Tab completes inside the prompt and the candidate list renders (a
  `TestBackend`)
- clicking a candidate fills the field
- paste inserts into the field

Verification: the standard ritual.

Prohibitions: do NOT add a fuzzy finder; do NOT create directories; do
NOT proceed to 02.15.

Stop: report and await review.

### 02.15 -- Keymap

Scope: `frext-term/src/keymap.rs` (new), `frext-term/src/lib.rs`.

What:

- `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum Focus {
  Editor, Sidebar }`.
- `#[derive(Debug, Clone, PartialEq, Eq)] pub enum Action {
  Command(Command), OpenFilePrompt, Quit, ToggleSidebar, ToggleFocus,
  FocusEditor, Scroll(ScrollBy) }`, where `pub enum ScrollBy { Lines(i32),
  Pages(i32), Top, Bottom }`.
- `pub struct KeyContext { pub keyboard: KeyboardMode, pub
  search_open: bool, pub focus: Focus }`.
- `pub fn action_for(key: KeyEvent, ctx: KeyContext) -> Option<Action>`.
  `Release` events return `None`, and `Repeat` is treated as `Press`.

| Key                         | Action                                                 |
| --------------------------- | ------------------------------------------------------ |
| Ctrl+N                      | `Command(NewTab)`                                      |
| Ctrl+O                      | `OpenFilePrompt`                                       |
| Ctrl+S                      | `Command(Save)`                                        |
| Ctrl+W                      | `Command(RequestCloseActive)`                          |
| Ctrl+F                      | `Command(OpenSearch(Find))`                            |
| Ctrl+R                      | `Command(OpenSearch(FindReplace))`                     |
| Ctrl+H (`Enhanced` only)    | `Command(OpenSearch(FindReplace))`                     |
| F3 (search open)            | `Command(StepMatch(Forward))`                          |
| Shift+F3 (search open)      | `Command(StepMatch(Backward))`                         |
| Esc (search open)           | `Command(CloseSearch)`                                 |
| Esc (focus Sidebar, closed) | `FocusEditor`                                          |
| Ctrl+Q                      | `Quit`                                                 |
| Ctrl+B                      | `ToggleSidebar`                                        |
| Ctrl+E                      | `ToggleFocus`                                          |
| Up / Down (focus Editor)    | `Scroll(Lines(-1 / 1))`                                |
| PageUp / PageDown (Editor)  | `Scroll(Pages(-1 / 1))`                                |
| Ctrl+Home / Ctrl+End        | `Scroll(Top / Bottom)`                                 |

  Ctrl letter matching is case-insensitive and accepts Shift (kitty
  may report Ctrl+Shift+N with an upper-case char). Shift+F3 is also
  matched when a terminal reports it as `F(15)`, as xterm-style
  terminals do.

Deliverable: one test per row. Plus Ctrl+H in `Legacy` gives `None`,
F3 and Esc with search closed give `None`, a `Release` gives `None`,
and a plain `n` gives `None`.

Verification: the standard ritual.

Prohibitions: do NOT bind editing keys (plan 03); do NOT proceed to
02.16.

Stop: report and await review.

### 02.16 -- Read-only editor view

Scope: `frext-term/src/editor_view.rs` (new), `frext-term/src/lib.rs`.

What: `#[derive(Default)] pub struct EditorView { top: HashMap<u64,
usize> }` holds the first visible line per tab id.

- `pub fn scroll(&mut self, tab: &Tab, by: ScrollBy, height: u16)`.
  `Pages(n)` moves `n * max(height - 1, 1)` lines. The result is
  clamped to `0..=line_count - 1`.
- `pub fn wheel(&mut self, tab, lines: i32)`.
- `pub fn reveal(&mut self, tab, chars: Range<usize>, height)` puts
  the line holding `chars.start` at the vertical centre.
- `pub fn forget(&mut self, live_ids: impl Iterator<Item = u64>)`
  drops closed tabs.
- `pub fn render(&self, tab: Option<&Tab>, gutter: Rect, editor: Rect,
  buf, hits)`. It draws the visible lines of `tab.text` (tabs expanded
  to 4 spaces, no wrap, cut at the editor width by display width) in
  `theme::base()`, right-aligned 1-based line numbers in
  `theme::gutter()` with one trailing space column, fills the rest
  with `BASE`, and pushes `Target::Editor` for the editor rect. With
  no tab, it fills both rects with `BASE`.

Deliverable: tests for scroll clamping (top, bottom, pages), `reveal`
(centre and clamp at the start), `forget`, render (`TestBackend`: line
numbers right-aligned, tab expansion, truncation of a long line, a
scrolled view starting at line 5, and an empty buffer showing line 1),
and a multi-byte line.

Verification: the standard ritual.

Prohibitions: do NOT add a caret, selection, editing or highlighting
(plan 03); do NOT proceed to 02.17.

Stop: report and await review.

### 02.17 -- `TermApp`: event handling and render

Scope: `frext-term/src/app.rs` (new), `frext-term/src/lib.rs`.

What: `pub struct TermApp` holds:

- `editor: Editor`
- `keyboard: KeyboardMode`
- `focus: Focus`
- `sidebar: SidebarVisibility { Shown, Hidden }`, a private enum, so
  the sidebar is shown iff a workspace exists and it is `Shown`, with
  width 30
- `prompt: PromptUi`
- `view: EditorView`
- `status: StatusLine`
- `hovered: Option<Target>`
- `hits: HitMap`
- `last_editor_height: u16`
- `effects: Vec<Effect>`
- `quit: Quit { Running, Requested }`, a private enum

Methods:

- `pub fn new(editor, keyboard, base: PathBuf, home: Option<PathBuf>)
  -> Self`. In `Legacy` mode it sets the Info message
  `Kitty keyboard protocol unavailable: use Ctrl+R for find and
  replace`.
- `pub fn handle(&mut self, event: AppEvent, now: Instant)` routes:
  - **Key** (`Press` / `Repeat` only):
    1. `prompt.sync`
    2. If a prompt is open, `PromptUi::handle_key`. A `Send(c)` goes
       to `send(c)`.
    3. Otherwise `keymap::action_for`:
       - `Command(c)` goes to `send(c)`, and for
         `OpenSearch(_)` also sets the message
         `Find bar is not available yet`
       - `OpenFilePrompt` calls `prompt.open_file`
       - `Quit` sets `quit`
       - `ToggleSidebar`, `ToggleFocus`, `FocusEditor` update the
         state (ToggleFocus only while the sidebar is shown)
       - `Scroll` scrolls the view using `last_editor_height`
  - **Mouse**:
    - `Moved` / `Drag` update `hovered` and `status.set_hint(target.hint())`
      from `hits.hit`
    - `Down(Left)`: with a prompt open it goes to
      `PromptUi::handle_click`; otherwise it maps
      `NewTab` / `Save` / `SelectTab` / `CloseTab` to `NewTab` /
      `Save` / `SelectTab(i)` / `RequestClose(i)`, and `OpenFile` to
      the open prompt
    - `ScrollUp` / `ScrollDown` over `Target::Editor` call
      `view.wheel(∓3)`
  - **Paste**: goes to the prompt if one is open, otherwise sets the
    message `Editing arrives in a later version`
  - **Resize**: nothing (the redraw follows)
  - **Tick**: `status.expire(now)`
  - **Wake**: nothing
  - **InputClosed**: sets `quit`
- `send` applies the command and queues the outcome's effects.
- `pub fn apply_effects(&mut self)` drains the queue as described
  under "Further decisions", then calls
  `view.forget(editor.tabs().iter().map(|t| t.id))`.
- `pub fn render(&mut self, frame: &mut Frame)` clears `hits` and draws
  the parts in this order:
  1. the layout `areas` (find bar open iff `search().is_open()`)
  2. the tab bar
  3. the find bar row, filled with `chrome()`
  4. the sidebar area, filled with `chrome()`
  5. the editor view
  6. the status line (left default
     `relative_to_root(path)` or `display_name()` for untitled, right
     `{language or "plain text"} · {n} lines`)
  7. the prompt, last, which sets the cursor position when it returns
     one

  It records `last_editor_height`.
- `pub fn should_quit(&self) -> bool`.
- `pub fn editor(&self) -> &Editor` (for tests and `run`).
- `pub fn shutdown(&self)` calls `editor.shutdown()`.

Deliverable: `TestBackend` tests over a real `Editor` in a temp dir:

- the first frame shows the tab bar, gutter and status
- a Ctrl+N keypress adds a tab
- clicking `×` on a dirty tab opens ConfirmClose, then Esc cancels and
  the tab stays
- hovering `+` puts `New tab (Ctrl+N)` on the status row
- Ctrl+O, typing a path and Enter opens the file in a new active tab
- Ctrl+Q sets `should_quit`
- in `Legacy` mode Ctrl+R opens `search` and Ctrl+H does not
- with a workspace, Ctrl+B hides and shows the sidebar column
- the wheel over the editor scrolls
- a Tick after 4 s clears the message
- `InputClosed` quits

Verification: the standard ritual.

Prohibitions: do NOT touch a real terminal; do NOT add features beyond
this list; do NOT proceed to 02.18.

Stop: report and await review.

### 02.18 -- Run loop and `main`

Scope: `frext-term/src/run.rs` (new), `frext-term/src/lib.rs`,
`frext-term/src/main.rs`.

What: `pub fn run(app: &mut TermApp, terminal: &mut Terminal<impl
Backend>, events: &EventLoop, now: impl Fn() -> Instant) ->
io::Result<()>` loops over these steps:

1. `editor.begin_frame(events.waker())` (through a
   `TermApp::begin_frame(notify)` passthrough added here)
2. draw
3. `end_frame`
4. `next_batch`, then `begin_frame` again, `handle` for each event,
   and `apply_effects`

It stops when `should_quit`, or when `next_batch` returns `None`. The
draw comes after `apply_effects` so that effects land before
rendering. `main.rs`:

1. Parse args.
2. Build `Store::new(Profile::Term)`.
3. Initialise logging.
4. Pick the clipboard with `ClipboardChoice::from_env(var_os
   WAYLAND_DISPLAY, var_os DISPLAY)`, logging the choice.
5. `Editor::open`.
6. Capture `base` (the canonicalised workspace dir, else
   `current_dir()`) and `home` (`var_os("HOME")`).
7. `TerminalGuard::enter()`. On error, `eprintln!` and exit 1, after
   `editor.shutdown()`.
8. Build the `EventLoop`, then `spawn_input(crossterm::event::read)`
   and `spawn_tick(TICK)`. This happens after `enter`, so the keyboard
   query is never raced by the reader.
9. `run`, then `app.shutdown()`, then drop the guard (restore).
10. On a `run` error, log it, `eprintln!` it after the restore, and
    exit 1.

Deliverable: a `run` test that uses a `TestBackend`, an `EventLoop`
with no input thread, and pre-queued events (Ctrl+N, then Ctrl+Q sent
through the event loop's sender, using a `pub(crate)` test-only
`EventLoop::sender()`). It returns `Ok`, and the editor has two tabs.

Verification: the standard ritual, plus `nix build .#frext` (the GUI
package still builds with the new member), the 02.4 `cargo tree`
checks, and the manual smoke checklist below, run by the orchestrator
in a real terminal.

Prohibitions: do NOT change `flake.nix` packaging (plan 09); do NOT
proceed further.

Stop: report and await review.

## Manual smoke checklist

Run in a real terminal (freminal or kitty, then a legacy terminal such
as `xterm`, or `TERM=xterm-256color` in tmux):

1. `frext-term` with no session opens one untitled tab. The tab bar,
   gutter and status line are drawn in Catppuccin colours.
2. `frext-term file1 file2 dir/` opens two tabs, and a sidebar column
   is shown (empty). Ctrl+B hides and shows it.
3. Ctrl+N, then Ctrl+W on a clean tab, closes it. Ctrl+O with Tab
   completion opens a file.
4. On a dirty tab (dirty from a swap restore, or from a GUI session
   edit), Ctrl+W shows the prompt, where Save, Discard and Cancel work
   by keyboard and by mouse. Ctrl+S on an untitled tab shows Save as,
   and overwriting asks twice.
5. Click tabs, `×`, `+`, Open and Save. Hovering shows the hints on the
   status line. The wheel scrolls the view.
6. Ctrl+F shows the "not available yet" message. Esc closes the search.
7. Kitty protocol terminal: Ctrl+H opens replace. Legacy terminal:
   Backspace does nothing, Ctrl+R opens replace, and the startup
   message explains it.
8. Modify an open clean file externally: it reloads within about 1 s.
9. Run under `ssh localhost` (no `DISPLAY`). The log says OSC 52.
10. Ctrl+Q restores the terminal fully: the mouse, paste, keyboard mode
    and cursor all behave normally in the shell afterwards. Then
    trigger a panic. There is no production panic path, so use a
    temporary local patch that is **not** committed. The terminal is
    restored, and the message prints and is logged.
11. Run the GUI and the TUI at once: their sessions are independent,
    and a word added to the dictionary in each survives.
12. `~/.local/state/frext/term/frext-term.log` exists, and the previous
    log was rotated to `.old`.
