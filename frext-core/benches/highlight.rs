//! Benchmarks for core syntax highlighting.
//!
//! Measures [`frext_core::highlight::highlight`] -- syntect highlighting into
//! [`StyledSpan`]s, with no front-end conversion -- over a deterministic
//! 5,000-line synthetic Rust source: highlighted as Rust and as plain text.
//! The `incremental_*` rows measure
//! [`IncrementalHighlighter`] on a 50-line viewport of the same source.
//!
//! [`StyledSpan`]: frext_core::highlight::StyledSpan

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use frext_core::highlight::{highlight, incremental::IncrementalHighlighter};

/// The viewport the warm incremental bench requests: 50 lines around the edit.
const VIEWPORT: std::ops::Range<usize> = 2_480..2_530;

/// A fixed 50-line Rust template; repeated [`REPEATS`] times to build the
/// benchmark source. `frobnicate` occurs exactly twice per template.
const TEMPLATE: &str = r#"//! A synthetic module used for highlighting benchmarks.

use std::collections::HashMap;
use std::fmt;

/// A widget with a name and a weight.
#[derive(Debug, Clone, PartialEq)]
pub struct Widget {
    /// The display name.
    pub name: String,
    /// The weight in grams.
    pub weight: u32,
}

impl Widget {
    /// Create a new widget.
    pub fn new(name: &str, weight: u32) -> Self {
        Self {
            name: name.to_owned(),
            weight,
        }
    }

    /// Frobnicate the widget, doubling its weight.
    pub fn frobnicate(&mut self) {
        self.weight = self.weight.saturating_mul(2);
    }
}

impl fmt::Display for Widget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({}g)", self.name, self.weight)
    }
}

/// Count widgets by name.
pub fn tally(widgets: &[Widget]) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for w in widgets {
        // Bump the count for this name.
        *counts.entry(w.name.clone()).or_insert(0) += 1;
    }
    counts
}

fn main() {
    let mut w = Widget::new("sprocket", 0x2A);
    w.frobnicate();
    println!("{w} weighs {:.2} kg", f64::from(w.weight) / 1000.0);
}
"#;

/// How many times [`TEMPLATE`] is repeated: 50 lines x 100 = 5,000 lines.
const REPEATS: usize = 100;

fn source() -> String {
    assert_eq!(TEMPLATE.lines().count(), 50, "template must be 50 lines");
    let src = TEMPLATE.repeat(REPEATS);
    assert_eq!(src.lines().count(), 5_000, "source must be 5,000 lines");
    src
}

fn bench_highlight(c: &mut Criterion) {
    let src = source();

    let mut group = c.benchmark_group("core_highlight");
    group.bench_function("full_rust_5k_lines", |b| {
        b.iter(|| highlight(black_box(&src), black_box("rs")));
    });
    group.bench_function("full_plain_5k_lines", |b| {
        b.iter(|| highlight(black_box(&src), black_box("")));
    });

    // A warm highlighter, as the TUI uses it per keystroke: each iteration
    // types and then deletes one char at line 2,500, requesting the viewport
    // after each of the two edits.
    group.bench_function("incremental_viewport_after_edit_rust_5k_lines", |b| {
        let mut text = src.clone();
        let edit_at: usize = text.split_inclusive('\n').take(2_500).map(str::len).sum();
        let mut hl = IncrementalHighlighter::new();
        let _ = hl.spans(&text, "rs", VIEWPORT);
        b.iter(|| {
            text.insert(edit_at, 'x');
            let typed = hl.spans(black_box(&text), black_box("rs"), VIEWPORT);
            text.remove(edit_at);
            let deleted = hl.spans(black_box(&text), black_box("rs"), VIEWPORT);
            (typed, deleted)
        });
    });
    group.bench_function("incremental_cold_viewport_rust_5k_lines", |b| {
        b.iter(|| {
            let mut hl = IncrementalHighlighter::new();
            hl.spans(black_box(&src), black_box("rs"), 4_950..5_000)
        });
    });
    group.finish();
}

criterion_group!(benches, bench_highlight);
criterion_main!(benches);
