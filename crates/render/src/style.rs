//! How atoms look: per-element colour and radii, and the representations.

use mold_mol::Element;

/// How the scene is drawn. One for the whole scene in this version.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Representation {
    /// Van der Waals spheres, no bonds.
    #[default]
    SpaceFilling,
    /// Small spheres (a quarter of the van der Waals radius) and thin sticks.
    BallAndStick,
    /// Sticks only, with round joints of the stick radius at the atoms.
    Sticks,
}

/// Radius of a stick (and of the joints in [`Representation::Sticks`]), Å.
pub const STICK_RADIUS: f32 = 0.15;

impl Representation {
    pub const ALL: [Representation; 3] = [Self::SpaceFilling, Self::BallAndStick, Self::Sticks];

    /// The radii the shaders use: an atom's sphere radius is
    /// `max(vdw * sphere_scale, sphere_min)`; sticks are drawn when
    /// `stick_radius > 0`.
    pub fn radii(self) -> Radii {
        match self {
            Self::SpaceFilling => Radii {
                sphere_scale: 1.0,
                sphere_min: 0.0,
                stick_radius: 0.0,
            },
            Self::BallAndStick => Radii {
                sphere_scale: 0.25,
                sphere_min: 0.0,
                stick_radius: STICK_RADIUS,
            },
            Self::Sticks => Radii {
                sphere_scale: 0.0,
                sphere_min: STICK_RADIUS,
                stick_radius: STICK_RADIUS,
            },
        }
    }

    /// The radius an atom of `element` is drawn with.
    pub fn sphere_radius(self, element: Element) -> f32 {
        let radii = self.radii();
        (vdw_radius(element) * radii.sphere_scale).max(radii.sphere_min)
    }

    pub fn draws_sticks(self) -> bool {
        self.radii().stick_radius > 0.0
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::SpaceFilling => "Spheres",
            Self::BallAndStick => "Ball & stick",
            Self::Sticks => "Sticks",
        }
    }
}

/// See [`Representation::radii`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Radii {
    pub sphere_scale: f32,
    pub sphere_min: f32,
    pub stick_radius: f32,
}

/// Van der Waals radius, Å (Bondi 1964; Rowland & Taylor 1996 for H).
/// Unknown and unlisted elements get carbon's.
pub fn vdw_radius(element: Element) -> f32 {
    match element.atomic_number() {
        1 => 1.10,
        5 => 1.92,
        6 => 1.70,
        7 => 1.55,
        8 => 1.52,
        9 => 1.47,
        11 => 2.27,
        12 => 1.73,
        14 => 2.10,
        15 => 1.80,
        16 => 1.80,
        17 => 1.75,
        19 => 2.75,
        20 => 2.31,
        26 => 1.94,
        29 => 1.40,
        30 => 1.39,
        34 => 1.90,
        35 => 1.85,
        53 => 1.98,
        _ => 1.70,
    }
}

/// Single-bond covalent radius, Å (Cordero et al. 2008). Unknown elements
/// get carbon's; unlisted ones 1.5 Å (metals).
pub fn covalent_radius(element: Element) -> f32 {
    match element.atomic_number() {
        0 => 0.76,
        1 => 0.31,
        5 => 0.84,
        6 => 0.76,
        7 => 0.71,
        8 => 0.66,
        9 => 0.57,
        11 => 1.66,
        12 => 1.41,
        14 => 1.11,
        15 => 1.07,
        16 => 1.05,
        17 => 1.02,
        19 => 2.03,
        20 => 1.76,
        26 => 1.32,
        29 => 1.32,
        30 => 1.22,
        34 => 1.20,
        35 => 1.20,
        53 => 1.39,
        _ => 1.50,
    }
}

/// Linear RGB(A) colour of an element: Jmol's CPK palette, carbon a little
/// lighter so it reads on a dark background. Unlisted elements are pink.
pub fn element_color(element: Element) -> [f32; 4] {
    let srgb: u32 = match element.atomic_number() {
        1 => 0xffffff,
        5 => 0xffb5b5,
        6 => 0xa0a0a0,
        7 => 0x3050f8,
        8 => 0xff0d0d,
        9 => 0x90e050,
        11 => 0xab5cf2,
        12 => 0x8aff00,
        14 => 0xf0c8a0,
        15 => 0xff8000,
        16 => 0xffff30,
        17 => 0x1ff01f,
        19 => 0x8f40d4,
        20 => 0x3dff00,
        26 => 0xe06633,
        29 => 0xc88033,
        30 => 0x7d80b0,
        34 => 0xffa100,
        35 => 0xa62929,
        53 => 0x940094,
        _ => 0xff1493,
    };
    let channel = |shift: u32| srgb_to_linear(((srgb >> shift) & 0xff) as f32 / 255.0);
    [channel(16), channel(8), channel(0), 1.0]
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// One entry of the shaders' element table.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct ElementStyle {
    pub color: [f32; 4],
    /// x: van der Waals radius.
    pub radius: [f32; 4],
}

/// The table the shaders index by atomic number.
pub(crate) fn element_table() -> [ElementStyle; 256] {
    std::array::from_fn(|z| {
        let element = Element(z as u8);
        ElementStyle {
            color: element_color(element),
            radius: [vdw_radius(element), 0.0, 0.0, 0.0],
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn representation_radii() {
        assert_eq!(Representation::SpaceFilling.sphere_radius(Element::C), 1.70);
        assert!(!Representation::SpaceFilling.draws_sticks());
        assert_eq!(
            Representation::Sticks.sphere_radius(Element::C),
            STICK_RADIUS
        );
        assert_eq!(
            Representation::Sticks.sphere_radius(Element::H),
            STICK_RADIUS
        );
        let ball = Representation::BallAndStick.sphere_radius(Element::O);
        assert!(ball > STICK_RADIUS && ball < 0.5, "{ball}");
    }

    #[test]
    fn colours_are_linear_and_distinct() {
        let c = element_color(Element::C);
        let o = element_color(Element::O);
        assert_ne!(c, o);
        // sRGB 0xa0 is about 0.35 linear.
        assert!((c[0] - 0.351).abs() < 0.01, "{c:?}");
        assert_eq!(element_color(Element::H), [1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn table_is_indexed_by_atomic_number() {
        let table = element_table();
        assert_eq!(table[8].color, element_color(Element::O));
        assert_eq!(table[16].radius[0], 1.80);
        assert_eq!(size_of::<ElementStyle>(), 32);
    }
}
