use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::render::RenderSettings;

/// Runtime configuration for a wallpaper run (the Rust counterpart of the C++
/// `ApplicationContext`). Built from the CLI by `main.rs` and consumed by
/// [`super::WallpaperApplication`].
#[derive(Debug, Clone)]
pub struct ApplicationContext {
    /// The wallpaper to show: a workshop directory, scene directory, or a
    /// plain image/video file.
    pub background: PathBuf,
    /// `--set-property name=value` overrides, applied to every scene loaded
    /// in this process via the engine property system.
    pub properties: HashMap<String, String>,
    /// Live render settings shared with the platform layer (quality, volume…).
    pub settings: Arc<Mutex<RenderSettings>>,
}

impl ApplicationContext {
    pub fn new(background: PathBuf) -> Self {
        Self {
            background,
            properties: HashMap::new(),
            settings: Arc::new(Mutex::new(RenderSettings::default())),
        }
    }

    /// Add `--set-property` style arguments (`name=value`, bare `name` = "1").
    pub fn add_property_args<I, S>(&mut self, args: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        for arg in args {
            let (name, value) = crate::engine::properties::parse_property_arg(arg.as_ref());
            self.properties.insert(name, value);
        }
    }
}
