# 06 -- Terminal capabilities and undercurl

- **Status:** stub
- **Depends on:** 02

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

## Decide at activation

- Undercurl when nothing answers either way: default `Curly` (most
  modern terminals ignore or degrade `4:3`) versus `Straight`.
- Whether the shared config file and its loader live in `frext-core`
  (proposed, so the GUI can read its own `[gui]` table later) or in
  `frext-term`.
- Probe timeout and whether to cache probe results per `TERM_PROGRAM`
  and version.
- Whether tmux's own answers to XTGETTCAP / XTVERSION (rather than the
  outer terminal's) need special handling.
