//! The find bar under the tab bar: a find row, any regex-error rows,
//! and an optional replace row.
//!
//! The core owns the text of both fields
//! ([`Editor::search_pattern_mut`] / [`Editor::search_replacement_mut`]);
//! the [`FindBar`] owns only their cursors and horizontal scroll, and the
//! rects they were last drawn at.
//!
//! Input reaches a field through [`FindBar::handle_key`],
//! [`FindBar::handle_click`] and [`FindBar::handle_paste`]; each key or
//! paste answers with a [`FindReply`] saying what the app should do next.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use frext_core::editor::{Command, Editor, SearchOption, SearchState, StepDirection};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use unicode_width::UnicodeWidthStr;

use crate::hit::{HitMap, Target};
use crate::paint;
use crate::text_input::{InputOutcome, TextInput};
use crate::theme;

/// One of the find bar's two text fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindField {
    /// The find field.
    Pattern,
    /// The replace "With" field.
    Replacement,
}

impl FindField {
    /// The other field.
    const fn other(self) -> Self {
        match self {
            Self::Pattern => Self::Replacement,
            Self::Replacement => Self::Pattern,
        }
    }

    /// The text of this field, which core owns.
    fn text(self, editor: &Editor) -> &str {
        match self {
            Self::Pattern => &editor.search().query().pattern,
            Self::Replacement => editor.search().replacement(),
        }
    }

    /// The text of this field, for editing.
    fn text_mut(self, editor: &mut Editor) -> &mut String {
        match self {
            Self::Pattern => editor.search_pattern_mut(),
            Self::Replacement => editor.search_replacement_mut(),
        }
    }
}

/// What the find bar made of a key or a paste.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FindReply {
    /// A command for the app to send.
    Send(Command),
    /// Move the focus to this field.
    Focus(FindField),
    /// The input was consumed; nothing else to do.
    Handled,
    /// The input is not for the find bar.
    Ignored,
}

/// Whether a toggle button is on or off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Toggle {
    On,
    Off,
}

impl Toggle {
    /// The toggle for a state core reports as a `bool`.
    const fn from_state(on: bool) -> Self {
        if on { Self::On } else { Self::Off }
    }
}

/// The kind of a find-bar button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ButtonKind {
    /// A button that acts when pushed.
    Push,
    /// A button that shows an on/off state.
    Toggle(Toggle),
}

/// Whether `key` switches between the fields: Tab or `BackTab`, each
/// without modifiers or with only Shift.
fn is_field_switch(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Tab | KeyCode::BackTab)
        && (key.modifiers == KeyModifiers::NONE || key.modifiers == KeyModifiers::SHIFT)
}

/// Whether `key` is Enter with Ctrl or Alt held, which the find bar
/// reserves.
fn is_modified_enter(key: KeyEvent) -> bool {
    key.code == KeyCode::Enter
        && key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
}

/// The most regex-error rows the find bar shows.
const MAX_ERROR_ROWS: u16 = 4;

/// The width of the match-count slot on the find row. A longer count
/// label is cut to it.
const COUNT_WIDTH: u16 = 11;

/// Every cell of the find row except the find field.
const FIXED_WIDTH: u16 = 66;

/// The narrowest the fields get (before clipping to the row).
const MIN_FIELD_WIDTH: u16 = 10;

/// The widest the fields get.
const MAX_FIELD_WIDTH: u16 = 40;

/// The placeholder drawn over an empty find field.
const PATTERN_HINT: &str = "search...";

/// The placeholder drawn over an empty "With" field.
const REPLACEMENT_HINT: &str = "replacement...";

/// The placeholder drawn over an empty "With" field while regex is on.
const REGEX_REPLACEMENT_HINT: &str = "replacement ($1 for groups)...";

/// The label in front of the find field.
const FIND_LABEL: &str = " Find ";

/// The label in front of the "With" field.
const WITH_LABEL: &str = " With ";

/// The find-row buttons between the find field and the match count, in
/// row order.
const FIND_BUTTONS_BEFORE_COUNT: [(&str, Target); 6] = [
    ("[Aa]", Target::FindOption(SearchOption::CaseSensitive)),
    ("[W]", Target::FindOption(SearchOption::WholeWord)),
    ("[.*]", Target::FindOption(SearchOption::Regex)),
    ("[sel]", Target::FindOption(SearchOption::InSelection)),
    ("[Prev]", Target::FindPrev),
    ("[Next]", Target::FindNext),
];

/// The find-row buttons after the match count, in row order.
const FIND_BUTTONS_AFTER_COUNT: [(&str, Target); 2] = [
    ("[Replace]", Target::FindReplaceRow),
    ("[x]", Target::FindClose),
];

/// The width of both fields on a find bar `width` columns wide.
fn field_width(width: u16) -> u16 {
    width
        .saturating_sub(FIXED_WIDTH)
        .clamp(MIN_FIELD_WIDTH, MAX_FIELD_WIDTH)
}

/// Keeps at most [`MAX_ERROR_ROWS`] lines: the first ones and the last.
fn cap_lines(mut lines: Vec<String>) -> Vec<String> {
    let max = usize::from(MAX_ERROR_ROWS);
    if lines.len() > max {
        let last = lines.pop();
        lines.truncate(max.saturating_sub(1));
        lines.extend(last);
    }
    lines
}

/// The regex-error rows of the find bar (none without an error).
#[must_use]
pub(crate) fn error_lines(search: &SearchState) -> Vec<String> {
    search.error().map_or_else(Vec::new, |err| {
        cap_lines(format!("regex: {err}").lines().map(str::to_owned).collect())
    })
}

/// The number of rows the find bar takes: 0 while it is closed,
/// otherwise the find row, the error rows and the replace row (when
/// shown).
#[must_use]
pub fn height(search: &SearchState) -> u16 {
    if !search.is_open() {
        return 0;
    }
    let errors = u16::try_from(error_lines(search).len()).unwrap_or(u16::MAX);
    1u16.saturating_add(errors)
        .saturating_add(u16::from(search.replace_open()))
}

/// The display width of `text`, saturated to a `u16`.
fn text_width(text: &str) -> u16 {
    u16::try_from(UnicodeWidthStr::width(text)).unwrap_or(u16::MAX)
}

