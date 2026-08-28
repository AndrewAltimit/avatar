//! Minimal `f32` vector / quaternion / matrix math for the scene view — just enough to mirror
//! the `glam` operations `render_scene.rs` and `avatar_pose` use, without pulling `glam` into
//! the wasm bundle (the workspace confines `glam` to the runtime-rig + render crates).
//!
//! Conventions match `glam`: column vectors, `Mat4` stored as four columns, quaternions as
//! `(x, y, z, w)`, and `Quat::from_euler_xyz` = `Rx · Ry · Rz` (glam's `EulerRot::XYZ`).

use avatar_fbx::LocalTransform;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const ZERO: Vec3 = Vec3::new(0.0, 0.0, 0.0);
    pub const X: Vec3 = Vec3::new(1.0, 0.0, 0.0);
    pub const Y: Vec3 = Vec3::new(0.0, 1.0, 0.0);
    pub const Z: Vec3 = Vec3::new(0.0, 0.0, 1.0);

    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Vec3 { x, y, z }
    }
    pub fn dot(self, o: Vec3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    pub fn cross(self, o: Vec3) -> Vec3 {
        Vec3::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }
    pub fn length(self) -> f32 {
        self.dot(self).sqrt()
    }
    pub fn normalize(self) -> Vec3 {
        self / self.length()
    }
    pub fn normalize_or_zero(self) -> Vec3 {
        let l = self.length();
        if l > 1e-12 && l.is_finite() {
            self / l
        } else {
            Vec3::ZERO
        }
    }
    pub fn min(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x.min(o.x), self.y.min(o.y), self.z.min(o.z))
    }
    pub fn max(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x.max(o.x), self.y.max(o.y), self.z.max(o.z))
    }
    pub fn abs(self) -> Vec3 {
        Vec3::new(self.x.abs(), self.y.abs(), self.z.abs())
    }
}

impl From<[f32; 3]> for Vec3 {
    fn from(a: [f32; 3]) -> Self {
        Vec3::new(a[0], a[1], a[2])
    }
}
impl From<Vec3> for [f32; 3] {
    fn from(v: Vec3) -> Self {
        [v.x, v.y, v.z]
    }
}
impl std::ops::Add for Vec3 {
    type Output = Vec3;
    fn add(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}
impl std::ops::Sub for Vec3 {
    type Output = Vec3;
    fn sub(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}
impl std::ops::Mul<f32> for Vec3 {
    type Output = Vec3;
    fn mul(self, s: f32) -> Vec3 {
        Vec3::new(self.x * s, self.y * s, self.z * s)
    }
}
impl std::ops::Div<f32> for Vec3 {
    type Output = Vec3;
    fn div(self, s: f32) -> Vec3 {
        Vec3::new(self.x / s, self.y / s, self.z / s)
    }
}

/// A unit quaternion `(x, y, z, w)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quat {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Quat {
    pub const IDENTITY: Quat = Quat {
        x: 0.0,
        y: 0.0,
        z: 0.0,
        w: 1.0,
    };

    pub fn from_axis_angle(axis: Vec3, angle: f32) -> Quat {
        let (s, c) = (angle * 0.5).sin_cos();
        let a = axis.normalize();
        Quat {
            x: a.x * s,
            y: a.y * s,
            z: a.z * s,
            w: c,
        }
    }

    /// Shortest-arc rotation taking unit vector `from` onto unit vector `to` (glam's
    /// `from_rotation_arc`, including the 180° antiparallel case).
    pub fn from_rotation_arc(from: Vec3, to: Vec3) -> Quat {
        const ONE_MINUS_EPS: f32 = 1.0 - 2.0 * f32::EPSILON;
        let d = from.dot(to);
        if d > ONE_MINUS_EPS {
            Quat::IDENTITY
        } else if d < -ONE_MINUS_EPS {
            // Any axis perpendicular to `from`; pick the one furthest from its dominant component.
            let a = from.abs();
            let ortho = if a.x <= a.y && a.x <= a.z {
                Vec3::X
            } else if a.y <= a.z {
                Vec3::Y
            } else {
                Vec3::Z
            };
            Quat::from_axis_angle(from.cross(ortho).normalize(), std::f32::consts::PI)
        } else {
            let c = from.cross(to);
            Quat {
                x: c.x,
                y: c.y,
                z: c.z,
                w: 1.0 + d,
            }
            .normalize()
        }
    }

    /// `Rx(x) · Ry(y) · Rz(z)` — FBX's default `Lcl Rotation` order (glam `EulerRot::XYZ`).
    /// Angles in radians.
    pub fn from_euler_xyz(x: f32, y: f32, z: f32) -> Quat {
        Quat::from_axis_angle(Vec3::X, x)
            * Quat::from_axis_angle(Vec3::Y, y)
            * Quat::from_axis_angle(Vec3::Z, z)
    }

    pub fn normalize(self) -> Quat {
        let l = (self.x * self.x + self.y * self.y + self.z * self.z + self.w * self.w).sqrt();
        Quat {
            x: self.x / l,
            y: self.y / l,
            z: self.z / l,
            w: self.w / l,
        }
    }

    /// Rotate a vector.
    pub fn rotate(self, v: Vec3) -> Vec3 {
        let u = Vec3::new(self.x, self.y, self.z);
        let t = u.cross(v) * 2.0;
        v + t * self.w + u.cross(t)
    }
}

/// Hamilton product `self · o` (apply `o` first, then `self`).
impl std::ops::Mul for Quat {
    type Output = Quat;
    fn mul(self, o: Quat) -> Quat {
        Quat {
            x: self.w * o.x + self.x * o.w + self.y * o.z - self.z * o.y,
            y: self.w * o.y - self.x * o.z + self.y * o.w + self.z * o.x,
            z: self.w * o.z + self.x * o.y - self.y * o.x + self.z * o.w,
            w: self.w * o.w - self.x * o.x - self.y * o.y - self.z * o.z,
        }
    }
}

impl From<Quat> for [f32; 4] {
    fn from(q: Quat) -> Self {
        [q.x, q.y, q.z, q.w]
    }
}

/// A 4×4 matrix stored as four columns (`cols[c][r]`), glam layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mat4 {
    pub cols: [[f32; 4]; 4],
}

