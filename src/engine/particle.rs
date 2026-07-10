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
    /// Real JSON key is singular `"controlpoint"` (an array), referenced by
    /// index (not `id`-keyed lookup at use-site) from operators/emitters like
    /// `controlpointattract` (CParticle.cpp/ObjectParser.cpp).
    #[serde(default)]
    pub controlpoint: Vec<ControlPointConfig>,
    /// Path to the material JSON providing this system's sprite texture
    /// (e.g. `"materials/presets/water_faucet.json"`). Resolved by callers
    /// (this module stays asset-agnostic) and passed into `render_onto`.
    #[serde(default)]
    pub material: Option<String>,
    /// How a particle's sprite-sheet frame advances over time: `"once"`
    /// (play through over the particle's lifetime), `"randomframe"` (pick
    /// one fixed random frame at spawn), or anything else/absent (loop
    /// continuously) — CParticle.cpp's animation-frame update.
    #[serde(default)]
    pub animationmode: Option<String>,
    /// Playback speed multiplier for sprite-sheet animation; `<= 0` (or
    /// absent) means 1.0, matching the reference's `sequenceMultiplier > 0
    /// ? sequenceMultiplier : 1.0`.
    #[serde(default)]
    pub sequencemultiplier: Option<f64>,
}

/// One resolved particle sprite: every sprite-sheet frame (already sliced
/// out of the `.tex` atlas by `TexFile::to_rgba_frames`) plus the total
/// loop duration in seconds (sum of each frame's `frametime`, 0 for a
/// single-frame/non-animated sprite).
#[derive(Clone)]
pub struct ParticleSprite {
    pub frames: Vec<RgbaImage>,
    pub duration: f32,
}

impl ParticleSprite {
    pub fn single(image: RgbaImage) -> Self {
        Self {
            frames: vec![image],
            duration: 0.0,
        }
    }
}

/// Parsed `animationmode` (defaults to `Loop` for anything unrecognized or
/// absent, matching the reference's `else` fallback).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
enum AnimationMode {
    #[default]
    Loop,
    Once,
    RandomFrame,
}

