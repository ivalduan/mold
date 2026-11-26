//! Ligand-internal geometry checks: bond lengths, bond angles, internal steric
//! clashes and aromatic ring flatness.
//!
//! Every check takes a precomputed [`Topology`] (built once per ligand) and one
//! pose's positions (indexed like the topology's atoms), and returns the number
//! behind the verdict as well as the verdict. Hydrogens are ignored.

use crate::linalg::{dist, dot, jacobi_eigen, mean, sub, v3};
use crate::topology::Topology;

/// Result of [`bond_lengths`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BondLengthCheck {
    /// Heavy-atom bonds checked.
    pub checked: usize,
    /// Bonds shorter than `(1 - tolerance) ×` their reference length.
    pub too_short: usize,
    /// Bonds longer than `(1 + tolerance) ×` their reference length.
    pub too_long: usize,
    /// Smallest `length / reference` over all bonds (1.0 if none).
    pub shortest_relative: f64,
    /// Largest `length / reference` over all bonds (1.0 if none).
    pub longest_relative: f64,
}

impl BondLengthCheck {
    pub fn pass(&self) -> bool {
        self.too_short == 0 && self.too_long == 0
    }
}

/// Every heavy-atom bond length within `±tolerance` (relative) of its reference
/// (sum of covalent radii for the bond's effective order).
pub fn bond_lengths(
    topology: &Topology,
    positions: &[[f32; 3]],
    tolerance: f32,
) -> BondLengthCheck {
    let tol = f64::from(tolerance);
    let mut r = BondLengthCheck {
        checked: 0,
        too_short: 0,
        too_long: 0,
        shortest_relative: 1.0,
        longest_relative: 1.0,
    };
    for b in topology.bond_refs() {
        let rel = dist(positions[b.a as usize], positions[b.b as usize]) / f64::from(b.length);
        r.checked += 1;
        r.shortest_relative = r.shortest_relative.min(rel);
        r.longest_relative = r.longest_relative.max(rel);
        if rel < 1.0 - tol {
            r.too_short += 1;
        } else if rel > 1.0 + tol {
            r.too_long += 1;
        }
    }
    r
}

/// Result of [`bond_angles`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BondAngleCheck {
    /// Heavy-atom angles checked.
    pub checked: usize,
    /// Angles whose 1-3 distance is outside `±tolerance` of the reference.
    pub outliers: usize,
    /// The `distance / reference` ratio furthest from 1 (1.0 if none).
    pub most_extreme_relative: f64,
}

impl BondAngleCheck {
    pub fn pass(&self) -> bool {
        self.outliers == 0
    }
}

/// Bond angles, judged as PoseBusters (via RDKit's distance bounds) does: by the
/// 1-3 distance of each angle `a–center–b` against a reference built from the
/// ideal angle and the reference bond lengths, within `±tolerance` (relative).
pub fn bond_angles(topology: &Topology, positions: &[[f32; 3]], tolerance: f32) -> BondAngleCheck {
    let tol = f64::from(tolerance);
    let mut r = BondAngleCheck {
        checked: 0,
        outliers: 0,
        most_extreme_relative: 1.0,
    };
    for a in topology.angle_refs() {
        let rel = dist(positions[a.a as usize], positions[a.b as usize]) / f64::from(a.distance);
        r.checked += 1;
        if (rel - 1.0).abs() > (r.most_extreme_relative - 1.0).abs() {
            r.most_extreme_relative = rel;
        }
        if rel < 1.0 - tol || rel > 1.0 + tol {
            r.outliers += 1;
        }
    }
    r
}

/// Result of [`internal_clashes`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InternalClashCheck {
    /// Non-bonded heavy-atom pairs (three or more bonds apart) checked.
    pub checked: usize,
    /// Pairs closer than `clash_ratio ×` their van der Waals radius sum.
    pub clashes: usize,
    /// Smallest `distance / vdw_sum` over all pairs (`f64::INFINITY` if none).
    pub shortest_relative_distance: f64,
}

impl InternalClashCheck {
    pub fn pass(&self) -> bool {
        self.clashes == 0
    }
}

/// Internal steric clashes: heavy-atom pairs separated by three or more bonds
/// (1-2 and 1-3 pairs excluded) closer than `clash_ratio × (r_vdw,a + r_vdw,b)`.
pub fn internal_clashes(
    topology: &Topology,
    positions: &[[f32; 3]],
    clash_ratio: f32,
) -> InternalClashCheck {
    let mut r = InternalClashCheck {
        checked: 0,
        clashes: 0,
        shortest_relative_distance: f64::INFINITY,
    };
    let ratio = f64::from(clash_ratio);
    for p in topology.non_bonded_pairs() {
        let rel = dist(positions[p.a as usize], positions[p.b as usize]) / f64::from(p.vdw_sum);
        r.checked += 1;
        r.shortest_relative_distance = r.shortest_relative_distance.min(rel);
        if rel < ratio {
            r.clashes += 1;
        }
    }
    r
}

