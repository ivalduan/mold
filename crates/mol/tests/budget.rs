//! Release-only budget test for the `mol` half of "load and show 300 ligand
//! poses from cache in < 2 s": loading the arena from the binary cache must
//! take well under that. Ignored by default; run with
//!
//! ```sh
//! cargo test -p mold-mol --release --test budget -- --ignored --nocapture
//! ```
//!
//! Skips (passes, with a printed reason) without the generated pose set or in a
//! debug build.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use mold_mol::Arena;
use mold_mol::cache;
use mold_mol::io::{self, LoadOptions};

/// The cache load gets a quarter of the shared 2 s; the rest is ui-panels'.
const CACHE_LOAD_BUDGET: Duration = Duration::from_millis(500);

#[test]
#[ignore = "release-only budget; needs fixtures/cache/egfr-300poses.cif"]
fn cache_load_of_300_poses_is_within_budget() {
    if cfg!(debug_assertions) {
        eprintln!("skipping: budgets are measured with --release only");
        return;
    }
    let cif =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/cache/egfr-300poses.cif");
    if !cif.exists() {
        eprintln!(
            "skipping: {} is missing; run `uv run fixtures/scripts/make_pose_set.py`",
            cif.display()
        );
        return;
    }
    let started = Instant::now();
    let mut arena = Arena::new();
    io::read_file(&mut arena, &cif, &LoadOptions::default()).unwrap();
    let parse = started.elapsed();
    assert_eq!(arena.pose_count(), 300);

    let dir = std::env::temp_dir().join(format!("mold-mol-budget-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("poses.{}", cache::EXTENSION));
    cache::save_file(&arena, &path).unwrap();

    // Best of five: the budget is about our code, not a cold disk.
    let best = (0..5)
        .map(|_| {
            let t = Instant::now();
            let back = cache::load_file(&path).unwrap();
            let elapsed = t.elapsed();
            assert_eq!(back.atom_count(), arena.atom_count());
            elapsed
        })
        .min()
        .unwrap();
    std::fs::remove_dir_all(&dir).ok();
    eprintln!("mmCIF parse {parse:?}; cache load {best:?} (budget {CACHE_LOAD_BUDGET:?})");
    assert!(
        best < CACHE_LOAD_BUDGET,
        "cache load {best:?} over {CACHE_LOAD_BUDGET:?}"
    );
}