impl AnimationMode {
    fn from_config(mode: Option<&str>) -> Self {
        match mode {
            Some("once") => Self::Once,
            Some("randomframe") => Self::RandomFrame,
            _ => Self::Loop,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ControlPointConfig {
    #[serde(default)]
    pub id: Option<i64>,
    #[serde(default)]
    pub offset: Option<serde_json::Value>,
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
    /// `turbulentvelocityrandom` (CParticle.cpp
    /// createTurbulentVelocityRandomInitializer): a curl-noise-directed
    /// spawn velocity. `forward`/`right` are `"x y z"` vector strings;
    /// `offset` here is the scalar tilt angle around `right`, unrelated to
    /// an emitter's positional offset. `speedmin`/`speedmax` are untyped
    /// because turbulentvelocityrandom uses plain numbers but
    /// `mapsequencearoundcontrolpoint` uses `"x y z"` vector strings.
    #[serde(default)]
    pub speedmin: Option<serde_json::Value>,
    #[serde(default)]
    pub speedmax: Option<serde_json::Value>,
    /// `mapsequencearoundcontrolpoint`: spawn the Nth particle at the
    /// control point, launched at angle `(N % count) / count * 2pi`.
    #[serde(default)]
    pub controlpoint: Option<i64>,
    #[serde(default)]
    pub count: Option<f64>,
    #[serde(default)]
    pub offset: Option<f64>,
    #[serde(default)]
    pub scale: Option<f64>,
    #[serde(default)]
    pub forward: Option<serde_json::Value>,
    #[serde(default)]
    pub timescale: Option<f64>,
    #[serde(default)]
    pub phasemin: Option<f64>,
    #[serde(default)]
    pub phasemax: Option<f64>,
    #[serde(default)]
    pub right: Option<serde_json::Value>,
}

#[derive(Debug, Default, Deserialize)]
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
    /// (CParticle.cpp's generic `fadeValue`). `startvalue`/`endvalue` are
    /// untyped because `sizechange`/`alphachange` use a plain number but
    /// `colorchange` uses an `"r g b"` vector string (e.g. `"1 0.75 0"`).
    #[serde(default)]
    pub starttime: Option<f64>,
    #[serde(default)]
    pub endtime: Option<f64>,
    #[serde(default)]
    pub startvalue: Option<serde_json::Value>,
    #[serde(default)]
    pub endvalue: Option<serde_json::Value>,
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
    /// `controlpointattract`: pulls particles toward `controlpoint` (index
    /// into the particle config's `controlpoint` array) whenever they're
    /// within `threshold/2` of it (CParticle.cpp createControlPointAttractOperator).
    #[serde(default)]
    pub controlpoint: Option<i64>,
    #[serde(default)]
    pub origin: Option<serde_json::Value>,
    #[serde(default)]
    pub scale: Option<f64>,
    #[serde(default)]
    pub threshold: Option<f64>,
    /// `angularmovement`: integrates `rotation.z` from `angularvelocityrandom`'s
    /// sampled `angular_velocity`, itself accelerated by `force.z` and decayed
    /// by `drag` each step (CParticle.cpp createAngularMovementOperator).
    #[serde(default)]
    pub drag: Option<f64>,
    #[serde(default)]
    pub force: Option<serde_json::Value>,
    /// `turbulence` (CParticle.cpp createTurbulenceOperator): curl-noise
    /// acceleration. Shares `scale`/`phasemin`/`phasemax` above.
    #[serde(default)]
    pub speedmin: Option<f64>,
    #[serde(default)]
    pub speedmax: Option<f64>,
    #[serde(default)]
    pub timescale: Option<f64>,
    #[serde(default)]
    pub mask: Option<serde_json::Value>,
    /// `vortex` (CParticle.cpp createVortexOperator): tangential spin around
    /// a control point. `flags`: 1 = infinite axis, 2 = maintain distance,
    /// 4 = ring shape.
    #[serde(default)]
    pub flags: Option<serde_json::Value>,
    #[serde(default)]
    pub axis: Option<serde_json::Value>,
    #[serde(default)]
    pub offset: Option<serde_json::Value>,
    #[serde(default)]
    pub distanceinner: Option<f64>,
    #[serde(default)]
    pub distanceouter: Option<f64>,
    #[serde(default)]
    pub speedinner: Option<f64>,
    #[serde(default)]
    pub speedouter: Option<f64>,
    #[serde(default)]
    pub centerforce: Option<f64>,
    #[serde(default)]
    pub ringradius: Option<f64>,
    #[serde(default)]
    pub ringwidth: Option<f64>,
    #[serde(default)]
    pub ringpulldistance: Option<f64>,
    #[serde(default)]
    pub ringpullforce: Option<f64>,
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
    /// Alpha/size/color as set by the initializers at spawn — `alphafade`/
    /// `sizechange`/`colorchange` scale *this*, not the live (already-modified)
    /// value. `initial_color` is 0..1 (matching `colorchange`'s multiplier).
    initial_alpha: f32,
    initial_size: f32,
    initial_color: [f32; 3],
    osc_alpha: Option<Oscillator1D>,
    osc_size: Option<Oscillator1D>,
    osc_pos: Option<OscillatorPos>,
    /// Screen-space (Z-axis) rotation angle, radians. Only visually relevant
    /// once a sprite texture is drawn (`render_onto`'s textured path) — a
    /// flat-color circle is rotation-invariant.
    rotation: f32,
    angular_velocity: f32,
    /// Current sprite-sheet frame (fractional, truncated at draw time);
    /// `-1.0` at spawn means "not yet assigned" (only meaningful for
    /// `AnimationMode::RandomFrame`, which assigns it once and freezes it).
    frame: f32,
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
    /// `alpharandom` initializer range (CParticle.cpp's `AlphaRandomInitializer`):
    /// defaults to `(1.0, 1.0)` — a no-op — when the config has no such
    /// initializer at all, matching `ParticleInstance::alpha`'s `1.0f` default.
    alpha_min: f32,
    alpha_max: f32,
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
    /// `colorchange`: (starttime, endtime, start_color, end_color), colors in
    /// 0..1 (unlike `colorrandom`'s 0..255 — matches the real presets, e.g.
    /// `"startvalue": "1 0.75 0"`).
    colorchange: Option<(f32, f32, [f32; 3], [f32; 3])>,
    oscillate_alpha: Option<OscillateParams>,
    oscillate_size: Option<OscillateParams>,
    oscillate_position: Option<OscillateParams>,
    alpha_mult: f32,
    size_mult: f32,
    speed_mult: f32,
    rate_mult: f32,
    color_override: Option<[u8; 3]>,
    /// Resolved once at construction: `spawn_center + offset` per control
    /// point, indexed positionally (falls back to declaration order when a
    /// config omits `id`). Nested parent-space transforms and mouse-linked
    /// (`locktopointer`)/world-space (`flags & 2`) control points are not
    /// modeled — same simplification already accepted for emitter origins.
    control_points: Vec<[f32; 3]>,
    /// `controlpointattract`: (control point index, origin offset, scale, threshold).
    control_point_attract: Option<(usize, [f32; 3], f32, f32)>,
    /// True when `config.renderer` names a `"rope"`/`"ropetrail"` renderer —
    /// draws a connected ribbon through living particles instead of
    /// independent circles (CParticle::renderRope).
    rope_mode: bool,
    rope_subdivision: usize,
    /// `angularvelocityrandom` initializer range (z-axis component only).
    angular_velocity_min: f32,
    angular_velocity_max: f32,
    /// `angularmovement` operator (z-axis `force` component, plus `drag`).
    angular_force: f32,
    angular_drag: f32,
    /// Sprite-sheet animation: how many frames the resolved sprite has (0 if
    /// no sprite, or a plain single-frame one) and its total loop duration
    /// in seconds — set post-construction via `set_sprite_frames` once the
    /// caller has actually resolved the sprite texture (this module stays
    /// asset-agnostic, same convention as `material`/`render_onto`).
    sprite_frame_count: usize,
    sprite_duration: f32,
    animation_mode: AnimationMode,
    sequence_multiplier: f32,
    /// `rotationrandom` initializer range (z-axis component, radians) —
    /// `None` leaves spawn rotation at 0, matching the reference's absent-
    /// initializer default.
    rotation_range: Option<(f32, f32)>,
    /// `spritetrail` renderer parameters.
    sprite_trail: Option<SpriteTrailParams>,
    /// `mapsequencearoundcontrolpoint` initializer + its running sequence
    /// counter (shared across spawns, wraps at `count`).
    map_sequence: Option<MapSequenceParams>,
    map_sequence_index: u32,
    /// `turbulentvelocityrandom` initializer parameters.
    turbulent_velocity: Option<TurbulentVelocityParams>,
    /// `turbulence` operator parameters.
    turbulence: Option<TurbulenceParams>,
    /// `vortex` operator parameters.
    vortex: Option<VortexParams>,
    /// Simulation clock (sum of `step` dts) — drives the time-scrolled
    /// noise fields, the reference's `m_time`/`currentTime`.
    time: f32,
}

/// `turbulentvelocityrandom` (CParticle.cpp): spawn velocity picked from a
/// curl-noise field, angle-limited around `forward`.
#[derive(Clone, Copy)]
struct TurbulentVelocityParams {
    speed_min: f32,
    speed_max: f32,
    /// Tilt angle (radians) around `right`.
    offset: f32,
    /// Direction cone: `< 2` limits deviation from `forward` to
    /// `scale/2 * pi`.
    scale: f32,
    forward: [f32; 3],
    right: [f32; 3],
    timescale: f32,
    phase_min: f32,
    phase_max: f32,
}

/// `turbulence` operator (CParticle.cpp): curl-noise acceleration. `phase`
/// and `speed` are drawn once per operator instance, not per particle.
#[derive(Clone, Copy)]
struct TurbulenceParams {
    noise_scale: f32,
    timescale: f32,
    mask: [f32; 3],
    phase: f32,
    speed: f32,
}

/// `spritetrail` renderer: velocity-aligned, velocity-stretched sprite quad.
#[derive(Clone, Copy)]
struct SpriteTrailParams {
    length: f32,
    max_length: f32,
    min_length: f32,
}

/// `mapsequencearoundcontrolpoint` initializer (CParticle.cpp): spawns each
/// particle at the control point with a velocity rotated by the next angle
/// in an evenly-divided circle (`sequence / count * 2pi`).
#[derive(Clone, Copy)]
struct MapSequenceParams {
    control_point: usize,
    count: u32,
    speed_min: [f32; 3],
    speed_max: [f32; 3],
}

/// `vortex` operator (CParticle.cpp): tangential spin around a control
/// point, with optional ring shape and center attraction.
#[derive(Clone, Copy)]
struct VortexParams {
    control_point: i64,
    infinite_axis: bool,
    maintain_distance: bool,
    ring_shape: bool,
    axis: [f32; 3],
    offset: [f32; 3],
    distance_inner: f32,
    distance_outer: f32,
    speed_inner: f32,
    speed_outer: f32,
    center_force: f32,
    ring_radius: f32,
    ring_width: f32,
    ring_pull_distance: f32,
    ring_pull_force: f32,
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
                let is_sphere = e.name == "sphererandom";

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
            scalar_range_from_initializers(&config.initializer, "lifetimerandom", 2.0, 6.0);
        let (size_min, size_max) =
            scalar_range_from_initializers(&config.initializer, "sizerandom", 2.0, 6.0);
        let (alpha_min, alpha_max) =
            scalar_range_from_initializers(&config.initializer, "alpharandom", 1.0, 1.0);
        let velocity_range = vec3_range_from_initializers(&config.initializer, "velocityrandom");
        let (velocity_min, velocity_max) = match velocity_range {
            Some((min, max)) => (Some(min), Some(max)),
            None => (None, None),
        };
        let (color_min, color_max) =
            vec3_range_from_initializers(&config.initializer, "colorrandom")
                .unwrap_or(([255.0; 3], [255.0; 3]));

        let gravity = config
            .operator
            .iter()
            .find(|op| op.name == "movement")
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
            .find(|op| op.name == "alphafade")
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
                .find(|op| op.name == name)
                .map(|op| {
                    (
                        op.starttime.unwrap_or(0.0) as f32,
                        op.endtime.unwrap_or(1.0) as f32,
                        op.startvalue.as_ref().and_then(value_as_f32).unwrap_or(0.0),
                        op.endvalue.as_ref().and_then(value_as_f32).unwrap_or(1.0),
                    )
                })
        };
        let sizechange = fade_range_op("sizechange");
        let alphachange = fade_range_op("alphachange");
        let colorchange = config
            .operator
            .iter()
            .find(|op| op.name == "colorchange")
            .map(|op| {
                (
                    op.starttime.unwrap_or(0.0) as f32,
                    op.endtime.unwrap_or(1.0) as f32,
                    op.startvalue
                        .as_ref()
                        .and_then(value_as_vec3)
                        .unwrap_or([1.0; 3]),
                    op.endvalue
                        .as_ref()
                        .and_then(value_as_vec3)
                        .unwrap_or([1.0; 3]),
                )
            });
        let oscillate_op = |name: &str| -> Option<OscillateParams> {
            config
                .operator
                .iter()
                .find(|op| op.name == name)
                .map(OscillateParams::from_operator)
        };
        let oscillate_alpha = oscillate_op("oscillatealpha");
        let oscillate_size = oscillate_op("oscillatesize");
        let oscillate_position = oscillate_op("oscillateposition");

        // Positional indexing (declaration order), not `id`-keyed slot
        // placement — real presets declare control points in id order, and
        // consumers (e.g. `controlpointattract`) reference them by a plain
        // array index anyway.
        let control_points: Vec<[f32; 3]> = config
            .controlpoint
            .iter()
            .map(|cp| {
                let offset = cp
                    .offset
                    .as_ref()
                    .and_then(value_as_vec3)
                    .unwrap_or([0.0; 3]);
                [
                    spawn_center[0] + offset[0],
                    spawn_center[1] + offset[1],
                    offset[2],
                ]
            })
            .collect();

        let control_point_attract = config
            .operator
            .iter()
            .find(|op| op.name == "controlpointattract")
            .map(|op| {
                let idx = op.controlpoint.unwrap_or(0).max(0) as usize;
                let origin = op
                    .origin
                    .as_ref()
                    .and_then(value_as_vec3)
                    .unwrap_or([0.0; 3]);
                let scale = op.scale.unwrap_or(100.0) as f32;
                let threshold = op.threshold.unwrap_or(1000.0) as f32;
                (idx, origin, scale, threshold)
            });

        let rope_renderer = config.renderer.iter().find(|r| {
            r.get("name")
                .and_then(|n| n.as_str())
                .map(|n| n == "rope" || n == "ropetrail")
                .unwrap_or(false)
        });
        let rope_mode = rope_renderer.is_some();
        let rope_subdivision = rope_renderer
            .and_then(|r| r.get("subdivision"))
            .and_then(|v| v.as_u64())
            .unwrap_or(1)
            .max(1) as usize;

        // `spritetrail` renderer: each particle draws as a quad stretched
        // along its velocity by `clamp(|v| * length, minlength, maxlength)`
        // (genericparticle.vert's TRAILRENDERER `ComputeParticleTrailTangents`;
        // parser defaults from ObjectParser::parseParticleRenderer).
        let sprite_trail = config
            .renderer
            .iter()
            .find(|r| {
                r.get("name").and_then(|n| n.as_str()) == Some("spritetrail")
            })
            .map(|r| {
                let f = |key: &str, default: f32| -> f32 {
                    r.get(key).and_then(|v| v.as_f64()).map(|v| v as f32).unwrap_or(default)
                };
                SpriteTrailParams {
                    length: f("length", 0.05),
                    max_length: f("maxlength", 10.0),
                    min_length: f("minlength", 0.0),
                }
            });

        // `angularvelocityrandom`'s min/max are vec3 (reference default
        // `(0,0,-5)`/`(0,0,5)`) — we only model the z-axis (screen-space)
        // component, matching how `rotation.z` is the only axis our flat
        // 2D renderer can express.
        let (angular_velocity_min, angular_velocity_max) = config
            .initializer
            .iter()
            .find(|init| init.name == "angularvelocityrandom")
            .map(|init| {
                let min = init.min.as_ref().and_then(value_as_vec3).unwrap_or([0.0, 0.0, -5.0]);
                let max = init.max.as_ref().and_then(value_as_vec3).unwrap_or([0.0, 0.0, 5.0]);
                (min[2], max[2])
            })
            .unwrap_or((0.0, 0.0));

        let angular_op = config
            .operator
            .iter()
            .find(|op| op.name == "angularmovement");
        let angular_drag = angular_op.and_then(|op| op.drag).unwrap_or(0.0) as f32;
        let angular_force = angular_op
            .and_then(|op| op.force.as_ref())
            .and_then(value_as_vec3)
            .map(|v| v[2])
            .unwrap_or(0.0);

        // `rotationrandom`: z component of the random vec3 (our renderer is
        // flat 2D, same reduction as `angularvelocityrandom`). Reference
        // defaults: min (0,0,0), max (0,0,2pi).
        let rotation_range = config
            .initializer
            .iter()
            .find(|init| init.name == "rotationrandom")
            .map(|init| {
                let min = init.min.as_ref().and_then(value_as_vec3).unwrap_or([0.0; 3]);
                let max = init
                    .max
                    .as_ref()
                    .and_then(value_as_vec3)
                    .unwrap_or([0.0, 0.0, std::f32::consts::TAU]);
                (min[2], max[2])
            });

        let turbulent_velocity = config
            .initializer
            .iter()
            .find(|init| init.name == "turbulentvelocityrandom")
            .map(|init| TurbulentVelocityParams {
                speed_min: init
                    .speedmin
                    .as_ref()
                    .and_then(value_as_f32)
                    .unwrap_or(100.0),
                speed_max: init
                    .speedmax
                    .as_ref()
                    .and_then(value_as_f32)
                    .unwrap_or(250.0),
                offset: init.offset.unwrap_or(0.0) as f32,
                scale: init.scale.unwrap_or(1.0) as f32,
                forward: init
                    .forward
                    .as_ref()
                    .and_then(value_as_vec3)
                    .unwrap_or([0.0, 1.0, 0.0]),
                timescale: init.timescale.unwrap_or(1.0) as f32,
                phase_min: init.phasemin.unwrap_or(0.0) as f32,
                phase_max: init.phasemax.unwrap_or(0.1) as f32,
                right: init
                    .right
                    .as_ref()
                    .and_then(value_as_vec3)
                    .unwrap_or([0.0, 0.0, 1.0]),
            });

        // `turbulence`: phase and speed are randomized once per operator
        // instance (the reference draws them in the factory, not per
        // particle or per frame).
        let turbulence = config
            .operator
            .iter()
            .find(|op| op.name == "turbulence")
            .map(|op| {
                let phase_min = op.phasemin.unwrap_or(0.0) as f32;
                let phase_max = op.phasemax.unwrap_or(0.0) as f32;
                let speed_min = op.speedmin.unwrap_or(500.0) as f32;
                let speed_max = op.speedmax.unwrap_or(1000.0) as f32;
                TurbulenceParams {
                    noise_scale: op.scale.unwrap_or(0.005) as f32 * 2.0,
                    timescale: op.timescale.unwrap_or(0.01) as f32,
                    mask: op
                        .mask
                        .as_ref()
                        .and_then(value_as_vec3)
                        .unwrap_or([1.0, 1.0, 0.0]),
                    phase: phase_min + fastrand::f32() * (phase_max - phase_min).max(0.0),
                    speed: speed_min + fastrand::f32() * (speed_max - speed_min).max(0.0),
                }
            });

        let map_sequence = config
            .initializer
            .iter()
            .find(|init| init.name == "mapsequencearoundcontrolpoint")
            .map(|init| MapSequenceParams {
                control_point: init.controlpoint.unwrap_or(0).max(0) as usize,
                count: (init.count.unwrap_or(1.0) as u32).max(1),
                speed_min: init
                    .speedmin
                    .as_ref()
                    .and_then(value_as_vec3)
                    .unwrap_or([0.0; 3]),
                speed_max: init
                    .speedmax
                    .as_ref()
                    .and_then(value_as_vec3)
                    .unwrap_or([100.0; 3]),
            });

        let vortex = config
            .operator
            .iter()
            .find(|op| op.name == "vortex" || op.name == "vortex_v2")
            .map(|op| {
                let flags = op.flags.as_ref().and_then(|v| v.as_i64()).unwrap_or(0);
                VortexParams {
                    control_point: op.controlpoint.unwrap_or(0),
                    infinite_axis: flags & 1 != 0,
                    maintain_distance: flags & 2 != 0,
                    ring_shape: flags & 4 != 0,
                    axis: op
                        .axis
                        .as_ref()
                        .and_then(value_as_vec3)
                        .unwrap_or([0.0, 0.0, 1.0]),
                    offset: op
                        .offset
                        .as_ref()
                        .and_then(value_as_vec3)
                        .unwrap_or([0.0; 3]),
                    distance_inner: op.distanceinner.unwrap_or(500.0) as f32,
                    distance_outer: op.distanceouter.unwrap_or(650.0) as f32,
                    speed_inner: op.speedinner.unwrap_or(2500.0) as f32,
                    speed_outer: op.speedouter.unwrap_or(0.0) as f32,
                    center_force: op.centerforce.unwrap_or(1.0) as f32,
                    ring_radius: op.ringradius.unwrap_or(300.0) as f32,
                    ring_width: op.ringwidth.unwrap_or(50.0) as f32,
                    ring_pull_distance: op.ringpulldistance.unwrap_or(50.0) as f32,
                    ring_pull_force: op.ringpullforce.unwrap_or(10.0) as f32,
                }
            });

        Self {
            particles: Vec::with_capacity(max_count),
            emitters,
            max_count,
            life_min,
            life_max,
            size_min,
            size_max,
            alpha_min,
            alpha_max,
            velocity_min,
            velocity_max,
            color_min,
            color_max,
            gravity,
            alphafade,
            sizechange,
            alphachange,
            colorchange,
            oscillate_alpha,
            oscillate_size,
            oscillate_position,
            alpha_mult: overrides.and_then(|o| o.alpha).unwrap_or(1.0) as f32,
            size_mult: overrides.and_then(|o| o.size).unwrap_or(1.0) as f32,
            speed_mult: overrides.and_then(|o| o.speed).unwrap_or(1.0) as f32,
            rate_mult: overrides.and_then(|o| o.rate).unwrap_or(1.0) as f32,
            color_override,
            control_points,
            control_point_attract,
            rope_mode,
            rope_subdivision,
            angular_velocity_min,
            angular_velocity_max,
            angular_force,
            angular_drag,
            sprite_frame_count: 0,
            sprite_duration: 0.0,
            animation_mode: AnimationMode::from_config(config.animationmode.as_deref()),
            sequence_multiplier: config
                .sequencemultiplier
                .map(|v| v as f32)
                .filter(|v| *v > 0.0)
                .unwrap_or(1.0),
            rotation_range,
            sprite_trail,
            map_sequence,
            map_sequence_index: 0,
            turbulent_velocity,
            turbulence,
            vortex,
            time: 0.0,
        }
    }

