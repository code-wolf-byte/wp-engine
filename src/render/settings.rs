use crate::platform::RenderQuality;

/// Render settings shared between the UI and the wallpaper thread.
pub struct RenderSettings {
    /// GPU render quality / source down-sample factor.
    pub quality: RenderQuality,
    /// Volume in 0.0–1.0 (stored for future audio support; not yet applied).
    pub volume: f32,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            quality: RenderQuality::Ultra,
            volume: 1.0,
        }
    }
}
