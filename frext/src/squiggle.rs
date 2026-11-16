//! Wavy underlines ("squiggles") under proofreading diagnostics.
//!
//! egui's text underline is a straight stroke, so squiggles are painted over
//! the editor after it lays out, from its own galley: each diagnostic's char
//! range is mapped onto the galley's rows (so a range crossing a soft wrap
//! gets one squiggle per row) and drawn as a zig-zag line.

use std::ops::Range;

use eframe::egui::{self, Color32, Pos2, Stroke, epaint::text::PlacedRow};

/// Peak distance of the wave above/below its centre line, in points.
const AMPLITUDE: f32 = 1.5;
/// Horizontal distance between a peak and the next dip, in points.
const HALF_WAVELENGTH: f32 = 2.0;
/// Gap between the bottom of the row and the wave's centre line.
const BASELINE_OFFSET: f32 = 1.5;

/// The row geometry squiggle placement needs, abstracted from egui's
/// [`PlacedRow`] so the mapping logic can be tested with synthetic rows.
pub trait RowGeometry {
    /// Characters this row covers, including its trailing `\n`, if any.
    fn char_count(&self) -> usize;
    /// Characters with a glyph on this row (excludes the `\n`).
    fn glyph_count(&self) -> usize;
    /// Left and right x of glyph `index`, relative to the galley.
    fn glyph_span(&self, index: usize) -> Option<(f32, f32)>;
    /// The y of the row's bottom edge, relative to the galley.
    fn bottom(&self) -> f32;
}

impl RowGeometry for PlacedRow {
    fn char_count(&self) -> usize {
        self.char_count_including_newline().0
    }

    fn glyph_count(&self) -> usize {
        self.glyphs.len()
    }

    fn glyph_span(&self, index: usize) -> Option<(f32, f32)> {
        self.glyphs
            .get(index)
            .map(|g| (self.pos.x + g.pos.x, self.pos.x + g.max_x()))
    }

    fn bottom(&self) -> f32 {
        self.max_y()
    }
}

/// A horizontal stretch to squiggle, relative to the galley.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    /// Left edge.
    pub x0: f32,
    /// Right edge.
    pub x1: f32,
    /// The wave's centre line.
    pub y: f32,
}

/// The first char index of every row; computed once per frame and shared by
/// all diagnostics.
pub fn row_starts<R: RowGeometry>(rows: &[R]) -> Vec<usize> {
    let mut starts = Vec::with_capacity(rows.len());
    let mut next = 0;
    for row in rows {
        starts.push(next);
        next += row.char_count();
    }
    starts
}

/// Map the char range `chars` onto per-row segments. `starts` must come from
/// [`row_starts`] over the same `rows`.
pub fn segments<R: RowGeometry>(
    rows: &[R],
    starts: &[usize],
    chars: &Range<usize>,
) -> Vec<Segment> {
    let mut out = Vec::new();
    if chars.is_empty() {
        return out;
    }
    // The last row starting at or before `chars.start`.
    let first = starts
        .partition_point(|&s| s <= chars.start)
        .saturating_sub(1);
    for (row, &row_start) in rows.iter().zip(starts).skip(first) {
        if row_start >= chars.end {
            break;
        }
        let local_start = chars.start.saturating_sub(row_start);
        let local_end = (chars.end - row_start).min(row.glyph_count());
        if local_start >= local_end {
            continue;
        }
        let (Some((x0, _)), Some((_, x1))) =
            (row.glyph_span(local_start), row.glyph_span(local_end - 1))
        else {
            continue;
        };
        out.push(Segment {
            x0,
            x1,
            y: row.bottom() - BASELINE_OFFSET,
        });
    }
    out
}

/// Zig-zag points for one segment: alternating peaks and dips every
/// `HALF_WAVELENGTH`, ending exactly at `x1`.
#[must_use]
pub fn wave_points(segment: Segment) -> Vec<Pos2> {
    let mut points = Vec::new();
    let mut x = segment.x0;
    let mut up = true;
    while x < segment.x1 {
        let dy = if up { -AMPLITUDE } else { AMPLITUDE };
        points.push(Pos2::new(x, segment.y + dy));
        x += HALF_WAVELENGTH;
        up = !up;
    }
    let dy = if up { -AMPLITUDE } else { AMPLITUDE };
    points.push(Pos2::new(segment.x1, segment.y + dy));
    points
}

