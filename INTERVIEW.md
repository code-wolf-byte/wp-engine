# Interview Questions

Questions an interviewer could ask about wp-engine, plus general system-design
questions the project touches. The project-specific ones have answer pointers
into the codebase; the design ones are open-ended.

## Project-specific

### Architecture & rendering
1. **Walk me through what happens when a user runs `wp-engine set <id>`.**
   → `main::cmd_set` → `ApplicationContext` → `WallpaperApplication::setup` →
   `WallpaperContent::from_any_path` (classify) → `detect_platform().spawn_wallpaper`
   → a dedicated render thread; the main thread waits on SIGINT/SIGTERM.

2. **Why is there both a CPU compositor and a wgpu renderer?** The wgpu path is
   the real port target; the CPU compositor (`engine::render`) is a stable
   fallback and smoke-test path while the GPU port grows. They're kept separate
   deliberately.

3. **How does a scene wallpaper get from `scene.json` to pixels?** assets →
   properties → tolerant scene parse → model/material/effect → backend-neutral
   scene graph → render graph → shader translation → `wgpu_scene` / GPU renderer.
   Per-frame state is centralized in `frame_context`.

4. **How are effects chained on the GPU?** Per-layer ping-pong FBOs sized to the
   object (not the scene), named effect FBOs (`target`/`bind`), then composite
   with blend mode + camera-dynamics UV offset, then an optional bloom chain over
   quarter/eighth-res buffers. See `gpu_renderer::render`.

5. **Why keep unknown `scene.json` fields as `serde_json::Value`?** Tolerant
   parsing — WE's schema varies by version; rejecting on schema drift would break
   valid wallpapers. Fields are interpreted only when a later stage understands
   them.

5b. **A layer says `visible:true` but Wallpaper Engine hides it — why, and how do
   you honor that?** WE hides a whole subtree when any ancestor is hidden, so
   visibility is a property of the `parent` chain, not the single object. We fold
   it into one `Scene::visibility_mask()` (own AND every ancestor) that every cull
   site indexes, rather than re-walking the tree at each of the four cull sites.
   Combos (e.g. a `language` selector) set the *group's* `visible`; the mask
   propagates that to its children.

### Shader translation
6. **Why go GLSL → shaderc → SPIR-V → naga instead of naga's GLSL frontend
   directly?** naga's `glsl-in` is too strict for WE's HLSL-flavored dialect;
   shaderc (glslang) is far more tolerant. naga is used only for SPIR-V → WGSL.

7. **How does the transpiler survive WE's lenient type rules?** A chain of named
   text-normalization passes (unroll array varyings/loops, coerce int/swizzle
   args) before compile, then an error-message-driven repair loop that parses
   shaderc's "cannot convert" errors and inserts the implicit conversion.

8. **How are texture bindings assigned, and why not by declaration order?** By the
   number in the uniform name (`g_TextureN`), because `#if`-stripped samplers
   would otherwise shift every later binding and sample the wrong slot.

9. **What are the fallback layers when a shader won't translate?** real VS →
   synthetic passthrough VS → skip the pass (logged, not fatal). Eight hot
   effects use handwritten WGSL kernels instead of translation.

### Platform layer
10. **What's the difference between `platform/mod.rs` and
    `platform/wayland/mod.rs`?** The former is the platform-neutral facade + GPU
    adapter abstraction shared by all backends; the latter is the concrete
    Wayland presentation backend implementing `DisplayPlatform`.

11. **How does the live path avoid a GPU→CPU readback?** Scenes render straight
    into a `wgpu::Surface` created from the raw `wl_surface` handle and
    `present()`; SHM readback is only the fallback. `render_to_view` does the
    aspect-fill blit.

12. **How is animation paced?** Wayland `wl_surface.frame` callbacks — request a
    callback before commit; the compositor fires it when the frame is presented,
    which drives the next draw (throttled to refresh rate).

13. **How would you add macOS support?** New `MacOSPlatform : DisplayPlatform`
    returned by `detect_platform`; winit window pinned at desktop level
    (`kCGDesktopWindowLevel`, all-Spaces/stationary via objc2-app-kit), wgpu Metal
    surface, same no-readback `render_to_view` loop.

### Build / correctness
14. **How does the project handle its C/C++ native deps across platforms?**
    shaderc + FFmpeg are system libs (documented per-platform); Wayland deps are
    target-gated to Linux; macOS deps target-gated to macOS. See DECISIONS.md.

15. **How does `test-scene` decide whether a wallpaper animates?** Collects N
    frames headlessly and takes the *max* per-pixel change of frame[0] vs every
    later frame (max, not first-vs-last, to avoid periodic-effect false negatives).

## System design (open-ended)

1. **Design a cross-platform desktop wallpaper engine.** Content types
   (image/video/scene/web), per-monitor surfaces, GPU vs CPU rendering, the OS
   window/surface abstraction, lifecycle & clean shutdown, power/idle behavior.

2. **Design a shader compatibility layer** that runs one engine's shaders on a
   different graphics API. Dialect differences, preprocessor/includes, uniform
   packing (std140), error recovery, fallbacks, and how you'd measure coverage.

3. **Design the platform abstraction for a renderer** that must target Wayland,
   X11, macOS, and Windows. Where's the seam? What's shared vs per-backend? How do
   you keep the core backend-neutral (see the `DisplayPlatform` trait)?

4. **How would you present GPU frames with zero CPU readback**, and what's the
   fallback when direct presentation isn't available? Discuss swapchain/surface
   ownership, format negotiation, and lifetime/ordering hazards (e.g. destroying a
   surface before its display connection).

5. **Design multi-monitor support:** independent surfaces, per-output resolution
   and scale, hotplug (add/remove displays), and aspect-fit vs aspect-fill.

6. **How would you make GPU shader translation robust at scale** across a corpus
   of thousands of third-party shaders you don't control? Batch validation,
   diagnostics, graceful degradation, and regression tracking.

7. **Budget the frame:** where does time go in a scene wallpaper (texture upload,
   effect passes, bloom, present), and how do you keep it light enough to run as a
   always-on background process (quality scaling, low-power GPU selection)?

8. **Design the dependency/build strategy** for a Rust project that wraps native
   C/C++ libraries (shaderc, FFmpeg, libwayland) and must build on several OSes.
   Target-gating, prebuilt vs from-source, and reproducibility.
