# 01 -- Core extraction (`frext-core`)

- **Status:** active
- **Depends on:** none
- **Branch:** `refactor/frext-core` (one branch, one commit per
  subtask, one PR)

## Goal

Move every frontend-independent piece of frext into a new library
crate, `frext-core`, and replace the egui-coupled controller inside
`FrextApp` with a frontend-neutral `Editor` driven by `Command`s. The
GUI must behave identically afterwards. This is the foundation that
`frext-term` (plans 02-09) is built on.

## Non-goals

- No TUI code. `frext-term` starts in plan 02.
- No change to undo/redo. The GUI keeps egui's built-in `TextEdit`
  undo; the TUI gets its own in plan 03.
- No change to the on-disk persistence format or location.
- No reorganisation beyond what the split needs (`module-cohesion`:
  opportunistic reorganisation is its own task).

## Why the current code needs this

The pure modules (`tab`, `persistence`, `workspace`, `search`,
`fs_ops`, `edit_ops`, `clipboard`, `error`, `proofread/{mod, regions,
state, worker}`) are already free of egui. The problems are:

- `app.rs` (~2,900 lines) is both controller and view. Tab lifecycle,
  persistence, search, rename/trash reconciliation and edit actions are
  methods on the same `FrextApp` that renders egui.
- Caret and selection live in egui memory (`TextEditState`):
  `store_caret`, Select All and search scroll-to all write it directly,
  and the selection is read back from `TextEditOutput` each frame.
- `highlight.rs` emits egui `LayoutJob`s directly, and
  `proofread/regions.rs` reaches into it for the shared `SyntaxSet`.
- `theme.rs` stores the Catppuccin palette as egui `Color32`.
- Save-as calls `rfd` from inside `save_tab`, and Copy Path uses
  `egui::Context::copy_text` while Cut/Copy/Paste use `arboard`.
- The dirty-close modal and the name-entry modal render and apply
  their result in one function (`resolve_pending_close`,
  `resolve_pending_fs`).

## Target layout

```text
Cargo.toml                   workspace: members = ["frext-core", "frext"]
frext-core/
  Cargo.toml
  src/
    lib.rs
    clipboard.rs             Clipboard trait, SystemClipboard, MemoryClipboard
    edit_ops.rs
    editor/                  the frontend-neutral controller
      mod.rs                 Editor: state, construction, accessors,
                             frame hooks, apply() dispatch
      command.rs             Command and its small domain enums
      effect.rs              Outcome, BufferChange, Effect, EditorFocus
      prompt.rs              Prompt, AfterSave, NamePrompt, NameKind,
                             CloseChoice
      tabs.rs                impl Editor: open/new/close/save, persistence
      search.rs              SearchState + impl Editor: find/replace
      tree.rs                impl Editor: workspace and filesystem ops
      editing.rs             impl Editor: clipboard edits, suggestions,
                             dictionary, ignore
    error.rs
    file_icon.rs             name -> icon stem lookup and SVG bytes (no egui)
    file_icon_bytes.rs       generated
    file_icon_table.rs       generated
    fs_ops.rs                + reveal()
    highlight.rs             StyledSpan output + search-match overlay
    lines.rs                 line_count, line_numbers_for_rows
    offsets.rs               char <-> byte index conversion
    palette.rs               Catppuccin Mocha as plain RGB + semantic roles
    persistence.rs
    proofread/{mod,regions,state,worker}.rs
    search.rs
    syntax.rs                shared syntect SyntaxSet + theme,
                             language_from_path
    tab.rs
    workspace.rs
frext/                       GUI; package and binary name unchanged
  Cargo.toml
  benches/highlight.rs
  src/
    main.rs
    lib.rs
    app.rs                   FrextApp: egui rendering; input -> Command;
                             Effect -> TextEditState
    editor_layout.rs         StyledSpan -> egui LayoutJob; layouter()
    font.rs
    icon.rs
    squiggle.rs              moved from proofread/squiggle.rs
    theme.rs                 palette -> egui Visuals
```

`frext-core` must not depend on `egui`, `eframe`, `egui_extras`, `rfd`,
`ratatui` or `crossterm`. The crate boundary enforces this from 01.7
onward.

## Durable design decisions

### The `Editor` controller

`Editor` owns everything `FrextApp` owns today except view state:
`store`, `tabs`, `active`, `next_id`, `workspace`, `search`, `prompt`,
`proofread`, `dictionary_file`, plus a `Box<dyn Clipboard>` and the
view-reported selection. The GUI's `FrextApp` keeps only view state:
the `Editor`, `menu_diagnostic` (which diagnostic was right-clicked)
and a queue of effects waiting to be applied on the next frame.

