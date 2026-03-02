use crate::platform::RenderQuality;

/// Render settings shared between the UI and the wallpaper thread.
pub struct RenderSettings {
    /// Quality hint passed to Wallpaper Engine (stored for UI display).
    pub quality: RenderQuality,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self { quality: RenderQuality::Ultra }
    }
}
