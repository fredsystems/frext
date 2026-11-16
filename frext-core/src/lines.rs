//! Line counting and line-number gutter logic.
//!
//! These helpers are pure functions over text and row metadata, kept free of
//! any GUI toolkit so the numbering rules can be unit-tested in isolation and
//! shared by every front end.

/// Number of editor lines in `text`.
///
/// At least one row, even for an empty buffer. A trailing newline opens a new
/// (empty) final line in the editor, so this is `newlines + 1`.
#[must_use]
pub fn line_count(text: &str) -> usize {
    text.bytes().filter(|&b| b == b'\n').count() + 1
}

/// Decide which visual rows of a laid-out editor galley carry a line number.
///
/// `ends_with_newline` yields, in row order, each laid-out row's
/// `ends_with_newline` flag (as reported by the GUI's text layout). A row
/// begins a new logical line — and therefore gets the next line number — when
/// it is the first row or the previous row ended with a real `\n`. Rows
/// produced by soft wrapping (whose predecessor did *not* end with a newline)
/// are skipped, so continuation rows stay blank.
///
/// Returns `(row_index, line_number)` pairs, with `line_number` starting at 1.
/// Splitting this out from the painting keeps the wrap-vs-newline logic — the
/// part that previously mis-numbered a continuation row right after Enter —
/// unit-testable without a live UI.
#[must_use]
pub fn line_numbers_for_rows(
    ends_with_newline: impl IntoIterator<Item = bool>,
) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut starts_logical_line = true;
    let mut number = 0usize;
    for (row_index, row_ends_with_newline) in ends_with_newline.into_iter().enumerate() {
        if starts_logical_line {
            number += 1;
            out.push((row_index, number));
        }
        starts_logical_line = row_ends_with_newline;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_count_handles_empty_and_trailing_newlines() {
        assert_eq!(line_count(""), 1);
        assert_eq!(line_count("one line"), 1);
        assert_eq!(line_count("a\nb\nc"), 3);
        // A trailing newline opens a new empty final line.
        assert_eq!(line_count("a\nb\n"), 3);
    }

    #[test]
    fn line_numbers_unwrapped_rows_number_consecutively() {
        // Three logical lines, none wrapped: every row ends with a newline
        // except the last, and each row carries the next number.
        let rows = [true, true, false];
        assert_eq!(line_numbers_for_rows(rows), vec![(0, 1), (1, 2), (2, 3)],);
    }

    #[test]
    fn line_numbers_skip_soft_wrap_continuation_rows() {
        // Row 0 is a logical line that soft-wraps into row 1 (row 0 does NOT
        // end with a newline). Row 1 ends the logical line with a newline, so
        // row 2 starts the next logical line. The wrap-continuation row 1 must
        // be left blank.
        let rows = [false, true, false];
        assert_eq!(line_numbers_for_rows(rows), vec![(0, 1), (2, 2)]);
    }

    #[test]
    fn line_numbers_after_newline_do_not_mark_a_wrapped_row() {
        // Regression for the reported bug: a wrapped first logical line
        // (rows 0+1) followed by a freshly typed newline (row 1 ends with a
        // newline) and then a new empty line (row 2). The wrap-continuation
        // row 1 must stay blank; only rows 0 and 2 get numbers.
        let rows = [false, true, false];
        let numbered: Vec<usize> = line_numbers_for_rows(rows)
            .into_iter()
            .map(|(row, _)| row)
            .collect();
        assert!(
            !numbered.contains(&1),
            "the soft-wrap continuation row must not be numbered"
        );
        assert_eq!(numbered, vec![0, 2]);
    }

    #[test]
    fn line_numbers_empty_galley_yields_nothing() {
        assert_eq!(line_numbers_for_rows(std::iter::empty()), vec![]);
    }

    #[test]
    fn line_numbers_single_row() {
        assert_eq!(line_numbers_for_rows([false]), vec![(0, 1)]);
    }
}
