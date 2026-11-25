//! Criterion: gate 500 ligand poses against the 4WKQ receptor, whose KD-tree is
//! built once. Budget (CLAUDE.md): under 200 ms, measured with `--release`
//! (`cargo bench -p mold-geom`).

#[path = "../tests/common/mod.rs"]
mod common;

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use mold_geom::{GateConfig, PoseGate, Receptor, Topology};

const POSES: usize = 500;

fn gate_500_poses(c: &mut Criterion) {
    let crystal = common::load_ligand("inputs/4WKQ_IRE_crystal.sdf");
    let (pp, pe) = common::load_protein("inputs/4WKQ_protein.pdb");
    let poses = common::perturbed_poses(&crystal.positions, POSES, 0x4D4F_4C44);

    c.bench_function("receptor_index_4wkq", |b| {
        b.iter(|| Receptor::new(black_box(&pp), black_box(&pe)))
    });

    let gate = PoseGate::new(
        Receptor::new(&pp, &pe),
        Topology::new(&crystal.elements, &crystal.bonds).unwrap(),
        GateConfig::default(),
    )
    .with_reference(&crystal.positions)
    .unwrap();
    c.bench_function("gate_500_poses_4wkq", |b| {
        b.iter(|| {
            let mut passed = 0usize;
            for pose in &poses {
                passed += usize::from(gate.evaluate(black_box(pose), &[]).pass());
            }
            black_box(passed)
        })
    });
}

criterion_group!(benches, gate_500_poses);
criterion_main!(benches);
