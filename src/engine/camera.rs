//! Orthogonal scene camera (mirrors Camera from linux-wallpaperengine).
//! Handles world-space → screen-space coordinate conversion for 2D scene objects.

/// Orthogonal projection camera for a 2D WE scene.
pub struct SceneCamera {
    pub width: f32,
    pub height: f32,
}

impl SceneCamera {
    pub fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }

    /// Derive dimensions from scene.json, falling back to `fallback_wh`.
    /// Priority: camera.orthogonal_projection → general.orthogonal_projection → fallback.
    pub fn from_scene(scene: &crate::engine::scene::Scene, fallback_wh: (u32, u32)) -> Self {
        if let Some(proj) = scene
            .camera
            .as_ref()
            .and_then(|c| c.orthogonal_projection.as_ref())
        {
            if let (Some(w), Some(h)) = (proj.width, proj.height) {
                return Self::new(w as f32, h as f32);
            }
        }
        if let Some(proj) = scene
            .general
            .as_ref()
            .and_then(|g| g.orthogonal_projection.as_ref())
        {
            if let (Some(w), Some(h)) = (proj.width, proj.height) {
                return Self::new(w as f32, h as f32);
            }
        }
        Self::new(fallback_wh.0 as f32, fallback_wh.1 as f32)
    }

    /// Convert WE world-space position+size to quad shader parameters.
    ///
    /// WE world space: Y-up, origin at scene center. Returns:
    /// - `offset_norm`: top-left corner as fraction of scene size `[0,1]`
    /// - `size_norm`: object size as fraction of scene size
    pub fn object_to_quad(
        &self,
        world_origin: [f32; 3],
        world_size: [f32; 2],
    ) -> ([f32; 2], [f32; 2]) {
        let left_px = self.width / 2.0 + world_origin[0] - world_size[0] / 2.0;
        let top_px = self.height / 2.0 - world_origin[1] - world_size[1] / 2.0;
        let offset_norm = [left_px / self.width, top_px / self.height];
        let size_norm = [world_size[0] / self.width, world_size[1] / self.height];
        (offset_norm, size_norm)
    }

    /// Column-major orthogonal projection matrix mapping world coords to NDC.
    /// Maps x∈[0,w], y∈[0,h] → NDC [-1,1].
    pub fn mvp_matrix(&self) -> [f32; 16] {
        let sx = 2.0 / self.width;
        let sy = -2.0 / self.height;
        [
            sx, 0.0, 0.0, 0.0, // col 0
            0.0, sy, 0.0, 0.0, // col 1
            0.0, 0.0, 1.0, 0.0, // col 2
            -1.0, 1.0, 0.0, 1.0, // col 3
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::scene::{Camera, General, OrthogonalProjection, Scene};

    fn scene_with_camera_proj(w: u32, h: u32) -> Scene {
        Scene {
            camera: Some(Camera {
                center: None,
                eye: None,
                parallax_amount: None,
                orthogonal_projection: Some(OrthogonalProjection {
                    width: Some(w),
                    height: Some(h),
                }),
            }),
            general: None,
            objects: vec![],
        }
    }

    fn scene_with_general_proj(w: u32, h: u32) -> Scene {
        Scene {
            camera: None,
            general: Some(General {
                supports_audio_processing: None,
                speed: None,
                clear_color: None,
                orthogonal_projection: Some(OrthogonalProjection {
                    width: Some(w),
                    height: Some(h),
                }),
            }),
            objects: vec![],
        }
    }

    fn empty_scene() -> Scene {
        Scene {
            camera: None,
            general: None,
            objects: vec![],
        }
    }

    #[test]
    fn from_scene_uses_camera_projection() {
        let cam = SceneCamera::from_scene(&scene_with_camera_proj(1920, 1080), (800, 600));
        assert_eq!(cam.width, 1920.0);
        assert_eq!(cam.height, 1080.0);
    }

    #[test]
    fn from_scene_falls_back_to_general() {
        let cam = SceneCamera::from_scene(&scene_with_general_proj(2560, 1440), (800, 600));
        assert_eq!(cam.width, 2560.0);
        assert_eq!(cam.height, 1440.0);
    }

    #[test]
    fn from_scene_falls_back_to_fallback_wh() {
        let cam = SceneCamera::from_scene(&empty_scene(), (1280, 720));
        assert_eq!(cam.width, 1280.0);
        assert_eq!(cam.height, 720.0);
    }

    #[test]
    fn object_to_quad_centered_full_screen() {
        let cam = SceneCamera::new(1920.0, 1080.0);
        let (offset, size) = cam.object_to_quad([0.0, 0.0, 0.0], [1920.0, 1080.0]);
        assert!((offset[0]).abs() < 1e-6);
        assert!((offset[1]).abs() < 1e-6);
        assert!((size[0] - 1.0).abs() < 1e-6);
        assert!((size[1] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn mvp_matrix_column_major() {
        let cam = SceneCamera::new(1920.0, 1080.0);
        let m = cam.mvp_matrix();
        assert!((m[0] - 2.0 / 1920.0).abs() < 1e-7);
        assert!((m[5] - -2.0 / 1080.0).abs() < 1e-7);
        assert_eq!(m[10], 1.0);
        assert_eq!(m[12], -1.0);
        assert_eq!(m[13], 1.0);
        assert_eq!(m[15], 1.0);
    }
}
