# wp-engine

`wp-engine` is a Rust Wallpaper Engine client. The scene-wallpaper renderer is
ported from [linux-wallpaperengine](https://github.com/Almamu/linux-wallpaperengine)
to a `wgpu` backend, keeping a CPU RGBA compositor as a fallback.

It presents on Wayland (wlr-layer-shell), X11 (root pixmap), and macOS, and
renders scene, video, image, and web wallpapers. Windows is out of scope — see
**Current Port Status**.

> This project stands on
> [Almamu/linux-wallpaperengine](https://github.com/Almamu/linux-wallpaperengine).
> A great deal of the maths here — the pass model, particle simulation, texture
> and model formats, camera behaviour, and shader translation — is based on the
> work done in that project. See [Credits](#credits).
>
> Licensed under **GPL-3.0**, the same licence as the project it derives from.

## Engine Pipeline

The scene engine is split into small stages so Wallpaper Engine's original
OpenGL pass model can be rebuilt incrementally in Rust.

### 1. Workshop Content Detection

The app starts from either a Steam Workshop item or a direct file path.

- `src/workshop` scans Wallpaper Engine Workshop projects.
- `src/render/content.rs` classifies content as static image, video, scene, or
  web.
- Static, video, and web wallpapers flow through the frame-source path.
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

For each visible object it records:

- the source scene object
- resolved model, if the object references one
- resolved base texture
- resolved effects
- render-target/fullscreen hints
- parent chain and transforms
- the draw order (`order_index`) both render paths share, so images, particles,
  and meshes interleave in true scene z-order rather than by category

The scene graph is backend-neutral, and useful for both diagnostics and
rendering.

### 6. Shader Translation Layer

`src/engine/shaders/transpiler.rs` turns Wallpaper Engine's GLSL into WGSL:
resolve `shaders/<name>.vert|frag`, expand includes and combo `#define`s,
compile to SPIR-V with shaderc, then translate to WGSL with Naga.

Real workshop shaders lean on legacy NVIDIA GLSL leniency, so the translator
also repairs them: vector-width coercion, `for`-loop unrolling ahead of array
varying expansion, `%` to `mod`, int literals to floats in `min`/`max`/`clamp`,
and float-to-int assignment casts. Engine uniforms (`g_Time`,
`g_PointerPosition`, the `g_AudioSpectrum*` arrays, …) are packed to std140 and
bound alongside.

`WP_DEBUG_DUMP_GLSL=1` writes the generated GLSL of any shader that fails to
compile — shaderc's line numbers refer to that text and nothing else prints it.
`WP_DEBUG_DUMP_WGSL=1` dumps each pass's translated WGSL.

### 7. GPU Scene Renderer

`src/engine/gpu_renderer.rs` is the production renderer — `GpuSceneInstance` and
`gpu_scene_render_loop`, used by `set`, `set-file`, and `test-scene`.

It owns the full pass model: per-object composite buffers, ping-pong FBOs,
named effect targets (`target`/`bind`), multi-pass effects, the reference's
bloom chain, particle systems, puppet skinning, 3D meshes for perspective
scenes, text layers, and SceneScript-driven properties ticked per frame.

`WP_DEBUG_DUMP_FBOS=1` dumps every pooled render target per frame;
`WP_ENGINE_SKIP_EFFECTS=name,name` disables effects by name;
`WP_DEBUG_DUMP_FRAME=path` saves the last GPU frame headlessly, which is the
only way to inspect the live path off-screen.

### 8. Web Wallpapers

`src/render/web.rs` renders HTML wallpapers with an embedded Chromium through
CEF, off-screen: each painted frame becomes an `RgbaImage` on the same channel
video and CPU scene rendering use, so every presentation path works unchanged.

It also injects the Wallpaper Engine browser API that pages expect —
`wallpaperPropertyListener.applyUserProperties` fed from `project.json`, and
`wallpaperRegisterAudioListener` fed from live capture as 64 left plus 64 right
bands. Behind the off-by-default `web` feature; see **Building**.

### 9. CPU Fallback Renderer

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

### 10. Live Output

`src/platform/display.rs` picks a backend at runtime: Wayland when
`WAYLAND_DISPLAY`/`WAYLAND_SOCKET` is set, else X11 when `DISPLAY` is, else
macOS. `WP_ENGINE_FORCE_X11=1` takes the X11 path even under a Wayland session.

On **Wayland**, scene wallpapers render on the GPU and present *directly into
the layer-shell surface* through a `wgpu::Surface` — no CPU readback, no SHM
copy. `src/platform/wayland` creates the surface from the `wl_surface` raw
handle; if that fails (or `WP_ENGINE_FORCE_SHM` is set) it falls back to RGBA
readback + SHM buffers. The same module feeds the real cursor position into
camera parallax and `g_PointerPosition`.

On **X11**, `src/platform/x11` draws into the root window's background pixmap
and publishes `_XROOTPMAP_ID`/`ESETROOT_PMAP_ID`, the convention every WM and
compositor honours (it is what `feh --bg` sets). There is no direct-presentation
equivalent, so every frame is read back and pushed with `PutImage`. X11 gets
better parallax for free: `QueryPointer` reports the global cursor regardless of
which window has focus.

Static images, videos, and web wallpapers flow through `src/render/frame.rs`
frame sources.

### 11. Application Lifecycle

`src/application` owns the run flow (the Rust counterpart of the C++
`WallpaperApplication`): `ApplicationContext` carries the background path,
`--set-property` overrides, and shared render settings;
`WallpaperApplication::setup()/show()` resolve content, spawn the platform
renderer, and block until SIGINT/SIGTERM. `main.rs` `set`/`set-file`
delegate to it.

### 12. User Properties

`src/engine/properties.rs` loads `project.json` `general.properties`,
applies `--set-property NAME=VALUE` overrides, and rewrites `{"user": ...}`
references inside `scene.json` before parsing — the Rust counterpart of the
reference `PropertyParser`/`UserSettingParser`. `wp-engine list-properties
<id>` lists what a wallpaper exposes.

### 13. Camera Dynamics

`src/engine/camera_dynamics.rs` implements scene-level `camerashake`,
`camerafade`, and `cameraparallax` from the resolved general settings. Shake
and per-layer parallax feed the composite pass as UV offsets; fade drives
global scene opacity.

## Current Port Status

Implemented:

- asset store for loose files and PKG archives
- tolerant `scene.json` parsing with user-property resolution
- model/material/effect parsing
- GLSL→WGSL shader translation (shaderc + Naga) with std140 uniform packing and
  the legacy-GLSL repair passes real workshop shaders need
- full effect pass chaining: ping-pong FBOs, named effect FBOs (`target`/`bind`),
  multi-pass effects, per-object composite buffers, reference bloom chain
- per-object quad placement (WE scene coordinates, Y-up), layer alignment,
  parent-chain transforms, true z-order interleaving
- particles: every emitter/initializer/operator/renderer type the corpus uses,
  including rope/trail renderers, textured sprites, child systems, and
  audio-reactive emission
- puppet `.mdl` mesh loading with bones, skinning, and animation layers
- text objects: system + bundled fonts, wrapping, alignment, backgrounds
- light and sound objects
- SceneScript property scripting (text, alpha, visible, scale, origin, angles)
- audio reactivity: desktop capture → FFT → `g_AudioSpectrum*` uniforms
- camera shake / fade / parallax, driven by the real cursor
- 3D perspective scenes with meshes
- video wallpapers, and embedded-video `.tex` layers, streamed with FFmpeg
- web (HTML) wallpapers via CEF, with the WE browser API bridge
  (`wallpaperPropertyListener`, `wallpaperRegisterAudioListener`)
- presentation: Wayland layer-shell (direct `wgpu::Surface`, no readback),
  X11 root pixmap, macOS desktop window, CPU/SHM fallback everywhere
- application lifecycle (`WallpaperApplication`, `--set-property`, signals)

Still pending:

- **screen-space backdrop capture** for refraction/distortion effects
  (waterripple/waterflow): their hidden `g_Texture0` is meant to sample the scene
  behind the layer, but we default it to the layer's own base texture, so water
  effects distort themselves. Currently softened with a brightness band-aid.
- MPRIS media integration, and the media/plugin halves of the web JS bridge
- mouse/keyboard input forwarding into web wallpapers (they render, but do not
  respond to the cursor); web wallpapers also render at a fixed 1080p
- per-object cameras (3 wallpapers in the local corpus)
- normal-mapped lighting, shadows, volumetrics, GPU instancing, and a true depth
  buffer — all measured against the corpus and found to have zero or near-zero
  reach, so they are deliberately not planned rather than merely missing

Out of scope:

- **Windows.** The renderer is portable (wgpu), but there is no `WorkerW`
  platform module and no way to test one from the Linux/macOS development
  environment. `application` (Windows-only `.exe`) wallpapers likewise.

Debugging aids:

- `wp-engine test-scene <id>` — headless animation check
- `WP_DEBUG_DUMP_FRAME=path` — save the last GPU frame (the only way to inspect
  the live path off-screen)
- `WP_DEBUG_DUMP_FBOS=1` — dump every pooled render target per frame
- `WP_DEBUG_DUMP_WGSL=1` / `WP_DEBUG_DUMP_GLSL=1` — dump translated WGSL, or the
  generated GLSL of any shader that fails to compile
- `WP_DEBUG_TEX_SLOTS=1` — dump effect texture-slot resolution
- `WP_DEBUG_PARTICLE_TIMING=1` — per-system particle cost, for "why is this slow"
- `WP_ENGINE_SKIP_EFFECTS=name1,name2` — disable specific effects. Note the
  hardcoded kernels (pulse/scroll/shake/tint/opacity/waterripple/waterwaves/spin)
  never log a load line, so read the object's effect list from `scene.json`
  rather than trusting the log when bisecting.
- `WP_ENGINE_FORCE_SHM=1` — force the CPU/SHM presentation path
- `WP_ENGINE_FORCE_X11=1` — take the X11 path even under a Wayland session

## Building

Two native libraries must be installed first — `cargo` links against them, it
can't fetch them like a crate:

- **shaderc** — GLSL→SPIR-V for the shader translation layer.
  - Linux: `apt install libshaderc-dev` (or `pacman -S shaderc`). Found
    automatically on the default search path.
  - macOS: `brew install shaderc`. Homebrew's prefix isn't on `shaderc-sys`'s
    search path, so point the build at it with `export
    SHADERC_LIB_DIR=/opt/homebrew/lib`, or add it to a local
    `.cargo/config.toml` `[env]` block. Keep that file out of version control —
    a committed global path would break Linux builds.
- **FFmpeg** — video-wallpaper decoding: `apt install ffmpeg` / `brew install
  ffmpeg`. Version 9 or newer; `ffmpeg-next` is pinned to match, and older
  system FFmpeg will fail to build the bindings.

### Web wallpapers (optional)

Web (HTML) wallpapers need an embedded Chromium and are behind an off-by-default
cargo feature:

```bash
cargo build --features web
```

The first such build downloads the CEF binary distribution (~400 MB extracted)
and places `libcef.so`, Chromium's `.pak` resource blobs, and `locales/` beside
the built binary; `build.rs` adds an `$ORIGIN` rpath so it finds them. Set
`CEF_PATH` to reuse an existing CEF distribution instead of downloading one.

Keep the feature off unless you need web wallpapers — the default build touches
none of this. Without it, web wallpapers report that the binary lacks web
support rather than failing obscurely.

## Development Checks

Useful commands:

```bash
cargo fmt
cargo check
cargo test
```

## Credits

This project would not exist without
**[Almamu/linux-wallpaperengine](https://github.com/Almamu/linux-wallpaperengine)**
— Almamu's C++ reverse-engineering of Wallpaper Engine's scene format and
renderer.

A great deal of the maths in this codebase is based on the work done there. The
formats and formulae were derived by that project first, and this port follows
them:

- the pass model: material and effect passes, ping-pong FBOs, named render
  targets, and the bloom chain
- particle simulation — emitter shapes, initialisers, and the operator formulae
  (`alphafade`, `sizechange`, the oscillators, `colorchange`,
  `controlpointattract`) follow `CParticle.cpp`
- the `.tex` container format, the `.mdl` mesh layout, and the texture/model
  chain resolution rules
- scene coordinates, layer alignment and quad placement (`CImage.cpp`), and text
  layout (`CText.cpp`)
- camera parallax and fade, and the pointer-position convention
- shader translation: uniform naming, combo handling, and the binding rules
  `CPass.cpp` establishes

Some parts went further than the reference and are original work here, informed
by its groundwork rather than ported from it: the puppet `.mdl` skeleton and
animation tracks (the reference reads only the mesh, and rejects the format
version these files use), rope/trail particle geometry, and the Wallpaper Engine
JavaScript bridge for web wallpapers.

Where this port deviates deliberately, the reason is recorded in a comment next
to the code (camera shake, for instance, is applied here even though the
reference parses but never applies it).

The reference implementation is not redistributed here — `cpp-implementation/`
is gitignored, and is a local checkout used as read-only reference material
while porting. Clone it yourself from the link above if you want to compare.

Wallpaper Engine itself is a product of Kristjan Skutta. This project is not
affiliated with, endorsed by, or derived from its source.

## License

Licensed under the **GNU General Public License v3.0** — see [LICENSE](LICENSE).

`linux-wallpaperengine`, which this project derives from and whose maths it is
based on, is GPL-3.0. As a derivative work this project carries the same licence.
Upstream ships the GPL-3 text with no "or any later version" grant, so this is
GPL-3.0-only rather than -or-later.