impl Mat4 {
    pub const IDENTITY: Mat4 = Mat4 {
        cols: [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ],
    };

    /// `T · R · S` (glam `from_scale_rotation_translation`).
    pub fn from_scale_rotation_translation(s: Vec3, r: Quat, t: Vec3) -> Mat4 {
        let cx = r.rotate(Vec3::X) * s.x;
        let cy = r.rotate(Vec3::Y) * s.y;
        let cz = r.rotate(Vec3::Z) * s.z;
        Mat4 {
            cols: [
                [cx.x, cx.y, cx.z, 0.0],
                [cy.x, cy.y, cy.z, 0.0],
                [cz.x, cz.y, cz.z, 0.0],
                [t.x, t.y, t.z, 1.0],
            ],
        }
    }

    /// `self · o`.
    pub fn mul(&self, o: &Mat4) -> Mat4 {
        let mut out = [[0.0f32; 4]; 4];
        for (c, col) in out.iter_mut().enumerate() {
            for (r, cell) in col.iter_mut().enumerate() {
                *cell = (0..4).map(|k| self.cols[k][r] * o.cols[c][k]).sum();
            }
        }
        Mat4 { cols: out }
    }

    pub fn translation(&self) -> Vec3 {
        Vec3::new(self.cols[3][0], self.cols[3][1], self.cols[3][2])
    }

    pub fn transform_point(&self, p: Vec3) -> Vec3 {
        let c = &self.cols;
        Vec3::new(
            c[0][0] * p.x + c[1][0] * p.y + c[2][0] * p.z + c[3][0],
            c[0][1] * p.x + c[1][1] * p.y + c[2][1] * p.z + c[3][1],
            c[0][2] * p.x + c[1][2] * p.y + c[2][2] * p.z + c[3][2],
        )
    }
}

/// An FBX `Model`'s local TRS as a matrix (mirrors `avatar_pose::lcl_to_mat4`): `Lcl Rotation`
/// is XYZ Euler in degrees.
pub fn lcl_to_mat4(t: &LocalTransform) -> Mat4 {
    let [tx, ty, tz] = t.translation.unwrap_or([0.0; 3]);
    let [rx, ry, rz] = t.rotation.unwrap_or([0.0; 3]);
    let [sx, sy, sz] = t.scaling.unwrap_or([1.0; 3]);
    let rot = Quat::from_euler_xyz(
        (rx as f32).to_radians(),
        (ry as f32).to_radians(),
        (rz as f32).to_radians(),
    );
    Mat4::from_scale_rotation_translation(
        Vec3::new(sx as f32, sy as f32, sz as f32),
        rot,
        Vec3::new(tx as f32, ty as f32, tz as f32),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < 1e-4
    }

    #[test]
    fn rotation_arc_aligns_vectors() {
        let q = Quat::from_rotation_arc(Vec3::Z, Vec3::Y);
        assert!(close(q.rotate(Vec3::Z), Vec3::Y));
        let q = Quat::from_rotation_arc(Vec3::new(0.0, -1.0, 0.0), Vec3::Y);
        assert!(close(q.rotate(Vec3::new(0.0, -1.0, 0.0)), Vec3::Y));
        assert_eq!(Quat::from_rotation_arc(Vec3::Y, Vec3::Y), Quat::IDENTITY);
    }

    #[test]
    fn z_up_correction_maps_z_to_y() {
        let q = Quat::from_axis_angle(Vec3::X, -std::f32::consts::FRAC_PI_2);
        assert!(close(
            q.rotate(Vec3::new(1.0, 2.0, 3.0)),
            Vec3::new(1.0, 3.0, -2.0)
        ));
    }

    #[test]
    fn euler_xyz_composes_x_then_y_then_z_matrices() {
        // Rx(90)·Ry(90) applied to +Z: Ry first maps Z → X, then Rx leaves X alone.
        let q = Quat::from_euler_xyz(90f32.to_radians(), 90f32.to_radians(), 0.0);
        assert!(close(q.rotate(Vec3::Z), Vec3::X));
        // Rx(90) alone: Y → Z.
        let q = Quat::from_euler_xyz(90f32.to_radians(), 0.0, 0.0);
        assert!(close(q.rotate(Vec3::Y), Vec3::Z));
    }

    #[test]
    fn trs_chain_composes_like_a_scene_graph() {
        let parent = lcl_to_mat4(&LocalTransform {
            translation: Some([0.0, 10.0, 0.0]),
            rotation: Some([0.0, 0.0, 90.0]),
            scaling: Some([2.0, 2.0, 2.0]),
        });
        let child = lcl_to_mat4(&LocalTransform {
            translation: Some([1.0, 0.0, 0.0]),
            rotation: None,
            scaling: None,
        });
        // child origin: scaled to (2,0,0), rotated 90° about Z → (0,2,0), plus (0,10,0).
        let t = parent.mul(&child).translation();
        assert!(close(t, Vec3::new(0.0, 12.0, 0.0)), "{t:?}");
        assert!(close(
            parent.transform_point(Vec3::new(1.0, 0.0, 0.0)),
            Vec3::new(0.0, 12.0, 0.0)
        ));
    }
}
