use anyhow::{Context, Result};
use image::RgbaImage;
use std::path::Path;
use std::sync::mpsc::SyncSender;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::effect::SceneEffect;
use super::particle::{ParticleConfig, ParticleSystem};
use super::render::ResolvedScene;
use super::scene::SceneObject;

struct SceneAnimState {
    base_layers: Vec<Arc<RgbaImage>>,
    particles: Vec<ParticleSystem>,
    effects: Vec<LayerEffect>,
    width: u32,
    height: u32,
}

struct LayerEffect {
    effect_name: String,
    values: serde_json::Value,
}

impl SceneAnimState {
    fn load(dir: &Path) -> Result<Self> {
        let resolved = ResolvedScene::from_directory(dir)?;
        let width = resolved.width;
        let height = resolved.height;

        let base_layers: Vec<Arc<RgbaImage>> = resolved
            .layers
            .into_iter()
            .map(|l| {
                let resized = if l.image.width() != width || l.image.height() != height {
                    image::imageops::resize(
                        &l.image,
                        width,
                        height,
                        image::imageops::FilterType::Lanczos3,
                    )
                } else {
                    l.image
                };
                Arc::new(resized)
            })
            .collect();

        // Parse particles from scene.json
        let scene_path = dir.join("scene.json");
        let mut particles = Vec::new();
        let mut effects = Vec::new();

        if let Ok(scene_json) = std::fs::read_to_string(&scene_path) {
            if let Ok(scene) = super::scene::Scene::from_json(&scene_json) {
                for obj in &scene.objects {
                    if !obj.is_visible() {
                        continue;
                    }
                    load_particles(dir, obj, width, height, &mut particles);
                    load_effects(obj, &mut effects);
                }
            }
        } else {
            let pkg_path = dir.join("scene.pkg");
            if pkg_path.exists() {
                if let Ok(pkg) = super::pkg::Package::from_file(&pkg_path) {
                    if let Some(data) = pkg.get("scene.json") {
                        if let Ok(json) = std::str::from_utf8(data) {
                            if let Ok(scene) = super::scene::Scene::from_json(json) {
                                for obj in &scene.objects {
                                    if !obj.is_visible() {
                                        continue;
                                    }
                                    load_particles_pkg(&pkg, obj, width, height, &mut particles);
                                    load_effects(obj, &mut effects);
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(Self {
            base_layers,
            particles,
            effects,
            width,
            height,
        })
    }

    fn render_frame(&mut self, time: f32, dt: f32) -> RgbaImage {
        let mut canvas = RgbaImage::new(self.width, self.height);

        for layer in &self.base_layers {
            image::imageops::overlay(&mut canvas, layer.as_ref(), 0, 0);
        }

        for effect in &self.effects {
            if let Some(eff) =
                SceneEffect::from_effect_name(&effect.effect_name, &effect.values, time)
            {
                eff.apply(&mut canvas);
            }
        }

        for ps in &mut self.particles {
            ps.step(dt);
            ps.render_onto(&mut canvas);
        }

        canvas
    }
}

/// Run an animated scene render loop, sending frames through the channel.
/// Exits when the receiver is dropped.
pub fn scene_render_loop(
    dir: &Path,
    tx: &SyncSender<Arc<RgbaImage>>,
    target_fps: f64,
) -> Result<()> {
    let mut state = SceneAnimState::load(dir)
        .with_context(|| format!("loading animated scene from {}", dir.display()))?;

    let frame_duration = Duration::from_secs_f64(1.0 / target_fps);
    let start = Instant::now();
    let mut last_frame = start;

    loop {
        let now = Instant::now();
        let time = now.duration_since(start).as_secs_f32();
        let dt = now.duration_since(last_frame).as_secs_f32();
        last_frame = now;

        let frame = state.render_frame(time, dt);
        if tx.send(Arc::new(frame)).is_err() {
            return Ok(());
        }

        let elapsed = now.elapsed();
        if elapsed < frame_duration {
            std::thread::sleep(frame_duration - elapsed);
        }
    }
}

fn load_particles(
    dir: &Path,
    obj: &SceneObject,
    width: u32,
    height: u32,
    particles: &mut Vec<ParticleSystem>,
) {
    let particle_ref = match &obj.particle {
        Some(serde_json::Value::String(s)) => s.clone(),
        _ => return,
    };

    let config_path = dir.join(&particle_ref);
    let data = match std::fs::read_to_string(&config_path) {
        Ok(d) => d,
        Err(_) => return,
    };

    if let Ok(config) = serde_json::from_str::<ParticleConfig>(&data) {
        particles.push(ParticleSystem::from_config(&config, width, height));
    }
}

fn load_particles_pkg(
    pkg: &super::pkg::Package,
    obj: &SceneObject,
    width: u32,
    height: u32,
    particles: &mut Vec<ParticleSystem>,
) {
    let particle_ref = match &obj.particle {
        Some(serde_json::Value::String(s)) => s.clone(),
        _ => return,
    };

    let data = match pkg.get(&particle_ref) {
        Some(d) => d,
        None => return,
    };

    let json = match std::str::from_utf8(data) {
        Ok(j) => j,
        Err(_) => return,
    };

    if let Ok(config) = serde_json::from_str::<ParticleConfig>(json) {
        particles.push(ParticleSystem::from_config(&config, width, height));
    }
}

fn load_effects(obj: &SceneObject, effects: &mut Vec<LayerEffect>) {
    for eff in &obj.effects {
        let effect_name = match &eff.name {
            Some(serde_json::Value::String(s)) => s.clone(),
            _ => continue,
        };

        for pass in &eff.passes {
            if let Some(material) = &pass.material {
                let name = material
                    .trim_start_matches("materials/effects/")
                    .trim_end_matches(".json")
                    .to_string();
                effects.push(LayerEffect {
                    effect_name: name,
                    values: serde_json::Value::Object(serde_json::Map::new()),
                });
            }
        }

        if effect_name.contains('/') {
            let short = effect_name
                .rsplit('/')
                .next()
                .unwrap_or(&effect_name)
                .trim_end_matches(".json");
            effects.push(LayerEffect {
                effect_name: short.to_string(),
                values: serde_json::Value::Object(serde_json::Map::new()),
            });
        }
    }
}
