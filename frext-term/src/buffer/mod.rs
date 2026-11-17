//! The editable text buffer view.
//!
//! [`cells`] decides how one grapheme is drawn and how many terminal
//! cells it takes, and [`wrap`] lays a text out into soft-wrapped visual
//! rows and maps a caret between char indices and (row, column).
//! [`words`] finds word and line boundaries for word motions and
//! double/triple-click selection. [`cursor`] holds the caret and
//! selection and the motions that move them. [`history`] groups edits
//! into undo/redo steps. [`edit`] applies an edit action (typing,
//! deleting, outdent, undo/redo, or a motion) to the text. [`render`]
//! draws laid-out, highlighted rows with their gutter and returns the
//! caret's screen position.
//!
//! [`BufferView`] ties them together for the app: it keeps each tab's
//! cursor, history, highlighter, layout and scroll position, applies
//! keyboard and mouse input to the active tab, detects text changes it
//! did not make, and renders the tab.

pub mod cells;
pub mod cursor;
pub mod edit;
pub mod history;
pub mod render;
pub mod words;
pub mod wrap;

use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use std::time::{Duration, Instant};

use frext_core::highlight::incremental::IncrementalHighlighter;
use frext_core::syntax::language_from_path;
use frext_core::tab::Tab;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;

use crate::buffer::cursor::{Cursor, Selecting};
use crate::buffer::edit::{Applied, EditAction, EditTarget};
use crate::buffer::history::{Edit, EditHistory};
use crate::buffer::render::RowsView;
use crate::buffer::wrap::WrapLayout;
use crate::hit::{HitMap, Target};
use crate::theme;

/// The longest pause between two presses on the same cell that still
/// counts as a double (or triple) click.
pub const DOUBLE_CLICK: Duration = Duration::from_millis(500);

/// How many visual rows one mouse-wheel notch scrolls.
pub const WHEEL_ROWS: i32 = 3;

/// The unit a press selects by, and a drag extends by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Granularity {
    Char,
    Word,
    Line,
}

/// The last left press, for counting clicks.
#[derive(Debug, Clone, Copy)]
struct Press {
    column: u16,
    row: u16,
    at: Instant,
    granularity: Granularity,
}

/// A drag in progress: its granularity and the range the press selected.
#[derive(Debug, Clone)]
struct Drag {
    granularity: Granularity,
    origin: Range<usize>,
}

/// The view state of one tab.
#[derive(Debug)]
struct TabView {
    cursor: Cursor,
    history: EditHistory,
    highlighter: IncrementalHighlighter,
    /// The text hash the layout was built for, and the layout.
    layout: Option<(u64, WrapLayout)>,
    /// The char index of the start of the top visual row.
    top: usize,
    /// The text hash after the view's last change.
    known: u64,
}

impl TabView {
    fn new(known: u64) -> Self {
        Self {
            cursor: Cursor::at(0),
            history: EditHistory::default(),
            highlighter: IncrementalHighlighter::new(),
            layout: None,
            top: 0,
            known,
        }
    }
}

/// The editable view of the open tabs: per-tab caret, selection, undo
/// history, highlighting and scroll position, plus the mouse state.
#[derive(Debug, Default)]
pub struct BufferView {
    tabs: HashMap<u64, TabView>,
    /// The editor rect of the last render.
    rect: Rect,
    press: Option<Press>,
    drag: Option<Drag>,
}

/// The hash of `text`, for detecting changes.
fn text_hash(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// The wrap width for an editor rect: one column is kept free.
fn wrap_width(rect: Rect) -> u16 {
    rect.width.saturating_sub(1).max(1)
}

/// The view state of `tab`, created if new, with outside changes
/// detected and the layout fresh for `rect`.
fn prepare<'a>(tabs: &'a mut HashMap<u64, TabView>, rect: Rect, tab: &Tab) -> &'a mut TabView {
    let hash = text_hash(&tab.text);
    let view = tabs.entry(tab.id).or_insert_with(|| TabView::new(hash));
    if view.known != hash {
        view.cursor.clamp(&tab.text);
        view.history.clear();
        view.known = hash;
    }
    refresh_layout(view, &tab.text, hash, wrap_width(rect));
    view
}

/// Rebuilds the layout of `view` unless it is for `hash` at `width`.
fn refresh_layout(view: &mut TabView, text: &str, hash: u64, width: u16) {
    let fresh = view
        .layout
        .as_ref()
        .is_some_and(|(h, layout)| *h == hash && layout.width() == width);
    if !fresh {
        view.layout = Some((hash, WrapLayout::new(text, width)));
    }
}

/// The top visual row for the scroll position `top`, clamped.
fn top_row(layout: &WrapLayout, top: usize) -> usize {
    layout
        .row_of_char(top)
        .min(layout.rows().len().saturating_sub(1))
}