    /// Tell the system its resolved sprite's frame count/loop duration, so
    /// `step()` can advance each particle's `frame` — called once by the
    /// caller right after resolving the sprite (this module itself never
    /// loads textures). A no-op frame count of 0/1 leaves every particle's
    /// `frame` at its spawn sentinel, matching `render_onto`'s "always frame
    /// 0" behavior for non-animated sprites.
    pub fn set_sprite_frames(&mut self, frame_count: usize, duration: f32) {
        self.sprite_frame_count = frame_count;
        self.sprite_duration = duration;
    }

    pub fn step(&mut self, dt: f32) {
        self.time += dt;
        for p in &mut self.particles {
            p.vx += self.gravity[0] * dt;
            p.vy += self.gravity[1] * dt;

            // `turbulence` operator (CParticle.cpp createTurbulenceOperator):
            // normalized curl-noise direction, sampled at a time-scrolled,
            // scaled particle position, masked per axis, added to velocity.
            if let Some(t) = self.turbulence {
                if t.speed > 0.0001 {
                    let noise_pos = [
                        (p.x + t.phase + t.timescale * self.time) * t.noise_scale,
                        p.y * t.noise_scale,
                        0.0,
                    ];
                    let curl = crate::engine::noise::curl_noise(noise_pos);
                    let len = vec3_length(curl);
                    if len > 0.0001 {
                        let s = t.speed / len * dt * self.speed_mult;
                        p.vx += curl[0] * t.mask[0] * s;
                        p.vy += curl[1] * t.mask[1] * s;
                    }
                }
            }

            // `vortex` operator (CParticle.cpp createVortexOperator).
            if let Some(v) = self.vortex {
                let center_base = self
                    .control_points
                    .get(v.control_point.max(0) as usize)
                    .copied()
                    .unwrap_or([0.0; 3]);
                let center = [
                    center_base[0] + v.offset[0],
                    center_base[1] + v.offset[1],
                    center_base[2] + v.offset[2],
                ];
                let axis = vec3_normalize_or(v.axis, [0.0, 0.0, 1.0]);
                let to_particle = [p.x - center[0], p.y - center[1], -center[2]];

                // Infinite axis: cylinder shape (project out the axis
                // component); else full 3D distance (sphere shape).
                let radial = if v.infinite_axis {
                    let axial = vec3_dot(to_particle, axis);
                    [
                        to_particle[0] - axis[0] * axial,
                        to_particle[1] - axis[1] * axial,
                        to_particle[2] - axis[2] * axial,
                    ]
                } else {
                    to_particle
                };
                let distance = vec3_length(radial);

                let tangent = vec3_cross(axis, radial);
                let tangent_len = vec3_length(tangent);
                if tangent_len > 0.001 {
                    let tangent = [
                        tangent[0] / tangent_len,
                        tangent[1] / tangent_len,
                        tangent[2] / tangent_len,
                    ];

                    let mut speed = 0.0;
                    let mut radial_force = [0.0f32; 3];
                    if v.ring_shape {
                        let ring_inner = v.ring_radius - v.ring_width * 0.5;
                        let ring_outer = v.ring_radius + v.ring_width * 0.5;
                        if distance < ring_inner {
                            // Hollow center: no spin.
                        } else if distance <= ring_outer {
                            let t = (distance - ring_inner) / v.ring_width;
                            speed = v.speed_inner + (v.speed_outer - v.speed_inner) * t;
                        } else if distance <= ring_outer + v.ring_pull_distance {
                            let pull_t = (distance - ring_outer) / v.ring_pull_distance;
                            speed = v.speed_outer * (1.0 - pull_t);
                            if distance > 0.001 {
                                let toward = vec3_normalize_or(radial, [0.0; 3]);
                                let f = -v.ring_pull_force * pull_t;
                                radial_force = [toward[0] * f, toward[1] * f, toward[2] * f];
                            }
                        }
                    } else {
                        let dis_mid = v.distance_outer - v.distance_inner + 0.1;
                        speed = if dis_mid < 0.0 || distance < v.distance_inner {
                            v.speed_inner
                        } else if distance > v.distance_outer {
                            v.speed_outer
                        } else {
                            let t = (distance - v.distance_inner) / dis_mid;
                            v.speed_inner + (v.speed_outer - v.speed_inner) * t
                        };
                    }

                    let k = dt * self.speed_mult;
                    p.vx += (tangent[0] * speed + radial_force[0]) * k;
                    p.vy += (tangent[1] * speed + radial_force[1]) * k;
                    if v.maintain_distance && distance > 0.001 {
                        let toward = vec3_normalize_or(radial, [0.0; 3]);
                        p.vx -= toward[0] * v.center_force * k;
                        p.vy -= toward[1] * v.center_force * k;
                    }
                }
            }

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

            if let Some((idx, origin, scale, threshold)) = self.control_point_attract {
                if let Some(cp) = self.control_points.get(idx) {
                    let center = [cp[0] + origin[0], cp[1] + origin[1]];
                    let to_center = [center[0] - p.x, center[1] - p.y];
                    let distance = (to_center[0] * to_center[0] + to_center[1] * to_center[1]).sqrt();
                    let radius = threshold / 2.0;
                    if distance > 0.001 && distance < radius {
                        let force = scale * dt * self.speed_mult;
                        p.vx += (to_center[0] / distance) * force;
                        p.vy += (to_center[1] / distance) * force;
                    }
                }
            }

            p.x += p.vx * dt;
            p.y += p.vy * dt;
            p.life -= dt;

            // Reference order (CParticle.cpp createAngularMovementOperator):
            // integrate rotation from the *current* angular velocity first,
            // then accelerate angular velocity, then decay it by drag.
            p.rotation += p.angular_velocity * dt;
            p.angular_velocity += self.angular_force * dt;
            let drag_factor = (1.0 - self.angular_drag * dt).max(0.0);
            p.angular_velocity *= drag_factor;
            p.rotation = wrap_angle(p.rotation);

            let age = p.max_life - p.life;
            let lifetime_pos = if p.max_life > 0.0 {
                (age / p.max_life).clamp(0.0, 1.0)
            } else {
                1.0
            };

            // Sprite-sheet frame advance (CParticle.cpp's "Update animation
            // frames"): only meaningful once the caller has told us the
            // resolved sprite actually has more than one frame.
            if self.sprite_frame_count > 1 {
                let frame_count = self.sprite_frame_count as f32;
                match self.animation_mode {
                    AnimationMode::RandomFrame => {
                        if p.frame < 0.0 {
                            p.frame = (fastrand::f32() * frame_count).floor().min(frame_count - 1.0);
                        }
                    }
                    AnimationMode::Once => {
                        p.frame = (lifetime_pos * frame_count * self.sequence_multiplier)
                            .min(frame_count - 1.0);
                    }
                    AnimationMode::Loop => {
                        p.frame = if self.sprite_duration > 0.0 {
                            let time_in_cycle = (age * self.sequence_multiplier) % self.sprite_duration;
                            let cycle_pos = time_in_cycle / self.sprite_duration;
                            (cycle_pos * frame_count) % frame_count
                        } else {
                            (lifetime_pos * frame_count * self.sequence_multiplier) % frame_count
                        };
                    }
                }
            }

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

            if let Some((start, end, start_color, end_color)) = self.colorchange {
                let mult = [
                    fade_value(lifetime_pos, start, end, start_color[0], end_color[0]),
                    fade_value(lifetime_pos, start, end, start_color[1], end_color[1]),
                    fade_value(lifetime_pos, start, end, start_color[2], end_color[2]),
                ];
                p.color = [
                    (p.initial_color[0] * mult[0] * 255.0).clamp(0.0, 255.0) as u8,
                    (p.initial_color[1] * mult[1] * 255.0).clamp(0.0, 255.0) as u8,
                    (p.initial_color[2] * mult[2] * 255.0).clamp(0.0, 255.0) as u8,
                ];
            }

            // `self.alpha_mult` (the instance-override multiplier) is already
            // baked into `p.initial_alpha` at spawn, matching the
            // reference's `p.alpha = random(min,max) * override` — applying
            // it again here would square it whenever an override isn't 1.0.
            p.alpha = alpha;
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

                let (mut vx, mut vy) = if let (Some(min), Some(max)) =
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
                let mut spawn_x = ox + (fastrand::f32() - 0.5) * spread_x;
                let mut spawn_y = oy + (fastrand::f32() - 0.5) * spread_y;

                // `mapsequencearoundcontrolpoint` (CParticle.cpp): spawn at
                // the control point, launching successive particles at
                // evenly-spaced angles around a circle (the initializer
                // *assigns* position/velocity, unlike the additive ones).
                if let Some(ms) = self.map_sequence {
                    let angle = (self.map_sequence_index as f32 / ms.count as f32)
                        * std::f32::consts::TAU;
                    self.map_sequence_index = (self.map_sequence_index + 1) % ms.count;
                    if let Some(cp) = self.control_points.get(ms.control_point) {
                        spawn_x = cp[0];
                        spawn_y = cp[1];
                    }
                    let sx = ms.speed_min[0]
                        + fastrand::f32() * (ms.speed_max[0] - ms.speed_min[0]).max(0.0);
                    let sy = ms.speed_min[1]
                        + fastrand::f32() * (ms.speed_max[1] - ms.speed_min[1]).max(0.0);
                    let (sin, cos) = angle.sin_cos();
                    // The reference's glm::mat3 is column-major:
                    // v' = (cos*x + sin*y, -sin*x + cos*y).
                    vx = (cos * sx + sin * sy) * self.speed_mult;
                    vy = (-sin * sx + cos * sy) * self.speed_mult;
                }

                // `turbulentvelocityrandom` (CParticle.cpp): curl-noise
                // spawn velocity, angle-limited around `forward`, added on
                // top of any plain `velocityrandom` contribution.
                if let Some(t) = self.turbulent_velocity {
                    let forward = vec3_normalize_or(t.forward, [0.0, 1.0, 0.0]);
                    let right = vec3_normalize_or(t.right, [1.0, 0.0, 0.0]);
                    let speed =
                        t.speed_min + fastrand::f32() * (t.speed_max - t.speed_min).max(0.0);
                    let phase =
                        t.phase_min + fastrand::f32() * (t.phase_max - t.phase_min).max(0.0);
                    let time_shift = self.time * t.timescale;
                    let sample = [
                        spawn_x * 0.1 + time_shift + phase,
                        spawn_y * 0.1 + time_shift + phase * 0.7,
                        time_shift + phase * 1.3,
                    ];
                    let curl = crate::engine::noise::curl_noise(sample);
                    let mut dir = vec3_normalize_or(curl, forward);

                    // `scale` < 2 limits how far the direction may deviate
                    // from `forward` (normalized angle, max = scale/2).
                    if t.scale < 2.0 {
                        let cos_angle = vec3_dot(dir, forward).clamp(-1.0, 1.0);
                        let angle = cos_angle.acos() / std::f32::consts::PI;
                        let max_angle = t.scale / 2.0;
                        if angle > max_angle && max_angle > 0.0001 {
                            let axis = vec3_cross(dir, forward);
                            if vec3_length(axis) > 0.0001 {
                                let axis = vec3_normalize_or(axis, [0.0, 0.0, 1.0]);
                                let rot = (angle - max_angle) * std::f32::consts::PI;
                                dir = vec3_rotate_axis(dir, axis, rot);
                            }
                        }
                    }

                    // `offset` tilts the result around `right`.
                    if t.offset.abs() > 0.0001 {
                        dir = vec3_rotate_axis(dir, right, -t.offset);
                    }

                    // 2D particles: project onto the XY plane and
                    // renormalize (the reference's `flags & 4 == 0` branch —
                    // our renderer is always orthographic/2D).
                    dir[2] = 0.0;
                    let dir = vec3_normalize_or(dir, forward);

                    vx += dir[0] * speed * self.speed_mult;
                    vy += dir[1] * speed * self.speed_mult;
                }

                // `rotationrandom`: uniform in [min.z, max.z], scaled by the
                // instance speed override (the reference multiplies rotation
                // by `speedOverride` too, odd as that reads).
                let spawn_rotation = self
                    .rotation_range
                    .map(|(min, max)| {
                        (min + fastrand::f32() * (max - min).max(0.0)) * self.speed_mult
                    })
                    .unwrap_or(0.0);

                let color = self.color_override.unwrap_or_else(|| {
                    [
                        lerp_u8(self.color_min[0], self.color_max[0]),
                        lerp_u8(self.color_min[1], self.color_max[1]),
                        lerp_u8(self.color_min[2], self.color_max[2]),
                    ]
                });

                // `alpharandom`: CParticle.cpp's `p.alpha = random(min, max) *
                // override` — a plain multiply (unlike sizerandom, not halved).
                let spawn_alpha = (self.alpha_min
                    + fastrand::f32() * (self.alpha_max - self.alpha_min).max(0.0))
                    * self.alpha_mult;

                self.particles.push(Particle {
                    x: spawn_x,
                    y: spawn_y,
                    vx,
                    vy,
                    life,
                    max_life: life,
                    size,
                    alpha: spawn_alpha,
                    color,
                    initial_alpha: spawn_alpha,
                    initial_size: size,
                    initial_color: [
                        color[0] as f32 / 255.0,
                        color[1] as f32 / 255.0,
                        color[2] as f32 / 255.0,
                    ],
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
                    rotation: spawn_rotation,
                    angular_velocity: self.angular_velocity_min
                        + fastrand::f32() * (self.angular_velocity_max - self.angular_velocity_min),
                    frame: -1.0,
                });
            }
        }
    }

    /// Bounding box (`min_x, min_y, max_x, max_y`) over all alive particles'
    /// `position ± size` (with a small margin for the soft-falloff glow
    /// radius) — `None` when nothing is alive, so callers can skip a wasted
    /// raster/upload entirely. Lets `render_onto` be called with a canvas
    /// sized to just this box instead of the full scene.
    pub fn bounds(&self) -> Option<(f32, f32, f32, f32)> {
        let mut min_x = f32::MAX;
        let mut min_y = f32::MAX;
        let mut max_x = f32::MIN;
        let mut max_y = f32::MIN;
        for p in &self.particles {
            // `p.size` is a radius (see `sizerandom`'s halving comment) for
            // both the circle and textured-quad draws; pad by a couple of
            // pixels for the soft falloff's antialiasing.
            let half = p.size.max(1.0) + 2.0;
            min_x = min_x.min(p.x - half);
            min_y = min_y.min(p.y - half);
            max_x = max_x.max(p.x + half);
            max_y = max_y.max(p.y + half);
        }
        (!self.particles.is_empty()).then_some((min_x, min_y, max_x, max_y))
    }

    /// `origin` shifts every particle's canvas-space position by `-origin` —
    /// lets callers raster into a canvas sized to just a system's bounding
    /// box (see `bounds()`) instead of the full scene. Existing full-canvas
    /// callers pass `[0.0, 0.0]`.
    pub fn render_onto(
        &self,
        canvas: &mut RgbaImage,
        sprite: Option<&ParticleSprite>,
        origin: [f32; 2],
    ) {
        self.render_onto_blended(canvas, sprite, origin, false);
    }

    /// `render_onto` with the material's blending mode: `additive` layers
    /// accumulate premultiplied contributions (the reference renders each
    /// particle quad with `glBlendFunc(GL_SRC_ALPHA, GL_ONE)`, CPass.cpp),
    /// so overlapping particles brighten each other instead of the newer
    /// one occluding the older via "over" compositing.
    pub fn render_onto_blended(
        &self,
        canvas: &mut RgbaImage,
        sprite: Option<&ParticleSprite>,
        origin: [f32; 2],
        additive: bool,
    ) {
        if self.rope_mode {
            self.render_rope_onto(canvas, origin, additive);
            return;
        }

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
            let px_pos = p.x - origin[0];
            let py_pos = p.y - origin[1];

            if let Some(sprite) = sprite {
                // `p.frame` is only ever set (in `step`) when the sprite has
                // more than one frame; a single-frame sprite always draws
                // frame 0 regardless of its (unused, still -1.0) value.
                let frame_idx = if sprite.frames.len() > 1 {
                    (p.frame.max(0.0) as usize).min(sprite.frames.len() - 1)
                } else {
                    0
                };
                let tex = &sprite.frames[frame_idx];
                // `spritetrail`: align the quad's V axis with the particle's
                // velocity and stretch it by clamp(|v|*length, min, max) —
                // genericparticle.vert's TRAILRENDERER path, where `up` =
                // normalized velocity scaled by that clamp and texture-top
                // (v=0) points along it.
                let (half_w, half_h, rotation) = match self.sprite_trail {
                    Some(t) => {
                        let vel_len = (p.vx * p.vx + p.vy * p.vy).sqrt();
                        let trail = (vel_len * t.length)
                            .clamp(t.min_length.min(t.max_length), t.max_length)
                            .max(0.05);
                        let angle = if vel_len > 0.0001 {
                            p.vy.atan2(p.vx) + std::f32::consts::FRAC_PI_2
                        } else {
                            p.rotation
                        };
                        (p.size, p.size * trail, angle)
                    }
                    None => (p.size, p.size, p.rotation),
                };
                draw_textured_particle(
                    canvas, px_pos, py_pos, half_w, half_h, rotation, p.color, alpha, tex, additive,
                );
                continue;
            }

            let cx = px_pos as i32;
            let cy = py_pos as i32;

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
                    let src_a = (alpha as f32 / 255.0) * falloff;
                    let src_c = [
                        p.color[0] as f32 / 255.0,
                        p.color[1] as f32 / 255.0,
                        p.color[2] as f32 / 255.0,
                    ];
                    let dst = canvas.get_pixel_mut(px as u32, py as u32);
                    blend_pixel(dst, src_c, src_a, additive);
                }
            }
        }
    }

    /// Draws a connected ribbon through living particles (oldest-first, since
    /// `step()`'s retain preserves order and new spawns are appended) instead
    /// of independent circles. Position is interpolated with the reference's
    /// exact Catmull-Rom formula (CParticle.cpp:2124-2130); size/color/alpha
    /// are linearly interpolated. The reference computes ribbon *width* in an
    /// external, proprietary shader (not present in the open-source reference
    /// repo) from a tangent/normal it derives per-vertex — this perpendicular-
    /// offset quad approach is our own design for the same visual effect,
    /// informed by, but not a line-for-line port of, that shader.
    fn render_rope_onto(&self, canvas: &mut RgbaImage, origin: [f32; 2], additive: bool) {
        let n = self.particles.len();
        if n < 2 {
            return;
        }

        struct SubPoint {
            pos: [f32; 2],
            size: f32,
            color: [u8; 3],
            alpha: f32,
        }

        let subdiv = self.rope_subdivision;
        let mut subpoints = Vec::with_capacity((n - 1) * subdiv + 1);
        for i in 0..(n - 1) {
            let idx0 = i.saturating_sub(1);
            let p0 = &self.particles[idx0];
            let p1 = &self.particles[i];
            let p2 = &self.particles[i + 1];
            let idx3 = (i + 2).min(n - 1);
            let p3 = &self.particles[idx3];

            for step in 0..subdiv {
                let t = step as f32 / subdiv as f32;
                let pos = catmull_rom_vec2(
                    [p0.x, p0.y],
                    [p1.x, p1.y],
                    [p2.x, p2.y],
                    [p3.x, p3.y],
                    t,
                );
                subpoints.push(SubPoint {
                    pos: [pos[0] - origin[0], pos[1] - origin[1]],
                    size: lerp_f32(p1.size, p2.size, t),
                    alpha: lerp_f32(p1.alpha, p2.alpha, t),
                    color: [
                        lerp_f32(p1.color[0] as f32, p2.color[0] as f32, t) as u8,
                        lerp_f32(p1.color[1] as f32, p2.color[1] as f32, t) as u8,
                        lerp_f32(p1.color[2] as f32, p2.color[2] as f32, t) as u8,
                    ],
                });
            }
        }
        let last = &self.particles[n - 1];
        subpoints.push(SubPoint {
            pos: [last.x - origin[0], last.y - origin[1]],
            size: last.size,
            color: last.color,
            alpha: last.alpha,
        });

        for pair in subpoints.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            fill_rope_segment(
                canvas, a.pos, b.pos, a.size, b.size, a.color, b.color, a.alpha, b.alpha, additive,
            );
        }
    }
}

