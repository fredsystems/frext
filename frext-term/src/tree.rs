//! The file-tree sidebar.
//!
//! Core owns the workspace (its root and the set of expanded folders)
//! and every file-system action; every change to the tree goes through a
//! [`frext_core::editor::Command`]. [`FileTree`] owns only the sidebar's
//! view state: the rows from the last refresh, the selected row, the
//! scroll position, the width and the border drag.
//!
//! While the sidebar has focus, keys the global keymap leaves unbound go
//! to [`FileTree::handle_key`], which moves the selection itself and
//! answers with a [`TreeReply`] for whatever the app must do: send a
//! command, open a context menu, or re-lay out after a resize.

use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use frext_core::editor::{Command, Editor, FolderState};
use frext_core::palette::MANTLE;
use frext_core::workspace::{RowKind, TreeRow, Workspace, visible_rows};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};

use crate::hit::{HitMap, Target};
use crate::icons::{AsciiIcons, ICON_WIDTH, IconCell, IconKind, IconProvider};
use crate::paint;
use crate::term::kitty::PLACEHOLDER_CELLS;
use crate::theme;

/// The sidebar width at launch, border included.
pub const DEFAULT_WIDTH: u16 = 30;

/// The narrowest the sidebar can be, border included.
pub const MIN_WIDTH: u16 = 12;

/// The narrowest the editor is squeezed to by a wide sidebar.
pub const MIN_EDITOR_WIDTH: u16 = 20;

/// The header's close button.
pub const CLOSE_LABEL: &str = "[x]";

/// The header cells left of the border needed to draw anything in it.
const HEADER_MIN: u16 = 4;

/// Clamps a sidebar `width` for a terminal `total` columns wide: at least
/// [`MIN_WIDTH`], and at most what leaves [`MIN_EDITOR_WIDTH`] for the
/// editor (but never less than [`MIN_WIDTH`]).
#[must_use]
pub fn clamp_width(width: u16, total: u16) -> u16 {
    width.clamp(
        MIN_WIDTH,
        total.saturating_sub(MIN_EDITOR_WIDTH).max(MIN_WIDTH),
    )
}

/// Whether the sidebar has the keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeFocus {
    /// Keys go to the sidebar; the selected row is highlighted.
    Focused,
    /// Keys go elsewhere.
    Unfocused,
}

/// Whether a [`FileTree::refresh`] changed the rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refresh {
    /// The rows are the same as before.
    Unchanged,
    /// The rows differ, so hit regions built from the old rows are stale.
    Changed,
}

/// What the app must do after [`FileTree::handle_key`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeReply {
    /// Send this command to the editor.
    Send(Command),
    /// Open the context menu for `path` (a row of `kind`) at `anchor`.
    OpenMenu {
        /// The file or folder the menu acts on.
        path: PathBuf,
        /// Whether it is a file or a folder, which picks the menu.
        kind: RowKind,
        /// The cell the menu is anchored at.
        anchor: Position,
    },
    /// The width changed (or was asked to), so the layout is stale.
    Resized,
    /// The tree used the key; nothing else to do.
    Handled,
    /// The tree does not use the key.
    Ignored,
}

/// The modifiers that make the tree ignore a key.
const BLOCKING_MODIFIERS: KeyModifiers = KeyModifiers::CONTROL
    .union(KeyModifiers::ALT)
    .union(KeyModifiers::SUPER)
    .union(KeyModifiers::HYPER)
    .union(KeyModifiers::META);

/// A selection move made by a motion key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Motion {
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
}

/// The state of the border drag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resize {
    /// The border is not being dragged.
    Idle,
    /// The border is being dragged with the mouse.
    Dragging,
}

/// The sidebar's view state: rows, selection, scroll, width and drag.
#[derive(Debug)]
pub struct FileTree {
    /// The rows from the last refresh.
    rows: Vec<TreeRow>,
    /// The workspace root at the last refresh.
    root: Option<PathBuf>,
    /// The selected row's path.
    selected: Option<PathBuf>,
    /// The selection's last known index in `rows`.
    selected_index: usize,
    /// The index of the first row drawn.
    top: usize,
    /// The requested width, border included.
    width: u16,
    /// The columns the sidebar and the editor text share, from the last
    /// [`FileTree::set_space`].
    total: u16,
    /// The sidebar at the last render.
    area: Rect,
    /// The border drag.
    resize: Resize,
    /// Chooses the icon in front of each row.
    icons: Box<dyn IconProvider>,
}

impl Default for FileTree {
    fn default() -> Self {
        Self::new()
    }
}