/// The kind of the find-row button that pushes `target`: the option
/// toggles and the replace-row toggle show their state, the rest are
/// push buttons.
fn find_button_kind(search: &SearchState, target: Target) -> ButtonKind {
    match target {
        Target::FindOption(option) => ButtonKind::Toggle(Toggle::from_state(search.option(option))),
        Target::FindReplaceRow => ButtonKind::Toggle(Toggle::from_state(search.replace_open())),
        _ => ButtonKind::Push,
    }
}

/// A field drawn on a row: its rect and the cursor `TextInput` returned.
struct DrawnField {
    rect: Rect,
    cursor: Option<Position>,
}

/// One row of the find bar, laid out left to right.
///
/// Once an item does not fit, the row is clipped: nothing after it is
/// drawn.
struct Row<'a> {
    buf: &'a mut Buffer,
    hits: &'a mut HitMap,
    hovered: Option<Target>,
    y: u16,
    x: u16,
    right: u16,
    clipped: bool,
}

impl<'a> Row<'a> {
    const fn new(
        area: Rect,
        y: u16,
        hovered: Option<Target>,
        buf: &'a mut Buffer,
        hits: &'a mut HitMap,
    ) -> Self {
        Self {
            buf,
            hits,
            hovered,
            y,
            x: area.x,
            right: area.right(),
            clipped: false,
        }
    }

    const fn room(&self) -> u16 {
        self.right.saturating_sub(self.x)
    }

    /// Draws a plain label, cut to the row.
    fn label(&mut self, text: &str) {
        let room = self.room();
        if room > 0 {
            self.buf
                .set_stringn(self.x, self.y, text, usize::from(room), theme::chrome());
        }
        self.x = self.x.saturating_add(text_width(text));
    }

    /// Takes a `width`-cell slot after a one-cell gap, or clips the row
    /// if the slot does not fit whole.
    const fn slot(&mut self, width: u16) -> Option<Rect> {
        if self.clipped {
            return None;
        }
        let Some(start) = self.x.checked_add(1) else {
            self.clipped = true;
            return None;
        };
        match start.checked_add(width) {
            Some(end) if end <= self.right => {
                self.x = end;
                Some(Rect::new(start, self.y, width, 1))
            }
            _ => {
                self.clipped = true;
                None
            }
        }
    }

    /// Draws a button labelled `label` that pushes `target`. An on toggle
    /// uses [`theme::toggle_on`]; any other button uses the chrome, with
    /// the hover background while `target` is hovered.
    fn button(&mut self, label: &str, target: Target, kind: ButtonKind) {
        let Some(rect) = self.slot(text_width(label)) else {
            return;
        };
        let style = match kind {
            ButtonKind::Toggle(Toggle::On) => theme::toggle_on(),
            ButtonKind::Push | ButtonKind::Toggle(Toggle::Off) => {
                if self.hovered == Some(target) {
                    theme::chrome().bg(theme::hover_bg())
                } else {
                    theme::chrome()
                }
            }
        };
        self.buf
            .set_stringn(rect.x, rect.y, label, usize::from(rect.width), style);
        self.hits.push(rect, target);
    }

    /// Draws the match count in its slot.
    fn match_count(&mut self, label: &str) {
        let Some(rect) = self.slot(COUNT_WIDTH) else {
            return;
        };
        self.buf.set_style(rect, theme::status());
        self.buf.set_stringn(
            rect.x,
            rect.y,
            label,
            usize::from(rect.width),
            theme::status(),
        );
    }

    /// Draws a text field of up to `width` cells, cut to the row, with
    /// `hint` over it while `text` is empty.
    fn field(
        &mut self,
        width: u16,
        input: &mut TextInput,
        text: &str,
        hint: &str,
        target: Target,
    ) -> Option<DrawnField> {
        let width = width.min(self.room());
        if self.clipped || width == 0 {
            self.clipped = true;
            return None;
        }
        let rect = Rect::new(self.x, self.y, width, 1);
        self.x = self.x.saturating_add(width);
        let cursor = input.render(text, rect, self.buf);
        if text.is_empty() {
            self.buf.set_stringn(
                rect.x,
                rect.y,
                hint,
                usize::from(width),
                theme::input_hint(),
            );
        }
        self.hits.push(rect, target);
        Some(DrawnField { rect, cursor })
    }
}

/// The view state of the find bar.
#[derive(Debug, Default)]
pub struct FindBar {
    pattern: TextInput,
    replacement: TextInput,
    pattern_rect: Option<Rect>,
    replacement_rect: Option<Rect>,
}

impl FindBar {
    /// A find bar with both cursors at the start.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The input of `field`.
    const fn input_mut(&mut self, field: FindField) -> &mut TextInput {
        match field {
            FindField::Pattern => &mut self.pattern,
            FindField::Replacement => &mut self.replacement,
        }
    }

    /// Where `field` was last drawn, if it was.
    const fn rect(&self, field: FindField) -> Option<Rect> {
        match field {
            FindField::Pattern => self.pattern_rect,
            FindField::Replacement => self.replacement_rect,
        }
    }

    /// Whether `field` was drawn by the last [`FindBar::render`].
    #[must_use]
    pub const fn is_drawn(&self, field: FindField) -> bool {
        self.rect(field).is_some()
    }

