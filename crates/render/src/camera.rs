//! The orbit camera: orbit, pan, zoom, fit to bounds, re-aim on a point, and
//! the ray ⇄ pixel maps picking and the tests use.
//!
//! Y-up, right-handed, perspective with **reversed Z and an infinite far
//! plane** (depth 1 at the near plane, 0 at infinity), which is what the
//! impostor shaders write. Pixels are physical, origin at the top-left.

use glam::{Mat4, Vec2, Vec3, Vec4Swizzles as _};

/// A ray in world space. `dir` is unit length.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ray {
    pub origin: Vec3,
    pub dir: Vec3,
}

impl Ray {
    pub fn at(&self, t: f32) -> Vec3 {
        self.origin + self.dir * t
    }
}

/// A sphere that contains a set of points, e.g. the atoms of a pose.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoundingSphere {
    pub center: Vec3,
    pub radius: f32,
}

impl BoundingSphere {
    /// Centroid of `points` and the largest distance from it, plus `pad`
    /// (e.g. the largest atom radius). `None` for no points.
    pub fn from_points(points: &[[f32; 3]], pad: f32) -> Option<Self> {
        if points.is_empty() {
            return None;
        }
        // Sum in f64: a million atoms of f32 sums lose the centroid.
        let sum = points.iter().fold([0f64; 3], |acc, p| {
            [
                acc[0] + f64::from(p[0]),
                acc[1] + f64::from(p[1]),
                acc[2] + f64::from(p[2]),
            ]
        });
        let n = points.len() as f64;
        let center = Vec3::new(
            (sum[0] / n) as f32,
            (sum[1] / n) as f32,
            (sum[2] / n) as f32,
        );
        let radius = points
            .iter()
            .map(|p| (Vec3::from(*p) - center).length_squared())
            .fold(0.0, f32::max)
            .sqrt();
        Some(Self {
            center,
            radius: radius + pad,
        })
    }
}

/// See the [module docs](self).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrbitCamera {
    pub target: Vec3,
    /// Around +Y, radians.
    pub yaw: f32,
    /// Above the XZ plane, radians, kept inside ±89.9°.
    pub pitch: f32,
    pub distance: f32,
    /// Vertical field of view, radians.
    pub fov_y: f32,
}

const PITCH_LIMIT: f32 = 89.9 * std::f32::consts::PI / 180.0;
/// Radians per physical pixel of drag.
const ORBIT_SPEED: f32 = 0.008;

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            target: Vec3::ZERO,
            yaw: 0.6,
            pitch: 0.35,
            distance: 50.0,
            fov_y: 40f32.to_radians(),
        }
    }
}

impl OrbitCamera {
    /// A camera that fits `bounds` in a viewport of `aspect` (width / height),
    /// from a three-quarter angle.
    pub fn framing(bounds: BoundingSphere, aspect: f32) -> Self {
        let mut camera = Self::default();
        camera.fit(bounds, aspect);
        camera
    }

    /// Keeps the orientation, centres `bounds` and backs off until the whole
    /// sphere is inside both the vertical and the horizontal field of view.
    pub fn fit(&mut self, bounds: BoundingSphere, aspect: f32) {
        let half_y = self.fov_y * 0.5;
        let half_x = (half_y.tan() * aspect.max(1e-3)).atan();
        let half = half_y.min(half_x);
        self.target = bounds.center;
        self.distance = bounds.radius.max(1e-3) / half.sin() * 1.02;
    }

    pub fn eye(&self) -> Vec3 {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
        self.target + Vec3::new(cos_pitch * sin_yaw, sin_pitch, cos_pitch * cos_yaw) * self.distance
    }

    pub fn forward(&self) -> Vec3 {
        (self.target - self.eye()).normalize()
    }

    pub fn right(&self) -> Vec3 {
        self.forward().cross(Vec3::Y).normalize()
    }

    pub fn up(&self) -> Vec3 {
        self.right().cross(self.forward())
    }

    pub fn near(&self) -> f32 {
        (self.distance * 0.01).max(0.01)
    }