impl FileTree {
    /// A tree with no rows, nothing selected, the default width and the
    /// ASCII icons.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rows: Vec::new(),
            root: None,
            selected: None,
            selected_index: 0,
            top: 0,
            width: DEFAULT_WIDTH,
            total: 0,
            area: Rect::default(),
            resize: Resize::Idle,
            icons: Box::new(AsciiIcons),
        }
    }

    /// Replaces the provider of the icon in front of each row.
    pub fn set_icons(&mut self, icons: Box<dyn IconProvider>) {
        self.icons = icons;
    }

    /// The bytes the icon provider queued since the last call, to be
    /// written to the terminal before the frame that shows them.
    pub fn take_uploads(&mut self) -> Vec<u8> {
        self.icons.take_uploads()
    }

    /// The bytes that delete every image the icon provider uploaded;
    /// afterwards it has nothing uploaded.
    pub fn release_icons(&mut self) -> Vec<u8> {
        self.icons.release()
    }

    /// The stored width, border included, as clamped at the last
    /// [`FileTree::set_space`] or resize.
    #[must_use]
    pub const fn width(&self) -> u16 {
        self.width
    }

    /// Sets the `columns` the sidebar and the editor text share (the
    /// terminal width minus the line-number gutter), so the editor text
    /// keeps [`MIN_EDITOR_WIDTH`] of them, and clamps the stored width
    /// against them. Keyboard and drag resizing clamp against the same
    /// columns.
    pub fn set_space(&mut self, columns: u16) {
        self.total = columns;
        self.width = clamp_width(self.width, self.total);
    }

    /// The width clamped for a terminal `total` columns wide.
    #[must_use]
    pub fn effective_width(&self, total: u16) -> u16 {
        clamp_width(self.width, total)
    }

    /// The rows from the last refresh.
    #[must_use]
    pub fn rows(&self) -> &[TreeRow] {
        &self.rows
    }

    /// The row at `index`, if there is one.
    #[must_use]
    pub fn row(&self, index: usize) -> Option<&TreeRow> {
        self.rows.get(index)
    }

    /// The selected row's path.
    #[must_use]
    pub fn selected(&self) -> Option<&Path> {
        self.selected.as_deref()
    }

    /// The workspace root at the last refresh.
    #[must_use]
    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// Selects the row at `index`; does nothing when there is none.
    pub fn select(&mut self, index: usize) {
        if let Some(row) = self.rows.get(index) {
            self.selected = Some(row.path.clone());
            self.selected_index = index;
        }
    }

    /// Re-reads the rows of `workspace` (none without one).
    ///
    /// When they changed, the selection is repaired: it keeps its path if
    /// that is still listed, else moves to its nearest listed ancestor,
    /// else to the row at its old index (clamped to the last row), and is
    /// `None` when there are no rows.
    pub fn refresh(&mut self, workspace: Option<&Workspace>) -> Refresh {
        self.root = workspace.map(|ws| ws.root.clone());
        let rows = workspace.map_or_else(Vec::new, visible_rows);
        if rows == self.rows {
            return Refresh::Unchanged;
        }
        self.rows = rows;
        self.repair_selection();
        self.top = self
            .top
            .min(self.rows.len().saturating_sub(self.rows_height()));
        Refresh::Changed
    }

    /// The index of the row for `path`, if it is listed.
    fn position(&self, path: &Path) -> Option<usize> {
        self.rows.iter().position(|row| row.path == path)
    }

    /// Re-points the selection at the current rows.
    fn repair_selection(&mut self) {
        let Some(path) = self.selected.take() else {
            return;
        };
        let index = path
            .ancestors()
            .find_map(|candidate| self.position(candidate))
            .or_else(|| {
                self.rows
                    .len()
                    .checked_sub(1)
                    .map(|last| self.selected_index.min(last))
            });
        if let Some(index) = index {
            self.select(index);
        }
    }

    /// The index of the selected row in the current rows, if any.
    fn current(&self) -> Option<usize> {
        let path = self.selected.as_deref()?;
        match self.rows.get(self.selected_index) {
            Some(row) if row.path == path => Some(self.selected_index),
            _ => self.position(path),
        }
    }

    /// The rows the sidebar showed at the last render (at least 1).
    fn rows_height(&self) -> usize {
        usize::from(self.area.height.saturating_sub(1)).max(1)
    }

    /// Selects row `index` and scrolls it into view.
    fn select_visible(&mut self, index: usize) {
        self.select(index);
        let height = self.rows_height();
        if index < self.top {
            self.top = index;
        } else if index >= self.top.saturating_add(height) {
            self.top = index.saturating_add(1).saturating_sub(height);
        }
    }

    /// Handles a key while the sidebar has focus, over the rows of the
    /// last refresh.
    ///
    /// Up / Down / PageUp / PageDown / Home / End move the selection;
    /// Right and Left expand, collapse or walk the folders; Enter toggles
    /// a folder or opens a file; the editor's menu keys
    /// ([`crate::keymap::is_menu_key`]: Shift+F10, or F22 / Menu plain or
    /// with Shift) open the context menu; `<` and `>` narrow and widen
    /// the sidebar. Any key with Ctrl, Alt, Super, Hyper or Meta, and any
    /// release, is [`TreeReply::Ignored`].
    pub fn handle_key(&mut self, key: KeyEvent) -> TreeReply {
        if key.kind == KeyEventKind::Release || key.modifiers.intersects(BLOCKING_MODIFIERS) {
            return TreeReply::Ignored;
        }
        if crate::keymap::is_menu_key(key.code, key.modifiers) {
            return self.open_menu();
        }
        let plain = key.modifiers.is_empty();
        let shifted = key.modifiers == KeyModifiers::SHIFT;
        match key.code {
            KeyCode::Up if plain => self.motion(Motion::Up),
            KeyCode::Down if plain => self.motion(Motion::Down),
            KeyCode::PageUp if plain => self.motion(Motion::PageUp),
            KeyCode::PageDown if plain => self.motion(Motion::PageDown),
            KeyCode::Home if plain => self.motion(Motion::Home),
            KeyCode::End if plain => self.motion(Motion::End),
            KeyCode::Right if plain => self.right(),
            KeyCode::Left if plain => self.left(),
            KeyCode::Enter if plain => self.enter(),
            KeyCode::Char('<') if plain || shifted => {
                self.width = clamp_width(self.width.saturating_sub(1), self.total);
                TreeReply::Resized
            }
            KeyCode::Char('>') if plain || shifted => {
                self.width = clamp_width(self.width.saturating_add(1), self.total);
                TreeReply::Resized
            }
            _ => TreeReply::Ignored,
        }
    }

    /// Moves the selection; with nothing selected, selects the first row
    /// (the last for End).
    fn motion(&mut self, motion: Motion) -> TreeReply {
        let Some(last) = self.rows.len().checked_sub(1) else {
            return TreeReply::Handled;
        };
        let page = self.rows_height().saturating_sub(1).max(1);
        let target = match (self.current(), motion) {
            (_, Motion::End) => last,
            (None, _) | (Some(_), Motion::Home) => 0,
            (Some(index), Motion::Up) => index.saturating_sub(1),
            (Some(index), Motion::Down) => index.saturating_add(1).min(last),
            (Some(index), Motion::PageUp) => index.saturating_sub(page),
            (Some(index), Motion::PageDown) => index.saturating_add(page).min(last),
        };
        self.select_visible(target);
        TreeReply::Handled
    }

    /// Right: expands a collapsed folder, or steps into an expanded one.
    fn right(&mut self) -> TreeReply {
        let Some(index) = self.current() else {
            return TreeReply::Handled;
        };
        let Some(row) = self.rows.get(index) else {
            return TreeReply::Handled;
        };
        match row.kind {
            RowKind::Dir(FolderState::Collapsed) => TreeReply::Send(Command::SetExpanded {
                dir: row.path.clone(),
                state: FolderState::Expanded,
            }),
            RowKind::Dir(FolderState::Expanded) => {
                let child = index.saturating_add(1);
                let is_child = self.rows.get(child).is_some_and(|next| {
                    next.depth == row.depth.saturating_add(1) && next.path.starts_with(&row.path)
                });
                if is_child {
                    self.select_visible(child);
                }
                TreeReply::Handled
            }
            RowKind::File => TreeReply::Handled,
        }
    }

    /// Left: collapses an expanded folder, or steps out to the parent.
    fn left(&mut self) -> TreeReply {
        let Some(index) = self.current() else {
            return TreeReply::Handled;
        };
        let Some(row) = self.rows.get(index) else {
            return TreeReply::Handled;
        };
        if row.kind == RowKind::Dir(FolderState::Expanded) {
            return TreeReply::Send(Command::SetExpanded {
                dir: row.path.clone(),
                state: FolderState::Collapsed,
            });
        }
        if row.depth > 0
            && let Some(parent) = row.path.parent().and_then(|path| self.position(path))
        {
            self.select_visible(parent);
        }
        TreeReply::Handled
    }

    /// Enter: toggles a folder or opens a file.
    fn enter(&self) -> TreeReply {
        let Some(row) = self.current().and_then(|index| self.rows.get(index)) else {
            return TreeReply::Handled;
        };
        let command = match row.kind {
            RowKind::Dir(state) => Command::SetExpanded {
                dir: row.path.clone(),
                state: match state {
                    FolderState::Expanded => FolderState::Collapsed,
                    FolderState::Collapsed => FolderState::Expanded,
                },
            },
            RowKind::File => Command::OpenPath(row.path.clone()),
        };
        TreeReply::Send(command)
    }

    /// The context menu for the selected row, or for the root with none.
    fn open_menu(&self) -> TreeReply {
        let anchor = self.menu_anchor();
        if let Some(row) = self.current().and_then(|index| self.rows.get(index)) {
            return TreeReply::OpenMenu {
                path: row.path.clone(),
                kind: row.kind,
                anchor,
            };
        }
        match &self.root {
            Some(root) => TreeReply::OpenMenu {
                path: root.clone(),
                kind: RowKind::Dir(FolderState::Expanded),
                anchor,
            },
            None => TreeReply::Ignored,
        }
    }

    /// Where a keyboard-opened menu is anchored: the selected row's name
    /// column when that row is drawn, else the first row's left edge.
    #[must_use]
    pub fn menu_anchor(&self) -> Position {
        let fallback = Position::new(self.area.x.saturating_add(1), self.area.y.saturating_add(1));
        let drawn = usize::from(self.area.height.saturating_sub(1));
        let Some((index, row)) = self
            .current()
            .and_then(|index| self.rows.get(index).map(|row| (index, row)))
        else {
            return fallback;
        };
        if index < self.top || index >= self.top.saturating_add(drawn) {
            return fallback;
        }
        let indent = u16::try_from(row.depth.saturating_mul(2)).unwrap_or(u16::MAX);
        let offset = u16::try_from(index.saturating_sub(self.top)).unwrap_or(u16::MAX);
        Position::new(
            fallback
                .x
                .saturating_add(indent)
                .saturating_add(2)
                .saturating_add(ICON_WIDTH),
            fallback.y.saturating_add(offset),
        )
    }

    /// Scrolls the rows by `rows` (down when positive), keeping the last
    /// row at the bottom at most. The selection does not move.
    pub fn wheel(&mut self, rows: i32) {
        let by = usize::try_from(rows.unsigned_abs()).unwrap_or(usize::MAX);
        let top = if rows >= 0 {
            self.top.saturating_add(by)
        } else {
            self.top.saturating_sub(by)
        };
        self.top = top.min(self.rows.len().saturating_sub(self.rows_height()));
    }

    /// Starts dragging the border.
    pub fn begin_resize(&mut self) {
        self.resize = Resize::Dragging;
    }

    /// Stops dragging the border.
    pub fn end_resize(&mut self) {
        self.resize = Resize::Idle;
    }

    /// Whether the border is being dragged.
    #[must_use]
    pub fn is_resizing(&self) -> bool {
        self.resize == Resize::Dragging
    }

    /// While dragging, sets the width so the border sits at `column`.
    pub fn drag_to(&mut self, column: u16) {
        if self.resize == Resize::Dragging {
            self.width = clamp_width(
                column.saturating_sub(self.area.x).saturating_add(1),
                self.total,
            );
        }
    }

    /// Draws the sidebar into `area` and records its hit regions.
    ///
    /// Without a workspace, or with an empty `area`, nothing is drawn.
    /// Returns whether the refresh before drawing changed the rows, in
    /// which case a hovered row index refers to the old rows.
    #[must_use]
    pub fn render(
        &mut self,
        editor: &Editor,
        focus: TreeFocus,
        hovered: Option<Target>,
        area: Rect,
        buf: &mut Buffer,
        hits: &mut HitMap,
    ) -> Refresh {
        self.area = area;
        let refresh = self.refresh(editor.workspace());
        // A hovered row index refers to the rows before a change.
        let hovered = match (refresh, hovered) {
            (Refresh::Changed, Some(Target::TreeRow(_))) => None,
            (_, hovered) => hovered,
        };
        let Some(workspace) = editor.workspace() else {
            return refresh;
        };
        let area = area.intersection(buf.area);
        if area.is_empty() {
            return refresh;
        }
        paint::fill(buf, area, theme::chrome());

        let inner = area.width.saturating_sub(1);
        let border_x = area.x.saturating_add(inner);
        let border = Rect::new(border_x, area.y, 1, area.height);
        let border_style = if hovered == Some(Target::TreeBorder) || self.resize == Resize::Dragging
        {
            theme::tree_border_active()
        } else {
            theme::tree_border()
        };
        paint::fill(buf, border, border_style);
        hits.push(border, Target::TreeBorder);

        if inner >= HEADER_MIN {
            let close_x = area.x.saturating_add(1);
            let mut close_style = theme::close_glyph();
            if hovered == Some(Target::TreeClose) {
                close_style = close_style.bg(theme::hover_bg());
            }
            buf.set_stringn(close_x, area.y, CLOSE_LABEL, 3, close_style);
            hits.push(Rect::new(close_x, area.y, 3, 1), Target::TreeClose);
            let root_x = close_x.saturating_add(4);
            buf.set_stringn(
                root_x,
                area.y,
                workspace.display_root(),
                usize::from(border_x.saturating_sub(root_x)),
                theme::chrome().add_modifier(Modifier::BOLD),
            );
        }

        let rows_y = area.y.saturating_add(1);
        let rows_height = area.height.saturating_sub(1);
        let rows_len = usize::from(rows_height);
        self.top = self.top.min(self.rows.len().saturating_sub(rows_len));

        let active = editor
            .active_tab()
            .and_then(|tab| tab.path.as_ref())
            .and_then(|path| path.canonicalize().ok());

        let mut drawn: u16 = 0;
        for (index, row) in self.rows.iter().enumerate().skip(self.top) {
            if drawn >= rows_height {
                break;
            }
            let y = rows_y.saturating_add(drawn);
            let line = Rect::new(area.x, y, inner, 1);
            let selected = focus == TreeFocus::Focused && self.selected.as_ref() == Some(&row.path);
            let bg = if selected {
                theme::tree_cursor_bg()
            } else if hovered == Some(Target::TreeRow(index)) {
                theme::hover_bg()
            } else {
                theme::color(MANTLE)
            };
            let is_active = row.kind == RowKind::File
                && active.is_some()
                && row.path.canonicalize().ok() == active;
            let base = if is_active {
                theme::tree_active()
            } else {
                theme::chrome()
            };
            draw_row(buf, line, row, self.icons.as_mut(), base.bg(bg));
            hits.push(line, Target::TreeRow(index));
            drawn = drawn.saturating_add(1);
        }

        if drawn < rows_height {
            hits.push(
                Rect::new(
                    area.x,
                    rows_y.saturating_add(drawn),
                    inner,
                    rows_height - drawn,
                ),
                Target::TreeEmpty,
            );
        }
        refresh
    }
}

