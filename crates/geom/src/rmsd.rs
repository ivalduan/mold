//! Accuracy against a reference pose: plain RMSD, superposed (Kabsch) RMSD and
//! centroid distance.
//!
//! All three take two position slices **in the same atom order**: atom `i` of
//! `pose` corresponds to atom `i` of `reference`. Symmetry-equivalent atom
//! mappings (graph automorphisms) are not searched; see the crate's Design notes.

use crate::linalg::{dot, jacobi_eigen, mean, sub, v3};

/// Root-mean-square deviation **without** superposition, in Å. This is the
/// docking RMSD: both poses live in the receptor frame.
///
/// Returns `None` if the slices differ in length or are empty.
pub fn rmsd(pose: &[[f32; 3]], reference: &[[f32; 3]]) -> Option<f64> {
    if pose.len() != reference.len() || pose.is_empty() {
        return None;
    }
    let sum: f64 = pose
        .iter()
        .zip(reference)
        .map(|(a, b)| {
            let d = sub(v3(*a), v3(*b));
            dot(d, d)
        })
        .sum();
    Some((sum / pose.len() as f64).sqrt())
}

/// RMSD after the optimal rigid superposition of `pose` onto `reference`
/// (rotation + translation, no reflection), in Å.
///
/// Uses Horn's quaternion method: the optimal rotation is the top eigenvector
/// of a 4×4 symmetric matrix, and the RMSD follows from its eigenvalue without
/// rotating anything.
///
/// Returns `None` if the slices differ in length or are empty.
pub fn kabsch_rmsd(pose: &[[f32; 3]], reference: &[[f32; 3]]) -> Option<f64> {
    if pose.len() != reference.len() || pose.is_empty() {
        return None;
    }
    let ca = mean(pose.iter().copied());
    let cb = mean(reference.iter().copied());
    let mut s = [[0.0f64; 3]; 3];
    let mut e0 = 0.0;
    for (a, b) in pose.iter().zip(reference) {
        let x = sub(v3(*a), ca);
        let y = sub(v3(*b), cb);
        e0 += dot(x, x) + dot(y, y);
        for i in 0..3 {
            for j in 0..3 {
                s[i][j] += x[i] * y[j];
            }
        }
    }
    let [[sxx, sxy, sxz], [syx, syy, syz], [szx, szy, szz]] = s;
    let k = [
        [sxx + syy + szz, syz - szy, szx - sxz, sxy - syx],
        [syz - szy, sxx - syy - szz, sxy + syx, szx + sxz],
        [szx - sxz, sxy + syx, -sxx + syy - szz, syz + szy],
        [sxy - syx, szx + sxz, syz + szy, -sxx - syy + szz],
    ];
    let (values, _) = jacobi_eigen(k);
    let lambda = values.into_iter().fold(f64::NEG_INFINITY, f64::max);
    let msd = (e0 - 2.0 * lambda) / pose.len() as f64;
    Some(msd.max(0.0).sqrt())
}

/// Centroid (unweighted mean position) of `points`, `None` if empty.
pub fn centroid(points: &[[f32; 3]]) -> Option<[f64; 3]> {
    (!points.is_empty()).then(|| mean(points.iter().copied()))
}

/// Distance in Å between the centroids of `pose` and `reference`. The two
/// slices need not have the same length. `None` if either is empty.
pub fn centroid_distance(pose: &[[f32; 3]], reference: &[[f32; 3]]) -> Option<f64> {
    let d = sub(centroid(pose)?, centroid(reference)?);
    Some(dot(d, d).sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_poses_have_zero_rmsd() {
        let p = [[0.0, 0.0, 0.0], [1.5, 0.0, 0.0], [1.5, 1.5, 0.3]];
        assert_eq!(rmsd(&p, &p), Some(0.0));
        assert!(kabsch_rmsd(&p, &p).unwrap() < 1e-6);
    }

    #[test]
    fn translation_shows_in_plain_rmsd_only() {
        let p = [[0.0, 0.0, 0.0], [1.5, 0.0, 0.0], [1.5, 1.5, 0.3]];
        let q = p.map(|[x, y, z]| [x + 3.0, y, z + 4.0]);
        assert!((rmsd(&p, &q).unwrap() - 5.0).abs() < 1e-6);
        assert!((centroid_distance(&p, &q).unwrap() - 5.0).abs() < 1e-6);
        assert!(kabsch_rmsd(&p, &q).unwrap() < 1e-5);
    }

    #[test]
    fn mirror_image_is_not_superposable() {
        // A chiral tetrahedron and its mirror: Kabsch (rotations only) must not reach 0.
        let p = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 2.0, 0.0],
            [0.0, 0.0, 3.0],
        ];
        let m = p.map(|[x, y, z]| [-x, y, z]);
        assert!(kabsch_rmsd(&p, &m).unwrap() > 0.1);
    }

    #[test]
    fn mismatched_lengths_are_rejected() {
        assert_eq!(rmsd(&[[0.0; 3]], &[]), None);
        assert_eq!(kabsch_rmsd(&[], &[]), None);
        assert_eq!(centroid_distance(&[], &[[0.0; 3]]), None);
    }
}
