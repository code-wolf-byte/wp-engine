//! JavaScript evaluation for SceneScript-driven properties, backed by the
//! [`boa_engine`] interpreter.
//!
//! Wallpaper Engine lets any animated property carry a small script with a
//! `function update(value) { ... return value; }` that runs each frame. We
//! evaluate it in a real JS engine so full expressions, locals, conditionals,
//! loops and `Math.*` all work — not just the arithmetic the previous
//! hand-rolled parser handled. The exposed API mirrors what those scripts
//! reference:
//!
//! - `value` — the property's current value (also passed as the `update` arg)
//! - `engine.runtime` — seconds since the wallpaper started
//! - `engine.timeOfDay` — fractional hours in `[0, 24)`
//! - `Math.*` — the standard library (native to boa)
//! - `WEMath.smoothStep(a, b, x)` — WE's smoothstep helper
//!
//! NOTE: each call spins up a fresh [`Context`]. That is fine for evaluating
//! independent property scripts, but a per-frame render loop wants a persistent
//! context (globals, the `update` closure, and the realm reused across frames);
//! that is the next milestone and will live behind a `ScriptContext` type.

use boa_engine::{Context, Source};

pub struct ScriptEnv {
    pub runtime: f32,
    pub time_of_day: f32,
}

/// Format an `f32` as a JS numeric literal, mapping non-finite values to `0`
/// so a stray `inf`/`NaN` can't inject invalid tokens into the source.
fn js_num(v: f32) -> String {
    if v.is_finite() {
        format!("{v:?}")
    } else {
        "0".to_string()
    }
}

/// The WE scripting globals, declared ahead of the user script.
fn prelude(current_value: f32, env: &ScriptEnv) -> String {
    format!(
        "var value = {v};\n\
         var engine = {{ runtime: {r}, timeOfDay: {t} }};\n\
         var WEMath = {{ smoothStep: function(a, b, x) {{ \
             var t = Math.min(Math.max((x - a) / (b - a), 0), 1); \
             return t * t * (3 - 2 * t); \
         }} }};\n",
        v = js_num(current_value),
        r = js_num(env.runtime),
        t = js_num(env.time_of_day),
    )
}

/// Run a SceneScript `update(value)` and return its numeric result, or `None`
/// if the script has no `update`, throws, or doesn't yield a finite number —
/// callers then keep the property's current value.
pub fn eval_update(script: &str, current_value: f32, env: &ScriptEnv) -> Option<f32> {
    let source = format!(
        "{prelude}\n{script}\n; (typeof update === 'function') ? update(value) : value;",
        prelude = prelude(current_value, env),
    );

    let mut context = Context::default();
    let result = context.eval(Source::from_bytes(source.as_bytes())).ok()?;
    let n = result.to_number(&mut context).ok()?;
    n.is_finite().then_some(n as f32)
}

/// A persistent JS runtime for the render loop: the realm, intrinsics, and the
/// WE globals (`engine`, `WEMath`, `value`, `update`) are created once and
/// reused across frames, so a per-frame tick only mutates `engine.*` and runs
/// each property's `update`. This avoids rebuilding the entire JS realm on
/// every property, every frame (what the free [`eval_update`] does).
pub struct ScriptContext {
    context: Context,
}

impl Default for ScriptContext {
    fn default() -> Self {
        Self::new()
    }
}

impl ScriptContext {
    pub fn new() -> Self {
        let mut context = Context::default();
        // Declare the globals once. `value`/`update` are reset per evaluation;
        // `engine`/`WEMath` persist and are mutated by `set_time`.
        let _ = context.eval(Source::from_bytes(
            "var value;\n\
             var update;\n\
             var engine = { runtime: 0, timeOfDay: 0 };\n\
             var WEMath = { smoothStep: function(a, b, x) { \
                 var t = Math.min(Math.max((x - a) / (b - a), 0), 1); \
                 return t * t * (3 - 2 * t); \
             } };",
        ));
        Self { context }
    }

    /// Update the clock globals for the current frame. Call once per frame
    /// before evaluating this frame's property scripts.
    pub fn set_time(&mut self, runtime: f32, time_of_day: f32) {
        let src = format!(
            "engine.runtime = {}; engine.timeOfDay = {};",
            js_num(runtime),
            js_num(time_of_day),
        );
        let _ = self.context.eval(Source::from_bytes(src.as_bytes()));
    }

    /// Evaluate one property's `update(value)` against the persistent context,
    /// returning its finite numeric result or `None` (no `update`, threw, or
    /// non-finite) so the caller keeps the property's current value.
    ///
    /// `update` is reset before the user script runs so a previous property's
    /// leftover `update` can't leak into a script that doesn't define its own.
    pub fn eval_update(&mut self, script: &str, current_value: f32) -> Option<f32> {
        // Reset in its own program: a function-declaration hoist in the user
        // script must win, which it can't if the reset shared the same program.
        let _ = self.context.eval(Source::from_bytes(b"update = undefined;"));
        let src = format!(
            "value = {v};\n{script}\n; (typeof update === 'function') ? update(value) : value;",
            v = js_num(current_value),
        );
        let result = self.context.eval(Source::from_bytes(src.as_bytes())).ok()?;
        let n = result.to_number(&mut self.context).ok()?;
        n.is_finite().then_some(n as f32)
    }
}

