# 06 -- Terminal capabilities and undercurl

- **Status:** complete
- **Depends on:** 02
- **Branch:** `feat/frext-term-capabilities` (one branch, one commit per
  subtask, one PR)

## Goal

Detect what the hosting terminal supports with a single startup probe,
record it in a `TerminalCaps` value that all rendering reads, let the
user override it, and ship a custom ratatui backend that draws curly,
coloured underlines ("undercurl"). Plans 07 (squiggles) and 08 (kitty
image icons) consume this.

## Task summary

- `frext-term::term::probe`: write one batch of queries, read the
  replies from stdin with a timeout, parse into `TerminalCaps`.
- `frext-term::term::backend::FrextBackend`: a ratatui `Backend` that is
  the stock crossterm backend's `draw` loop plus the undercurl rule.
- User overrides from config and CLI flags.
- The interim crossterm keyboard-enhancement check from plan 02 is
  folded into the probe.

## Durable decisions

### Escape sequences

- Undercurl on: `CSI 4:3 m` (colon sub-parameter). Never the semicolon
  form `4;3`, which many terminals read as "underline + italic".
  Underline off: `CSI 24 m`. crossterm's `Attribute::Undercurled` emits
  `4:3`.
- Underline colour: `CSI 58:2::r:g:b m` / `CSI 58;2;r;g;b m`, reset
  `CSI 59 m` (crossterm `SetUnderlineColor`, ratatui `Style::underline_color`
  behind the `underline-color` feature).

### Why a custom backend

ratatui 0.30's `Modifier` has no undercurl (only `UNDERLINED`), and the
stock `CrosstermBackend::draw` only ever emits a straight underline.
`FrextBackend` copies that roughly 150-line loop and adds one rule:
a cell with `Modifier::UNDERLINED` **and** a non-`Reset`
`underline_color` is drawn with `Attribute::Undercurled` when
`TerminalCaps::underline == UnderlineStyle::Curly`, else with a
straight underline. frext uses underline colour only for proofreading,
so the rule is unambiguous and the rest of the UI keeps plain ratatui
styles. Its tests draw into a `Vec<u8>` and assert the exact bytes, as
ratatui's own backend tests do.

Rejected alternative: embedding `CSI 4:3 m` inside a cell's symbol with
`CellDiffOption::ForcedWidth`. It works, but it desynchronises the
backend's tracked style state. That trick is reserved for image
transmission (plan 08).

Other ways of sending escapes from ratatui, for reference: in-cell
symbols with `ForcedWidth` (per-cell payloads), and out-of-band writes
through `terminal.backend_mut()` between frames (one-shot commands:
OSC 52, keyboard-protocol push/pop, window title, kitty image upload
and delete).

### The probe

One write, one read loop, run after enabling raw mode and **before**
crossterm's event reader starts (otherwise replies are swallowed as
key events). The queries, in this order:

