use image::RgbaImage;
use serde::Deserialize;

// Numeric/count fields use `Option<T>` rather than bare `T` with
// `#[serde(default)]`: real presets frequently write an explicit JSON `null`
// for unused fields (e.g. `"flags": null`), and `#[serde(default)]` only
// fills in *missing* keys — it still rejects an explicit `null` against a
// non-Option numeric type.
#[derive(Debug, Deserialize)]
pub struct ParticleConfig {
    #[serde(default)]
    pub emitter: Vec<Emitter>,
    #[serde(default)]
    pub maxcount: Option<u32>,
    #[serde(default)]
    pub initializer: Vec<Initializer>,
    #[serde(default)]
    pub operator: Vec<Operator>,
    #[serde(default)]
    pub renderer: Vec<serde_json::Value>,
    #[serde(default)]
    pub flags: Option<u32>,
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
    /// A scalar radius for sphere emitters, but a `"x y z"` box-extent vector
    /// for box emitters (e.g. `"distancemax": "1024 512 0"`).
    #[serde(default)]
    pub distancemin: Option<serde_json::Value>,
    #[serde(default)]
    pub distancemax: Option<serde_json::Value>,
    #[serde(default)]
    pub sign: Option<String>,
    #[serde(default)]
    pub id: Option<u32>,
}

/// `min`/`max` are untyped because real presets mix plain numbers
/// (`lifetimerandom`, `sizerandom`, `alpharandom`) with `"r g b"`/`"x y z"`
/// vector strings (`velocityrandom`, `colorrandom`, `angularvelocityrandom`) —
/// declaring these as `f64` makes serde reject any config using a vector
/// initializer.
#[derive(Debug, Deserialize)]
pub struct Initializer {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub max: Option<serde_json::Value>,
    #[serde(default)]
    pub min: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct Operator {
    #[serde(default)]
    pub name: String,
    /// The real field on a WE "movement" operator (a constant acceleration
    /// vector), not a generic scalar.
    #[serde(default)]
    pub gravity: Option<String>,
    /// `alphafade`: fade in over `[0, fadeintime]`, hold, fade out over
    /// `[fadeouttime, 1]` of normalized lifetime (CParticle.cpp createAlphaFadeOperator).
    #[serde(default)]
    pub fadeintime: Option<f64>,
    #[serde(default)]
    pub fadeouttime: Option<f64>,
    /// `sizechange`/`alphachange`/`colorchange`: linear ramp from `startvalue`
    /// to `endvalue` over normalized-lifetime `[starttime, endtime]`
    /// (CParticle.cpp's generic `fadeValue`).
    #[serde(default)]
    pub starttime: Option<f64>,
    #[serde(default)]
    pub endtime: Option<f64>,
    #[serde(default)]
    pub startvalue: Option<f64>,
    #[serde(default)]
    pub endvalue: Option<f64>,
    /// `oscillatealpha`/`oscillatesize`/`oscillateposition`: per-particle
    /// random frequency/scale/phase, resampled once at spawn.
    #[serde(default)]
    pub frequencymin: Option<f64>,
    #[serde(default)]
    pub frequencymax: Option<f64>,
    #[serde(default)]
    pub scalemin: Option<f64>,
    #[serde(default)]
    pub scalemax: Option<f64>,
    #[serde(default)]
    pub phasemin: Option<f64>,
    #[serde(default)]
    pub phasemax: Option<f64>,
}

/// Scene.json's per-instance `instanceoverride` on a particle object —
/// overrides the referenced preset's rate/size/speed/alpha/color.
#[derive(Debug, Default, Deserialize)]
pub struct InstanceOverride {
    pub alpha: Option<f64>,
    pub color: Option<String>,
    pub rate: Option<f64>,
    pub size: Option<f64>,
    pub speed: Option<f64>,
}

/// Per-particle oscillator state for `oscillatealpha`/`oscillatesize`: a
/// random frequency/scale/phase drawn once at spawn (CParticle.cpp captures
/// these lazily on first use, but since we never re-use particle slots
/// across configs, drawing them at spawn is equivalent).
#[derive(Clone, Copy)]
struct Oscillator1D {
    frequency: f32,
    scale: f32,
    phase: f32,
}

/// Same idea as `Oscillator1D` but per-axis for `oscillateposition`, which
/// integrates a velocity (derivative of `scale*cos(w*t+phase)`) into position
/// each frame rather than overwriting an absolute value.
#[derive(Clone, Copy)]
struct OscillatorPos {
    frequency: [f32; 2],
    scale: [f32; 2],
    phase: [f32; 2],
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
    /// Alpha/size as set by the initializers at spawn — `alphafade`/
    /// `sizechange` scale *this*, not the live (already-modified) value.
    initial_alpha: f32,
    initial_size: f32,
    osc_alpha: Option<Oscillator1D>,
    osc_size: Option<Oscillator1D>,
    osc_pos: Option<OscillatorPos>,
}

/// Parsed `oscillatealpha`/`oscillatesize`/`oscillateposition` operator
/// config — random ranges each spawned particle draws its own values from.
#[derive(Clone, Copy)]
struct OscillateParams {
    freq_min: f32,
    freq_max: f32,
    scale_min: f32,
    scale_max: f32,
    phase_min: f32,
    phase_max: f32,
}

impl OscillateParams {
    fn from_operator(op: &Operator) -> Self {
        let freq_min = op.frequencymin.unwrap_or(0.0) as f32;
        let freq_max = op.frequencymax.unwrap_or(freq_min as f64) as f32;
        let scale_min = op.scalemin.or(op.scalemax).unwrap_or(0.0) as f32;
        let scale_max = op.scalemax.or(op.scalemin).unwrap_or(0.0) as f32;
        let phase_min = op.phasemin.unwrap_or(0.0) as f32;
        let phase_max = op.phasemax.unwrap_or(0.0) as f32;
        Self {
            freq_min,
            freq_max,
            scale_min,
            scale_max,
            phase_min,
            phase_max,
        }
    }

