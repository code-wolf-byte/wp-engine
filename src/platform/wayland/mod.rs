use anyhow::{anyhow, Result};
use calloop::LoopSignal;
use calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    shell::WaylandSurface,
    delegate_compositor, delegate_layer, delegate_output, delegate_registry, delegate_shm,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::wlr_layer::{
        Anchor, Layer, LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure,
    },
    shm::{slot::SlotPool, Shm, ShmHandler},
};
use std::sync::{Arc, Mutex, mpsc::SyncSender};
use std::thread;
use std::time::Duration;
use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_output, wl_shm, wl_surface},
    Connection, QueueHandle,
};

use crate::render::{FrameSource, RenderSettings};
use super::display::{DisplayPlatform, WallpaperHandle, WallpaperHandleInner};

// ── Platform implementation ───────────────────────────────────────────────────

pub(super) struct WaylandPlatform;

impl DisplayPlatform for WaylandPlatform {
    fn spawn_wallpaper(
        &self,
        frame_source: FrameSource,
        settings: Arc<Mutex<RenderSettings>>,
    ) -> Result<WallpaperHandle> {
        let wayland_handle = spawn_wayland_wallpaper(frame_source, Arc::clone(&settings))?;
        Ok(WallpaperHandle::new(Box::new(wayland_handle), settings))
    }
}

// ── Internal handle ───────────────────────────────────────────────────────────

struct WaylandHandle {
    stop_signal: LoopSignal,
    thread:      thread::JoinHandle<()>,
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
    frame_source: FrameSource,
    settings: Arc<Mutex<RenderSettings>>,
) -> Result<WaylandHandle> {
    let (signal_tx, signal_rx) = std::sync::mpsc::sync_channel::<LoopSignal>(0);

    let settings_thread = Arc::clone(&settings);
    let thread = thread::spawn(move || {
        if let Err(e) = wallpaper_loop(frame_source, settings_thread, signal_tx) {
            eprintln!("wallpaper thread error: {e}");
        }
    });

    let stop_signal = signal_rx
        .recv()
        .map_err(|_| anyhow!("wallpaper thread exited before sending the loop signal"))?;

    Ok(WaylandHandle { stop_signal, thread })
}

// ── Internal renderer ─────────────────────────────────────────────────────────

struct WallpaperSurface {
    layer: LayerSurface,
    width: u32,
    height: u32,
}

struct WallpaperState {
    registry_state:   RegistryState,
    output_state:     OutputState,
    compositor_state: CompositorState,
    shm:              Shm,
    layer_shell:      LayerShell,
    surfaces:         Vec<WallpaperSurface>,
    /// Live frame source receiving RGBA frames from the IPC thread.
    frame_source:     FrameSource,
    /// Render settings shared with the UI thread.
    settings:         Arc<Mutex<RenderSettings>>,
}

impl WallpaperState {
    /// Draw the current frame onto the surface at `surfaces[idx]`.
    fn draw_at(&self, idx: usize) {
        let width = self.surfaces[idx].width;
        let height = self.surfaces[idx].height;
        if width == 0 || height == 0 {
            return;
        }

        let frame = Arc::clone(self.frame_source.current_frame());

        // Verify the frame dimensions match the surface.
        // WE is launched at the exact monitor size, so they should always match.
        let frame_w = frame.width();
        let frame_h = frame.height();
        if frame_w == 0 || frame_h == 0 {
            return;
        }

        let stride = width * 4;
        let mut pool = match SlotPool::new((width * height * 4) as usize, &self.shm) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("wallpaper: failed to create shm pool: {e}");
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
                eprintln!("wallpaper: failed to create buffer: {e}");
                return;
            }
        };

        // Frame pixels are RGBA (from the IPC receiver which swaps B↔R).
        // Wayland ARGB8888 on little-endian stores pixels as [B, G, R, A] in memory.
        // We need to swap R↔B when writing to the canvas.
        if frame_w == width && frame_h == height {
            let src = frame.as_raw();
            for (dst_pixel, src_pixel) in canvas.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                // src_pixel = [R, G, B, A] (RGBA)
                // dst_pixel = [B, G, R, A] (ARGB8888 LE)
                dst_pixel[0] = src_pixel[2]; // B
                dst_pixel[1] = src_pixel[1]; // G
                dst_pixel[2] = src_pixel[0]; // R
                dst_pixel[3] = src_pixel[3]; // A
            }
        } else {
            // Dimensions mismatch — fill with black
            for b in canvas.iter_mut() {
                *b = 0;
            }
            // Set alpha to fully opaque
            for pixel in canvas.chunks_exact_mut(4) {
                pixel[3] = 0xFF;
            }
        }

        let wl_surf = self.surfaces[idx].layer.wl_surface();
        wl_surf.attach(Some(buffer.wl_buffer()), 0, 0);
        wl_surf.damage_buffer(0, 0, width as i32, height as i32);
        wl_surf.commit();
    }
}