```rust
impl Editor {
    pub fn open(
        store: Store,
        clipboard: Box<dyn Clipboard>,
        files: &[PathBuf],
        dir: Option<&Path>,
    ) -> Self;

    // Frame hooks, called by every front end.
    pub fn begin_frame(&mut self, notify: impl Fn() + Send + 'static);
    pub fn end_frame(&mut self);
    pub fn shutdown(&self);

    // Commands.
    pub fn apply(&mut self, command: Command) -> Outcome;

    // Live inputs from the view.
    pub fn set_selection(&mut self, chars: Option<Range<usize>>);
    pub fn active_view_mut(&mut self) -> Option<ActiveView<'_>>;
    pub fn commit_edit(&mut self);
    pub fn search_pattern_mut(&mut self) -> &mut String;
    pub fn search_replacement_mut(&mut self) -> &mut String;
    pub fn prompt_name_mut(&mut self) -> Option<&mut String>;

    // Read-only accessors.
    pub fn tabs(&self) -> &[Tab];
    pub fn active_index(&self) -> usize;
    pub fn active_tab(&self) -> Option<&Tab>;
    pub fn workspace(&self) -> Option<&Workspace>;
    pub fn search(&self) -> &SearchState;
    pub fn prompt(&self) -> Option<&Prompt>;
    pub fn proofread(&self) -> &Proofreading;
    pub fn match_highlights(&self) -> MatchHighlights;
    pub fn relative_to_root(&self, path: &Path) -> String;
}

/// Split borrow for the editor surface: the active tab's buffer (mutable,
/// so an immediate-mode text widget can bind to it) alongside the
/// proofreading results it renders.
pub struct ActiveView<'a> {
    pub tab: &'a mut Tab,
    pub proofread: &'a Proofreading,
}
```

- `begin_frame` replaces the top of `FrextApp::ui`: start the proofread
  worker (`notify` is the repaint hook; only the first call's closure
  is used), poll results, and reload the active tab if its file changed
  on disk (persisting swap and session when it does).
- `end_frame` queues a proofread of the active buffer.
- `shutdown` is today's `on_exit` flush.
- A view that mutated the buffer through `active_view_mut` calls
  `commit_edit`: persist the active swap and recompute matches when the
  search bar is open. This is today's `if changed { ... }` block.
- `set_selection` takes a **char** range (what both egui and the TUI
  speak). The byte range that search-in-selection needs is derived
  inside the core with `offsets`.

### Commands

The GUI's four deferred-action enums (`MenuAction`, `EditorAction`,
`TreeAction`, `SearchAction`) and `CloseChoice` collapse into one
`Command`:

```rust
pub enum Command {
    // Tabs.
    NewTab,
    SelectTab(usize),
    RequestClose(usize),
    RequestCloseActive,
    OpenPath(PathBuf),
    Save,
    // Prompts.
    ResolveClose(CloseChoice),
    SubmitSavePath(PathBuf),
    SubmitName,
    CancelPrompt,
    // Search.
    OpenSearch(SearchBar),
    CloseSearch,
    StepMatch(StepDirection),
    RecomputeMatches,
    ToggleSearchOption(SearchOption),
    ToggleReplaceRow,
    ReplaceCurrent,
    ReplaceAll,
    // Editing.
    Cut,
    Copy,
    Paste,
    SelectAll,
    ApplySuggestion { bytes: Range<usize>, flagged: String, replacement: String },
    AddToDictionary(String),
    Ignore(IgnoreKey),
    // Workspace.
    SetExpanded { dir: PathBuf, state: FolderState },
    CloseWorkspace,
    CopyPath(PathBuf),
    CopyRelativePath(PathBuf),
    Reveal(PathBuf),
    BeginRename(PathBuf),
    BeginNewFile(PathBuf),
    BeginNewFolder(PathBuf),
    Trash(PathBuf),
}

pub enum SearchBar { Find, FindReplace }
pub enum StepDirection { Forward, Backward }
pub enum SearchOption { CaseSensitive, WholeWord, Regex, InSelection }
pub enum FolderState { Expanded, Collapsed }
```

"Open file" is deliberately **not** a command. Choosing a path is a
front-end concern (`rfd` in the GUI, an in-terminal prompt in the TUI);
the front end then sends `OpenPath`.

### Outcomes and effects

```rust
pub struct Outcome {
    pub buffer: BufferChange,
    pub effects: Vec<Effect>,
}

pub enum BufferChange { Unchanged, Edited }

pub enum Effect {
    /// Set the editor's selection (a char range; empty = caret).
    SetSelection { chars: Range<usize>, focus: EditorFocus },
    /// Scroll the editor so its selection is visible, centred.
    RevealSelection,
    /// Give keyboard focus to the find field.
    FocusSearchField,
    /// Give keyboard focus to the active prompt's text field.
    FocusPromptField,
}

pub enum EditorFocus { Focus, Keep }
```

