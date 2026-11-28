//! What counts as protein (product-owner decision, 2026-11-28).
//!
//! `geom`'s kernels take whatever atoms they are given; picking them is the node's job
//! (`crates/geom/CLAUDE.md`, "What counts as protein is the caller's choice"). The two
//! settings differ on **modified residues** — 4WKQ deposits Cys797 as the S-oxidised
//! `CSX`, a HETATM residue that is part of the polymer:
//!
//! | `protein` | Protein is | 4WKQ clash pose |
//! |---|---|---|
//! | `polymer` (default) | every residue of a polymer chain, modified residues included | **65 clashes** |
//! | `posebusters` | ATOM records only; every HETATM residue is a cofactor | **57 clashes** |
//!
//! The default is `polymer` because physically a modified residue *is* protein;
//! `posebusters` mirrors what Python PoseBusters sees, so numbers can be compared with
//! it (`crates/geom/tests/posebusters.rs`). Waters are never protein under either rule.
//!
//! It is a parameter, so it is part of the item's cache key: changing it re-gates.

use mold_geom::Receptor;
use mold_graph::{NodeError, ParamSpec, Params};
use mold_mol::{Arena, PoseId};

/// Which atoms of a structure the protein checks see.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProteinRule {
    /// Every residue of a polymer chain, modified residues included. The default.
    #[default]
    Polymer,
    /// ATOM records only: every HETATM residue is a cofactor, as PoseBusters files them.
    PoseBusters,
}

/// Name of the parameter every node with a protein/ligand split declares.
pub const PARAM: &str = "protein";

const POLYMER: &str = "polymer";
const POSEBUSTERS: &str = "posebusters";

/// Residue names that are never protein, under either rule.
const WATERS: [&str; 3] = ["HOH", "WAT", "DOD"];

/// A chain is a polymer when it has at least this many residues one of which is a
/// standard amino acid or nucleotide. Two residues is the smallest thing that can be
/// linked, and it keeps a lone ligand chain (one residue) out.
const MIN_POLYMER_RESIDUES: usize = 2;

/// The 20 standard amino acids plus the nucleotides: enough to recognise a polymer
/// chain. A modified residue need not be here — its chain is identified by its
/// neighbours.
const STANDARD: [&str; 33] = [
    "ALA", "ARG", "ASN", "ASP", "CYS", "GLN", "GLU", "GLY", "HIS", "ILE", "LEU", "LYS", "MET",
    "PHE", "PRO", "SER", "THR", "TRP", "TYR", "VAL", "A", "C", "G", "U", "I", "DA", "DC", "DG",
    "DT", "DI", "DU", "SEC", "PYL",
];

impl ProteinRule {
    /// The parameter declaration, with its documentation.
    pub fn param_spec() -> ParamSpec {
        ParamSpec::new(PARAM, POLYMER).doc(
            "what counts as protein: `polymer` (every residue of a polymer chain, \
             modified residues included) or `posebusters` (ATOM records only)",
        )
    }

    pub fn from_params(params: &Params) -> Result<Self, NodeError> {
        match params.str(PARAM)? {
            POLYMER => Ok(Self::Polymer),
            POSEBUSTERS => Ok(Self::PoseBusters),
            other => Err(NodeError::msg(format!(
                "`{PARAM}` is {other:?}; it is `{POLYMER}` or `{POSEBUSTERS}`"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Polymer => POLYMER,
            Self::PoseBusters => POSEBUSTERS,
        }
    }

    /// `pose`'s protein atoms as a `geom` receptor. `None` if the arena has no such
    /// pose; a receptor with no atoms if the rule keeps none.
    pub fn receptor(self, arena: &Arena, pose: PoseId) -> Option<Receptor> {
        let keep = self.filter(arena, pose)?;
        Receptor::from_arena_filtered(arena, pose, |i| keep[i])
    }

    /// Per **global** atom index of the whole arena: whether this rule keeps the atom.
    fn filter(self, arena: &Arena, pose: PoseId) -> Option<Vec<bool>> {
        let atoms = arena.pose(pose)?.atoms.clone();
        let polymer: Vec<bool> = arena
            .chains()
            .iter()
            .map(|c| {
                let residues = &arena.residues()[c.residues.clone()];
                let named: Vec<&str> = residues
                    .iter()
                    .filter(|r| !is_water(&r.name))
                    .map(|r| r.name.as_str())
                    .collect();
                named.len() >= MIN_POLYMER_RESIDUES && named.iter().any(|n| is_standard(n))
            })
            .collect();
        let mut keep = vec![false; arena.atom_count()];
        for i in atoms {
            let r = &arena.residues()[arena.residue_indices()[i] as usize];
            keep[i] = !is_water(&r.name)
                && match self {
                    Self::Polymer => polymer[arena.chain_indices()[i] as usize],
                    Self::PoseBusters => !r.hetero,
                };
        }
        Some(keep)
    }
}

fn is_water(name: &str) -> bool {
    WATERS.contains(&name.trim().to_ascii_uppercase().as_str())
}

fn is_standard(name: &str) -> bool {
    STANDARD.contains(&name.trim().to_ascii_uppercase().as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(v: &str) -> Params {
        let mut p = Params::new();
        p.set(PARAM, v);
        p
    }

    #[test]
    fn the_parameter_names_both_rules_and_refuses_anything_else() {
        assert_eq!(ProteinRule::param_spec().default, POLYMER.into());
        assert_eq!(
            ProteinRule::from_params(&params(POLYMER)).unwrap(),
            ProteinRule::Polymer
        );
        assert_eq!(
            ProteinRule::from_params(&params(POSEBUSTERS)).unwrap(),
            ProteinRule::PoseBusters
        );
        let e = ProteinRule::from_params(&params("atoms")).unwrap_err();
        assert!(e.to_string().contains("posebusters"), "{e}");
    }
}