fn wallpaper_loop(
    frame_source: FrameSource,
    settings: Arc<Mutex<RenderSettings>>,
    signal_tx: SyncSender<LoopSignal>,
) -> Result<()> {
    let conn = Connection::connect_to_env()
        .map_err(|e| anyhow!("cannot connect to Wayland display: {e}"))?;

    let (globals, mut event_queue) = registry_queue_init::<WallpaperState>(&conn)
        .map_err(|e| anyhow!("Wayland registry init failed: {e}"))?;

    let qh = event_queue.handle();

    let compositor_state = CompositorState::bind(&globals, &qh)
        .map_err(|_| anyhow!("compositor does not advertise wl_compositor"))?;
    let shm = Shm::bind(&globals, &qh)
        .map_err(|_| anyhow!("compositor does not advertise wl_shm"))?;
    let output_state = OutputState::new(&globals, &qh);
    let layer_shell = LayerShell::bind(&globals, &qh).map_err(|_| {
        anyhow!("compositor does not support zwlr_layer_shell_v1 (wlr-layer-shell)")
    })?;
    let registry_state = RegistryState::new(&globals);

    let mut state = WallpaperState {
        registry_state,
        output_state,
        compositor_state,
        shm,
        layer_shell,
        surfaces: Vec::new(),
        frame_source,
        settings,
    };

    // First roundtrip: discover outputs → create surfaces
    event_queue
        .roundtrip(&mut state)
        .map_err(|e| anyhow!("initial Wayland roundtrip failed: {e}"))?;

    // Second roundtrip: compositor sends configure events for our layer surfaces
    event_queue
        .roundtrip(&mut state)
        .map_err(|e| anyhow!("second Wayland roundtrip failed: {e}"))?;

    let mut event_loop: calloop::EventLoop<WallpaperState> =
        calloop::EventLoop::try_new().map_err(|e| anyhow!("calloop init failed: {e}"))?;

    WaylandSource::new(conn, event_queue)
        .insert(event_loop.handle())
        .map_err(|e| anyhow!("WaylandSource insert failed: {e}"))?;

    // Poll at ~120 Hz for animated (IPC) frame sources.
    if state.frame_source.is_animated() {
        const POLL_INTERVAL: Duration = Duration::from_millis(8);

        event_loop
            .handle()
            .insert_source(
                calloop::timer::Timer::from_duration(POLL_INTERVAL),
                |_deadline, _metadata, state: &mut WallpaperState| {
                    if state.frame_source.try_advance() {
                        let n = state.surfaces.len();
                        for idx in 0..n {
                            state.draw_at(idx);
                        }
                    }
                    calloop::timer::TimeoutAction::ToDuration(POLL_INTERVAL)
                },
            )
            .map_err(|e| anyhow!("timer source insert failed: {e}"))?;
    }

    let _ = signal_tx.send(event_loop.get_signal());

    event_loop
        .run(None, &mut state, |_| {})
        .map_err(|e| anyhow!("event loop error: {e}"))?;

    Ok(())
}

// ── SCTK handler implementations ──────────────────────────────────────────────

impl CompositorHandler for WallpaperState {
    fn scale_factor_changed(
        &mut self, _: &Connection, _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface, _: i32,
    ) {}
    fn transform_changed(
        &mut self, _: &Connection, _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface, _: wl_output::Transform,
    ) {}
    fn frame(
        &mut self, _: &Connection, _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface, _time: u32,
    ) {}
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

        self.surfaces.push(WallpaperSurface { layer, width: 0, height: 0 });
    }

    fn update_output(
        &mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput,
    ) {}

    fn output_destroyed(
        &mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput,
    ) {}
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

impl ProvidesRegistryState for WallpaperState {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers!(OutputState);
}

delegate_compositor!(WallpaperState);
delegate_output!(WallpaperState);
delegate_shm!(WallpaperState);
delegate_layer!(WallpaperState);
delegate_registry!(WallpaperState);
