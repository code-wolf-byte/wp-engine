use anyhow::{anyhow, Result};
use calloop::LoopSignal;
use calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_layer, delegate_output, delegate_registry, delegate_shm,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::wlr_layer::{
        Anchor, Layer, LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure,
    },
    shell::WaylandSurface,
    shm::{slot::SlotPool, Shm, ShmHandler},
};
use std::sync::{mpsc::SyncSender, Arc, Mutex};
use std::thread;
use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_output, wl_shm, wl_surface},
    Connection, QueueHandle,
};

use super::display::{DisplayPlatform, WallpaperHandle, WallpaperHandleInner};
use crate::{
    platform,
    render::{FrameSource, RenderSettings},
};

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

    Ok(WaylandHandle {
        stop_signal,
        thread,
    })
}

// ── Internal renderer ─────────────────────────────────────────────────────────

struct WallpaperSurface {
    layer: LayerSurface,
    width: u32,
    height: u32,
    /// Previous frame's SHM pool — kept alive until compositor releases the buffer.
    pool: Option<SlotPool>,
}

struct WallpaperState {
    registry_state: RegistryState,
    output_state: OutputState,
    compositor_state: CompositorState,
    shm: Shm,
    layer_shell: LayerShell,
    surfaces: Vec<WallpaperSurface>,
    frame_source: FrameSource,
    gpu_scaler: platform::GpuScaler,
    settings: Arc<Mutex<RenderSettings>>,
    /// Queue handle stored so draw_at can request wl_surface_frame callbacks.
    qh: Option<QueueHandle<WallpaperState>>,
}

impl WallpaperState {
    fn draw_at(&mut self, idx: usize) {
        let width = self.surfaces[idx].width;
        let height = self.surfaces[idx].height;
        if width == 0 || height == 0 {
            return;
        }

        let frame = std::sync::Arc::clone(self.frame_source.current_frame());

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

        let quality = self.settings.lock().unwrap().quality;
        let pixels = self
            .gpu_scaler
            .scale(frame.as_ref(), width, height, quality);
        canvas.copy_from_slice(&pixels);

        let wl_surf = self.surfaces[idx].layer.wl_surface();

        // Request a wl_surface_frame callback before committing — only for
        // animated sources. The callback fires after the compositor presents
        // this frame, at which point we advance to the next frame and draw again.
        // Static sources draw once on configure and never request more callbacks.
        if self.frame_source.is_animated() {
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

fn wallpaper_loop(
    frame_source: FrameSource,
    settings: Arc<Mutex<RenderSettings>>,
    signal_tx: SyncSender<LoopSignal>,
) -> Result<()> {
    // Open GPU device (prefer iGPU for background tasks; fall back to best).
    let gpu = platform::GpuDevice::open_low_power()
        .or_else(|_| platform::GpuDevice::open_best())
        .map_err(|e| anyhow!("no GPU device available: {e}"))?;
    let gpu_scaler = platform::GpuScaler::from_device(gpu)
        .map_err(|e| anyhow!("GPU scaler init failed: {e}"))?;

    let conn = Connection::connect_to_env()
        .map_err(|e| anyhow!("cannot connect to Wayland display: {e}"))?;

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

    let mut state = WallpaperState {
        registry_state,
        output_state,
        compositor_state,
        shm,
        layer_shell,
        surfaces: Vec::new(),
        frame_source,
        gpu_scaler,
        settings,
        qh: Some(qh.clone()),
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
    event_loop
        .run(None, &mut state, |_| {})
        .map_err(|e| anyhow!("event loop error: {e}"))?;

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
            self.frame_source.try_advance();
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
