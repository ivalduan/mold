//! CPU ray casting against atom spheres and stick cylinders: the same
//! intersections the impostor shaders compute, so it is
//! the oracle the GPU pick is tested against, and a fallback where no GPU
//! readback exists. Linear in the atom count.

use glam::Vec3;
use mold_mol::Element;

use crate::camera::Ray;
use crate::style::Representation;

/// Distance along `ray` to the first point of the sphere in front of the
/// origin, or `None` if the ray misses.
pub fn ray_sphere(ray: &Ray, center: Vec3, radius: f32) -> Option<f32> {
    let oc = ray.origin - center;
    let b = oc.dot(ray.dir);
    let c = oc.length_squared() - radius * radius;
    let discriminant = b * b - c;
    if discriminant < 0.0 {
        return None;
    }
    let near = -b - discriminant.sqrt();
    (near > 0.0).then_some(near)
}

/// The nearest atom sphere hit, drawn as `representation` draws it (spheres
/// only; sticks are ignored): `(global atom index, distance)`.
/// `atoms` restricts the search, e.g. to visible poses.
pub fn cast_spheres(
    ray: &Ray,
    positions: &[[f32; 3]],
    elements: &[Element],
    representation: Representation,
    atoms: std::ops::Range<usize>,
) -> Option<(u32, f32)> {
    atoms
        .filter_map(|i| {
            let radius = representation.sphere_radius(elements[i]);
            ray_sphere(ray, Vec3::from(positions[i]), radius).map(|t| (i as u32, t))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
}

/// Distance along `ray` to the open cylinder of radius `radius` around the
/// segment `a → b`, and where along the segment it was hit (0 at `a`, 1 at
/// `b`). The same intersection the stick shader computes.
pub fn ray_cylinder(ray: &Ray, a: Vec3, b: Vec3, radius: f32) -> Option<(f32, f32)> {
    let ba = b - a;
    let oc = ray.origin - a;
    let baba = ba.dot(ba);
    let bard = ba.dot(ray.dir);
    let baoc = ba.dot(oc);
    let k2 = baba - bard * bard;
    if k2 < 1e-6 {
        return None;
    }
    let k1 = baba * oc.dot(ray.dir) - baoc * bard;
    let k0 = baba * oc.dot(oc) - baoc * baoc - radius * radius * baba;
    let h = k1 * k1 - k2 * k0;
    if h < 0.0 {
        return None;
    }
    let t = (-k1 - h.sqrt()) / k2;
    let y = baoc + t * bard;
    (t > 0.0 && (0.0..=baba).contains(&y)).then_some((t, y / baba))
}

/// The nearest stick hit (bonds as global atom pairs): `(atom, distance)`,
/// where the atom is the one whose half of the bond was hit.
pub fn cast_sticks(
    ray: &Ray,
    positions: &[[f32; 3]],
    bonds: &[[u32; 2]],
    radius: f32,
) -> Option<(u32, f32)> {
    bonds
        .iter()
        .filter_map(|&[a, b]| {
            let (pa, pb) = (
                Vec3::from(positions[a as usize]),
                Vec3::from(positions[b as usize]),
            );
            let (t, along) = ray_cylinder(ray, pa, pb, radius)?;
            Some((if along > 0.5 { b } else { a }, t))
        })
        .min_by(|x, y| x.1.total_cmp(&y.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cylinder_hits_the_right_half() {
        let r = ray(Vec3::new(0.3, 0.0, 10.0), -Vec3::Z);
        let (t, along) = ray_cylinder(&r, Vec3::ZERO, Vec3::X, 0.2).unwrap();
        assert!((t - 9.8).abs() < 1e-4, "{t}");
        assert!((along - 0.3).abs() < 1e-4);
        // Past the end of the open cylinder.
        let past = ray(Vec3::new(1.3, 0.0, 10.0), -Vec3::Z);
        assert!(ray_cylinder(&past, Vec3::ZERO, Vec3::X, 0.2).is_none());
        let positions = [[0.0; 3], [1.0, 0.0, 0.0]];
        let near_b = ray(Vec3::new(0.8, 0.0, 10.0), -Vec3::Z);
        assert_eq!(
            cast_sticks(&near_b, &positions, &[[0, 1]], 0.2).map(|h| h.0),
            Some(1)
        );
    }

    fn ray(origin: Vec3, dir: Vec3) -> Ray {
        Ray {
            origin,
            dir: dir.normalize(),
        }
    }

    #[test]
    fn hits_and_misses() {
        let center = Vec3::new(0.0, 0.0, -10.0);
        let t = ray_sphere(&ray(Vec3::ZERO, -Vec3::Z), center, 2.0).unwrap();
        assert!((t - 8.0).abs() < 1e-5);
        assert!(ray_sphere(&ray(Vec3::ZERO, Vec3::Z), center, 2.0).is_none());
        assert!(ray_sphere(&ray(Vec3::new(3.0, 0.0, 0.0), -Vec3::Z), center, 2.0).is_none());
    }

    #[test]
    fn nearest_wins() {
        let positions = [[0.0, 0.0, -20.0], [0.0, 0.5, -8.0]];
        let elements = [Element::C, Element::O];
        let hit = cast_spheres(
            &ray(Vec3::ZERO, -Vec3::Z),
            &positions,
            &elements,
            Representation::SpaceFilling,
            0..2,
        );
        assert_eq!(hit.map(|h| h.0), Some(1));
        let skipped = cast_spheres(
            &ray(Vec3::ZERO, -Vec3::Z),
            &positions,
            &elements,
            Representation::SpaceFilling,
            0..1,
        );
        assert_eq!(skipped.map(|h| h.0), Some(0));
    }
}