/// Reference's exact Catmull-Rom formula (CParticle.cpp:2124-2130), applied
/// per-axis.
fn catmull_rom_vec2(p0: [f32; 2], p1: [f32; 2], p2: [f32; 2], p3: [f32; 2], t: f32) -> [f32; 2] {
    let t2 = t * t;
    let t3 = t2 * t;
    let mut out = [0.0; 2];
    for axis in 0..2 {
        let (p0, p1, p2, p3) = (p0[axis], p1[axis], p2[axis], p3[axis]);
        out[axis] = 0.5
            * ((2.0 * p1)
                + (-p0 + p2) * t
                + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
                + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3);
    }
    out
}

fn lerp_f32(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Rasterizes one ribbon quad between two consecutive sub-points: offsets
/// each endpoint by its half-size along the segment's perpendicular normal,
/// then fills the resulting trapezoid with "over" alpha blending (same style
/// as the per-particle circle draw).
#[allow(clippy::too_many_arguments)]
fn fill_rope_segment(
    canvas: &mut RgbaImage,
    a: [f32; 2],
    b: [f32; 2],
    size_a: f32,
    size_b: f32,
    color_a: [u8; 3],
    color_b: [u8; 3],
    alpha_a: f32,
    alpha_b: f32,
    additive: bool,
) {
    let dir = [b[0] - a[0], b[1] - a[1]];
    let len = (dir[0] * dir[0] + dir[1] * dir[1]).sqrt();
    if len < 0.0001 {
        return;
    }
    let dir = [dir[0] / len, dir[1] / len];
    let normal = [-dir[1], dir[0]];
    let (half_a, half_b) = (size_a.max(0.5) / 2.0, size_b.max(0.5) / 2.0);

    let corners = [
        [a[0] + normal[0] * half_a, a[1] + normal[1] * half_a],
        [b[0] + normal[0] * half_b, b[1] + normal[1] * half_b],
        [b[0] - normal[0] * half_b, b[1] - normal[1] * half_b],
        [a[0] - normal[0] * half_a, a[1] - normal[1] * half_a],
    ];

    let min_x = corners.iter().map(|c| c[0]).fold(f32::MAX, f32::min).floor().max(0.0) as i32;
    let max_x = corners
        .iter()
        .map(|c| c[0])
        .fold(f32::MIN, f32::max)
        .ceil()
        .min(canvas.width() as f32 - 1.0) as i32;
    let min_y = corners.iter().map(|c| c[1]).fold(f32::MAX, f32::min).floor().max(0.0) as i32;
    let max_y = corners
        .iter()
        .map(|c| c[1])
        .fold(f32::MIN, f32::max)
        .ceil()
        .min(canvas.height() as f32 - 1.0) as i32;
    if min_x > max_x || min_y > max_y {
        return;
    }

    let w = canvas.width() as i32;
    let h = canvas.height() as i32;
    for py in min_y..=max_y {
        for px in min_x..=max_x {
            if px < 0 || py < 0 || px >= w || py >= h {
                continue;
            }
            let point = [px as f32 + 0.5, py as f32 + 0.5];
            if !point_in_convex_quad(point, &corners) {
                continue;
            }
            // Blend color/alpha by which endpoint the pixel is nearer along
            // the segment (a cheap stand-in for true per-pixel interpolation).
            let seg_t = ((point[0] - a[0]) * dir[0] + (point[1] - a[1]) * dir[1]) / len;
            let seg_t = seg_t.clamp(0.0, 1.0);
            let alpha = (lerp_f32(alpha_a, alpha_b, seg_t) * 180.0).clamp(0.0, 255.0) as u8;
            if alpha == 0 {
                continue;
            }
            let color = [
                lerp_f32(color_a[0] as f32, color_b[0] as f32, seg_t) as u8,
                lerp_f32(color_a[1] as f32, color_b[1] as f32, seg_t) as u8,
                lerp_f32(color_a[2] as f32, color_b[2] as f32, seg_t) as u8,
            ];

            let src_a = alpha as f32 / 255.0;
            let src_c = [
                color[0] as f32 / 255.0,
                color[1] as f32 / 255.0,
                color[2] as f32 / 255.0,
            ];
            let dst = canvas.get_pixel_mut(px as u32, py as u32);
            blend_pixel(dst, src_c, src_a, additive);
        }
    }
}

/// Point-in-convex-quad test via consistent cross-product sign across all
/// four edges. `corners` must be wound consistently (either all-CW or
/// all-CCW) — true by construction here (two parallel perpendicular offsets).
fn point_in_convex_quad(point: [f32; 2], corners: &[[f32; 2]; 4]) -> bool {
    let mut sign = 0.0f32;
    for i in 0..4 {
        let a = corners[i];
        let b = corners[(i + 1) % 4];
        let edge = [b[0] - a[0], b[1] - a[1]];
        let to_point = [point[0] - a[0], point[1] - a[1]];
        let cross = edge[0] * to_point[1] - edge[1] * to_point[0];
        if i == 0 {
            sign = cross;
        } else if cross * sign < 0.0 {
            return false;
        }
    }
    true
}

/// Wraps an angle to `[-pi, pi]`, matching `createAngularMovementOperator`'s
/// per-axis wrap (CParticle.cpp).
fn wrap_angle(a: f32) -> f32 {
    let mut a = a % std::f32::consts::TAU;
    if a > std::f32::consts::PI {
        a -= std::f32::consts::TAU;
    } else if a < -std::f32::consts::PI {
        a += std::f32::consts::TAU;
    }
    a
}

// Minimal vec3 math for the turbulent/vortex ports (the reference uses glm;
// pulling in a linear-algebra crate for five one-liners isn't worth it).

fn vec3_dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn vec3_cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn vec3_length(v: [f32; 3]) -> f32 {
    vec3_dot(v, v).sqrt()
}

/// Normalize, or return `fallback` for a (near-)zero vector — the
/// reference's recurring `length > 0.0001 ? normalize(v) : default` guard.
fn vec3_normalize_or(v: [f32; 3], fallback: [f32; 3]) -> [f32; 3] {
    let len = vec3_length(v);
    if len > 0.0001 {
        [v[0] / len, v[1] / len, v[2] / len]
    } else {
        fallback
    }
}

/// Rotate `v` by `angle` radians around unit-length `axis` (Rodrigues'
/// formula — equivalent to the reference's `glm::rotate` mat3 use).
fn vec3_rotate_axis(v: [f32; 3], axis: [f32; 3], angle: f32) -> [f32; 3] {
    let (sin, cos) = angle.sin_cos();
    let cross = vec3_cross(axis, v);
    let dot = vec3_dot(axis, v) * (1.0 - cos);
    [
        v[0] * cos + cross[0] * sin + axis[0] * dot,
        v[1] * cos + cross[1] * sin + axis[1] * dot,
        v[2] * cos + cross[2] * sin + axis[2] * dot,
    ]
}

/// Draws one particle as a rotated, textured quad: inverse-rotates each
/// destination pixel in the quad's bounding box back into texture space,
/// bilinear-samples, and composites tinted-by-`color`/scaled-by-`alpha_byte`
/// over the canvas. The reference hands rotation+size to an external,
/// proprietary vertex shader to build the quad (not present in the
/// open-source reference repo) — this CPU inverse-mapping is our own design
/// for the same visual effect.
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_arguments)]
fn draw_textured_particle(
    canvas: &mut RgbaImage,
    cx: f32,
    cy: f32,
    half_w: f32,
    half_h: f32,
    rotation: f32,
    color: [u8; 3],
    alpha_byte: u8,
    tex: &RgbaImage,
    additive: bool,
) {
    // `p.size` is already a radius, not a diameter (see `sizerandom`'s
    // comment on the halving done at spawn) — matches the flat-color circle
    // draw's `sz = p.size` convention, so a particle looks the same size
    // whether or not it has a material/texture. The two half-extents differ
    // only for `spritetrail` quads (stretched along velocity).
    let half_w = half_w.max(0.5);
    let half_h = half_h.max(0.5);
    let cos_r = rotation.cos();
    let sin_r = rotation.sin();

    // Bounding box of the rotated quad: the diagonal half-extent covers any
    // rotation angle.
    let diag = (half_w * half_w + half_h * half_h).sqrt();
    let w = canvas.width() as i32;
    let h = canvas.height() as i32;
    let min_x = ((cx - diag).floor() as i32).max(0);
    let max_x = ((cx + diag).ceil() as i32).min(w - 1);
    let min_y = ((cy - diag).floor() as i32).max(0);
    let max_y = ((cy + diag).ceil() as i32).min(h - 1);

    for py in min_y..=max_y {
        for px in min_x..=max_x {
            let dx = px as f32 + 0.5 - cx;
            let dy = py as f32 + 0.5 - cy;
            // Inverse-rotate the destination offset back into the quad's own
            // (unrotated) local space.
            let lx = dx * cos_r + dy * sin_r;
            let ly = -dx * sin_r + dy * cos_r;
            if lx < -half_w || lx > half_w || ly < -half_h || ly > half_h {
                continue;
            }
            let u = (lx + half_w) / (2.0 * half_w);
            let v = (ly + half_h) / (2.0 * half_h);
            let sample = sample_bilinear(tex, u, v);
            let src_a = (sample[3] as f32 / 255.0) * (alpha_byte as f32 / 255.0);
            if src_a <= 0.0 {
                continue;
            }
            let src_c = [
                (sample[0] as f32 / 255.0) * (color[0] as f32 / 255.0),
                (sample[1] as f32 / 255.0) * (color[1] as f32 / 255.0),
                (sample[2] as f32 / 255.0) * (color[2] as f32 / 255.0),
            ];
            let dst = canvas.get_pixel_mut(px as u32, py as u32);
            blend_pixel(dst, src_c, src_a, additive);
        }
    }
}