    /// CParticle.cpp always draws phase from `[phaseMin, phaseMax + 2*PI]` —
    /// even with no phase fields at all (both 0), this yields a fully random
    /// phase in `[0, 2*PI]`, which is what real presets (lacking phase
    /// fields entirely) actually rely on.
    fn sample(&self) -> Oscillator1D {
        Oscillator1D {
            frequency: self.freq_min + fastrand::f32() * (self.freq_max - self.freq_min),
            scale: self.scale_min + fastrand::f32() * (self.scale_max - self.scale_min),
            phase: self.phase_min
                + fastrand::f32() * (self.phase_max + std::f32::consts::TAU - self.phase_min),
        }
    }
}

pub struct ParticleSystem {
    particles: Vec<Particle>,
    emitters: Vec<EmitterState>,
    max_count: usize,
    life_min: f32,
    life_max: f32,
    size_min: f32,
    size_max: f32,
    velocity_min: Option<[f32; 3]>,
    velocity_max: Option<[f32; 3]>,
    color_min: [f32; 3],
    color_max: [f32; 3],
    gravity: [f32; 2],
    /// `alphafade`: (fadeintime, fadeouttime), normalized-lifetime fractions.
    alphafade: Option<(f32, f32)>,
    /// `sizechange`/`alphachange`: (starttime, endtime, startvalue, endvalue).
    sizechange: Option<(f32, f32, f32, f32)>,
    alphachange: Option<(f32, f32, f32, f32)>,
    oscillate_alpha: Option<OscillateParams>,
    oscillate_size: Option<OscillateParams>,
    oscillate_position: Option<OscillateParams>,
    alpha_mult: f32,
    size_mult: f32,
    speed_mult: f32,
    rate_mult: f32,
    color_override: Option<[u8; 3]>,
}

struct EmitterState {
    id: u32,
    rate: f32,
    origin: [f32; 3],
    directions: [f32; 3],
    sign: [f32; 3],
    speed_min: f32,
    speed_max: f32,
    /// Spawn-position spread around `origin`: a box emitter's real extent
    /// (from `distancemax`, an `"x y z"` vector) when available, else a small
    /// default so particles don't all spawn from the exact same point.
    spread: [f32; 2],
    accumulator: f32,
}

impl ParticleSystem {
    /// Build a running particle system for one scene object.
    ///
    /// `spawn_center` is the object's own scene position, already converted to
    /// canvas pixel coordinates (see `render.rs`'s WE-origin-to-pixel
    /// conversion) — a WE emitter's own `origin` field is a *local* offset
    /// from the object's position, not an absolute canvas coordinate.
    pub fn from_config(
        config: &ParticleConfig,
        spawn_center: [f32; 2],
        overrides: Option<&InstanceOverride>,
    ) -> Self {
        let emitters = config
            .emitter
            .iter()
            .map(|e| {
                let local_origin = parse_f32_vec3(e.origin.as_deref());
                let directions = parse_f32_vec3(e.directions.as_deref());
                let sign = parse_f32_vec3(e.sign.as_deref());
                let is_sphere = e.name.contains("sphere");

                // Sphere emitters use a scalar radius for distancemin/max;
                // box emitters use an "x y z" extent vector — mixing the two
                // representations up would misparse either shape's real values.
                let (speed_min, speed_max, spread) = if is_sphere {
                    let min = e
                        .distancemin
                        .as_ref()
                        .and_then(value_as_f32)
                        .unwrap_or(10.0);
                    let max = e
                        .distancemax
                        .as_ref()
                        .and_then(value_as_f32)
                        .unwrap_or(100.0);
                    (min, max, [30.0, 30.0])
                } else {
                    let extent = e.distancemax.as_ref().and_then(value_as_vec3);
                    let spread = extent
                        .map(|v| [v[0].abs().max(1.0), v[1].abs().max(1.0)])
                        .unwrap_or([60.0, 60.0]);
                    (10.0, 100.0, spread)
                };

                EmitterState {
                    id: e.id.unwrap_or(0),
                    rate: e.rate as f32,
                    origin: [
                        spawn_center[0] + local_origin[0],
                        spawn_center[1] + local_origin[1],
                        local_origin[2],
                    ],
                    directions,
                    sign,
                    speed_min,
                    speed_max,
                    spread,
                    accumulator: 0.0,
                }
            })
            .collect();

        let max_count = match config.maxcount {
            Some(n) if n > 0 => n as usize,
            _ => 500,
        };

        let (life_min, life_max) =
            scalar_range_from_initializers(&config.initializer, "lifetime", 2.0, 6.0);
        let (size_min, size_max) =
            scalar_range_from_initializers(&config.initializer, "size", 2.0, 6.0);
        let velocity_range = vec3_range_from_initializers(&config.initializer, "velocity");
        let (velocity_min, velocity_max) = match velocity_range {
            Some((min, max)) => (Some(min), Some(max)),
            None => (None, None),
        };
        let (color_min, color_max) = vec3_range_from_initializers(&config.initializer, "color")
            .unwrap_or(([255.0; 3], [255.0; 3]));

        let gravity = config
            .operator
            .iter()
            .find(|op| op.name.contains("movement"))
            .and_then(|op| op.gravity.as_deref())
            .map(|s| parse_f32_vec3(Some(s)))
            .map(|v| [v[0], v[1]])
            .unwrap_or([0.0, 0.0]);

        let color_override = overrides
            .and_then(|o| o.color.as_deref())
            .map(|s| parse_f32_vec3(Some(s)))
            .map(|v| [v[0] as u8, v[1] as u8, v[2] as u8]);

        let alphafade = config
            .operator
            .iter()
            .find(|op| op.name.contains("alphafade"))
            .map(|op| {
                (
                    op.fadeintime.unwrap_or(0.0) as f32,
                    op.fadeouttime.unwrap_or(1.0) as f32,
                )
            });
        let fade_range_op = |name: &str| -> Option<(f32, f32, f32, f32)> {
            config
                .operator
                .iter()
                .find(|op| op.name.contains(name))
                .map(|op| {
                    (
                        op.starttime.unwrap_or(0.0) as f32,
                        op.endtime.unwrap_or(1.0) as f32,
                        op.startvalue.unwrap_or(0.0) as f32,
                        op.endvalue.unwrap_or(1.0) as f32,
                    )
                })
        };
        let sizechange = fade_range_op("sizechange");
        let alphachange = fade_range_op("alphachange");
        let oscillate_op = |name: &str| -> Option<OscillateParams> {
            config
                .operator
                .iter()
                .find(|op| op.name.contains(name))
                .map(OscillateParams::from_operator)
        };
        let oscillate_alpha = oscillate_op("oscillatealpha");
        let oscillate_size = oscillate_op("oscillatesize");
        let oscillate_position = oscillate_op("oscillateposition");

        Self {
            particles: Vec::with_capacity(max_count),
            emitters,
            max_count,
            life_min,
            life_max,
            size_min,
            size_max,
            velocity_min,
            velocity_max,
            color_min,
            color_max,
            gravity,
            alphafade,
            sizechange,
            alphachange,
            oscillate_alpha,
            oscillate_size,
            oscillate_position,
            alpha_mult: overrides.and_then(|o| o.alpha).unwrap_or(1.0) as f32,
            size_mult: overrides.and_then(|o| o.size).unwrap_or(1.0) as f32,
            speed_mult: overrides.and_then(|o| o.speed).unwrap_or(1.0) as f32,
            rate_mult: overrides.and_then(|o| o.rate).unwrap_or(1.0) as f32,
            color_override,
        }
    }

