# wp-engine

`wp-engine` is a Rust/Wayland Wallpaper Engine client. The scene-wallpaper
renderer is being ported from `linux-wallpaperengine` to a `wgpu` backend while
keeping the existing CPU RGBA compositor as a fallback.

## Engine Pipeline

The scene engine is split into small stages so Wallpaper Engine's original
OpenGL pass model can be rebuilt incrementally in Rust.

### 1. Workshop Content Detection

The app starts from either a Steam Workshop item or a direct file path.

- `src/workshop` scans Wallpaper Engine Workshop projects.
- `src/render/content.rs` classifies content as static image, video, or scene.
- Static and video wallpapers still flow through the frame-source path.
- Scene wallpapers are handed to the engine pipeline below.

### 2. Asset Store

Scene assets can exist as loose files or packed archives.

- `src/engine/assets.rs` mounts the wallpaper directory.
- Loose files are preferred.
- `scene.pkg` and `gifscene.pkg` are loaded as fallbacks.
- Asset paths are normalized to Wallpaper Engine's forward-slash layout.
- Textures are resolved from material-style names such as `foo` to candidates
  like `materials/foo.tex`.

This stage is the Rust equivalent of the original engine's asset/container
lookup layer.

### 3. Scene Parsing

The engine reads `scene.json` into loose, tolerant Rust structs.

- `src/engine/scene.rs` parses objects, camera data, effects, visibility,
  origins, sizes, parallax, and texture overrides.
- Fields that vary between Wallpaper Engine versions are kept as
  `serde_json::Value` until a later stage knows how to interpret them.

The goal is to avoid rejecting valid wallpapers just because a creator used a
slightly different schema.

### 4. Model, Material, And Effect Loading

The scene graph resolves object references into typed engine data.

- `src/engine/material.rs` loads model JSON files.
- Models point at material JSON files.
- Materials contain passes, shaders, texture bindings, combos, constants,
  blending, culling, and depth flags.
- Effect JSON files are loaded into effect passes and FBO declarations.

This mirrors the original engine's structure:

```text
scene object
  -> model
    -> material
      -> material pass
        -> shader + textures + constants
  -> effects
    -> effect pass
      -> material/command/source/target
```

### 5. Scene Graph

`src/engine/graph.rs` builds a resolved scene graph.

For each visible image object it records:

- the source scene object
- resolved model, if the object references one
- resolved base texture
- resolved effects
- render-target/fullscreen hints

The scene graph is still backend-neutral. It is useful for both diagnostics and
rendering.

### 6. Frame Context

`src/engine/frame_context.rs` creates the per-frame state shared by renderer
passes.

It currently tracks:

- frame index
- time and delta time
- output resolution
- camera state
- object transforms
- future effect uniform storage

Particles, audio reactivity, parallax, mouse input, and effect uniforms should
all feed through this context instead of being threaded through unrelated code.

### 7. Render Graph

`src/engine/render_graph.rs` converts the scene graph into a Wallpaper
Engine-style render plan.

It records:

- renderable scene objects
- material passes
- effect passes
- texture bindings
- render target/FBO descriptors
- copy/swap/effect pass commands
- final composite metadata
- missing-feature diagnostics

This is the bridge between Wallpaper Engine's pass model and `wgpu` execution.

### 8. Shader Translation Layer

`src/engine/shader.rs` owns shader compatibility work.

The current strategy is:

- resolve material shader names using the original convention:
  `shaders/<name>.vert` and `shaders/<name>.frag`
- run GLSL/WGSL sources through Naga
- emit WGSL when translation succeeds
- record diagnostics when translation fails
- use handwritten WGSL fallbacks for known/common passes

The translator is only one part of compatibility. Wallpaper Engine shaders also
need engine-provided uniforms, texture/sampler bindings, render targets,
preprocessor defines, and include handling.

### 9. WGPU Scene Renderer

`src/render/wgpu_scene.rs` is the first GPU vertical slice.

It currently:

- builds the scene graph and render graph
- selects the first model-backed image layer
- loads its resolved base texture
- probes the material shader through the Naga translator
- creates a `wgpu` pipeline using a built-in textured-quad WGSL fallback
- draws into an offscreen `wgpu::Texture`
- reads the texture back to RGBA for CLI PNG output

This path is intentionally separate from the live wallpaper frame loop while the
port is still young.

Run it with:

```bash
cargo run -- render-scene <workshop-id-or-directory> --gpu --output gpu_scene.png
```

### 10. CPU Fallback Renderer

`src/engine/render.rs` keeps the older CPU compositor alive.

It:

- resolves visible image layers
- decodes texture/image assets to RGBA
- resizes each layer to the output size
- overlays layers into one `RgbaImage`

Run it with:

```bash
cargo run -- render-scene <workshop-id-or-directory> --output scene_output.png
```

The CPU path is not feature-equivalent to Wallpaper Engine, but it remains useful
for smoke tests and as a stable fallback while the `wgpu` renderer grows.

### 11. Live Frame Output

`src/render/frame.rs` converts wallpaper content into a frame source.

- Static images return one shared RGBA frame.
- Videos decode frames on a background thread with FFmpeg.
- Scene wallpapers currently use the animated CPU scene loop.
- The new `wgpu` scene renderer is not yet wired into the live Wayland output.

`src/platform` takes the produced frames and presents them on Wayland surfaces.

## Current Port Status

Implemented:

- asset store for loose files and PKG archives
- tolerant `scene.json` parsing
- model/material/effect parsing
- backend-neutral scene graph
- backend-neutral render graph skeleton
- frame context skeleton
- Naga shader translation/probing
- first `wgpu` image-layer render path
- CPU fallback compositor

Still pending:

- real model/puppet mesh loading for `wgpu`
- full material pass execution
- translated shader binding layout generation
- render target/FBO allocation and pass chaining
- effect pass execution
- final composite pass
- live Wayland output from GPU textures
- particle rendering on `wgpu`
- audio-reactive uniforms

## Development Checks

Useful commands:

```bash
cargo fmt
cargo check
cargo test
```
