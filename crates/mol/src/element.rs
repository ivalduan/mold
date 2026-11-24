use std::fmt;

/// Chemical element, stored as its atomic number in one byte.
///
/// `0` means unknown (the file gave no recognisable element). One byte per atom
/// keeps the per-atom element array small enough to upload next to positions.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Element(pub u8);

impl Element {
    pub const UNKNOWN: Element = Element(0);
    pub const H: Element = Element(1);
    pub const C: Element = Element(6);
    pub const N: Element = Element(7);
    pub const O: Element = Element(8);
    pub const F: Element = Element(9);
    pub const P: Element = Element(15);
    pub const S: Element = Element(16);
    pub const CL: Element = Element(17);

    /// Atomic number; `0` for unknown.
    pub const fn atomic_number(self) -> u8 {
        self.0
    }

    pub const fn is_unknown(self) -> bool {
        self.0 == 0
    }

    /// Element from a symbol, case-insensitive (`"CL"`, `"Cl"`, `"cl"`).
    pub fn from_symbol(symbol: &str) -> Option<Element> {
        pdbtbx::Element::from_symbol(symbol.trim()).and_then(Element::from_pdbtbx)
    }

    /// Upper-case symbol as written in PDB files (`"CL"`), or `"X"` for unknown.
    pub fn symbol(self) -> &'static str {
        pdbtbx::Element::new(usize::from(self.0)).map_or("X", |e| e.symbol())
    }

    pub(crate) fn from_pdbtbx(e: pdbtbx::Element) -> Option<Element> {
        u8::try_from(e.atomic_number()).ok().map(Element)
    }
}

impl fmt::Debug for Element {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Element({})", self.symbol())
    }
}

impl fmt::Display for Element {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.symbol())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbols_round_trip() {
        assert_eq!(Element::from_symbol("CL"), Some(Element::CL));
        assert_eq!(Element::from_symbol("c"), Some(Element::C));
        assert_eq!(Element::CL.symbol(), "CL");
        assert_eq!(Element::UNKNOWN.symbol(), "X");
        assert_eq!(Element::from_symbol("Qq"), None);
    }
}
