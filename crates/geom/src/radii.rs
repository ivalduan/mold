//! Per-element radii: van der Waals (clashes, volume overlap) and covalent
//! (reference bond lengths, covalent-attachment detection).

use mold_mol::Element;

use crate::topology::BondOrder;

/// van der Waals radius in Å.
///
/// Values are RDKit's periodic table (`PeriodicTable::getRvdw`, Bondi-derived),
/// which is what PoseBusters' protein–ligand checks divide by: its reported
/// `most_extreme_*_vdw_protein` are exactly C 1.7, N 1.6, S 1.8. Unlisted
/// elements get 2.0 Å.
pub fn vdw_radius(element: Element) -> f32 {
    match element.atomic_number() {
        1 => 1.2,
        5 => 1.92,
        6 => 1.7,
        7 => 1.6,
        8 => 1.55,
        9 => 1.5,
        11 => 2.27,
        12 => 1.73,
        14 => 2.1,
        15 => 1.8,
        16 => 1.8,
        17 => 1.8,
        19 => 2.75,
        20 => 2.31,
        29 => 1.4,
        30 => 1.39,
        34 => 1.9,
        35 => 1.9,
        53 => 2.1,
        _ => 2.0,
    }
}

/// Covalent radius in Å for a bond of the given order (Pyykkö & Atsumi 2009
/// single/double/triple-bond radii). Aromatic is the mean of single and double.
/// Unlisted elements use their single-bond radius, 1.5 Å if unknown.
pub fn covalent_radius(element: Element, order: BondOrder) -> f32 {
    let (single, double, triple) = match element.atomic_number() {
        1 => (0.32, 0.32, 0.32),
        5 => (0.85, 0.78, 0.73),
        6 => (0.75, 0.67, 0.60),
        7 => (0.71, 0.60, 0.54),
        8 => (0.63, 0.57, 0.53),
        9 => (0.64, 0.59, 0.53),
        14 => (1.16, 1.07, 1.02),
        15 => (1.11, 1.02, 0.94),
        16 => (1.03, 0.94, 0.95),
        17 => (0.99, 0.95, 0.93),
        34 => (1.16, 1.07, 1.07),
        35 => (1.14, 1.09, 1.10),
        53 => (1.33, 1.29, 1.25),
        26 => (1.16, 1.16, 1.16),
        30 => (1.18, 1.18, 1.18),
        12 => (1.39, 1.39, 1.39),
        _ => (1.5, 1.5, 1.5),
    };
    match order {
        BondOrder::Single => single,
        BondOrder::Double => double,
        BondOrder::Triple => triple,
        BondOrder::Aromatic => 0.5 * (single + double),
    }
}

/// Reference length in Å of a bond between `a` and `b`: the sum of their
/// covalent radii for that order.
pub fn reference_bond_length(a: Element, b: Element, order: BondOrder) -> f32 {
    covalent_radius(a, order) + covalent_radius(b, order)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_lengths_are_textbook_close() {
        let c = Element::C;
        let cases = [
            (c, c, BondOrder::Single, 1.53),
            (c, c, BondOrder::Double, 1.34),
            (c, c, BondOrder::Triple, 1.20),
            (c, c, BondOrder::Aromatic, 1.39),
            (c, Element::N, BondOrder::Single, 1.47),
            (c, Element::O, BondOrder::Double, 1.21),
            (c, Element::CL, BondOrder::Single, 1.75),
            (c, Element::S, BondOrder::Single, 1.82),
        ];
        for (a, b, order, textbook) in cases {
            let r = reference_bond_length(a, b, order);
            assert!(
                (r - textbook).abs() / textbook < 0.06,
                "{a}-{b} {order:?}: {r} vs {textbook}"
            );
        }
    }
}
