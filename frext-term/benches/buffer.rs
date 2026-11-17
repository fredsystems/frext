//! Benchmarks for the editable buffer view.
//!
//! Measures [`BufferView`] over a deterministic 5,000-line synthetic Rust
//! source in an 80x24 terminal: typing and rendering in a warm view at
//! line 2,500, and rendering with no change.

use std::hint::black_box;
use std::time::Instant;

use criterion::{Criterion, criterion_group, criterion_main};
use frext_core::tab::Tab;
use frext_term::buffer::BufferView;
use frext_term::buffer::edit::EditAction;
use frext_term::hit::HitMap;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

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

const GUTTER: Rect = Rect::new(0, 0, 5, 24);
const EDITOR: Rect = Rect::new(5, 0, 75, 24);

fn render(view: &mut BufferView, tab: &Tab, buf: &mut Buffer, hits: &mut HitMap) {
    hits.clear();
    black_box(view.render(Some(tab), GUTTER, EDITOR, buf, hits));
}

fn bench_buffer(c: &mut Criterion) {
    let src = source();
    let line_2500: usize = src
        .split('\n')
        .take(2_500)
        .map(|l| l.chars().count() + 1)
        .sum();
    let mut tab = Tab::new_untitled(1);
    tab.text = src;
    tab.path = Some("bench.rs".into());
    let mut buf = Buffer::empty(Rect::new(0, 0, 80, 24));
    let mut hits = HitMap::default();
    let mut view = BufferView::default();
    render(&mut view, &tab, &mut buf, &mut hits);
    view.set_selection(&tab, line_2500..line_2500);
    render(&mut view, &tab, &mut buf, &mut hits);

    let mut group = c.benchmark_group("buffer");
    // Two keystrokes per iteration: type a char and render, then delete
    // it and render, so the text is the same at the start of every one.
    group.bench_function("type_and_render_rust_5k_lines", |b| {
        b.iter(|| {
            view.handle(&mut tab, EditAction::Type('x'), Instant::now());
            render(&mut view, &tab, &mut buf, &mut hits);
            view.handle(&mut tab, EditAction::Backspace, Instant::now());
            render(&mut view, &tab, &mut buf, &mut hits);
        });
    });
    group.bench_function("render_only_rust_5k_lines", |b| {
        b.iter(|| render(&mut view, &tab, &mut buf, &mut hits));
    });
    group.finish();
}

criterion_group!(benches, bench_buffer);
criterion_main!(benches);
