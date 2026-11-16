//! Benchmarks for terminal soft-wrap layout.
//!
//! Measures [`WrapLayout::new`] over a deterministic 5,000-line synthetic
//! Rust source at wrap widths 79 and 39 (an 80- and 40-column editor).

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use frext_term::buffer::wrap::WrapLayout;

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

fn bench_wrap(c: &mut Criterion) {
    let src = source();

    let mut group = c.benchmark_group("wrap");
    group.bench_function("wrap_rust_5k_lines_80", |b| {
        b.iter(|| WrapLayout::new(black_box(&src), black_box(79)));
    });
    group.bench_function("wrap_rust_5k_lines_40", |b| {
        b.iter(|| WrapLayout::new(black_box(&src), black_box(39)));
    });
    group.finish();
}

criterion_group!(benches, bench_wrap);
criterion_main!(benches);