/// The scroll position (row start char) for visual row `row`, clamped.
fn row_start(layout: &WrapLayout, row: usize) -> usize {
    let rows = layout.rows();
    rows.get(row.min(rows.len().saturating_sub(1)))
        .map_or(0, |visual| visual.chars.start)
}

/// Scrolls `view` the least so the caret is inside `height` rows.
fn keep_caret_visible(view: &mut TabView, height: u16) {
    let Some((_, layout)) = view.layout.as_ref() else {
        return;
    };
    let caret_row = layout.row_of_char(view.cursor.caret());
    let height = usize::from(height);
    let mut row = top_row(layout, view.top);
    if caret_row < row {
        row = caret_row;
    } else if height > 0 && caret_row >= row.saturating_add(height) {
        row = caret_row.saturating_add(1).saturating_sub(height);
    }
    view.top = row_start(layout, row);
}

/// Scrolls `view` by `delta` visual rows, clamped to the layout.
fn scroll_rows(view: &mut TabView, delta: i64) {
    let Some((_, layout)) = view.layout.as_ref() else {
        return;
    };
    let current = i64::try_from(top_row(layout, view.top)).unwrap_or(i64::MAX);
    let last = i64::try_from(layout.rows().len().saturating_sub(1)).unwrap_or(i64::MAX);
    let target = current.saturating_add(delta).clamp(0, last);
    view.top = row_start(layout, usize::try_from(target).unwrap_or(0));
}

/// Refreshes `view` after `applied` changed `text` or the cursor, and
/// keeps the caret visible.
fn after_change(view: &mut TabView, text: &str, rect: Rect, applied: Applied) {
    match applied {
        Applied::Edited => {
            let hash = text_hash(text);
            view.known = hash;
            refresh_layout(view, text, hash, wrap_width(rect));
            keep_caret_visible(view, rect.height);
        }
        Applied::Moved => keep_caret_visible(view, rect.height),
        Applied::Unchanged => {}
    }
}

/// The char under the absolute cell (`column`, `row`) of `rect`, with the
/// column clamped into the rect.
fn char_at_cell(view: &TabView, text: &str, rect: Rect, column: u16, row: u16) -> usize {
    let Some((_, layout)) = view.layout.as_ref() else {
        return 0;
    };
    let column = column
        .saturating_sub(rect.x)
        .min(rect.width.saturating_sub(1));
    let layout_row =
        top_row(layout, view.top).saturating_add(usize::from(row.saturating_sub(rect.y)));
    layout.char_at(text, layout_row, column)
}

/// The word or line around `char_idx`, by `granularity`; a single point
/// for [`Granularity::Char`].
fn unit_at(text: &str, char_idx: usize, granularity: Granularity) -> Range<usize> {
    match granularity {
        Granularity::Char => char_idx..char_idx,
        Granularity::Word => words::word_at(text, char_idx),
        Granularity::Line => words::line_at(text, char_idx),
    }
}

/// The common prefix and suffix of `before` and `after` as byte lengths
/// and the prefix as a char count. The suffix never overlaps the prefix.
fn common_affixes(before: &str, after: &str) -> (usize, usize, usize) {
    let mut prefix_bytes = 0;
    let mut prefix_chars = 0;
    for (a, b) in before.chars().zip(after.chars()) {
        if a != b {
            break;
        }
        prefix_bytes += a.len_utf8();
        prefix_chars += 1;
    }
    let before_rest = before.get(prefix_bytes..).unwrap_or("");
    let after_rest = after.get(prefix_bytes..).unwrap_or("");
    let suffix_bytes = before_rest
        .chars()
        .rev()
        .zip(after_rest.chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum();
    (prefix_bytes, prefix_chars, suffix_bytes)
}

/// Blanks every cell of `area` with `style`.
fn fill(buf: &mut Buffer, area: Rect, style: Style) {
    let area = area.intersection(buf.area);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.reset();
                cell.set_symbol(" ").set_style(style);
            }
        }
    }
}

impl BufferView {
    /// Applies `action` to `tab` (the active tab) and keeps the caret
    /// visible. PageUp/PageDown move by the last rendered editor height;
    /// `now` decides whether an edit coalesces with the previous one.
    pub fn handle(&mut self, tab: &mut Tab, action: EditAction, now: Instant) -> Applied {
        let rect = self.rect;
        let view = prepare(&mut self.tabs, rect, tab);
        let Some((_, layout)) = view.layout.as_ref() else {
            return Applied::Unchanged;
        };
        let is_move = matches!(action, EditAction::Move(..));
        let applied = edit::apply(
            EditTarget {
                text: &mut tab.text,
                cursor: &mut view.cursor,
                history: &mut view.history,
            },
            layout,
            action,
            rect.height,
            now,
        );
        after_change(view, &tab.text, rect, applied);
        // A motion that leaves the caret where it was (Ctrl+Home at 0)
        // still brings a wheel-scrolled caret back into view.
        if is_move && applied == Applied::Unchanged {
            keep_caret_visible(view, rect.height);
        }
        applied
    }