    pub fn view(&self) -> Mat4 {
        glam::camera::rh::view::look_at_mat4(self.eye(), self.target, Vec3::Y)
    }

    /// WebGPU convention: depth in [0, 1], reversed, infinite far plane.
    pub fn projection(&self, aspect: f32) -> Mat4 {
        glam::camera::rh::proj::directx::perspective_infinite_reverse(
            self.fov_y,
            aspect,
            self.near(),
        )
    }

    pub fn view_projection(&self, width: f32, height: f32) -> Mat4 {
        self.projection(width / height.max(1.0)) * self.view()
    }

    /// The ray from the eye through a pixel. Built from the camera basis,
    /// independently of the matrices, so the tests can check one against the
    /// other.
    pub fn ray_through(&self, x: f32, y: f32, width: f32, height: f32) -> Ray {
        let (width, height) = (width.max(1.0), height.max(1.0));
        let ndc_x = 2.0 * x / width - 1.0;
        let ndc_y = 1.0 - 2.0 * y / height;
        let half_height = (self.fov_y * 0.5).tan();
        let half_width = half_height * width / height;
        let dir = self.forward()
            + self.right() * (ndc_x * half_width)
            + self.up() * (ndc_y * half_height);
        Ray {
            origin: self.eye(),
            dir: dir.normalize(),
        }
    }

    /// Where a world point lands, in pixels; `None` behind the eye.
    pub fn project(&self, point: Vec3, width: f32, height: f32) -> Option<Vec2> {
        let clip = self.view_projection(width, height) * point.extend(1.0);
        if clip.w <= 1e-6 {
            return None;
        }
        let ndc = clip.xy() / clip.w;
        Some(Vec2::new(
            (ndc.x + 1.0) * 0.5 * width,
            (1.0 - ndc.y) * 0.5 * height,
        ))
    }

    /// Turns the camera to look at `point` from where it is: the eye stays
    /// put, so the scene does not jump, and orbiting then pivots on `point`.
    pub fn re_aim(&mut self, point: Vec3) {
        let offset = self.eye() - point;
        let distance = offset.length();
        if distance < 1e-4 {
            self.target = point;
            return;
        }
        let dir = offset / distance;
        self.target = point;
        self.distance = distance;
        self.pitch = dir
            .y
            .clamp(-1.0, 1.0)
            .asin()
            .clamp(-PITCH_LIMIT, PITCH_LIMIT);
        self.yaw = dir.x.atan2(dir.z);
    }

    /// Drag in physical pixels.
    pub fn orbit(&mut self, dx: f32, dy: f32) {
        self.yaw -= dx * ORBIT_SPEED;
        self.pitch = (self.pitch + dy * ORBIT_SPEED).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    }

    /// Drag in physical pixels: the point under the pointer at the target's
    /// depth follows the pointer.
    pub fn pan(&mut self, dx: f32, dy: f32, viewport_height: f32) {
        let world_per_pixel =
            2.0 * self.distance * (self.fov_y * 0.5).tan() / viewport_height.max(1.0);
        self.target += (self.up() * dy - self.right() * dx) * world_per_pixel;
    }

