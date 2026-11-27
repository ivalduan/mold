//! Seeded synthetic pose tables with plausible values in every column, for the
//! examples, tests and benches of the panels. Not chemistry: no pose behind a
//! row exists. Real pose tables come from the `nodes` workstream.

use mold_mol::pose_columns as col;
use polars::prelude::{Column, DataFrame};

/// Poses per ligand (`model_rank` runs 1..=this).
pub const POSES_PER_LIGAND: usize = 5;

/// A pose table of `rows` poses, the same for the same `seed`.
///
/// `pose_id` is `0..rows` in order; ligands come in blocks of
/// [`POSES_PER_LIGAND`]. About 3 % of `strain_kcal` and 1 % of `pocket_rmsd`
/// values are null, and `gate_pass` is null where either is, so every view
/// has missing values to show. `gate_pass` = no more than 2 clashes, strain
/// under 8 kcal/mol and pocket RMSD under 2.5 Å (roughly half the poses pass).
pub fn pose_table(rows: usize, seed: u64) -> DataFrame {
    let mut rng = SplitMix64(seed ^ 0x6d6f_6c64); // "mold"
    let n = rows;
    let mut pose_id = Vec::with_capacity(n);
    let mut ligand_id = Vec::with_capacity(n);
    let mut model_rank = Vec::with_capacity(n);
    let mut plddt = Vec::with_capacity(n);
    let mut iptm = Vec::with_capacity(n);
    let mut affinity = Vec::with_capacity(n);
    let mut binder = Vec::with_capacity(n);
    let mut clashes = Vec::with_capacity(n);
    let mut strain = Vec::with_capacity(n);
    let mut hbonds = Vec::with_capacity(n);
    let mut salt = Vec::with_capacity(n);
    let mut rmsd = Vec::with_capacity(n);
    let mut gate = Vec::with_capacity(n);

    let mut ligand_quality = 0.0;
    for i in 0..n {
        let rank = i % POSES_PER_LIGAND;
        if rank == 0 {
            // Per-ligand quality in [0, 1): good ligands get good poses.
            ligand_quality = rng.unit();
        }
        // Lower ranks are the model's better guesses.
        let q = (ligand_quality * 0.7 + rng.unit() * 0.3 - rank as f64 * 0.04).clamp(0.0, 1.0);

        pose_id.push(i as u32);
        ligand_id.push(format!("LIG-{:04}", i / POSES_PER_LIGAND + 1));
        model_rank.push(rank as u32 + 1);
        plddt.push(Some(
            round(40.0 + 55.0 * q + rng.normal() * 3.0, 2).clamp(20.0, 98.0),
        ));
        iptm.push(Some(
            round(0.25 + 0.7 * q + rng.normal() * 0.04, 4).clamp(0.05, 0.98),
        ));
        // Boltz-2 style affinity: log10(IC50 / µM), lower binds tighter.
        affinity.push(Some(round(2.5 - 4.0 * q + rng.normal() * 0.5, 3)));
        binder.push(Some(round(sigmoid((q - 0.5) * 8.0 + rng.normal()), 4)));
        let c = (rng.unit() * rng.unit() * 9.0 * (1.2 - q)).floor() as u32;
        clashes.push(Some(c));
        let s = (rng.unit() > 0.03).then(|| round(rng.unit() * 14.0 * (1.1 - q) + 0.3, 2));
        strain.push(s);
        hbonds.push(Some((rng.unit() * 4.0 + q * 3.0).floor() as u32));
        salt.push(Some((rng.unit() * rng.unit() * 3.5).floor() as u32));
        let r = (rng.unit() > 0.01).then(|| round(0.3 + (1.0 - q) * 4.5 * rng.unit() + 0.2, 3));
        rmsd.push(r);
        gate.push(match (s, r) {
            (Some(s), Some(r)) => Some(c <= 2 && s < 8.0 && r < 2.5),
            _ => None,
        });
    }

    DataFrame::new(
        n,
        vec![
            Column::new(col::POSE_ID.into(), pose_id),
            Column::new(col::LIGAND_ID.into(), ligand_id),
            Column::new(col::MODEL_RANK.into(), model_rank),
            Column::new(col::PLDDT.into(), plddt),
            Column::new(col::IPTM.into(), iptm),
            Column::new(col::AFFINITY_PRED.into(), affinity),
            Column::new(col::BINDER_PROB.into(), binder),
            Column::new(col::CLASH_COUNT.into(), clashes),
            Column::new(col::STRAIN_KCAL.into(), strain),
            Column::new(col::N_HBONDS.into(), hbonds),
            Column::new(col::N_SALTBRIDGE.into(), salt),
            Column::new(col::POCKET_RMSD.into(), rmsd),
            Column::new(col::GATE_PASS.into(), gate),
        ],
    )
    .expect("columns of equal length")
}

fn round(x: f64, decimals: i32) -> f64 {
    let f = 10f64.powi(decimals);
    (x * f).round() / f
}

fn sigmoid(x: f64) -> f64 {
    1.0 / (1.0 + (-x).exp())
}

/// SplitMix64: tiny, seeded, good enough for demo data.
struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Roughly standard normal (Irwin–Hall, 4 terms).
    fn normal(&mut self) -> f64 {
        ((0..4).map(|_| self.unit()).sum::<f64>() - 2.0) * 3f64.sqrt()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mold_mol::pose_table::SCHEMA;

    #[test]
    fn synthetic_table_has_the_contract_schema_and_is_seeded() {
        let df = pose_table(300, 1);
        assert_eq!(df.height(), 300);
        for (name, ty) in SCHEMA {
            assert_eq!(df.column(name).unwrap().dtype(), &ty.to_polars(), "{name}");
        }
        assert!(pose_table(300, 1).equals_missing(&df));
        assert!(!pose_table(300, 2).equals_missing(&df));
    }
}