/// Draws one tree row into `line` (which excludes the border) in
/// `style`; the icon keeps its own foreground.
fn draw_row(
    buf: &mut Buffer,
    line: Rect,
    row: &TreeRow,
    icons: &mut dyn IconProvider,
    style: Style,
) {
    paint::fill(buf, line, style);

    let end = line.right();
    let room = |x: u16| usize::from(end.saturating_sub(x));

    let marker = match row.kind {
        RowKind::Dir(FolderState::Collapsed) => '>',
        RowKind::Dir(FolderState::Expanded) => 'v',
        RowKind::File => ' ',
    };
    let indent = row.depth.saturating_mul(2).min(usize::from(line.width));
    let prefix = format!(" {}{marker} ", " ".repeat(indent));
    let (x, _) = buf.set_stringn(line.x, line.y, prefix, room(line.x), style);

    let name = row
        .path
        .file_name()
        .map_or_else(|| row.path.to_string_lossy(), |name| name.to_string_lossy());
    let kind = match row.kind {
        RowKind::Dir(state) => IconKind::Dir(state),
        RowKind::File => IconKind::File,
    };
    let icon_room = room(x).min(usize::from(ICON_WIDTH));
    if icon_room > 0 {
        match icons.icon(&name, kind) {
            IconCell::Text { text, color } => {
                buf.set_stringn(x, line.y, text, icon_room, style.fg(theme::color(color)));
            }
            IconCell::Image { id } => {
                // Kitty reserves every attribute but fg and bg on
                // placeholder cells, so keep only the row's background.
                // `set_style` only adds modifiers, hence the reset first.
                let mut icon_style = Style::new().fg(id.color());
                if let Some(bg) = style.bg {
                    icon_style = icon_style.bg(bg);
                }
                for (offset, symbol) in (0u16..).zip(PLACEHOLDER_CELLS.iter().take(icon_room)) {
                    if let Some(cell) =
                        buf.cell_mut(Position::new(x.saturating_add(offset), line.y))
                    {
                        cell.reset();
                        cell.set_symbol(symbol).set_style(icon_style);
                    }
                }
            }
        }
    }

    let name_x = x.saturating_add(ICON_WIDTH);
    if name_x < end {
        buf.set_stringn(name_x, line.y, name, room(name_x), style);
    }
}

#[cfg(test)]
mod tests {
    // `unwrap` is acceptable in test code: a panic on an unexpected `Err`
    // is exactly the failure signal we want from a test.
    #![allow(clippy::unwrap_used)]

    use std::fs;

    use frext_core::clipboard::MemoryClipboard;
    use frext_core::editor::Command;
    use frext_core::persistence::Store;
    use ratatui::style::Color;

    use super::*;

    /// A temp directory, removed (ignoring errors) when the guard drops.
    struct TempDir(PathBuf);

