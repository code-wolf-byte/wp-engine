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
}

struct EmitterState {
    rate: f32,
    origin: [f32; 3],
    directions: [f32; 3],
    speed_min: f32,
    speed_max: f32,
    accumulator: f32,
}

impl ParticleSystem {
    pub fn from_config(config: &ParticleConfig, width: u32, height: u32) -> Self {
        let emitters = config.emitter.iter().map(|e| {
            let origin = parse_f32_vec3(e.origin.as_deref());
            let directions = parse_f32_vec3(e.directions.as_deref());
            EmitterState {
                rate: e.rate as f32,
                origin,
                directions,
                speed_min: e.distancemin.unwrap_or(10.0) as f32,
                speed_max: e.distancemax.unwrap_or(100.0) as f32,
                accumulator: 0.0,
            }
        }).collect();

        let max_count = if config.maxcount > 0 {
            config.maxcount as usize
        } else {
            500
        };

        Self {
            particles: Vec::with_capacity(max_count),
            emitters,
            max_count,
            width,
            height,
        }
    }

    pub fn step(&mut self, dt: f32) {
        let w = self.width as f32;
        let h = self.height as f32;

        for p in &mut self.particles {
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
                let speed = emitter.speed_min
                    + fastrand::f32() * (emitter.speed_max - emitter.speed_min);
                let angle = fastrand::f32() * std::f32::consts::TAU;
                let life = 2.0 + fastrand::f32() * 4.0;

                let ox = if emitter.origin[0] > 0.0 { emitter.origin[0] } else { w * 0.5 };
                let oy = if emitter.origin[1] > 0.0 { emitter.origin[1] } else { h * 0.5 };

                let vx = angle.cos() * speed * emitter.directions[0].max(0.1);
                let vy = angle.sin() * speed * emitter.directions[1].max(0.1);

                self.particles.push(Particle {
                    x: ox + (fastrand::f32() - 0.5) * w * 0.2,
                    y: oy + (fastrand::f32() - 0.5) * h * 0.2,
                    vx,
                    vy,
                    life,
                    max_life: life,
                    size: 2.0 + fastrand::f32() * 4.0,
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
            let alpha = (p.alpha * 180.0) as u8;
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

fn parse_f32_vec3(s: Option<&str>) -> [f32; 3] {
    let Some(s) = s else { return [0.0; 3] };
    let parts: Vec<f32> = s.split_whitespace().filter_map(|p| p.parse().ok()).collect();
    [
        parts.first().copied().unwrap_or(0.0),
        parts.get(1).copied().unwrap_or(0.0),
        parts.get(2).copied().unwrap_or(0.0),
    ]
}
