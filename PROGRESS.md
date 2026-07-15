# Progress

Status of the macOS-compatibility work. Engine-feature progress (particles,
puppets, audio, etc.) lives in README.md's "Current Port Status".

## Done

- [x] Flattened `src/platform/macos/mod.rs` (removed the double-nested inner
      `pub mod macos`).
- [x] Gated the Wayland dependency stack to Linux in `Cargo.toml`
      (`[target.'cfg(target_os = "linux")'.dependencies]`); kept
      `raw-window-handle` shared.
- [x] Gated the `wayland` module (`platform/mod.rs`) and the `WaylandPlatform`
      branch of `detect_platform` (`display.rs`) to `#[cfg(target_os = "linux")]`.
- [x] shaderc on macOS: `brew install shaderc` + `SHADERC_LIB_DIR` wired in a
      gitignored `.cargo/config.toml`. Documented in README + CLAUDE.md.
- [x] Whole crate (lib + both bins) compiles on macOS — `cargo check` green.
- [x] `MacOSPlatform` implements `DisplayPlatform`, returned by `detect_platform()`
      on macOS. Because winit needs the main thread, `spawn_wallpaper` errors and
      the wallpaper runs via `run_wallpaper_on_main` (see DECISIONS.md).
- [x] Pinned macOS deps to winit's objc2 line: `winit 0.30`, `objc2 0.5`,
      `objc2-app-kit 0.2` (see DECISIONS.md).
- [x] Enabled the wgpu `metal` feature; `wp-engine probe` opens Apple M1 (Metal).
- [x] **Step 2 done + validated on Apple M1:** `wp-engine set-file <scene-dir>`
      opens a winit window rendering the scene via a wgpu Metal surface, no CPU
      readback (`GpuSceneInstance::render_to_view`). Confirmed visually (cat scene
      animating) and headless (`test-scene`, 99.9% pixels changed).
- [x] Wired into the app lifecycle: `WallpaperApplication::show()`/`setup()` have a
      `#[cfg(target_os = "macos")]` branch that runs `run_wallpaper_on_main` on the
      main thread; the Linux SIGINT sleep-poll path is `#[cfg(not(macos))]`-gated.
- [x] Docs: DECISIONS.md, INTERVIEW.md, PROGRESS.md.
- [x] **Desktop-level placement:** `pin_to_desktop` sets `NSWindow` level to
      `kCGDesktopWindowLevel`, `collectionBehavior = CanJoinAllSpaces | Stationary`,
      borderless, `setIgnoresMouseEvents(true)`. Renders behind app windows on the
      live desktop (Apple M1).
- [x] **GUI lists wallpapers on macOS:** `find_steam_library_roots`
      (`src/workshop/mod.rs`) now also checks `~/Library/Application Support/Steam`
      (existence-filtered, inert on Linux). `wp-engine` with no args opens the egui
      browser populated with installed Workshop items.
- [x] **Consolidated the render module:** `render_TRS.rs` (the live module, wired
      via a `#[path]` override) is now just `src/engine/render.rs`; the old
      shadowed `render.rs` is gone.

## Next

- [ ] SIGINT/SIGTERM → exit the winit loop cleanly (check a stop flag in
      `about_to_wait`); today only the window's close button ends it.
- [ ] **Window still takes focus / isn't click-through:** it receives
      `Focused(true)` + `CursorMoved` despite `setIgnoresMouseEvents(true)` (winit
      re-activates it ~1s after `resumed()`, after `pin_to_desktop`). So it can
      become key and be quit (Cmd+Q → `CloseRequested` → clean exit). Fix: set
      `NSApp` activation policy `.accessory` and re-assert level/ignore-mouse after
      winit finishes (re-pin on first `Focused`/`Resized`, not only in `resumed`).

## Not a macOS bug — pre-existing engine gaps (found while testing)

Testing the cat scene surfaced rendering issues that reproduce in the **headless**
render (`test-scene`), so they are engine-side and independent of the macOS window:

- **Grey box (FIXED):** full-screen effect-only overlay layers (no image + an
  effect, e.g. "Light shafts - linear") hit `render.rs::placeholder_for`, which
  substituted an **opaque** grey quad (`Rgba(140,140,140,255)`) — a giant grey box
  covering the scene when the effect is a SKIP or an additive overlay. Fixed by
  making the placeholder transparent (`Rgba(0,0,0,0)`). Proper backdrop capture
  (so screen-space effects can sample the real scene behind them) is still not
  implemented. (`composelayer.json` layers are separately skipped via
  `is_special_layer`, so they weren't the culprit here.)
- **Stray white sparkles:** some particle sprites (`fireflies`/`halo`/`shootingstar`)
  fail texture resolution → untextured white-circle fallback.
- These belong on the engine-features track (README "Still pending"), not this branch.

## Ripple / water refraction (next up — engine, not macOS)

The LonelyCat water-ripple layer (`ripple1440p`, id 88) renders as an opaque white
blob instead of translucent ripples over the scene. Fully diagnosed:

- **Blend/POT are innocent.** Composite math (mode 9 = Add, `s.a * opacity`) is
  faithful; effect FBOs are sized to `object_size`, textures cropped to image dims,
  UVs normalized (POT hypothesis refuted).
- **Root cause:** `waterripple`/`waterflow` declare `g_Texture0` as `null` +
  `{"hidden":true}` — WE's convention for *"sample the scene framebuffer behind
  this layer"* (the backdrop). They sample a normal map (`g_Texture2`), displace
  the UV, then sample `g_Texture0` at the displaced coord. Our effect executor
  (`gpu_renderer.rs`, the `pass.binds` loop, `src_view = chain_view`) resolves a
  null bind-0 to the **chain source = the layer's own base texture**, so the water
  refracts itself and, under additive blend, blows out.
- **Backdrop machinery already exists:** `gpu_renderer.rs` snapshots
  `self.target → self.scene_copy` for passes that name `_rt_FullFrameBuffer` — but
  these water passes use `null`, so it never fires.
- **Tried and reverted (important):** routing null bind-0 for water effects to the
  `scene_copy` snapshot made it **worse** — the whole screen went white. The layer
  is full-frame (2560×1440) and composites **additively**, so "refracted whole
  scene" gets *added on top of* the scene → ~2× brightness everywhere. Backdrop
  capture must be **coupled with a replace (normal) composite** for that layer: the
  refracted backdrop *stands in for* the scene, it doesn't add to it. Do both or
  neither.
- **Current band-aid:** `WP_ENGINE_ADDITIVE_BRIGHTNESS` (default 0.55) scales
  additive image layers down in `render.rs::layer_from_object`, so the ripple
  structure stays visible and roughly matches WE's reference. Remove once real
  backdrop capture + replace-composite lands.

## Later: make it an actual wallpaper

- [ ] `NSApp` activation policy `.accessory` + non-activating window (see "Next").
- [ ] Multi-monitor: one window per `NSScreen`.
- [ ] Content routing: `Scene` → GPU surface; `Static`/`Video` → decide present
      path (surface blit vs a CPU fallback).

## Later

- [ ] Test on a real macOS desktop (window actually sits behind app windows, all
      Spaces, survives display hotplug).
- [ ] CI job for macOS (`brew install shaderc ffmpeg`, `cargo test`).
- [ ] Revisit whether `Static`/`Video` need the SHM-equivalent fallback on macOS
      or can always go through the surface.