| Query                                         | Purpose                                                    |
| --------------------------------------------- | ---------------------------------------------------------- |
| `ESC _G i=31,s=1,v=1,a=q,t=d,f=24;AAAA ESC \` | Kitty graphics. An `OK` reply means supported.             |
| `CSI 16 t`                                    | Cell size in pixels (`CSI 6;h;w t`), for icon raster size. |
| `CSI > q` (XTVERSION)                         | Terminal name and version (`DCS >\| ... ST`).              |
| `DCS + q 536d756c78 ST` (XTGETTCAP `Smulx`)   | Styled-underline support.                                  |
| `CSI ? u`                                     | Kitty keyboard protocol flags.                             |
| `CSI c` (DA1)                                 | Terminator. Every terminal answers it; stop reading here.  |

Reads use a non-blocking poll on stdin (via `rustix`, which wraps the
syscalls safely; this crate allows no `unsafe`) with a default timeout
of about 1 s. Unix only for now. Reply parsing is a pure function over
bytes, tested against recorded replies from kitty, Ghostty, foot,
WezTerm, freminal, Alacritty and tmux.

```rust
pub struct TerminalCaps {
    pub images: ImageSupport,        // KittyPlaceholders { cell_px } | None
    pub underline: UnderlineStyle,   // Curly | Straight
    pub keyboard: KeyboardProtocol,  // Kitty { flags } | Legacy
    pub multiplexer: Multiplexer,    // None | Tmux
    pub version: Option<String>,     // raw XTVERSION reply
}
```

- **Images**: `KittyPlaceholders` only when the kitty query got `OK`,
  a cell size came back, and neither `WEZTERM_EXECUTABLE` nor
  `KONSOLE_VERSION` is set. Both of those answer the query but do not
  implement unicode placeholders correctly (the same blocklist
  ratatui-image uses). Sixel and iTerm2 are never used for icons: they
  draw outside the cell grid, so every scroll of the tree would re-send
  and clear every icon.
- **Underline**: `Curly` when XTGETTCAP returns `Smulx`, or when
  XTVERSION / `TERM_PROGRAM` names a terminal known to support it.
- **tmux**: detected from `TMUX` / `TERM`. Kitty graphics escapes are
  wrapped for passthrough (`ESC P tmux; ... ESC \` with inner ESCs
  doubled); passthrough requires the user's `allow-passthrough on`.
  Undercurl through tmux needs
  `set -as terminal-features ',*:usstyle'`.

### Why our own probe, not ratatui-image's `Picker`

ratatui-image's `Picker::from_query_stdio()` decides by query, not by a
hard-coded terminal list (the `OK` reply is authoritative; env vars
only exclude WezTerm/Konsole or guess iTerm2), and it allows forcing a
protocol with `set_protocol_type`. It would work. We still write our
own because:

1. frext needs XTVERSION, XTGETTCAP `Smulx` and the kitty keyboard
   query anyway, and one combined probe is one round trip.
2. frext only ever wants kitty-with-placeholders or nothing, and plan 08
   needs per-cell control of placeholder encoding (see there), which
   ratatui-image's kitty renderer does not offer.
3. It avoids `image`, `rand`, `flate2`, `base64-simd` and an optional
   C library (`libchafa`) for a feature that needs a fraction of them.

### User overrides

- Config (shared `$XDG_CONFIG_HOME/frext/config.toml`, the file the
  Home Manager module already writes), `[term]` table:
  `icons = "auto" | "kitty" | "nerd" | "ascii" | "none"` and
  `undercurl = "auto" | "on" | "off"`. Plan 08 consumes `icons`.
- CLI flags `--icons <mode>` and `--undercurl <mode>` override config.
- Forcing `icons = "kitty"` still runs the probe for the cell size; with
  no cell size it logs a warning and falls back to `nerd`.

### freminal (fred's terminal)

Audited against freminal 0.13.0-beta.2 source:

| Capability              | freminal                                                |
| ----------------------- | ------------------------------------------------------- |
| Kitty graphics query    | `ESC _Gi=31;OK ESC \`                                   |
| `CSI 16 t`              | Answered immediately (`CSI 6;h;w t`)                    |
| XTVERSION               | `DCS >\|XTerm(Freminal <version>) ST`                   |
| XTGETTCAP `Smulx`       | Yes (also `Setulc`, `RGB`, `Tc`)                        |
| `4:3` + `58:2::r:g:b`   | Parsed and drawn as a sine wave in the underline colour |
| Kitty keyboard protocol | Yes (query, push, pop, set)                             |
| DA1                     | `CSI ?65;1;2;4;6;17;18;22 c` (advertises sixel too)     |
| OSC 52                  | Write allowed by default; read off by default           |
| `TERM` / `TERM_PROGRAM` | `xterm-256color` / `freminal`                           |

So freminal is detected as `KittyPlaceholders` + `Curly` + `Kitty`
keyboard purely by query, with no special-casing. Two freminal quirks
that matter elsewhere: `CSI 14 t` reports the outer viewport and is
answered asynchronously (the probe uses `16 t`, so this does not
matter), and `DSR 1337n` gets no reply (also unused).

## Decided at activation

- **Undercurl when nothing answers either way: `Straight`.** Curly is
  chosen only on an XTGETTCAP `Smulx` reply, or when XTVERSION,
  `TERM_PROGRAM` or `TERM` names a terminal known to support it. Users
  can force it with `undercurl = "on"`.
- **The shared config file and its loader live in `frext-core`**
  (`frext_core::config`), so the GUI can read its own `[gui]` table
  later. Core stays toolkit-free: the config holds only plain enums.
- **Probe timeout 1 s, no cache.** The read stops as soon as the DA1
  reply arrives, so the timeout only matters for a terminal that never
  answers.
- **tmux's own answers are trusted.** They describe what actually
  survives tmux (tmux reports `Smulx` only with `usstyle` set). Only
  the kitty graphics query is wrapped for passthrough. Under tmux,
  undercurl is used only on a `Smulx` reply: the known-terminal list
  does not apply there.

## Further decisions (orchestrator)

These come from reading the current code: `frext-term`'s `terminal.rs`,
`main.rs`, `cli.rs`, `run.rs`, `app.rs` (its `KeyboardMode` uses) and
`theme.rs`; `frext-core`'s `persistence.rs` and `error.rs`; the Home
Manager module; ratatui-crossterm 0.1.2's `CrosstermBackend`; crossterm
0.29.0's SGR output; and rustix 1.1.6.

- **Facts from the audit.**
  - crossterm writes the underline colour as `CSI 58;2;r;g;b m`
    (semicolons) and its reset as `CSI 59 m`. Both forms are accepted
    above; the semicolon one is what the backend emits.
  - `theme::span()` already sets `Modifier::UNDERLINED` with **no**
    underline colour for syntax-theme underlines. The undercurl rule
    keys on a non-`Reset` underline colour, so those stay straight.
  - ratatui-crossterm's `ModifierDiff` is private, so `FrextBackend`
    copies it. `IntoCrossterm` (colour conversion) is public as
    `ratatui::backend::IntoCrossterm`. No direct `ratatui-crossterm`
    dependency is needed.
  - With `NO_COLOR` set, crossterm writes no colours at all. Byte-exact
    backend tests call `crossterm::style::Colored::set_ansi_color_disabled(false)`
    first.
  - rustix 1.1.6 is already in the lock (via crossterm), but its
    `event` feature (`rustix::event::poll`) is not enabled by the TUI's
    own graph. frext-term depends on rustix directly with
    `features = ["event", "stdio"]`.
  - No config loader exists. The Home Manager module writes
    `programs.frext.settings` verbatim as TOML to
    `$XDG_CONFIG_HOME/frext/config.toml` and defines no keys, so
    unknown keys (e.g. its example `theme`) must be ignored.
- **New dependencies** (pinned, alphabetical, workspace-level):
  - `toml = { version = "1.2.1", default-features = false, features = ["parse", "serde"] }`
    for `frext-core`. (Planned with `"std"` as well; 06.1 found
    `parse` + `serde` is the minimal set that parses into serde types,
    and `toml::de::Error` still implements `Error` without `std`.)
  - `rustix = { version = "1.1.6", features = ["event", "stdio"] }` for
    `frext-term`.
- **Modules.**
  - `frext-core/src/config.rs` (new): `Config`, `TermConfig`,
    `IconsMode`, `UndercurlMode`, loading. `ConfigError` goes in
    `error.rs`.
  - `frext-term/src/term/mod.rs` (new): module root for terminal
    capabilities. It sits beside the existing `terminal.rs` (terminal
    lifecycle), which stays where it is.
  - `term/caps.rs`: the capability types, `TermEnv`, `resolve` and
    `IconChoice`.
  - `term/tmux.rs`: tmux passthrough wrapping (the probe uses it now,
    plan 08's uploads later).
  - `term/probe.rs`: the query bytes, the reply parser (`Replies`) and
    the stdin read loop.
  - `term/backend.rs`: `FrextBackend`.
- **The capability types** (`term::caps`), all
  `#[derive(Debug, Clone, Copy, PartialEq, Eq)]` except `TerminalCaps`
  (`Clone`, not `Copy`, because of the `String`):

  ```rust
  pub struct CellPx { pub width: u16, pub height: u16 }
  pub enum ImageSupport { KittyPlaceholders { cell_px: CellPx }, #[default] None }
  pub enum UnderlineStyle { Curly, #[default] Straight }
  pub enum KeyboardProtocol { Kitty { flags: u16 }, #[default] Legacy }
  pub enum Multiplexer { #[default] None, Tmux }
  #[derive(Debug, Clone, Default, PartialEq, Eq)]
  pub struct TerminalCaps {
      pub images: ImageSupport,
      pub underline: UnderlineStyle,
      pub keyboard: KeyboardProtocol,
      pub multiplexer: Multiplexer,
      pub version: Option<String>,
  }
  pub enum IconChoice { Kitty { cell_px: CellPx }, Nerd, Ascii, None }
  ```

  The enums derive `Default` where marked. `TerminalCaps::default()`
  describes a terminal that answered nothing.
- **Resolution rules** (`caps::resolve(replies, env, settings)`), with
  `settings: frext_core::config::TermConfig` already merged from config
  and CLI:
  - `multiplexer = detect_multiplexer(env)`: `Tmux` when `TMUX` is set,
    or when `TERM` starts with `tmux`; otherwise `None`.
  - `keyboard`: `Kitty { flags }` when the `CSI ? flags u` reply came
    back; otherwise `Legacy`.
  - `version`: the XTVERSION text, the part after `>|`.
  - `underline`:
    - `UndercurlMode::On` gives `Curly`, and `Off` gives `Straight`.
    - Under `Auto`:
      - `Curly` when `Smulx` was reported as supported.
      - Otherwise, when **not** under tmux, `Curly` when any of these
        names a known terminal:
        - XTVERSION contains (ASCII case-insensitive) `kitty`,
          `ghostty`, `foot`, `wezterm`, `freminal`, `contour`, `iTerm2`
          or `alacritty`.
        - `TERM_PROGRAM` equals (case-insensitive) `ghostty`,
          `wezterm`, `freminal`, `iTerm.app` or `vscode`.
        - `TERM` equals (case-insensitive) `xterm-kitty`,
          `xterm-ghostty`, `wezterm`, `alacritty` or `contour`, or
          starts with `foot`.
      - Otherwise `Straight`.
  - `images`, by `settings.icons`:
    - `Auto`: `KittyPlaceholders { cell_px }` when the kitty reply was
      `OK`, a cell size came back, and neither `WEZTERM_EXECUTABLE` nor
      `KONSOLE_VERSION` is set; otherwise `None`.
    - `Kitty` (forced): `KittyPlaceholders { cell_px }` whenever a cell
      size came back, ignoring the `OK` reply and the blocklist. With
      no cell size the result is `None`, and `resolve` logs
      `log::warn!` that `icons = "kitty"` needs the terminal's cell
      size and falls back to Nerd Font icons.
    - `Nerd`, `Ascii`, `None`: `None`. frext uses images only for
      icons.
  - Environment variables set to an empty string count as unset.
- **`IconChoice::select(requested: IconsMode, caps: &TerminalCaps)`**:
  - `Auto` and `Kitty` give `Kitty { cell_px }` when `caps.images` is
    `KittyPlaceholders { cell_px }`, otherwise `Nerd`.
  - `Nerd`, `Ascii` and `None` map to themselves.

  This is the input plan 08's provider selection consumes. In this
  plan it is only computed and logged.
