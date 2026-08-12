use anyhow::{anyhow, Result};
use calloop::LoopSignal;
use calloop_wayland_source::WaylandSource;
use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_layer, delegate_output, delegate_pointer, delegate_registry,
    delegate_seat, delegate_shm,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
        Capability, SeatHandler, SeatState,
    },
    shell::wlr_layer::{
        Anchor, Layer, LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure,
    },
    shell::WaylandSurface,
    shm::{slot::SlotPool, Shm, ShmHandler},
};
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::{mpsc::SyncSender, Arc, Mutex};
use std::thread;
use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
    Connection, Proxy, QueueHandle,
};

use super::display::{DisplayPlatform, WallpaperHandle, WallpaperHandleInner};
use crate::{
    engine::gpu_renderer::GpuSceneInstance,
    platform,
    render::{FrameSource, RenderSettings, WallpaperContent},
};

// ── Platform implementation ───────────────────────────────────────────────────

pub(super) struct WaylandPlatform;

impl DisplayPlatform for WaylandPlatform {
    fn spawn_wallpaper(
        &self,
        content: WallpaperContent,
        settings: Arc<Mutex<RenderSettings>>,
    ) -> Result<WallpaperHandle> {
        let wayland_handle = spawn_wayland_wallpaper(content, Arc::clone(&settings))?;
        Ok(WallpaperHandle::new(Box::new(wayland_handle), settings))
    }
}

// ── Internal handle ───────────────────────────────────────────────────────────

struct WaylandHandle {
    stop_signal: LoopSignal,
    thread: thread::JoinHandle<()>,
}

impl WallpaperHandleInner for WaylandHandle {
    fn stop(self: Box<Self>) {
        self.stop_signal.stop();
        let _ = self.thread.join();
    }

    fn wait(self: Box<Self>) {
        let _ = self.thread.join();
    }
}

fn spawn_wayland_wallpaper(
    content: WallpaperContent,
    settings: Arc<Mutex<RenderSettings>>,
) -> Result<WaylandHandle> {
    let (signal_tx, signal_rx) = std::sync::mpsc::sync_channel::<LoopSignal>(0);

    let settings_thread = Arc::clone(&settings);
    let thread = thread::spawn(move || {
        if let Err(e) = wallpaper_loop(content, settings_thread, signal_tx) {
            tracing::error!(target: "wallpaper", "wallpaper thread error: {e}");
        }
    });

    let stop_signal = signal_rx
        .recv()
        .map_err(|_| anyhow!("wallpaper thread exited before sending the loop signal"))?;

    Ok(WaylandHandle {
        stop_signal,
        thread,
    })
}

// ── Content renderer ──────────────────────────────────────────────────────────

/// How the wallpaper content produces pixels.
enum ContentRenderer {
    /// CPU frames (static images, videos, and the scene fallback loop);
    /// presented through SHM buffers.
    Frames(FrameSource),
    /// GPU-rendered scene sharing our device — presented directly into the
    /// wgpu surface (no readback) when the compositor allows, with an RGBA
    /// readback + SHM fallback otherwise.
    Scene(Box<GpuSceneInstance>),
}

impl ContentRenderer {
    fn is_animated(&self) -> bool {
        match self {
            ContentRenderer::Frames(fs) => fs.is_animated(),
            ContentRenderer::Scene(_) => true,
        }
    }
}

// ── Internal renderer state ───────────────────────────────────────────────────

/// GPU presentation state for one output surface.
struct SurfaceGpu {
    surface: wgpu::Surface<'static>,
    format: wgpu::TextureFormat,
    configured: (u32, u32),
}

struct WallpaperSurface {
    /// wgpu surface — declared before `layer` so it drops before the
    /// wl_surface it was created from.
    gpu: Option<SurfaceGpu>,
    /// `None` once GPU surface creation failed for this output (don't retry).
    gpu_failed: bool,
    layer: LayerSurface,
    width: u32,
    height: u32,
    /// Previous frame's SHM pool — kept alive until compositor releases the buffer.
    pool: Option<SlotPool>,
}

