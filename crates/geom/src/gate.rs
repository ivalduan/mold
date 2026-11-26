//! The Pose Gate aggregate: every physical-validity check for one pose, each
//! as a number plus a verdict, under one config whose fields are what the Pose
//! Gate node exposes as parameters.

use crate::ligand::{
    BondAngleCheck, BondLengthCheck, FlatnessCheck, InternalClashCheck, aromatic_ring_flatness,
    bond_angles, bond_lengths, internal_clashes,
};
use crate::receptor::{
    CovalentAttachment, DistanceParams, ProteinLigandDistances, Receptor, VolumeParams,
    covalent_exclusions, detect_covalent_attachments, protein_ligand_distances, volume_overlap,
};
use crate::rmsd::{centroid_distance, kabsch_rmsd, rmsd};
use crate::topology::Topology;

/// Thresholds of the gate. Defaults follow PoseBusters 0.6's `redock`/`dock`
/// configuration wherever geom computes the same quantity; see the crate's
/// `CLAUDE.md` (Design notes) for provenance and divergences.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GateConfig {
    /// Bond length outlier: outside `(1 ± tol) ×` reference. Default 0.25
    /// (PoseBusters `threshold_bad_bond_length`).
    pub bond_length_tolerance: f32,
    /// Bond angle outlier: 1-3 distance outside `(1 ± tol) ×` reference.
    /// Default 0.25 (PoseBusters `threshold_bad_angle`).
    pub bond_angle_tolerance: f32,
    /// Internal clash: a non-bonded pair (≥ 3 bonds apart) closer than this
    /// fraction of its van der Waals radius sum. Default 0.5 (geom's own; see
    /// Design notes).
    pub internal_clash_ratio: f32,
    /// Aromatic ring flatness: max atom distance from the ring plane, Å.
    /// Default 0.25 (PoseBusters `threshold_flatness`).
    pub aromatic_flatness_threshold: f32,
    /// Protein–ligand clash: pair distance below this fraction of the vdW
    /// radius sum. Default 0.75 (PoseBusters `clash_cutoff`).
    pub protein_clash_cutoff: f32,
    /// Scale on the vdW radius sum for protein–ligand clashes. Default 1.0.
    pub protein_radius_scale: f32,
    /// The ligand must come within this many Å of the protein. Default 5.0
    /// (PoseBusters `max_distance`).
    pub max_distance_to_protein: f32,
    /// Radius of the protein neighbourhood examined around each ligand atom,
    /// Å. Default 6.0 (PoseBusters `search_distance`).
    pub search_distance: f32,
    /// Volume overlap: fail above this fraction of ligand volume inside the
    /// protein. Default 0.075 (PoseBusters `clash_cutoff` of `volume_overlap`).
    pub volume_overlap_cutoff: f32,
    /// Grid parameters of the volume overlap.
    pub volume: VolumeParams,
    /// Accuracy: RMSD to the reference at or below this, Å. Default 2.0.
    pub rmsd_threshold: f32,
    /// Detect a covalent ligand–protein bond automatically (in addition to any
    /// declared one) and exclude it from the clash check. Default `true`.
    pub detect_covalent: bool,
}

impl Default for GateConfig {
    fn default() -> Self {
        GateConfig {
            bond_length_tolerance: 0.25,
            bond_angle_tolerance: 0.25,
            internal_clash_ratio: 0.5,
            aromatic_flatness_threshold: 0.25,
            protein_clash_cutoff: 0.75,
            protein_radius_scale: 1.0,
            max_distance_to_protein: 5.0,
            search_distance: 6.0,
            volume_overlap_cutoff: 0.075,
            volume: VolumeParams::default(),
            rmsd_threshold: 2.0,
            detect_covalent: true,
        }
    }
}

impl GateConfig {
    fn distance_params(&self) -> DistanceParams {
        DistanceParams {
            clash_cutoff: self.protein_clash_cutoff,
            radius_scale: self.protein_radius_scale,
            search_distance: self.search_distance,
        }
    }
}

/// Accuracy against the reference (crystal) pose.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Accuracy {
    /// RMSD without superposition, Å.
    pub rmsd: f64,
    /// RMSD after optimal superposition, Å.
    pub kabsch_rmsd: f64,
    /// Distance between centroids, Å.
    pub centroid_distance: f64,
}

/// One verdict per check. `true` is good. Names follow PoseBusters' columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Verdicts {
    pub bond_lengths: bool,
    pub bond_angles: bool,
    pub internal_steric_clash: bool,
    pub aromatic_ring_flatness: bool,
    /// No protein–ligand clash, after excluding covalent attachments.
    pub minimum_distance_to_protein: bool,
    pub protein_ligand_maximum_distance: bool,
    pub volume_overlap_with_protein: bool,
    /// `None` when the gate has no reference pose.
    pub rmsd_le_2a: Option<bool>,
}

impl Verdicts {
    /// Physical validity: every check except accuracy passes. This is the
    /// Pose Gate's pass/fail (`gate_pass`); accuracy needs a reference pose
    /// and is reported separately.
    pub fn physically_valid(&self) -> bool {
        self.bond_lengths
            && self.bond_angles
            && self.internal_steric_clash
            && self.aromatic_ring_flatness
            && self.minimum_distance_to_protein
            && self.protein_ligand_maximum_distance
            && self.volume_overlap_with_protein
    }
}

