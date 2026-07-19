//! Perspective camera for genuine 3D scenes (`Scene::is_perspective`).
//!
//! The C++ reference has no perspective path — CScene.cpp unconditionally
//! calls `setOrthogonalProjection`, so 3D scenes render broken there. This
//! module implements standard lookAt + perspective math (same conventions as
//! `glm::lookAt`/`glm::perspective`, which the reference does use for
//! particle-space projections in CParticle.cpp) so image layers placed at 3D
//! origins can be projected to screen space.

use crate::engine::scene::Scene;

/// Column-major 4x4, `m[col][row]` — glm's layout, so it uploads to a WGSL
/// `mat4x4<f32>` as-is.
pub type Mat4 = [[f32; 4]; 4];

pub fn mat4_mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [[0.0f32; 4]; 4];
    for (col, out_col) in out.iter_mut().enumerate() {
        for (row, cell) in out_col.iter_mut().enumerate() {
            *cell = (0..4).map(|k| a[k][row] * b[col][k]).sum();
        }
    }
    out
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = dot(v, v).sqrt();
    if len <= 1e-12 {
        return [0.0, 0.0, 1.0];
    }
    [v[0] / len, v[1] / len, v[2] / len]
}

/// Right-handed lookAt, identical to `glm::lookAt`.
fn look_at(eye: [f32; 3], center: [f32; 3], up: [f32; 3]) -> Mat4 {
    let f = normalize(sub(center, eye));
    let s = normalize(cross(f, up));
    let u = cross(s, f);
    [
        [s[0], u[0], -f[0], 0.0],
        [s[1], u[1], -f[1], 0.0],
        [s[2], u[2], -f[2], 0.0],
        [-dot(s, eye), -dot(u, eye), dot(f, eye), 1.0],
    ]
}

/// Right-handed perspective projection, identical to `glm::perspective`
/// (vertical FOV in radians, GL clip-space z).
fn perspective(fovy: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    let t = 1.0 / (fovy / 2.0).tan();
    let mut m = [[0.0f32; 4]; 4];
    m[0][0] = t / aspect;
    m[1][1] = t;
    m[2][2] = (far + near) / (near - far);
    m[2][3] = -1.0;
    m[3][2] = (2.0 * far * near) / (near - far);
    m
}

/// A resolved perspective camera: projects world-space points to NDC.
pub struct PerspectiveCamera {
    view: Mat4,
    view_proj: Mat4,
}

impl PerspectiveCamera {
    /// Build from scene.json camera/general. Returns `None` unless
    /// `scene.is_perspective()`.
    pub fn from_scene(scene: &Scene, aspect: f32) -> Option<Self> {
        if !scene.is_perspective() {
            return None;
        }
        let cam = scene.camera.as_ref()?;
        let eye3 = cam.parsed_eye()?;
        let center3 = cam.parsed_center()?;
        let up3 = cam.parsed_up().unwrap_or([0.0, 1.0, 0.0]);
        let g = scene.general.as_ref();
        let fov_deg = g
            .and_then(|g| g.fov.as_ref())
            .and_then(crate::engine::scene::parse_value_f32)
            .unwrap_or(50.0);
        let near = g
            .and_then(|g| g.nearz.as_ref())
            .and_then(crate::engine::scene::parse_value_f32)
            .unwrap_or(0.1)
            .max(1e-4);
        let far = g
            .and_then(|g| g.farz.as_ref())
            .and_then(crate::engine::scene::parse_value_f32)
            .unwrap_or(10000.0)
            .max(near * 2.0);

        let eye = eye3.map(|v| v as f32);
        let center = center3.map(|v| v as f32);
        let up = up3.map(|v| v as f32);
        let view = look_at(eye, center, up);
        let proj = perspective(fov_deg.to_radians(), aspect, near, far);
        Some(Self {
            view,
            view_proj: mat4_mul(&proj, &view),
        })
    }

    /// Project a world-space point. Returns `(ndc_x, ndc_y)`; `None` when the
    /// point is at or behind the eye plane (w <= 0).
    pub fn project(&self, p: [f32; 3]) -> Option<[f32; 2]> {
        let m = &self.view_proj;
        let x = m[0][0] * p[0] + m[1][0] * p[1] + m[2][0] * p[2] + m[3][0];
        let y = m[0][1] * p[0] + m[1][1] * p[1] + m[2][1] * p[2] + m[3][1];
        let w = m[0][3] * p[0] + m[1][3] * p[1] + m[2][3] * p[2] + m[3][3];
        if w <= 1e-6 {
            return None;
        }
        Some([x / w, y / w])
    }

    /// Distance of a world-space point along the camera's forward axis
    /// (positive in front of the camera). Used for painter's-algorithm depth
    /// sorting.
    pub fn view_depth(&self, p: [f32; 3]) -> f32 {
        let m = &self.view;
        -(m[0][2] * p[0] + m[1][2] * p[1] + m[2][2] * p[2] + m[3][2])
    }

    /// `view_proj` with GL's clip-space z ([-1,1]) remapped to wgpu's ([0,1]),
    /// column-major and ready to upload. [`project`](Self::project) reads only
    /// x/y so it never needed this; a depth buffer does.
    pub fn view_proj_gpu(&self) -> Mat4 {
        let mut fix = identity();
        fix[2][2] = 0.5;
        fix[3][2] = 0.5;
        mat4_mul(&fix, &self.view_proj)
    }

    /// Full model-view-projection for an object, column-major and ready to
    /// upload as a `mat4x4<f32>` uniform.
    pub fn mvp(&self, origin: [f32; 3], angles: [f32; 3], scale: [f32; 3]) -> Mat4 {
        mat4_mul(&self.view_proj_gpu(), &model_matrix(origin, angles, scale))
    }
}

