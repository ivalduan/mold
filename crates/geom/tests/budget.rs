//! Performance budget from the root CLAUDE.md: gate 500 poses in under 200 ms.
//! Only meaningful in an optimised build, so it is ignored under
//! `debug_assertions`; run it with
//! `cargo test --release -p mold-geom --test budget`.
//! The Criterion bench (`cargo bench -p mold-geom`) gives the full statistics.

mod common;

use std::time::{Duration, Instant};

use mold_geom::{ContactConfig, GateConfig, PoseGate, Receptor, Topology, contacts};

const BUDGET: Duration = Duration::from_millis(200);

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "performance budget is measured with --release"
)]
fn gate_500_poses_under_200_ms() {
    let crystal = common::load_ligand("inputs/4WKQ_IRE_crystal.sdf");
    let (pp, pe) = common::load_protein("inputs/4WKQ_protein.pdb");
    let poses = common::perturbed_poses(&crystal.positions, 500, 0x4D4F_4C44);
    let gate = PoseGate::new(
        Receptor::new(&pp, &pe),
        Topology::new(&crystal.elements, &crystal.bonds).unwrap(),
        GateConfig::default(),
    )
    .with_reference(&crystal.positions)
    .unwrap();

    // Best of five: the budget is about the code, not about a noisy neighbour.
    let mut best = Duration::MAX;
    let mut passed = 0;
    for _ in 0..5 {
        let t = Instant::now();
        passed = poses
            .iter()
            .filter(|p| gate.evaluate(p, &[]).pass())
            .count();
        best = best.min(t.elapsed());
    }
    println!("gate 500 poses: {best:?} ({passed} physically valid)");
    assert!(
        best < BUDGET,
        "gate 500 poses took {best:?}, budget {BUDGET:?}"
    );
}

/// Contact profiling of the same 500 poses. Not a budget of its own: the
/// 200 ms is the gate's. Asserted against it anyway so a regression shows,
/// and gate + contacts is printed for the record.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "performance budget is measured with --release"
)]
fn contacts_500_poses_within_the_gate_budget() {
    let (arena, receptor_pose, ligand_pose) =
        common::load_complex("inputs/4WKQ_protein.pdb", "inputs/4WKQ_IRE_crystal.sdf");
    let poses =
        common::perturbed_poses(arena.pose_positions(ligand_pose).unwrap(), 500, 0x4D4F_4C44);
    let gate = PoseGate::new(
        Receptor::from_arena(&arena, receptor_pose).unwrap(),
        Topology::from_arena(&arena, ligand_pose).unwrap(),
        GateConfig::default(),
    );
    let config = ContactConfig::default();
    let time = |f: &dyn Fn() -> usize| {
        let mut best = Duration::MAX;
        let mut out = 0;
        for _ in 0..5 {
            let t = Instant::now();
            out = f();
            best = best.min(t.elapsed());
        }
        (best, out)
    };
    let (only, hbonds) = time(&|| {
        poses
            .iter()
            .map(|p| contacts(gate.receptor(), gate.topology(), p, &config).n_hbonds())
            .sum()
    });
    let (both, _) = time(&|| {
        poses
            .iter()
            .map(|p| {
                usize::from(gate.evaluate(p, &[]).pass())
                    + contacts(gate.receptor(), gate.topology(), p, &config).n_hbonds()
            })
            .sum()
    });
    println!("contacts 500 poses: {only:?} ({hbonds} H-bonds); gate + contacts: {both:?}");
    assert!(
        only < BUDGET,
        "contacts 500 poses took {only:?}, budget {BUDGET:?}"
    );
}
