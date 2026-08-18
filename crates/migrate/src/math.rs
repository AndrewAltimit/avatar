//! The little bit of f64 linear algebra the migration needs (composing prefab transforms to world
//! space, deriving eye-look rotations). Kept local and `glam`-free on purpose: this crate is in
//! the diagnose/generate graph, which stays free of the f32 runtime-rig math (see `CLAUDE.md`).
//! Conventions are Unity's: left-handed, +Y up, +Z forward, quaternion `(x, y, z, w)`.

/// A 3-vector.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    pub const ZERO: Vec3 = Vec3 {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };
    pub const ONE: Vec3 = Vec3 {
        x: 1.0,
        y: 1.0,
        z: 1.0,
    };
    pub const X: Vec3 = Vec3 {
        x: 1.0,
        y: 0.0,
        z: 0.0,
    };
    pub const Y: Vec3 = Vec3 {
        x: 0.0,
        y: 1.0,
        z: 0.0,
    };
    pub const Z: Vec3 = Vec3 {
        x: 0.0,
        y: 0.0,
        z: 1.0,
    };

    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Vec3 { x, y, z }
    }
    pub fn scale(self, s: f64) -> Vec3 {
        Vec3::new(self.x * s, self.y * s, self.z * s)
    }
    pub fn dot(self, o: Vec3) -> f64 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    pub fn cross(self, o: Vec3) -> Vec3 {
        Vec3::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }
    pub fn length(self) -> f64 {
        self.dot(self).sqrt()
    }
    pub fn distance(self, o: Vec3) -> f64 {
        (self - o).length()
    }
    pub fn normalized(self) -> Vec3 {
        let l = self.length();
        if l > 1e-12 { self.scale(1.0 / l) } else { self }
    }
    /// Unity's inline serialization: `{x: 0, y: 1.5, z: 0}`.
    pub fn to_yaml(self) -> String {
        format!(
            "{{x: {}, y: {}, z: {}}}",
            fmt(self.x),
            fmt(self.y),
            fmt(self.z)
        )
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

/// Component-wise product (non-uniform scale).
impl std::ops::Mul for Vec3 {
    type Output = Vec3;
    fn mul(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x * o.x, self.y * o.y, self.z * o.z)
    }
}

/// A unit quaternion `(x, y, z, w)`, Unity layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quat {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub w: f64,
}

impl Default for Quat {
    fn default() -> Self {
        Quat::IDENTITY
    }
}

impl Quat {
    pub const IDENTITY: Quat = Quat {
        x: 0.0,
        y: 0.0,
        z: 0.0,
        w: 1.0,
    };

    pub const fn new(x: f64, y: f64, z: f64, w: f64) -> Self {
        Quat { x, y, z, w }
    }

    /// Rotation of `angle_deg` degrees about `axis` (Unity's `Quaternion.AngleAxis`).
    pub fn axis_angle(axis: Vec3, angle_deg: f64) -> Quat {
        let a = axis.normalized();
        let half = angle_deg.to_radians() * 0.5;
        let s = half.sin();
        Quat::new(a.x * s, a.y * s, a.z * s, half.cos())
    }

    /// The inverse (conjugate, for a unit quaternion).
    pub fn inverse(self) -> Quat {
        Quat::new(-self.x, -self.y, -self.z, self.w)
    }

    /// Rotate a vector.
    pub fn rotate(self, v: Vec3) -> Vec3 {
        let qv = Vec3::new(self.x, self.y, self.z);
        let t = qv.cross(v).scale(2.0);
        v + t.scale(self.w) + qv.cross(t)
    }

    pub fn normalized(self) -> Quat {
        let l = (self.x * self.x + self.y * self.y + self.z * self.z + self.w * self.w).sqrt();
        if l > 1e-12 {
            Quat::new(self.x / l, self.y / l, self.z / l, self.w / l)
        } else {
            Quat::IDENTITY
        }
    }

    /// Unity's inline serialization: `{x: 0, y: 0, z: 0, w: 1}`.
    pub fn to_yaml(self) -> String {
        format!(
            "{{x: {}, y: {}, z: {}, w: {}}}",
            fmt(self.x),
            fmt(self.y),
            fmt(self.z),
            fmt(self.w)
        )
    }
}

