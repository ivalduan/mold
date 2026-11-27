//! The `mol` half of the "load and show 300 ligand poses from cache in < 2 s"
//! budget (the rest belongs to ui-panels).
//!
//! - `parse_mmcif_300`: pdbtbx on the 59 MB, 300-pose mmCIF (the slow path, for the record);
//! - `cache_load_300`: the same arena from the binary cache file;
//! - `cache_save_300`: writing that cache.
//!
//! Input: `uv run fixtures/scripts/make_pose_set.py` (git-ignored
//! `fixtures/cache/egfr-300poses.cif`). Without it the bench prints why and
//! measures nothing, as on CI. Run with `cargo bench -p mold-mol --bench load`.

use std::path::PathBuf;
use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use mold_mol::Arena;
use mold_mol::cache;
use mold_mol::io::{self, LoadOptions};

fn pose_set() -> Option<PathBuf> {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/cache/egfr-300poses.cif");
    if path.exists() {
        Some(path)
    } else {
        eprintln!(
            "skipping the mol load bench: {} is missing; generate it with \
             `uv run fixtures/scripts/make_pose_set.py`",
            path.display()
        );
        None
    }
}

fn parse(path: &PathBuf) -> Arena {
    let mut arena = Arena::new();
    io::read_file(&mut arena, path, &LoadOptions::default()).expect("pose set parses");
    arena
}

fn load_budget(c: &mut Criterion) {
    let Some(cif) = pose_set() else { return };
    let arena = parse(&cif);
    assert_eq!(arena.pose_count(), 300);
    let dir = std::env::temp_dir().join(format!("mold-mol-bench-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cached = dir.join(format!("egfr-300poses.{}", cache::EXTENSION));
    cache::save_file(&arena, &cached).unwrap();
    eprintln!(
        "300 poses, {} atoms; mmCIF {:.1} MB, cache {:.1} MB",
        arena.atom_count(),
        std::fs::metadata(&cif).unwrap().len() as f64 / 1e6,
        std::fs::metadata(&cached).unwrap().len() as f64 / 1e6,
    );

    let mut group = c.benchmark_group("mol_load_300_poses");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(35));
    group.bench_function("parse_mmcif_300", |b| b.iter(|| parse(&cif)));
    group.measurement_time(Duration::from_secs(5));
    group.bench_function("cache_load_300", |b| {
        b.iter(|| cache::load_file(&cached).unwrap())
    });
    group.bench_function("cache_save_300", |b| {
        b.iter(|| cache::save_file(&arena, &cached).unwrap())
    });
    group.finish();
    std::fs::remove_dir_all(&dir).ok();
}

criterion_group!(benches, load_budget);
criterion_main!(benches);