/// Composites one source sample into a canvas pixel.
///
/// Normal/translucent layers use standard "over" compositing (not a plain
/// lerp): the destination isn't always opaque — the GPU path renders
/// particles into their own transparent scene-sized buffer before
/// compositing it as a texture, so overlapping particles must accumulate
/// alpha correctly rather than assuming a fully-opaque backdrop.
///
/// Additive layers accumulate premultiplied contributions instead — the
/// reference draws every particle quad with
/// `glBlendFuncSeparate(GL_SRC_ALPHA, GL_ONE, GL_SRC_ALPHA, GL_ONE)`
/// (CPass.cpp), i.e. `dst += src * src_a` — so overlapping glows brighten
/// each other rather than the newest quad occluding the ones below. The
/// buffer then holds already-premultiplied color, which the GPU composite's
/// pure-add mode adds to the scene without re-weighting by alpha.
fn blend_pixel(dst: &mut image::Rgba<u8>, src_c: [f32; 3], src_a: f32, additive: bool) {
    if additive {
        for i in 0..3 {
            let dst_c = dst[i] as f32 / 255.0;
            dst[i] = ((dst_c + src_c[i] * src_a) * 255.0).clamp(0.0, 255.0) as u8;
        }
        let dst_a = dst[3] as f32 / 255.0;
        dst[3] = ((dst_a + src_a * src_a) * 255.0).clamp(0.0, 255.0) as u8;
        return;
    }
    let dst_a = dst[3] as f32 / 255.0;
    let out_a = src_a + dst_a * (1.0 - src_a);
    if out_a > 0.0 {
        for i in 0..3 {
            let dst_c = dst[i] as f32 / 255.0;
            let out_c = (src_c[i] * src_a + dst_c * dst_a * (1.0 - src_a)) / out_a;
            dst[i] = (out_c * 255.0).clamp(0.0, 255.0) as u8;
        }
    }
    dst[3] = (out_a * 255.0).clamp(0.0, 255.0) as u8;
}

