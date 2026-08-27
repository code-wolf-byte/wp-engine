//! Web (HTML) wallpapers, rendered by an embedded Chromium through CEF.
//!
//! CEF runs windowless (off-screen rendering): Chromium paints into a CPU BGRA
//! buffer instead of a real window, and each painted frame is pushed down the
//! same `SyncSender<Arc<RgbaImage>>` that video and CPU scene rendering use.
//! Every presentation path — Wayland SHM, the X11 root pixmap, the GPU scaler
//! — therefore works unchanged; a web wallpaper is just another frame producer.
//!
//! The whole module is behind the off-by-default `web` cargo feature, because
//! building it downloads the CEF binary distribution (~400 MB extracted) and
//! the resulting binary needs `libcef.so` plus Chromium's resource blobs beside
//! it at runtime. Without the feature the stubs below report that clearly
//! rather than silently doing nothing.

use anyhow::Result;
use image::RgbaImage;
use std::path::Path;
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::Arc;

/// Off-screen render size for web wallpapers.
///
/// ponytail: fixed 1080p. CEF needs explicit dimensions up front and
/// `FrameSource` is built before any output is known, so the platform scaler
/// resizes to each monitor exactly as it does for a 1080p video. Plumb real
/// output dimensions through `FrameSource::from_content` if someone runs these
/// on a 4K panel and complains about softness.
pub const WEB_WIDTH: u32 = 1920;
pub const WEB_HEIGHT: u32 = 1080;

/// Start rendering `html` and return its first frame, a stream of the rest,
/// and a sender the caller can push mouse input into (see [`WebInputEvent`]).
///
/// Mirrors `render::ffmpeg::video_decode_loop`'s contract: the receiver yields
/// frames until the sender is dropped, and blocking for the first frame means
/// callers never present a blank surface.
pub fn start_web_stream(
    html: &Path,
) -> Result<(RgbaImage, Receiver<Arc<RgbaImage>>, SyncSender<WebInputEvent>)> {
    imp::start_web_stream(html)
}

/// `true` when this binary can actually render web wallpapers.
pub fn is_supported() -> bool {
    cfg!(feature = "web")
}

