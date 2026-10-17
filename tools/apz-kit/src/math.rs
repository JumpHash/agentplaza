//! Platform-independent math. The kit's files are compared byte for byte (`files::tests`), and each platform's own libm
//! rounds `sin`, `cos` and `powf` its own way (macOS differs from glibc), so every transcendental call in the kit goes
//! through the `libm` crate, which computes the same bits everywhere. Rotations are built here from those values too,
//! rather than with glam's, which call the platform's.

use glam::{Mat4, Quat};

pub fn sin(x: f32) -> f32 {
    libm::sinf(x)
}

pub fn cos(x: f32) -> f32 {
    libm::cosf(x)
}

/// The sine and the cosine of `x`.
pub fn sin_cos(x: f32) -> (f32, f32) {
    (sin(x), cos(x))
}

pub fn powf(x: f32, y: f32) -> f32 {
    libm::powf(x, y)
}

/// A rotation of `angle` radians about x.
pub fn rot_x(angle: f32) -> Quat {
    let (s, c) = sin_cos(angle * 0.5);
    Quat::from_xyzw(s, 0.0, 0.0, c)
}

/// A rotation of `angle` radians about y.
pub fn rot_y(angle: f32) -> Quat {
    let (s, c) = sin_cos(angle * 0.5);
    Quat::from_xyzw(0.0, s, 0.0, c)
}

/// A rotation of `angle` radians about z.
pub fn rot_z(angle: f32) -> Quat {
    let (s, c) = sin_cos(angle * 0.5);
    Quat::from_xyzw(0.0, 0.0, s, c)
}

/// [`rot_x`] as a matrix.
pub fn mat_rot_x(angle: f32) -> Mat4 {
    Mat4::from_quat(rot_x(angle))
}

/// [`rot_z`] as a matrix.
pub fn mat_rot_z(angle: f32) -> Mat4 {
    Mat4::from_quat(rot_z(angle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    #[test]
    fn the_rotations_turn_as_glams_do() {
        for a in [0.3f32, -1.83, std::f32::consts::FRAC_PI_2] {
            let v = Vec3::new(0.2, 0.7, -0.4);
            assert!((rot_x(a) * v).abs_diff_eq(Quat::from_rotation_x(a) * v, 1e-6));
            assert!((rot_y(a) * v).abs_diff_eq(Quat::from_rotation_y(a) * v, 1e-6));
            assert!((rot_z(a) * v).abs_diff_eq(Quat::from_rotation_z(a) * v, 1e-6));
            assert!(mat_rot_x(a).abs_diff_eq(Mat4::from_rotation_x(a), 1e-6));
            assert!(mat_rot_z(a).abs_diff_eq(Mat4::from_rotation_z(a), 1e-6));
        }
    }
}