fn sample_bilinear(tex: &RgbaImage, u: f32, v: f32) -> [u8; 4] {
    let (tw, th) = (tex.width(), tex.height());
    let fx = (u.clamp(0.0, 1.0) * (tw as f32 - 1.0).max(0.0)).max(0.0);
    let fy = (v.clamp(0.0, 1.0) * (th as f32 - 1.0).max(0.0)).max(0.0);
    let x0 = fx.floor() as u32;
    let y0 = fy.floor() as u32;
    let x1 = (x0 + 1).min(tw - 1);
    let y1 = (y0 + 1).min(th - 1);
    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);

    let p00 = tex.get_pixel(x0, y0);
    let p10 = tex.get_pixel(x1, y0);
    let p01 = tex.get_pixel(x0, y1);
    let p11 = tex.get_pixel(x1, y1);

    let mut out = [0u8; 4];
    for i in 0..4 {
        let top = p00[i] as f32 * (1.0 - tx) + p10[i] as f32 * tx;
        let bottom = p01[i] as f32 * (1.0 - tx) + p11[i] as f32 * tx;
        out[i] = (top * (1.0 - ty) + bottom * ty).clamp(0.0, 255.0) as u8;
    }
    out
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

/// Initializer names are matched exactly — they're fixed strings in the
/// reference's ObjectParser, and substring matching let
/// `turbulentvelocityrandom`/`angularvelocityrandom` shadow
/// `velocityrandom` (or `angularmovement` shadow `movement`) depending on
/// declaration order.
fn scalar_range_from_initializers(
    initializers: &[Initializer],
    name: &str,
    default_min: f32,
    default_max: f32,
) -> (f32, f32) {
    let Some(init) = initializers.iter().find(|init| init.name == name) else {
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
/// Exact-name matching for the same reason as `scalar_range_from_initializers`.
fn vec3_range_from_initializers(
    initializers: &[Initializer],
    name: &str,
) -> Option<([f32; 3], [f32; 3])> {
    let init = initializers.iter().find(|init| init.name == name)?;
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

    /// A real `colorchange` preset (torch.json): "r g b" 0..1 vector strings
    /// for startvalue/endvalue — these previously broke parsing entirely
    /// (Operator.startvalue/endvalue were typed as bare f64).
    #[test]
    fn colorchange_parses_and_multiplies_initial_color_over_lifetime() {
        let json = r#"{
            "maxcount": 5,
            "emitter": [{"name":"box","rate":1}],
            "initializer": [{"id":1,"name":"colorrandom","min":"255 255 255","max":"255 255 255"}],
            "operator": [{"id":1,"name":"colorchange","startvalue":"1 0.75 0","endvalue":"1 0 0","endtime":1}]
        }"#;
        let config: ParticleConfig = serde_json::from_str(json).expect("should parse");
        let mut sys = ParticleSystem::from_config(&config, [0.0, 0.0], None);
        sys.life_min = 10.0;
        sys.life_max = 10.0;
        sys.emitters[0].rate = 1000.0;
        sys.step(0.001); // spawns the particle; fade math runs on existing particles first
        assert_eq!(sys.particles.len(), 1);

        sys.step(0.001); // age≈0 → at startvalue (1, 0.75, 0) * white
        let c0 = sys.particles[0].color;
        assert_eq!(c0[0], 255);
        assert!((c0[1] as i32 - 191).abs() <= 2); // 0.75*255≈191
        assert_eq!(c0[2], 0);

        sys.step(9.99); // age≈10 → lifetime_pos=1.0 → at endvalue (1, 0, 0) * white
        let c1 = sys.particles[0].color;
        assert_eq!(c1, [255, 0, 0]);
    }

    /// A real `controlpointattract` preset pulls particles within `threshold/2`
    /// of a control point toward it, and leaves particles outside that radius
    /// untouched (CParticle.cpp createControlPointAttractOperator).
    #[test]
    fn controlpointattract_pulls_nearby_particles_toward_control_point() {
        let json = r#"{
            "maxcount": 5,
            "emitter": [{"name":"box","rate":1}],
            "controlpoint": [{"id":0,"offset":"200 0 0"}],
            "operator": [{"id":1,"name":"controlpointattract","controlpoint":0,"scale":500,"threshold":1000}]
        }"#;
        let config: ParticleConfig = serde_json::from_str(json).expect("should parse");
        // spawn_center [0,0] + control point offset "200 0 0" -> control point at [200, 0].
        let mut sys = ParticleSystem::from_config(&config, [0.0, 0.0], None);
        assert_eq!(sys.control_points, vec![[200.0, 0.0, 0.0]]);

        sys.life_min = 10.0;
        sys.life_max = 10.0;
        sys.emitters[0].rate = 1000.0;
        sys.step(0.001); // spawns the particle at/near [0,0]
        assert_eq!(sys.particles.len(), 1);
        let start_dist = ((sys.particles[0].x - 200.0).powi(2) + sys.particles[0].y.powi(2)).sqrt();

        for _ in 0..10 {
            sys.step(0.05);
        }
        let end_dist = ((sys.particles[0].x - 200.0).powi(2) + sys.particles[0].y.powi(2)).sqrt();
        assert!(
            end_dist < start_dist,
            "expected particle to be pulled toward control point: start={start_dist} end={end_dist}"
        );
    }

    /// A particle outside the attract radius (`threshold/2`) should be left
    /// alone entirely.
    #[test]
    fn controlpointattract_ignores_particles_outside_threshold() {
        let json = r#"{
            "maxcount": 5,
            "emitter": [{"name":"box","rate":1}],
            "controlpoint": [{"id":0,"offset":"5000 0 0"}],
            "operator": [{"id":1,"name":"controlpointattract","controlpoint":0,"scale":500,"threshold":10}]
        }"#;
        let config: ParticleConfig = serde_json::from_str(json).expect("should parse");
        let mut sys = ParticleSystem::from_config(&config, [0.0, 0.0], None);
        sys.life_min = 10.0;
        sys.life_max = 10.0;
        sys.emitters[0].rate = 1000.0;
        sys.step(0.001);
        assert_eq!(sys.particles.len(), 1);
        // Capture the emitter-assigned spawn velocity (unrelated to the
        // control point), then confirm a step leaves it untouched since the
        // particle is well outside the attract radius.
        let (vx0, vy0) = (sys.particles[0].vx, sys.particles[0].vy);
        sys.step(0.05);
        assert_eq!(sys.particles[0].vx, vx0);
        assert_eq!(sys.particles[0].vy, vy0);
    }

    /// A `rope`/`ropetrail` renderer should draw a connected ribbon through
    /// living particles rather than isolated circles — verify pixels are
    /// filled *between* particle centers, not just at them.
    #[test]
    fn rope_renderer_fills_ribbon_between_particles() {
        let json = r#"{
            "maxcount": 5,
            "emitter": [{"name":"box","rate":1}],
            "initializer": [{"id":1,"name":"sizerandom","min":40,"max":40}],
            "renderer": [{"id":1,"name":"rope","subdivision":4}]
        }"#;
        let config: ParticleConfig = serde_json::from_str(json).expect("should parse");
        let sys = ParticleSystem::from_config(&config, [0.0, 0.0], None);
        assert!(sys.rope_mode);
        assert_eq!(sys.rope_subdivision, 4);

        // Manually place three particles in a horizontal line, all alive.
        let mut sys = sys;
        for (i, x) in [0.0f32, 50.0, 100.0].into_iter().enumerate() {
            sys.particles.push(Particle {
                x,
                y: 100.0,
                vx: 0.0,
                vy: 0.0,
                life: 5.0,
                max_life: 5.0,
                size: 20.0,
                alpha: 1.0,
                color: [255, 0, 0],
                initial_alpha: 1.0,
                initial_size: 20.0,
                initial_color: [1.0, 0.0, 0.0],
                osc_alpha: None,
                osc_size: None,
                osc_pos: None,
                rotation: 0.0,
                angular_velocity: 0.0,
                frame: -1.0,
            });
            let _ = i;
        }

        let mut canvas = RgbaImage::new(200, 200);
        sys.render_onto(&mut canvas, None, [0.0, 0.0]);

        // Midpoint between the first two particles (25, 100) should be filled
        // — a plain per-particle circle draw (radius 10) would leave this gap
        // empty since it's 25px from the nearest particle center.
        let mid_pixel = canvas.get_pixel(25, 100);
        assert!(mid_pixel[3] > 0, "expected ribbon fill at midpoint, got {mid_pixel:?}");
    }

    fn make_particle(x: f32, y: f32, size: f32, rotation: f32) -> Particle {
        Particle {
            x,
            y,
            vx: 0.0,
            vy: 0.0,
            life: 5.0,
            max_life: 5.0,
            size,
            alpha: 1.0,
            color: [255, 255, 255],
            initial_alpha: 1.0,
            initial_size: size,
            initial_color: [1.0, 1.0, 1.0],
            osc_alpha: None,
            osc_size: None,
            osc_pos: None,
            rotation,
            angular_velocity: 0.0,
            frame: -1.0,
        }
    }

    /// A textured particle's rotation should actually affect what's drawn —
    /// sampling the same canvas location at two different rotation angles
    /// must produce different colors, proving the quad (and the texture
    /// sampled onto it) is genuinely rotated, not just passed through.
    #[test]
    fn textured_particle_rotation_changes_sampled_footprint() {
        // 8x8 sprite: left half red, right half blue, fully opaque.
        let mut tex = RgbaImage::new(8, 8);
        for y in 0..8 {
            for x in 0..8 {
                let color = if x < 4 { [255, 0, 0, 255] } else { [0, 0, 255, 255] };
                tex.put_pixel(x, y, image::Rgba(color));
            }
        }

        let config: ParticleConfig = serde_json::from_str(
            r#"{"maxcount":1,"emitter":[{"name":"box","rate":1}]}"#,
        )
        .expect("should parse");

        let sprite = ParticleSprite::single(tex.clone());
        let render_at = |rotation: f32| -> image::Rgba<u8> {
            let mut sys = ParticleSystem::from_config(&config, [0.0, 0.0], None);
            sys.particles.push(make_particle(50.0, 50.0, 20.0, rotation));
            let mut canvas = RgbaImage::new(100, 100);
            sys.render_onto(&mut canvas, Some(&sprite), [0.0, 0.0]);
            *canvas.get_pixel(44, 50)
        };

        let unrotated = render_at(0.0);
        let rotated = render_at(std::f32::consts::FRAC_PI_2);
        assert_ne!(
            unrotated, rotated,
            "expected rotation to change the sampled footprint at a fixed canvas point"
        );
    }

    /// `rotationrandom` with a degenerate `[min.z, max.z]` range must spawn
    /// every particle at exactly that rotation (CParticle.cpp
    /// createRotationRandomInitializer, z-axis reduction); absent, spawn
    /// rotation stays 0.
    #[test]
    fn rotationrandom_sets_spawn_rotation() {
        let json = r#"{
            "maxcount": 4,
            "emitter": [{"name":"box","rate":1000}],
            "initializer": [{"id":1,"name":"rotationrandom","min":"0 0 1","max":"0 0 1"}]
        }"#;
        let config: ParticleConfig = serde_json::from_str(json).expect("should parse");
        let mut sys = ParticleSystem::from_config(&config, [0.0, 0.0], None);
        sys.step(0.001);
        assert!((sys.particles[0].rotation - 1.0).abs() < 1e-4);

        let plain: ParticleConfig =
            serde_json::from_str(r#"{"maxcount":1,"emitter":[{"name":"box","rate":1000}]}"#)
                .expect("should parse");
        let mut sys = ParticleSystem::from_config(&plain, [0.0, 0.0], None);
        sys.step(0.001);
        assert_eq!(sys.particles[0].rotation, 0.0);
    }

    /// `turbulentvelocityrandom` adds a curl-noise-directed velocity of
    /// magnitude in `[speedmin, speedmax]` on top of the plain velocity —
    /// with a zero base velocity, the spawn speed must equal the turbulent
    /// speed exactly (the direction is noise, the magnitude is not).
    #[test]
    fn turbulentvelocityrandom_sets_spawn_speed_magnitude() {
        let json = r#"{
            "maxcount": 8,
            "emitter": [{"name":"box","rate":1000}],
            "initializer": [
                {"id":1,"name":"velocityrandom","min":"0 0 0","max":"0 0 0"},
                {"id":2,"name":"turbulentvelocityrandom","speedmin":100,"speedmax":100}
            ]
        }"#;
        let config: ParticleConfig = serde_json::from_str(json).expect("should parse");
        let mut sys = ParticleSystem::from_config(&config, [0.0, 0.0], None);
        sys.step(0.001);
        assert!(!sys.particles.is_empty());
        for p in &sys.particles {
            let speed = (p.vx * p.vx + p.vy * p.vy).sqrt();
            assert!(
                (speed - 100.0).abs() < 0.5,
                "expected turbulent spawn speed 100, got {speed}"
            );
        }
    }

    /// The `turbulence` operator must push otherwise-motionless particles
    /// around (curl-noise acceleration, CParticle.cpp
    /// createTurbulenceOperator).
    #[test]
    fn turbulence_operator_accelerates_particles() {
        let json = r#"{
            "maxcount": 4,
            "emitter": [{"name":"box","rate":1000}],
            "initializer": [{"id":1,"name":"velocityrandom","min":"0 0 0","max":"0 0 0"}],
            "operator": [{"id":1,"name":"turbulence","speedmin":500,"speedmax":500,"scale":0.01}]
        }"#;
        let config: ParticleConfig = serde_json::from_str(json).expect("should parse");
        let mut sys = ParticleSystem::from_config(&config, [10.0, 20.0], None);
        for _ in 0..10 {
            sys.step(1.0 / 30.0);
        }
        let moving = sys
            .particles
            .iter()
            .any(|p| (p.vx * p.vx + p.vy * p.vy).sqrt() > 1.0);
        assert!(moving, "turbulence should have accelerated particles");
    }

    /// `vortex` spins particles tangentially around its control point: a
    /// particle at +X from the center with a +Z axis must gain +Y velocity
    /// (cross(axis, radial)), scaled by speedinner inside distanceinner.
    #[test]
    fn vortex_operator_adds_tangential_velocity() {
        let json = r#"{
            "maxcount": 4,
            "emitter": [{"name":"box","rate":0}],
            "controlpoint": [{"id":0,"offset":"0 0 0"}],
            "operator": [{"id":1,"name":"vortex","controlpoint":0,
                          "distanceinner":500,"distanceouter":650,
                          "speedinner":100,"speedouter":0}]
        }"#;
        let config: ParticleConfig = serde_json::from_str(json).expect("should parse");
        let mut sys = ParticleSystem::from_config(&config, [0.0, 0.0], None);
        sys.particles.push(make_particle(10.0, 0.0, 5.0, 0.0));
        sys.step(0.1);
        let p = &sys.particles[0];
        assert!(
            p.vy > 0.5,
            "expected tangential +Y velocity from vortex, got vy={}",
            p.vy
        );
        assert!(p.vx.abs() < 1e-3, "no radial force expected, got vx={}", p.vx);
    }

    /// `mapsequencearoundcontrolpoint` spawns at the control point and
    /// launches successive particles at evenly-divided angles: with
    /// `count: 4` and a +X-only speed, consecutive spawn velocities must
    /// point right/down-rotated/left/up-rotated (the reference's clockwise
    /// mat3), all with the same magnitude.
    #[test]
    fn mapsequence_spawns_at_control_point_with_rotating_velocity() {
        let json = r#"{
            "maxcount": 4,
            "emitter": [{"name":"box","rate":100000}],
            "controlpoint": [{"id":0,"offset":"50 60 0"}],
            "initializer": [{"id":1,"name":"mapsequencearoundcontrolpoint",
                             "controlpoint":0,"count":4,
                             "speedmin":"80 0 0","speedmax":"80 0 0"}]
        }"#;
        let config: ParticleConfig = serde_json::from_str(json).expect("should parse");
        let mut sys = ParticleSystem::from_config(&config, [0.0, 0.0], None);
        sys.step(0.001);
        assert!(sys.particles.len() >= 4, "expected at least 4 spawns");
        for p in &sys.particles {
            assert_eq!((p.x, p.y), (50.0, 60.0), "must spawn at the control point");
            let speed = (p.vx * p.vx + p.vy * p.vy).sqrt();
            assert!((speed - 80.0).abs() < 1e-3, "speed must stay 80, got {speed}");
        }
        // First two spawns: angle 0 (+X) then 2pi/4 = 90deg rotated.
        assert!((sys.particles[0].vx - 80.0).abs() < 1e-3);
        assert!(sys.particles[0].vy.abs() < 1e-3);
        assert!(sys.particles[1].vx.abs() < 1e-3);
        assert!(sys.particles[1].vy.abs() > 79.0);
    }

    /// `spritetrail` stretches the quad along the particle's velocity:
    /// a fast horizontal particle must paint a footprint wider than tall
    /// (its quad's long axis lies along X), unlike the square default.
    #[test]
    fn spritetrail_stretches_quad_along_velocity() {
        let json = r#"{
            "maxcount": 1,
            "emitter": [{"name":"box","rate":0}],
            "renderer": [{"name":"spritetrail","length":1.0,"maxlength":10}]
        }"#;
        let config: ParticleConfig = serde_json::from_str(json).expect("should parse");
        let sys = ParticleSystem::from_config(&config, [0.0, 0.0], None);
        let mut p = make_particle(50.0, 50.0, 5.0, 0.0);
        p.vx = 8.0; // trail = clamp(8 * 1.0, 0, 10) = 8 -> half-height 40 along X
        let mut sys = sys;
        sys.particles.push(p);

        let tex = RgbaImage::from_pixel(4, 4, image::Rgba([255, 255, 255, 255]));
        let sprite = ParticleSprite::single(tex);
        let mut canvas = RgbaImage::new(100, 100);
        sys.render_onto(&mut canvas, Some(&sprite), [0.0, 0.0]);

        let painted_x = canvas.get_pixel(80, 50)[3] > 0; // 30px right of center
        let painted_y = canvas.get_pixel(50, 80)[3] > 0; // 30px below center
        assert!(painted_x, "trail should extend along +X (velocity)");
        assert!(!painted_y, "trail should stay narrow across velocity");
    }

    /// Name matching must be exact: `turbulentvelocityrandom` declared
    /// before `velocityrandom` (and `angularmovement` before `movement`)
    /// must not shadow them — the old substring matcher (`contains`)
    /// read the turbulent initializer's fields as the plain velocity
    /// range and the angular operator's (absent) gravity as movement's.
    #[test]
    fn similarly_named_initializers_do_not_shadow_each_other() {
        let json = r#"{
            "maxcount": 4,
            "emitter": [{"name":"box","rate":1000}],
            "initializer": [
                {"id":1,"name":"turbulentvelocityrandom","speedmin":500,"speedmax":900},
                {"id":2,"name":"velocityrandom","min":"3 3 0","max":"3 3 0"}
            ],
            "operator": [
                {"id":1,"name":"angularmovement","drag":0.5},
                {"id":2,"name":"movement","gravity":"0 -7 0"}
            ]
        }"#;
        let config: ParticleConfig = serde_json::from_str(json).expect("should parse");
        let sys = ParticleSystem::from_config(&config, [0.0, 0.0], None);
        assert_eq!(sys.velocity_min, Some([3.0, 3.0, 0.0]));
        assert_eq!(sys.velocity_max, Some([3.0, 3.0, 0.0]));
        // WE Y-up gravity -7 maps to our Y-down +7.
        assert_eq!(sys.gravity[1].abs(), 7.0);
    }

    /// A config with no `alpharandom` initializer at all must stay a no-op
    /// (full alpha, matching `ParticleInstance::alpha`'s `1.0` default) —
    /// the common case, and the one that must not regress.
    #[test]
    fn no_alpharandom_initializer_defaults_to_full_alpha() {
        let config: ParticleConfig =
            serde_json::from_str(r#"{"maxcount":1,"emitter":[{"name":"box","rate":1000}]}"#)
                .expect("should parse");
        let mut sys = ParticleSystem::from_config(&config, [0.0, 0.0], None);
        sys.step(0.001);
        assert_eq!(sys.particles[0].initial_alpha, 1.0);
    }

    /// `alpharandom` (e.g. fog1.json's `"min":0.15,"max":0.2`) must actually
    /// constrain spawn alpha to that range instead of always spawning at
    /// full opacity — this was silently ignored before.
    #[test]
    fn alpharandom_constrains_spawn_alpha_to_its_range() {
        let json = r#"{
            "maxcount": 20,
            "emitter": [{"name":"box","rate":1000}],
            "initializer": [{"id":1,"name":"alpharandom","min":0.15,"max":0.2}]
        }"#;
        let config: ParticleConfig = serde_json::from_str(json).expect("should parse");
        let mut sys = ParticleSystem::from_config(&config, [0.0, 0.0], None);
        sys.step(0.05); // spawn several at once (rate 1000 * 0.05s = 50, capped at maxcount)
        assert!(!sys.particles.is_empty());
        for p in &sys.particles {
            assert!(
                (0.15..=0.2).contains(&p.initial_alpha),
                "expected initial_alpha in [0.15, 0.2], got {}",
                p.initial_alpha
            );
            assert_eq!(p.alpha, p.initial_alpha, "no alphafade — alpha should equal initial_alpha before any fade operator runs");
        }
    }

    /// An instance-override alpha multiplier must apply exactly once (not
    /// squared) even when combined with `alpharandom`.
    #[test]
    fn alpha_override_multiplier_applies_once_not_twice() {
        let json = r#"{
            "maxcount": 1,
            "emitter": [{"name":"box","rate":1000}],
            "initializer": [{"id":1,"name":"alpharandom","min":1.0,"max":1.0}]
        }"#;
        let config: ParticleConfig = serde_json::from_str(json).expect("should parse");
        let overrides = InstanceOverride {
            alpha: Some(0.5),
            color: None,
            rate: None,
            size: None,
            speed: None,
        };
        let mut sys = ParticleSystem::from_config(&config, [0.0, 0.0], Some(&overrides));
        sys.step(0.01);
        assert_eq!(sys.particles[0].initial_alpha, 0.5);
        sys.step(0.01);
        // No alphafade/alphachange configured, so the plain lifetime-based
        // death-fade fallback (`initial_alpha * life/max_life`) applies —
        // barely distinguishable from 0.5 this soon after spawn, but not
        // bit-exact. The point of this assertion is that it's nowhere near
        // 0.5*0.5=0.25 (the double-multiply bug), not exact equality.
        assert!(
            (sys.particles[0].alpha - 0.5).abs() < 0.01,
            "override should apply once (~0.5), not 0.5*0.5=0.25; got {}",
            sys.particles[0].alpha
        );
    }

    /// Default (no `animationmode`) sprite-sheet playback loops continuously
    /// based on lifetime position: at half life with 4 frames, the particle
    /// should be sitting on frame 2.
    #[test]
    fn sprite_frame_loops_by_lifetime_position() {
        let config: ParticleConfig =
            serde_json::from_str(r#"{"maxcount":1,"emitter":[{"name":"box","rate":1000}]}"#)
                .expect("should parse");
        let mut sys = ParticleSystem::from_config(&config, [0.0, 0.0], None);
        sys.life_min = 10.0;
        sys.life_max = 10.0;
        sys.set_sprite_frames(4, 0.0);
        sys.step(0.001); // spawn
        assert_eq!(sys.particles.len(), 1);
        sys.step(5.0); // half of a 10s life
        assert!((sys.particles[0].frame - 2.0).abs() < 0.01);
    }

    /// `animationmode: "once"` plays through the sheet across the particle's
    /// lifetime and clamps at the last frame instead of wrapping.
    #[test]
    fn sprite_frame_clamps_at_last_frame_when_animation_mode_once() {
        let config: ParticleConfig = serde_json::from_str(
            r#"{"maxcount":1,"animationmode":"once","emitter":[{"name":"box","rate":1000}]}"#,
        )
        .expect("should parse");
        let mut sys = ParticleSystem::from_config(&config, [0.0, 0.0], None);
        sys.life_min = 10.0;
        sys.life_max = 10.0;
        sys.set_sprite_frames(4, 0.0);
        sys.step(0.001); // spawn
        sys.step(9.99); // nearly the whole 10s life
        assert_eq!(sys.particles[0].frame, 3.0);
    }

    /// `animationmode: "randomframe"` assigns one frame at spawn and freezes
    /// it — a second `step()` must not change it.
    #[test]
    fn sprite_random_frame_stays_fixed_after_assignment() {
        let config: ParticleConfig = serde_json::from_str(
            r#"{"maxcount":1,"animationmode":"randomframe","emitter":[{"name":"box","rate":1000}]}"#,
        )
        .expect("should parse");
        let mut sys = ParticleSystem::from_config(&config, [0.0, 0.0], None);
        sys.life_min = 10.0;
        sys.life_max = 10.0;
        sys.set_sprite_frames(5, 0.0);
        sys.step(0.001); // spawn (frame assignment lags by one step, like other operators)
        sys.step(0.001); // assigns a random frame
        let frame = sys.particles[0].frame;
        assert!((0.0..5.0).contains(&frame));
        sys.step(1.0);
        assert_eq!(sys.particles[0].frame, frame);
    }

    /// A sprite with only one frame should never advance `p.frame` at all —
    /// `render_onto` always draws frame 0 for it regardless.
    #[test]
    fn single_frame_sprite_leaves_frame_at_spawn_sentinel() {
        let config: ParticleConfig =
            serde_json::from_str(r#"{"maxcount":1,"emitter":[{"name":"box","rate":1000}]}"#)
                .expect("should parse");
        let mut sys = ParticleSystem::from_config(&config, [0.0, 0.0], None);
        sys.life_min = 10.0;
        sys.life_max = 10.0;
        sys.set_sprite_frames(1, 0.0);
        sys.step(0.001);
        sys.step(5.0);
        assert_eq!(sys.particles[0].frame, -1.0);
    }

    #[test]
    fn bounds_is_none_when_no_particles_alive() {
        let config: ParticleConfig =
            serde_json::from_str(r#"{"maxcount":5,"emitter":[{"name":"box","rate":0}]}"#)
                .expect("should parse");
        let sys = ParticleSystem::from_config(&config, [0.0, 0.0], None);
        assert!(sys.bounds().is_none());
    }

    #[test]
    fn bounds_covers_alive_particle_extents() {
        let json = r#"{
            "maxcount": 5,
            "emitter": [{"name":"box","rate":1,"distancemax":"0 0 0"}],
            "initializer": [
                {"id":1,"name":"lifetimerandom","min":10,"max":10},
                {"id":2,"name":"sizerandom","min":20,"max":20},
                {"id":3,"name":"velocityrandom","min":"0 0 0","max":"0 0 0"}
            ]
        }"#;
        let config: ParticleConfig = serde_json::from_str(json).expect("should parse");
        let mut sys = ParticleSystem::from_config(&config, [100.0, 100.0], None);
        sys.emitters[0].rate = 1000.0;
        sys.step(0.001);
        assert_eq!(sys.particles.len(), 1);

        let (min_x, min_y, max_x, max_y) = sys.bounds().expect("should have bounds");
        // Particle spawned at (100,100) (spread disabled via distancemax=0).
        // `sizerandom`'s value is halved at spawn (diameter → radius, see
        // the comment on that halving elsewhere in this file), so a
        // `sizerandom` of 20 gives an actual radius of 10; `bounds()` adds a
        // couple of pixels of margin on top of that.
        assert!(min_x < 100.0 - 9.0 && min_x > 100.0 - 16.0, "min_x={min_x}");
        assert!(max_x > 100.0 + 9.0 && max_x < 100.0 + 16.0, "max_x={max_x}");
        assert!(min_y < 100.0 - 9.0 && min_y > 100.0 - 16.0, "min_y={min_y}");
        assert!(max_y > 100.0 + 9.0 && max_y < 100.0 + 16.0, "max_y={max_y}");
    }

    /// `render_onto`'s `origin` param should shift particle canvas positions,
    /// letting a caller raster into a canvas sized to just the bbox instead
    /// of the full scene.
    #[test]
    fn render_onto_origin_shifts_particle_positions() {
        let json = r#"{
            "maxcount": 5,
            "emitter": [{"name":"box","rate":1}],
            "initializer": [
                {"id":1,"name":"lifetimerandom","min":10,"max":10},
                {"id":2,"name":"sizerandom","min":10,"max":10},
                {"id":3,"name":"velocityrandom","min":"0 0 0","max":"0 0 0"},
                {"id":4,"name":"alpharandom","min":1,"max":1}
            ]
        }"#;
        let config: ParticleConfig = serde_json::from_str(json).expect("should parse");
        let mut sys = ParticleSystem::from_config(&config, [50.0, 50.0], None);
        sys.emitters[0].rate = 1000.0;
        sys.step(0.001);
        assert_eq!(sys.particles.len(), 1);

        // A canvas the same size as the bbox, with origin shifted to the
        // bbox's top-left, should show the particle rendered near its
        // center rather than clipped at (0,0).
        let (min_x, min_y, max_x, max_y) = sys.bounds().unwrap();
        let w = (max_x - min_x).ceil() as u32;
        let h = (max_y - min_y).ceil() as u32;
        let mut canvas = RgbaImage::new(w, h);
        sys.render_onto(&mut canvas, None, [min_x, min_y]);

        let cx = (w / 2).min(w - 1);
        let cy = (h / 2).min(h - 1);
        assert!(
            canvas.get_pixel(cx, cy)[3] > 0,
            "expected particle to render near the shifted canvas center"
        );
    }

    #[test]
    fn oscillate_params_defaults_scale_min_from_scale_max_when_absent() {
        let op = Operator {
            name: "oscillatealpha".to_string(),
            frequencymin: Some(0.5),
            frequencymax: Some(1.0),
            scalemax: Some(10.0),
            ..Default::default()
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
            frequencymin: Some(1.0),
            frequencymax: Some(1.0),
            scalemin: Some(1.0),
            scalemax: Some(1.0),
            ..Default::default()
        };
        let params = OscillateParams::from_operator(&op);
        for _ in 0..50 {
            let s = params.sample();
            assert!((0.0..std::f32::consts::TAU).contains(&s.phase));
        }
    }
}
