use wp_engine::engine::scene::{Camera, General, OrthogonalProjection, Scene};
use wp_engine::engine::SceneCamera;

fn scene_with_camera_proj(w: u32, h: u32) -> Scene {
    Scene {
        camera: Some(Camera {
            center: None,
            eye: None,
            up: None,
            parallax_amount: None,
            orthogonal_projection: Some(OrthogonalProjection {
                auto: None,
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
            orthogonal_projection: Some(OrthogonalProjection {
                auto: None,
                width: Some(w),
                height: Some(h),
            }),
            ..Default::default()
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
    // WE origins are absolute scene coordinates: a fullscreen object's center
    // is (w/2, h/2), not (0, 0).
    let cam = SceneCamera::new(1920.0, 1080.0);
    let (offset, size) = cam.object_to_quad([960.0, 540.0, 0.0], [1920.0, 1080.0]);
    assert!((offset[0]).abs() < 1e-6);
    assert!((offset[1]).abs() < 1e-6);
    assert!((size[0] - 1.0).abs() < 1e-6);
    assert!((size[1] - 1.0).abs() < 1e-6);
}

#[test]
fn object_to_quad_y_axis_points_up() {
    // An object near the top of the scene (large Y in WE coords) must land
    // near the top of the screen (small offset_norm[1]).
    let cam = SceneCamera::new(1000.0, 1000.0);
    let (offset, _) = cam.object_to_quad([500.0, 900.0, 0.0], [100.0, 100.0]);
    assert!(
        offset[1] < 0.1,
        "high WE Y should map near screen top, got {}",
        offset[1]
    );
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