    pub fn step(&mut self, dt: f32) {
        for p in &mut self.particles {
            p.vx += self.gravity[0] * dt;
            p.vy += self.gravity[1] * dt;

            // Position-oscillate integrates a velocity (derivative of
            // scale*cos(w*t+phase)) into position each frame, matching
            // CParticle.cpp's createOscillatePositionOperator exactly.
            if let Some(osc) = &p.osc_pos {
                let age = p.max_life - p.life;
                for axis in 0..2 {
                    let w = osc.frequency[axis];
                    let d = -osc.scale[axis] * w * (w * age + osc.phase[axis]).sin() * dt;
                    if axis == 0 {
                        p.x += d;
                    } else {
                        p.y += d;
                    }
                }
            }

            p.x += p.vx * dt;
            p.y += p.vy * dt;
            p.life -= dt;

            let age = p.max_life - p.life;
            let lifetime_pos = if p.max_life > 0.0 {
                (age / p.max_life).clamp(0.0, 1.0)
            } else {
                1.0
            };

            // Alpha: alphafade takes precedence (its very common fade-in/hold/
            // fade-out shape), then a generic alphachange ramp, else fall back
            // to a plain linear death-fade so particles don't hard-cut when a
            // config has no alpha-affecting operator at all.
            let mut alpha = if let Some((fade_in, fade_out)) = self.alphafade {
                let fade = if lifetime_pos <= fade_in {
                    fade_value(lifetime_pos, 0.0, fade_in, 0.0, 1.0)
                } else if lifetime_pos > fade_out {
                    1.0 - fade_value(lifetime_pos, fade_out, 1.0, 0.0, 1.0)
                } else {
                    1.0
                };
                p.initial_alpha * fade
            } else if let Some((start, end, sv, ev)) = self.alphachange {
                p.initial_alpha * fade_value(lifetime_pos, start, end, sv, ev)
            } else {
                p.initial_alpha * (p.life / p.max_life).max(0.0)
            };

            let mut size = if let Some((start, end, sv, ev)) = self.sizechange {
                p.initial_size * fade_value(lifetime_pos, start, end, sv, ev)
            } else {
                p.size
            };

            if let Some(osc) = &p.osc_alpha {
                let cos_val = ((osc.frequency * age + osc.phase).cos() + 1.0) * 0.5;
                let mult = self
                    .oscillate_alpha
                    .map(|o| o.scale_min + (o.scale_max - o.scale_min) * cos_val)
                    .unwrap_or(1.0);
                alpha *= mult;
            }
            if let Some(osc) = &p.osc_size {
                let cos_val = ((osc.frequency * age + osc.phase).cos() + 1.0) * 0.5;
                let mult = self
                    .oscillate_size
                    .map(|o| o.scale_min + (o.scale_max - o.scale_min) * cos_val)
                    .unwrap_or(1.0);
                size *= mult;
            }

            p.alpha = alpha * self.alpha_mult;
            p.size = size;
        }

        self.particles.retain(|p| p.life > 0.0);

        for emitter in &mut self.emitters {
            emitter.accumulator += emitter.rate * self.rate_mult * dt;
            while emitter.accumulator >= 1.0 && self.particles.len() < self.max_count {
                emitter.accumulator -= 1.0;
                let life =
                    self.life_min + fastrand::f32() * (self.life_max - self.life_min).max(0.0);
                // Reference halves the sizerandom range (CParticle.cpp: `size = (min +
                // t*(max-min)) * override / 2.0`) — the initializer's value is a
                // diameter, not a radius.
                let size = (self.size_min
                    + fastrand::f32() * (self.size_max - self.size_min).max(0.0))
                    * self.size_mult
                    / 2.0;

                let ox = emitter.origin[0];
                let oy = emitter.origin[1];

                let (vx, vy) = if let (Some(min), Some(max)) =
                    (self.velocity_min, self.velocity_max)
                {
                    (
                        (min[0] + fastrand::f32() * (max[0] - min[0])) * self.speed_mult,
                        (min[1] + fastrand::f32() * (max[1] - min[1])) * self.speed_mult,
                    )
                } else {
                    let speed = (emitter.speed_min
                        + fastrand::f32() * (emitter.speed_max - emitter.speed_min))
                        * self.speed_mult;
                    let angle = fastrand::f32() * std::f32::consts::TAU;
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
                    let id_phase = emitter.id as f32 * 0.173;
                    (
                        (angle + id_phase).cos() * speed * emitter.directions[0].max(0.1) * sign_x,
                        (angle + id_phase).sin() * speed * emitter.directions[1].max(0.1) * sign_y,
                    )
                };

                let (spread_x, spread_y) = (emitter.spread[0], emitter.spread[1]);

                let color = self.color_override.unwrap_or_else(|| {
                    [
                        lerp_u8(self.color_min[0], self.color_max[0]),
                        lerp_u8(self.color_min[1], self.color_max[1]),
                        lerp_u8(self.color_min[2], self.color_max[2]),
                    ]
                });

                self.particles.push(Particle {
                    x: ox + (fastrand::f32() - 0.5) * spread_x,
                    y: oy + (fastrand::f32() - 0.5) * spread_y,
                    vx,
                    vy,
                    life,
                    max_life: life,
                    size,
                    alpha: self.alpha_mult,
                    color,
                    initial_alpha: self.alpha_mult,
                    initial_size: size,
                    osc_alpha: self.oscillate_alpha.map(|o| o.sample()),
                    osc_size: self.oscillate_size.map(|o| o.sample()),
                    osc_pos: self.oscillate_position.map(|o| {
                        let a = o.sample();
                        let b = o.sample();
                        OscillatorPos {
                            frequency: [a.frequency, b.frequency],
                            scale: [a.scale, b.scale],
                            phase: [a.phase, b.phase],
                        }
                    }),
                });
            }
        }
    }

