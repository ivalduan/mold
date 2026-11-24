//! SPIKE ONLY (WS-00). CPU ray picking against spheres. The impostors are
//! exact spheres, so this agrees with what is drawn without a GPU readback.
//! `mold-render` (WS-08) owns real picking (GPU readback, sticks, ribbons).

use glam::Vec3;

use super::camera::Ray;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sphere {
    pub center: Vec3,
    pub radius: f32,
}

/// Distance along `ray` to the first surface point of `sphere` in front of
/// the origin, or `None` if the ray misses.
pub fn ray_sphere(ray: &Ray, sphere: &Sphere) -> Option<f32> {
    let oc = ray.origin - sphere.center;
    let b = oc.dot(ray.dir);
    let c = oc.length_squared() - sphere.radius * sphere.radius;
    let discriminant = b * b - c;
    if discriminant < 0.0 {
        return None;
    }
    let root = discriminant.sqrt();
    let near = -b - root;
    if near >= 0.0 {
        return Some(near);
    }
    // Inside the sphere: the exit point is the visible one.
    let far = -b + root;
    (far >= 0.0).then_some(far)
}

/// The nearest sphere the ray hits: its index and distance.
pub fn pick(ray: &Ray, spheres: impl IntoIterator<Item = Sphere>) -> Option<(usize, f32)> {
    spheres
        .into_iter()
        .enumerate()
        .filter_map(|(index, sphere)| ray_sphere(ray, &sphere).map(|t| (index, t)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
}

#[cfg(test)]
mod tests {
    use super::super::camera::OrbitCamera;
    use super::super::scene::spike_scene;
    use super::*;

    const W: f32 = 1600.0;
    const H: f32 = 1000.0;

    fn ray(origin: Vec3, dir: Vec3) -> Ray {
        Ray {
            origin,
            dir: dir.normalize(),
        }
    }

    #[test]
    fn hits_misses_and_inside() {
        let sphere = Sphere {
            center: Vec3::new(0.0, 0.0, -10.0),
            radius: 2.0,
        };
        let t = ray_sphere(&ray(Vec3::ZERO, -Vec3::Z), &sphere).unwrap();
        assert!((t - 8.0).abs() < 1e-5);
        assert!(
            ray_sphere(&ray(Vec3::ZERO, Vec3::Z), &sphere).is_none(),
            "behind"
        );
        assert!(ray_sphere(&ray(Vec3::new(3.0, 0.0, 0.0), -Vec3::Z), &sphere).is_none());
        let inside = ray_sphere(&ray(sphere.center, Vec3::X), &sphere).unwrap();
        assert!((inside - 2.0).abs() < 1e-5);
    }

    #[test]
    fn nearest_of_two_on_the_ray_wins() {
        let far = Sphere {
            center: Vec3::new(0.0, 0.0, -20.0),
            radius: 3.0,
        };
        let near = Sphere {
            center: Vec3::new(0.0, 0.5, -8.0),
            radius: 1.0,
        };
        let hit = pick(&ray(Vec3::ZERO, -Vec3::Z), [far, near]).unwrap();
        assert_eq!(hit.0, 1);
    }

    #[test]
    fn empty_space_picks_nothing() {
        let scene = spike_scene();
        let camera = OrbitCamera::framing(scene.center, scene.radius);
        // A corner of the viewport looks past the framed bounding sphere.
        let ray = camera.ray_through(1.0, 1.0, W, H);
        assert_eq!(pick(&ray, scene.spheres()), None);
    }

    /// The spike's acceptance check, headless: clicking where an atom is drawn
    /// picks that atom unless a nearer one covers the pixel, and re-aiming then
    /// puts it at the centre of the viewport, where picking finds it again.
    #[test]
    fn click_on_a_visible_atom_picks_it_and_re_aims_on_it() {
        let scene = spike_scene();
        let spheres: Vec<Sphere> = scene.spheres().collect();
        let camera = OrbitCamera::framing(scene.center, scene.radius);
        let mut checked = 0;
        for (index, sphere) in spheres.iter().enumerate() {
            let Some(pixel) = camera.project(sphere.center, W, H) else {
                continue;
            };
            let ray = camera.ray_through(pixel.x, pixel.y, W, H);
            let (picked, t) = pick(&ray, spheres.iter().copied()).expect("an atom is there");
            if picked != index {
                // Covered: what was picked must be in front of this atom's surface.
                let own = ray_sphere(&ray, sphere).expect("the ray passes its centre");
                assert!(t <= own, "picked {picked} behind {index}");
                continue;
            }
            checked += 1;
            let mut aimed = camera;
            aimed.re_aim(sphere.center);
            let centre = aimed.project(sphere.center, W, H).unwrap();
            assert!((centre.x - W / 2.0).abs() < 0.5 && (centre.y - H / 2.0).abs() < 0.5);
            let again = aimed.ray_through(W / 2.0, H / 2.0, W, H);
            let (repicked, _) = pick(&again, spheres.iter().copied()).unwrap();
            // Re-aiming keeps the eye, so the same atom stays in front.
            assert_eq!(repicked, index);
        }
        assert!(checked > 50, "only {checked} atoms were visible");
    }

    #[test]
    fn picking_500_atoms_is_far_inside_the_budget() {
        let scene = spike_scene();
        let spheres: Vec<Sphere> = scene.spheres().collect();
        let camera = OrbitCamera::framing(scene.center, scene.radius);
        let started = std::time::Instant::now();
        for i in 0..100 {
            let ray = camera.ray_through(W * 0.3 + i as f32, H * 0.5, W, H);
            std::hint::black_box(pick(&ray, spheres.iter().copied()));
        }
        // 100 picks; the budget for one click to re-aim is 100 ms.
        assert!(
            started.elapsed().as_millis() < 100,
            "{:?}",
            started.elapsed()
        );
    }
}
