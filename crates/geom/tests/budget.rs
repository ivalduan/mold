//! Performance budget from the root CLAUDE.md: gate 500 poses in under 200 ms.
//! Only meaningful in an optimised build, so it is ignored under
//! `debug_assertions`; run it with
//! `cargo test --release -p mold-geom --test budget`.
//! The Criterion bench (`cargo bench -p mold-geom`) gives the full statistics.

mod common;

use std::time::{Duration, Instant};

use mold_geom::{GateConfig, PoseGate, Receptor, Topology};

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