    pub fn render_onto(&self, canvas: &mut RgbaImage) {
        let w = canvas.width() as i32;
        let h = canvas.height() as i32;

        for p in &self.particles {
            let alpha = (p.alpha * 180.0).clamp(0.0, 255.0) as u8;
            if alpha == 0 {
                continue;
            }
            let sz = p.size as i32;
            if sz <= 0 {
                continue;
            }
            let cx = p.x as i32;
            let cy = p.y as i32;

            let sz2 = (sz * sz) as f32;
            for dy in -sz..=sz {
                for dx in -sz..=sz {
                    let d2 = (dx * dx + dy * dy) as f32;
                    if d2 > sz2 {
                        continue;
                    }
                    let px = cx + dx;
                    let py = cy + dy;
                    if px < 0 || py < 0 || px >= w || py >= h {
                        continue;
                    }
                    // Soft radial falloff — a flat-alpha disc reads as a harsh,
                    // hard-edged blob, especially for large ambient sprites
                    // (fog/light shafts) that are meant to be soft glows.
                    let t = 1.0 - (d2 / sz2).sqrt();
                    let falloff = t * t;
                    let dst = canvas.get_pixel_mut(px as u32, py as u32);
                    // Standard "over" compositing (not a plain lerp): the
                    // destination isn't always opaque — the GPU path renders
                    // particles into their own transparent scene-sized buffer
                    // before compositing it as a texture, so overlapping
                    // particles must accumulate alpha correctly rather than
                    // assuming a fully-opaque backdrop.
                    let src_a = (alpha as f32 / 255.0) * falloff;
                    let dst_a = dst[3] as f32 / 255.0;
                    let out_a = src_a + dst_a * (1.0 - src_a);
                    if out_a > 0.0 {
                        for i in 0..3 {
                            let src_c = p.color[i] as f32 / 255.0;
                            let dst_c = dst[i] as f32 / 255.0;
                            let out_c = (src_c * src_a + dst_c * dst_a * (1.0 - src_a)) / out_a;
                            dst[i] = (out_c * 255.0).clamp(0.0, 255.0) as u8;
                        }
                    }
                    dst[3] = (out_a * 255.0).clamp(0.0, 255.0) as u8;
                }
            }
        }
    }
}

fn lerp_u8(min: f32, max: f32) -> u8 {
    (min + fastrand::f32() * (max - min)).clamp(0.0, 255.0) as u8
}

/// Linear ramp from `start_val` to `end_val` over `[start, end]` of `t`,
/// clamped flat outside that range. Matches `Maths::fadeValue` exactly.
fn fade_value(t: f32, start: f32, end: f32, start_val: f32, end_val: f32) -> f32 {
    if t <= start {
        start_val
    } else if t >= end {
        end_val
    } else {
        let f = (t - start) / (end - start);
        start_val + f * (end_val - start_val)
    }
}

fn scalar_range_from_initializers(
    initializers: &[Initializer],
    needle: &str,
    default_min: f32,
    default_max: f32,
) -> (f32, f32) {
    let Some(init) = initializers.iter().find(|init| init.name.contains(needle)) else {
        return (default_min, default_max);
    };
    (
        init.min
            .as_ref()
            .and_then(value_as_f32)
            .unwrap_or(default_min),
        init.max
            .as_ref()
            .and_then(value_as_f32)
            .unwrap_or(default_max),
    )
}

/// Returns `Some((min, max))` when the named initializer exists and both
/// bounds parse as a 3-component vector (e.g. `velocityrandom`/`colorrandom`).
fn vec3_range_from_initializers(
    initializers: &[Initializer],
    needle: &str,
) -> Option<([f32; 3], [f32; 3])> {
    let init = initializers
        .iter()
        .find(|init| init.name.contains(needle))?;
    let min = init.min.as_ref().and_then(value_as_vec3)?;
    let max = init.max.as_ref().and_then(value_as_vec3)?;
    Some((min, max))
}

fn value_as_f32(v: &serde_json::Value) -> Option<f32> {
    v.as_f64().map(|f| f as f32)
}

fn value_as_vec3(v: &serde_json::Value) -> Option<[f32; 3]> {
    match v {
        serde_json::Value::String(s) => Some(parse_f32_vec3(Some(s))),
        serde_json::Value::Array(arr) => {
            let parts: Vec<f32> = arr
                .iter()
                .filter_map(|x| x.as_f64().map(|f| f as f32))
                .collect();
            (parts.len() >= 3).then(|| [parts[0], parts[1], parts[2]])
        }
        _ => None,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Real preset fields (velocityrandom, colorrandom, angularvelocityrandom)
    /// use `"x y z"` vector strings for min/max, not numbers — the old
    /// `Option<f64>` typing rejected these outright.
    #[test]
    fn parses_real_preset_with_vector_initializers() {
        let json = r#"{
            "maxcount": 20,
            "emitter": [{"name":"sphererandom","rate":1.5,"distancemin":0,"distancemax":512}],
            "initializer": [
                {"id":2,"name":"lifetimerandom","min":3,"max":5},
                {"id":3,"name":"sizerandom","min":1000,"max":2200},
                {"id":4,"name":"velocityrandom","min":"25 0 0","max":"80 0 0"},
                {"id":5,"name":"colorrandom","min":"255 255 255","max":"255 255 255"}
            ],
            "operator": [{"id":9,"name":"movement","gravity":"0 0 0"}],
            "renderer": [{"id":1,"name":"sprite"}]
        }"#;
        let config: ParticleConfig = serde_json::from_str(json).expect("should parse");
        let sys = ParticleSystem::from_config(&config, [100.0, 200.0], None);
        assert_eq!(sys.velocity_min, Some([25.0, 0.0, 0.0]));
        assert_eq!(sys.velocity_max, Some([80.0, 0.0, 0.0]));
        assert_eq!(sys.color_min, [255.0, 255.0, 255.0]);
    }

    #[test]
    fn scalar_initializer_without_vector_fields_still_parses() {
        let json = r#"{
            "maxcount": 16,
            "emitter": [{"name":"box","rate":1}],
            "initializer": [{"id":2,"name":"lifetimerandom","min":1,"max":1}]
        }"#;
        let config: ParticleConfig = serde_json::from_str(json).expect("should parse");
        let sys = ParticleSystem::from_config(&config, [0.0, 0.0], None);
        assert_eq!(sys.life_min, 1.0);
        assert_eq!(sys.life_max, 1.0);
        assert!(sys.velocity_min.is_none());
    }

