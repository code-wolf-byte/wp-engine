use image::RgbaImage;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct ParticleConfig {
    #[serde(default)]
    pub emitter: Vec<Emitter>,
    #[serde(default)]
    pub maxcount: u32,
    #[serde(default)]
    pub initializer: Vec<Initializer>,
    #[serde(default)]
    pub operator: Vec<Operator>,
    #[serde(default)]
    pub renderer: Vec<serde_json::Value>,
    #[serde(default)]
    pub flags: u32,
}

#[derive(Debug, Deserialize)]
pub struct Emitter {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub rate: f64,
    #[serde(default)]
    pub origin: Option<String>,
    #[serde(default)]
    pub directions: Option<String>,
    #[serde(default)]
    pub distancemin: Option<f64>,
    #[serde(default)]
    pub distancemax: Option<f64>,
    #[serde(default)]
    pub sign: Option<String>,
    #[serde(default)]
    pub id: u32,
}

#[derive(Debug, Deserialize)]
pub struct Initializer {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default)]
    pub min: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub struct Operator {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub value: Option<f64>,
}

#[derive(Clone)]
struct Particle {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    life: f32,
    max_life: f32,
    size: f32,
    alpha: f32,
    color: [u8; 3],
}

pub struct ParticleSystem {
    particles: Vec<Particle>,
    emitters: Vec<EmitterState>,
    max_count: usize,
    width: u32,
    height: u32,
    life_min: f32,
    life_max: f32,
    size_min: f32,
    size_max: f32,
    drag: f32,
    renderer_count: usize,
    flags: u32,
}

struct EmitterState {
    id: u32,
    kind: EmitterKind,
    rate: f32,
    origin: [f32; 3],
    directions: [f32; 3],
    sign: [f32; 3],
    speed_min: f32,
    speed_max: f32,
    accumulator: f32,
}

#[derive(Clone, Copy)]
enum EmitterKind {
    Box,
    Sphere,
}

impl ParticleSystem {
    pub fn from_config(config: &ParticleConfig, width: u32, height: u32) -> Self {
        let emitters = config
            .emitter
            .iter()
            .map(|e| {
                let origin = parse_f32_vec3(e.origin.as_deref());
                let directions = parse_f32_vec3(e.directions.as_deref());
                let sign = parse_f32_vec3(e.sign.as_deref());
                EmitterState {
                    id: e.id,
                    kind: if e.name.contains("sphere") {
                        EmitterKind::Sphere
                    } else {
                        EmitterKind::Box
                    },
                    rate: e.rate as f32,
                    origin,
                    directions,
                    sign,
                    speed_min: e.distancemin.unwrap_or(10.0) as f32,
                    speed_max: e.distancemax.unwrap_or(100.0) as f32,
                    accumulator: 0.0,
                }
            })
            .collect();

        let max_count = if config.maxcount > 0 {
            config.maxcount as usize
        } else {
            500
        };

        let (life_min, life_max) =
            random_range_from_initializers(&config.initializer, "lifetime", 2.0, 6.0);
        let (size_min, size_max) =
            random_range_from_initializers(&config.initializer, "size", 2.0, 6.0);
        let drag = config
            .operator
            .iter()
            .find(|op| op.name.contains("movement"))
            .and_then(|op| op.value)
            .unwrap_or(0.0) as f32;

        Self {
            particles: Vec::with_capacity(max_count),
            emitters,
            max_count,
            width,
            height,
            life_min,
            life_max,
            size_min,
            size_max,
            drag,
            renderer_count: config.renderer.len(),
            flags: config.flags,
        }
    }