These replace every place the controller pokes view state today:
`store_caret` and Select All become `SetSelection { focus: Focus }`;
search's `scroll_to` becomes `SetSelection { focus: Keep }` plus
`RevealSelection`; `search.focus_requested` and
`FsPrompt::focus_requested` become the two focus effects. The GUI
queues effects and applies them during the next frame's render, which
is exactly when it applies `scroll_to` and the focus flags today.
`BufferChange::Edited` replaces the `bool` returned by
`apply_editor_action` (`state-representation`: a bool crossing a crate
boundary becomes a named enum).

### One prompt at a time

`pending_close: Option<usize>` and `pending_fs: Option<FsPrompt>` become
a single `prompt: Option<Prompt>`, so two simultaneous modals are
unrepresentable (`state-representation`, case 2):

```rust
pub enum Prompt {
    ConfirmClose { tab_id: u64 },
    SavePath { tab_id: u64, then: AfterSave },
    Name(NamePrompt),
}

pub enum AfterSave { Nothing, CloseTab }

pub struct NamePrompt {
    pub kind: NameKind,
    pub target: PathBuf,
    pub name: String,
    pub error: Option<String>,
}

pub enum NameKind { Rename, NewFile, NewFolder }

pub enum CloseChoice { Save, Discard, Cancel }
```

- Prompts refer to tabs by **id**, not index, so a reordered or closed
  tab can never be confused with another.
- `save_tab` no longer calls `rfd`. Saving an untitled tab sets
  `Prompt::SavePath`. The GUI renders that prompt by running
  `rfd_save_file()` straight away and answering `SubmitSavePath(path)`
  or `CancelPrompt`. "Save and close" on an untitled dirty tab becomes
  `ConfirmClose` -> `SavePath { then: CloseTab }`, and the visible
  behaviour is unchanged.
- GUI-local "no button pressed yet" stays local (`Option<CloseChoice>`
  in the modal code).

### Clipboard

`clipboard.rs` gains a trait. `SystemClipboard` wraps the existing
`arboard` functions (same create-per-call behaviour as today);
`MemoryClipboard` backs tests.

```rust
pub trait Clipboard {
    fn read_text(&mut self) -> Option<String>;
    fn write_text(&mut self, text: String) -> bool;
}
```

Copy Path and Copy Relative Path move from `egui::Context::copy_text`
to the trait, so every clipboard write goes through one path. plan 02
adds an OSC 52 implementation for the TUI.

### Highlighting spans and palette

```rust
// frext-core/src/palette.rs
pub struct Rgb { pub r: u8, pub g: u8, pub b: u8 }
pub struct Tint { pub color: Rgb, pub opacity: f32 }

// frext-core/src/highlight.rs
pub struct StyledSpan {
    pub bytes: Range<usize>,
    pub fg: Rgb,
    pub background: Option<MatchBackground>,
    pub italic: bool,
    pub underline: bool,
}

pub enum MatchBackground { Other, Current }

pub fn highlight(text: &str, language: &str) -> Vec<StyledSpan>;
pub fn overlay_matches(spans: Vec<StyledSpan>, matches: &MatchHighlights) -> Vec<StyledSpan>;
```

- `italic` and `underline` are independent signals that may both hold,
  so they stay bools (`state-representation`, "bools stay").
- A search background is a **role** (`MatchBackground`), not a colour.
  Each front end maps the role to its own colour. The GUI keeps exactly
  today's `SURFACE2` and `BLUE.linear_multiply(0.55)`, and the TUI
  blends the same tint over `BASE`.
- The two translucent palette roles (`search_current_match`, selection
  fill) are exposed as `Tint` so egui can keep `linear_multiply` (pixel
  identical) while the terminal blends to an opaque colour.
- The plain-text fallback keeps today's foreground (`Color32::LIGHT_GRAY`
  = `Rgb { r: 220, g: 220, b: 220 }`). Switching it to palette `TEXT`
  would be a visible change and is out of scope.

### Deliberate behaviour changes

Only these. Anything else that changes is a bug.

1. Copy Path / Copy Relative Path write through `arboard` instead of
   egui's clipboard. Check it by hand on Wayland and X11.
2. A dirty-close prompt and a name prompt can no longer be open at the
   same time. Today egui's modal backdrop already prevents this in
   practice.

## Execution model

All subtasks run **strictly in sequence** on one branch. Nearly every
subtask touches `frext/src/app.rs` or the Cargo manifests, so there is
no parallel-safe split. Each subtask is one commit, leaves the full
verification suite green (`commit-discipline`), and gets a CODE-REVIEW
pass before the next starts.