- **Keyboard mode.** `terminal::KeyboardMode` stays the type the app
  and keymap use. `KeyboardMode::from_protocol(KeyboardProtocol)` maps
  `Kitty { .. }` to `Enhanced` and `Legacy` to `Legacy`.
  `crossterm::terminal::supports_keyboard_enhancement` is no longer
  called, which completes the hand-over plan 02 promised.
- **Probe placement.** `TerminalGuard::enter` runs the probe right
  after `enable_raw_mode` and before `write_setup`, long before
  `EventLoop::spawn_input`. A probe I/O error is logged at `warn` and
  treated as "no replies" (`Replies::default()`). It never aborts
  startup. (06.9 moves it after entering the alternate screen and
  reads from the tty crossterm reads.)
- **Bytes after DA1.** Whatever the read loop reads after the DA1 reply
  (a key typed during startup) is discarded. Only replies before DA1
  are parsed. Non-reply bytes before DA1 are skipped too, so keys typed
  during the probe's few milliseconds are lost; accepted (06.9 review,
  finding 8). (06.9 keeps interpreting kitty APC replies after DA1.)
- **Config.** `Config::load()` reads `Config::default_path()`
  (`ProjectDirs::from("io.github", "fredsystems", "frext")`'s
  `config_dir()` joined with `config.toml`, the same `ProjectDirs` the
  `Store` uses). A missing file gives the default silently. A file
  that cannot be read or parsed is logged at `warn` and gives the
  default. Unknown keys and tables are ignored. `main` loads the config
  after logging is initialised.