    /// Inserts `text` (a paste) into `tab` at the caret, replacing the
    /// selection, as one undo group, and keeps the caret visible.
    pub fn insert_text(&mut self, tab: &mut Tab, text: &str, now: Instant) -> Applied {
        let rect = self.rect;
        let view = prepare(&mut self.tabs, rect, tab);
        let applied = edit::insert_text(
            EditTarget {
                text: &mut tab.text,
                cursor: &mut view.cursor,
                history: &mut view.history,
            },
            text,
            now,
        );
        after_change(view, &tab.text, rect, applied);
        applied
    }

    /// Records a change made to `tab` outside the view (a core command),
    /// where `before` is the text before it, as one undo group. The
    /// cursor is left alone; the app's effects place it. A tab the view
    /// has never seen has nothing to record. A recorded group is never
    /// coalesced, so `_now` does not affect grouping.
    ///
    /// `after` is the selection a redo restores, clamped to the new text:
    /// the core command's own selection when it set one. With `None` the
    /// caret goes to the end of the changed span found by diffing `before`
    /// against the new text, which is ambiguous in repeated text.
    pub fn record_outside_edit(
        &mut self,
        tab: &Tab,
        before: &str,
        after: Option<Range<usize>>,
        _now: Instant,
    ) {
        if !self.tabs.contains_key(&tab.id) {
            prepare(&mut self.tabs, self.rect, tab);
            return;
        }
        let Some(view) = self.tabs.get_mut(&tab.id) else {
            return;
        };
        if before == tab.text {
            return;
        }
        let text = tab.text.as_str();
        let (prefix_bytes, at, suffix_bytes) = common_affixes(before, text);
        let removed = before
            .get(prefix_bytes..before.len().saturating_sub(suffix_bytes))
            .unwrap_or("")
            .to_owned();
        let inserted = text
            .get(prefix_bytes..text.len().saturating_sub(suffix_bytes))
            .unwrap_or("")
            .to_owned();
        let after = after.map_or_else(
            || Cursor::at(at.saturating_add(inserted.chars().count())),
            |chars| {
                let mut cursor = Cursor::spanning(chars.start, chars.end);
                cursor.clamp(text);
                cursor
            },
        );
        view.history.record_group(vec![Edit {
            at,
            removed,
            inserted,
            before: view.cursor,
            after,
        }]);
        view.known = text_hash(text);
    }

    /// The selected chars of the tab with `tab_id`, if the view knows it.
    #[must_use]
    pub fn selection(&self, tab_id: u64) -> Option<Range<usize>> {
        self.tabs.get(&tab_id).map(|view| view.cursor.selection())
    }

    /// The caret of the tab with `tab_id`, if the view knows it.
    #[must_use]
    pub fn caret(&self, tab_id: u64) -> Option<usize> {
        self.tabs.get(&tab_id).map(|view| view.cursor.caret())
    }

    /// Selects `chars` of `tab` (anchor at the start, caret at the end),
    /// clamped to the text, and keeps the caret visible.
    pub fn set_selection(&mut self, tab: &Tab, chars: Range<usize>) {
        let rect = self.rect;
        let view = prepare(&mut self.tabs, rect, tab);
        view.cursor = Cursor::spanning(chars.start, chars.end);
        view.cursor.clamp(&tab.text);
        view.history.break_group();
        keep_caret_visible(view, rect.height);
    }

    /// Scrolls `tab` so the caret row is vertically centred.
    pub fn reveal(&mut self, tab: &Tab) {
        let rect = self.rect;
        let view = prepare(&mut self.tabs, rect, tab);
        let Some((_, layout)) = view.layout.as_ref() else {
            return;
        };
        let caret_row = layout.row_of_char(view.cursor.caret());
        let row = caret_row.saturating_sub(usize::from(rect.height / 2));
        view.top = row_start(layout, row);
    }

    /// Scrolls `tab` by `rows` visual rows (negative is up) without
    /// moving the caret.
    pub fn wheel(&mut self, tab: &Tab, rows: i32) {
        let view = prepare(&mut self.tabs, self.rect, tab);
        scroll_rows(view, i64::from(rows));
    }

