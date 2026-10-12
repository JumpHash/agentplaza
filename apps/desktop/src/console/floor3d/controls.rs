//! three's OrbitControls as the floor configures them: orbit (left drag), pan (right or
//! middle drag), dolly (wheel), damping 0.08, polar angle 0.2–1.3 rad, distance 6–300.

use glam::Vec3;

use super::scene::Camera;

#[derive(Debug, Clone)]
pub struct Orbit {
    pub target: Vec3,
    /// Spherical coordinates of the eye about the target: radius, polar angle from +Y, azimuth.
    pub radius: f32,
    pub phi: f32,
    pub theta: f32,
    pub min_polar: f32,
    pub max_polar: f32,
    pub min_distance: f32,
    pub max_distance: f32,
    pub damping: f32,
    // contract: OC — the pending (damped) rotation, pan and zoom deltas.
}

impl Default for Orbit {
    fn default() -> Self {
        Orbit {
            target: Vec3::ZERO,
            radius: 60.0,
            phi: 0.95,
            theta: std::f32::consts::FRAC_PI_4,
            min_polar: 0.2,
            max_polar: 1.3,
            min_distance: 6.0,
            max_distance: 300.0,
            damping: 0.08,
        }
    }
}

impl Orbit {
    /// Left drag by (dx, dy) pixels in a view `height` pixels tall (three: 2π·dx/height).
    pub fn rotate(&mut self, dx: f32, dy: f32, height: f32) {
        let _ = (dx, dy, height); // contract: OC
    }
    /// Right/middle drag by (dx, dy) pixels: moves the target in the view plane.
    pub fn pan(&mut self, dx: f32, dy: f32, height: f32, fov_deg: f32) {
        let _ = (dx, dy, height, fov_deg); // contract: OC
    }
    /// Wheel: positive `steps` zoom out (three: scale 0.95^steps).
    pub fn dolly(&mut self, steps: f32) {
        let _ = steps; // contract: OC
    }
    /// Applies the damped deltas for one frame; true while still moving.
    pub fn update(&mut self) -> bool {
        false // contract: OC
    }
    /// Places the eye at `eye` looking at `target` (resets pending motion).
    pub fn look(&mut self, eye: Vec3, target: Vec3) {
        let _ = (eye, target); // contract: OC
    }
    /// The camera now (fov 40°, near 0.5, far 1000).
    pub fn camera(&self, aspect: f32) -> Camera {
        let eye = self.target
            + Vec3::new(self.radius * self.phi.sin() * self.theta.sin(), self.radius * self.phi.cos(), self.radius * self.phi.sin() * self.theta.cos());
        Camera { eye, target: self.target, fov: 40.0, near: 0.5, far: 1000.0, aspect }
    }
}
