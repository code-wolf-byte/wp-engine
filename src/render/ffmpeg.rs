use anyhow::{anyhow, Result};
use image::RgbaImage;
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::SyncSender;

/// Open the container and read the video stream's average frame rate.
/// Does not decode any frames.
pub fn probe_fps(path: &Path) -> Result<f64> {
    ffmpeg_next::init().map_err(|e| anyhow!("FFmpeg init: {e}"))?;

    let ctx = ffmpeg_next::format::input(&path)
        .map_err(|e| anyhow!("cannot open '{}': {e}", path.display()))?;

    let stream = ctx
        .streams()
        .best(ffmpeg_next::media::Type::Video)
        .ok_or_else(|| anyhow!("no video stream found in '{}'", path.display()))?;

    let rate = stream.avg_frame_rate();
    let fps = rate.numerator() as f64 / rate.denominator() as f64;

    if !fps.is_finite() || fps <= 0.0 {
        return Err(anyhow!(
            "could not determine FPS for '{}' (reported {}/{})",
            path.display(),
            rate.numerator(),
            rate.denominator(),
        ));
    }

    Ok(fps)
}

/// Decode every frame of the video file and send each as an `Arc<RgbaImage>`
/// through `tx`. When the file ends the decoder loops back to the beginning
/// for seamless repeat. Exits when `tx.send()` returns `Err` (receiver dropped).
pub fn video_decode_loop(path: &Path, tx: &SyncSender<Arc<RgbaImage>>) -> Result<()> {
    use ffmpeg_next::{
        codec::context::Context as CodecCtx,
        format,
        format::Pixel,
        media::Type,
        software::scaling::{context::Context as ScaleCtx, flag::Flags},
        util::frame::video::Video as VideoFrame,
    };
    use std::thread;
    use std::time::{Duration, Instant};

    ffmpeg_next::init().map_err(|e| anyhow!("FFmpeg init: {e}"))?;

    // Outer loop: re-open the file on each iteration to loop the video.
    loop {
        let mut ictx = format::input(&path)
            .map_err(|e| anyhow!("cannot open '{}': {e}", path.display()))?;

        let stream_idx = ictx
            .streams()
            .best(Type::Video)
            .ok_or_else(|| anyhow!("no video stream in '{}'", path.display()))?
            .index();

        // Capture the stream's time base (num/den) before borrowing ictx for
        // decoding — Rational is Copy so this is free.
        let time_base = ictx.stream(stream_idx).unwrap().time_base();

        // Build the codec context and video decoder.
        let mut decoder = {
            let stream = ictx.stream(stream_idx).unwrap();
            CodecCtx::from_parameters(stream.parameters())
                .map_err(|e| anyhow!("codec context: {e}"))?
                .decoder()
                .video()
                .map_err(|e| anyhow!("video decoder: {e}"))?
        };

        // Build a software scaler: decoded pixel format → packed RGBA8.
        // The output dimensions match the input so scaling is handled later
        // in the Wayland renderer (which scales to each monitor's resolution).
        let mut scaler = ScaleCtx::get(
            decoder.format(),
            decoder.width(),
            decoder.height(),
            Pixel::RGBA,
            decoder.width(),
            decoder.height(),
            Flags::BILINEAR,
        )
        .map_err(|e| anyhow!("scaler context: {e}"))?;

        let mut raw = VideoFrame::empty();
        let mut scaled = VideoFrame::empty();

        // PTS state — reset on every loop-back so playback restarts correctly.
        let mut first_pts: Option<i64> = None;
        let mut loop_start: Option<Instant> = None;

        // Inner packet loop.
        for (stream, packet) in ictx.packets() {
            if stream.index() != stream_idx {
                continue;
            }
            if decoder.send_packet(&packet).is_err() {
                continue;
            }

            while decoder.receive_frame(&mut raw).is_ok() {
                // ── PTS-based real-time pacing ────────────────────────────
                // Sleep until this frame's presentation time has elapsed so
                // the decoder sends frames at the video's native speed rather
                // than as fast as the CPU allows.
                if let Some(pts) = raw.pts() {
                    let tb_num = time_base.numerator() as f64;
                    let tb_den = time_base.denominator() as f64;

                    if tb_den > 0.0 {
                        let first = *first_pts.get_or_insert(pts);
                        let start = *loop_start.get_or_insert_with(Instant::now);

                        let frame_offset = Duration::from_secs_f64(
                            ((pts - first) as f64 * tb_num / tb_den).max(0.0),
                        );

                        if let Some(wait) = frame_offset.checked_sub(start.elapsed()) {
                            thread::sleep(wait);
                        }
                    }
                }

                if scaler.run(&raw, &mut scaled).is_err() {
                    continue;
                }

                if let Some(img) = frame_to_rgba(&scaled) {
                    // `send` blocks when the channel is full (back-pressure).
                    // It returns `Err` when the receiver is dropped → exit.
                    if tx.send(Arc::new(img)).is_err() {
                        return Ok(());
                    }
                }
            }
        }

        // Flush any frames buffered inside the decoder.
        let _ = decoder.send_eof();
        while decoder.receive_frame(&mut raw).is_ok() {
            if scaler.run(&raw, &mut scaled).is_err() {
                continue;
            }
            if let Some(img) = frame_to_rgba(&scaled) {
                if tx.send(Arc::new(img)).is_err() {
                    return Ok(());
                }
            }
        }

        // File ended — the outer `loop` re-opens it for seamless repeat.
    }
}

