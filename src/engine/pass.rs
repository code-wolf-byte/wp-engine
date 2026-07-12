//! Single render pass descriptor (mirrors CPass from linux-wallpaperengine).
//! Holds the parameters for one effect step: pipeline key, extra textures, and UBO bytes.
//! Does not own the GPU pipeline (that lives in GpuSceneRenderer).

/// One effect pass: pipeline key + uniform data + extra texture keys.
pub struct ScenePass {
    /// Key into the pipeline registry (e.g. "waterripple", "pulse", "custom_abc123").
    pub pipeline_key: String,
    /// Packed bytes for the effect UBO (group 1, binding 0). Always ≥16 bytes.
    pub uniform_data: Vec<u8>,
    /// Extra texture keys (g_Texture1, ...) looked up in ResourceManager.
    pub extra_texture_keys: Vec<String>,
}

impl ScenePass {
    pub fn new(
        pipeline_key: impl Into<String>,
        uniform_data: Vec<u8>,
        extra_texture_keys: Vec<String>,
    ) -> Self {
        let mut data = uniform_data;
        while data.len() < 16 || data.len() % 16 != 0 {
            data.push(0);
        }
        Self {
            pipeline_key: pipeline_key.into(),
            uniform_data: data,
            extra_texture_keys,
        }
    }

    /// Pack f32 values into a 16-byte-aligned UBO byte vec.
    pub fn pack_uniforms(values: &[f32]) -> Vec<u8> {
        let mut bytes: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        while bytes.len() < 16 || bytes.len() % 16 != 0 {
            bytes.push(0);
        }
        bytes
    }

    /// True if this is one of the hardcoded built-in WGSL effects.
    pub fn is_builtin(&self) -> bool {
        const BUILTIN: &[&str] = &[
            "pulse",
            "scroll",
            "shake",
            "tint",
            "opacity",
            "waterripple",
            "waterwaves",
            "spin",
        ];
        BUILTIN.contains(&self.pipeline_key.as_str())
    }
}

/// Named f32 uniform value for building pass UBOs.
pub struct UniformValue {
    pub name: String,
    pub value: f32,
}

impl UniformValue {
    pub fn new(name: impl Into<String>, value: f32) -> Self {
        Self {
            name: name.into(),
            value,
        }
    }
}
