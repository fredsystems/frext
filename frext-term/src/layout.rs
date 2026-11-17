//! Splitting the terminal into the regions of the editor screen.

use ratatui::layout::Rect;

/// The regions of the editor screen. Regions that do not fit are
/// zero-size rects inside the screen area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Areas {
    /// The tab bar, one row at the top.
    pub tab_bar: Rect,
    /// The find bar under the tab bar, `find_rows` rows tall (0 when
    /// closed).
    pub find_bar: Rect,
    /// The file-tree sidebar on the left of the body.
    pub sidebar: Rect,
    /// The line-number gutter between the sidebar and the editor.
    pub gutter: Rect,
    /// The editor text.
    pub editor: Rect,
    /// The status line, one row at the bottom.
    pub status: Rect,
}

/// Splits `area` into the editor screen regions.
///
/// Rows, top to bottom: tab bar (1), find bar (`find_rows`, 0 when
/// closed), body (the rest), status line (1). The body splits left to right
/// into the sidebar (`sidebar_width`, or 0), the gutter (`gutter_width`)
/// and the editor (the rest). Widths and heights are clamped so every
/// region lies inside `area`.
#[must_use]
pub fn areas(area: Rect, find_rows: u16, sidebar_width: Option<u16>, gutter_width: u16) -> Areas {
    let mut rows_left = area.height;
    let mut take = |wanted: u16| {
        let got = wanted.min(rows_left);
        rows_left -= got;
        got
    };
    let tab_height = take(1);
    let status_height = take(1);
    let find_height = take(find_rows);
    let body_height = rows_left;

    let tab_bar = Rect::new(area.x, area.y, area.width, tab_height);
    let find_bar = Rect::new(area.x, tab_bar.bottom(), area.width, find_height);
    let body_y = find_bar.bottom();
    let status = Rect::new(area.x, body_y + body_height, area.width, status_height);

    let sidebar_w = sidebar_width.unwrap_or(0).min(area.width);
    let gutter_w = gutter_width.min(area.width - sidebar_w);
    let editor_w = area.width - sidebar_w - gutter_w;

    let sidebar = Rect::new(area.x, body_y, sidebar_w, body_height);
    let gutter = Rect::new(sidebar.right(), body_y, gutter_w, body_height);
    let editor = Rect::new(gutter.right(), body_y, editor_w, body_height);

    Areas {
        tab_bar,
        find_bar,
        sidebar,
        gutter,
        editor,
        status,
    }
}