struct WallpaperState {
    registry_state: RegistryState,
    output_state: OutputState,
    seat_state: SeatState,
    compositor_state: CompositorState,
    shm: Shm,
    layer_shell: LayerShell,
    /// Pointer for the first seat that advertises one — drives camera parallax
    /// and `g_PointerPosition`.
    pointer: Option<wl_pointer::WlPointer>,
    surfaces: Vec<WallpaperSurface>,
    renderer: ContentRenderer,
    gpu_scaler: platform::GpuScaler,
    settings: Arc<Mutex<RenderSettings>>,
    /// Queue handle stored so draw_at can request wl_surface_frame callbacks.
    qh: Option<QueueHandle<WallpaperState>>,
    // GPU presentation
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    display_ptr: *mut c_void,
    allow_gpu_surface: bool,
}

impl WallpaperState {
    /// Try to create + configure a wgpu surface for output `idx`.
    /// On any failure the output falls back to the SHM path permanently.
    fn ensure_gpu_surface(&mut self, idx: usize) {
        if !self.allow_gpu_surface
            || self.surfaces[idx].gpu_failed
            || !matches!(self.renderer, ContentRenderer::Scene(_))
        {
            return;
        }
        let (width, height) = (self.surfaces[idx].width, self.surfaces[idx].height);
        if width == 0 || height == 0 {
            return;
        }

        if self.surfaces[idx].gpu.is_none() {
            match self.create_gpu_surface(idx) {
                Ok(gpu) => self.surfaces[idx].gpu = Some(gpu),
                Err(e) => {
                    tracing::warn!(
                        target: "wallpaper",
                        "GPU surface unavailable for output {idx}: {e} — using SHM path"
                    );
                    self.surfaces[idx].gpu_failed = true;
                    return;
                }
            }
        }

        let gpu = self.surfaces[idx].gpu.as_mut().unwrap();
        if gpu.configured != (width, height) {
            gpu.surface.configure(
                &self.device,
                &wgpu::SurfaceConfiguration {
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    format: gpu.format,
                    width,
                    height,
                    present_mode: wgpu::PresentMode::Fifo,
                    desired_maximum_frame_latency: 2,
                    alpha_mode: wgpu::CompositeAlphaMode::Auto,
                    view_formats: vec![],
                },
            );
            gpu.configured = (width, height);
        }
    }

    fn create_gpu_surface(&self, idx: usize) -> Result<SurfaceGpu> {
        let display =
            NonNull::new(self.display_ptr).ok_or_else(|| anyhow!("null wl_display pointer"))?;
        let surface_ptr = self.surfaces[idx].layer.wl_surface().id().as_ptr() as *mut c_void;
        let surface_ptr =
            NonNull::new(surface_ptr).ok_or_else(|| anyhow!("null wl_surface pointer"))?;

        let raw_display = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(display));
        let raw_window = RawWindowHandle::Wayland(WaylandWindowHandle::new(surface_ptr));