    /// `lines` > 0 zooms in.
    pub fn zoom(&mut self, lines: f32) {
        self.distance = (self.distance * 0.88f32.powf(lines)).max(0.5);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: f32 = 1280.0;
    const H: f32 = 800.0;

    fn camera() -> OrbitCamera {
        OrbitCamera::framing(
            BoundingSphere {
                center: Vec3::new(1.0, -2.0, 3.0),
                radius: 20.0,
            },
            W / H,
        )
    }

    #[test]
    fn ray_through_projected_point_passes_through_it() {
        let camera = camera();
        for point in [
            Vec3::new(1.0, -2.0, 3.0),
            Vec3::new(10.0, 4.0, -6.0),
            Vec3::new(-12.0, 9.0, 8.0),
        ] {
            let pixel = camera.project(point, W, H).expect("in front of the eye");
            let ray = camera.ray_through(pixel.x, pixel.y, W, H);
            let t = (point - ray.origin).dot(ray.dir);
            let miss = (ray.at(t) - point).length();
            assert!(miss < 1e-3 * t, "ray misses {point} by {miss}");
        }
    }

    #[test]
    fn centre_ray_is_the_view_direction() {
        let camera = camera();
        let ray = camera.ray_through(W / 2.0, H / 2.0, W, H);
        assert!((ray.dir - camera.forward()).length() < 1e-5);
        assert!((ray.origin - camera.eye()).length() < 1e-4);
    }

    #[test]
    fn re_aim_centres_the_point_and_keeps_the_eye() {
        let mut camera = camera();
        let eye = camera.eye();
        let point = Vec3::new(9.0, 5.0, -7.0);
        camera.re_aim(point);
        assert!((camera.eye() - eye).length() < 1e-3, "the eye moved");
        let pixel = camera.project(point, W, H).expect("in front");
        assert!(
            (pixel - Vec2::new(W / 2.0, H / 2.0)).length() < 0.5,
            "{pixel}"
        );
    }

    #[test]
    fn reversed_z_puts_near_at_one_and_far_towards_zero() {
        let camera = camera();
        let view_proj = camera.view_projection(W, H);
        let depth = |t: f32| {
            let clip = view_proj * camera.ray_through(W / 2.0, H / 2.0, W, H).at(t).extend(1.0);
            clip.z / clip.w
        };
        assert!((depth(camera.near()) - 1.0).abs() < 1e-4);
        assert!(depth(10.0) > depth(100.0));
        assert!(depth(1e6) < 1e-4);
    }

    /// Every point of the fitted sphere projects inside the viewport, for wide
    /// and for tall viewports, and the sphere is not tiny either.
    #[test]
    fn fit_keeps_the_whole_sphere_in_view() {
        let bounds = BoundingSphere {
            center: Vec3::new(5.0, 1.0, -3.0),
            radius: 12.0,
        };
        for (w, h) in [(1600.0, 900.0), (500.0, 1200.0), (800.0, 800.0)] {
            let mut camera = OrbitCamera {
                yaw: 2.1,
                ..OrbitCamera::default()
            };
            camera.fit(bounds, w / h);
            let (mut min, mut max) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
            for i in 0..2000 {
                // Points on the sphere (golden-angle spiral).
                let z = 1.0 - 2.0 * (i as f32 + 0.5) / 2000.0;
                let r = (1.0 - z * z).sqrt();
                let a = i as f32 * 2.399_963;
                let p = bounds.center + Vec3::new(r * a.cos(), r * a.sin(), z) * bounds.radius;
                let px = camera.project(p, w, h).expect("in front");
                min = min.min(px);
                max = max.max(px);
            }
            assert!(min.x >= 0.0 && min.y >= 0.0, "{w}x{h}: {min}");
            assert!(max.x <= w && max.y <= h, "{w}x{h}: {max}");
            let span = (max - min) / Vec2::new(w, h);
            assert!(span.max_element() > 0.85, "{w}x{h}: fills only {span}");
            assert_eq!(camera.yaw, 2.1, "fit keeps the orientation");
        }
    }

    #[test]
    fn pan_moves_the_target_with_the_pointer() {
        let mut camera = camera();
        let before = camera.project(camera.target, W, H).unwrap();
        let anchor = camera.target;
        camera.pan(40.0, -25.0, H);
        let after = camera.project(anchor, W, H).unwrap();
        // The old target moved by the drag on screen.
        assert!(
            ((after - before) - Vec2::new(40.0, -25.0)).length() < 0.5,
            "{before} -> {after}"
        );
    }

    #[test]
    fn bounding_sphere_contains_its_points() {
        let points = [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 4.0, -2.0]];
        let sphere = BoundingSphere::from_points(&points, 1.5).unwrap();
        for p in points {
            assert!((Vec3::from(p) - sphere.center).length() + 1.5 <= sphere.radius + 1e-4);
        }
        assert!(BoundingSphere::from_points(&[], 1.0).is_none());
    }
}
