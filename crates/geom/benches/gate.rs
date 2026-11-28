//! Criterion: gate and contact-profile 500 ligand poses against the 4WKQ
//! receptor, whose KD-tree (and contact typing) is built once. Budget
//! (CLAUDE.md): gate under 200 ms, measured with `--release`
//! (`cargo bench -p mold-geom`). Contacts are reported against the same budget.

#[path = "../tests/common/mod.rs"]
mod common;

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use mold_geom::{ContactConfig, GateConfig, PoseGate, Receptor, Topology, contacts};

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

fn contacts_500_poses(c: &mut Criterion) {
    let (arena, receptor_pose, ligand_pose) =
        common::load_complex("inputs/4WKQ_protein.pdb", "inputs/4WKQ_IRE_crystal.sdf");
    let crystal = arena.pose_positions(ligand_pose).unwrap();
    let poses = common::perturbed_poses(crystal, POSES, 0x4D4F_4C44);
    let topology = Topology::from_arena(&arena, ligand_pose).unwrap();

    c.bench_function("receptor_from_arena_typed_4wkq", |b| {
        b.iter(|| Receptor::from_arena(black_box(&arena), receptor_pose))
    });

    let receptor = Receptor::from_arena(&arena, receptor_pose).unwrap();
    let config = ContactConfig::default();
    c.bench_function("contacts_500_poses_4wkq", |b| {
        b.iter(|| {
            let mut n = 0usize;
            for pose in &poses {
                n += contacts(&receptor, &topology, black_box(pose), &config).n_hbonds();
            }
            black_box(n)
        })
    });

    let gate = PoseGate::new(
        Receptor::from_arena(&arena, receptor_pose).unwrap(),
        Topology::from_arena(&arena, ligand_pose).unwrap(),
        GateConfig::default(),
    );
    c.bench_function("gate_and_contacts_500_poses_4wkq", |b| {
        b.iter(|| {
            let mut n = 0usize;
            for pose in &poses {
                let report = gate.evaluate(black_box(pose), &[]);
                let found = contacts(gate.receptor(), gate.topology(), pose, &config);
                n += usize::from(report.pass()) + found.n_hbonds();
            }
            black_box(n)
        })
    });
}

criterion_group!(benches, gate_500_poses, contacts_500_poses);
criterion_main!(benches);