        // Safety: the wl_display lives for the whole wallpaper thread and the
        // wl_surface is owned by our LayerSurface, which outlives the wgpu
        // surface (field order in WallpaperSurface drops `gpu` first).
        let surface = unsafe {
            self.instance
                .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                    raw_display_handle: raw_display,
                    raw_window_handle: raw_window,
                })
        }
        .map_err(|e| anyhow!("create_surface failed: {e}"))?;

        if !self.adapter.is_surface_supported(&surface) {
            return Err(anyhow!(
                "adapter does not support presenting to this surface"
            ));
        }

        let caps = surface.get_capabilities(&self.adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| {
                matches!(
                    f,
                    wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm
                )
            })
            .or_else(|| caps.formats.first().copied())
            .ok_or_else(|| anyhow!("surface reports no supported formats"))?;

        Ok(SurfaceGpu {
            surface,
            format,
            configured: (0, 0),
        })
    }

    fn draw_at(&mut self, idx: usize) {
        if self.surfaces[idx].width == 0 || self.surfaces[idx].height == 0 {
            return;
        }
        self.ensure_gpu_surface(idx);
        if self.surfaces[idx].gpu.is_some() {
            self.draw_gpu(idx);
        } else {
            self.draw_shm(idx);
        }
    }

    /// Direct GPU presentation: render the scene straight into the acquired
    /// surface texture — no CPU readback, no SHM copy.
    fn draw_gpu(&mut self, idx: usize) {
        let (width, height) = (self.surfaces[idx].width, self.surfaces[idx].height);

        // Request the next frame callback before present() commits the surface.
        if self.renderer.is_animated() {
            if let Some(qh) = &self.qh {
                let wl_surf = self.surfaces[idx].layer.wl_surface();
                wl_surf.frame(qh, wl_surf.clone());
            }
        }

        let acquired = self.surfaces[idx]
            .gpu
            .as_ref()
            .unwrap()
            .surface
            .get_current_texture();
        let frame = match acquired {
            Ok(f) => f,
            Err(wgpu::SurfaceError::Outdated) | Err(wgpu::SurfaceError::Lost) => {
                self.surfaces[idx].gpu.as_mut().unwrap().configured = (0, 0);
                self.ensure_gpu_surface(idx);
                match self.surfaces[idx]
                    .gpu
                    .as_ref()
                    .unwrap()
                    .surface
                    .get_current_texture()
                {
                    Ok(f) => f,
                    Err(e) => {
                        tracing::error!(target: "wallpaper", "surface acquire failed after reconfigure: {e}");
                        return;
                    }
                }
            }
            Err(e) => {
                tracing::error!(target: "wallpaper", "surface acquire failed: {e}");
                return;
            }
        };

        let view = frame.texture.create_view(&Default::default());
        let format = self.surfaces[idx].gpu.as_ref().unwrap().format;
        match &mut self.renderer {
            ContentRenderer::Scene(instance) => {
                instance.render_to_view(&view, width, height, format);
            }
            // GPU surfaces are only created for scene renderers.
            ContentRenderer::Frames(_) => return,
        }
        frame.present();
    }

    /// SHM path: CPU frame → GPU scaler → SHM buffer (static/video content
    /// and scene fallback when direct presentation is unavailable).
    fn draw_shm(&mut self, idx: usize) {
        let width = self.surfaces[idx].width;
        let height = self.surfaces[idx].height;

        let frame: Arc<image::RgbaImage> = match &mut self.renderer {
            ContentRenderer::Frames(fs) => Arc::clone(fs.current_frame()),
            ContentRenderer::Scene(instance) => match instance.render_rgba() {
                Ok(img) => Arc::new(img),
                Err(e) => {
                    tracing::error!(target: "wallpaper", "scene readback failed: {e}");
                    return;
                }
            },
        };

        let row_bytes = width as usize * 4;
        let stride = row_bytes;
        let active_len = stride * height as usize;
        let mut pool = match SlotPool::new(active_len, &self.shm) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(target: "wallpaper", "failed to create shm pool: {e}");
                return;
            }
        };

        let (buffer, canvas) = match pool.create_buffer(
            width as i32,
            height as i32,
            stride as i32,
            wl_shm::Format::Argb8888,
        ) {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(target: "wallpaper", "failed to create buffer: {e}");
                return;
            }
        };

        let quality = self.settings.lock().unwrap().quality;
        let pixels = self
            .gpu_scaler
            .scale(frame.as_ref(), width, height, quality);
        if pixels.len() != active_len {
            tracing::error!(
                target: "wallpaper",
                "scaler returned {} bytes for {}x{} frame, expected {}",
                pixels.len(),
                width,
                height,
                active_len
            );
            return;
        }

        copy_frame_into_shm_canvas(canvas, &pixels, width as usize, height as usize, stride);

        let wl_surf = self.surfaces[idx].layer.wl_surface();

        // Request a wl_surface_frame callback before committing — only for
        // animated sources. The callback fires after the compositor presents
        // this frame, at which point we advance to the next frame and draw again.
        // Static sources draw once on configure and never request more callbacks.
        if self.renderer.is_animated() {
            if let Some(qh) = &self.qh {
                wl_surf.frame(qh, wl_surf.clone());
            }
        }

        wl_surf.attach(Some(buffer.wl_buffer()), 0, 0);
        wl_surf.damage_buffer(0, 0, width as i32, height as i32);
        wl_surf.commit();

        // Keep the SHM pool alive until the compositor reads the buffer.
        self.surfaces[idx].pool = Some(pool);
    }
}