    /// Handles one key event in the focused `field`.
    ///
    /// Releases are [`FindReply::Ignored`]. Tab, `BackTab` and Shift+Tab
    /// switch fields while the replace row is shown, and are
    /// [`FindReply::Handled`] without it. Enter with Ctrl or Alt held is
    /// reserved: [`FindReply::Handled`] in either field. Every other key
    /// goes to the field's [`TextInput`]: an edit of the find field
    /// recomputes the matches, Enter in the find field steps to the next
    /// match (the previous one with Shift), Enter in the "With" field
    /// replaces the current match, and Shift+Enter there is
    /// [`FindReply::Handled`].
    pub fn handle_key(
        &mut self,
        editor: &mut Editor,
        field: FindField,
        key: KeyEvent,
    ) -> FindReply {
        if key.kind == KeyEventKind::Release {
            return FindReply::Ignored;
        }
        if is_field_switch(key) {
            return if editor.search().replace_open() {
                FindReply::Focus(field.other())
            } else {
                FindReply::Handled
            };
        }
        if is_modified_enter(key) {
            return FindReply::Handled;
        }
        let outcome = self
            .input_mut(field)
            .handle_key(field.text_mut(editor), key);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match (field, outcome) {
            (FindField::Pattern, InputOutcome::Edited) => {
                FindReply::Send(Command::RecomputeMatches)
            }
            (FindField::Pattern, InputOutcome::Submit) => {
                let direction = if shift {
                    StepDirection::Backward
                } else {
                    StepDirection::Forward
                };
                FindReply::Send(Command::StepMatch(direction))
            }
            (FindField::Replacement, InputOutcome::Submit) if !shift => {
                FindReply::Send(Command::ReplaceCurrent)
            }
            (FindField::Replacement, InputOutcome::Edited | InputOutcome::Submit)
            | (_, InputOutcome::Moved) => FindReply::Handled,
            // Unreachable through the app: while the search is open the
            // keymap binds Esc to `CloseSearch` before the find bar sees
            // it. Kept for direct callers.
            (_, InputOutcome::Cancel) => FindReply::Send(Command::CloseSearch),
            (_, InputOutcome::Ignored) => FindReply::Ignored,
        }
    }

    /// Moves `field`'s cursor to the end of its text, for when it takes
    /// the focus.
    pub fn focus_field(&mut self, editor: &Editor, field: FindField) {
        self.input_mut(field).move_to_end(field.text(editor));
    }

    /// Places `field`'s cursor for a click at terminal `column`, relative
    /// to where the field was last drawn. Does nothing if it was not
    /// drawn.
    pub fn handle_click(&mut self, editor: &Editor, field: FindField, column: u16) {
        if let Some(rect) = self.rect(field) {
            self.input_mut(field)
                .click(field.text(editor), column.saturating_sub(rect.x));
        }
    }

    /// Inserts pasted `text` at `field`'s cursor, with line breaks turned
    /// into spaces. A paste into the find field recomputes the matches.
    pub fn handle_paste(&mut self, editor: &mut Editor, field: FindField, text: &str) -> FindReply {
        self.input_mut(field)
            .insert_str(field.text_mut(editor), text);
        match field {
            FindField::Pattern => FindReply::Send(Command::RecomputeMatches),
            FindField::Replacement => FindReply::Handled,
        }
    }

    /// Draws the find bar into `area` and records its hit regions.
    ///
    /// Draws nothing while the search is closed. `focus` names the field
    /// whose cursor is returned, and an off button whose target is
    /// `hovered` gets the hover background. When `area` has fewer rows
    /// than [`height`] asks for, error rows are cut first, so the replace
    /// row stays drawn as long as there is a row for it.
    pub fn render(
        &mut self,
        editor: &Editor,
        focus: Option<FindField>,
        hovered: Option<Target>,
        area: Rect,
        buf: &mut Buffer,
        hits: &mut HitMap,
    ) -> Option<Position> {
        self.pattern_rect = None;
        self.replacement_rect = None;
        let search = editor.search();
        let area = area.intersection(buf.area);
        if !search.is_open() || area.is_empty() {
            return None;
        }
        paint::fill(buf, area, theme::chrome());

        let mut y = area.y;
        let pattern_cursor = self.render_find_row(search, hovered, area, y, buf, hits);
        let error_room = area
            .height
            .saturating_sub(1)
            .saturating_sub(u16::from(search.replace_open()));
        for line in error_lines(search).iter().take(usize::from(error_room)) {
            y = y.saturating_add(1);
            let room = area.width.saturating_sub(2);
            if room > 0 {
                buf.set_stringn(
                    area.x.saturating_add(1),
                    y,
                    line,
                    usize::from(room),
                    theme::status_error(),
                );
            }
        }
        let mut replacement_cursor = None;
        if search.replace_open() {
            y = y.saturating_add(1);
            if y < area.bottom() {
                replacement_cursor = self.render_replace_row(search, hovered, area, y, buf, hits);
            }
        }
        match focus? {
            FindField::Pattern => pattern_cursor,
            FindField::Replacement => replacement_cursor,
        }
    }

    /// Draws the find row at row `y` and returns the find field's cursor.
    fn render_find_row(
        &mut self,
        search: &SearchState,
        hovered: Option<Target>,
        area: Rect,
        y: u16,
        buf: &mut Buffer,
        hits: &mut HitMap,
    ) -> Option<Position> {
        let mut row = Row::new(area, y, hovered, buf, hits);
        row.label(FIND_LABEL);
        let field = row.field(
            field_width(area.width),
            &mut self.pattern,
            &search.query().pattern,
            PATTERN_HINT,
            Target::FindPattern,
        );
        for (label, target) in FIND_BUTTONS_BEFORE_COUNT {
            row.button(label, target, find_button_kind(search, target));
        }
        row.match_count(&search.count_label());
        for (label, target) in FIND_BUTTONS_AFTER_COUNT {
            row.button(label, target, find_button_kind(search, target));
        }

        let field = field?;
        self.pattern_rect = Some(field.rect);
        field.cursor
    }

    /// Draws the replace row at row `y` and returns the "With" field's
    /// cursor.
    fn render_replace_row(
        &mut self,
        search: &SearchState,
        hovered: Option<Target>,
        area: Rect,
        y: u16,
        buf: &mut Buffer,
        hits: &mut HitMap,
    ) -> Option<Position> {
        let hint = if search.query().regex {
            REGEX_REPLACEMENT_HINT
        } else {
            REPLACEMENT_HINT
        };
        let mut row = Row::new(area, y, hovered, buf, hits);
        row.label(WITH_LABEL);
        let field = row.field(
            field_width(area.width),
            &mut self.replacement,
            search.replacement(),
            hint,
            Target::FindReplacement,
        );
        row.button("[Replace]", Target::FindReplaceCurrent, ButtonKind::Push);
        row.button("[Replace all]", Target::FindReplaceAll, ButtonKind::Push);

        let field = field?;
        self.replacement_rect = Some(field.rect);
        field.cursor
    }
}