/// A mouse input event to forward into the embedded page — mirrors exactly
/// what the C++ reference itself forwards (`CWeb::updateMouse`): move plus
/// left/right click. Nothing else (scroll, middle click, keyboard) is
/// forwarded there either — its own `// TODO: ANY OTHER MOUSE EVENTS TO
/// SEND?` confirms that's the real engine's actual current scope, not an
/// approximation. Coordinates are normalized `[0,1]²` (top-left origin,
/// matching every other consumer of the platform layer's own pointer
/// tracking) so callers never need to know CEF's fixed pixel canvas size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WebInputEvent {
    MouseMove { x_norm: f32, y_norm: f32 },
    MouseButton { x_norm: f32, y_norm: f32, button: WebMouseButton, pressed: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebMouseButton {
    Left,
    Right,
}

/// Handle a CEF subprocess launch, if this process is one.
///
/// CEF starts its renderer/GPU/utility processes by re-executing this same
/// binary with `--type=...`. `main` must call this before parsing arguments and
/// exit with the returned code when it is `Some` — otherwise the child runs the
/// whole wallpaper app instead of a Chromium subprocess.
pub fn subprocess_main() -> Option<i32> {
    #[cfg(feature = "web")]
    {
        imp::subprocess_main()
    }
    #[cfg(not(feature = "web"))]
    {
        None
    }
}

#[cfg(not(feature = "web"))]
mod imp {
    use super::*;
    use anyhow::anyhow;

    pub fn start_web_stream(
        html: &Path,
    ) -> Result<(RgbaImage, Receiver<Arc<RgbaImage>>, SyncSender<WebInputEvent>)> {
        Err(anyhow!(
            "cannot render web wallpaper {}: this build has no web support.\n\
             Rebuild with `cargo build --features web` (downloads the CEF/Chromium \
             runtime, ~400 MB; set CEF_PATH to reuse an existing distribution).",
            html.display()
        ))
    }
}

#[cfg(feature = "web")]
mod imp {
    use super::*;
    use anyhow::{anyhow, Context};
    // Glob imports: the `wrap_*!` macros expand to impls of `WrapClient` /
    // `ImplClient` / `WrapRenderHandler` / `ImplRenderHandler` and the `Rc`
    // machinery, all of which must be in scope at the expansion site.
    use cef::rc::*;
    use cef::*;
    use cef::{args::Args, wrap_client, wrap_render_handler};
    use std::fmt::Write as _;
    use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
    use std::sync::OnceLock;
    use std::time::{Duration, Instant};

    /// How long to wait for Chromium to paint the first frame before giving up.
    const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(30);

    /// Work item for the CEF thread. CEF may only be initialised once per
    /// process and its message loop must be pumped from the one thread that
    /// initialised it, so every browser is created there rather than on the
    /// caller's thread.
    struct OpenRequest {
        url: String,
        frames: SyncSender<Arc<RgbaImage>>,
        /// project.json's `general.properties`, already serialised, ready to
        /// hand to the page's `applyUserProperties`.
        properties: String,
        /// Whether this wallpaper calls `wallpaperRegisterAudioListener`. Only
        /// then do we open a capture device — a page that ignores audio has no
        /// business making us grab the desktop's output stream.
        wants_audio: bool,
        /// Mouse events the platform layer pushes in — drained once per
        /// message-loop tick and forwarded to CEF's `BrowserHost`. See
        /// `WebInputEvent`.
        input_rx: Receiver<WebInputEvent>,
    }

    /// The Wallpaper Engine browser API, as much of it as this corpus uses.
    ///
    /// Injected into every frame before page scripts run. `wallpaperPropertyListener`
    /// is an accessor rather than a plain slot on purpose: properties are pushed
    /// from Rust as soon as the browser exists, which usually beats the page
    /// assigning its listener, so the setter re-delivers whatever arrived early.
    /// Without that the common case is a silent no-op.
    const BOOTSTRAP_JS: &str = r#"
(function () {
  if (window.__wpEngineBridge) return;
  var pendingProps = null, propListener = null, audioCb = null;

  Object.defineProperty(window, 'wallpaperPropertyListener', {
    configurable: true,
    get: function () { return propListener; },
    set: function (v) {
      propListener = v;
      if (v && pendingProps && typeof v.applyUserProperties === 'function') {
        try { v.applyUserProperties(pendingProps); } catch (e) { console.error(e); }
      }
    }
  });

  window.wallpaperRegisterAudioListener = function (cb) { audioCb = cb; };

  // No file-picker UI exists here, so the callback simply never fires — the
  // page keeps whatever default it started with.
  window.wallpaperRequestRandomFileForProperty = function (name, cb) {};

  window.__wpProps = function (p) {
    pendingProps = p;
    if (propListener && typeof propListener.applyUserProperties === 'function') {
      try { propListener.applyUserProperties(p); } catch (e) { console.error(e); }
    }
  };
  window.__wpAudio = function (a) {
    if (audioCb) { try { audioCb(a); } catch (e) { console.error(e); } }
  };
  window.__wpEngineBridge = true;
})();
"#;

    fn cef_thread() -> &'static SyncSender<OpenRequest> {
        static TX: OnceLock<SyncSender<OpenRequest>> = OnceLock::new();
        TX.get_or_init(|| {
            let (tx, rx) = sync_channel::<OpenRequest>(1);
            std::thread::Builder::new()
                .name("cef".into())
                .spawn(move || cef_main(rx))
                .expect("spawning the CEF thread");
            tx
        })
    }

    /// Pin the CEF API version for this process.
    ///
    /// CEF 148 introduced API versioning: every call crossing the libcef
    /// boundary checks that the host has pinned a version first, and without
    /// this libcef aborts with `CefClient_0_CToCpp called with invalid version
    /// -1` the moment it calls back into us. It must run in BOTH roles — the
    /// subprocess path and the browser-process path — before any other CEF
    /// call. `CEF_API_VERSION_LAST` is the newest version these bindings were
    /// generated against.
    fn pin_api_version() {
        let _ = api_hash(cef::sys::CEF_API_VERSION_LAST, 0);
    }

    // CEF requires an App on every initialize path. Ours also carries the
    // render-process handler, which is the only place the WE bridge can be
    // injected early enough: `on_context_created` runs in the render process
    // before the page's own scripts, whereas anything driven from the browser
    // process races them.
    wrap_app! {
        struct WallpaperApp {
            render_process_handler: RenderProcessHandler,
        }

        impl App {
            fn render_process_handler(&self) -> Option<RenderProcessHandler> {
                Some(self.render_process_handler.clone())
            }
        }
    }

    fn make_app() -> App {
        WallpaperApp::new(WallpaperRenderProcess::new())
    }

    wrap_render_process_handler! {
        struct WallpaperRenderProcess;

        impl RenderProcessHandler {
            fn on_context_created(
                &self,
                _browser: Option<&mut Browser>,
                frame: Option<&mut Frame>,
                _context: Option<&mut V8Context>,
            ) {
                if let Some(frame) = frame {
                    frame.execute_java_script(
                        Some(&BOOTSTRAP_JS.into()),
                        Some(&"wp-engine://bridge".into()),
                        0,
                    );
                }
            }
        }
    }

    pub fn subprocess_main() -> Option<i32> {
        pin_api_version();
        let args = Args::new();
        let mut app = make_app();
        let code = cef::execute_process(
            Some(args.as_main_args()),
            Some(&mut app),
            std::ptr::null_mut(),
        );
        (code >= 0).then_some(code)
    }

    /// How often the audio spectrum is pushed into the page. WE's own listener
    /// fires at roughly frame rate; 30 Hz is smooth for a visualiser and keeps
    /// the per-push `execute_java_script` cost off the CEF thread's back.
    const AUDIO_PUSH_INTERVAL: Duration = Duration::from_millis(33);

    /// Owns CEF for the life of the process: initialise once, then pump the
    /// message loop forever, creating browsers as requests arrive.
    ///
    /// ponytail: never calls `cef::shutdown`. Re-initialising CEF in the same
    /// process is not supported, and a wallpaper switch would otherwise do
    /// exactly that; letting process exit reclaim it is the honest trade. The
    /// old browser is closed when its frame channel drops.
    fn cef_main(rx: std::sync::mpsc::Receiver<OpenRequest>) {
        pin_api_version();
        let args = Args::new();
        let mut app = make_app();
        let settings = Settings {
            no_sandbox: 1,
            windowless_rendering_enabled: 1,
            // Keep Chromium's own cache out of the user's cwd.
            root_cache_path: cef_cache_dir().as_str().into(),
            ..Default::default()
        };

        if initialize(
            Some(args.as_main_args()),
            Some(&settings),
            Some(&mut app),
            std::ptr::null_mut(),
        ) != 1
        {
            tracing::error!(target: "web", "cef::initialize failed — web wallpapers unavailable");
            return;
        }

        // Browsers are kept alive here; dropping one closes it. Paired with
        // its own input receiver since each `start_web_stream` call creates a
        // fresh channel (see `OpenRequest::input_rx`).
        let mut open: Vec<(Browser, Receiver<WebInputEvent>)> = Vec::new();
        let mut properties = String::new();
        let mut audio: Option<crate::engine::audio::AudioCapture> = None;
        let mut last_audio_push = Instant::now();

        loop {
            match rx.try_recv() {
                Ok(req) => {
                    // A new wallpaper replaces the previous one.
                    open.clear();
                    audio = None;
                    properties = req.properties.clone();
                    let wants_audio = req.wants_audio;
                    let browser = create_browser(&req);
                    match browser {
                        Some(browser) => open.push((browser, req.input_rx)),
                        None => tracing::error!(target: "web", "failed to create CEF browser"),
                    }
                    if wants_audio {
                        audio = crate::engine::audio::AudioCapture::start();
                        if audio.is_none() {
                            tracing::warn!(
                                target: "web",
                                "page uses wallpaperRegisterAudioListener but audio capture \
                                 could not start; the visualiser will sit at silence"
                            );
                        }
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }

            if let Some((browser, input_rx)) = open.first() {
                // Properties are re-sent every tick rather than once: the page
                // may not have assigned its listener yet, and the injected
                // setter re-delivers on assignment anyway, so this is cheap
                // insurance against a lost first delivery. `__wpProps` is
                // idempotent.
                if !properties.is_empty() {
                    eval_in_page(
                        browser,
                        &format!("window.__wpProps&&window.__wpProps({properties});"),
                    );
                    // One delivery per navigation is enough once it lands.
                    properties.clear();
                }

                if let Some(capture) = &audio {
                    if last_audio_push.elapsed() >= AUDIO_PUSH_INTERVAL {
                        last_audio_push = Instant::now();
                        eval_in_page(browser, &audio_push_script(&capture.spectrum()));
                    }
                }

                // Drain every pending event rather than just the latest: a
                // dropped click is a real bug in a way a dropped mouse-move
                // sample never is.
                while let Ok(event) = input_rx.try_recv() {
                    forward_input_event(browser, event);
                }
            }

            // Chromium paints from inside this call — without it there are no
            // OnPaint callbacks at all and the wallpaper never advances.
            do_message_loop_work();
            std::thread::sleep(Duration::from_millis(4));
        }
    }

    fn eval_in_page(browser: &Browser, script: &str) {
        if let Some(frame) = browser.main_frame() {
            frame.execute_java_script(Some(&script.into()), None, 0);
        }
    }

    /// Forward one mouse event to CEF — mirrors `CWeb::updateMouse` exactly
    /// (`SendMouseMoveEvent`/`SendMouseClickEvent`, no other event types; see
    /// `WebInputEvent`'s own doc comment). Normalized `[0,1]²` coordinates
    /// convert to CEF's fixed `WEB_WIDTH`×`WEB_HEIGHT` pixel canvas here, so
    /// callers only ever deal in the same normalized space every other
    /// pointer consumer in this codebase already uses.
    fn forward_input_event(browser: &Browser, event: WebInputEvent) {
        let Some(host) = browser.host() else {
            return;
        };
        let to_px = |x_norm: f32, y_norm: f32| MouseEvent {
            x: (x_norm.clamp(0.0, 1.0) * WEB_WIDTH as f32) as i32,
            y: (y_norm.clamp(0.0, 1.0) * WEB_HEIGHT as f32) as i32,
            modifiers: 0,
        };
        match event {
            WebInputEvent::MouseMove { x_norm, y_norm } => {
                host.send_mouse_move_event(Some(&to_px(x_norm, y_norm)), 0);
            }
            WebInputEvent::MouseButton { x_norm, y_norm, button, pressed } => {
                let type_ = match button {
                    WebMouseButton::Left => MouseButtonType::LEFT,
                    WebMouseButton::Right => MouseButtonType::RIGHT,
                };
                host.send_mouse_click_event(
                    Some(&to_px(x_norm, y_norm)),
                    type_,
                    (!pressed) as i32,
                    1,
                );
            }
        }
    }

    /// Build the `__wpAudio` call for one spectrum snapshot.
    ///
    /// WE hands the listener 128 values: 64 left-channel bands followed by 64
    /// right-channel bands — precisely `AudioSpectrum`'s `s64_*` pair, so no
    /// resampling is needed.
    fn audio_push_script(spectrum: &crate::engine::audio::AudioSpectrum) -> String {
        let mut s = String::with_capacity(1024);
        s.push_str("window.__wpAudio&&window.__wpAudio([");
        for (i, v) in spectrum
            .s64_left
            .iter()
            .chain(spectrum.s64_right.iter())
            .enumerate()
        {
            if i > 0 {
                s.push(',');
            }
            // Three decimals is well past what a visualiser can show and keeps
            // the script string small enough to parse cheaply at 30 Hz.
            let _ = write!(s, "{v:.3}");
        }
        s.push_str("]);");
        s
    }

    fn create_browser(req: &OpenRequest) -> Option<Browser> {
        let render_handler = WallpaperRenderHandler::new(req.frames.clone());
        let mut client = WallpaperClient::new(render_handler);
        let window_info = WindowInfo {
            windowless_rendering_enabled: 1,
            bounds: Rect {
                x: 0,
                y: 0,
                width: WEB_WIDTH as i32,
                height: WEB_HEIGHT as i32,
            },
            ..Default::default()
        }
        .set_as_windowless(0);
        let browser_settings = BrowserSettings {
            windowless_frame_rate: 60,
            ..Default::default()
        };

        browser_host_create_browser_sync(
            Some(&window_info),
            Some(&mut client),
            Some(&req.url.as_str().into()),
            Some(&browser_settings),
            None,
            None,
        )
    }

    /// project.json's `general.properties` as a JSON object literal.
    ///
    /// Its shape is already what `applyUserProperties` expects — each entry is
    /// `{ "value": …, "type": …, … }` and pages read `properties.<name>.value`
    /// — so it passes through untouched. Empty string when there are none.
    fn user_properties_json(dir: &Path) -> String {
        let Ok(text) = std::fs::read_to_string(dir.join("project.json")) else {
            return String::new();
        };
        let Ok(project) = serde_json::from_str::<serde_json::Value>(&text) else {
            return String::new();
        };
        match project.get("general").and_then(|g| g.get("properties")) {
            Some(props) if props.is_object() => props.to_string(),
            _ => String::new(),
        }
    }

    /// Does this wallpaper call `wallpaperRegisterAudioListener`?
    ///
    /// Grepping the bundle is cruder than asking the page, but asking means an
    /// async round-trip into the render process, and the answer decides whether
    /// we open a desktop-audio capture device at all — a side effect worth
    /// avoiding for the pages that never use it.
    ///
    /// ponytail: scans .html/.js up to 2 levels deep. A wallpaper hiding the
    /// call in a .json blob or deeper tree just gets a silent spectrum; widen
    /// the walk if one turns up.
    fn uses_audio_listener(dir: &Path) -> bool {
        fn scan(dir: &Path, depth: usize) -> bool {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return false;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if depth > 0 && scan(&path, depth - 1) {
                        return true;
                    }
                    continue;
                }
                let is_script = matches!(
                    path.extension().and_then(|e| e.to_str()),
                    Some("html" | "htm" | "js")
                );
                if is_script
                    && std::fs::read(&path).is_ok_and(|bytes| {
                        memmem_contains(&bytes, b"wallpaperRegisterAudioListener")
                    })
                {
                    return true;
                }
            }
            false
        }
        scan(dir, 2)
    }

    /// Substring search over raw bytes — these bundles are minified and not
    /// always valid UTF-8, so `str::contains` is not an option.
    fn memmem_contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack
            .windows(needle.len())
            .any(|window| window == needle)
    }

    fn cef_cache_dir() -> String {
        let dir = dirs::cache_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("wp-engine/cef");
        let _ = std::fs::create_dir_all(&dir);
        dir.to_string_lossy().into_owned()
    }

    wrap_client! {
        struct WallpaperClient {
            render_handler: RenderHandler,
        }

        impl Client {
            fn render_handler(&self) -> Option<RenderHandler> {
                Some(self.render_handler.clone())
            }
        }
    }

    wrap_render_handler! {
        struct WallpaperRenderHandler {
            frames: SyncSender<Arc<RgbaImage>>,
        }

        impl RenderHandler {
            fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
                // CEF treats an empty rect as an error and paints nothing.
                if let Some(rect) = rect {
                    *rect = Rect {
                        x: 0,
                        y: 0,
                        width: WEB_WIDTH as i32,
                        height: WEB_HEIGHT as i32,
                    };
                }
            }

            fn on_paint(
                &self,
                _browser: Option<&mut Browser>,
                type_: PaintElementType,
                _dirty_rects: Option<&[Rect]>,
                buffer: *const u8,
                width: ::std::os::raw::c_int,
                height: ::std::os::raw::c_int,
            ) {
                // PET_POPUP is the dropdown/select overlay drawn separately; we
                // only present the main view.
                if type_ != PaintElementType::VIEW || buffer.is_null() || width <= 0 || height <= 0 {
                    return;
                }
                let (w, h) = (width as u32, height as u32);
                let len = w as usize * h as usize * 4;
                // SAFETY: CEF guarantees `buffer` holds width*height*4 bytes of
                // BGRA for the duration of this callback. We copy out before
                // returning and never retain the pointer.
                let src = unsafe { std::slice::from_raw_parts(buffer, len) };

                let mut rgba = Vec::with_capacity(len);
                for px in src.chunks_exact(4) {
                    // CEF paints BGRA; RgbaImage wants RGBA.
                    rgba.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
                }
                let Some(img) = RgbaImage::from_raw(w, h, rgba) else {
                    return;
                };

                // Drop frames rather than block: this runs on the CEF UI
                // thread, and stalling it stalls Chromium itself.
                match self.frames.try_send(Arc::new(img)) {
                    Ok(()) | Err(TrySendError::Full(_)) => {}
                    Err(TrySendError::Disconnected(_)) => {}
                }
            }
        }
    }

    pub fn start_web_stream(
        html: &Path,
    ) -> Result<(RgbaImage, Receiver<Arc<RgbaImage>>, SyncSender<WebInputEvent>)> {
        let html = html
            .canonicalize()
            .with_context(|| format!("resolving web wallpaper path {}", html.display()))?;
        let url = format!("file://{}", html.to_string_lossy());

        let dir = html.parent().unwrap_or(&html).to_path_buf();
        let (frames, rx) = sync_channel::<Arc<RgbaImage>>(2);
        // Bounded, but generously so relative to `cef_main`'s ~4ms poll —
        // this should never realistically fill. `try_send` on the platform
        // side means a full channel drops rather than blocking the render
        // loop, same trade `frames` itself already makes.
        let (input_tx, input_rx) = sync_channel::<WebInputEvent>(64);
        cef_thread()
            .send(OpenRequest {
                url,
                frames,
                properties: user_properties_json(&dir),
                wants_audio: uses_audio_listener(&dir),
                input_rx,
            })
            .map_err(|_| anyhow!("the CEF thread is not running"))?;

        // Block for the first paint so callers never present a blank surface,
        // matching the video decoder's contract.
        let deadline = Instant::now() + FIRST_FRAME_TIMEOUT;
        let first = loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or_else(|| anyhow!("timed out waiting for the first web frame"))?;
            match rx.recv_timeout(remaining) {
                Ok(frame) => break frame,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    return Err(anyhow!("timed out waiting for the first web frame"))
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(anyhow!("the CEF browser closed before painting a frame"))
                }
            }
        };

        Ok((
            Arc::try_unwrap(first).unwrap_or_else(|arc| (*arc).clone()),
            rx,
            input_tx,
        ))
    }
}