fn copy_frame_into_shm_canvas(
    canvas: &mut [u8],
    pixels: &[u8],
    width: usize,
    height: usize,
    stride: usize,
) {
    let row_bytes = width * 4;
    let active_len = stride * height;
    let copy_len = active_len.min(canvas.len());

    canvas.fill(0);
    if copy_len < active_len || pixels.len() < row_bytes * height {
        return;
    }

    for row in 0..height {
        let src_start = row * row_bytes;
        let src_end = src_start + row_bytes;
        let dst_start = row * stride;
        let dst_end = dst_start + row_bytes;
        canvas[dst_start..dst_end].copy_from_slice(&pixels[src_start..src_end]);
    }
}

fn wallpaper_loop(
    content: WallpaperContent,
    settings: Arc<Mutex<RenderSettings>>,
    signal_tx: SyncSender<LoopSignal>,
) -> Result<()> {
    // Open GPU device (prefer iGPU for background tasks; fall back to best).
    let gpu = platform::GpuDevice::open_low_power()
        .or_else(|_| platform::GpuDevice::open_best())
        .map_err(|e| anyhow!("no GPU device available: {e}"))?;
    let instance = gpu.instance.clone();
    let adapter = gpu.adapter.clone();
    let device = gpu.device.clone();
    let queue = gpu.queue.clone();
    let gpu_scaler = platform::GpuScaler::from_device(gpu)
        .map_err(|e| anyhow!("GPU scaler init failed: {e}"))?;

    let allow_gpu_surface = std::env::var("WP_ENGINE_FORCE_SHM").is_err();

    // Build the content renderer. Scenes render on our own device so frames
    // can be presented directly; everything else produces CPU frames.
    let renderer = match content {
        WallpaperContent::Scene { dir } if allow_gpu_surface => {
            match GpuSceneInstance::with_device(device.clone(), queue.clone(), &dir) {
                Ok(instance) => ContentRenderer::Scene(Box::new(instance)),
                Err(e) => {
                    tracing::warn!(target: "wallpaper", "GPU scene init failed ({e}); using frame-loop fallback");
                    ContentRenderer::Frames(FrameSource::from_content(WallpaperContent::Scene {
                        dir,
                    })?)
                }
            }
        }
        other => ContentRenderer::Frames(FrameSource::from_content(other)?),
    };

    let conn = Connection::connect_to_env()
        .map_err(|e| anyhow!("cannot connect to Wayland display: {e}"))?;
    let display_ptr = conn.backend().display_ptr() as *mut c_void;

    let (globals, mut event_queue) = registry_queue_init::<WallpaperState>(&conn)
        .map_err(|e| anyhow!("Wayland registry init failed: {e}"))?;

    let qh = event_queue.handle();

    let compositor_state = CompositorState::bind(&globals, &qh)
        .map_err(|_| anyhow!("compositor does not advertise wl_compositor"))?;
    let shm =
        Shm::bind(&globals, &qh).map_err(|_| anyhow!("compositor does not advertise wl_shm"))?;
    let output_state = OutputState::new(&globals, &qh);
    let layer_shell = LayerShell::bind(&globals, &qh).map_err(|_| {
        anyhow!("compositor does not support zwlr_layer_shell_v1 (wlr-layer-shell)")
    })?;
    let registry_state = RegistryState::new(&globals);
    let seat_state = SeatState::new(&globals, &qh);

    let mut state = WallpaperState {
        registry_state,
        output_state,
        seat_state,
        compositor_state,
        shm,
        layer_shell,
        pointer: None,
        surfaces: Vec::new(),
        renderer,
        gpu_scaler,
        settings,
        qh: Some(qh.clone()),
        instance,
        adapter,
        device,
        display_ptr,
        allow_gpu_surface,
    };

    // First roundtrip: discovers all current outputs → triggers new_output → creates surfaces.
    event_queue
        .roundtrip(&mut state)
        .map_err(|e| anyhow!("initial Wayland roundtrip failed: {e}"))?;

    // Second roundtrip: compositor sends configure events for our layer surfaces.
    event_queue
        .roundtrip(&mut state)
        .map_err(|e| anyhow!("second Wayland roundtrip failed: {e}"))?;

    // Hand the queue to calloop.
    let mut event_loop: calloop::EventLoop<WallpaperState> =
        calloop::EventLoop::try_new().map_err(|e| anyhow!("calloop init failed: {e}"))?;

    WaylandSource::new(conn, event_queue)
        .insert(event_loop.handle())
        .map_err(|e| anyhow!("WaylandSource insert failed: {e}"))?;

    // Give the loop signal to the spawning thread before blocking.
    let _ = signal_tx.send(event_loop.get_signal());

    // Run until stop_signal.stop() is called from the UI thread.
    let run_result = event_loop.run(None, &mut state, |_| {});

    // Destroy wgpu surfaces BEFORE the event loop (and with it the Wayland
    // connection) is dropped — vkDestroySurfaceKHR against a closed
    // wl_display segfaults.
    for surface in &mut state.surfaces {
        surface.gpu = None;
    }

    run_result.map_err(|e| anyhow!("event loop error: {e}"))?;
    Ok(())
}

