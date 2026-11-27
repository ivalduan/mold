//! The ui-panels share of "load and show 300 ligands from cache in < 2 s":
//! building the pose table's view model from an in-memory pose table.
//!
//! - `build/{300,10000}`: read the columns, sort by affinity, filter
//!   `gate_pass`, format the first visible window (what the first frame needs);
//! - `resort/{300,10000}`: change the sort on a built model (a header click);
//! - `refilter/{300,10000}`: change the filter on a built model.
//!
//! Run with `cargo bench -p mold-ui-panels --bench table`.

use std::time::Duration;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use mold_mol::pose_columns as col;
use mold_ui_panels::synthetic;
use mold_ui_panels::table::{Filter, PoseTableModel, Predicate, visible_rows};

fn gate() -> Filter {
    Filter::none().and(Predicate::is(col::GATE_PASS, true))
}

fn build(df: &polars::prelude::DataFrame) -> PoseTableModel {
    let mut m = PoseTableModel::from_frame(df).unwrap();
    m.sort_by(col::AFFINITY_PRED, false);
    m.set_filter(gate()).unwrap();
    let window = visible_rows(0.0, 900.0, 32.0, m.row_count(), 4);
    std::hint::black_box(m.window(window));
    m
}

fn table_view_model(c: &mut Criterion) {
    let mut group = c.benchmark_group("pose_table_view_model");
    group.measurement_time(Duration::from_secs(3));
    for rows in [300usize, 10_000] {
        let df = synthetic::pose_table(rows, 7);
        group.bench_with_input(BenchmarkId::new("build", rows), &df, |b, df| {
            b.iter(|| build(df))
        });
        let mut m = build(&df);
        let mut desc = false;
        group.bench_function(BenchmarkId::new("resort", rows), |b| {
            b.iter(|| {
                desc = !desc;
                m.sort_by(col::PLDDT, desc);
            })
        });
        let plddt = Filter::none().and(Predicate::at_least(col::PLDDT, 70.0));
        let mut flip = false;
        group.bench_function(BenchmarkId::new("refilter", rows), |b| {
            b.iter(|| {
                flip = !flip;
                m.set_filter(if flip { plddt.clone() } else { gate() })
                    .unwrap();
            })
        });
    }
    group.finish();
}

criterion_group!(benches, table_view_model);
criterion_main!(benches);
