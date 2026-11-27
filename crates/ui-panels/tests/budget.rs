//! The ui-panels share of "load and show 300 ligands from cache in < 2 s"
//! (CLAUDE.md). mol's cache load takes ~25 ms of it; building the table view
//! model (read the pose table, sort, filter, format the first visible window)
//! must stay a small slice of the rest, at 300 and at 10,000 poses.
//!
//! The limits are generous on purpose: this runs unoptimized on CI. The real
//! numbers come from `cargo bench -p mold-ui-panels --bench table` (release).

use std::time::{Duration, Instant};

use mold_mol::pose_columns as col;
use mold_ui_panels::synthetic;
use mold_ui_panels::table::{Filter, PoseTableModel, Predicate, visible_rows};

/// Everything between "the pose table is in memory" and "the first frame of
/// the table can be painted". Returns the number of cells formatted.
fn build_view_model(df: &polars::prelude::DataFrame) -> usize {
    let mut m = PoseTableModel::from_frame(df).expect("synthetic table is valid");
    m.sort_by(col::AFFINITY_PRED, false);
    m.set_filter(Filter::none().and(Predicate::is(col::GATE_PASS, true)))
        .unwrap();
    // A 900 px tall panel with 32 px rows, scrolled to the top, 4 rows overscan.
    let window = visible_rows(0.0, 900.0, 32.0, m.row_count(), 4);
    m.window(window).iter().map(Vec::len).sum()
}

fn median_of(runs: usize, mut f: impl FnMut()) -> Duration {
    let mut t: Vec<Duration> = (0..runs)
        .map(|_| {
            let start = Instant::now();
            f();
            start.elapsed()
        })
        .collect();
    t.sort();
    t[runs / 2]
}

#[test]
fn building_the_table_view_model_fits_the_budget_share() {
    for (rows, limit) in [
        (300, Duration::from_millis(100)),
        (10_000, Duration::from_millis(1_000)),
    ] {
        let df = synthetic::pose_table(rows, 7);
        let cells = build_view_model(&df);
        assert!(cells > 0, "the first window has cells");
        let t = median_of(5, || {
            std::hint::black_box(build_view_model(&df));
        });
        eprintln!("view model, {rows} poses: {t:?} (limit {limit:?})");
        assert!(t < limit, "{rows} poses took {t:?}, limit {limit:?}");
    }
}