- **CLI.**
  - `--icons <mode>` / `--icons=<mode>` and `--undercurl <mode>` /
    `--undercurl=<mode>` set the mode. Repeated flags: the last one
    wins.
  - `--` ends option parsing.
  - Any other argument starting with `--` (before `--`) is an error.
  - Everything else keeps today's path rules.
  - A CLI error prints `frext-term: <error>` to stderr and exits with
    failure before anything else happens.
  - There is no `--help` (plan 09's docs work can add it).
- **The backend's undercurl rule** (exactly):
  - For each cell, its underline is `Off` without
    `Modifier::UNDERLINED`.
  - With `UNDERLINED` and `underline_color != Color::Reset`, it is
    `Curly` when the backend's `UnderlineStyle` is `Curly`.
  - In every other `UNDERLINED` case it is `Straight`.
  - The copied `ModifierDiff` also gets the previous and next underline
    state:
    - It emits `NoUnderline` (`CSI 24 m`) where stock removes
      `UNDERLINED` (next state `Off`).
    - It emits `Underlined` (`CSI 4 m`) or `Undercurled`
      (`CSI 4:3 m`) where stock adds `UNDERLINED`, whenever the next
      state is not `Off` and differs from the previous one.
    - The diff runs when the modifier **or** the underline state
      changes.
  - Everything else is byte-for-byte the stock loop, so a frame with no
    underline colour produces exactly the stock bytes.
- **Not in this plan:**
  - Drawing squiggles (plan 07).
  - Icon providers, rasterising and uploads (plan 08).
  - README, man page and `--help` (plan 09).
  - Re-probing on resize.
  - Windows support for the probe: the crate is Unix-only today.

## Execution model

All subtasks run **strictly in sequence** on
`feat/frext-term-capabilities`. Each subtask is one commit and leaves
the suite green (`commit-discipline`). The orchestrator reviews the full
diff of each subtask before committing it. Implementers do not commit,
merge or push.

Each subtask's verification includes the standard ritual, run inside
`nix develop`:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
cargo machete
```

All code follows the workspace rules:

- no `unwrap` / `expect` outside tests
- no `unsafe`
- no indexing that can panic (use `get`, or ranges proven in bounds)
- every public item tested
- doc comments on every public item
- no new dependency beyond the two named above, each added only in its
  own subtask
- no bare `bool` parameters or fields for states (`state-representation`)

Benchmarks: no subtask touches a benchmarked path (`buffer`, `wrap`,
the GUI's `highlight`). `FrextBackend::draw` replaces the stock draw
loop with the same work plus one comparison per cell. No ratatui draw
benchmark exists, and the byte-equality test against the stock backend
(06.6) pins the behaviour.

## Subtasks

### 06.1 -- Core: the shared config file

Scope: `Cargo.toml` (workspace `toml` dependency), `Cargo.lock`,
`frext-core/Cargo.toml`, `frext-core/src/config.rs` (new),
`frext-core/src/lib.rs`, `frext-core/src/error.rs`.

What:

- Add `toml` as decided under "New dependencies", alphabetically, at
  workspace level and in `frext-core`.
- `lib.rs`: `pub mod config;`.
- `error.rs`: `ConfigError` (`thiserror`), variants:
  - `Read { path: PathBuf, source: std::io::Error }`: "could not read
    {path}: {source}"
  - `Parse(#[from] toml::de::Error)`: "invalid config: {0}"
  - `InvalidValue { field: &'static str, value: String }`: "invalid
    value {value:?} for {field}"
- `config.rs`:
  - `#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
    #[serde(rename_all = "lowercase")] pub enum IconsMode { #[default]
    Auto, Kitty, Nerd, Ascii, None }`
  - `#[derive(..same..)] pub enum UndercurlMode { #[default] Auto, On,
    Off }`
  - `impl FromStr` for both, accepting exactly the lowercase spellings
    serde accepts (`"auto"`, `"kitty"`, `"nerd"`, `"ascii"`, `"none"`;
    `"auto"`, `"on"`, `"off"`). Anything else is
    `ConfigError::InvalidValue` with `field` `"icons"` or
    `"undercurl"`.
  - `#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
    #[serde(default)] pub struct TermConfig { pub icons: IconsMode, pub
    undercurl: UndercurlMode }`
  - `#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
    #[serde(default)] pub struct Config { pub term: TermConfig }`
  - `impl Config`:
    - `pub fn parse(text: &str) -> Result<Self, ConfigError>`
    - `pub fn load_from(path: &Path) -> Result<Self, ConfigError>`: a
      missing file (`ErrorKind::NotFound`) is `Ok(Self::default())`.
    - `#[must_use] pub fn default_path() -> Option<PathBuf>`: as
      decided under "Config".
    - `#[must_use] pub fn load() -> Self`: `default_path` then
      `load_from`. On `Err` it logs `log::warn!` (with the path and
      error) and returns the default.
  - No `deny_unknown_fields`.
- The module doc names the file, says which front end reads which
  table, and says that unknown keys are ignored because the Home
  Manager module writes free-form settings.

Deliverable: tests in `config.rs`:

- empty text is the default
- a full `[term]` table with every value of both fields
- a file with only top-level `theme = "catppuccin-mocha"` and an
  unknown `[gui]` table parses to the default
- a partial `[term]` (only `undercurl`) keeps the other field's default
- an invalid value (`icons = "sixel"`) is `Err(ConfigError::Parse(_))`
- `FromStr` round-trips each spelling, and rejects `"Auto"`, `""` and
  `"sixel"` with `InvalidValue` naming the right field
- `load_from` a missing path is the default, and a temp file with
  content parses
- `load_from` a directory path is `Err(ConfigError::Read { .. })`
- `default_path()` (when `Some`) ends with `frext/config.toml`

Verification: the standard ritual.

Prohibitions: do NOT touch `frext` or `frext-term`; do NOT add any
other dependency; do NOT add a `[gui]` table; do NOT proceed to 06.2.

Stop: report files changed and verification; await review.

### 06.2 -- Term: the capability types and tmux passthrough

Scope: `frext-term/src/lib.rs`, `frext-term/src/term/mod.rs` (new),
`frext-term/src/term/caps.rs` (new), `frext-term/src/term/tmux.rs`
(new).

What:

- `lib.rs`: `pub mod term;`.
- `term/mod.rs`: the module doc (terminal capability detection, the
  probe, and the undercurl-capable backend; lifecycle stays in
  `crate::terminal`), with `pub mod caps;` and `pub mod tmux;`.
- `term/caps.rs`: exactly the types listed under "The capability
  types", minus `IconChoice` (06.4). Each type and variant gets a doc
  comment. `CellPx` documents width and height in pixels of one cell.
  No functions yet.
- `term/tmux.rs`:
  - `#[must_use] pub fn passthrough(seq: &[u8]) -> Vec<u8>`: `ESC P
    tmux ;`, then `seq` with every `ESC` (0x1b) doubled, then `ESC \`.
  - `#[must_use] pub fn wrap_for(multiplexer: Multiplexer, seq: &[u8])
    -> Vec<u8>`: `passthrough(seq)` under `Tmux`, else `seq.to_vec()`.
  - The module doc says passthrough needs the user's `set -g
    allow-passthrough on`.

Deliverable: tests:

- `TerminalCaps::default()` has `images: None`, `underline: Straight`,
  `keyboard: Legacy`, `multiplexer: None`, `version: None`
- `passthrough(b"\x1b_Gq\x1b\\")` is exactly
  `b"\x1bPtmux;\x1b\x1b_Gq\x1b\x1b\\\x1b\\"`
- `passthrough(b"")` is `b"\x1bPtmux;\x1b\\"`
- `wrap_for(Multiplexer::None, ..)` returns the input unchanged

Verification: the standard ritual.

Prohibitions: do NOT add `resolve`, the probe or the backend; do NOT
touch `terminal.rs`; do NOT proceed to 06.3.

Stop: report files changed and verification; await review.

### 06.3 -- Term: the probe query and the reply parser

Scope: `frext-term/src/term/mod.rs`, `frext-term/src/term/probe.rs`
(new).

What:

- `mod.rs`: `pub mod probe;`.
- `probe.rs` (pure parts only):
  - `pub const KITTY_QUERY: &[u8] =
    b"\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\";`
  - `#[must_use] pub fn query(multiplexer: Multiplexer) -> Vec<u8>`:
    `tmux::wrap_for(multiplexer, KITTY_QUERY)`, then `b"\x1b[16t"`,
    `b"\x1b[>q"`, `b"\x1bP+q536d756c78\x1b\\"`, `b"\x1b[?u"`,
    `b"\x1b[c"`, in that order.
  - `#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)] pub enum
    KittyReply { Ok, Error, #[default] Missing }`
  - `#[derive(..same..)] pub enum TcapReply { Supported, Unsupported,
    #[default] Missing }`
  - `#[derive(..same..)] pub enum ReplyEnd { Da1, #[default]
    Incomplete }`
  - `#[derive(Debug, Clone, Default, PartialEq, Eq)] pub struct Replies
    { pub kitty_graphics: KittyReply, pub cell_px: Option<CellPx>, pub
    xtversion: Option<String>, pub smulx: TcapReply, pub
    keyboard_flags: Option<u16>, pub end: ReplyEnd }`
  - `#[must_use] pub fn parse(bytes: &[u8]) -> Replies`, a small
    tokenizer over the escape sequences:
    - CSI is `ESC [`, then parameter bytes `0x30..=0x3F`, intermediates
      `0x20..=0x2F`, and a final byte `0x40..=0x7E`.
    - DCS (`ESC P`), APC (`ESC _`) and OSC (`ESC ]`) run to `ESC \` or
      BEL (0x07).
    - Any other byte is skipped.
    - A sequence cut off at the end of the input is ignored.
    - Parsing stops after the DA1 reply (`CSI ? … c`), which sets
      `end = Da1`. Later bytes are ignored.
    - Interpretation:
      - APC body `G<keys>;<message>` whose keys include `i=31`:
        `message == "OK"` gives `Ok`, anything else `Error`. Other ids
        are ignored.
      - `CSI 6 ; h ; w t` gives `cell_px = CellPx { width: w, height:
        h }`. A zero or unparsable value is ignored.
      - DCS `>|<text>` gives `xtversion = Some(text)` (lossy UTF-8).
      - DCS `1+r<names>[=…]` whose `;`-separated names include
        `536d756c78` (ASCII case-insensitive) gives `Supported`. `0+r`
        with that name gives `Unsupported`.
      - `CSI ? <n> u` gives `keyboard_flags = Some(n)`.
- The fixtures are `pub(crate) const`s in a `#[cfg(test)] pub(crate)
  mod fixtures` inside `probe.rs` (06.4 and 06.5 reuse them), one per
  terminal, built from each
  terminal's documented reply format. The doc comment says so: they are
  synthesised, not recorded. The manual smoke checklist records real
  ones.

  | Fixture     | Bytes                                                                                                                                                 |
  | ----------- | ----------------------------------------------------------------------------------------------------------------------------------------------------- |
  | `KITTY`     | `\x1b_Gi=31;OK\x1b\\` `\x1b[6;20;10t` `\x1bP>\|kitty(0.39.1)\x1b\\` `\x1bP1+r536d756c78=1b5b343a25703125646d\x1b\\` `\x1b[?0u` `\x1b[?62;c`           |
  | `GHOSTTY`   | `\x1b_Gi=31;OK\x1b\\` `\x1b[6;17;8t` `\x1bP>\|ghostty 1.1.3\x1b\\` `\x1bP1+r536d756c78=1b5b343a25703125646d\x1b\\` `\x1b[?0u` `\x1b[?62;22c`          |
  | `FOOT`      | `\x1b[6;16;8t` `\x1bP>\|foot(1.20.2)\x1b\\` `\x1bP1+r536d756c78=1b5b343a25703125646d\x1b\\` `\x1b[?0u` `\x1b[?62;4;22c`                              |
  | `WEZTERM`   | `\x1b_Gi=31;OK\x1b\\` `\x1b[6;18;9t` `\x1bP>\|WezTerm 20240203-110809-5046fc22\x1b\\` `\x1bP0+r536d756c78\x1b\\` `\x1b[?65;4;6;18;22c`                 |
  | `FREMINAL`  | `\x1b_Gi=31;OK\x1b\\` `\x1b[6;20;10t` `\x1bP>\|XTerm(Freminal 0.13.0-beta.3)\x1b\\` `\x1bP1+r536d756c78=1B5B343A25703125646D\x1b\\` `\x1b[?0u` `\x1b[?65;1;2;4;6;17;18;22c` |
  | `ALACRITTY` | `\x1b[?0u` `\x1b[?6c`                                                                                                                                 |
  | `TMUX`      | `\x1bP>\|tmux 3.5a\x1b\\` `\x1b[?1;2c`                                                                                                                |

  (The spaces between sequences are for reading only. `\|` is a
  literal `|`.) `FREMINAL` was replaced by a real recording from
  freminal 0.13.0-beta.3 after 06.8; the others are still synthesised.

Deliverable: tests in `probe.rs`:

- `query(Multiplexer::None)` is exactly the concatenation above, and
  `query(Multiplexer::Tmux)` starts with
  `tmux::passthrough(KITTY_QUERY)` and has the same tail
- each fixture parses to its expected `Replies`, with every field
  asserted (e.g. `FOOT`: `Missing`, `Some(8x16)`, `Some("foot(1.20.2)")`,
  `Supported`, `Some(0)`, `Da1`)
- empty input is `Replies::default()`
- a BEL-terminated XTVERSION reply parses
- a kitty error reply (`\x1b_Gi=31;ENOTSUPPORTED:no\x1b\\`) gives
  `Error`, and an `OK` for `i=99` gives `Missing`
- `CSI 6;0;0t` leaves `cell_px` `None`
- an uppercase `536D756C78` name gives `Supported`
- noise bytes (`b"xy"`) before a reply are skipped
- bytes after DA1 (`b"x\x1b[?5u"`) are ignored, so `keyboard_flags`
  keeps the value from before DA1
- input cut off mid-sequence (`KITTY` without its last 3 bytes) gives
  `end: Incomplete` and still has the earlier fields

Verification: the standard ritual.

Prohibitions: do NOT add I/O (06.5); do NOT add `resolve` (06.4); do
NOT add a dependency; do NOT proceed to 06.4.

Stop: report files changed and verification; await review.

### 06.4 -- Term: resolving `TerminalCaps`

Scope: `frext-term/src/term/caps.rs`.

What:

- `#[derive(Debug, Clone, Default, PartialEq, Eq)] pub struct TermEnv
  { pub term: Option<String>, pub term_program: Option<String>, pub
  tmux: Option<String>, pub wezterm_executable: Option<String>, pub
  konsole_version: Option<String> }`
  - `pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) ->
    Self`: reads `TERM`, `TERM_PROGRAM`, `TMUX`, `WEZTERM_EXECUTABLE`
    and `KONSOLE_VERSION`. Empty values become `None`.
  - `#[must_use] pub fn from_process() -> Self`: `from_lookup(|k|
    std::env::var(k).ok())`.
- `#[must_use] pub fn detect_multiplexer(env: &TermEnv) -> Multiplexer`
- `#[must_use] pub fn resolve(replies: &Replies, env: &TermEnv,
  settings: TermConfig) -> TerminalCaps`: exactly the rules under
  "Resolution rules", including the `warn` log for forced kitty with no
  cell size. The known-terminal lists are private `const` slices.
- `IconChoice` (with its doc) and `#[must_use] pub fn select(requested:
  IconsMode, caps: &TerminalCaps) -> IconChoice` as an associated
  function, per "`IconChoice::select`".

Deliverable: tests, using the fixtures from
`crate::term::probe::fixtures` parsed with `probe::parse`:

- per fixture, with `TermConfig::default()` and this environment:

  | Fixture     | Environment                                                     | images        | underline  | keyboard        | multiplexer |
  | ----------- | --------------------------------------------------------------- | ------------- | ---------- | --------------- | ----------- |
  | `KITTY`     | `TERM=xterm-kitty`                                              | Kitty 10x20   | `Curly`    | `Kitty { 0 }`   | `None`      |
  | `GHOSTTY`   | `TERM=xterm-ghostty`, `TERM_PROGRAM=ghostty`                    | Kitty 8x17    | `Curly`    | `Kitty { 0 }`   | `None`      |
  | `FOOT`      | `TERM=foot`                                                     | `None`        | `Curly`    | `Kitty { 0 }`   | `None`      |
  | `WEZTERM`   | `TERM_PROGRAM=WezTerm`, `WEZTERM_EXECUTABLE=/usr/bin/wezterm`   | `None`        | `Curly`    | `Legacy`        | `None`      |
  | `FREMINAL`  | `TERM=xterm-256color`, `TERM_PROGRAM=freminal`                  | Kitty 10x20   | `Curly`    | `Kitty { 0 }`   | `None`      |
  | `ALACRITTY` | `TERM=alacritty`                                                | `None`        | `Curly`    | `Kitty { 0 }`   | `None`      |
  | `TMUX`      | `TMUX=/tmp/tmux-1000/default,1,0`, `TERM=tmux-256color`, `TERM_PROGRAM=tmux` | `None` | `Straight` | `Legacy` | `Tmux` |

  plus `version` equal to the fixture's XTVERSION text.
- `FREMINAL` replies with no environment at all still give Kitty
  images, `Curly` (from `Smulx`) and `Kitty { 0 }`: detection is by
  query alone
- the `KITTY` replies with `KONSOLE_VERSION=230800` give images `None`
- the `TMUX` replies with `smulx: Supported` give `Curly`
- `GHOSTTY` replies with `smulx: Missing` under the `TMUX` environment
  give `Straight` (the known list does not apply under tmux)
- `Replies::default()` with `TERM=xterm-256color` gives exactly
  `TerminalCaps::default()`
- overrides: `undercurl = On` on default replies gives `Curly`, and
  `Off` on `KITTY` gives `Straight`
- `icons = Kitty` on `WEZTERM` (blocklisted) gives `KittyPlaceholders
  { 9x18 }`, and on `FOOT` with `cell_px: None` gives images `None`
- `icons = Nerd`, `Ascii` and `None` on `KITTY` each give images `None`
- `detect_multiplexer`: `TMUX` set, `TERM=tmux-256color` with no
  `TMUX`, and `TERM=screen-256color` with no `TMUX` (`None`)
- `from_lookup` maps an empty `TERM` to `None` and reads each variable
- `IconChoice::select`: `Auto` and `Kitty` with Kitty images give
  `Kitty { cell_px }`, and without images give `Nerd`; `Nerd`, `Ascii`,
  `None` map to themselves

Verification: the standard ritual.

Prohibitions: do NOT add I/O or read the real environment in tests;
do NOT touch `terminal.rs`; do NOT proceed to 06.5.

Stop: report files changed and verification; await review.

### 06.5 -- Term: the probe's read loop

Scope: `Cargo.toml` (workspace `rustix` dependency), `Cargo.lock`,
`frext-term/Cargo.toml`, `frext-term/src/term/probe.rs`.

What:

- Add `rustix` as decided under "New dependencies".
- `pub const PROBE_TIMEOUT: Duration = Duration::from_secs(1);`
- `pub fn read_replies(input: impl AsFd, timeout: Duration) ->
  io::Result<Vec<u8>>`:
  - Loop until a deadline of `Instant::now() + timeout`.
  - Each turn: `rustix::event::poll` on one `PollFd` (`PollFlags::IN`)
    with the remaining time as a `Timespec`. A poll that returns 0
    ends the loop.
  - Then `rustix::io::read` into a 1024-byte buffer, appending what was
    read:
    - `0` bytes (EOF) ends the loop.
    - `Errno::INTR` from either call retries.
    - Any other error is returned (converted with `io::Error::from`).
  - The loop ends early once `parse(&bytes).end == ReplyEnd::Da1`.
- `pub fn probe(out: &mut impl Write, input: impl AsFd, multiplexer:
  Multiplexer, timeout: Duration) -> io::Result<Replies>`:
  - Writes `query(multiplexer)` and flushes.
  - Calls `read_replies`.
  - Logs the raw bytes at `debug` as `bytes.escape_ascii()` (so real
    replies can be recorded from the log).
  - Returns `parse(&bytes)`.

Deliverable: tests using `std::io::pipe()`:

- the `KITTY` fixture written into the pipe, with the writer kept open
  and a 60 s timeout, is read back whole, and the call returns in under
  30 s (it stopped at DA1, not the timeout)
- an open, empty pipe with a 50 ms timeout returns `Ok(vec![])`
- `b"\x1b[?1u"` written and then the writer dropped returns those
  bytes (EOF ends the loop) without waiting for a 60 s timeout
- `probe` into a `Vec<u8>` writes exactly `query(Multiplexer::None)`
  and returns the `FREMINAL` fixture's `Replies`

Verification: the standard ritual. The timing bounds above are
deliberately loose (`flaky-tests-are-bugs`).

Prohibitions: do NOT use `unsafe`; do NOT call `rustix::stdio::stdin`
here (06.8 does); do NOT touch `terminal.rs`; do NOT proceed to 06.6.

Stop: report files changed and verification; await review.

### 06.6 -- Term: `FrextBackend`

Scope: `frext-term/src/term/mod.rs`, `frext-term/src/term/backend.rs`
(new).

What:

- `mod.rs`: `pub mod backend;`.
- `backend.rs`:
  - `#[derive(Debug, Clone, PartialEq, Eq)] pub struct FrextBackend<W:
    Write> { writer: W, underline: UnderlineStyle }`
  - `pub const fn new(writer: W, underline: UnderlineStyle) -> Self`,
    `pub const fn underline(&self) -> UnderlineStyle`,
    `pub const fn writer(&self) -> &W`,
    `pub const fn writer_mut(&mut self) -> &mut W` (const where the
    compiler allows, otherwise plain)
  - `impl<W: Write> Write for FrextBackend<W>`, passing through.
  - `impl<W: Write> Backend for FrextBackend<W>` with `type Error =
    io::Error`:
    - Every method is copied from ratatui-crossterm 0.1.2's
      `CrosstermBackend`: `draw`, `hide_cursor`, `show_cursor`,
      `get_cursor_position`, `set_cursor_position`, `clear`,
      `clear_region`, `append_lines`, `size`, `window_size`, `flush`.
    - The copy is not under `#[cfg(feature = "underline-color")]`: the
      workspace always enables it.
    - `draw` is changed only by "The backend's undercurl rule".
    - The module doc credits ratatui-crossterm (MIT) as the source of
      the copied loop.
  - A private `enum UnderlineDraw { Off, Straight, Curly }` and a
    private copy of `ModifierDiff` carrying `from_underline` /
    `to_underline: UnderlineDraw`. `UNDERLINED` is handled only
    through those, at the positions stock uses.

Deliverable: tests that draw into a `Vec<u8>`. Every test first calls
`crossterm::style::Colored::set_ansi_color_disabled(false)`. Cells come
from `ratatui::buffer::Cell` with symbols and styles set.

- Parity:
  - A row of cells covering bold, dim, italic, reversed, coloured
    fg/bg, bold-to-dim and a plain `UNDERLINED` cell without underline
    colour.
  - Drawn with `FrextBackend` under both `UnderlineStyle`s and with
    `ratatui::backend::CrosstermBackend::new(Vec::new())`.
  - All three byte vectors are equal.
- A single cell `"a"` at (0, 0) with `UNDERLINED` and underline colour
  `Rgb(243, 139, 168)` under `Curly` is exactly
  `"\x1b[1;1H\x1b[4:3m\x1b[58;2;243;139;168ma\x1b[39m\x1b[49m\x1b[59m\x1b[0m"`.
- The same cell under `Straight` is the same with `\x1b[4m` in place of
  `\x1b[4:3m`.
- `UNDERLINED` with no underline colour under `Curly` emits `\x1b[4m`
  and never `4:3`.
- An underline colour without `UNDERLINED` emits the colour and no
  underline attribute.
- Curly cell then plain-underlined cell: `\x1b[4m` appears between the
  two symbols, and `\x1b[24m` does not.
- Curly cell then a cell with no underline: `\x1b[24m` then `\x1b[59m`
  appear between them.
- Plain-underlined then curly (same modifier): `\x1b[4:3m` appears
  between them.
- Non-adjacent cells emit a `MoveTo`, as stock does.
- `underline()` returns the constructor's value, and writing through
  `Write` reaches `writer()`.

Verification: the standard ritual.

Prohibitions: do NOT change the rule; do NOT add a dependency
(`ratatui-crossterm` stays indirect); do NOT switch the app to the new
backend (06.8); do NOT proceed to 06.7.

Stop: report files changed and verification; await review.

### 06.7 -- CLI: `--icons` and `--undercurl`

Scope: `frext-term/src/cli.rs`, `frext-term/src/main.rs` (only the
`cli::parse` call site).

What:

- `Args` gains `pub icons: Option<IconsMode>` and `pub undercurl:
  Option<UndercurlMode>` (from `frext_core::config`). It still derives
  `Default`.
- `#[derive(Debug, Clone, PartialEq, Eq)] pub enum CliError {
  MissingValue { flag: &'static str }, InvalidValue { flag: &'static
  str, value: String }, UnknownOption(String) }`. It gets a manual
  `Display` and `impl std::error::Error`, because frext-term has no
  `thiserror`. The messages:
  - `"--icons needs a value (auto, kitty, nerd, ascii, none)"`
  - `"invalid value \"x\" for --icons (expected auto, kitty, nerd,
    ascii, none)"`
  - the same two for `--undercurl` with `auto, on, off`
  - `"unknown option \"--foo\" (put -- before a file name that starts
    with --)"`
- `pub fn parse(args: impl IntoIterator<Item = OsString>) ->
  Result<Args, CliError>` implements the rules under "CLI". Values are
  parsed with the core `FromStr` impls. A non-UTF-8 value is
  `InvalidValue` with its lossy text.
- `impl Args { #[must_use] pub fn apply_to(&self, config: TermConfig)
  -> TermConfig }`: each `Some` flag replaces the config's field.
- `main.rs`: on `Err(err)`, `eprintln!("frext-term: {err}")` and return
  `ExitCode::FAILURE` before the store is created. Nothing else in
  `main` changes.

Deliverable: the existing tests updated for `Result`, plus:

- `--icons nerd`, `--icons=ascii`, `--undercurl off`, `--undercurl=on`
- the last of two `--icons` wins
- flags mixed between paths keep the paths' order and the first-dir
  rule
- `--icons` as the last argument is `MissingValue`, and `--icons=` is
  `InvalidValue` with `""`
- `--icons sixel` and `--undercurl yes` are `InvalidValue`
- `--frobnicate` is `UnknownOption`
- `-- --icons` gives a file `--icons` and no flag
- a single `-` is still a file
- each `CliError` message, exactly
- `apply_to`: no flags keeps the config, and each flag overrides only
  its field

Verification: the standard ritual.

Prohibitions: do NOT load the config or touch the terminal setup in
`main` (06.8); do NOT add `--help`; do NOT add a dependency; do NOT
proceed to 06.8.

Stop: report files changed and verification; await review.

### 06.8 -- Wire the probe, the caps and the backend into startup

Scope: `frext-term/src/terminal.rs`, `frext-term/src/main.rs`.

What:

- `terminal.rs`:
  - `impl KeyboardMode { #[must_use] pub const fn
    from_protocol(protocol: KeyboardProtocol) -> Self }`, per
    "Keyboard mode".
  - `TerminalGuard` gains `caps: TerminalCaps` and `#[must_use] pub
    const fn caps(&self) -> &TerminalCaps`.
  - `TerminalGuard::enter(settings: TermConfig) -> io::Result<(Self,
    Terminal<FrextBackend<Stdout>>)>`:
    - after `enable_raw_mode`:
      - `env = TermEnv::from_process()`
      - `multiplexer = detect_multiplexer(&env)`
      - `replies = probe(&mut io::stdout(), rustix::stdio::stdin(),
        multiplexer, PROBE_TIMEOUT)`, with an `Err` logged at `warn`
        and replaced by `Replies::default()`
      - `caps = resolve(&replies, &env, settings)`, logged at `info`
      - `mode = KeyboardMode::from_protocol(caps.keyboard)`
    - The rest of the setup is unchanged: `write_setup(mode)`, the
      undo path, the `ACTIVE` / `ENHANCED` flags and the panic hook.
    - The terminal is `Terminal::new(FrextBackend::new(io::stdout(),
      caps.underline))`.
    - Docs are updated: no more crossterm keyboard check, and the probe
      runs before any event reader exists.
  - Remove the `supports_keyboard_enhancement` import and call.
  - Keep the `CrosstermBackend` import only if still used (it should
    not be).
- `main.rs`:
  - after logging is initialised, `let settings =
    args.apply_to(Config::load().term);`
  - `TerminalGuard::enter(settings)`
  - after entering, `log::info!("icons: {:?}",
    IconChoice::select(settings.icons, guard.caps()))` (plan 08 wires
    the provider)
  - The spawn comment says the probe, not the keyboard query, must not
    be raced by the input reader.

Deliverable: tests in `terminal.rs`:

- `from_protocol` for `Kitty { flags: 0 }`, `Kitty { flags: 31 }` and
  `Legacy`
- the direct-construction guard test also asserts `caps()` returns
  the value it was built with
- the existing byte tests still pass unchanged

Verification: the standard ritual, plus a manual run of `cargo run -p
frext-term` in a real terminal: it starts, `frext-term.log` shows the
caps line, and it quits cleanly. Report the logged caps line.

Prohibitions: do NOT change `app.rs`, the keymap or `run.rs`; do NOT
draw any underline colour (plan 07); do NOT proceed further.

Stop: report files changed, verification, and the logged caps line;
await review.

### 06.9 -- Review fixes (adversarial branch review)

Surfaced by a CODE-REVIEW pass over `main...60f510b`. Findings 9 and
10 (the case-insensitive `TERM` match and the trimmed `toml` features)
were plan-text mismatches, fixed in the plan above. Finding 8 (keys
typed during the probe are lost) is accepted and recorded under "Bytes
after DA1". Finding 4 was deferred to 06.10, which the PR #44
review then promoted to a pre-merge fix.

Scope: `frext-term/src/term/probe.rs`, `frext-term/src/terminal.rs`,
`frext-term/src/term/caps.rs` (one doc comment).

The fixes:

- **A -- read the tty crossterm reads** (bug, finding 1). crossterm
  reads stdin only when it is a terminal and `/dev/tty` otherwise. The
  probe always read stdin, so `xargs frext-term` (stdin `/dev/null`) or
  a pipe gave no replies, and the real replies later reached crossterm
  as Alt+P / Alt+_ and typed text in the buffer. Now `terminal.rs` has
  a private `fn run_probe(multiplexer: Multiplexer) ->
  io::Result<Replies>`:
  - When `io::stdin().is_terminal()` (`std::io::IsTerminal`), it probes
    with `rustix::stdio::stdin()`.
  - Otherwise it opens `/dev/tty` with `std::fs::File::open` and probes
    with that file (a failure to open is returned as the error, which
    `enter` already logs and treats as no replies).
  - Queries are still written to `io::stdout()`, which crossterm and
    ratatui write to.
- **B -- the timeout and size are enforced** (bug, finding 2; risk,
  finding 7). In `read_replies`:
  - At the top of each turn, when a deadline exists and
    `Instant::now() >= deadline`, the loop ends.
  - `const MAX_REPLY_BYTES: usize = 64 * 1024;`: once `bytes.len()`
    reaches it, the loop ends.
  - `Errno::AGAIN` from `read` is retried like `Errno::INTR` (the
    deadline check bounds the retries).
- **C -- tmux: wait for late kitty replies** (bug, finding 3). tmux
  answers DA1 itself, at once, while the wrapped kitty query is still
  on its way to the outer terminal. Now:
  - `pub enum Settle { AtDa1, QuietAfterDa1(Duration) }` (derive
    `Debug, Clone, Copy, PartialEq, Eq`) and `pub const TMUX_SETTLE:
    Duration = Duration::from_millis(100);`.
  - `read_replies(input, timeout, settle: Settle)`: with `AtDa1`, the
    loop ends at DA1 as before. With `QuietAfterDa1(quiet)`, once DA1
    has been seen each poll waits at most `quiet` (and never past the
    deadline), and a poll that returns 0 ends the loop.
  - `probe` passes `QuietAfterDa1(TMUX_SETTLE)` under
    `Multiplexer::Tmux` and `AtDa1` otherwise.
  - `parse` keeps tokenizing after DA1 but interprets only APC tokens
    (the kitty reply), which no key press produces. Everything else
    after DA1 stays ignored.
- **D -- probe inside the alternate screen, then clear** (risk,
  finding 5). A terminal without APC/DCS parsing (the Linux console)
  prints the queries' text. Now:
  - `pub fn write_enter(out: &mut impl Write) -> io::Result<()>`
    queues `EnterAlternateScreen` and flushes. `write_setup` no longer
    enters the alternate screen (it writes mouse capture, bracketed
    paste and the keyboard push). `write_restore` is unchanged and
    still leaves the alternate screen last.
  - `enter`: `enable_raw_mode`, then `write_enter` (on error: the same
    undo as a `write_setup` failure, with `KeyboardMode::Legacy`, then
    return the error), then the probe and caps, then `write_setup`,
    unchanged from there.
  - `write_setup` starts with `Clear(ClearType::All)` (`CSI 2 J`). It
    runs right after the probe, inside the alternate screen, so it
    wipes anything a terminal printed. (Not `Terminal::clear`, which
    first reads the cursor position, a round trip that fails startup
    on a terminal that does not answer it; found in the orchestrator's
    pseudo-terminal run.)
- **E -- an ESC cancels a string sequence** (risk, finding 6). In the
  tokenizer, an `ESC` inside a DCS/APC/OSC body that is not followed by
  `\` ends the string without a token, and tokenizing resumes at that
  `ESC`. An `ESC` as the last byte is still a cut-off sequence.
- **F -- docs.** `TerminalCaps::version` says it holds the XTVERSION
  text after `>|`. The probe module doc says non-reply bytes before
  DA1 are skipped and keys typed during the probe are lost.

Tests:

- B: a writer thread that keeps writing non-reply bytes (until the
  write fails) makes `read_replies` with a 60 s timeout return in
  under 30 s with between `MAX_REPLY_BYTES` and `MAX_REPLY_BYTES +
  1024` bytes; a writer thread writing one byte every 10 ms makes it
  return within 30 s with a 200 ms timeout
- C: `parse` of the `TMUX` fixture followed by
  `\x1b_Gi=31;OK\x1b\\` gives `kitty_graphics: Ok` and `end: Da1`;
  `read_replies` with `QuietAfterDa1(500 ms)` returns the `TMUX`
  fixture plus a kitty reply written 30 ms later by a writer thread,
  in under 30 s with a 60 s timeout; `probe` under tmux writes
  `query(Multiplexer::Tmux)`
- D: `write_enter` writes exactly `\x1b[?1049h`; `write_setup` no
  longer contains it and starts with `\x1b[2J`; `write_enter` +
  `write_setup` equals the old setup bytes with `\x1b[2J` after the
  alternate-screen switch
- E: `\x1bPjunk` followed by the `KITTY` fixture parses like `KITTY`;
  an unterminated `\x1b_Gi=31;OK` followed by `\x1b[?0u\x1b[?62c`
  gives `keyboard_flags: Some(0)`, `end: Da1`, `kitty_graphics:
  Missing`
- A is checked by hand: the orchestrator's pseudo-terminal run with
  stdin redirected from `/dev/null` still detects freminal

Verification: the standard ritual.

### 06.10 -- Late probe replies are not typed into the editor

Surfaced by the 06.9 review (finding 4) on `60f510b` and first recorded
as deferred. Promoted by the CodeRabbit review of PR #44: a reply that
arrives after the probe gave up reaches crossterm's reader, which turns
it into key events that type text into the open buffer. That is a
startup defect, so it is fixed before merge.

How late replies look (crossterm 0.29's parser, checked):

- A DCS / APC / OSC reply (`ESC P …`, `ESC _ …`, `ESC ] …`) arrives as
  one key press `Alt+P` (`ALT | SHIFT`, `Char('P')`), `Alt+_` or
  `Alt+]`, then one plain (or Shift for upper case) `Char` press per
  payload byte, then `Alt+\` for the `ESC \` terminator (or `Ctrl+G`
  for a BEL terminator). Those events come out back to back, because
  they were parsed from one read.
- A late CSI reply (`CSI ? flags u`, DA1, `CSI 6;h;w t`) is never
  delivered as an event (crossterm keeps the first two internal and
  drops what it cannot parse), so it needs nothing.
- A late reply can only happen when the probe ended without DA1
  (`ReplyEnd::Incomplete`: a timeout, or a probe error) or under tmux
  (the passthrough kitty reply). Otherwise every reply came before DA1
  and was consumed.

Scope: `frext-term/src/term/late_replies.rs` (new),
`frext-term/src/term/mod.rs`, `frext-term/src/terminal.rs`,
`frext-term/src/main.rs`.

What:

- `term/late_replies.rs`:
  - `pub const LATE_REPLY_WINDOW: Duration = Duration::from_secs(5);`,
    `pub const REPLY_GAP: Duration = Duration::from_millis(50);`,
    `pub const MAX_REPLY_EVENTS: usize = 512;`
  - `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum Verdict {
    Pass, Drop }`
  - `#[derive(Debug, Clone)] pub struct LateReplyFilter` with private
    state: a private `enum Arming { Never, Until(Instant), NoEnd }`, and
    a private `enum State { Idle, InReply { last: Instant, events:
    usize } }`.
  - `#[must_use] pub fn new(end: ReplyEnd, multiplexer: Multiplexer,
    now: Instant) -> Self`: armed until `now + LATE_REPLY_WINDOW` when
    `end == ReplyEnd::Incomplete` or `multiplexer == Multiplexer::Tmux`
    (`Until`; an overflowing `checked_add` gives `NoEnd`); otherwise
    `Never`.
  - `pub fn verdict(&mut self, event: &Event, now: Instant) -> Verdict`.
    Only key events with `KeyEventKind::Press` count as key events
    here; everything else is "another event".
    - `Idle`: when armed (`now` before `armed_until`) and the event is a
      key press whose modifiers contain `ALT` and whose code is
      `Char('P')`, `Char('_')` or `Char(']')`: become `InReply { last:
      now, events: 1 }` and `Drop`. Otherwise `Pass`.
    - `InReply { last, events }`:
      - When `now - last > REPLY_GAP` (a person typing, not a burst),
        or `events >= MAX_REPLY_EVENTS`: become `Idle`, then judge the
        event as `Idle` would.
      - A key press `Alt+\` (modifiers contain `ALT`, code
        `Char('\\')`) or `Ctrl+G` (modifiers exactly `CONTROL`, code
        `Char('g')`): become `Idle` and `Drop`. One `log::info!` says a
        late terminal reply of `events + 1` events was dropped (never
        its content).
      - A key press `Char(c)` with `c` in `' '..='~'` and modifiers
        empty or exactly `SHIFT`: `Drop`, setting `last = now` and
        adding one to `events`.
      - Anything else: become `Idle` and `Pass`.
  - `pub fn filtered(mut read: impl FnMut() -> io::Result<Event>, mut
    filter: LateReplyFilter, now: impl Fn() -> Instant) -> impl FnMut()
    -> io::Result<Event>`: each call reads until an event the filter
    passes and returns it. Errors are returned as they come.
- `terminal.rs`: `TerminalGuard` also stores the probe's `ReplyEnd`
  (`Replies::default().end` after a probe error) and gains
  `#[must_use] pub const fn probe_end(&self) -> ReplyEnd`.
- `main.rs`: `spawn_input(late_replies::filtered(crossterm::event::read,
  LateReplyFilter::new(guard.probe_end(), guard.caps().multiplexer,
  Instant::now()), Instant::now))`.

Deliverable: tests in `late_replies.rs` (events built with
`KeyEvent::new`; `now` values built from one `Instant` plus offsets):

- not armed (`Da1`, no tmux): `Alt+P` and its payload all pass
- armed (`Incomplete`): the events of `ESC P >|tmux 3.5a ESC \` within
  1 ms of each other are all dropped, and a following plain `x` passes
- armed under tmux with `Da1`: the events of `ESC _ Gi=31;OK ESC \`
  are all dropped
- a BEL-terminated reply (`Alt+]`, payload, `Ctrl+G`) is dropped
- after `LATE_REPLY_WINDOW`, `Alt+P` passes
- `Alt+P` followed 200 ms later by `x`: `Alt+P` is dropped, `x` passes
  (the gap ends the reply)
- `Alt+P` followed by an arrow key: the arrow passes, and a later
  plain `y` passes
- `Alt+P` followed by more than `MAX_REPLY_EVENTS` printable presses
  in a burst: the ones past the limit pass
- key release events and a mouse event inside a reply pass and end it
- `filtered` over a scripted `read` returning a late reply then `q`
  returns `q` first; an `Err` from `read` is returned
- `probe_end` on a directly built guard returns what it was built with

Verification: the standard ritual, plus the orchestrator's
pseudo-terminal run where the fake terminal answers only after 1.5 s:
nothing reaches the buffer (the session's swap file stays empty).

### 06.11 -- Review fixes (PR #44)

Surfaced by the Copilot and CodeRabbit reviews of PR #44, checked
against the code and valid.

Scope: `assets/licenses/ratatui-LICENSE` (new), `ATTRIBUTIONS.md`,
`frext-term/src/term/backend.rs` (module doc), `frext-term/src/term/probe.rs`.

The fixes:

- **A -- ratatui's licence notice** (Copilot). `backend.rs` copies
  ratatui-crossterm 0.1.2 code, and MIT requires its copyright and
  permission notice to be kept.
  - `assets/licenses/ratatui-LICENSE`: the `LICENSE` file shipped in
    the ratatui-crossterm 0.1.2 crate, verbatim.
  - `ATTRIBUTIONS.md`: the intro also mentions adapted third-party
    source code, and a new `## Adapted source code` section lists
    `ratatui-crossterm` (upstream URL, the MIT licence, the licence text
    link, and the file `frext-term/src/term/backend.rs`).
  - The `backend.rs` module doc points at `ATTRIBUTIONS.md` and the
    licence file.
- **B -- the probe log keeps no typed text** (CodeRabbit). `probe`
  logged every byte it read, including anything typed or pasted during
  startup. Now a private `fn reply_sequences(bytes: &[u8]) -> Vec<u8>`
  returns only the complete escape sequences the tokenizer recognises
  (CSI, DCS, APC, OSC), byte for byte, and `probe` logs that at `debug`
  instead. Tests: the `KITTY` fixture is returned unchanged; with
  `hello` before it and `x` inside the gaps, the result is the fixture
  alone; a cut-off sequence at the end is dropped.

Verification: the standard ritual.

## Manual smoke checklist

Run after 06.8, in each terminal available (freminal, kitty, Ghostty,
foot, WezTerm, Alacritty, and inside tmux), with `RUST_LOG=debug`:

1. `frext-term` starts with no visible probe garbage and no startup
   delay in a terminal that answers.
2. `frext-term.log` shows the raw reply bytes (`debug`) and the
   resolved caps (`info`), matching the table in 06.4 for that
   terminal. Record any terminal whose real replies differ from the
   synthesised fixture, and update the fixture.
3. In a terminal with the kitty keyboard protocol, Ctrl+H opens
   replace (Enhanced). Without it, the Legacy status message shows.
4. `--undercurl on` / `--undercurl off` and `[term] undercurl = "on"` /
   `"off"` change the logged `underline`. CLI beats config.
5. `--icons kitty` in a terminal without a cell-size reply logs the
   fallback warning and `icons: Nerd`.
6. An invalid `config.toml` logs a warning and starts with defaults.
   `--icons bogus` exits with the error message.
7. Quitting restores the terminal (no stray keyboard-protocol state).
