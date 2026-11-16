//! Benchmarks for building the editor's highlighted [`LayoutJob`].
//!
//! Measures [`frext::editor_layout::layout_job`] -- syntect highlighting plus
//! search-match painting, without egui galley layout -- over a deterministic
//! 5,000-line synthetic Rust source: highlighted as Rust, highlighted as Rust
//! with 200 search matches painted on top, and as plain text.
//!
//! [`LayoutJob`]: eframe::egui::text::LayoutJob

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use eframe::egui::FontId;
use frext::editor_layout::layout_job;
use frext::highlight::MatchHighlights;
use frext::search::SearchQuery;

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

/// Expected match count for the search pattern over the whole source.
const EXPECTED_MATCHES: usize = 200;

fn source() -> String {
    assert_eq!(TEMPLATE.lines().count(), 50, "template must be 50 lines");
    let src = TEMPLATE.repeat(REPEATS);
    assert_eq!(src.lines().count(), 5_000, "source must be 5,000 lines");
    src
}

fn search_matches(src: &str) -> MatchHighlights {
    let query = SearchQuery {
        pattern: "frobnicate".to_owned(),
        case_sensitive: true,
        regex: false,
        whole_word: false,
    };
    let ranges = match query.compile() {
        Ok(matcher) => matcher.find_matches(src),
        Err(err) => panic!("benchmark search query failed to compile: {err}"),
    };
    assert_eq!(ranges.len(), EXPECTED_MATCHES, "unexpected match count");
    MatchHighlights {
        ranges,
        current: Some(0),
    }
}

fn bench_highlight(c: &mut Criterion) {
    let src = source();
    let matches = search_matches(&src);
    let none = MatchHighlights::default();

    let mut group = c.benchmark_group("highlight");
    group.bench_function("rust_5k_lines", |b| {
        b.iter(|| {
            layout_job(
                black_box(&src),
                black_box("rs"),
                FontId::monospace(12.0),
                black_box(&none),
            )
        });
    });
    group.bench_function("rust_5k_lines_200_matches", |b| {
        b.iter(|| {
            layout_job(
                black_box(&src),
                black_box("rs"),
                FontId::monospace(12.0),
                black_box(&matches),
            )
        });
    });
    group.bench_function("plain_5k_lines", |b| {
        b.iter(|| {
            layout_job(
                black_box(&src),
                black_box(""),
                FontId::monospace(12.0),
                black_box(&none),
            )
        });
    });
    group.finish();
}

criterion_group!(benches, bench_highlight);
criterion_main!(benches);
