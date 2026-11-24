//! SPIKE ONLY (WS-00). An orbit camera with the two operations the spike has
//! to prove: a ray through a screen point, and re-aiming at a picked point.
//! The real camera belongs to `mold-render` (WS-08).

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

/// A Y-up orbit camera looking at `target` from `distance` away. Perspective,
/// reversed-Z with an infinite far plane (depth 1 at the near plane, 0 at
/// infinity), which is what the impostor shader writes.
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

impl OrbitCamera {
    /// A camera that fits a bounding sphere in view, looking from a pleasant
    /// three-quarter angle.
    pub fn framing(center: Vec3, radius: f32) -> Self {
        let fov_y = 40f32.to_radians();
        Self {
            target: center,
            yaw: 0.6,
            pitch: 0.35,
            distance: radius.max(1e-3) / (fov_y * 0.5).sin() * 1.05,
            fov_y,
        }
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

    pub fn projection(&self, aspect: f32) -> Mat4 {
        // WebGPU convention: depth in [0, 1], Y up.
        glam::camera::rh::proj::directx::perspective_infinite_reverse(
            self.fov_y,
            aspect,
            self.near(),
        )
    }

    pub fn view_projection(&self, width: f32, height: f32) -> Mat4 {
        self.projection(width / height.max(1.0)) * self.view()
    }

    /// The ray from the eye through a point of the viewport, in physical
    /// pixels with the origin at the top-left corner. Built from the camera
    /// basis, independently of the matrices, so the tests can check one
    /// against the other.
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

    /// Where a world point lands in the viewport, in physical pixels; `None`
    /// behind the eye.
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

    /// Turns the camera to look at `point` from where it is: the eye stays put,
    /// so the scene does not jump, and orbiting then pivots around the picked
    /// atom.
    pub fn re_aim(&mut self, point: Vec3) {
        let eye = self.eye();
        let offset = eye - point;
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
        self.yaw -= dx * 0.008;
        self.pitch = (self.pitch + dy * 0.008).clamp(-PITCH_LIMIT, PITCH_LIMIT);
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
        OrbitCamera::framing(Vec3::new(1.0, -2.0, 3.0), 20.0)
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
}