#[cfg(test)]
mod tests {
    // `unwrap` is acceptable in test code: a panic on an unexpected `Err`
    // is exactly the failure signal we want from a test.
    #![allow(clippy::unwrap_used)]

    use std::fs;
    use std::path::PathBuf;

    use frext_core::clipboard::MemoryClipboard;
    use frext_core::editor::{Command, SearchBar};
    use frext_core::persistence::Store;
    use ratatui::style::Style;

    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "frext-term-find-bar-test-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// An editor over a file holding `text`, with the search closed.
    fn fixture(tag: &str, text: &str) -> (PathBuf, Editor) {
        let dir = temp_dir(tag);
        let file = dir.join("notes.txt");
        fs::write(&file, text).unwrap();
        let editor = Editor::open(
            Store::at(dir.join("state")).unwrap(),
            Box::new(MemoryClipboard::default()),
            &[file],
            None,
        );
        (dir, editor)
    }

    fn find(editor: &mut Editor, pattern: &str) {
        pattern.clone_into(editor.search_pattern_mut());
        editor.apply(Command::RecomputeMatches);
    }

    fn draw(
        bar: &mut FindBar,
        editor: &Editor,
        focus: Option<FindField>,
        hovered: Option<Target>,
        width: u16,
        height: u16,
    ) -> (Buffer, HitMap, Option<Position>) {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        let mut hits = HitMap::default();
        let cursor = bar.render(editor, focus, hovered, area, &mut buf, &mut hits);
        (buf, hits, cursor)
    }

    fn row_text(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
    }

    /// The colours and modifiers of `style`, ignoring the underline
    /// colour a buffer cell always carries.
    fn look(
        style: Style,
    ) -> (
        Option<ratatui::style::Color>,
        Option<ratatui::style::Color>,
        ratatui::style::Modifier,
    ) {
        (style.fg, style.bg, style.add_modifier)
    }

    /// One item of the find row as drawn at 80 columns.
    struct Item {
        x: u16,
        width: u16,
        /// The hit target; `None` for the match count.
        target: Option<Target>,
        /// The button label; `None` for the field and the match count.
        label: Option<&'static str>,
    }

