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
- [x] `MacOSPlatform` implements `DisplayPlatform` and is returned by
      `detect_platform()` on macOS (rendering still stubbed — `spawn_wallpaper`
      bails with a clear message).
- [x] Pinned macOS deps to winit's objc2 line: `winit 0.30`, `objc2 0.5`,
      `objc2-app-kit 0.2` (see DECISIONS.md).
- [x] Docs: DECISIONS.md, INTERVIEW.md, PROGRESS.md.

## Next: implement `MacOSPlatform::spawn_wallpaper`

Mirror `wayland/mod.rs`'s `spawn_wayland_wallpaper`:

- [ ] Spawn a render thread; return a `WallpaperHandle` whose `stop()` ends the
      winit event loop and joins the thread.
- [ ] Create a borderless winit window; via objc2-app-kit set
      `NSWindow.level = kCGDesktopWindowLevel` and
      `collectionBehavior = canJoinAllSpaces | stationary`.
- [ ] Multi-monitor: one window per `NSScreen`.
- [ ] Build a `wgpu::Surface` on the window (Metal), negotiate format, configure.
- [ ] Drive the no-readback path: `GpuSceneInstance::render_to_view` per frame,
      paced by the display link / winit redraw.
- [ ] Content routing: `Scene` → GPU; `Static`/`Video` → frame source (decide
      present path — surface blit vs a CPU fallback).
- [ ] Clean shutdown ordering: drop the wgpu surface before the window/loop.

## Later

- [ ] Test on a real macOS desktop (window actually sits behind app windows, all
      Spaces, survives display hotplug).
- [ ] CI job for macOS (`brew install shaderc ffmpeg`, `cargo test`).
- [ ] Revisit whether `Static`/`Video` need the SHM-equivalent fallback on macOS
      or can always go through the surface.