    impl TempDir {
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn temp_dir(tag: &str) -> TempDir {
        let dir = std::env::temp_dir().join(format!(
            "frext-term-tree-test-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    fn open_editor(dir: &Path, files: &[PathBuf], workspace: Option<&Path>) -> Editor {
        Editor::open(
            Store::at(dir.join("state")).unwrap(),
            Box::new(MemoryClipboard::default()),
            files,
            workspace,
        )
    }

    /// A project with `src/main.rs`, `docs/`, `Cargo.toml` and
    /// `README.md`, in its own temp directory. Returns `(dir, project)`.
    fn project(tag: &str) -> (TempDir, PathBuf) {
        let dir = temp_dir(tag);
        let project = dir.path().join("project");
        fs::create_dir_all(project.join("src")).unwrap();
        fs::create_dir_all(project.join("docs")).unwrap();
        fs::write(project.join("src").join("main.rs"), "fn main() {}").unwrap();
        fs::write(project.join("Cargo.toml"), "[package]").unwrap();
        fs::write(project.join("README.md"), "# readme").unwrap();
        (dir, project)
    }

    /// An editor over the project fixture, with the project as workspace.
    /// Returns `(dir, project, editor)`; the test holds `dir` throughout.
    fn fixture(tag: &str, files: &[&str]) -> (TempDir, PathBuf, Editor) {
        let (dir, project) = project(tag);
        let files: Vec<PathBuf> = files.iter().map(|name| project.join(name)).collect();
        let editor = open_editor(dir.path(), &files, Some(&project));
        (dir, project, editor)
    }

    const SIDEBAR: Rect = Rect::new(0, 0, 30, 8);
    const SCREEN: Rect = Rect::new(0, 0, 50, 8);

    fn render(
        tree: &mut FileTree,
        editor: &Editor,
        focus: TreeFocus,
        hovered: Option<Target>,
        area: Rect,
    ) -> (Buffer, HitMap) {
        let mut buf = Buffer::empty(SCREEN);
        let mut hits = HitMap::default();
        let _refresh = tree.render(editor, focus, hovered, area, &mut buf, &mut hits);
        (buf, hits)
    }

    /// The text of row `y` from column `x` for `width` cells, trailing
    /// blanks removed.
    fn text(buf: &Buffer, y: u16, x: u16, width: u16) -> String {
        let line: String = (x..x + width).map(|x| buf[(x, y)].symbol()).collect();
        line.trim_end().to_owned()
    }

    /// The text of sidebar row `y`, left of the default border.
    fn row_text(buf: &Buffer, y: u16) -> String {
        text(buf, y, 0, SIDEBAR.width - 1)
    }

    fn mantle() -> Option<Color> {
        theme::chrome().bg
    }

    #[test]
    fn clamp_width_table() {
        let cases: [(u16, [u16; 4]); 5] = [
            (0, [12, 12, 12, 12]),
            (20, [12, 12, 12, 12]),
            (32, [12, 12, 12, 12]),
            (80, [12, 12, 30, 60]),
            (200, [12, 12, 30, 180]),
        ];
        for (total, expected) in cases {
            for (width, want) in [0, 12, 30, 500].into_iter().zip(expected) {
                assert_eq!(clamp_width(width, total), want, "{width} in {total}");
            }
        }
    }

    #[test]
    fn new_tree_defaults() {
        let tree = FileTree::default();
        assert_eq!(tree.width(), DEFAULT_WIDTH);
        assert_eq!(tree.effective_width(200), DEFAULT_WIDTH);
        assert_eq!(tree.effective_width(40), 20);
        assert_eq!(tree.effective_width(10), MIN_WIDTH);
        assert!(tree.rows().is_empty());
        assert_eq!(tree.row(0), None);
        assert_eq!(tree.selected(), None);
        assert_eq!(tree.root(), None);
        assert_eq!(tree.total, 0);
        assert_eq!(tree.area, Rect::default());
        assert_eq!(tree.resize, Resize::Idle);
    }

    #[test]
    fn render_stores_the_area_but_not_the_space() {
        let (_dir, _, editor) = fixture("stores", &[]);
        let mut tree = FileTree::new();
        render(&mut tree, &editor, TreeFocus::Unfocused, None, SIDEBAR);
        assert_eq!(tree.area, SIDEBAR);
        // Only `set_space` sets the space and clamps the width.
        assert_eq!(tree.total, 0);
        assert_eq!(tree.width(), DEFAULT_WIDTH);
        assert_eq!(tree.root(), editor.workspace().map(|ws| ws.root.as_path()));
        assert_eq!(tree.rows().len(), 4);
        assert_eq!(
            tree.row(1)
                .map(|row| row.path.file_name().unwrap().to_owned()),
            Some("src".into())
        );
    }

    #[test]
    fn header_shows_close_and_root() {
        let (_dir, _, editor) = fixture("header", &[]);
        let mut tree = FileTree::new();
        let (buf, hits) = render(&mut tree, &editor, TreeFocus::Unfocused, None, SIDEBAR);
        let root = editor.workspace().unwrap().display_root();
        let expected: String = format!(" [x] {root}").chars().take(29).collect();
        assert_eq!(row_text(&buf, 0), expected.trim_end());
        assert_eq!(buf[(1, 0)].fg, theme::close_glyph().fg.unwrap());
        assert!(buf[(5, 0)].modifier.contains(Modifier::BOLD));
        assert_eq!(hits.hit(0, 0), None);
        for x in 1..4 {
            assert_eq!(hits.hit(x, 0), Some(Target::TreeClose), "{x}");
        }
        assert_eq!(hits.hit(4, 0), None);

        let (buf, _) = render(
            &mut tree,
            &editor,
            TreeFocus::Unfocused,
            Some(Target::TreeClose),
            SIDEBAR,
        );
        assert_eq!(Some(buf[(2, 0)].bg), Some(theme::hover_bg()));
        assert_eq!(Some(buf[(4, 0)].bg), mantle());
    }

    #[test]
    fn narrow_header_is_blank() {
        let (_dir, _, editor) = fixture("narrow-header", &[]);
        let mut tree = FileTree::new();
        let (buf, hits) = render(
            &mut tree,
            &editor,
            TreeFocus::Unfocused,
            None,
            Rect::new(0, 0, 4, 8),
        );
        assert_eq!(text(&buf, 0, 0, 3), "");
        assert_eq!(hits.hit(1, 0), None);
        assert_eq!(hits.hit(3, 0), Some(Target::TreeBorder));
    }

    #[test]
    fn collapsed_and_expanded_rows() {
        let (_dir, project, mut editor) = fixture("rows", &[]);
        let mut tree = FileTree::new();
        let (buf, _) = render(&mut tree, &editor, TreeFocus::Unfocused, None, SIDEBAR);
        let rows: Vec<String> = (1..5).map(|y| row_text(&buf, y)).collect();
        assert_eq!(
            rows,
            [" > / docs", " > / src", "     Cargo.toml", "     README.md"]
        );
        assert_eq!(buf[(3, 1)].fg, theme::color(frext_core::palette::BLUE));

        editor.apply(Command::SetExpanded {
            dir: project.join("src"),
            state: FolderState::Expanded,
        });
        let (buf, _) = render(&mut tree, &editor, TreeFocus::Unfocused, None, SIDEBAR);
        assert_eq!(row_text(&buf, 2), " v / src");
        assert_eq!(row_text(&buf, 3), "       main.rs");
        assert_eq!(row_text(&buf, 4), "     Cargo.toml");
    }

    #[test]
    fn border_column() {
        let (_dir, _, editor) = fixture("border", &[]);
        let mut tree = FileTree::new();
        let (buf, hits) = render(&mut tree, &editor, TreeFocus::Unfocused, None, SIDEBAR);
        for y in 0..SIDEBAR.height {
            assert_eq!(buf[(29, y)].symbol(), " ");
            assert_eq!(Some(buf[(29, y)].bg), theme::tree_border().bg, "{y}");
            assert_eq!(hits.hit(29, y), Some(Target::TreeBorder), "{y}");
            assert_eq!(hits.hit(30, y), None, "{y}");
        }

        let (buf, _) = render(
            &mut tree,
            &editor,
            TreeFocus::Unfocused,
            Some(Target::TreeBorder),
            SIDEBAR,
        );
        for y in 0..SIDEBAR.height {
            assert_eq!(Some(buf[(29, y)].bg), theme::tree_border_active().bg, "{y}");
        }

        tree.resize = Resize::Dragging;
        let (buf, _) = render(&mut tree, &editor, TreeFocus::Unfocused, None, SIDEBAR);
        assert_eq!(Some(buf[(29, 3)].bg), theme::tree_border_active().bg);
    }

    #[test]
    fn row_and_empty_regions() {
        let (_dir, _, editor) = fixture("regions", &[]);
        let mut tree = FileTree::new();
        let (_, hits) = render(&mut tree, &editor, TreeFocus::Unfocused, None, SIDEBAR);
        for index in 0..4u16 {
            let y = index + 1;
            assert_eq!(hits.hit(0, y), Some(Target::TreeRow(usize::from(index))));
            assert_eq!(hits.hit(28, y), Some(Target::TreeRow(usize::from(index))));
        }
        for y in 5..SIDEBAR.height {
            assert_eq!(hits.hit(0, y), Some(Target::TreeEmpty), "{y}");
            assert_eq!(hits.hit(28, y), Some(Target::TreeEmpty), "{y}");
            assert_eq!(hits.hit(29, y), Some(Target::TreeBorder), "{y}");
        }
    }

    #[test]
    fn full_sidebar_has_no_empty_region() {
        let (_dir, _, editor) = fixture("full", &[]);
        let mut tree = FileTree::new();
        let area = Rect::new(0, 0, 30, 5);
        let (_, hits) = render(&mut tree, &editor, TreeFocus::Unfocused, None, area);
        assert_eq!(hits.hit(0, 4), Some(Target::TreeRow(3)));
        assert_eq!(hits.hit(0, 5), None);
    }

    #[test]
    fn active_selected_and_hovered_styles() {
        let (_dir, _, editor) = fixture("styles", &["Cargo.toml"]);
        let mut tree = FileTree::new();
        let (buf, _) = render(&mut tree, &editor, TreeFocus::Unfocused, None, SIDEBAR);
        // Cargo.toml is row 2, drawn at y = 3; its name starts at x = 5.
        assert_eq!(Some(buf[(5, 3)].fg), theme::tree_active().fg);
        assert!(buf[(5, 3)].modifier.contains(Modifier::BOLD));
        assert_eq!(Some(buf[(5, 4)].fg), theme::chrome().fg);
        assert_eq!(Some(buf[(5, 1)].fg), theme::chrome().fg);

        tree.select(3);
        let (buf, _) = render(&mut tree, &editor, TreeFocus::Unfocused, None, SIDEBAR);
        assert_eq!(Some(buf[(0, 4)].bg), mantle());

        let (buf, _) = render(&mut tree, &editor, TreeFocus::Focused, None, SIDEBAR);
        assert_eq!(buf[(0, 4)].bg, theme::tree_cursor_bg());
        assert_eq!(buf[(28, 4)].bg, theme::tree_cursor_bg());
        assert_eq!(Some(buf[(0, 3)].bg), mantle());

        let (buf, _) = render(
            &mut tree,
            &editor,
            TreeFocus::Focused,
            Some(Target::TreeRow(1)),
            SIDEBAR,
        );
        assert_eq!(buf[(0, 2)].bg, theme::hover_bg());
        // The folder icon keeps its colour over the hover background.
        assert_eq!(buf[(3, 2)].fg, theme::color(frext_core::palette::BLUE));
        assert_eq!(buf[(3, 2)].bg, theme::hover_bg());
        assert_eq!(buf[(0, 4)].bg, theme::tree_cursor_bg());
    }

    #[test]
    fn long_name_is_cut_at_the_border() {
        let (_dir, project, editor) = fixture("long", &[]);
        let name = "a-really-long-file-name-that-overflows.txt";
        fs::write(project.join(name), "x").unwrap();
        let mut tree = FileTree::new();
        let area = Rect::new(0, 0, 20, 8);
        let (buf, hits) = render(&mut tree, &editor, TreeFocus::Unfocused, None, area);
        let index = tree
            .rows()
            .iter()
            .position(|row| row.path == project.join(name))
            .unwrap();
        let y = u16::try_from(index).unwrap() + 1;
        let expected: String = format!("     {name}").chars().take(19).collect();
        assert_eq!(text(&buf, y, 0, 19), expected);
        assert_eq!(buf[(19, y)].symbol(), " ");
        assert_eq!(Some(buf[(19, y)].bg), theme::tree_border().bg);
        assert_eq!(hits.hit(19, y), Some(Target::TreeBorder));
        assert_eq!(buf[(20, y)].symbol(), " ");
        assert_eq!(buf[(20, y)].bg, Color::Reset);
    }

    #[test]
    fn wide_name_is_never_split() {
        let (_dir, project, editor) = fixture("wide", &[]);
        // "     " + 8 wide chars in 20 cells: the 8th would straddle
        // columns 19 and 20, the border, so it is dropped whole.
        let name = "\u{65e5}".repeat(8);
        fs::write(project.join(&name), "x").unwrap();
        let mut tree = FileTree::new();
        let area = Rect::new(0, 0, 21, 8);
        let (buf, _) = render(&mut tree, &editor, TreeFocus::Unfocused, None, area);
        let index = tree
            .rows()
            .iter()
            .position(|row| row.path == project.join(&name))
            .unwrap();
        let y = u16::try_from(index).unwrap() + 1;
        assert_eq!(buf[(17, y)].symbol(), "\u{65e5}");
        assert_eq!(buf[(19, y)].symbol(), " ");
        assert_eq!(Some(buf[(19, y)].bg), mantle());
        assert_eq!(buf[(20, y)].symbol(), " ");
        assert_eq!(Some(buf[(20, y)].bg), theme::tree_border().bg);
    }

    #[test]
    fn short_sidebar_scrolls_from_top() {
        let (_dir, _, editor) = fixture("scroll", &[]);
        let mut tree = FileTree::new();
        let area = Rect::new(0, 0, 30, 3);
        render(&mut tree, &editor, TreeFocus::Unfocused, None, area);
        tree.top = 1;
        let (buf, hits) = render(&mut tree, &editor, TreeFocus::Unfocused, None, area);
        assert_eq!(row_text(&buf, 1), " > / src");
        assert_eq!(row_text(&buf, 2), "     Cargo.toml");
        assert_eq!(hits.hit(0, 1), Some(Target::TreeRow(1)));
        assert_eq!(hits.hit(0, 2), Some(Target::TreeRow(2)));

        tree.top = 10;
        let (buf, hits) = render(&mut tree, &editor, TreeFocus::Unfocused, None, area);
        assert_eq!(tree.top, 2);
        assert_eq!(row_text(&buf, 1), "     Cargo.toml");
        assert_eq!(row_text(&buf, 2), "     README.md");
        assert_eq!(hits.hit(0, 2), Some(Target::TreeRow(3)));
    }

    #[test]
    fn refresh_unchanged_then_changed_on_new_file() {
        let (_dir, project, editor) = fixture("refresh", &[]);
        let mut tree = FileTree::new();
        assert_eq!(tree.refresh(editor.workspace()), Refresh::Changed);
        assert_eq!(tree.refresh(editor.workspace()), Refresh::Unchanged);
        fs::write(project.join("new.txt"), "").unwrap();
        assert_eq!(tree.refresh(editor.workspace()), Refresh::Changed);
        assert_eq!(tree.rows().len(), 5);
    }

    #[test]
    fn select_ignores_missing_rows() {
        let (_dir, project, editor) = fixture("select", &[]);
        let mut tree = FileTree::new();
        tree.refresh(editor.workspace());
        tree.select(9);
        assert_eq!(tree.selected(), None);
        tree.select(2);
        let cargo = project.join("Cargo.toml");
        assert_eq!(tree.selected(), Some(cargo.as_path()));
        tree.select(9);
        assert_eq!(tree.selected(), Some(cargo.as_path()));
    }

    #[test]
    fn selection_follows_its_path_when_a_folder_above_expands() {
        let (_dir, project, mut editor) = fixture("follow", &[]);
        let mut tree = FileTree::new();
        tree.refresh(editor.workspace());
        tree.select(3);
        editor.apply(Command::SetExpanded {
            dir: project.join("src"),
            state: FolderState::Expanded,
        });
        assert_eq!(tree.refresh(editor.workspace()), Refresh::Changed);
        let readme = project.join("README.md");
        assert_eq!(tree.selected(), Some(readme.as_path()));
        assert_eq!(tree.selected_index, 4);
    }

    #[test]
    fn selection_moves_to_the_collapsed_parent() {
        let (_dir, project, mut editor) = fixture("parent", &[]);
        let src = project.join("src");
        editor.apply(Command::SetExpanded {
            dir: src.clone(),
            state: FolderState::Expanded,
        });
        let mut tree = FileTree::new();
        tree.refresh(editor.workspace());
        tree.select(2);
        assert_eq!(tree.selected(), Some(src.join("main.rs").as_path()));
        editor.apply(Command::SetExpanded {
            dir: src.clone(),
            state: FolderState::Collapsed,
        });
        tree.refresh(editor.workspace());
        assert_eq!(tree.selected(), Some(src.as_path()));
        assert_eq!(tree.selected_index, 1);
    }

    #[test]
    fn selection_falls_back_to_the_clamped_index() {
        let (_dir, project, editor) = fixture("deleted", &[]);
        let mut tree = FileTree::new();
        tree.refresh(editor.workspace());
        tree.select(3);
        fs::remove_file(project.join("README.md")).unwrap();
        tree.refresh(editor.workspace());
        let cargo = project.join("Cargo.toml");
        assert_eq!(tree.selected(), Some(cargo.as_path()));
        assert_eq!(tree.selected_index, 2);
    }

    #[test]
    fn selection_is_none_without_a_workspace() {
        let (_dir, _, editor) = fixture("none", &[]);
        let mut tree = FileTree::new();
        tree.refresh(editor.workspace());
        tree.select(1);
        tree.top = 2;
        assert_eq!(tree.refresh(None), Refresh::Changed);
        assert_eq!(tree.selected(), None);
        assert_eq!(tree.root(), None);
        assert!(tree.rows().is_empty());
        assert_eq!(tree.top, 0);
    }

    #[test]
    fn nothing_selected_stays_none() {
        let (_dir, project, editor) = fixture("unselected", &[]);
        let mut tree = FileTree::new();
        tree.refresh(editor.workspace());
        fs::write(project.join("new.txt"), "").unwrap();
        tree.refresh(editor.workspace());
        assert_eq!(tree.selected(), None);
    }

    #[test]
    fn no_workspace_draws_nothing() {
        let dir = temp_dir("no-workspace");
        let editor = open_editor(dir.path(), &[], None);
        let mut tree = FileTree::new();
        let (buf, hits) = render(&mut tree, &editor, TreeFocus::Focused, None, SIDEBAR);
        assert_eq!(buf, Buffer::empty(SCREEN));
        for (x, y) in [(0, 0), (2, 0), (0, 1), (29, 3), (10, 7)] {
            assert_eq!(hits.hit(x, y), None);
        }
    }

    /// A provider that shows image `0x010203` for every row and hands
    /// back fixed upload and release bytes.
    #[derive(Debug)]
    struct FakeIcons {
        uploads: Vec<u8>,
        release: Vec<u8>,
    }

    impl IconProvider for FakeIcons {
        fn icon(&mut self, _name: &str, _kind: IconKind) -> IconCell {
            IconCell::Image {
                id: crate::term::kitty::ImageId::new(0x01_0203).unwrap(),
            }
        }

        fn take_uploads(&mut self) -> Vec<u8> {
            std::mem::take(&mut self.uploads)
        }

        fn release(&mut self) -> Vec<u8> {
            std::mem::take(&mut self.release)
        }
    }

    fn fake_icons() -> Box<FakeIcons> {
        Box::new(FakeIcons {
            uploads: b"up".to_vec(),
            release: b"del".to_vec(),
        })
    }

    #[test]
    fn image_icons_draw_both_placeholder_cells() {
        let (_dir, _, editor) = fixture("image", &[]);
        let mut tree = FileTree::new();
        tree.set_icons(fake_icons());
        let (buf, _) = render(&mut tree, &editor, TreeFocus::Unfocused, None, SIDEBAR);
        for y in 1..5 {
            assert_eq!(buf[(3, y)].symbol(), PLACEHOLDER_CELLS[0], "{y}");
            assert_eq!(buf[(4, y)].symbol(), PLACEHOLDER_CELLS[1], "{y}");
            assert_eq!(buf[(3, y)].fg, Color::Rgb(1, 2, 3), "{y}");
            assert_eq!(buf[(4, y)].fg, Color::Rgb(1, 2, 3), "{y}");
            assert_eq!(Some(buf[(3, y)].bg), mantle(), "{y}");
        }
        assert_eq!(
            row_text(&buf, 3),
            format!(
                "   {}{}Cargo.toml",
                PLACEHOLDER_CELLS[0], PLACEHOLDER_CELLS[1]
            )
        );
    }

    #[test]
    fn image_icons_keep_the_selected_rows_background() {
        let (_dir, _, editor) = fixture("image-selected", &[]);
        let mut tree = FileTree::new();
        tree.set_icons(fake_icons());
        render(&mut tree, &editor, TreeFocus::Focused, None, SIDEBAR);
        tree.select(2);
        let (buf, _) = render(&mut tree, &editor, TreeFocus::Focused, None, SIDEBAR);
        for x in [3, 4] {
            assert_eq!(buf[(x, 3)].bg, theme::tree_cursor_bg(), "{x}");
            assert_eq!(buf[(x, 3)].fg, Color::Rgb(1, 2, 3), "{x}");
        }
        assert_eq!(buf[(3, 3)].symbol(), PLACEHOLDER_CELLS[0]);
        assert_eq!(buf[(4, 3)].symbol(), PLACEHOLDER_CELLS[1]);
    }

    #[test]
    fn image_icon_in_a_one_cell_slot_draws_only_the_first_placeholder() {
        let (_dir, _, editor) = fixture("image-narrow", &[]);
        let mut tree = FileTree::new();
        tree.set_icons(fake_icons());
        // Four cells left of the border: the prefix takes three, so one
        // slot cell fits.
        let (buf, _) = render(
            &mut tree,
            &editor,
            TreeFocus::Unfocused,
            None,
            Rect::new(0, 0, 5, 8),
        );
        for y in 1..5 {
            assert_eq!(buf[(3, y)].symbol(), PLACEHOLDER_CELLS[0], "{y}");
            assert_eq!(buf[(3, y)].fg, Color::Rgb(1, 2, 3), "{y}");
            assert_eq!(buf[(4, y)].symbol(), " ", "{y}");
            assert_eq!(Some(buf[(4, y)].bg), theme::tree_border().bg, "{y}");
            assert_eq!(buf[(5, y)], Buffer::empty(SCREEN)[(5, y)], "{y}");
        }
    }

    #[test]
    fn image_placeholders_on_the_active_row_carry_only_fg_and_bg() {
        let (_dir, _, editor) = fixture("image-active", &["Cargo.toml"]);
        let mut tree = FileTree::new();
        tree.set_icons(fake_icons());
        let (buf, _) = render(&mut tree, &editor, TreeFocus::Unfocused, None, SIDEBAR);
        // Cargo.toml is row 2, drawn at y = 3.
        assert!(theme::tree_active().add_modifier.contains(Modifier::BOLD));
        for x in [3, 4] {
            let cell = &buf[(x, 3)];
            assert_eq!(cell.modifier, Modifier::empty(), "{x}");
            assert_eq!(cell.underline_color, Color::Reset, "{x}");
            assert_eq!(cell.fg, Color::Rgb(1, 2, 3), "{x}");
            assert_eq!(Some(cell.bg), mantle(), "{x}");
        }
        assert_eq!(buf[(3, 3)].symbol(), PLACEHOLDER_CELLS[0]);
        assert_eq!(buf[(4, 3)].symbol(), PLACEHOLDER_CELLS[1]);
        assert!(buf[(5, 3)].modifier.contains(Modifier::BOLD));
        assert_eq!(Some(buf[(5, 3)].fg), theme::tree_active().fg);
    }

    /// A provider that counts how often it is asked for an icon.
    #[derive(Debug)]
    struct CountingIcons {
        calls: std::rc::Rc<std::cell::Cell<usize>>,
    }

    impl IconProvider for CountingIcons {
        fn icon(&mut self, _name: &str, _kind: IconKind) -> IconCell {
            self.calls.set(self.calls.get() + 1);
            IconCell::Image {
                id: crate::term::kitty::ImageId::new(0x01_0203).unwrap(),
            }
        }

        fn take_uploads(&mut self) -> Vec<u8> {
            Vec::new()
        }

        fn release(&mut self) -> Vec<u8> {
            Vec::new()
        }
    }

    #[test]
    fn a_row_without_icon_room_never_asks_the_provider() {
        let (_dir, _, editor) = fixture("image-no-room", &[]);
        let calls = std::rc::Rc::new(std::cell::Cell::new(0));
        let mut tree = FileTree::new();
        tree.set_icons(Box::new(CountingIcons {
            calls: std::rc::Rc::clone(&calls),
        }));
        // Three cells left of the border: the prefix fills them all.
        let (buf, _) = render(
            &mut tree,
            &editor,
            TreeFocus::Unfocused,
            None,
            Rect::new(0, 0, 4, 8),
        );
        assert_eq!(buf[(2, 1)].symbol(), " ");
        assert_eq!(calls.get(), 0);

        // With one cell of room the provider is asked once per row.
        render(
            &mut tree,
            &editor,
            TreeFocus::Unfocused,
            None,
            Rect::new(0, 0, 5, 8),
        );
        assert_eq!(calls.get(), 4);
    }

    #[test]
    fn uploads_and_release_pass_through_the_provider() {
        let mut tree = FileTree::new();
        assert!(tree.take_uploads().is_empty());
        assert!(tree.release_icons().is_empty());
        tree.set_icons(fake_icons());
        assert_eq!(tree.take_uploads(), b"up");
        assert_eq!(tree.take_uploads(), b"");
        assert_eq!(tree.release_icons(), b"del");
        assert_eq!(tree.release_icons(), b"");
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn key_with(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    /// Gives `tree` the whole `screen` width as its space, as the app
    /// does with no gutter, then renders it once into a `screen`-sized
    /// buffer at `area`.
    fn render_in(tree: &mut FileTree, editor: &Editor, area: Rect, screen: Rect) -> Refresh {
        tree.set_space(screen.width);
        let mut buf = Buffer::empty(screen);
        let mut hits = HitMap::default();
        tree.render(editor, TreeFocus::Focused, None, area, &mut buf, &mut hits)
    }

    /// A tree rendered once over the fixture in `area`, with `SCREEN`'s
    /// width as its space. Returns `(dir, project, editor, tree)`.
    fn keyed(tag: &str, area: Rect) -> (TempDir, PathBuf, Editor, FileTree) {
        let (dir, project, editor) = fixture(tag, &[]);
        let mut tree = FileTree::new();
        render_in(&mut tree, &editor, area, SCREEN);
        (dir, project, editor, tree)
    }

    fn selected_index(tree: &FileTree) -> Option<usize> {
        tree.selected().and_then(|path| tree.position(path))
    }

    #[test]
    fn down_from_nothing_selects_the_first_row() {
        let (_dir, _, _, mut tree) = keyed("down-first", SIDEBAR);
        assert_eq!(tree.handle_key(key(KeyCode::Down)), TreeReply::Handled);
        assert_eq!(selected_index(&tree), Some(0));
    }

    #[test]
    fn end_from_nothing_selects_the_last_row() {
        let (_dir, _, _, mut tree) = keyed("end-first", SIDEBAR);
        assert_eq!(tree.handle_key(key(KeyCode::End)), TreeReply::Handled);
        assert_eq!(selected_index(&tree), Some(3));
    }

    #[test]
    fn up_and_down_step_and_stop_at_the_ends() {
        let (_dir, _, _, mut tree) = keyed("step", SIDEBAR);
        tree.handle_key(key(KeyCode::Down));
        for want in [1, 2, 3, 3] {
            assert_eq!(tree.handle_key(key(KeyCode::Down)), TreeReply::Handled);
            assert_eq!(selected_index(&tree), Some(want));
        }
        for want in [2, 1, 0, 0] {
            assert_eq!(tree.handle_key(key(KeyCode::Up)), TreeReply::Handled);
            assert_eq!(selected_index(&tree), Some(want));
        }
    }

    #[test]
    fn home_and_end() {
        let (_dir, _, _, mut tree) = keyed("home-end", SIDEBAR);
        tree.select(1);
        assert_eq!(tree.handle_key(key(KeyCode::End)), TreeReply::Handled);
        assert_eq!(selected_index(&tree), Some(3));
        assert_eq!(tree.handle_key(key(KeyCode::Home)), TreeReply::Handled);
        assert_eq!(selected_index(&tree), Some(0));
    }

    #[test]
    fn motion_keys_with_no_rows_are_handled() {
        let mut tree = FileTree::new();
        for code in [
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::PageUp,
            KeyCode::PageDown,
            KeyCode::Home,
            KeyCode::End,
        ] {
            assert_eq!(tree.handle_key(key(code)), TreeReply::Handled, "{code:?}");
        }
        assert_eq!(tree.selected(), None);
    }

    #[test]
    fn page_keys_move_by_rows_height_minus_one() {
        let (_dir, project, editor) = fixture("page", &[]);
        for name in ["w1.txt", "w2.txt", "w3.txt", "w4.txt"] {
            fs::write(project.join(name), "").unwrap();
        }
        let mut tree = FileTree::new();
        // Four rows shown, so a page is three rows.
        render_in(&mut tree, &editor, Rect::new(0, 0, 30, 5), SCREEN);
        assert_eq!(tree.rows().len(), 8);
        tree.select(0);
        for want in [3, 6, 7] {
            assert_eq!(tree.handle_key(key(KeyCode::PageDown)), TreeReply::Handled);
            assert_eq!(selected_index(&tree), Some(want));
        }
        for want in [4, 1, 0] {
            assert_eq!(tree.handle_key(key(KeyCode::PageUp)), TreeReply::Handled);
            assert_eq!(selected_index(&tree), Some(want));
        }
    }

    #[test]
    fn page_keys_move_by_one_in_a_one_row_sidebar() {
        let (_dir, _, _, mut tree) = keyed("page-one", Rect::new(0, 0, 30, 2));
        tree.select(0);
        tree.handle_key(key(KeyCode::PageDown));
        assert_eq!(selected_index(&tree), Some(1));
        tree.handle_key(key(KeyCode::PageUp));
        assert_eq!(selected_index(&tree), Some(0));
    }

    #[test]
    fn selection_scrolls_into_view_in_a_short_sidebar() {
        let area = Rect::new(0, 0, 30, 3);
        let (_dir, project, editor, mut tree) = keyed("into-view", area);
        for _ in 0..3 {
            tree.handle_key(key(KeyCode::Down));
        }
        assert_eq!(selected_index(&tree), Some(2));
        assert_eq!(tree.top, 1);
        tree.handle_key(key(KeyCode::End));
        assert_eq!(tree.top, 2);
        let mut buf = Buffer::empty(SCREEN);
        let mut hits = HitMap::default();
        let refresh = tree.render(&editor, TreeFocus::Focused, None, area, &mut buf, &mut hits);
        assert_eq!(refresh, Refresh::Unchanged);
        assert_eq!(row_text(&buf, 2), "     README.md");
        assert_eq!(buf[(0, 2)].bg, theme::tree_cursor_bg());
        assert_eq!(tree.selected(), Some(project.join("README.md").as_path()));

        tree.handle_key(key(KeyCode::Home));
        assert_eq!(tree.top, 0);
    }

    #[test]
    fn right_expands_then_steps_into_a_folder() {
        let (_dir, project, mut editor, mut tree) = keyed("right", SIDEBAR);
        let src = project.join("src");
        tree.select(1);
        let reply = tree.handle_key(key(KeyCode::Right));
        let expand = Command::SetExpanded {
            dir: src.clone(),
            state: FolderState::Expanded,
        };
        assert_eq!(reply, TreeReply::Send(expand.clone()));
        assert_eq!(tree.selected(), Some(src.as_path()));

        editor.apply(expand);
        tree.refresh(editor.workspace());
        assert_eq!(tree.handle_key(key(KeyCode::Right)), TreeReply::Handled);
        assert_eq!(tree.selected(), Some(src.join("main.rs").as_path()));
    }

    #[test]
    fn right_on_a_file_or_an_empty_expanded_folder_does_nothing() {
        let (_dir, project, mut editor, mut tree) = keyed("right-file", SIDEBAR);
        let cargo = project.join("Cargo.toml");
        tree.select(2);
        assert_eq!(tree.handle_key(key(KeyCode::Right)), TreeReply::Handled);
        assert_eq!(tree.selected(), Some(cargo.as_path()));

        let docs = project.join("docs");
        editor.apply(Command::SetExpanded {
            dir: docs.clone(),
            state: FolderState::Expanded,
        });
        tree.refresh(editor.workspace());
        tree.select(0);
        assert_eq!(tree.handle_key(key(KeyCode::Right)), TreeReply::Handled);
        assert_eq!(tree.selected(), Some(docs.as_path()));
    }

    #[test]
    fn left_collapses_then_steps_out() {
        let (_dir, project, mut editor, mut tree) = keyed("left", SIDEBAR);
        let src = project.join("src");
        editor.apply(Command::SetExpanded {
            dir: src.clone(),
            state: FolderState::Expanded,
        });
        tree.refresh(editor.workspace());
        tree.select(1);
        assert_eq!(
            tree.handle_key(key(KeyCode::Left)),
            TreeReply::Send(Command::SetExpanded {
                dir: src.clone(),
                state: FolderState::Collapsed,
            })
        );

        tree.select(2);
        assert_eq!(tree.selected(), Some(src.join("main.rs").as_path()));
        assert_eq!(tree.handle_key(key(KeyCode::Left)), TreeReply::Handled);
        assert_eq!(tree.selected(), Some(src.as_path()));
    }

    #[test]
    fn left_at_depth_zero_changes_nothing() {
        let (_dir, project, _, mut tree) = keyed("left-top", SIDEBAR);
        for (index, name) in [(0, "docs"), (3, "README.md")] {
            tree.select(index);
            let width = tree.width();
            let top = tree.top;
            assert_eq!(tree.handle_key(key(KeyCode::Left)), TreeReply::Handled);
            assert_eq!(tree.selected(), Some(project.join(name).as_path()));
            assert_eq!((tree.width(), tree.top), (width, top));
        }
    }

    #[test]
    fn enter_toggles_a_folder_and_opens_a_file() {
        let (_dir, project, mut editor, mut tree) = keyed("enter", SIDEBAR);
        let src = project.join("src");
        tree.select(1);
        let expand = Command::SetExpanded {
            dir: src.clone(),
            state: FolderState::Expanded,
        };
        assert_eq!(
            tree.handle_key(key(KeyCode::Enter)),
            TreeReply::Send(expand.clone())
        );
        editor.apply(expand);
        tree.refresh(editor.workspace());
        assert_eq!(
            tree.handle_key(key(KeyCode::Enter)),
            TreeReply::Send(Command::SetExpanded {
                dir: src.clone(),
                state: FolderState::Collapsed,
            })
        );

        tree.select(2);
        assert_eq!(
            tree.handle_key(key(KeyCode::Enter)),
            TreeReply::Send(Command::OpenPath(src.join("main.rs")))
        );
    }

    #[test]
    fn right_left_enter_with_nothing_selected_are_handled() {
        let (_dir, _, _, mut tree) = keyed("unselected-keys", SIDEBAR);
        for code in [KeyCode::Right, KeyCode::Left, KeyCode::Enter] {
            assert_eq!(tree.handle_key(key(code)), TreeReply::Handled, "{code:?}");
        }
        assert_eq!(tree.selected(), None);
    }

    #[test]
    fn menu_keys_open_the_selected_rows_menu() {
        let (_dir, project, _, mut tree) = keyed("menu", SIDEBAR);
        let menu_keys = [
            key_with(KeyCode::F(10), KeyModifiers::SHIFT),
            key(KeyCode::F(22)),
            key_with(KeyCode::F(22), KeyModifiers::SHIFT),
            key(KeyCode::Menu),
            key_with(KeyCode::Menu, KeyModifiers::SHIFT),
        ];
        for (index, name, kind, anchor) in [
            (2, "Cargo.toml", RowKind::File, Position::new(5, 3)),
            (
                1,
                "src",
                RowKind::Dir(FolderState::Collapsed),
                Position::new(5, 2),
            ),
        ] {
            tree.select(index);
            assert_eq!(tree.menu_anchor(), anchor);
            for event in menu_keys {
                assert_eq!(
                    tree.handle_key(event),
                    TreeReply::OpenMenu {
                        path: project.join(name),
                        kind,
                        anchor,
                    },
                    "{event:?}"
                );
            }
        }
        assert_eq!(tree.handle_key(key(KeyCode::F(10))), TreeReply::Ignored);
    }

    #[test]
    fn menu_keys_with_other_modifiers_do_not_open_the_menu() {
        let (_dir, _, _, mut tree) = keyed("menu-modifiers", SIDEBAR);
        tree.select(2);
        for event in [
            key_with(KeyCode::F(10), KeyModifiers::CONTROL | KeyModifiers::SHIFT),
            key_with(KeyCode::F(10), KeyModifiers::ALT),
            key_with(KeyCode::Menu, KeyModifiers::ALT),
            key_with(KeyCode::Menu, KeyModifiers::CONTROL),
            key_with(KeyCode::F(22), KeyModifiers::ALT | KeyModifiers::SHIFT),
        ] {
            assert_eq!(tree.handle_key(event), TreeReply::Ignored, "{event:?}");
        }
        for event in [
            key_with(KeyCode::F(10), KeyModifiers::SHIFT),
            key(KeyCode::F(22)),
            key(KeyCode::Menu),
        ] {
            assert!(
                matches!(tree.handle_key(event), TreeReply::OpenMenu { .. }),
                "{event:?}"
            );
        }
    }

    #[test]
    fn menu_keys_with_nothing_selected_open_the_root_menu() {
        let (_dir, _, editor, mut tree) = keyed("menu-root", SIDEBAR);
        let root = editor.workspace().unwrap().root.clone();
        assert_eq!(
            tree.handle_key(key_with(KeyCode::F(10), KeyModifiers::SHIFT)),
            TreeReply::OpenMenu {
                path: root,
                kind: RowKind::Dir(FolderState::Expanded),
                anchor: Position::new(1, 1),
            }
        );
    }

    #[test]
    fn menu_keys_without_a_workspace_are_ignored() {
        let mut tree = FileTree::new();
        assert_eq!(tree.handle_key(key(KeyCode::Menu)), TreeReply::Ignored);
        assert_eq!(tree.handle_key(key(KeyCode::F(22))), TreeReply::Ignored);
    }

    #[test]
    fn menu_anchor_follows_depth_area_and_scroll() {
        let (_dir, project, mut editor) = fixture("anchor", &[]);
        editor.apply(Command::SetExpanded {
            dir: project.join("src"),
            state: FolderState::Expanded,
        });
        let mut tree = FileTree::new();
        let area = Rect::new(5, 2, 30, 4);
        render_in(&mut tree, &editor, area, Rect::new(0, 0, 60, 8));
        // main.rs: depth 1, row 2, drawn on the third rows line.
        tree.select(2);
        assert_eq!(tree.menu_anchor(), Position::new(12, 5));

        tree.top = 1;
        assert_eq!(tree.menu_anchor(), Position::new(12, 4));
        // README.md (row 4) is below the three drawn rows.
        tree.select(4);
        assert_eq!(tree.menu_anchor(), Position::new(6, 3));
        tree.top = 2;
        assert_eq!(tree.menu_anchor(), Position::new(10, 5));
        // docs (row 0) is now scrolled off.
        tree.select(0);
        assert_eq!(tree.menu_anchor(), Position::new(6, 3));
    }

    #[test]
    fn angle_brackets_resize_and_clamp() {
        let (_dir, _, _, mut tree) = keyed("resize-keys", SIDEBAR);
        // SCREEN is 50 wide, so the widest is 30, the default.
        assert_eq!(tree.handle_key(key(KeyCode::Char('>'))), TreeReply::Resized);
        assert_eq!(tree.width(), 30);
        assert_eq!(tree.handle_key(key(KeyCode::Char('<'))), TreeReply::Resized);
        assert_eq!(tree.width(), 29);
        assert_eq!(
            tree.handle_key(key_with(KeyCode::Char('<'), KeyModifiers::SHIFT)),
            TreeReply::Resized
        );
        assert_eq!(tree.width(), 28);
        assert_eq!(
            tree.handle_key(key_with(KeyCode::Char('>'), KeyModifiers::SHIFT)),
            TreeReply::Resized
        );
        assert_eq!(tree.width(), 29);
        for _ in 0..40 {
            assert_eq!(tree.handle_key(key(KeyCode::Char('<'))), TreeReply::Resized);
        }
        assert_eq!(tree.width(), MIN_WIDTH);
        for _ in 0..40 {
            tree.handle_key(key(KeyCode::Char('>')));
        }
        assert_eq!(tree.width(), SCREEN.width - MIN_EDITOR_WIDTH);
    }

    #[test]
    fn modified_and_unbound_keys_are_ignored() {
        let (_dir, _, _, mut tree) = keyed("ignored", SIDEBAR);
        tree.select(1);
        let width = tree.width();
        let release =
            KeyEvent::new_with_kind(KeyCode::Down, KeyModifiers::NONE, KeyEventKind::Release);
        for event in [
            key_with(KeyCode::Down, KeyModifiers::CONTROL),
            key_with(KeyCode::Right, KeyModifiers::ALT),
            key_with(KeyCode::Down, KeyModifiers::SHIFT),
            key_with(KeyCode::Char('<'), KeyModifiers::CONTROL),
            key_with(KeyCode::Menu, KeyModifiers::SUPER),
            key(KeyCode::Char('x')),
            release,
        ] {
            assert_eq!(tree.handle_key(event), TreeReply::Ignored, "{event:?}");
        }
        assert_eq!(selected_index(&tree), Some(1));
        assert_eq!(tree.width(), width);
    }

    #[test]
    fn wheel_clamps_and_leaves_the_selection() {
        let (_dir, _, _, mut tree) = keyed("wheel", Rect::new(0, 0, 30, 3));
        tree.select(1);
        tree.wheel(1);
        assert_eq!(tree.top, 1);
        tree.wheel(5);
        assert_eq!(tree.top, 2);
        tree.wheel(i32::MAX);
        assert_eq!(tree.top, 2);
        tree.wheel(-1);
        assert_eq!(tree.top, 1);
        tree.wheel(i32::MIN);
        assert_eq!(tree.top, 0);
        assert_eq!(selected_index(&tree), Some(1));
    }

    #[test]
    fn drag_to_only_while_resizing() {
        let (_dir, _, editor) = fixture("drag", &[]);
        let mut tree = FileTree::new();
        render_in(
            &mut tree,
            &editor,
            Rect::new(5, 0, 30, 8),
            Rect::new(0, 0, 80, 8),
        );
        assert!(!tree.is_resizing());
        tree.drag_to(20);
        assert_eq!(tree.width(), DEFAULT_WIDTH);

        tree.begin_resize();
        assert!(tree.is_resizing());
        tree.drag_to(20);
        assert_eq!(tree.width(), 16);
        tree.drag_to(0);
        assert_eq!(tree.width(), MIN_WIDTH);
        tree.drag_to(200);
        assert_eq!(tree.width(), 80 - MIN_EDITOR_WIDTH);

        tree.end_resize();
        assert!(!tree.is_resizing());
        tree.drag_to(20);
        assert_eq!(tree.width(), 80 - MIN_EDITOR_WIDTH);
    }

    #[test]
    fn a_stale_hovered_row_after_a_refresh_is_not_painted() {
        let (_dir, project, editor) = fixture("stale-hover", &[]);
        let mut tree = FileTree::new();
        render(&mut tree, &editor, TreeFocus::Unfocused, None, SIDEBAR);
        // Row 1 is `src`; removing `docs` moves `Cargo.toml` into row 1.
        fs::remove_dir_all(project.join("docs")).unwrap();
        let hovered = Some(Target::TreeRow(1));
        let (buf, _) = render(&mut tree, &editor, TreeFocus::Unfocused, hovered, SIDEBAR);
        assert_eq!(row_text(&buf, 2), "     Cargo.toml");
        assert_eq!(Some(buf[(0, 2)].bg), mantle());

        // With the rows unchanged, the hover is painted again.
        let (buf, _) = render(&mut tree, &editor, TreeFocus::Unfocused, hovered, SIDEBAR);
        assert_eq!(buf[(0, 2)].bg, theme::hover_bg());
    }

    #[test]
    fn refresh_clamps_top_to_the_last_full_page() {
        let (_dir, project, mut editor) = fixture("refresh-top", &[]);
        let src = project.join("src");
        editor.apply(Command::SetExpanded {
            dir: src.clone(),
            state: FolderState::Expanded,
        });
        let mut tree = FileTree::new();
        // Two rows shown out of five.
        render_in(&mut tree, &editor, Rect::new(0, 0, 30, 3), SCREEN);
        tree.top = 3;
        editor.apply(Command::SetExpanded {
            dir: src,
            state: FolderState::Collapsed,
        });
        assert_eq!(tree.refresh(editor.workspace()), Refresh::Changed);
        assert_eq!(tree.top, 2);
    }

    #[test]
    fn zero_area_draws_nothing() {
        let (_dir, _, editor) = fixture("zero", &[]);
        let mut tree = FileTree::new();
        let (buf, hits) = render(
            &mut tree,
            &editor,
            TreeFocus::Focused,
            None,
            Rect::new(0, 0, 0, 0),
        );
        assert_eq!(buf, Buffer::empty(SCREEN));
        assert_eq!(hits.hit(0, 0), None);
        let (buf, hits) = render(
            &mut tree,
            &editor,
            TreeFocus::Focused,
            None,
            Rect::new(0, 0, 30, 0),
        );
        assert_eq!(buf, Buffer::empty(SCREEN));
        assert_eq!(hits.hit(0, 0), None);
    }

    #[test]
    fn set_space_clamps_the_stored_width() {
        let mut tree = FileTree::new();
        tree.set_space(200);
        assert_eq!((tree.total, tree.width()), (200, DEFAULT_WIDTH));
        // 40 columns leave the editor text 20, so the sidebar gets 20.
        tree.set_space(40);
        assert_eq!((tree.total, tree.width()), (40, 40 - MIN_EDITOR_WIDTH));
        // The stored width stays clamped when the space grows again.
        tree.set_space(200);
        assert_eq!(tree.width(), 40 - MIN_EDITOR_WIDTH);
        tree.set_space(10);
        assert_eq!((tree.total, tree.width()), (10, MIN_WIDTH));
        // Keyboard resizing clamps against the same space.
        tree.set_space(57);
        for _ in 0..60 {
            tree.handle_key(key(KeyCode::Char('>')));
        }
        assert_eq!(tree.width(), 57 - MIN_EDITOR_WIDTH);
    }

    #[test]
    fn render_returns_whether_the_rows_changed() {
        let (_dir, project, editor) = fixture("render-refresh", &[]);
        let mut tree = FileTree::new();
        assert_eq!(
            render_in(&mut tree, &editor, SIDEBAR, SCREEN),
            Refresh::Changed
        );
        assert_eq!(
            render_in(&mut tree, &editor, SIDEBAR, SCREEN),
            Refresh::Unchanged
        );
        fs::write(project.join("new.txt"), "").unwrap();
        assert_eq!(
            render_in(&mut tree, &editor, SIDEBAR, SCREEN),
            Refresh::Changed
        );
        // An empty area draws nothing but still reports the refresh.
        fs::write(project.join("newer.txt"), "").unwrap();
        assert_eq!(
            render_in(&mut tree, &editor, Rect::new(0, 0, 0, 0), SCREEN),
            Refresh::Changed
        );
    }
}
