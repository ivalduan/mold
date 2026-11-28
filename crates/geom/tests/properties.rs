//! Property tests: RMSD is invariant under rigid transforms, clash counts are
//! symmetric, and the KD-tree agrees with brute force.

use mold_geom::SpatialIndex;
use mold_geom::ligand::{aromatic_ring_flatness, internal_clashes};
use mold_geom::receptor::{DistanceParams, Receptor, protein_ligand_distances};
use mold_geom::rmsd::{centroid_distance, kabsch_rmsd, rmsd};
use mold_geom::{Bond, BondOrder, Topology};
use mold_mol::Element;
use proptest::prelude::*;

type P = [f32; 3];

/// A rigid transform: unit quaternion rotation + translation.
#[derive(Debug, Clone, Copy)]
struct Rigid {
    q: [f64; 4],
    t: [f64; 3],
}

impl Rigid {
    fn apply(&self, p: P) -> P {
        let [w, x, y, z] = self.q;
        let m = [
            [
                1.0 - 2.0 * (y * y + z * z),
                2.0 * (x * y - w * z),
                2.0 * (x * z + w * y),
            ],
            [
                2.0 * (x * y + w * z),
                1.0 - 2.0 * (x * x + z * z),
                2.0 * (y * z - w * x),
            ],
            [
                2.0 * (x * z - w * y),
                2.0 * (y * z + w * x),
                1.0 - 2.0 * (x * x + y * y),
            ],
        ];
        let v = p.map(f64::from);
        let r: [f64; 3] =
            std::array::from_fn(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2] + self.t[i]);
        r.map(|c| c as f32)
    }

    fn apply_all(&self, ps: &[P]) -> Vec<P> {
        ps.iter().map(|&p| self.apply(p)).collect()
    }
}

fn rigid() -> impl Strategy<Value = Rigid> {
    (
        prop::array::uniform4(-1.0f64..1.0),
        prop::array::uniform3(-50.0f64..50.0),
    )
        .prop_filter_map("degenerate quaternion", |(q, t)| {
            let n = q.iter().map(|c| c * c).sum::<f64>().sqrt();
            (n > 0.1).then(|| Rigid {
                q: q.map(|c| c / n),
                t,
            })
        })
}

/// Ligand-sized coordinate sets in a 20 Å box.
fn points(n: std::ops::Range<usize>) -> impl Strategy<Value = Vec<P>> {
    prop::collection::vec(prop::array::uniform3(-10.0f32..10.0), n)
}

fn element() -> impl Strategy<Value = Element> {
    prop::sample::select(vec![
        Element::C,
        Element::N,
        Element::O,
        Element::S,
        Element::CL,
    ])
}

/// Two poses of the same molecule: `a`, and `a` with every atom jittered.
fn pose_pair() -> impl Strategy<Value = (Vec<P>, Vec<P>)> {
    points(3..40).prop_flat_map(|a| {
        let n = a.len();
        (
            Just(a),
            prop::collection::vec(prop::array::uniform3(-2.0f32..2.0), n),
        )
            .prop_map(|(a, j)| {
                let b = a
                    .iter()
                    .zip(&j)
                    .map(|(p, d)| [p[0] + d[0], p[1] + d[1], p[2] + d[2]])
                    .collect();
                (a, b)
            })
    })
}

