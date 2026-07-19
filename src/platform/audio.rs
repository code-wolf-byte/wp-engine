//! Platform-specific capture-device selection for the audio-reactive path.
//!
//! Capture, FFT and the `g_AudioSpectrum*` uniforms are all cross-platform and
//! live in [`crate::engine::audio`] on top of cpal. The one genuinely
//! OS-dependent decision is *which* device carries desktop output, because
//! "record what the speakers are playing" is spelled differently everywhere:
//!
//! - **Linux** (PulseAudio/PipeWire): output sinks expose a paired `.monitor`
//!   source. Real loopback, present by default.
//! - **macOS**: the OS ships no loopback source. It needs a virtual device
//!   (BlackHole, Soundflower, Loopback) that the user has installed and set as
//!   an output, so we match those by name.
//! - **Windows**: WASAPI exposes loopback on the *output* device rather than as
//!   an input, so the default output is the right handle.
//!
//! Falling through to the default input device (usually a microphone) is
//! deliberate: a mic still makes the wallpaper react to sound, which is closer
//! to the intent than silence. The fallback is logged, because "reacting to the
//! room" and "reacting to the music" look similar enough to hide a
//! misconfiguration for a long time.
//!
//! On Linux the hint list alone isn't enough: cpal's ALSA backend enumerates
//! `pipewire`/`pulse`/`default`, never the per-sink `.monitor` sources, so a
//! substring match finds nothing on a PulseAudio/PipeWire desktop. But ALSA's
//! `pulse` plugin honours the `PULSE_SOURCE` environment variable, so naming
//! the monitor there and opening `pulse` gets real desktop capture without a
//! libpulse dependency. See [`route_pulse_monitor`].

use cpal::traits::{DeviceTrait, HostTrait};

/// Device-name fragments that indicate a desktop-output capture source on this
/// platform, in priority order. Matched case-insensitively as substrings.
#[cfg(target_os = "linux")]
const LOOPBACK_HINTS: &[&str] = &["monitor"];

#[cfg(target_os = "macos")]
const LOOPBACK_HINTS: &[&str] = &["blackhole", "soundflower", "loopback", "aggregate"];

#[cfg(target_os = "windows")]
const LOOPBACK_HINTS: &[&str] = &["stereo mix", "what u hear", "loopback"];

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
const LOOPBACK_HINTS: &[&str] = &["monitor", "loopback"];

/// How the chosen device was found — for logging, and so callers can tell a
/// real desktop capture from a microphone fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureSource {
    /// A real desktop-output loopback: the wallpaper reacts to what's playing.
    Loopback,
    /// Whatever the OS calls the default input — usually a microphone.
    DefaultInput,
    /// Windows-style loopback, which hangs off the output device.
    OutputLoopback,
}

/// Pick the best capture device for audio reactivity on this platform.
///
/// Returns the device plus how it was found. `None` means the machine exposes
/// no usable capture device at all, and the caller keeps a silent spectrum.
pub fn pick_capture_device(host: &cpal::Host) -> Option<(cpal::Device, CaptureSource)> {
    // Preferred on Linux: point ALSA's pulse plugin at the default sink's
    // monitor. Tried first because it's the only path that reaches real
    // desktop audio on PipeWire.
    #[cfg(target_os = "linux")]
    if let Some(d) = route_pulse_monitor(host) {
        return Some((d, CaptureSource::Loopback));
    }
    if let Some(d) = match_hint(host) {
        return Some((d, CaptureSource::Loopback));
    }
    // WASAPI does loopback through the render endpoint, so the default output
    // is a capture handle there — but not on hosts where it isn't.
    #[cfg(target_os = "windows")]
    if let Some(d) = host.default_output_device() {
        return Some((d, CaptureSource::OutputLoopback));
    }
    host.default_input_device()
        .map(|d| (d, CaptureSource::DefaultInput))
}

/// Route ALSA's `pulse` device at the default sink's `.monitor` source.
///
/// `pactl` ships with PulseAudio/PipeWire, so it's present wherever this can
/// work at all; a missing binary just means we fall through. Setting
/// `PULSE_SOURCE` is process-global and must happen before the stream opens,
/// which is why it lives here rather than at the call site.
#[cfg(target_os = "linux")]
fn route_pulse_monitor(host: &cpal::Host) -> Option<cpal::Device> {
    let sink = std::process::Command::new("pactl")
        .arg("get-default-sink")
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let sink = String::from_utf8(sink.stdout).ok()?.trim().to_string();
    if sink.is_empty() {
        return None;
    }
    let monitor = format!("{sink}.monitor");

    // Confirm the monitor actually exists before claiming Loopback — a stale
    // or renamed sink would otherwise open a stream that never delivers.
    let sources = std::process::Command::new("pactl")
        .args(["list", "short", "sources"])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    if !String::from_utf8_lossy(&sources.stdout).contains(&monitor) {
        tracing::debug!(target: "audio", "monitor source '{monitor}' not present");
        return None;
    }

    let device = host
        .input_devices()
        .ok()?
        .find(|d| d.name().map(|n| n == "pulse").unwrap_or(false))?;
    // SAFETY-adjacent note: single-threaded setup path, set before any audio
    // stream exists, and the ALSA pulse plugin reads it at stream open.
    std::env::set_var("PULSE_SOURCE", &monitor);
    tracing::info!(target: "audio", "capturing desktop audio from '{monitor}'");
    Some(device)
}

fn match_hint(host: &cpal::Host) -> Option<cpal::Device> {
    let devices = host.input_devices().ok()?;
    // Collect once: `Devices` is a one-shot iterator, and we need several
    // passes to honour hint priority rather than device enumeration order.
    let named: Vec<(String, cpal::Device)> = devices
        .filter_map(|d| Some((d.name().ok()?.to_lowercase(), d)))
        .collect();
    LOOPBACK_HINTS.iter().find_map(|hint| {
        named
            .iter()
            .find(|(name, _)| name.contains(hint))
            .map(|(_, d)| d.clone())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every platform must offer at least one hint, or `match_hint` silently
    /// degrades to the microphone fallback on that OS.
    #[test]
    fn platform_has_loopback_hints() {
        assert!(!LOOPBACK_HINTS.is_empty());
        assert!(
            LOOPBACK_HINTS.iter().all(|h| *h == h.to_lowercase()),
            "hints are matched against a lowercased name, so they must be lowercase"
        );
    }

    /// Hint order is priority order, so the list must not contain a substring
    /// of an earlier entry (which could never be reached).
    #[test]
    fn hints_are_not_shadowed_by_earlier_ones() {
        for (i, h) in LOOPBACK_HINTS.iter().enumerate() {
            for earlier in &LOOPBACK_HINTS[..i] {
                assert!(
                    !h.contains(earlier),
                    "'{h}' is unreachable: '{earlier}' matches it first"
                );
            }
        }
    }
}