    /// A left press at the absolute cell (`column`, `row`) on `tab`:
    /// places the caret, or extends from the anchor with
    /// [`Selecting::Extend`]. A second or third press on the same cell
    /// within [`DOUBLE_CLICK`] selects the word or the line.
    pub fn mouse_down(
        &mut self,
        tab: &Tab,
        column: u16,
        row: u16,
        selecting: Selecting,
        now: Instant,
    ) {
        let rect = self.rect;
        let repeat = self.press.and_then(|press| {
            (press.column == column
                && press.row == row
                && now.saturating_duration_since(press.at) <= DOUBLE_CLICK)
                .then_some(press.granularity)
        });
        let view = prepare(&mut self.tabs, rect, tab);
        let p = char_at_cell(view, &tab.text, rect, column, row);
        let (granularity, origin) = if selecting == Selecting::Extend {
            let anchor = view.cursor.anchor();
            view.cursor = Cursor::spanning(anchor, p);
            (Granularity::Char, anchor..anchor)
        } else {
            let granularity = match repeat {
                None => Granularity::Char,
                Some(Granularity::Char) => Granularity::Word,
                Some(Granularity::Word | Granularity::Line) => Granularity::Line,
            };
            let unit = unit_at(&tab.text, p, granularity);
            view.cursor = Cursor::spanning(unit.start, unit.end);
            (granularity, unit)
        };
        view.history.break_group();
        self.press = Some(Press {
            column,
            row,
            at: now,
            granularity,
        });
        self.drag = Some(Drag {
            granularity,
            origin,
        });
    }

    /// Extends a drag to the absolute cell (`column`, `row`) by the
    /// press's granularity. Above or below the editor it scrolls one row
    /// and uses the edge row. Does nothing with no drag in progress.
    pub fn mouse_drag(&mut self, tab: &Tab, column: u16, row: u16) {
        let Some(drag) = self.drag.clone() else {
            return;
        };
        let rect = self.rect;
        if rect.height == 0 {
            return;
        }
        let view = prepare(&mut self.tabs, rect, tab);
        let row = if row < rect.y {
            scroll_rows(view, -1);
            rect.y
        } else if row >= rect.bottom() {
            scroll_rows(view, 1);
            rect.bottom().saturating_sub(1)
        } else {
            row
        };
        let p = char_at_cell(view, &tab.text, rect, column, row);
        let origin = drag.origin;
        view.cursor = if drag.granularity == Granularity::Char {
            Cursor::spanning(origin.start, p)
        } else {
            let unit = unit_at(&tab.text, p, drag.granularity);
            if p < origin.start {
                Cursor::spanning(origin.end, unit.start)
            } else if p > origin.end || origin.is_empty() {
                Cursor::spanning(origin.start, unit.end)
            } else {
                Cursor::spanning(origin.start, origin.end)
            }
        };
    }

    /// Ends a drag.
    pub fn mouse_up(&mut self) {
        self.drag = None;
    }

    /// Whether a drag is in progress.
    #[must_use]
    pub const fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Drops the state of every tab not in `live_ids`.
    pub fn forget(&mut self, live_ids: impl IntoIterator<Item = u64>) {
        let live: HashSet<u64> = live_ids.into_iter().collect();
        self.tabs.retain(|id, _| live.contains(id));
    }

    /// Draws `tab` into `editor` and its line numbers into `gutter`,
    /// records `editor` as [`Target::Editor`], and returns the caret's
    /// screen position when it is visible. With no tab, both rects are
    /// blank.
    pub fn render(
        &mut self,
        tab: Option<&Tab>,
        gutter: Rect,
        editor: Rect,
        buf: &mut Buffer,
        hits: &mut HitMap,
    ) -> Option<Position> {
        self.rect = editor;
        hits.push(editor, Target::Editor);
        let Some(tab) = tab else {
            fill(buf, gutter, theme::base());
            fill(buf, editor, theme::base());
            return None;
        };
        let view = prepare(&mut self.tabs, editor, tab);
        let (_, layout) = view.layout.as_ref()?;
        let first = top_row(layout, view.top);
        view.top = row_start(layout, first);
        let rows = layout.rows();
        let end = first
            .saturating_add(usize::from(editor.height))
            .min(rows.len());
        let lines = match (
            rows.get(first),
            end.checked_sub(1).and_then(|i| rows.get(i)),
        ) {
            (Some(top), Some(bottom)) if end > first => top.line..bottom.line.saturating_add(1),
            _ => 0..0,
        };
        let spans =
            view.highlighter
                .spans(&tab.text, &language_from_path(tab.path.as_deref()), lines);
        render::render(
            &RowsView {
                text: &tab.text,
                layout,
                top_row: first,
                spans: &spans,
                selection: view.cursor.selection(),
                caret: view.cursor.caret(),
            },
            gutter,
            editor,
            buf,
        )
    }
}

#[cfg(test)]
mod tests {
    // Tests unwrap freely: a failure is a test failure.
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::buffer::cursor::Motion;
    use frext_core::palette::BASE;

    const GUTTER: u16 = 4;

    fn tab(id: u64, text: &str) -> Tab {
        let mut tab = Tab::new_untitled(id);
        tab.text = text.to_owned();
        tab
    }