// ── SCTK handler implementations ──────────────────────────────────────────────

impl CompositorHandler for WallpaperState {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: i32,
    ) {
    }
    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }
    fn frame(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
        // Compositor has presented the previous frame and is ready for the next.
        // This mirrors linux-wallpaperengine's surfaceFrameCallback pattern.
        let idx = self
            .surfaces
            .iter()
            .position(|s| s.layer.wl_surface() == surface);
        if let Some(idx) = idx {
            self.qh = Some(qh.clone());
            if let ContentRenderer::Frames(fs) = &mut self.renderer {
                fs.try_advance();
            }
            self.draw_at(idx);
        }
    }
}

impl OutputHandler for WallpaperState {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        let wl_surface = self.compositor_state.create_surface(qh);
        let layer = self.layer_shell.create_layer_surface(
            qh,
            wl_surface,
            Layer::Background,
            Some("wp-engine"),
            Some(&output),
        );

        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_exclusive_zone(-1);
        layer.set_size(0, 0);
        layer.commit();

        self.surfaces.push(WallpaperSurface {
            gpu: None,
            gpu_failed: false,
            layer,
            width: 0,
            height: 0,
            pool: None,
        });
    }

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl LayerShellHandler for WallpaperState {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {}

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        let idx = self.surfaces.iter().position(|s| s.layer == *layer);
        if let Some(idx) = idx {
            self.surfaces[idx].width = configure.new_size.0;
            self.surfaces[idx].height = configure.new_size.1;
            self.draw_at(idx);
        }
    }
}