/// Everything the gate found for one pose.
#[derive(Debug, Clone, PartialEq)]
pub struct GateReport {
    pub bond_lengths: BondLengthCheck,
    pub bond_angles: BondAngleCheck,
    pub internal_steric_clash: InternalClashCheck,
    pub aromatic_ring_flatness: FlatnessCheck,
    /// Protein–ligand distances exactly as PoseBusters computes them (no
    /// covalent exclusion): a covalent crystal pose clashes here.
    pub protein_raw: ProteinLigandDistances,
    /// The same with the covalent attachments' pairs excluded; the verdict
    /// uses this.
    pub protein: ProteinLigandDistances,
    /// Declared plus detected covalent attachments that were excluded.
    pub covalent: Vec<CovalentAttachment>,
    /// Fraction of ligand volume overlapping the protein.
    pub volume_overlap: f64,
    pub accuracy: Option<Accuracy>,
    pub verdicts: Verdicts,
}

impl GateReport {
    /// Shorthand for [`Verdicts::physically_valid`].
    pub fn pass(&self) -> bool {
        self.verdicts.physically_valid()
    }
}

/// Why a [`PoseGate`] could not be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateError {
    /// The reference pose has a different atom count from the ligand topology.
    ReferenceLength { expected: usize, got: usize },
}

impl std::fmt::Display for GateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GateError::ReferenceLength { expected, got } => {
                write!(
                    f,
                    "reference pose has {got} atoms, the ligand has {expected}"
                )
            }
        }
    }
}

impl std::error::Error for GateError {}

/// One receptor, one ligand topology, optionally a reference pose: evaluate
/// any number of poses of that ligand. Build once; [`PoseGate::evaluate`] is
/// `&self` and allocation-light.
#[derive(Debug)]
pub struct PoseGate {
    receptor: Receptor,
    topology: Topology,
    heavy: Vec<u32>,
    reference: Option<Vec<[f32; 3]>>,
    config: GateConfig,
}

impl PoseGate {
    pub fn new(receptor: Receptor, topology: Topology, config: GateConfig) -> PoseGate {
        let heavy = topology.heavy_atoms().collect();
        PoseGate {
            receptor,
            topology,
            heavy,
            reference: None,
            config,
        }
    }

    /// Add the reference pose (same atom order as the topology) that accuracy
    /// is measured against. RMSDs use heavy atoms only.
    pub fn with_reference(mut self, reference: &[[f32; 3]]) -> Result<PoseGate, GateError> {
        if reference.len() != self.topology.atom_count() {
            return Err(GateError::ReferenceLength {
                expected: self.topology.atom_count(),
                got: reference.len(),
            });
        }
        self.reference = Some(self.heavy.iter().map(|&i| reference[i as usize]).collect());
        Ok(self)
    }

    pub fn config(&self) -> &GateConfig {
        &self.config
    }

    pub fn receptor(&self) -> &Receptor {
        &self.receptor
    }

    pub fn topology(&self) -> &Topology {
        &self.topology
    }

    /// Run every check on one pose. `positions` is indexed like the topology's
    /// atoms. `covalent` declares known ligand–protein bonds (may be empty);
    /// with [`GateConfig::detect_covalent`] one more may be found.
    ///
    /// # Panics
    /// If `positions.len()` differs from the topology's atom count.
    pub fn evaluate(&self, positions: &[[f32; 3]], covalent: &[CovalentAttachment]) -> GateReport {
        assert_eq!(
            positions.len(),
            self.topology.atom_count(),
            "pose has a different atom count from the ligand topology"
        );
        let c = &self.config;
        let t = &self.topology;
        let elements = t.elements();

        let bond_lengths = bond_lengths(t, positions, c.bond_length_tolerance);
        let bond_angles = bond_angles(t, positions, c.bond_angle_tolerance);
        let internal = internal_clashes(t, positions, c.internal_clash_ratio);
        let flatness = aromatic_ring_flatness(t, positions, c.aromatic_flatness_threshold);

        let params = c.distance_params();
        let protein_raw =
            protein_ligand_distances(&self.receptor, positions, elements, &params, &[]);
        let mut attachments = covalent.to_vec();
        if c.detect_covalent && !protein_raw.no_clashes() {
            for a in detect_covalent_attachments(&self.receptor, positions, elements) {
                if !attachments.iter().any(|d| d.ligand_atom == a.ligand_atom) {
                    attachments.push(a);
                }
            }
        }
        let protein = if attachments.is_empty() {
            protein_raw
        } else {
            let exclude = covalent_exclusions(&self.receptor, t, &attachments);
            protein_ligand_distances(&self.receptor, positions, elements, &params, &exclude)
        };
        let overlap = volume_overlap(&self.receptor, positions, elements, &c.volume);

        let accuracy = self.reference.as_ref().map(|reference| {
            let pose: Vec<[f32; 3]> = self.heavy.iter().map(|&i| positions[i as usize]).collect();
            Accuracy {
                rmsd: rmsd(&pose, reference).unwrap_or(f64::NAN),
                kabsch_rmsd: kabsch_rmsd(&pose, reference).unwrap_or(f64::NAN),
                centroid_distance: centroid_distance(&pose, reference).unwrap_or(f64::NAN),
            }
        });

        let verdicts = Verdicts {
            bond_lengths: bond_lengths.pass(),
            bond_angles: bond_angles.pass(),
            internal_steric_clash: internal.pass(),
            aromatic_ring_flatness: flatness.pass(),
            minimum_distance_to_protein: protein.no_clashes(),
            protein_ligand_maximum_distance: protein_raw
                .within_max_distance(c.max_distance_to_protein),
            volume_overlap_with_protein: overlap <= f64::from(c.volume_overlap_cutoff),
            rmsd_le_2a: accuracy.map(|a| a.rmsd <= f64::from(c.rmsd_threshold)),
        };
        GateReport {
            bond_lengths,
            bond_angles,
            internal_steric_clash: internal,
            aromatic_ring_flatness: flatness,
            protein_raw,
            protein,
            covalent: attachments,
            volume_overlap: overlap,
            accuracy,
            verdicts,
        }
    }
}