/// Decode the first video frame from `path` and return it as an `RgbaImage`.
/// Useful for extracting a static thumbnail from a video texture layer.
pub fn decode_first_frame(path: &Path) -> Result<RgbaImage> {
    use ffmpeg_next::{
        codec::context::Context as CodecCtx,
        format,
        format::Pixel,
        media::Type,
        software::scaling::{context::Context as ScaleCtx, flag::Flags},
        util::frame::video::Video as VideoFrame,
    };

    ffmpeg_next::init().map_err(|e| anyhow!("FFmpeg init: {e}"))?;

    let mut ictx = format::input(&path)
        .map_err(|e| anyhow!("cannot open '{}': {e}", path.display()))?;

    let stream_idx = ictx
        .streams()
        .best(Type::Video)
        .ok_or_else(|| anyhow!("no video stream in '{}'", path.display()))?
        .index();

    let mut decoder = {
        let stream = ictx.stream(stream_idx).unwrap();
        CodecCtx::from_parameters(stream.parameters())
            .map_err(|e| anyhow!("codec context: {e}"))?
            .decoder()
            .video()
            .map_err(|e| anyhow!("video decoder: {e}"))?
    };

    let mut scaler = ScaleCtx::get(
        decoder.format(),
        decoder.width(),
        decoder.height(),
        Pixel::RGBA,
        decoder.width(),
        decoder.height(),
        Flags::BILINEAR,
    )
    .map_err(|e| anyhow!("scaler context: {e}"))?;

    let mut raw = VideoFrame::empty();
    let mut scaled = VideoFrame::empty();

    for (stream, packet) in ictx.packets() {
        if stream.index() != stream_idx {
            continue;
        }
        if decoder.send_packet(&packet).is_err() {
            continue;
        }
        while decoder.receive_frame(&mut raw).is_ok() {
            if scaler.run(&raw, &mut scaled).is_ok() {
                if let Some(img) = frame_to_rgba(&scaled) {
                    return Ok(img);
                }
            }
        }
    }

    Err(anyhow!("no decodable video frame found in '{}'", path.display()))
}

/// Copy one RGBA `VideoFrame` into an `RgbaImage`, trimming stride padding.
fn frame_to_rgba(frame: &ffmpeg_next::util::frame::video::Video) -> Option<RgbaImage> {
    let w = frame.width();
    let h = frame.height();
    let stride = frame.stride(0);
    let data = frame.data(0);

    let mut pixels = Vec::with_capacity((w * h * 4) as usize);
    for row in 0..h as usize {
        let start = row * stride;
        pixels.extend_from_slice(&data[start..start + w as usize * 4]);
    }

    RgbaImage::from_raw(w, h, pixels)
}