    pub fn step(&mut self, dt: f32) {
        let w = self.width as f32;
        let h = self.height as f32;

        for p in &mut self.particles {
            if self.drag > 0.0 {
                let drag = (1.0 - self.drag * dt).clamp(0.0, 1.0);
                p.vx *= drag;
                p.vy *= drag;
            }
            p.x += p.vx * dt;
            p.y += p.vy * dt;
            p.life -= dt;
            p.alpha = (p.life / p.max_life).max(0.0);
        }

        self.particles.retain(|p| p.life > 0.0);

        for emitter in &mut self.emitters {
            emitter.accumulator += emitter.rate * dt;
            while emitter.accumulator >= 1.0 && self.particles.len() < self.max_count {
                emitter.accumulator -= 1.0;
                let speed =
                    emitter.speed_min + fastrand::f32() * (emitter.speed_max - emitter.speed_min);
                let angle = fastrand::f32() * std::f32::consts::TAU;
                let life =
                    self.life_min + fastrand::f32() * (self.life_max - self.life_min).max(0.0);
                let size =
                    self.size_min + fastrand::f32() * (self.size_max - self.size_min).max(0.0);

                let ox = if emitter.origin[0] > 0.0 {
                    emitter.origin[0]
                } else {
                    w * 0.5
                };
                let oy = if emitter.origin[1] > 0.0 {
                    emitter.origin[1]
                } else {
                    h * 0.5
                };

                let sign_x = if emitter.sign[0] == 0.0 {
                    1.0
                } else {
                    emitter.sign[0].signum()
                };
                let sign_y = if emitter.sign[1] == 0.0 {
                    1.0
                } else {
                    emitter.sign[1].signum()
                };
                let (spread_x, spread_y) = match emitter.kind {
                    EmitterKind::Box => (w * 0.2, h * 0.2),
                    EmitterKind::Sphere => (w * 0.1, h * 0.1),
                };
                let id_phase = emitter.id as f32 * 0.173;
                let vx = (angle + id_phase).cos() * speed * emitter.directions[0].max(0.1) * sign_x;
                let vy = (angle + id_phase).sin() * speed * emitter.directions[1].max(0.1) * sign_y;

                self.particles.push(Particle {
                    x: ox + (fastrand::f32() - 0.5) * spread_x,
                    y: oy + (fastrand::f32() - 0.5) * spread_y,
                    vx,
                    vy,
                    life,
                    max_life: life,
                    size,
                    alpha: 1.0,
                    color: [255, 255, 255],
                });
            }
        }
    }

    pub fn render_onto(&self, canvas: &mut RgbaImage) {
        let w = canvas.width() as i32;
        let h = canvas.height() as i32;

        for p in &self.particles {
            let renderer_boost = if self.renderer_count > 1 { 1.1 } else { 1.0 };
            let flag_boost = if self.flags & 1 != 0 { 1.15 } else { 1.0 };
            let alpha = (p.alpha * 180.0 * renderer_boost * flag_boost).clamp(0.0, 255.0) as u8;
            if alpha == 0 {
                continue;
            }
            let sz = p.size as i32;
            let cx = p.x as i32;
            let cy = p.y as i32;

            for dy in -sz..=sz {
                for dx in -sz..=sz {
                    if dx * dx + dy * dy > sz * sz {
                        continue;
                    }
                    let px = cx + dx;
                    let py = cy + dy;
                    if px < 0 || py < 0 || px >= w || py >= h {
                        continue;
                    }
                    let dst = canvas.get_pixel_mut(px as u32, py as u32);
                    let a = alpha as u16;
                    let inv = 255 - a;
                    dst[0] = ((dst[0] as u16 * inv + p.color[0] as u16 * a) / 255) as u8;
                    dst[1] = ((dst[1] as u16 * inv + p.color[1] as u16 * a) / 255) as u8;
                    dst[2] = ((dst[2] as u16 * inv + p.color[2] as u16 * a) / 255) as u8;
                }
            }
        }
    }
}

fn random_range_from_initializers(
    initializers: &[Initializer],
    needle: &str,
    default_min: f32,
    default_max: f32,
) -> (f32, f32) {
    let Some(init) = initializers.iter().find(|init| init.name.contains(needle)) else {
        return (default_min, default_max);
    };

    (
        init.min.unwrap_or(default_min as f64) as f32,
        init.max.unwrap_or(default_max as f64) as f32,
    )
}

fn parse_f32_vec3(s: Option<&str>) -> [f32; 3] {
    let Some(s) = s else { return [0.0; 3] };
    let parts: Vec<f32> = s
        .split_whitespace()
        .filter_map(|p| p.parse().ok())
        .collect();
    [
        parts.first().copied().unwrap_or(0.0),
        parts.get(1).copied().unwrap_or(0.0),
        parts.get(2).copied().unwrap_or(0.0),
    ]
}