pub fn identity() -> Mat4 {
    let mut m = [[0.0f32; 4]; 4];
    for (i, row) in m.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    m
}

/// Model matrix for a scene object: translate(origin) · Rz·Ry·Rx · scale —
/// the same convention as [`rotate_euler`].
pub fn model_matrix(origin: [f32; 3], angles: [f32; 3], scale: [f32; 3]) -> Mat4 {
    let (sx, cx) = angles[0].sin_cos();
    let (sy, cy) = angles[1].sin_cos();
    let (sz, cz) = angles[2].sin_cos();
    // Rz·Ry·Rx, expanded, with each column pre-scaled.
    let r = [
        [cz * cy, sz * cy, -sy],
        [cz * sy * sx - sz * cx, sz * sy * sx + cz * cx, cy * sx],
        [cz * sy * cx + sz * sx, sz * sy * cx - cz * sx, cy * cx],
    ];
    let mut m = identity();
    for c in 0..3 {
        for row in 0..3 {
            m[c][row] = r[c][row] * scale[c];
        }
    }
    m[3][0] = origin[0];
    m[3][1] = origin[1];
    m[3][2] = origin[2];
    m
}

/// Rotate `v` by intrinsic Euler angles in radians (WE `angles`, stored as
/// radians in scene.json), applied as Rz·Ry·Rx like a standard model matrix.
pub fn rotate_euler(v: [f32; 3], angles: [f32; 3]) -> [f32; 3] {
    let (sx, cx) = angles[0].sin_cos();
    let (sy, cy) = angles[1].sin_cos();
    let (sz, cz) = angles[2].sin_cos();
    // Rx
    let v = [v[0], cx * v[1] - sx * v[2], sx * v[1] + cx * v[2]];
    // Ry
    let v = [cy * v[0] + sy * v[2], v[1], -sy * v[0] + cy * v[2]];
    // Rz
    [cz * v[0] - sz * v[1], sz * v[0] + cz * v[1], v[2]]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `model_matrix` expands the same Rz·Ry·Rx that `rotate_euler` applies
    /// step by step — they must not drift apart.
    #[test]
    fn model_matrix_rotation_matches_rotate_euler() {
        let angles = [0.3, -1.1, 2.4];
        let scale = [2.0, 0.5, 3.0];
        let origin = [10.0, -4.0, 7.0];
        let m = model_matrix(origin, angles, scale);
        for v in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.3, -0.7, 0.5]] {
            let want = rotate_euler([v[0] * scale[0], v[1] * scale[1], v[2] * scale[2]], angles);
            let got: Vec<f32> = (0..3)
                .map(|r| m[0][r] * v[0] + m[1][r] * v[1] + m[2][r] * v[2] + m[3][r])
                .collect();
            for i in 0..3 {
                assert!(
                    (got[i] - (want[i] + origin[i])).abs() < 1e-5,
                    "axis {i}: got {got:?} want {want:?} + {origin:?}"
                );
            }
        }
    }

    fn scene_json(json: &str) -> Scene {
        serde_json::from_str(json).unwrap()
    }

    fn test_scene() -> Scene {
        scene_json(
            r#"{
                "camera": {"eye": "0 0 10", "center": "0 0 0", "up": "0 1 0"},
                "general": {"orthogonalprojection": null, "fov": 90.0, "nearz": 0.1, "farz": 100.0}
            }"#,
        )
    }

    #[test]
    fn detects_perspective_scene() {
        assert!(test_scene().is_perspective());
        // Usable ortho projection → not perspective.
        let ortho = scene_json(
            r#"{
                "camera": {"eye": "0 0 50", "center": "0 0 0",
                           "orthogonalprojection": {"width": 1920, "height": 1080}}
            }"#,
        );
        assert!(!ortho.is_perspective());
        // No camera eye/center → cannot build a perspective view.
        let no_cam = scene_json(r#"{"general": {}}"#);
        assert!(!no_cam.is_perspective());
    }

    #[test]
    fn projects_points_in_front_of_camera() {
        let cam = PerspectiveCamera::from_scene(&test_scene(), 1.0).unwrap();
        // Point straight ahead lands at NDC center.
        let ndc = cam.project([0.0, 0.0, 0.0]).unwrap();
        assert!(ndc[0].abs() < 1e-5 && ndc[1].abs() < 1e-5);
        // fov 90°, distance 10: y=10 sits exactly on the top edge (ndc.y = 1).
        let ndc = cam.project([0.0, 10.0, 0.0]).unwrap();
        assert!((ndc[1] - 1.0).abs() < 1e-4, "got {ndc:?}");
        // Behind the camera → culled.
        assert!(cam.project([0.0, 0.0, 20.0]).is_none());
    }

    #[test]
    fn view_depth_orders_near_to_far() {
        let cam = PerspectiveCamera::from_scene(&test_scene(), 1.0).unwrap();
        let near = cam.view_depth([0.0, 0.0, 5.0]);
        let far = cam.view_depth([0.0, 0.0, -20.0]);
        assert!(near > 0.0 && far > near);
        assert!(cam.view_depth([0.0, 0.0, 15.0]) < 0.0); // behind eye
    }

    #[test]
    fn euler_rotation_z_only_matches_2d() {
        let v = rotate_euler([1.0, 0.0, 0.0], [0.0, 0.0, std::f32::consts::FRAC_PI_2]);
        assert!((v[0]).abs() < 1e-6 && (v[1] - 1.0).abs() < 1e-6);
    }
}
