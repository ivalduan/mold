//! One-letter sequences, for the `Sequence` port (and, later, the MSA query).

use mold_mol::{Arena, PoseId};

/// The one-letter code of a residue name, `X` for anything else (a modified or
/// unknown residue). `None` for a water.
fn one_letter(name: &str) -> Option<char> {
    let n = name.trim().to_ascii_uppercase();
    Some(match n.as_str() {
        "HOH" | "WAT" | "DOD" => return None,
        "ALA" => 'A',
        "ARG" => 'R',
        "ASN" => 'N',
        "ASP" => 'D',
        "CYS" => 'C',
        "GLN" => 'Q',
        "GLU" => 'E',
        "GLY" => 'G',
        "HIS" => 'H',
        "ILE" => 'I',
        "LEU" => 'L',
        "LYS" => 'K',
        "MET" => 'M',
        "PHE" => 'F',
        "PRO" => 'P',
        "SER" => 'S',
        "THR" => 'T',
        "TRP" => 'W',
        "TYR" => 'Y',
        "VAL" => 'V',
        "SEC" => 'U',
        "PYL" => 'O',
        "A" | "DA" => 'A',
        "C" | "DC" => 'C',
        "G" | "DG" => 'G',
        "U" | "DU" => 'U',
        "T" | "DT" => 'T',
        _ => 'X',
    })
}

/// The pose's sequence: one-letter residues per chain, chains joined by `/` in chain
/// order. Waters are skipped; a chain that ends up empty is skipped too.
///
/// Gaps in the author numbering are **not** filled: this is the sequence of the
/// residues the file models, which is what a co-folding worker is given.
pub fn sequence(arena: &Arena, pose: PoseId) -> String {
    let Some(p) = arena.pose(pose) else {
        return String::new();
    };
    let mut chains = Vec::new();
    for chain in &arena.chains()[p.chains.clone()] {
        let s: String = arena.residues()[chain.residues.clone()]
            .iter()
            .filter_map(|r| one_letter(&r.name))
            .collect();
        if !s.is_empty() {
            chains.push(s);
        }
    }
    chains.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modified_residues_read_as_x_and_waters_are_skipped() {
        assert_eq!(one_letter("CSX"), Some('X'));
        assert_eq!(one_letter("hoh"), None);
        assert_eq!(one_letter("GLY"), Some('G'));
    }
}
