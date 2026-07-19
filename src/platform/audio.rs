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
//! ponytail: known ceiling on PipeWire. cpal's ALSA backend enumerates
//! `pipewire`/`pulse`/`default`, not the per-sink `.monitor` sources, so the
//! hint below finds nothing and we land on the mic. Real desktop capture there
//! needs the PulseAudio API (or `pactl load-module module-loopback`) rather
//! than a better substring — upgrade path is a libpulse source enumerator
//! behind this same function.

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
