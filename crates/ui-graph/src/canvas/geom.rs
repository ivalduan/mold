//! Plain `f32` geometry for the canvas: points, rectangles and the pan/zoom camera.
//! "World" = canvas coordinates where node positions live; "screen" = pixels
//! relative to the canvas's top-left corner.

use std::ops::{Add, Mul, Sub};

/// A point or a vector.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub const ZERO: Self = Self { x: 0.0, y: 0.0 };

    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub fn distance(self, other: Self) -> f32 {
        (self - other).length()
    }

    pub fn length(self) -> f32 {
        self.x.hypot(self.y)
    }
}

impl Add for Vec2 {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        Self::new(self.x + o.x, self.y + o.y)
    }
}

impl Sub for Vec2 {
    type Output = Self;
    fn sub(self, o: Self) -> Self {
        Self::new(self.x - o.x, self.y - o.y)
    }
}

impl Mul<f32> for Vec2 {
    type Output = Self;
    fn mul(self, k: f32) -> Self {
        Self::new(self.x * k, self.y * k)
    }
}

/// An axis-aligned rectangle: top-left corner and size.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub origin: Vec2,
    pub size: Vec2,
}

impl Rect {
    pub const fn new(origin: Vec2, size: Vec2) -> Self {
        Self { origin, size }
    }

    pub fn max(&self) -> Vec2 {
        self.origin + self.size
    }

    pub fn center(&self) -> Vec2 {
        self.origin + self.size * 0.5
    }

    /// Inclusive of the top/left edge, exclusive of the bottom/right one.
    pub fn contains(&self, p: Vec2) -> bool {
        let max = self.max();
        p.x >= self.origin.x && p.y >= self.origin.y && p.x < max.x && p.y < max.y
    }
}

/// Pan and zoom: `screen = world * zoom + pan`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    /// Screen position of the world origin.
    pub pan: Vec2,
    pub zoom: f32,
}

impl Camera {
    pub const MIN_ZOOM: f32 = 0.25;
    pub const MAX_ZOOM: f32 = 3.0;

    pub fn to_screen(&self, world: Vec2) -> Vec2 {
        world * self.zoom + self.pan
    }

    pub fn to_world(&self, screen: Vec2) -> Vec2 {
        (screen - self.pan) * (1.0 / self.zoom)
    }

    pub fn rect_to_screen(&self, world: Rect) -> Rect {
        Rect::new(self.to_screen(world.origin), world.size * self.zoom)
    }

    /// Multiply the zoom by `factor` (clamped), keeping the world point under
    /// `anchor` (screen) where it is.
    pub fn zoom_at(&mut self, anchor: Vec2, factor: f32) {
        let world = self.to_world(anchor);
        self.zoom = (self.zoom * factor).clamp(Self::MIN_ZOOM, Self::MAX_ZOOM);
        self.pan = anchor - world * self.zoom;
    }
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            pan: Vec2::ZERO,
            zoom: 1.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_round_trips_and_zooms_about_the_anchor() {
        let mut cam = Camera {
            pan: Vec2::new(30.0, -10.0),
            zoom: 1.5,
        };
        let w = Vec2::new(12.0, 40.0);
        let s = cam.to_screen(w);
        assert!(cam.to_world(s).distance(w) < 1e-4);

        let anchor = Vec2::new(200.0, 120.0);
        let before = cam.to_world(anchor);
        cam.zoom_at(anchor, 2.0);
        assert_eq!(cam.zoom, 3.0);
        assert!(cam.to_world(anchor).distance(before) < 1e-3);
        cam.zoom_at(anchor, 10.0);
        assert_eq!(cam.zoom, Camera::MAX_ZOOM);
    }

    #[test]
    fn rect_contains_is_half_open() {
        let r = Rect::new(Vec2::new(0.0, 0.0), Vec2::new(10.0, 5.0));
        assert!(r.contains(Vec2::new(0.0, 0.0)));
        assert!(r.contains(Vec2::new(9.9, 4.9)));
        assert!(!r.contains(Vec2::new(10.0, 2.0)));
    }
}