/// The width of the line-number gutter for a buffer of `line_count`
/// lines: the digits of the largest line number (at least 2) plus one
/// column of padding.
#[must_use]
pub fn gutter_width(line_count: usize) -> u16 {
    let mut digits: u32 = 1;
    let mut rest = line_count / 10;
    while rest > 0 {
        digits += 1;
        rest /= 10;
    }
    u16::try_from(digits.max(2) + 1).unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inside(outer: Rect, inner: Rect) -> bool {
        inner.x >= outer.x
            && inner.y >= outer.y
            && inner.right() <= outer.right()
            && inner.bottom() <= outer.bottom()
    }

    fn all(a: &Areas) -> [Rect; 6] {
        [
            a.tab_bar, a.find_bar, a.sidebar, a.gutter, a.editor, a.status,
        ]
    }

    #[test]
    fn find_bar_closed() {
        let a = areas(Rect::new(0, 0, 80, 24), 0, None, 4);
        assert_eq!(a.tab_bar, Rect::new(0, 0, 80, 1));
        assert_eq!(a.find_bar.height, 0);
        assert_eq!(a.sidebar, Rect::new(0, 1, 0, 22));
        assert_eq!(a.gutter, Rect::new(0, 1, 4, 22));
        assert_eq!(a.editor, Rect::new(4, 1, 76, 22));
        assert_eq!(a.status, Rect::new(0, 23, 80, 1));
    }

    #[test]
    fn find_bar_open() {
        let a = areas(Rect::new(0, 0, 80, 24), 1, None, 4);
        assert_eq!(a.find_bar, Rect::new(0, 1, 80, 1));
        assert_eq!(a.editor, Rect::new(4, 2, 76, 21));
        assert_eq!(a.status, Rect::new(0, 23, 80, 1));
    }

    #[test]
    fn three_row_find_bar_pushes_editor_down() {
        let a = areas(Rect::new(0, 0, 80, 24), 3, None, 4);
        assert_eq!(a.find_bar, Rect::new(0, 1, 80, 3));
        assert_eq!(a.editor, Rect::new(4, 4, 76, 19));
        assert_eq!(a.status, Rect::new(0, 23, 80, 1));
    }

    #[test]
    fn find_bar_taller_than_space_left() {
        let area = Rect::new(0, 0, 80, 5);
        let a = areas(area, 6, None, 4);
        assert_eq!(a.tab_bar, Rect::new(0, 0, 80, 1));
        assert_eq!(a.find_bar, Rect::new(0, 1, 80, 3));
        assert_eq!(a.editor.height, 0);
        assert_eq!(a.status, Rect::new(0, 4, 80, 1));
        for r in all(&a) {
            assert!(inside(area, r), "{r:?}");
        }
    }

    #[test]
    fn sidebar_open() {
        let a = areas(Rect::new(0, 0, 80, 24), 0, Some(20), 3);
        assert_eq!(a.sidebar, Rect::new(0, 1, 20, 22));
        assert_eq!(a.gutter, Rect::new(20, 1, 3, 22));
        assert_eq!(a.editor, Rect::new(23, 1, 57, 22));
    }

    #[test]
    fn offset_area() {
        let area = Rect::new(5, 3, 40, 10);
        let a = areas(area, 1, Some(10), 3);
        for r in all(&a) {
            assert!(inside(area, r), "{r:?}");
        }
        assert_eq!(a.tab_bar, Rect::new(5, 3, 40, 1));
        assert_eq!(a.editor, Rect::new(18, 5, 27, 7));
        assert_eq!(a.status, Rect::new(5, 12, 40, 1));
    }

    #[test]
    fn widths_wider_than_body_are_clamped() {
        let area = Rect::new(0, 0, 10, 5);
        let a = areas(area, 0, Some(8), 5);
        assert_eq!(a.sidebar.width, 8);
        assert_eq!(a.gutter.width, 2);
        assert_eq!(a.editor.width, 0);
        let a = areas(area, 0, Some(50), 5);
        assert_eq!(a.sidebar.width, 10);
        assert_eq!(a.gutter.width, 0);
        assert_eq!(a.editor.width, 0);
    }

    #[test]
    fn one_by_one() {
        let area = Rect::new(0, 0, 1, 1);
        let a = areas(area, 1, Some(20), 4);
        assert_eq!(a.tab_bar, Rect::new(0, 0, 1, 1));
        assert_eq!(a.status.height, 0);
        assert_eq!(a.find_bar.height, 0);
        assert_eq!(a.editor.area(), 0);
        for r in all(&a) {
            assert!(inside(area, r), "{r:?}");
        }
    }

    #[test]
    fn zero_by_zero() {
        let area = Rect::new(0, 0, 0, 0);
        let a = areas(area, 1, Some(20), 4);
        for r in all(&a) {
            assert_eq!(r.area(), 0);
            assert!(inside(area, r), "{r:?}");
        }
    }

    #[test]
    fn every_area_within_bounds() {
        for width in 0..12 {
            for height in 0..6 {
                let area = Rect::new(2, 1, width, height);
                for find_rows in 0..=6 {
                    for sidebar in [None, Some(0), Some(3), Some(100)] {
                        for gutter in [0, 3, 100] {
                            let a = areas(area, find_rows, sidebar, gutter);
                            for r in all(&a) {
                                assert!(inside(area, r), "{area:?} {r:?}");
                            }
                            let rows = a.tab_bar.height
                                + a.find_bar.height
                                + a.editor.height
                                + a.status.height;
                            assert_eq!(rows, height);
                            let cols = a.sidebar.width + a.gutter.width + a.editor.width;
                            assert_eq!(cols, width);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn gutter_widths() {
        assert_eq!(gutter_width(0), 3);
        assert_eq!(gutter_width(1), 3);
        assert_eq!(gutter_width(9), 3);
        assert_eq!(gutter_width(10), 3);
        assert_eq!(gutter_width(99), 3);
        assert_eq!(gutter_width(100), 4);
        assert_eq!(gutter_width(100_000), 7);
    }
}