    /// Text with lines "1" through "n".
    fn numbered(n: usize) -> String {
        (1..=n)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The char index of the start of 0-based `line`.
    fn line_start(text: &str, line: usize) -> usize {
        text.split('\n')
            .take(line)
            .map(|l| l.chars().count() + 1)
            .sum()
    }

    /// Renders into a `width` x `height` buffer: a `GUTTER`-wide gutter,
    /// then the editor.
    fn draw(
        view: &mut BufferView,
        tab: Option<&Tab>,
        width: u16,
        height: u16,
    ) -> (Buffer, Option<Position>, HitMap) {
        let mut buf = Buffer::empty(Rect::new(0, 0, width, height));
        let mut hits = HitMap::default();
        let caret = view.render(
            tab,
            Rect::new(0, 0, GUTTER, height),
            Rect::new(GUTTER, 0, width - GUTTER, height),
            &mut buf,
            &mut hits,
        );
        (buf, caret, hits)
    }

    fn row_text(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
    }

    fn top_row_of(view: &BufferView, id: u64) -> usize {
        let tab_view = &view.tabs[&id];
        let (_, layout) = tab_view.layout.as_ref().unwrap();
        top_row(layout, tab_view.top)
    }

    fn ms(base: Instant, millis: u64) -> Instant {
        base + Duration::from_millis(millis)
    }

    #[test]
    fn typing_and_rendering_show_the_text_and_the_caret() {
        let mut tab = tab(1, "");
        let mut view = BufferView::default();
        let base = Instant::now();
        draw(&mut view, Some(&tab), 12, 2);
        assert_eq!(
            view.handle(&mut tab, EditAction::Type('h'), base),
            Applied::Edited
        );
        view.handle(&mut tab, EditAction::Type('i'), ms(base, 10));
        assert_eq!(tab.text, "hi");
        let (buf, caret, hits) = draw(&mut view, Some(&tab), 12, 2);
        assert_eq!(row_text(&buf, 0), "  1 hi      ");
        assert_eq!(caret, Some(Position { x: 6, y: 0 }));
        assert_eq!(hits.hit(5, 1), Some(Target::Editor));
        assert_eq!(view.caret(1), Some(2));
        assert_eq!(view.selection(1), Some(2..2));
    }

    #[test]
    fn the_caret_is_kept_visible_when_typing_past_the_bottom() {
        let mut tab = tab(1, "");
        let mut view = BufferView::default();
        let base = Instant::now();
        draw(&mut view, Some(&tab), 12, 3);
        for i in 0..5 {
            view.handle(&mut tab, EditAction::Newline, ms(base, i));
        }
        view.handle(&mut tab, EditAction::Type('z'), ms(base, 10));
        assert_eq!(top_row_of(&view, 1), 3);
        let (buf, caret, _) = draw(&mut view, Some(&tab), 12, 3);
        assert_eq!(row_text(&buf, 2), "  6 z       ");
        assert_eq!(caret, Some(Position { x: 5, y: 2 }));
        // Moving back up scrolls up just enough.
        view.handle(
            &mut tab,
            EditAction::Move(Motion::DocStart, Selecting::Move),
            ms(base, 20),
        );
        assert_eq!(top_row_of(&view, 1), 0);
    }

    #[test]
    fn reveal_centres_the_caret_row() {
        let text = numbered(100);
        let tab = tab(1, &text);
        let mut view = BufferView::default();
        draw(&mut view, Some(&tab), 12, 10);
        let start = line_start(&text, 50);
        view.set_selection(&tab, start..start);
        assert_eq!(top_row_of(&view, 1), 41);
        view.reveal(&tab);
        assert_eq!(top_row_of(&view, 1), 45);
        let (buf, _, _) = draw(&mut view, Some(&tab), 12, 10);
        assert_eq!(row_text(&buf, 0), " 46 46      ");
    }

    #[test]
    fn the_wheel_scrolls_without_moving_the_caret() {
        let tab = tab(1, &numbered(100));
        let mut view = BufferView::default();
        draw(&mut view, Some(&tab), 12, 10);
        view.wheel(&tab, WHEEL_ROWS);
        assert_eq!(top_row_of(&view, 1), 3);
        assert_eq!(view.caret(1), Some(0));
        let (_, caret, _) = draw(&mut view, Some(&tab), 12, 10);
        assert_eq!(caret, None);
        view.wheel(&tab, -10);
        assert_eq!(top_row_of(&view, 1), 0);
        view.wheel(&tab, 1_000);
        assert_eq!(top_row_of(&view, 1), 99);
        assert_eq!(view.caret(1), Some(0));
    }

    #[test]
    fn an_unmoved_doc_start_or_end_reveals_the_caret() {
        let text = numbered(100);
        let mut tab = tab(1, &text);
        let mut view = BufferView::default();
        let base = Instant::now();
        draw(&mut view, Some(&tab), 12, 10);
        assert_eq!(view.caret(1), Some(0));
        view.wheel(&tab, 30);
        assert_eq!(top_row_of(&view, 1), 30);
        assert_eq!(
            view.handle(
                &mut tab,
                EditAction::Move(Motion::DocStart, Selecting::Move),
                base
            ),
            Applied::Unchanged
        );
        assert_eq!(top_row_of(&view, 1), 0);

        let end = text.chars().count();
        view.set_selection(&tab, end..end);
        assert_eq!(top_row_of(&view, 1), 90);
        view.wheel(&tab, -30);
        assert_eq!(top_row_of(&view, 1), 60);
        assert_eq!(
            view.handle(
                &mut tab,
                EditAction::Move(Motion::DocEnd, Selecting::Move),
                ms(base, 10)
            ),
            Applied::Unchanged
        );
        let top = top_row_of(&view, 1);
        assert!(top <= 99 && 99 < top + 10, "caret row 99, top {top}");
        let (_, caret, _) = draw(&mut view, Some(&tab), 12, 10);
        assert!(caret.is_some());
    }

    #[test]
    fn a_resize_keeps_the_top_char() {
        let text = numbered(100);
        let tab = tab(1, &text);
        let mut view = BufferView::default();
        draw(&mut view, Some(&tab), 12, 10);
        view.wheel(&tab, 10);
        let top = view.tabs[&1].top;
        assert_eq!(top, line_start(&text, 10));
        let (buf, _, _) = draw(&mut view, Some(&tab), 30, 5);
        assert_eq!(view.tabs[&1].top, top);
        assert_eq!(&row_text(&buf, 0)[..6], " 11 11");
    }

    #[test]
    fn an_outside_change_clears_undo() {
        let mut tab = tab(1, "abc");
        let mut view = BufferView::default();
        let base = Instant::now();
        draw(&mut view, Some(&tab), 12, 3);
        view.handle(&mut tab, EditAction::Type('x'), base);
        assert_eq!(tab.text, "xabc");
        tab.text = "a disk reload".to_owned();
        assert_eq!(
            view.handle(&mut tab, EditAction::Undo, ms(base, 10)),
            Applied::Unchanged
        );
        assert_eq!(tab.text, "a disk reload");
    }

    #[test]
    fn an_outside_change_clamps_the_cursor() {
        let mut tab = tab(1, "abcdef");
        let mut view = BufferView::default();
        draw(&mut view, Some(&tab), 12, 3);
        view.set_selection(&tab, 2..6);
        tab.text = "ab".to_owned();
        draw(&mut view, Some(&tab), 12, 3);
        assert_eq!(view.selection(1), Some(2..2));
    }

    #[test]
    fn record_outside_edit_is_undoable_as_one_step() {
        let mut tab = tab(1, "hello world");
        let mut view = BufferView::default();
        let base = Instant::now();
        draw(&mut view, Some(&tab), 20, 3);
        view.set_selection(&tab, 6..6);
        let before = tab.text.clone();
        tab.text = "hello big wide world".to_owned();
        view.record_outside_edit(&tab, &before, None, base);
        // Not treated as an unknown change: the group survives.
        draw(&mut view, Some(&tab), 20, 3);
        assert_eq!(
            view.handle(&mut tab, EditAction::Undo, ms(base, 10)),
            Applied::Edited
        );
        assert_eq!(tab.text, "hello world");
        assert_eq!(view.selection(1), Some(6..6));
        view.handle(&mut tab, EditAction::Redo, ms(base, 20));
        assert_eq!(tab.text, "hello big wide world");
        assert_eq!(view.caret(1), Some(15));
    }

    #[test]
    fn record_outside_edit_handles_repeated_chars_and_unknown_tabs() {
        let mut tab = tab(1, "aaa");
        let mut view = BufferView::default();
        let base = Instant::now();
        // An unknown tab just starts being tracked.
        view.record_outside_edit(&tab, "zz", None, base);
        assert_eq!(view.caret(1), Some(0));
        let before = tab.text.clone();
        tab.text = "aaaa".to_owned();
        view.record_outside_edit(&tab, &before, None, base);
        view.handle(&mut tab, EditAction::Undo, ms(base, 10));
        assert_eq!(tab.text, "aaa");
        // No change records nothing.
        let before = tab.text.clone();
        view.record_outside_edit(&tab, &before, None, base);
        view.handle(&mut tab, EditAction::Undo, ms(base, 20));
        assert_eq!(tab.text, "aaa");
    }

    /// Records "aaa" -> "aaaa" with `after`, then undoes and redoes it,
    /// returning the caret after the redo.
    fn redo_caret_after_repeated_insert(after: Option<Range<usize>>) -> Option<usize> {
        let mut tab = tab(1, "aaa");
        let mut view = BufferView::default();
        let base = Instant::now();
        draw(&mut view, Some(&tab), 12, 3);
        let before = tab.text.clone();
        tab.text = "aaaa".to_owned();
        view.record_outside_edit(&tab, &before, after, base);
        view.handle(&mut tab, EditAction::Undo, ms(base, 10));
        assert_eq!(tab.text, "aaa");
        view.handle(&mut tab, EditAction::Redo, ms(base, 20));
        assert_eq!(tab.text, "aaaa");
        view.caret(1)
    }

    #[test]
    fn record_outside_edit_redo_restores_the_given_selection() {
        assert_eq!(redo_caret_after_repeated_insert(Some(1..1)), Some(1));
        // Out-of-range selections are clamped to the new text.
        assert_eq!(redo_caret_after_repeated_insert(Some(9..9)), Some(4));
    }

    #[test]
    fn record_outside_edit_without_a_selection_uses_the_diff_end() {
        // The diff cannot tell where in "aaa" the 'a' went: it picks the end.
        assert_eq!(redo_caret_after_repeated_insert(None), Some(4));
    }

    #[test]
    fn single_double_and_triple_clicks() {
        let tab = tab(1, "hello world\nsecond line");
        let mut view = BufferView::default();
        let base = Instant::now();
        draw(&mut view, Some(&tab), 20, 3);
        let x = GUTTER + 7;
        view.mouse_down(&tab, x, 0, Selecting::Move, base);
        assert_eq!(view.selection(1), Some(7..7));
        view.mouse_down(&tab, x, 0, Selecting::Move, ms(base, 100));
        assert_eq!(view.selection(1), Some(6..11));
        view.mouse_down(&tab, x, 0, Selecting::Move, ms(base, 200));
        assert_eq!(view.selection(1), Some(0..11));
        view.mouse_down(&tab, x, 0, Selecting::Move, ms(base, 300));
        assert_eq!(view.selection(1), Some(0..11));
        // More than 500 ms later counts as a single click.
        view.mouse_down(&tab, x, 0, Selecting::Move, ms(base, 801));
        assert_eq!(view.selection(1), Some(7..7));
        // A different cell is a single click too.
        view.mouse_down(&tab, x + 1, 0, Selecting::Move, ms(base, 850));
        assert_eq!(view.selection(1), Some(8..8));
    }

    #[test]
    fn a_click_past_the_row_end_goes_to_the_end() {
        let tab = tab(1, "ab\ncd");
        let mut view = BufferView::default();
        draw(&mut view, Some(&tab), 20, 4);
        view.mouse_down(&tab, 19, 0, Selecting::Move, Instant::now());
        assert_eq!(view.caret(1), Some(2));
        view.mouse_down(&tab, 0, 3, Selecting::Move, Instant::now());
        assert_eq!(view.caret(1), Some(3));
    }

    #[test]
    fn shift_click_extends_from_the_anchor() {
        let tab = tab(1, "hello world");
        let mut view = BufferView::default();
        let base = Instant::now();
        draw(&mut view, Some(&tab), 20, 3);
        view.mouse_down(&tab, GUTTER + 1, 0, Selecting::Move, base);
        view.mouse_up();
        view.mouse_down(&tab, GUTTER + 8, 0, Selecting::Extend, ms(base, 50));
        assert_eq!(view.selection(1), Some(1..8));
        assert_eq!(view.caret(1), Some(8));
        // A drag after a Shift+press extends by char from the anchor.
        view.mouse_drag(&tab, GUTTER + 3, 0);
        assert_eq!(view.selection(1), Some(1..3));
    }

    #[test]
    fn drag_by_char() {
        let tab = tab(1, "hello world\nsecond line");
        let mut view = BufferView::default();
        let base = Instant::now();
        draw(&mut view, Some(&tab), 20, 3);
        // No drag in progress: nothing happens.
        view.mouse_drag(&tab, GUTTER + 5, 1);
        assert_eq!(view.selection(1), Some(0..0));
        view.mouse_down(&tab, GUTTER + 1, 0, Selecting::Move, base);
        assert!(view.is_dragging());
        view.mouse_drag(&tab, GUTTER + 7, 1);
        assert_eq!(view.selection(1), Some(1..19));
        assert_eq!(view.caret(1), Some(19));
        view.mouse_drag(&tab, GUTTER, 0);
        assert_eq!(view.selection(1), Some(0..1));
        view.mouse_up();
        assert!(!view.is_dragging());
        view.mouse_drag(&tab, GUTTER + 7, 1);
        assert_eq!(view.selection(1), Some(0..1));
    }

    #[test]
    fn drag_by_word_keeps_the_original_word() {
        let tab = tab(1, "hello world\nsecond line");
        let mut view = BufferView::default();
        let base = Instant::now();
        draw(&mut view, Some(&tab), 20, 3);
        view.mouse_down(&tab, GUTTER + 7, 0, Selecting::Move, base);
        view.mouse_down(&tab, GUTTER + 7, 0, Selecting::Move, ms(base, 100));
        assert_eq!(view.selection(1), Some(6..11));
        // Backwards: from the word's end to the start of "hello".
        view.mouse_drag(&tab, GUTTER + 1, 0);
        assert_eq!(view.selection(1), Some(0..11));
        assert_eq!(view.caret(1), Some(0));
        // Inside the original word: just the word.
        view.mouse_drag(&tab, GUTTER + 8, 0);
        assert_eq!(view.selection(1), Some(6..11));
        // Forwards: to the end of "second".
        view.mouse_drag(&tab, GUTTER + 2, 1);
        assert_eq!(view.selection(1), Some(6..18));
        assert_eq!(view.caret(1), Some(18));
    }

    #[test]
    fn a_drag_below_the_editor_scrolls() {
        let text = numbered(100);
        let tab = tab(1, &text);
        let mut view = BufferView::default();
        draw(&mut view, Some(&tab), 12, 5);
        view.mouse_down(&tab, GUTTER, 0, Selecting::Move, Instant::now());
        view.mouse_drag(&tab, GUTTER, 10);
        assert_eq!(top_row_of(&view, 1), 1);
        assert_eq!(view.caret(1), Some(line_start(&text, 5)));
        view.mouse_drag(&tab, GUTTER, 10);
        assert_eq!(top_row_of(&view, 1), 2);
        assert_eq!(view.caret(1), Some(line_start(&text, 6)));
        assert_eq!(view.selection(1), Some(0..line_start(&text, 6)));
    }

    #[test]
    fn a_drag_above_the_editor_scrolls_up() {
        let text = numbered(100);
        let tab = tab(1, &text);
        let mut view = BufferView::default();
        let (_, _, _) = draw(&mut view, Some(&tab), 12, 5);
        view.wheel(&tab, 10);
        // Draw with the editor one row down, so a row above it exists.
        let mut buf = Buffer::empty(Rect::new(0, 0, 12, 6));
        let mut hits = HitMap::default();
        view.render(
            Some(&tab),
            Rect::new(0, 1, GUTTER, 5),
            Rect::new(GUTTER, 1, 8, 5),
            &mut buf,
            &mut hits,
        );
        view.mouse_down(&tab, GUTTER, 3, Selecting::Move, Instant::now());
        assert_eq!(view.caret(1), Some(line_start(&text, 12)));
        view.mouse_drag(&tab, GUTTER, 0);
        assert_eq!(top_row_of(&view, 1), 9);
        assert_eq!(view.caret(1), Some(line_start(&text, 9)));
    }

    #[test]
    fn set_selection_clamps_a_range_past_the_end() {
        let tab = tab(1, "abc");
        let mut view = BufferView::default();
        view.set_selection(&tab, 1..100);
        assert_eq!(view.selection(1), Some(1..3));
        assert_eq!(view.caret(1), Some(3));
    }

    #[test]
    fn forget_drops_closed_tabs() {
        let first = tab(1, "a");
        let second = tab(2, "b");
        let mut view = BufferView::default();
        view.set_selection(&first, 1..1);
        view.set_selection(&second, 1..1);
        view.forget([2]);
        assert_eq!(view.selection(1), None);
        assert_eq!(view.selection(2), Some(1..1));
        view.forget(std::iter::empty());
        assert_eq!(view.caret(2), None);
    }

    #[test]
    fn insert_text_pastes_as_one_group() {
        let mut tab = tab(1, "ab");
        let mut view = BufferView::default();
        let base = Instant::now();
        draw(&mut view, Some(&tab), 12, 3);
        view.set_selection(&tab, 1..1);
        assert_eq!(view.insert_text(&mut tab, "x\r\ny", base), Applied::Edited);
        assert_eq!(tab.text, "ax\nyb");
        assert_eq!(view.caret(1), Some(4));
        view.handle(&mut tab, EditAction::Undo, ms(base, 10));
        assert_eq!(tab.text, "ab");
    }

    #[test]
    fn with_no_tab_it_renders_base() {
        let mut view = BufferView::default();
        let mut buf = Buffer::empty(Rect::new(0, 0, 8, 2));
        for y in 0..2 {
            for x in 0..8 {
                buf[(x, y)].set_symbol("x");
            }
        }
        let mut hits = HitMap::default();
        let caret = view.render(
            None,
            Rect::new(0, 0, GUTTER, 2),
            Rect::new(GUTTER, 0, 4, 2),
            &mut buf,
            &mut hits,
        );
        assert_eq!(caret, None);
        assert_eq!(row_text(&buf, 0), "        ");
        assert_eq!(row_text(&buf, 1), "        ");
        assert_eq!(buf[(0, 0)].style().bg, Some(theme::color(BASE)));
        assert_eq!(buf[(6, 1)].style().bg, Some(theme::color(BASE)));
        assert_eq!(hits.hit(5, 1), Some(Target::Editor));
    }

    #[test]
    fn common_affixes_do_not_overlap() {
        assert_eq!(common_affixes("aaa", "aaaa"), (3, 3, 0));
        assert_eq!(common_affixes("abXcd", "abcd"), (2, 2, 2));
        assert_eq!(common_affixes("é1é", "é2é"), (2, 1, 2));
    }
}