    #[test]
    fn instance_override_sets_color_and_multipliers() {
        let json = r#"{
            "maxcount": 5,
            "emitter": [{"name":"box","rate":1}]
        }"#;
        let config: ParticleConfig = serde_json::from_str(json).expect("should parse");
        let overrides = InstanceOverride {
            alpha: Some(0.5),
            color: Some("192 192 192".to_string()),
            rate: Some(2.0),
            size: None,
            speed: None,
        };
        let sys = ParticleSystem::from_config(&config, [0.0, 0.0], Some(&overrides));
        assert_eq!(sys.color_override, Some([192, 192, 192]));
        assert_eq!(sys.alpha_mult, 0.5);
        assert_eq!(sys.rate_mult, 2.0);
    }

    #[test]
    fn emitter_origin_is_local_offset_from_spawn_center() {
        let json = r#"{
            "maxcount": 5,
            "emitter": [{"name":"box","rate":1,"origin":"10 20 0"}]
        }"#;
        let config: ParticleConfig = serde_json::from_str(json).expect("should parse");
        let sys = ParticleSystem::from_config(&config, [100.0, 200.0], None);
        assert_eq!(sys.emitters[0].origin[0], 110.0);
        assert_eq!(sys.emitters[0].origin[1], 220.0);
    }

    #[test]
    fn movement_operator_gravity_field_parses() {
        let json = r#"{
            "maxcount": 5,
            "emitter": [{"name":"box","rate":1}],
            "operator": [{"id":1,"name":"movement","gravity":"0 -50 0"}]
        }"#;
        let config: ParticleConfig = serde_json::from_str(json).expect("should parse");
        let sys = ParticleSystem::from_config(&config, [0.0, 0.0], None);
        assert_eq!(sys.gravity, [0.0, -50.0]);
    }

    #[test]
    fn fade_value_clamps_outside_range_and_lerps_inside() {
        assert_eq!(fade_value(0.0, 0.3, 0.7, 0.0, 1.0), 0.0);
        assert_eq!(fade_value(0.3, 0.3, 0.7, 0.0, 1.0), 0.0);
        assert_eq!(fade_value(0.5, 0.3, 0.7, 0.0, 1.0), 0.5);
        assert_eq!(fade_value(0.7, 0.3, 0.7, 0.0, 1.0), 1.0);
        assert_eq!(fade_value(1.0, 0.3, 0.7, 0.0, 1.0), 1.0);
    }

    /// A real `alphafade` preset (fog1.json): fadeintime=0.5 means alpha
    /// should be 0 at spawn, ~full partway through the ramp, and full for
    /// the rest of the particle's life (no fadeouttime = holds at 1.0).
    #[test]
    fn alphafade_ramps_in_then_holds() {
        let json = r#"{
            "maxcount": 5,
            "emitter": [{"name":"box","rate":1}],
            "initializer": [{"id":1,"name":"alpharandom","min":1,"max":1}],
            "operator": [{"id":1,"name":"alphafade","fadeintime":0.5}]
        }"#;
        let config: ParticleConfig = serde_json::from_str(json).expect("should parse");
        let mut sys = ParticleSystem::from_config(&config, [0.0, 0.0], None);
        // Force-spawn one particle with a known, fixed lifetime.
        sys.life_min = 10.0;
        sys.life_max = 10.0;
        sys.emitters[0].rate = 1000.0; // spawn immediately
        sys.step(0.001); // this step only spawns; the fade math runs on existing particles first
        assert_eq!(sys.particles.len(), 1);

        // Just after spawn (age≈0), fully faded in from 0.
        sys.step(0.001);
        assert!(sys.particles[0].alpha < 0.1);

        // Halfway through the fade-in window (age=2.5s of 10s life → lifetime_pos=0.25,
        // half of fadeintime=0.5): alpha should be roughly half.
        sys.step(2.499);
        let mid = sys.particles[0].alpha;
        assert!((0.3..0.7).contains(&mid), "expected ~0.5, got {mid}");

        // Well past the fade-in window, alpha should be at full (no fadeouttime set → holds at 1.0).
        sys.step(5.0);
        assert!(sys.particles[0].alpha > 0.9);
    }

    /// A real `sizechange` preset (magic_pulse.json): startvalue=0, endvalue=2,
    /// endtime=1 (normalized) — size should start near 0 and grow past the
    /// initializer-sampled size as lifetime_pos approaches endtime.
    #[test]
    fn sizechange_scales_initial_size_over_lifetime() {
        let json = r#"{
            "maxcount": 5,
            "emitter": [{"name":"box","rate":1}],
            "initializer": [{"id":1,"name":"sizerandom","min":100,"max":100}],
            "operator": [{"id":1,"name":"sizechange","startvalue":0,"endvalue":2,"endtime":1}]
        }"#;
        let config: ParticleConfig = serde_json::from_str(json).expect("should parse");
        let mut sys = ParticleSystem::from_config(&config, [0.0, 0.0], None);
        sys.life_min = 10.0;
        sys.life_max = 10.0;
        sys.emitters[0].rate = 1000.0;
        sys.step(0.001); // this step only spawns; the fade math runs on existing particles first
        assert_eq!(sys.particles.len(), 1);
        let initial_size = sys.particles[0].initial_size;
        sys.step(0.001);
        assert!(sys.particles[0].size < initial_size * 0.1);

        // lifetime_pos=1.0 reached at age=10s (endtime=1 normalized == full life here).
        sys.step(9.99);
        assert!((sys.particles[0].size - initial_size * 2.0).abs() < initial_size * 0.1);
    }

    #[test]
    fn oscillate_params_defaults_scale_min_from_scale_max_when_absent() {
        let op = Operator {
            name: "oscillatealpha".to_string(),
            gravity: None,
            fadeintime: None,
            fadeouttime: None,
            starttime: None,
            endtime: None,
            startvalue: None,
            endvalue: None,
            frequencymin: Some(0.5),
            frequencymax: Some(1.0),
            scalemin: None,
            scalemax: Some(10.0),
            phasemin: None,
            phasemax: None,
        };
        let params = OscillateParams::from_operator(&op);
        assert_eq!(params.scale_min, 10.0);
        assert_eq!(params.scale_max, 10.0);
        assert_eq!(params.freq_min, 0.5);
        assert_eq!(params.freq_max, 1.0);
    }

    #[test]
    fn oscillate_sample_phase_covers_full_circle_when_unspecified() {
        let op = Operator {
            name: "oscillatealpha".to_string(),
            gravity: None,
            fadeintime: None,
            fadeouttime: None,
            starttime: None,
            endtime: None,
            startvalue: None,
            endvalue: None,
            frequencymin: Some(1.0),
            frequencymax: Some(1.0),
            scalemin: Some(1.0),
            scalemax: Some(1.0),
            phasemin: None,
            phasemax: None,
        };
        let params = OscillateParams::from_operator(&op);
        for _ in 0..50 {
            let s = params.sample();
            assert!((0.0..std::f32::consts::TAU).contains(&s.phase));
        }
    }
}
