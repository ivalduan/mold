//! SPIKE ONLY (WS-00). 500 atom-like spheres, deterministic, so the spike
//! draws and picks the same scene on every run and in the tests.

use glam::Vec3;

use super::pick::Sphere;

/// One sphere as the impostor pipeline reads it: an instance.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SphereInstance {
    pub center: [f32; 3],
    pub radius: f32,
    pub color: [f32; 4],
}

impl SphereInstance {
    pub fn sphere(&self) -> Sphere {
        Sphere {
            center: Vec3::from(self.center),
            radius: self.radius,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SpikeScene {
    pub atoms: Vec<SphereInstance>,
    /// Bounding sphere of all atoms.
    pub center: Vec3,
    pub radius: f32,
}

impl SpikeScene {
    pub fn spheres(&self) -> impl Iterator<Item = Sphere> + '_ {
        self.atoms.iter().map(SphereInstance::sphere)
    }
}

pub const ATOM_COUNT: usize = 500;

/// 500 atoms scattered through a ball like a small protein's atoms, with van
/// der Waals radii and CPK colours of C, N, O and S.
pub fn spike_scene() -> SpikeScene {
    // (radius Å, colour, weight out of 100)
    const ELEMENTS: [(f32, [f32; 4], u32); 4] = [
        (1.70, [0.56, 0.56, 0.56, 1.0], 62),
        (1.55, [0.19, 0.31, 0.97, 1.0], 16),
        (1.52, [1.00, 0.05, 0.05, 1.0], 19),
        (1.80, [1.00, 0.78, 0.20, 1.0], 3),
    ];
    const BALL: f32 = 18.0;
    let mut rng = SplitMix64(0x6d6f_6c64_5350_494b); // "moldSPIK"
    let mut atoms = Vec::with_capacity(ATOM_COUNT);
    while atoms.len() < ATOM_COUNT {
        let p = Vec3::new(rng.unit(), rng.unit(), rng.unit()) * 2.0 - Vec3::ONE;
        if p.length_squared() > 1.0 {
            continue;
        }
        let roll = (rng.next() % 100) as u32;
        let mut acc = 0;
        let &(radius, color, _) = ELEMENTS
            .iter()
            .find(|(_, _, weight)| {
                acc += weight;
                roll < acc
            })
            .unwrap_or(&ELEMENTS[0]);
        atoms.push(SphereInstance {
            center: (p * BALL + Vec3::new(4.0, -2.0, 7.0)).into(),
            radius,
            color,
        });
    }
    let center = atoms.iter().map(|a| Vec3::from(a.center)).sum::<Vec3>() / atoms.len() as f32;
    let radius = atoms
        .iter()
        .map(|a| (Vec3::from(a.center) - center).length() + a.radius)
        .fold(0.0, f32::max);
    SpikeScene {
        atoms,
        center,
        radius,
    }
}

/// A tiny deterministic generator; the spike needs no `rand` dependency.
struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn five_hundred_atoms_deterministic_and_bounded() {
        let a = spike_scene();
        let b = spike_scene();
        assert_eq!(a.atoms.len(), ATOM_COUNT);
        assert_eq!(a.atoms, b.atoms);
        for sphere in a.spheres() {
            assert!((sphere.center - a.center).length() + sphere.radius <= a.radius + 1e-3);
        }
    }
}