// f32 coordinates up to ~60 Å carry ~4e-6 Å of rounding each.
const EPS: f64 = 1e-3;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn kabsch_rmsd_of_a_rigidly_moved_copy_is_zero(a in points(3..60), g in rigid()) {
        let b = g.apply_all(&a);
        prop_assert!(kabsch_rmsd(&b, &a).unwrap() < EPS);
    }

    #[test]
    fn rmsd_is_invariant_when_both_poses_move_together((a, b) in pose_pair(), g in rigid()) {
        let before = rmsd(&a, &b).unwrap();
        let after = rmsd(&g.apply_all(&a), &g.apply_all(&b)).unwrap();
        prop_assert!((before - after).abs() < EPS, "{before} vs {after}");
        let cb = centroid_distance(&a, &b).unwrap();
        let ca = centroid_distance(&g.apply_all(&a), &g.apply_all(&b)).unwrap();
        prop_assert!((cb - ca).abs() < EPS);
    }

    #[test]
    fn kabsch_rmsd_ignores_any_rigid_move_of_either_pose(
        (a, b) in pose_pair(), g in rigid(), h in rigid()
    ) {
        let base = kabsch_rmsd(&a, &b).unwrap();
        let moved = kabsch_rmsd(&g.apply_all(&a), &h.apply_all(&b)).unwrap();
        prop_assert!((base - moved).abs() < EPS, "{base} vs {moved}");
        prop_assert!(base <= rmsd(&a, &b).unwrap() + 1e-9, "superposition never hurts");
    }

    #[test]
    fn protein_ligand_clash_count_is_symmetric(
        a in points(1..80), ea in prop::collection::vec(element(), 80),
        b in points(1..80), eb in prop::collection::vec(element(), 80),
        cutoff in 0.3f32..1.0,
    ) {
        let ea = &ea[..a.len()];
        let eb = &eb[..b.len()];
        let params = DistanceParams { clash_cutoff: cutoff, ..DistanceParams::default() };
        let ab = protein_ligand_distances(&Receptor::new(&a, ea), &b, eb, &params, &[]);
        let ba = protein_ligand_distances(&Receptor::new(&b, eb), &a, ea, &params, &[]);
        prop_assert_eq!(ab.clashes, ba.clashes);
        let (da, db) = (ab.smallest_distance().unwrap(), ba.smallest_distance().unwrap());
        prop_assert!((da - db).abs() < 1e-5);
    }

    #[test]
    fn internal_checks_are_invariant_under_rigid_transform(a in points(6..30), g in rigid()) {
        // A chain with one six-ring at the start, so flatness has a ring to fit.
        let n = a.len() as u32;
        let mut bonds: Vec<Bond> = (0..n - 1)
            .map(|i| Bond { a: i, b: i + 1, order: if i % 2 == 0 && i < 6 { BondOrder::Double } else { BondOrder::Single } })
            .collect();
        bonds.push(Bond { a: 0, b: 5, order: BondOrder::Single });
        let t = Topology::new(&vec![Element::C; a.len()], &bonds).unwrap();
        let b = g.apply_all(&a);
        let (ca, cb) = (internal_clashes(&t, &a, 0.5), internal_clashes(&t, &b, 0.5));
        prop_assert_eq!(ca.checked, cb.checked);
        prop_assert!((ca.shortest_relative_distance - cb.shortest_relative_distance).abs() < EPS);
        let (fa, fb) = (aromatic_ring_flatness(&t, &a, 0.25), aromatic_ring_flatness(&t, &b, 0.25));
        prop_assert!((fa.max_distance_from_plane - fb.max_distance_from_plane).abs() < EPS);
    }

    #[test]
    fn radius_queries_match_brute_force(pts in points(1..300), q in prop::array::uniform3(-12.0f32..12.0), r in 0.0f32..8.0) {
        let index = SpatialIndex::new(&pts);
        let mut got: Vec<u32> = index.within(q, r).iter().map(|n| n.index).collect();
        got.sort_unstable();
        let d2 = |p: &P| (0..3).map(|i| (p[i] - q[i]) * (p[i] - q[i])).sum::<f32>();
        let expected: Vec<u32> = (0..pts.len() as u32).filter(|&i| d2(&pts[i as usize]) <= r * r).collect();
        // Points within float rounding of the boundary may go either way.
        let near_boundary = |i: u32| (d2(&pts[i as usize]).sqrt() - r).abs() < 1e-4;
        for i in got.iter().filter(|i| !expected.contains(i)) { prop_assert!(near_boundary(*i)); }
        for i in expected.iter().filter(|i| !got.contains(i)) { prop_assert!(near_boundary(*i)); }
        let nearest = index.nearest(q).unwrap();
        let best = pts.iter().map(|p| d2(p).sqrt()).fold(f32::INFINITY, f32::min);
        prop_assert!((nearest.distance - best).abs() < 1e-4);
    }
}