Every subtask's verification includes the standard ritual, run inside
`nix develop`:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
cargo machete
```

## Subtasks

### 01.1 -- Characterisation test and highlight benchmark

Scope: `frext/src/highlight.rs`, `frext/benches/highlight.rs` (new),
`frext/Cargo.toml`, `Cargo.toml`.

What: Make the body of `layouter` callable without an `egui::Ui` by
extracting `pub fn layout_job(text: &str, language: &str, font_id:
FontId, matches: &MatchHighlights) -> LayoutJob` (it does what the
closure does today minus `wrap.max_width` and `fonts_mut`), and have
`layouter` call it. Add a characterisation test,
`layout_job_matches_golden_sections`: a fixed 6-line Rust snippet with
two literal matches, one of them current, asserting every section's
`(byte_range, color, italics, underline, background)`. Add a criterion
benchmark, `benches/highlight.rs` (`harness = false`), with three
cases over a deterministic synthetic Rust source of 5,000 lines (a
fixed 50-line template repeated 100 times, built in the bench, with no
fixture file):

- `highlight/rust_5k_lines` (no matches)
- `highlight/rust_5k_lines_200_matches` (a literal `SearchQuery`
  producing about 200 matches, one current)
- `highlight/plain_5k_lines` (empty language token)

Add `criterion` as a workspace dev-dependency, pinned to the current
crates.io release at full `major.minor.patch` (check with
`cargo search criterion`), alphabetically sorted.

Deliverable: the extraction, the golden test, the benchmark, and a
saved baseline: `cargo bench -p frext --bench highlight --
--save-baseline before-core-split`. Record the three numbers in the
completion report.

Verification: the standard ritual, plus the bench command above.

Prohibitions: do NOT change highlighting behaviour; do NOT touch
`app.rs`; do NOT proceed to 01.2.

Stop: report files changed, verification results and baseline numbers;
await review.

### 01.2 -- Neutral palette

Scope: `frext/src/palette.rs` (new), `frext/src/theme.rs`,
`frext/src/lib.rs`.

What: Add `palette.rs` with `Rgb`, `Tint`, the Catppuccin Mocha
constants now in `theme::mocha` (`ROSEWATER`, `BLUE`, `TEXT`,
`SUBTEXT0`, `OVERLAY1`, `OVERLAY0`, `SURFACE2`, `SURFACE1`, `SURFACE0`,
`BASE`, `MANTLE`, `RED`, `GREEN`) as `Rgb`, and the semantic role
functions now in `theme.rs` (`accent`, `gutter`, `search_match`,
`spelling_squiggle`, `grammar_squiggle`, `error` returning `Rgb`;
`search_current_match` returning `Tint { color: BLUE, opacity: 0.55 }`;
a new `selection` role returning `Tint { color: BLUE, opacity: 0.35 }`).
`theme.rs` keeps `apply()` and keeps public functions with today's
names and `Color32` return types, implemented as conversions from
`palette` (`Tint` via `Color32::from(rgb).linear_multiply(opacity)`), so
no caller changes. Add `palette` unit tests (constants match the
published Mocha hex values; `Tint` opacities).

Deliverable: `palette.rs` plus tests; `theme.rs` reduced to egui
conversion and `apply()`.

Verification: the standard ritual.

Prohibitions: do NOT change any rendered colour; do NOT add a
blend-over helper yet (plan 02 adds it, beside its only user); do NOT
proceed to 01.3.

Stop: report and await review.

### 01.3 -- Syntax resources and neutral highlight spans

Scope: `frext/src/syntax.rs` (new), `frext/src/highlight.rs`,
`frext/src/editor_layout.rs` (new), `frext/src/proofread/regions.rs`,
`frext/src/app.rs` (import paths only), `frext/src/lib.rs`,
`frext/benches/highlight.rs`.

What:

- Move the `Highlighter` `OnceLock`, `syntax_set()` and
  `language_from_path()` (with its tests) from `highlight.rs` into
  `syntax.rs`. `syntax.rs` exposes `pub(crate) fn syntax_set()`,
  `pub(crate) fn theme()` and `pub fn language_from_path`.
- Rewrite `highlight.rs` to be egui-free: `StyledSpan`,
  `MatchBackground`, `MatchHighlights` (unchanged fields),
  `pub fn highlight(text, language) -> Vec<StyledSpan>` (plain
  fallback: one span, fg `Rgb { r: 220, g: 220, b: 220 }`), and
  `pub fn overlay_matches(...)` (a port of `apply_match_highlights` /
  `split_section_by_background` operating on spans).
- `editor_layout.rs` holds the egui side: `layout_job` (moved from
  01.1), now built from `overlay_matches(highlight(..))` by converting
  spans to `LayoutSection`s (fg -> `Color32`, italic -> `italics`,
  underline -> `Stroke::new(1.0, fg)`, `MatchBackground` ->
  `theme::search_match()` / `theme::search_current_match()`), plus
  `layouter`.
- Port the existing highlight tests: span-level tests go to
  `highlight.rs`, and the golden `LayoutJob` test from 01.1 goes to
  `editor_layout.rs` **unchanged** (it must still pass; that is the
  proof the GUI renders identically).
- `regions.rs` uses `crate::syntax::syntax_set()`.

Deliverable: the split, ported tests, and a benchmark comparison:
`cargo bench -p frext --bench highlight -- --baseline before-core-split`.

Verification: the standard ritual, plus the bench comparison. A
regression over 15% on any case must be justified in the report or
fixed (`performance-benchmarks`).

Prohibitions: do NOT alter the golden test's expectations; do NOT
change match-overlay semantics; do NOT proceed to 01.4.

Stop: report, including the before/after benchmark table; await
review.

### 01.4 -- Line numbering and text offsets modules

Scope: `frext/src/lines.rs` (new), `frext/src/offsets.rs` (new),
`frext/src/edit_ops.rs`, `frext/src/app.rs`, `frext/src/lib.rs`.

What: Move `line_count` and `line_numbers_for_rows` (and their six
tests) from `app.rs` to `lines.rs` as `pub fn`. Add `offsets.rs` with
`pub fn byte_of_char(text: &str, char_idx: usize) -> usize` and
`pub fn char_of_byte(text: &str, byte: usize) -> usize` (today's
`app.rs` `byte_index` / `char_index`, clamping identically), with tests
covering ASCII, multi-byte and out-of-range input. Replace `app.rs`'s
`byte_index` / `char_index` and `edit_ops.rs`'s private `byte_offset`
with calls to `offsets`.

Deliverable: two modules with tests; no remaining duplicate index
helpers.

Verification: the standard ritual.

Prohibitions: do NOT change clamping behaviour; do NOT proceed to 01.5.

Stop: report and await review.

### 01.5 -- Remove the last egui touchpoints from soon-to-be-core modules

Scope: `frext/src/file_icon.rs`, `frext/src/fs_ops.rs`,
`frext/src/proofread/mod.rs`, `frext/src/proofread/squiggle.rs` ->
`frext/src/squiggle.rs` (git mv), `frext/src/app.rs`,
`frext/src/lib.rs`.

What:

- Move `file_icon::image_source` and `stem_uri` (with its test) into
  `app.rs` beside its only caller, `FrextApp::file_icon`.
  `file_icon.rs` keeps the lookups and `icon_bytes`.
- Move `reveal_in_file_manager` from `app.rs` into `fs_ops` as
  `pub fn reveal(path: &Path)`, splitting out
  `pub fn reveal_target(path: &Path) -> PathBuf` (directory -> itself,
  file -> parent), with tests for both cases.
- `git mv frext/src/proofread/squiggle.rs frext/src/squiggle.rs`; drop
  `pub mod squiggle` from `proofread/mod.rs` and its doc-comment
  reference; update the call site in `app.rs`.

Deliverable: `file_icon`, `fs_ops` and `proofread` contain no `egui`
references.

Verification: the standard ritual, plus
`rg -n 'egui|eframe' frext/src/{tab,persistence,workspace,search,fs_ops,edit_ops,clipboard,error,file_icon,palette,highlight,syntax,lines,offsets}.rs frext/src/proofread/`
which must print nothing except prose in doc comments, each of which
is reworded to not name egui.

Prohibitions: do NOT change squiggle rendering; do NOT proceed to 01.6.

Stop: report and await review.

### 01.6 -- Clipboard trait

Scope: `frext/src/clipboard.rs`, `frext/src/app.rs`.

What: Add the `Clipboard` trait, `SystemClipboard` (the existing
`read_text` / `write_text` bodies, unchanged create-per-call
behaviour) and `MemoryClipboard` (`Default`, holds an
`Option<String>`), each with tests (`MemoryClipboard` round-trip; the
trait object works through `Box<dyn Clipboard>`). Remove the free
functions. `FrextApp` gains `clipboard: Box<dyn Clipboard>`, built in
`with_args` as `SystemClipboard`. Cut/Copy/Paste and Copy Path / Copy
Relative Path all go through it (the `ctx.copy_text` calls go away).
Existing `app.rs` clipboard tests switch to `MemoryClipboard` through a
test-only constructor, `FrextApp::with_clipboard(store, clipboard)`.

Deliverable: one clipboard path; tests no longer touch the real OS
clipboard.

Verification: the standard ritual. Manual: Copy Path from the sidebar
pastes correctly into another application (record which display server
was used).

Prohibitions: do NOT add OSC 52 (plan 02); do NOT proceed to 01.7.

Stop: report and await review.

### 01.7 -- Create the `frext-core` crate

Scope: `Cargo.toml`, `frext/Cargo.toml`, `frext-core/**` (new, via
`git mv`), `frext/src/lib.rs`, `frext/src/main.rs`, `frext/src/app.rs`,
`frext/src/editor_layout.rs`, `frext/src/theme.rs`,
`frext/src/squiggle.rs`, `frext/benches/highlight.rs`, `flake.nix`
(pre-commit `extraExcludes` paths only).

What: Create `frext-core` (inheriting `workspace.package` fields and
`[lints] workspace = true`) and `git mv` into `frext-core/src/`:
`clipboard.rs`, `edit_ops.rs`, `error.rs`, `file_icon.rs`,
`file_icon_bytes.rs`, `file_icon_table.rs`, `fs_ops.rs`,
`highlight.rs`, `lines.rs`, `offsets.rs`, `palette.rs`,
`persistence.rs`, `proofread/{mod,regions,state,worker}.rs`,
`search.rs`, `syntax.rs`, `tab.rs`, `workspace.rs`. Move each module's
dependencies to `frext-core/Cargo.toml` (`arboard`, `directories`,
`harper-core`, `log`, `regex`, `serde`, `serde_json`, `thiserror`,
`trash`, `two-face`) and remove from `frext/Cargo.toml` whatever the
GUI no longer uses directly. `frext` depends on `frext-core` by path.
`file_icon_bytes.rs`'s `include_bytes!` paths stay valid (same depth).
Update the two `flake.nix` excludes from `^frext/src/file_icon_*` to
`^frext-core/src/file_icon_*`. Any visibility widened from
`pub(crate)` to `pub` must be listed in the report with a reason; if
`syntax_set` / `theme` must become `pub` for the GUI, they become
`pub` with doc comments.

Deliverable: a two-crate workspace; the GUI builds and behaves as
before.

Verification: the standard ritual, plus `nix build .#frext` and
`cargo tree -p frext-core | rg -i 'egui|eframe|winit|rfd'` printing
nothing.

Prohibitions: do NOT change any code beyond `use` paths, `crate::` ->
`frext_core::` and required visibility; do NOT proceed to 01.8.

Stop: report (including the visibility list) and await review.

### 01.8 -- `Editor` foundation: tab lifecycle and prompts

Scope: `frext-core/src/editor/{mod,command,effect,prompt,tabs}.rs`
(new), `frext-core/src/lib.rs`, `frext/src/app.rs`.

What:

- Add `Editor` with every field listed under "The `Editor`
  controller", `Editor::open` (today's `with_args` logic,
  dictionary-load handling included), the read-only accessors,
  `active_view_mut` / `commit_edit`, `set_selection`, `shutdown`, and
  `apply`.
- Add `Outcome`, `BufferChange`, `Effect` and `EditorFocus` in full, as
  specified above.
- Add `Prompt` with the `ConfirmClose` and `SavePath` variants only
  (`Name` arrives in 01.10), plus `AfterSave` and `CloseChoice`.
- Add the tab-lifecycle commands: `NewTab`, `SelectTab`,
  `RequestClose`, `RequestCloseActive`, `OpenPath`, `Save`,
  `ResolveClose`, `SubmitSavePath`, `CancelPrompt`.
- In `tabs.rs`, move `open_path`, `alloc_id`, `persist_session`,
  `persist_active_swap`, `new_tab`, `request_close_tab`, `close_tab`
  and `save_tab` from `FrextApp`. `save_tab` returns
  `SaveOutcome { Saved, NeedsPath, Failed }` (private) and never calls
  `rfd`.
- `FrextApp` holds `editor: Editor` and keeps search, tree and editing
  logic in place for now (calling through to the editor's accessors
  where it needs tabs). `MenuAction` is deleted (the tab bar and
  shortcuts build `Command`s). The confirm-close modal renders from
  `editor.prompt()` and answers with `ResolveClose`. A
  `Prompt::SavePath` is resolved in the same frame by calling
  `rfd_save_file()` and answering `SubmitSavePath` or `CancelPrompt`.
- The GUI's effect applier (`FrextApp::apply_effects`, run inside the
  editor closure next frame) handles all four `Effect` variants,
  replacing `store_caret`.
- Move the tab-lifecycle tests from `app.rs` to `editor` tests,
  rewritten against `Editor` with `Store::at(tempdir)` and
  `MemoryClipboard`. New tests: untitled `Save` sets `Prompt::SavePath`;
  `SubmitSavePath` writes the file and clears the prompt; confirm-close
  with `Save` on an untitled tab chains into
  `SavePath { then: CloseTab }` and closes only after a successful save;
  `CancelPrompt` on that chain keeps the tab open and dirty.

Deliverable: tab lifecycle owned by `Editor`; the GUI drives it with
`Command`s.

Verification: the standard ritual.

Prohibitions: do NOT move search, tree or editing logic yet; do NOT
add `Prompt::Name`; do NOT proceed to 01.9.

Stop: report and await review.

### 01.9 -- Search into the `Editor`

Scope: `frext-core/src/editor/{mod,command,search}.rs`,
`frext/src/app.rs`.

What: Move `SearchState` and `open_search`, `active_selection_text`,
`close_search`, `recompute_matches`, `step_match`, `replace_current`,
`replace_all`, `match_highlights` and `match_count_label` into
`editor/search.rs`. `SearchState` fields become private with read
accessors (`is_open`, `replace_open`, `in_selection`, `query`,
`matches`, `current`, `error`, `count_label`). `focus_requested` and
`scroll_to` are removed in favour of `Effect::FocusSearchField`,
`Effect::SetSelection { focus: Keep }` and `Effect::RevealSelection`.
Add the search commands (`OpenSearch`, `CloseSearch` (which also
persists the session, as today), `StepMatch`, `RecomputeMatches`,
`ToggleSearchOption`, `ToggleReplaceRow`, `ReplaceCurrent`,
`ReplaceAll`) and `search_pattern_mut` / `search_replacement_mut`.
Delete `SearchAction`. The GUI's search bar binds its two text fields
to the `_mut` accessors, sends `RecomputeMatches` when they change,
and sends `ToggleSearchOption` for each toggle. Move the search tests
from `app.rs` to `editor` tests.

Deliverable: find/replace owned by `Editor`.

Verification: the standard ritual. Manual: Ctrl+F seeds from a
single-line selection; F3 / Shift+F3 step and scroll; Esc closes;
regex error shows inline; replace current/all.

Prohibitions: do NOT change search semantics; do NOT proceed to 01.10.

Stop: report and await review.

### 01.10 -- Workspace and filesystem operations into the `Editor`

Scope: `frext-core/src/editor/{mod,command,prompt,tree}.rs`,
`frext/src/app.rs`.

What: Add `Prompt::Name(NamePrompt)` and `NameKind`. Move
`close_workspace`, `relative_to_root`, `trash_path`,
`close_tabs_under`, `rename_path`, `prune_workspace_expanded`, the
apply half of `resolve_pending_fs` (as `SubmitName`) and the
non-render half of `apply_tree_action` into `editor/tree.rs`. Add
`SetExpanded`, `CloseWorkspace`, `CopyPath`, `CopyRelativePath`,
`Reveal` (via `fs_ops::reveal`), `BeginRename`, `BeginNewFile`,
`BeginNewFolder`, `Trash`, `SubmitName` and `prompt_name_mut`.
`Begin*` sets `Prompt::Name` and emits `Effect::FocusPromptField`; a
failed `SubmitName` keeps the prompt open with `error` set and emits
`FocusPromptField` (today's behaviour). Delete `TreeAction`, `FsPrompt`
and `FsPromptKind`. The sidebar and its context menus build `Command`s;
the name modal renders from `editor.prompt()`. Move the rename / trash
/ relative-path tests to `editor` tests (renaming the existing `close_tabs_under_*`
test, whose name `typos` flags as a misspelling, to
`close_tabs_under_closes_open_files_in_a_directory`), and add tests for
`SubmitName` success (new file opens in a tab) and failure (prompt
stays, error set).

Deliverable: sidebar behaviour owned by `Editor`.

Verification: the standard ritual. Manual: rename a file and a folder
with open tabs under them; new file and new folder; trash; Copy
relative path.

Prohibitions: do NOT change the tree's egui rendering (the
`CollapsingState` mirroring stays as is, now sending `SetExpanded`); do
NOT proceed to 01.11.

Stop: report and await review.

### 01.11 -- Editing, proofreading and frame hooks into the `Editor`

Scope: `frext-core/src/editor/{mod,command,editing}.rs`,
`frext/src/app.rs`, `frext/src/main.rs`.

What: Move the egui-free body of `apply_editor_action` into
`editor/editing.rs` as the `Cut`, `Copy`, `Paste`, `SelectAll`,
`ApplySuggestion`, `AddToDictionary` and `Ignore` commands, acting on
the selection given by `set_selection`. Caret moves become
`Effect::SetSelection { focus: Focus }`, and the edited flag becomes
`BufferChange`. `Find`, `FindReplace` and `Save` from the context menu
map to the existing commands. Add `begin_frame` and `end_frame` as
specified. Delete `EditorAction`, `DictionaryFile` from `app.rs` and
`FrextApp::apply_editor_action`. `FrextApp::ui` now calls
`editor.begin_frame(..)` first and `editor.end_frame()` last, and
`on_exit` calls `editor.shutdown()`. Move the cut/copy/suggestion/
dictionary tests to `editor` tests.

Deliverable: `FrextApp` holds only view state (`editor`,
`menu_diagnostic`, queued effects); every behaviour lives in
`frext-core`.

Verification: the standard ritual, plus
`rg -n 'fn (open_path|save_tab|close_tab|recompute_matches|rename_path)' frext/src`
printing nothing. Manual: run through the full smoke checklist below.

Prohibitions: do NOT touch rendering code beyond replacing action
plumbing; do NOT proceed to 01.12.

Stop: report and await review.

### 01.12 -- Documentation

Scope: `agents.md`, `README.md`.

What: Update `agents.md`'s workspace layout and architecture paragraph
for the two-crate workspace, the `Editor` / `Command` / `Effect` flow,
and a new non-negotiable rule: "`frext-core` never depends on a UI
toolkit." Fix the layout block's paths (it predates the `frext/`
subdirectory). Update `README.md`'s Features list (syntax highlighting
no longer goes through `egui_extras`) and add a short "Workspace"
section for contributors. Both must pass markdownlint.

Deliverable: docs describe the code as it now is.

Verification: `pre-commit run --all-files` (markdownlint, typos).

Prohibitions: do NOT document `frext-term` (it does not exist yet); do
NOT add summary or progress documents (`no-summary-documents`).

Stop: report and await review.

### 01.13 -- Cleanup: broken intra-doc link in `squiggle.rs`

- **Surfaced:** 01.7 (`d92a291`), by
  `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`.
  Pre-existing: the same link is on `main` at `af9f421`
  (`frext/src/proofread/squiggle.rs:114`).
- **Impact:** `cargo doc` fails with `-D warnings`: the public
  `wave_points` doc links to the private const `HALF_WAVELENGTH`. No
  runtime effect; the verification ritual does not run `cargo doc`.
- **Scope of fix:** `frext/src/squiggle.rs` doc comment only.
- **Suggested approach:** replace the intra-doc link with plain code
  formatting (`` `HALF_WAVELENGTH` ``), or document the constant's value
  inline.
- **Verification:** the command above exits 0, plus the standard ritual.
- **Scheduling:** independent of every other subtask; may land at any
  point before the PR is marked `pending merge`.

### 01.14 -- Fix: keep the system clipboard handle alive

- **Surfaced:** manual check of deliberate behaviour change 1 on the
  final branch (`49f01af`), on Wayland: Copy Path / Copy Relative Path
  put nothing on the clipboard, logging
  `arboard::platform::linux::x11: Could not hand the clipboard contents
  over to the clipboard manager. The request timed out.`
- **Cause:** 01.6 kept `SystemClipboard`'s create-per-call behaviour
  (this plan's instruction was wrong). On Linux, arboard serves the
  selection from the owning process; dropping the last
  `arboard::Clipboard` hands the contents to a clipboard manager (100 ms
  timeout) and then destroys its window, so without a manager the
  contents are lost immediately. The editor context menu's Cut / Copy
  had the same pre-existing bug on `main`; Ctrl+C worked only because
  egui handles it natively.
- **Scope of fix:** `frext-core/src/clipboard.rs` and the
  `SystemClipboard` construction site in `frext/src/app.rs`.
- **Approach:** `SystemClipboard` holds one lazily created arboard
  handle for the editor's lifetime, dropping it (and reconnecting on
  the next call) only after a failed operation. A private backend seam
  makes the retention logic testable without the OS clipboard.
- **Verification:** regression tests for handle reuse and reconnect;
  the standard ritual; manual: Copy Path and context-menu Copy paste
  into another application on Wayland.
- **Scheduling:** before the PR is marked `pending merge`.

## Manual smoke checklist (whole plan)

The GUI is not covered by UI tests, so before the PR is marked
`pending merge`, run this by hand on the final branch:

1. Restore a session with three tabs, one dirty and one untitled.
2. Type in each tab; restart; edits persist (swap files).
3. Ctrl+N, Ctrl+O (native dialog), Ctrl+S on untitled (native save
   dialog), Ctrl+W on a dirty tab: save / discard / cancel, including
   cancelling the save dialog after "Save and close".
4. `frext <dir>`: expand folders, restart, expansion restored; open a
   file from the tree; the active file is highlighted.
5. Tree context menus: new file, new folder, rename, trash, copy path,
   copy relative path, reveal.
6. Find / replace: Ctrl+F, Ctrl+H, F3 / Shift+F3, regex with capture
   groups, in-selection, Esc.
7. Editor context menu: cut, copy, paste, select all; right-click a
   misspelling: apply a suggestion, add to dictionary, ignore.
8. Syntax highlighting in `.rs`, `.toml`, `.nix`, `.md`; plain text
   for untitled.
9. Modify an open clean file externally: it reloads. Modify a dirty
   one: it does not.