/// Signed distances in Å of `points` from their least-squares plane (through
/// the centroid, normal to the smallest principal axis). The normal's sign is
/// arbitrary, so only magnitudes and the split into two sides are meaningful.
/// Empty for fewer than three points.
pub fn plane_distances(points: &[[f32; 3]]) -> Vec<f64> {
    if points.len() < 3 {
        return Vec::new();
    }
    let c = mean(points.iter().copied());
    let mut cov = [[0.0f64; 3]; 3];
    for p in points {
        let d = sub(v3(*p), c);
        for i in 0..3 {
            for j in 0..3 {
                cov[i][j] += d[i] * d[j];
            }
        }
    }
    let (vals, vecs) = jacobi_eigen(cov);
    let k = (0..3)
        .min_by(|&a, &b| vals[a].total_cmp(&vals[b]))
        .unwrap_or(0);
    let normal = [vecs[0][k], vecs[1][k], vecs[2][k]];
    points.iter().map(|p| dot(sub(v3(*p), c), normal)).collect()
}

/// Largest distance in Å of any of `points` from their least-squares plane
/// (see [`plane_distances`]). 0.0 for fewer than four points.
pub fn max_distance_from_plane(points: &[[f32; 3]]) -> f64 {
    if points.len() < 4 {
        return 0.0;
    }
    plane_distances(points)
        .into_iter()
        .map(f64::abs)
        .fold(0.0, f64::max)
}

/// Result of [`aromatic_ring_flatness`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlatnessCheck {
    pub rings_checked: usize,
    pub rings_pass: usize,
    /// Largest atom distance from its ring's best-fit plane, Å (0.0 if no rings).
    pub max_distance_from_plane: f64,
}

impl FlatnessCheck {
    pub fn pass(&self) -> bool {
        self.rings_pass == self.rings_checked
    }
}

/// Every perceived aromatic 5-/6-ring has all atoms within `threshold` Å of its
/// best-fit plane.
pub fn aromatic_ring_flatness(
    topology: &Topology,
    positions: &[[f32; 3]],
    threshold: f32,
) -> FlatnessCheck {
    let mut r = FlatnessCheck {
        rings_checked: 0,
        rings_pass: 0,
        max_distance_from_plane: 0.0,
    };
    let mut buf = Vec::with_capacity(6);
    for ring in topology.aromatic_rings() {
        buf.clear();
        buf.extend(ring.iter().map(|&i| positions[i as usize]));
        let d = max_distance_from_plane(&buf);
        r.rings_checked += 1;
        if d <= f64::from(threshold) {
            r.rings_pass += 1;
        }
        r.max_distance_from_plane = r.max_distance_from_plane.max(d);
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::BondOrder;
    use mold_mol::Element;

    fn hexagon(z_bump: f32) -> Vec<[f32; 3]> {
        (0..6)
            .map(|k| {
                let t = std::f32::consts::FRAC_PI_3 * k as f32;
                [
                    1.39 * t.cos(),
                    1.39 * t.sin(),
                    if k == 0 { z_bump } else { 0.0 },
                ]
            })
            .collect()
    }

    fn benzene() -> Topology {
        let bonds: Vec<_> = (0..6u32)
            .map(|i| {
                let o = if i % 2 == 0 {
                    BondOrder::Double
                } else {
                    BondOrder::Single
                };
                (i, (i + 1) % 6, o)
            })
            .collect();
        Topology::new(&[Element::C; 6], &bonds).unwrap()
    }

    #[test]
    fn ideal_benzene_passes_everything() {
        let t = benzene();
        let p = hexagon(0.0);
        assert!(bond_lengths(&t, &p, 0.25).pass());
        assert!(bond_angles(&t, &p, 0.25).pass());
        assert!(internal_clashes(&t, &p, 0.7).pass());
        let f = aromatic_ring_flatness(&t, &p, 0.25);
        assert_eq!((f.rings_checked, f.rings_pass), (1, 1));
        assert!(f.max_distance_from_plane < 1e-6);
    }

    #[test]
    fn a_puckered_atom_breaks_flatness() {
        let t = benzene();
        let f = aromatic_ring_flatness(&t, &hexagon(0.7), 0.25);
        assert!(!f.pass());
        assert!(f.max_distance_from_plane > 0.25);
    }

    #[test]
    fn plane_distance_of_a_tilted_square_is_zero() {
        let pts = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 1.0],
            [1.0, 1.0, 1.0],
            [0.0, 1.0, 0.0],
        ];
        assert!(max_distance_from_plane(&pts) < 1e-6);
    }
}
