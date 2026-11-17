//! The file-tree sidebar.
//!
//! Core owns the workspace (its root and the set of expanded folders)
//! and every file-system action; every change to the tree goes through a
//! [`frext_core::editor::Command`]. [`FileTree`] owns only the sidebar's
//! view state: the rows from the last refresh, the selected row, the
//! scroll position, the width and the border drag.

use std::path::{Path, PathBuf};

use frext_core::editor::{Editor, FolderState};
use frext_core::palette::MANTLE;
use frext_core::workspace::{RowKind, TreeRow, Workspace, visible_rows};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use crate::hit::{HitMap, Target};
use crate::icons::{AsciiIcons, ICON_WIDTH, IconCell, IconKind, IconProvider};
use crate::paint;
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
    /// The terminal width at the last render.
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

    /// The requested width, border included, before clamping.
    #[must_use]
    pub const fn width(&self) -> u16 {
        self.width
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
        self.top = self.top.min(self.rows.len().saturating_sub(1));
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

    /// Draws the sidebar into `area` and records its hit regions.
    ///
    /// Without a workspace, or with an empty `area`, nothing is drawn.
    pub fn render(
        &mut self,
        editor: &Editor,
        focus: TreeFocus,
        hovered: Option<Target>,
        area: Rect,
        buf: &mut Buffer,
        hits: &mut HitMap,
    ) {
        self.area = area;
        self.total = buf.area.width;
        self.refresh(editor.workspace());
        let Some(workspace) = editor.workspace() else {
            return;
        };
        let area = area.intersection(buf.area);
        if area.is_empty() {
            return;
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
            draw_row(buf, line, row, self.icons.as_ref(), base.bg(bg));
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
    }
}

/// Draws one tree row into `line` (which excludes the border) in
/// `style`; the icon keeps its own foreground.
fn draw_row(buf: &mut Buffer, line: Rect, row: &TreeRow, icons: &dyn IconProvider, style: Style) {
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
    let IconCell::Text { text, color } = icons.icon(&name, kind);
    let icon_room = room(x).min(usize::from(ICON_WIDTH));
    buf.set_stringn(x, line.y, text, icon_room, style.fg(theme::color(color)));

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

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "frext-term-tree-test-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
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
    fn project(tag: &str) -> (PathBuf, PathBuf) {
        let dir = temp_dir(tag);
        let project = dir.join("project");
        fs::create_dir_all(project.join("src")).unwrap();
        fs::create_dir_all(project.join("docs")).unwrap();
        fs::write(project.join("src").join("main.rs"), "fn main() {}").unwrap();
        fs::write(project.join("Cargo.toml"), "[package]").unwrap();
        fs::write(project.join("README.md"), "# readme").unwrap();
        (dir, project)
    }

    /// An editor over the project fixture, with the project as workspace.
    fn fixture(tag: &str, files: &[&str]) -> (PathBuf, Editor) {
        let (dir, project) = project(tag);
        let files: Vec<PathBuf> = files.iter().map(|name| project.join(name)).collect();
        let editor = open_editor(&dir, &files, Some(&project));
        (project, editor)
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
        tree.render(editor, focus, hovered, area, &mut buf, &mut hits);
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
    fn render_stores_area_and_total() {
        let (_, editor) = fixture("stores", &[]);
        let mut tree = FileTree::new();
        render(&mut tree, &editor, TreeFocus::Unfocused, None, SIDEBAR);
        assert_eq!(tree.area, SIDEBAR);
        assert_eq!(tree.total, SCREEN.width);
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
        let (_, editor) = fixture("header", &[]);
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
        let (_, editor) = fixture("narrow-header", &[]);
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
        let (project, mut editor) = fixture("rows", &[]);
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
        let (_, editor) = fixture("border", &[]);
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
        let (_, editor) = fixture("regions", &[]);
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
        let (_, editor) = fixture("full", &[]);
        let mut tree = FileTree::new();
        let area = Rect::new(0, 0, 30, 5);
        let (_, hits) = render(&mut tree, &editor, TreeFocus::Unfocused, None, area);
        assert_eq!(hits.hit(0, 4), Some(Target::TreeRow(3)));
        assert_eq!(hits.hit(0, 5), None);
    }

    #[test]
    fn active_selected_and_hovered_styles() {
        let (_, editor) = fixture("styles", &["Cargo.toml"]);
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
        let (project, editor) = fixture("long", &[]);
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
        let (project, editor) = fixture("wide", &[]);
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
        let (_, editor) = fixture("scroll", &[]);
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
        let (project, editor) = fixture("refresh", &[]);
        let mut tree = FileTree::new();
        assert_eq!(tree.refresh(editor.workspace()), Refresh::Changed);
        assert_eq!(tree.refresh(editor.workspace()), Refresh::Unchanged);
        fs::write(project.join("new.txt"), "").unwrap();
        assert_eq!(tree.refresh(editor.workspace()), Refresh::Changed);
        assert_eq!(tree.rows().len(), 5);
    }

    #[test]
    fn select_ignores_missing_rows() {
        let (project, editor) = fixture("select", &[]);
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
        let (project, mut editor) = fixture("follow", &[]);
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
        let (project, mut editor) = fixture("parent", &[]);
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
        let (project, editor) = fixture("deleted", &[]);
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
        let (_, editor) = fixture("none", &[]);
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
        let (project, editor) = fixture("unselected", &[]);
        let mut tree = FileTree::new();
        tree.refresh(editor.workspace());
        fs::write(project.join("new.txt"), "").unwrap();
        tree.refresh(editor.workspace());
        assert_eq!(tree.selected(), None);
    }

    #[test]
    fn no_workspace_draws_nothing() {
        let dir = temp_dir("no-workspace");
        let editor = open_editor(&dir, &[], None);
        let mut tree = FileTree::new();
        let (buf, hits) = render(&mut tree, &editor, TreeFocus::Focused, None, SIDEBAR);
        assert_eq!(buf, Buffer::empty(SCREEN));
        for (x, y) in [(0, 0), (2, 0), (0, 1), (29, 3), (10, 7)] {
            assert_eq!(hits.hit(x, y), None);
        }
    }

    #[test]
    fn zero_area_draws_nothing() {
        let (_, editor) = fixture("zero", &[]);
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
}