/// Paint a squiggle under each `(char range, colour)` onto `galley`, which
/// was drawn at `galley_pos`.
pub fn paint<'a>(
    painter: &egui::Painter,
    galley: &egui::Galley,
    galley_pos: Pos2,
    underlines: impl IntoIterator<Item = (&'a Range<usize>, Color32)>,
) {
    let rows = galley.rows.as_slice();
    let starts = row_starts(rows);
    let offset = galley_pos.to_vec2();
    for (chars, color) in underlines {
        for segment in segments(rows, &starts, chars) {
            let points = wave_points(segment)
                .into_iter()
                .map(|p| p + offset)
                .collect();
            painter.add(egui::Shape::line(points, Stroke::new(1.0, color)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic row of fixed-width (10pt) glyphs.
    struct Row {
        glyphs: usize,
        newline: bool,
        y: f32,
    }

    impl RowGeometry for Row {
        fn char_count(&self) -> usize {
            self.glyphs + usize::from(self.newline)
        }
        fn glyph_count(&self) -> usize {
            self.glyphs
        }
        fn glyph_span(&self, index: usize) -> Option<(f32, f32)> {
            (index < self.glyphs).then(|| {
                let left = 10.0 * f32::from(u16::try_from(index).unwrap_or(u16::MAX));
                (left, left + 10.0)
            })
        }
        fn bottom(&self) -> f32 {
            self.y + 20.0
        }
    }

    /// "hello world\nsecond" with the first line soft-wrapped after "hello ".
    fn rows() -> Vec<Row> {
        vec![
            Row {
                glyphs: 6,
                newline: false,
                y: 0.0,
            },
            Row {
                glyphs: 5,
                newline: true,
                y: 20.0,
            },
            Row {
                glyphs: 6,
                newline: false,
                y: 40.0,
            },
        ]
    }

    fn spans(chars: Range<usize>) -> Vec<(f32, f32, f32)> {
        let rows = rows();
        let starts = row_starts(&rows);
        segments(&rows, &starts, &chars)
            .into_iter()
            .map(|s| (s.x0, s.x1, s.y))
            .collect()
    }

    #[test]
    fn row_starts_include_newlines() {
        assert_eq!(row_starts(&rows()), vec![0, 6, 12]);
    }

    #[test]
    fn range_within_one_row_is_one_segment() {
        // "ell" in the first row.
        assert_eq!(spans(1..4), vec![(10.0, 40.0, 18.5)]);
    }

    #[test]
    fn range_on_a_later_row_uses_that_row() {
        // "sec" on the third row.
        assert_eq!(spans(12..15), vec![(0.0, 30.0, 58.5)]);
    }

    #[test]
    fn range_across_a_soft_wrap_splits_per_row() {
        // "lo wor" spans the wrap between rows one and two.
        assert_eq!(spans(3..9), vec![(30.0, 60.0, 18.5), (0.0, 30.0, 38.5)]);
    }

    #[test]
    fn newline_is_never_underlined() {
        // "world\n": the newline has no glyph, so the segment stops at "d".
        assert_eq!(spans(6..12), vec![(0.0, 50.0, 38.5)]);
    }

    #[test]
    fn empty_and_out_of_range_produce_nothing() {
        assert!(spans(4..4).is_empty());
        assert!(spans(100..105).is_empty());
    }

    #[test]
    fn wave_spans_the_segment_and_stays_within_amplitude() {
        let segment = Segment {
            x0: 10.0,
            x1: 25.0,
            y: 5.0,
        };
        let points = wave_points(segment);
        assert_eq!(points.first().map(|p| p.x), Some(10.0));
        assert_eq!(points.last().map(|p| p.x), Some(25.0));
        assert!(points.len() >= 4, "a 15pt squiggle has several peaks");
        for pair in points.windows(2) {
            assert!(pair[0].x < pair[1].x, "x strictly increases");
            assert_ne!(pair[0].y, pair[1].y, "peaks and dips alternate");
        }
        assert!(points.iter().all(|p| (p.y - 5.0).abs() <= AMPLITUDE));
    }
}