/// Hamilton product `self * o` (apply `o` first, then `self` — Unity's `lhs * rhs`).
impl std::ops::Mul for Quat {
    type Output = Quat;
    fn mul(self, o: Quat) -> Quat {
        Quat::new(
            self.w * o.x + self.x * o.w + self.y * o.z - self.z * o.y,
            self.w * o.y - self.x * o.z + self.y * o.w + self.z * o.x,
            self.w * o.z + self.x * o.y - self.y * o.x + self.z * o.w,
            self.w * o.w - self.x * o.x - self.y * o.y - self.z * o.z,
        )
    }
}

/// A local transform (translation, rotation, scale) — what a Unity `Transform` serializes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Trs {
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Default for Trs {
    fn default() -> Self {
        Trs {
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        }
    }
}

impl Trs {
    /// Compose `parent ∘ child` (child expressed in the parent's space) into the parent's space.
    /// Scale composes component-wise, which is exact for the axis-aligned/uniform scales avatars
    /// use and Unity's own approximation otherwise (its `lossyScale` is likewise approximate).
    pub fn then(self, child: Trs) -> Trs {
        Trs {
            position: self.position + self.rotation.rotate(child.position * self.scale),
            rotation: (self.rotation * child.rotation).normalized(),
            scale: self.scale * child.scale,
        }
    }

    /// Transform a point from this space to the parent space.
    pub fn transform_point(&self, p: Vec3) -> Vec3 {
        self.position + self.rotation.rotate(p * self.scale)
    }
}

/// Render an `f64` the way Unity writes floats: shortest round-trip form, integers bare.
pub fn fmt(v: f64) -> String {
    if v == v.trunc() && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        // f32 precision is what Unity stores; printing the f32 keeps the text short and faithful.
        format!("{}", v as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3) -> bool {
        a.distance(b) < 1e-6
    }

    #[test]
    fn axis_angle_rotates_forward_up_for_negative_x() {
        // Unity: a negative rotation about +X pitches +Z (forward) upward.
        let q = Quat::axis_angle(Vec3::X, -90.0);
        assert!(close(q.rotate(Vec3::Z), Vec3::Y));
        // Positive Y yaw turns forward toward +X (to the right).
        let q = Quat::axis_angle(Vec3::Y, 90.0);
        assert!(close(q.rotate(Vec3::Z), Vec3::X));
    }

    #[test]
    fn quat_mul_applies_rhs_first() {
        let a = Quat::axis_angle(Vec3::Y, 90.0);
        let b = Quat::axis_angle(Vec3::X, 90.0);
        // (a*b) v == a(b v)
        let v = Vec3::new(0.3, -0.2, 0.9);
        assert!(close((a * b).rotate(v), a.rotate(b.rotate(v))));
        assert!(close((a * a.inverse()).rotate(v), v));
    }

    #[test]
    fn trs_composition_matches_manual() {
        let parent = Trs {
            position: Vec3::new(1.0, 2.0, 3.0),
            rotation: Quat::axis_angle(Vec3::Y, 90.0),
            scale: Vec3::new(2.0, 2.0, 2.0),
        };
        let child = Trs {
            position: Vec3::new(0.0, 0.0, 1.0),
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        };
        let w = parent.then(child);
        // Child sits 1 (×2 scale) along the parent's forward, which the yaw turned to +X.
        assert!(close(w.position, Vec3::new(3.0, 2.0, 3.0)));
        assert!(close(w.transform_point(Vec3::ZERO), w.position));
    }

    #[test]
    fn fmt_is_unity_like() {
        assert_eq!(fmt(0.0), "0");
        assert_eq!(fmt(1.0), "1");
        assert_eq!(fmt(-2.0), "-2");
        assert_eq!(fmt(0.5), "0.5");
        assert_eq!(
            Vec3::new(0.0, 0.995, 0.06).to_yaml(),
            "{x: 0, y: 0.995, z: 0.06}"
        );
        assert_eq!(Quat::IDENTITY.to_yaml(), "{x: 0, y: 0, z: 0, w: 1}");
    }
}
