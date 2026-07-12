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

### 11. Live Output

Scene wallpapers render on the GPU and present **directly into the Wayland
layer-shell surface** through a `wgpu::Surface` — no CPU readback, no SHM
copy. `src/platform/wayland` creates the surface from the `wl_surface` raw
handle; if surface creation fails (or `WP_ENGINE_FORCE_SHM` is set) the
renderer falls back to RGBA readback + SHM buffers.

Static images and videos flow through `src/render/frame.rs` frame sources and
the SHM path.

### 12. Application Lifecycle

`src/application` owns the run flow (the Rust counterpart of the C++
`WallpaperApplication`): `ApplicationContext` carries the background path,
`--set-property` overrides, and shared render settings;
`WallpaperApplication::setup()/show()` resolve content, spawn the platform
renderer, and block until SIGINT/SIGTERM. `main.rs` `set`/`set-file`
delegate to it.

### 13. User Properties

`src/engine/properties.rs` loads `project.json` `general.properties`,
applies `--set-property NAME=VALUE` overrides, and rewrites `{"user": ...}`
references inside `scene.json` before parsing — the Rust counterpart of the
reference `PropertyParser`/`UserSettingParser`. `wp-engine list-properties
<id>` lists what a wallpaper exposes.

### 14. Camera Dynamics

`src/engine/camera_dynamics.rs` implements scene-level `camerashake`,
`camerafade`, and `cameraparallax` from the resolved general settings. Shake
and per-layer parallax feed the composite pass as UV offsets; fade drives
global scene opacity.

## Current Port Status

Implemented:

- asset store for loose files and PKG archives
- tolerant `scene.json` parsing with user-property resolution
- model/material/effect parsing
- backend-neutral scene graph + render graph
- GLSL→WGSL shader translation (shaderc + naga) with std140 uniform packing
- full effect pass chaining: ping-pong FBOs, named effect FBOs
  (`target`/`bind`), multi-pass effects, quarter/eighth-res bloom chain
- per-object quad placement (WE scene coordinates, Y-up)
- camera shake / fade / parallax
- direct GPU presentation to Wayland (wgpu surface on layer-shell)
- application lifecycle (`WallpaperApplication`, `--set-property`, signals)
- CPU fallback compositor + SHM fallback path

Still pending:

- real model/puppet mesh loading (bones, skinning)
- particle simulation + rendering
- text objects, lighting/shadow atlas
- mouse input → parallax/pointer uniforms (parallax rests at center for now)
- audio-reactive uniforms (FFT), sound playback
- JavaScript scene scripting (current evaluator handles simple `update()` returns)
- web (CEF) wallpapers, MPRIS media integration, X11 backend

Debugging aids:

- `wp-engine test-scene <id>` — headless animation check, dumps frames to /tmp
- `WP_ENGINE_SKIP_EFFECTS=name1,name2` — disable specific effects
- `WP_ENGINE_FORCE_SHM=1` — force the CPU/SHM presentation path

## Development Checks

Useful commands:

```bash
cargo fmt
cargo check
cargo test
```