impl ShmHandler for WallpaperState {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl SeatHandler for WallpaperState {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        // ponytail: one pointer, first seat that offers one. Multi-seat setups
        // would need a pointer per seat; nobody runs a wallpaper on two.
        if capability == Capability::Pointer && self.pointer.is_none() {
            match self.seat_state.get_pointer(qh, &seat) {
                Ok(pointer) => self.pointer = Some(pointer),
                Err(e) => {
                    tracing::warn!(target: "wallpaper", "cannot get wl_pointer ({e}); parallax stays centered")
                }
            }
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer {
            if let Some(pointer) = self.pointer.take() {
                pointer.release();
            }
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl PointerHandler for WallpaperState {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            // Leave keeps the last position rather than snapping back to
            // centre — the cursor moving onto a window shouldn't yank the
            // parallax. Presses/axis aren't wired to anything yet.
            if !matches!(
                event.kind,
                PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. }
            ) {
                continue;
            }
            let Some(surface) = self
                .surfaces
                .iter()
                .find(|s| s.layer.wl_surface() == &event.surface)
            else {
                continue;
            };
            if surface.width == 0 || surface.height == 0 {
                continue;
            }

            let norm = pointer_norm(event.position, surface.width, surface.height);

            // ponytail: only tracks while the cursor is over our own layer
            // surface. The reference additionally queries Hyprland's IPC socket
            // for a global cursor when another window has it; add that (or the
            // equivalent per-compositor call) if parallax-under-windows matters.
            if let ContentRenderer::Scene(scene) = &mut self.renderer {
                scene.set_mouse(norm);
            }
        }
    }
}

/// Surface-local pointer coordinates → the `[0,1]²` pair the engine wants for
/// `g_PointerPosition` and camera parallax.
///
/// Top-origin, i.e. y=0 is the top edge. That matches the reference by way of a
/// double negative: `WaylandMouseInput::update` stores `size.y - localY`
/// (bottom-origin) and `CScene::updateMouse` then computes
/// `mouseY = 1.0 - normalizedMouseY`, putting it back. Only
/// `m_mousePositionNormalized` (particles, the scripting `input` object) stays
/// bottom-origin — `m_mousePosition`, which feeds both uniforms we care about
/// here, does not.
fn pointer_norm(position: (f64, f64), width: u32, height: u32) -> [f32; 2] {
    [
        (position.0 as f32 / width as f32).clamp(0.0, 1.0),
        (position.1 as f32 / height as f32).clamp(0.0, 1.0),
    ]
}

impl ProvidesRegistryState for WallpaperState {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers!(OutputState, SeatState);
}

delegate_compositor!(WallpaperState);
delegate_output!(WallpaperState);
delegate_shm!(WallpaperState);
delegate_layer!(WallpaperState);
delegate_seat!(WallpaperState);
delegate_pointer!(WallpaperState);
delegate_registry!(WallpaperState);

#[cfg(test)]
mod tests {
    use super::pointer_norm;

    #[test]
    fn pointer_norm_is_top_origin_and_clamped() {
        // Top-left of the surface maps to (0,0), bottom-right to (1,1).
        // If someone "fixes" the Y axis to bottom-origin, this flips and
        // every parallax wallpaper drifts the wrong way vertically.
        assert_eq!(pointer_norm((0.0, 0.0), 1920, 1080), [0.0, 0.0]);
        assert_eq!(pointer_norm((1920.0, 1080.0), 1920, 1080), [1.0, 1.0]);
        assert_eq!(pointer_norm((960.0, 540.0), 1920, 1080), [0.5, 0.5]);

        // A quarter of the way down is 0.25, not 0.75.
        assert_eq!(pointer_norm((0.0, 270.0), 1920, 1080), [0.0, 0.25]);

        // Compositors can report positions outside the surface during drags.
        assert_eq!(pointer_norm((-40.0, 5000.0), 1920, 1080), [0.0, 1.0]);
    }
}