pub fn eval_script_opt(script: Option<&str>, current_value: f32, env: &ScriptEnv) -> f32 {
    match script {
        Some(s) => eval_update(s, current_value, env).unwrap_or(current_value),
        None => current_value,
    }
}

pub fn eval_vec3_script_opt(
    animated: &crate::engine::model::AnimatedValue,
    env: &ScriptEnv,
) -> [f32; 3] {
    // Default to [1.0; 3] so scale and color don't zero-out the output when missing
    let base = animated.as_vec3().unwrap_or([1.0; 3]);
    match &animated.script {
        None => base,
        Some(script) => [
            eval_update(script, base[0], env).unwrap_or(base[0]),
            eval_update(script, base[1], env).unwrap_or(base[1]),
            eval_update(script, base[2], env).unwrap_or(base[2]),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(runtime: f32, time_of_day: f32) -> ScriptEnv {
        ScriptEnv {
            runtime,
            time_of_day,
        }
    }

    #[test]
    fn returns_current_value_when_no_update_function() {
        let out = eval_update("var x = 3;", 7.0, &env(0.0, 0.0));
        assert_eq!(out, Some(7.0));
    }

    #[test]
    fn simple_arithmetic_update() {
        let script = "function update(value) { return value * 2 + 1; }";
        assert_eq!(eval_update(script, 5.0, &env(0.0, 0.0)), Some(11.0));
    }

    #[test]
    fn reads_engine_runtime() {
        let script = "function update(value) { return engine.runtime; }";
        assert_eq!(eval_update(script, 0.0, &env(4.5, 0.0)), Some(4.5));
    }

    #[test]
    fn multi_statement_body_with_locals_and_math() {
        // The old hand-rolled parser could only handle a single `return <expr>`.
        let script = "function update(value) {\n\
                          var a = Math.floor(value);\n\
                          var b = Math.sin(0) + 1;\n\
                          return a * b;\n\
                      }";
        assert_eq!(eval_update(script, 3.7, &env(0.0, 0.0)), Some(3.0));
    }

    #[test]
    fn conditional_and_loop() {
        let script = "function update(value) {\n\
                          var sum = 0;\n\
                          for (var i = 0; i < 4; i++) { sum += i; }\n\
                          return value > 0 ? sum : -sum;\n\
                      }";
        assert_eq!(eval_update(script, 1.0, &env(0.0, 0.0)), Some(6.0));
        assert_eq!(eval_update(script, -1.0, &env(0.0, 0.0)), Some(-6.0));
    }

    #[test]
    fn wemath_smoothstep_matches_definition() {
        let script = "function update(value) { return WEMath.smoothStep(0, 1, value); }";
        // midpoint of smoothstep is exactly 0.5
        assert_eq!(eval_update(script, 0.5, &env(0.0, 0.0)), Some(0.5));
        // clamps below/above the edges
        assert_eq!(eval_update(script, -1.0, &env(0.0, 0.0)), Some(0.0));
        assert_eq!(eval_update(script, 2.0, &env(0.0, 0.0)), Some(1.0));
    }

    #[test]
    fn throwing_script_falls_back_to_current_value() {
        let script = "function update(value) { return nonexistent.field; }";
        assert_eq!(eval_update(script, 9.0, &env(0.0, 0.0)), None);
        assert_eq!(eval_script_opt(Some(script), 9.0, &env(0.0, 0.0)), 9.0);
    }

    #[test]
    fn persistent_context_reevaluates_across_frames() {
        let mut ctx = ScriptContext::new();
        let script = "function update(value) { return engine.runtime * 2; }";

        ctx.set_time(1.0, 0.0);
        assert_eq!(ctx.eval_update(script, 0.0), Some(2.0));

        // Same context, new frame: only the clock changed, result tracks it.
        ctx.set_time(5.5, 0.0);
        assert_eq!(ctx.eval_update(script, 0.0), Some(11.0));
    }

    #[test]
    fn persistent_context_passes_current_value_through() {
        let mut ctx = ScriptContext::new();
        let script = "function update(value) { return value + 1; }";
        assert_eq!(ctx.eval_update(script, 4.0), Some(5.0));
        assert_eq!(ctx.eval_update(script, 40.0), Some(41.0));
    }

    #[test]
    fn persistent_context_does_not_leak_update_between_scripts() {
        let mut ctx = ScriptContext::new();
        // First property defines an `update`.
        assert_eq!(
            ctx.eval_update("function update(value) { return 99; }", 1.0),
            Some(99.0)
        );
        // Second property has no `update`: it must keep its own value, not
        // reuse the leftover `update` from the first property.
        assert_eq!(ctx.eval_update("var unrelated = 3;", 7.0), Some(7.0));
    }

    #[test]
    fn persistent_context_survives_a_throwing_script() {
        let mut ctx = ScriptContext::new();
        assert_eq!(ctx.eval_update("function update(v){ return boom.x; }", 2.0), None);
        // The context is still usable afterwards.
        assert_eq!(ctx.eval_update("function update(v){ return v * 10; }", 2.0), Some(20.0));
    }
}
