//! three's OrbitControls as the floor configures them: orbit (left drag), pan (right or
//! middle drag), dolly (wheel), damping 0.08, polar angle 0.2–1.3 rad, distance 6–300.

use std::f32::consts::PI;

use glam::Vec3;

use super::scene::Camera;

/// three's `EPS`: keeps the polar angle off the poles.
const POLE_EPS: f32 = 1e-6;
/// The camera's vertical field of view, degrees.
pub const FOV_DEG: f32 = 40.0;
/// A pending delta at or below this is settled; `update` reports `false` once all are.
const SETTLED: f32 = 1e-4;
/// three's zoom base: one wheel step scales the distance by 0.95.
const ZOOM_BASE: f32 = 0.95;

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
    // contract: OC — pending (damped) motion, applied and decayed by `update`.
    /// three's sphericalDelta: rotation still to be applied.
    delta_theta: f32,
    delta_phi: f32,
    /// three's panOffset: world-space target motion still to be applied.
    pan: Vec3,
    /// three's scale: zoom factor still to be applied to the radius (1 = none).
    scale: f32,
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
            delta_theta: 0.0,
            delta_phi: 0.0,
            pan: Vec3::ZERO,
            scale: 1.0,
        }
    }
}

impl Orbit {
    /// Left drag by (dx, dy) pixels in a view `height` pixels tall (three: 2π·dx/height).
    pub fn rotate(&mut self, dx: f32, dy: f32, height: f32) {
        self.delta_theta -= 2.0 * PI * dx / height;
        self.delta_phi -= 2.0 * PI * dy / height;
    }

    /// Right/middle drag by (dx, dy) pixels: moves the target in the view plane.
    pub fn pan(&mut self, dx: f32, dy: f32, height: f32, fov_deg: f32) {
        let target_distance = self.radius * (fov_deg.to_radians() / 2.0).tan();
        let (right, up) = self.view_axes();
        let move_left = 2.0 * dx * target_distance / height;
        let move_up = 2.0 * dy * target_distance / height;
        self.pan += -right * move_left + up * move_up;
    }

    /// Wheel: positive `steps` zoom out (three: scale 0.95^steps).
    pub fn dolly(&mut self, steps: f32) {
        self.scale *= ZOOM_BASE.powf(-steps);
    }

    /// Applies the damped deltas for one frame; true while still moving.
    pub fn update(&mut self) -> bool {
        self.theta += self.delta_theta * self.damping;
        self.phi += self.delta_phi * self.damping;
        self.phi = self.phi.max(self.min_polar).min(self.max_polar).max(POLE_EPS).min(PI - POLE_EPS);
        self.radius = (self.radius * self.scale).max(self.min_distance).min(self.max_distance);
        self.target += self.pan * self.damping;

        let keep = 1.0 - self.damping;
        self.delta_theta *= keep;
        self.delta_phi *= keep;
        self.pan *= keep;
        self.scale = 1.0;

        self.delta_theta.abs() > SETTLED || self.delta_phi.abs() > SETTLED || self.pan.length() > SETTLED
    }

    /// Places the eye at `eye` looking at `target` (resets pending motion).
    pub fn look(&mut self, eye: Vec3, target: Vec3) {
        let offset = eye - target;
        let radius = offset.length().max(POLE_EPS);
        self.target = target;
        self.radius = radius;
        self.phi = (offset.y / radius).clamp(-1.0, 1.0).acos();
        self.theta = offset.x.atan2(offset.z);
        self.delta_theta = 0.0;
        self.delta_phi = 0.0;
        self.pan = Vec3::ZERO;
        self.scale = 1.0;
    }

    /// The camera now (fov `FOV_DEG`, near 0.5, far 1000).
    pub fn camera(&self, aspect: f32) -> Camera {
        let eye = self.target + self.radius * self.eye_direction();
        Camera { eye, target: self.target, fov: FOV_DEG, near: 0.5, far: 1000.0, aspect }
    }

    /// Unit vector from the target to the eye, from the spherical coordinates.
    fn eye_direction(&self) -> Vec3 {
        Vec3::new(self.phi.sin() * self.theta.sin(), self.phi.cos(), self.phi.sin() * self.theta.cos())
    }

    /// The camera's right and up axes for a look at the target with world up +Y (three's lookAt).
    fn view_axes(&self) -> (Vec3, Vec3) {
        let z = self.eye_direction();
        let right = Vec3::Y.cross(z).normalize_or_zero();
        let up = z.cross(right);
        (right, up)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < 1e-3
    }

    #[test]
    fn look_then_camera_returns_the_same_eye() {
        let mut orbit = Orbit::default();
        let eye = Vec3::new(10.0, 20.0, 30.0);
        let target = Vec3::new(1.0, 2.0, 3.0);
        orbit.look(eye, target);
        let camera = orbit.camera(1.5);
        assert!(close(camera.eye, eye), "eye {:?}", camera.eye);
        assert!(close(camera.target, target), "target {:?}", camera.target);
    }

    #[test]
    fn rotate_converges_within_polar_limits() {
        let mut orbit = Orbit::default();
        // Pushes phi toward the max polar limit and theta a little.
        orbit.rotate(40.0, -300.0, 600.0);
        let mut steps = 0;
        while orbit.update() {
            steps += 1;
            assert!(steps < 2000, "damping never settled");
            assert!(orbit.phi >= orbit.min_polar && orbit.phi <= orbit.max_polar, "phi {}", orbit.phi);
        }
        assert!(orbit.phi >= orbit.min_polar && orbit.phi <= orbit.max_polar);
        assert!((orbit.phi - orbit.max_polar).abs() < 1e-4, "phi {}", orbit.phi);
    }

    #[test]
    fn dolly_out_grows_radius_up_to_max_distance() {
        let mut orbit = Orbit::default();
        let start = orbit.radius;
        orbit.dolly(1.0);
        orbit.update();
        assert!(orbit.radius > start);
        for _ in 0..200 {
            orbit.dolly(1.0);
            orbit.update();
            assert!(orbit.radius <= orbit.max_distance, "radius {}", orbit.radius);
        }
        assert!((orbit.radius - orbit.max_distance).abs() < 1e-3, "radius {}", orbit.radius);
    }
}