    /// Every find-row item at 80 columns, in row order: the find field
    /// after the label, then the buttons and the match count, each after
    /// a one-cell gap, with widths taken from the labels.
    fn find_row_80() -> Vec<Item> {
        let mut items = vec![Item {
            x: text_width(FIND_LABEL),
            width: field_width(80),
            target: Some(Target::FindPattern),
            label: None,
        }];
        let mut push = |width: u16, target: Option<Target>, label: Option<&'static str>| {
            let last = items.last().unwrap();
            let x = last.x + last.width + 1;
            items.push(Item {
                x,
                width,
                target,
                label,
            });
        };
        for (label, target) in FIND_BUTTONS_BEFORE_COUNT {
            push(text_width(label), Some(target), Some(label));
        }
        push(COUNT_WIDTH, None, None);
        for (label, target) in FIND_BUTTONS_AFTER_COUNT {
            push(text_width(label), Some(target), Some(label));
        }
        items
    }

    /// The column of the match count at 80 columns.
    fn count_column_80() -> u16 {
        find_row_80()
            .iter()
            .find(|item| item.target.is_none())
            .unwrap()
            .x
    }

    /// The column of the find-row item for `target` at 80 columns.
    fn column_80(target: Target) -> u16 {
        find_row_80()
            .iter()
            .find(|item| item.target == Some(target))
            .unwrap()
            .x
    }

    #[test]
    fn fixed_width_is_the_labels_and_the_count() {
        let buttons: u16 = FIND_BUTTONS_BEFORE_COUNT
            .iter()
            .chain(&FIND_BUTTONS_AFTER_COUNT)
            .map(|(label, _)| 1 + text_width(label))
            .sum();
        assert_eq!(
            text_width(FIND_LABEL) + buttons + 1 + COUNT_WIDTH,
            FIXED_WIDTH
        );
    }

    #[test]
    fn find_row_80_fills_the_row() {
        let items = find_row_80();
        let last = items.last().unwrap();
        assert_eq!(last.x + last.width, 80);
        assert_eq!(items.first().unwrap().width, 14);
    }

    #[test]
    fn field_width_clamps() {
        assert_eq!(field_width(0), 10);
        assert_eq!(field_width(76), 10);
        assert_eq!(field_width(80), 14);
        assert_eq!(field_width(106), 40);
        assert_eq!(field_width(200), 40);
    }

    #[test]
    fn height_counts_rows() {
        let (dir, mut editor) = fixture("height", "hello");
        assert_eq!(height(editor.search()), 0);
        editor.apply(Command::OpenSearch(SearchBar::Find));
        assert_eq!(height(editor.search()), 1);
        editor.apply(Command::OpenSearch(SearchBar::FindReplace));
        assert_eq!(height(editor.search()), 2);
        editor.apply(Command::OpenSearch(SearchBar::Find));
        editor.apply(Command::ToggleSearchOption(SearchOption::Regex));
        find(&mut editor, "(");
        let lines = error_lines(editor.search());
        assert!(!lines.is_empty());
        assert_eq!(
            height(editor.search()),
            1 + u16::try_from(lines.len()).unwrap()
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn error_lines_format_the_regex_error() {
        let (dir, mut editor) = fixture("errors", "hello");
        editor.apply(Command::OpenSearch(SearchBar::Find));
        assert!(error_lines(editor.search()).is_empty());
        editor.apply(Command::ToggleSearchOption(SearchOption::Regex));
        find(&mut editor, "(");
        let lines = error_lines(editor.search());
        assert!(
            lines
                .first()
                .is_some_and(|line| line.starts_with("regex: invalid regular expression:")),
            "{lines:?}"
        );
        assert!(lines.len() <= usize::from(MAX_ERROR_ROWS));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cap_lines_keeps_the_first_ones_and_the_last() {
        let lines = |n: usize| (0..n).map(|i| i.to_string()).collect::<Vec<_>>();
        let max = usize::from(MAX_ERROR_ROWS);
        let capped = |n: usize| {
            let mut expected = lines(max - 1);
            expected.push((n - 1).to_string());
            expected
        };
        assert_eq!(cap_lines(lines(max + 2)), capped(max + 2));
        assert_eq!(cap_lines(lines(max + 1)), capped(max + 1));
        assert_eq!(cap_lines(lines(5)), capped(5));
        assert_eq!(cap_lines(lines(max)), lines(max));
        assert_eq!(cap_lines(lines(0)), lines(0));
    }

    #[test]
    fn find_row_at_80_columns() {
        let (dir, mut editor) = fixture("row80", "hello hello");
        editor.apply(Command::OpenSearch(SearchBar::Find));
        find(&mut editor, "hello");
        let (buf, _, _) = draw(&mut FindBar::new(), &editor, None, None, 80, 1);
        let row = row_text(&buf, 0);
        let expected = format!(
            " Find {:<14} [Aa] [W] [.*] [sel] [Prev] [Next] {:<11} [Replace] [x]",
            "hello", "1/2"
        );
        assert_eq!(row, expected);
        assert_eq!(row.chars().count(), 80);
        let at =
            |x: u16, len: usize| -> String { row.chars().skip(usize::from(x)).take(len).collect() };
        for item in find_row_80() {
            let text = at(item.x, usize::from(item.width));
            match (item.target, item.label) {
                (Some(_), Some(label)) => assert_eq!(text, label, "at {}", item.x),
                (Some(_), None) => assert_eq!(text.trim_end(), "hello", "at {}", item.x),
                (None, _) => assert_eq!(text.trim_end(), "1/2", "at {}", item.x),
            }
        }
        assert_eq!(buf[(count_column_80(), 0)].style().fg, theme::status().fg);
        assert_eq!(buf[(0, 0)].style().bg, theme::chrome().bg);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn placeholder_shows_while_the_pattern_is_empty() {
        let (dir, mut editor) = fixture("hint", "hello");
        editor.apply(Command::OpenSearch(SearchBar::Find));
        let mut bar = FindBar::new();
        let (buf, _, _) = draw(&mut bar, &editor, None, None, 80, 1);
        assert!(row_text(&buf, 0).starts_with(" Find search..."));
        assert_eq!(buf[(6, 0)].style().fg, theme::input_hint().fg);
        assert_eq!(buf[(6, 0)].style().bg, theme::input_hint().bg);

        find(&mut editor, "wor");
        let (buf, _, _) = draw(&mut bar, &editor, None, None, 80, 1);
        assert!(row_text(&buf, 0).starts_with(" Find wor "));
        assert_eq!(buf[(6, 0)].style().fg, theme::input().fg);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn case_toggle_style_follows_the_query() {
        let (dir, mut editor) = fixture("toggle", "hello");
        editor.apply(Command::OpenSearch(SearchBar::Find));
        let mut bar = FindBar::new();
        let case = Target::FindOption(SearchOption::CaseSensitive);
        let x = column_80(case);
        let (buf, _, _) = draw(&mut bar, &editor, None, None, 80, 1);
        assert_eq!(look(buf[(x, 0)].style()), look(theme::chrome()));
        // An off toggle gets the hover background, which differs from on.
        let (buf, _, _) = draw(&mut bar, &editor, None, Some(case), 80, 1);
        let hovered = theme::chrome().bg(theme::hover_bg());
        assert_eq!(look(buf[(x, 0)].style()), look(hovered));
        assert_ne!(look(hovered), look(theme::toggle_on()));
        editor.apply(Command::ToggleSearchOption(SearchOption::CaseSensitive));
        let (buf, _, _) = draw(&mut bar, &editor, None, None, 80, 1);
        assert_eq!(look(buf[(x, 0)].style()), look(theme::toggle_on()));
        assert_eq!(look(buf[(x + 3, 0)].style()), look(theme::toggle_on()));
        // An on toggle keeps its style while hovered.
        let (buf, _, _) = draw(&mut bar, &editor, None, Some(case), 80, 1);
        assert_eq!(look(buf[(x, 0)].style()), look(theme::toggle_on()));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn replace_toggle_follows_the_replace_row() {
        let (dir, mut editor) = fixture("replace-toggle", "hello");
        editor.apply(Command::OpenSearch(SearchBar::Find));
        let mut bar = FindBar::new();
        let x = column_80(Target::FindReplaceRow);
        let (buf, _, _) = draw(&mut bar, &editor, None, None, 80, 2);
        assert_eq!(look(buf[(x, 0)].style()), look(theme::chrome()));
        editor.apply(Command::ToggleReplaceRow);
        let (buf, _, _) = draw(&mut bar, &editor, None, None, 80, 2);
        assert_eq!(look(buf[(x, 0)].style()), look(theme::toggle_on()));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn hovered_next_has_the_hover_background() {
        let (dir, mut editor) = fixture("hover", "hello");
        editor.apply(Command::OpenSearch(SearchBar::Find));
        let (buf, _, _) = draw(
            &mut FindBar::new(),
            &editor,
            None,
            Some(Target::FindNext),
            80,
            1,
        );
        let hovered = theme::chrome().bg(theme::hover_bg());
        let next = column_80(Target::FindNext);
        let prev = column_80(Target::FindPrev);
        assert_eq!(look(buf[(next, 0)].style()), look(hovered));
        assert_eq!(look(buf[(next + 5, 0)].style()), look(hovered));
        assert_eq!(look(buf[(next + 6, 0)].style()), look(theme::chrome()));
        assert_eq!(look(buf[(prev, 0)].style()), look(theme::chrome()));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn replace_row_lines_up_with_the_find_row() {
        let (dir, mut editor) = fixture("replace-row", "hello");
        editor.apply(Command::OpenSearch(SearchBar::FindReplace));
        let mut bar = FindBar::new();
        let (buf, hits, _) = draw(&mut bar, &editor, None, None, 80, 2);
        assert_eq!(
            row_text(&buf, 1),
            format!(
                " With {REPLACEMENT_HINT:<14} [Replace] [Replace all]{:36}",
                ""
            )
        );
        assert_eq!(bar.pattern_rect, Some(Rect::new(6, 0, 14, 1)));
        assert_eq!(bar.replacement_rect, Some(Rect::new(6, 1, 14, 1)));
        assert_eq!(hits.hit(6, 1), Some(Target::FindReplacement));
        assert_eq!(hits.hit(19, 1), Some(Target::FindReplacement));
        assert_eq!(hits.hit(20, 1), None);
        assert_eq!(hits.hit(21, 1), Some(Target::FindReplaceCurrent));
        assert_eq!(hits.hit(29, 1), Some(Target::FindReplaceCurrent));
        assert_eq!(hits.hit(30, 1), None);
        assert_eq!(hits.hit(31, 1), Some(Target::FindReplaceAll));
        assert_eq!(hits.hit(43, 1), Some(Target::FindReplaceAll));
        assert_eq!(hits.hit(44, 1), None);

        editor.apply(Command::ToggleSearchOption(SearchOption::Regex));
        let (buf, _, _) = draw(&mut bar, &editor, None, None, 80, 2);
        let field: String = row_text(&buf, 1).chars().skip(6).take(14).collect();
        let hint: String = REGEX_REPLACEMENT_HINT.chars().take(14).collect();
        assert_eq!(field, hint);
        assert_eq!(buf[(6, 1)].style().fg, theme::input_hint().fg);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn replacement_text_shows_instead_of_the_hint() {
        let (dir, mut editor) = fixture("replacement", "hello");
        editor.apply(Command::OpenSearch(SearchBar::FindReplace));
        "bye".clone_into(editor.search_replacement_mut());
        let (buf, _, _) = draw(&mut FindBar::new(), &editor, None, None, 80, 2);
        assert!(row_text(&buf, 1).starts_with(" With bye "));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn error_rows_sit_between_the_find_and_replace_rows() {
        let (dir, mut editor) = fixture("error-rows", "hello");
        editor.apply(Command::OpenSearch(SearchBar::FindReplace));
        editor.apply(Command::ToggleSearchOption(SearchOption::Regex));
        find(&mut editor, "(");
        let lines = error_lines(editor.search());
        let count = u16::try_from(lines.len()).unwrap();
        let (buf, hits, _) = draw(&mut FindBar::new(), &editor, None, None, 80, 8);
        assert!(row_text(&buf, 0).starts_with(" Find ("));
        for (y, line) in (1..).zip(&lines) {
            let row = row_text(&buf, y);
            let shown: String = line.chars().take(78).collect();
            assert!(row[1..].starts_with(&shown), "{row:?} vs {line:?}");
            assert_eq!(buf[(1, y)].style().fg, theme::status_error().fg);
        }
        assert!(row_text(&buf, count + 1).starts_with(" With "));
        assert_eq!(hits.hit(6, count + 1), Some(Target::FindReplacement));
        assert!(row_text(&buf, count + 2).trim().is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_short_bar_cuts_error_rows_before_the_replace_row() {
        let (dir, mut editor) = fixture("short", "hello");
        editor.apply(Command::OpenSearch(SearchBar::FindReplace));
        editor.apply(Command::ToggleSearchOption(SearchOption::Regex));
        find(&mut editor, "(");
        assert!(!error_lines(editor.search()).is_empty());
        let mut bar = FindBar::new();
        let (buf, hits, cursor) =
            draw(&mut bar, &editor, Some(FindField::Replacement), None, 80, 2);
        assert!(row_text(&buf, 0).starts_with(" Find ("));
        assert!(row_text(&buf, 1).starts_with(" With "));
        assert_eq!(hits.hit(6, 1), Some(Target::FindReplacement));
        assert!(bar.replacement_rect.unwrap().contains(cursor.unwrap()));

        let (buf, hits, cursor) =
            draw(&mut bar, &editor, Some(FindField::Replacement), None, 80, 1);
        assert!(row_text(&buf, 0).starts_with(" Find ("));
        assert_eq!(hits.hit(6, 0), Some(Target::FindPattern));
        assert_eq!(cursor, None);
        assert_eq!(bar.replacement_rect, None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn is_drawn_follows_the_last_render() {
        let (dir, mut editor) = fixture("is-drawn", "hello");
        let mut bar = FindBar::new();
        editor.apply(Command::OpenSearch(SearchBar::FindReplace));
        draw(&mut bar, &editor, None, None, 80, 2);
        assert!(bar.is_drawn(FindField::Pattern));
        assert!(bar.is_drawn(FindField::Replacement));

        draw(&mut bar, &editor, None, None, 80, 1);
        assert!(bar.is_drawn(FindField::Pattern));
        assert!(!bar.is_drawn(FindField::Replacement));

        editor.apply(Command::CloseSearch);
        draw(&mut bar, &editor, None, None, 80, 2);
        assert!(!bar.is_drawn(FindField::Pattern));
        assert!(!bar.is_drawn(FindField::Replacement));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn every_item_has_its_hit_region() {
        let (dir, mut editor) = fixture("hits", "hello");
        editor.apply(Command::OpenSearch(SearchBar::Find));
        let (_, hits, _) = draw(&mut FindBar::new(), &editor, None, None, 80, 1);
        let items = find_row_80();
        for item in &items {
            let x = item.x;
            assert_eq!(hits.hit(x, 0), item.target, "first cell at {x}");
            assert_eq!(
                hits.hit(x + item.width - 1, 0),
                item.target,
                "last cell at {x}"
            );
        }
        for pair in items.windows(2) {
            if let [before, after] = pair {
                for gap in before.x + before.width..after.x {
                    assert_eq!(hits.hit(gap, 0), None, "gap at {gap}");
                }
            }
        }
        for label in 0..text_width(FIND_LABEL) {
            assert_eq!(hits.hit(label, 0), None);
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn narrow_bars_clip_trailing_items() {
        let (dir, mut editor) = fixture("narrow", "hello");
        editor.apply(Command::OpenSearch(SearchBar::FindReplace));
        let mut bar = FindBar::new();
        let (buf, hits, _) = draw(&mut bar, &editor, None, None, 40, 2);
        assert_eq!(
            row_text(&buf, 0),
            format!(" Find {PATTERN_HINT:<10} [Aa] [W] [.*] [sel]{:4}", "")
        );
        let targets: Vec<_> = (0..40).filter_map(|x| hits.hit(x, 0)).collect();
        for missing in [
            Target::FindPrev,
            Target::FindNext,
            Target::FindReplaceRow,
            Target::FindClose,
        ] {
            assert!(!targets.contains(&missing), "{missing:?}");
        }
        assert!(targets.contains(&Target::FindOption(SearchOption::InSelection)));
        // The replace row's buttons still fit at 40.
        assert_eq!(hits.hit(27, 1), Some(Target::FindReplaceAll));

        for width in [0, 1, 3, 6, 7, 17] {
            for height in [0, 1, 3] {
                let (_, hits, cursor) = draw(
                    &mut bar,
                    &editor,
                    Some(FindField::Pattern),
                    None,
                    width,
                    height,
                );
                if width <= 6 {
                    assert_eq!(cursor, None);
                    assert_eq!(bar.pattern_rect, None);
                    assert_eq!(hits.hit(0, 0), None);
                } else if height >= 1 {
                    let rect = Rect::new(6, 0, (width - 6).min(MIN_FIELD_WIDTH), 1);
                    assert_eq!(bar.pattern_rect, Some(rect), "{width}x{height}");
                    let cursor = cursor.unwrap();
                    assert!(rect.contains(cursor), "{cursor:?} in {width}x{height}");
                }
            }
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cursor_is_in_the_focused_field() {
        let (dir, mut editor) = fixture("cursor", "hello");
        editor.apply(Command::OpenSearch(SearchBar::FindReplace));
        find(&mut editor, "hello");
        let mut bar = FindBar::new();
        let (_, _, cursor) = draw(&mut bar, &editor, Some(FindField::Pattern), None, 80, 2);
        let cursor = cursor.unwrap();
        assert!(bar.pattern_rect.unwrap().contains(cursor), "{cursor:?}");
        let (_, _, cursor) = draw(&mut bar, &editor, Some(FindField::Replacement), None, 80, 2);
        let cursor = cursor.unwrap();
        assert!(bar.replacement_rect.unwrap().contains(cursor), "{cursor:?}");
        let (_, _, cursor) = draw(&mut bar, &editor, None, None, 80, 2);
        assert_eq!(cursor, None);
        // The replace row is not drawn in a one-row area.
        let (_, _, cursor) = draw(&mut bar, &editor, Some(FindField::Replacement), None, 80, 1);
        assert_eq!(cursor, None);
        assert_eq!(bar.replacement_rect, None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn closed_bar_draws_nothing() {
        let (dir, editor) = fixture("closed", "hello");
        let mut bar = FindBar::new();
        let (buf, hits, cursor) = draw(&mut bar, &editor, Some(FindField::Pattern), None, 80, 2);
        assert_eq!(buf, Buffer::empty(Rect::new(0, 0, 80, 2)));
        assert_eq!(cursor, None);
        assert_eq!(hits.hit(6, 0), None);
        assert_eq!(bar.pattern_rect, None);
        fs::remove_dir_all(&dir).unwrap();
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn shift(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::SHIFT)
    }

    const RECOMPUTE: FindReply = FindReply::Send(Command::RecomputeMatches);

    #[test]
    fn typing_into_find_recomputes_each_time() {
        let (dir, mut editor) = fixture("type-find", "hello");
        editor.apply(Command::OpenSearch(SearchBar::Find));
        let mut bar = FindBar::new();
        let reply = bar.handle_key(&mut editor, FindField::Pattern, press(KeyCode::Char('a')));
        assert_eq!(reply, RECOMPUTE);
        assert_eq!(editor.search().query().pattern, "a");
        let reply = bar.handle_key(&mut editor, FindField::Pattern, press(KeyCode::Char('b')));
        assert_eq!(reply, RECOMPUTE);
        assert_eq!(editor.search().query().pattern, "ab");

        let reply = bar.handle_key(&mut editor, FindField::Pattern, press(KeyCode::Backspace));
        assert_eq!(reply, RECOMPUTE);
        assert_eq!(editor.search().query().pattern, "a");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cursor_keys_in_find_are_handled() {
        let (dir, mut editor) = fixture("move-find", "hello");
        editor.apply(Command::OpenSearch(SearchBar::Find));
        find(&mut editor, "abc");
        let mut bar = FindBar::new();
        bar.focus_field(&editor, FindField::Pattern);
        for code in [KeyCode::Left, KeyCode::Home] {
            let reply = bar.handle_key(&mut editor, FindField::Pattern, press(code));
            assert_eq!(reply, FindReply::Handled, "{code:?}");
        }
        assert_eq!(editor.search().query().pattern, "abc");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn enter_in_find_steps_through_matches() {
        let (dir, mut editor) = fixture("enter-find", "hello");
        editor.apply(Command::OpenSearch(SearchBar::Find));
        let mut bar = FindBar::new();
        assert_eq!(
            bar.handle_key(&mut editor, FindField::Pattern, press(KeyCode::Enter)),
            FindReply::Send(Command::StepMatch(StepDirection::Forward))
        );
        assert_eq!(
            bar.handle_key(&mut editor, FindField::Pattern, shift(KeyCode::Enter)),
            FindReply::Send(Command::StepMatch(StepDirection::Backward))
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn enter_in_with_replaces_the_current_match() {
        let (dir, mut editor) = fixture("enter-with", "hello");
        editor.apply(Command::OpenSearch(SearchBar::FindReplace));
        let mut bar = FindBar::new();
        assert_eq!(
            bar.handle_key(&mut editor, FindField::Replacement, press(KeyCode::Enter)),
            FindReply::Send(Command::ReplaceCurrent)
        );
        assert_eq!(
            bar.handle_key(&mut editor, FindField::Replacement, shift(KeyCode::Enter)),
            FindReply::Handled
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn typing_into_with_edits_the_replacement() {
        let (dir, mut editor) = fixture("type-with", "hello");
        editor.apply(Command::OpenSearch(SearchBar::FindReplace));
        let mut bar = FindBar::new();
        for c in ['b', 'y', 'e'] {
            let reply =
                bar.handle_key(&mut editor, FindField::Replacement, press(KeyCode::Char(c)));
            assert_eq!(reply, FindReply::Handled);
        }
        assert_eq!(editor.search().replacement(), "bye");
        assert_eq!(editor.search().query().pattern, "");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn tab_switches_fields_only_with_the_replace_row() {
        let (dir, mut editor) = fixture("tab", "hello");
        let keys = [
            press(KeyCode::Tab),
            press(KeyCode::BackTab),
            shift(KeyCode::BackTab),
            shift(KeyCode::Tab),
        ];
        let mut bar = FindBar::new();
        editor.apply(Command::OpenSearch(SearchBar::FindReplace));
        for key in keys {
            assert_eq!(
                bar.handle_key(&mut editor, FindField::Pattern, key),
                FindReply::Focus(FindField::Replacement),
                "{key:?}"
            );
            assert_eq!(
                bar.handle_key(&mut editor, FindField::Replacement, key),
                FindReply::Focus(FindField::Pattern),
                "{key:?}"
            );
        }
        editor.apply(Command::OpenSearch(SearchBar::Find));
        for key in keys {
            assert_eq!(
                bar.handle_key(&mut editor, FindField::Pattern, key),
                FindReply::Handled,
                "{key:?}"
            );
        }
        assert_eq!(editor.search().query().pattern, "");
        assert_eq!(editor.search().replacement(), "");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn modified_tabs_do_not_switch_fields() {
        let (dir, mut editor) = fixture("ctrl-tab", "hello");
        editor.apply(Command::OpenSearch(SearchBar::FindReplace));
        let mut bar = FindBar::new();
        for code in [KeyCode::Tab, KeyCode::BackTab] {
            for modifiers in [
                KeyModifiers::CONTROL,
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
                KeyModifiers::ALT,
            ] {
                let key = KeyEvent::new(code, modifiers);
                for field in [FindField::Pattern, FindField::Replacement] {
                    let reply = bar.handle_key(&mut editor, field, key);
                    assert!(
                        !matches!(reply, FindReply::Focus(_)),
                        "{key:?} in {field:?}: {reply:?}"
                    );
                }
            }
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ctrl_and_alt_enter_are_reserved() {
        let (dir, mut editor) = fixture("ctrl-enter", "hello hello");
        editor.apply(Command::OpenSearch(SearchBar::FindReplace));
        find(&mut editor, "hello");
        let mut bar = FindBar::new();
        for modifiers in [
            KeyModifiers::CONTROL,
            KeyModifiers::ALT,
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        ] {
            let key = KeyEvent::new(KeyCode::Enter, modifiers);
            for field in [FindField::Pattern, FindField::Replacement] {
                assert_eq!(
                    bar.handle_key(&mut editor, field, key),
                    FindReply::Handled,
                    "{key:?} in {field:?}"
                );
            }
        }
        assert_eq!(editor.search().query().pattern, "hello");
        assert_eq!(editor.search().replacement(), "");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn esc_closes_the_search() {
        let (dir, mut editor) = fixture("esc", "hello");
        editor.apply(Command::OpenSearch(SearchBar::FindReplace));
        let mut bar = FindBar::new();
        for field in [FindField::Pattern, FindField::Replacement] {
            assert_eq!(
                bar.handle_key(&mut editor, field, press(KeyCode::Esc)),
                FindReply::Send(Command::CloseSearch)
            );
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn alt_chars_and_releases_are_ignored() {
        let (dir, mut editor) = fixture("ignored", "hello");
        editor.apply(Command::OpenSearch(SearchBar::FindReplace));
        let mut bar = FindBar::new();
        let alt = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT);
        let release = KeyEvent::new_with_kind(
            KeyCode::Char('r'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        );
        let tab_release =
            KeyEvent::new_with_kind(KeyCode::Tab, KeyModifiers::NONE, KeyEventKind::Release);
        for field in [FindField::Pattern, FindField::Replacement] {
            for key in [alt, release, tab_release] {
                assert_eq!(
                    bar.handle_key(&mut editor, field, key),
                    FindReply::Ignored,
                    "{key:?}"
                );
            }
        }
        assert_eq!(editor.search().query().pattern, "");
        assert_eq!(editor.search().replacement(), "");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn focus_field_puts_the_cursor_at_the_end() {
        let (dir, mut editor) = fixture("focus", "hello");
        editor.apply(Command::OpenSearch(SearchBar::Find));
        find(&mut editor, "hello");
        let mut bar = FindBar::new();
        bar.focus_field(&editor, FindField::Pattern);
        let (_, _, cursor) = draw(&mut bar, &editor, Some(FindField::Pattern), None, 80, 1);
        assert_eq!(cursor, Some(Position::new(6 + 5, 0)));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn click_places_the_cursor_after_a_render() {
        let (dir, mut editor) = fixture("click", "hello");
        editor.apply(Command::OpenSearch(SearchBar::Find));
        find(&mut editor, "hello");
        let mut bar = FindBar::new();
        // Without a rect the click does nothing.
        bar.handle_click(&editor, FindField::Pattern, 6 + 2);
        assert_eq!(bar.pattern.cursor(), 0);
        draw(&mut bar, &editor, None, None, 80, 1);
        bar.handle_click(&editor, FindField::Pattern, 6 + 2);
        let (_, _, cursor) = draw(&mut bar, &editor, Some(FindField::Pattern), None, 80, 1);
        assert_eq!(cursor.map(|c| c.x), Some(8));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn paste_turns_line_breaks_into_spaces() {
        let (dir, mut editor) = fixture("paste", "hello");
        editor.apply(Command::OpenSearch(SearchBar::FindReplace));
        let mut bar = FindBar::new();
        assert_eq!(
            bar.handle_paste(&mut editor, FindField::Pattern, "a\nb"),
            RECOMPUTE
        );
        assert_eq!(editor.search().query().pattern, "a b");
        assert_eq!(
            bar.handle_paste(&mut editor, FindField::Replacement, "x\r\ny"),
            FindReply::Handled
        );
        assert_eq!(editor.search().replacement(), "x y");
        fs::remove_dir_all(&dir).unwrap();
    }
}
