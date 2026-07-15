# Decisions

Architecture/engineering decisions for the macOS-compatibility work (and the
port in general). Newest first. Each entry: context → decision → why.

## macOS backend

### The winit event loop runs on the main thread
- **Context:** we initially planned to keep `spawn_wallpaper` symmetric with the
  Wayland backend — run the loop on a spawned thread via winit's
  `with_any_thread` — so the main thread could park in
  `WallpaperApplication::show()` polling for signals.
- **Finding:** `with_any_thread` does **not** exist for macOS in winit 0.30 (only
  X11/Windows). AppKit requires the event loop on the process main thread and
  winit enforces it; no winit version offers an off-main-thread loop on macOS.
- **Decision:** run the macOS wallpaper on the main thread via
  `platform::macos::run_wallpaper_on_main`. `MacOSPlatform::spawn_wallpaper` can't
  express "take over the main thread" (and `WallpaperHandleInner: Send` can't hold
  the `!Send` `EventLoop`), so it errors and points at the main-thread entry; the
  app lifecycle calls `run_wallpaper_on_main` directly on macOS.
- **Consequence:** `WallpaperApplication::show()` needs a small
  `#[cfg(target_os = "macos")]` branch (run the loop on main instead of the
  sleep-poll). That's the one unavoidable touch to the shared lifecycle.

### Platform selection goes through the existing `DisplayPlatform` trait
- **Context:** wp-engine already abstracts "put a wallpaper on screen" behind
  `platform::display::DisplayPlatform` (implemented by `WaylandPlatform`), with
  `detect_platform()` choosing at runtime.
- **Decision:** the macOS backend is a new `MacOSPlatform` implementing that same
  trait; `detect_platform()` returns it under `#[cfg(target_os = "macos")]`.
- **Why:** no new abstraction — the seam already exists. `application` and the
  rest of the engine stay platform-agnostic. Mirrors the Wayland path exactly.

### `detect_platform()` error tail is scoped to non-macOS
- **Decision:** the "no Wayland compositor found" error+`exit(1)` block is
  `#[cfg(not(target_os = "macos"))]`; the macOS branch `return`s before it.
- **Why:** avoids an unreachable-code warning on macOS and stops the misleading
  "requires a Wayland compositor / X11 unsupported" message from firing there.

### `macos/mod.rs` is flat, not a nested `pub mod macos`
- **Context:** the initial stub wrapped everything in an inner
  `#[cfg(target_os = "macos")] pub mod macos { … }`, yielding the doubled path
  `platform::macos::macos::MacOSDisplay`.
- **Decision:** flatten it — contents live at the file root, gated only by
  `platform/mod.rs`'s `#[cfg(target_os = "linux")]`-style `pub mod macos;`.
- **Why:** one cfg, one module level; matches how `wayland/mod.rs` is laid out.

## Engine

### `render_TRS.rs` folded back into `render.rs`
- **Context:** the CPU-fallback render module was iterated as a parallel
  `render_TRS.rs` wired in via a `#[path = "render_TRS.rs"] pub mod render;`
  override, leaving the original `render.rs` tracked-but-dead (shadowed).
- **Decision:** make the iterated file the canonical `src/engine/render.rs` and
  drop the `#[path]` override; the old shadowed `render.rs` is deleted.
- **Why:** one file per module, no path override, no dead tracked copy to confuse
  greps/diffs. Content is unchanged — purely a rename + delete.

### Ripple/water: brightness band-aid now, backdrop capture + replace-composite later
- **Context:** WE refraction effects (`waterripple`/`waterflow`) sample the scene
  framebuffer behind the layer via a hidden null `g_Texture0`; we default that to
  the layer's own base texture, so under additive blend the layer blows out white.
- **Tried:** routing null bind-0 to the existing `scene_copy` backdrop snapshot for
  water effects. **Reverted** — the layer is full-frame + additive, so adding the
  refracted whole-scene on top doubled brightness everywhere (whole screen white).
- **Decision:** ship `WP_ENGINE_ADDITIVE_BRIGHTNESS` (default 0.55) to scale
  additive image layers down until real backdrop capture lands. The real fix must
  **couple** backdrop-sampling with a **replace** composite for that layer (the
  refracted backdrop replaces the scene, doesn't add to it) — do both or neither.
- **Why:** the band-aid is a one-line, env-tunable stopgap that matches WE's
  reference look; the coupled fix is a bigger change deferred to the engine track.

## Dependencies

### Wayland stack gated to Linux; macOS stack gated to macOS
- **Decision:** `smithay-client-toolkit`, `wayland-client`, `wayland-backend`,
  `wayland-protocols-wlr`, `calloop`, `calloop-wayland-source` live under
  `[target.'cfg(target_os = "linux")'.dependencies]`; `winit`, `objc2`,
  `objc2-app-kit` under the macOS target. `raw-window-handle` stays shared.
- **Why:** `wayland-backend`'s `client_system` feature links libwayland, which
  isn't present on macOS and blocked the build. `raw-window-handle` is pure Rust
  with no system lib and is needed by both the Wayland and Metal surface paths,
  so gating it would just cause churn.

### macOS deps pinned to winit's objc2 line (0.5.2 / 0.2.2)
- **Context:** `cargo tree` shows winit 0.30.12 (already pulled transitively via
  `eframe 0.33`) resolving `objc2 0.5.2` + `objc2-app-kit 0.2.2`. A second,
  newer `objc2-app-kit 0.3.2` also exists elsewhere in the tree.
- **Decision:** pin `winit = "0.30"`, `objc2 = "0.5"`, `objc2-app-kit = "0.2"`.
- **Why:** the backend must call `setLevel:` / `collectionBehavior` on the *same*
  `NSWindow` type winit hands back. Different objc2-app-kit majors are different
  types — pinning to winit's line keeps the handle interoperable. These crates
  are already compiled via eframe, so declaring them direct adds ~no build cost.

### winit for window+loop; drop to objc2 only for the wallpaper trick
- **Decision:** use winit for window creation and the event loop; reach into the
  raw `NSWindow` via objc2/objc2-app-kit only to set desktop level +
  all-Spaces/stationary collection behavior.
- **Why:** winit is the already-solved cross-platform window+loop (the macOS
  analogue of `smithay-client-toolkit` + `calloop`). Hand-rolling the app
  lifecycle/run loop with raw objc2 would be more code for no gain. winit doesn't
  expose desktop-level placement, so that one thing drops to AppKit.

## Build / toolchain

### shaderc via a prebuilt system lib, not build-from-source or naga
- **Context:** `shaderc-sys` couldn't find a native shaderc on macOS and its
  from-source fallback needs cmake+ninja (and broke on cmake 4's policy change).
- **Decision:** install `brew install shaderc` and point `shaderc-sys` at it with
  `SHADERC_LIB_DIR=/opt/homebrew/lib`, wired in a **gitignored**
  `.cargo/config.toml` `[env]` block. Linux uses distro `libshaderc-dev`.
- **Why:** prebuilt lib is fast and reliable; from-source is slow/fragile. The
  config is gitignored because cargo `[env]` is global — a committed macOS path
  would break Linux. Dropping shaderc for naga's `glsl-in` was rejected: the
  transpiler routes through shaderc precisely *because* naga's GLSL frontend is
  too strict for Wallpaper Engine's HLSL-flavored dialect.

### CLAUDE.md is not tracked
- **Decision:** `CLAUDE.md` is gitignored (and currently lives in the parent
  folder, outside this repo anyway).
- **Why:** it's assistant guidance, not project source.
