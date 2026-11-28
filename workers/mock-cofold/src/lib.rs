//! The deterministic core of the mock co-fold worker, shared by the binary and by
//! tests that need a Rust reference for its output (parity: the same item cooked
//! in-process and through the worker must give identical results).

pub mod double;

use std::collections::BTreeMap;

/// Where the worker writes pose `sample` of ligand record `ligand`, relative to the
/// run directory.
pub fn pose_path(ligand: usize, sample: u32) -> String {
    format!("poses/lig{ligand}_s{sample}.cif")
}

/// The scores of pose `sample` of ligand record `ligand` under `seed`: plausible
/// co-folding metrics under the pose-table column names, rounded to 4 decimals so
/// the wire bytes are short and stable.
pub fn scores(seed: u64, ligand: u64, sample: u64) -> BTreeMap<String, f64> {
    let mut rng = Rng::new(seed, ligand, sample);
    let round = |x: f64| (x * 1e4).round() / 1e4;
    BTreeMap::from([
        ("plddt".into(), round(0.60 + 0.35 * rng.unit())),
        ("iptm".into(), round(0.40 + 0.50 * rng.unit())),
        ("affinity_pred".into(), round(5.0 + 4.0 * rng.unit())),
        ("binder_prob".into(), round(rng.unit())),
    ])
}

/// SplitMix64: tiny, seedable, identical on every platform.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64, ligand: u64, sample: u64) -> Self {
        let mut r = Rng(seed ^ 0x9E37_79B9_7F4A_7C15);
        for v in [ligand, sample] {
            r.0 ^= v.wrapping_mul(0xBF58_476D_1CE4_E5B9);
            r.next_u64();
        }
        r
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scores_are_seeded_plausible_and_stable() {
        let a = scores(7, 0, 1);
        assert_eq!(a, scores(7, 0, 1));
        assert_ne!(a, scores(7, 0, 2));
        assert_ne!(a, scores(8, 0, 1));
        assert!((0.6..0.95).contains(&a["plddt"]));
        assert!((0.4..0.9).contains(&a["iptm"]));
        assert!((5.0..9.0).contains(&a["affinity_pred"]));
        assert!((0.0..1.0).contains(&a["binder_prob"]));
        // The generator is pinned: a change here changes every recorded run.
        assert_eq!(Rng::new(0, 0, 0).next_u64(), 0xF88B_B8A8_724C_81EC);
        assert_eq!(pose_path(3, 1), "poses/lig3_s1.cif");
    }
}
