//! Wine child-process management.
//!
//! - [`WineProcess::launch`] — generic Windows executable inside Wine's virtual desktop
//! - [`WineProcess::launch_wallpaper_engine`] — launch WE with hook DLL injection

use anyhow::{Context, Result};
use std::path::Path;
use std::process::{Child, Stdio};

use super::WineEnv;

// ── Public API ────────────────────────────────────────────────────────────────

/// A running Wine child process.
///
/// The process is killed automatically when this struct is dropped.
pub struct WineProcess {
    child: Child,
    /// Linux PID of the `wine` binary.
    pid: u32,
    /// Width of the virtual desktop the process renders into.
    width: u32,
    /// Height of the virtual desktop the process renders into.
    height: u32,
}

impl WineProcess {
    /// Launch `exe` inside a Wine virtual desktop named `"wallpaper"`.
    ///
    /// Invocation:
    /// ```text
    /// wine explorer /desktop=wallpaper,<width>x<height> <exe>
    /// ```
    pub fn launch(env: &WineEnv, exe: &Path, width: u32, height: u32) -> Result<Self> {
        let desktop_arg = format!("/desktop=wallpaper,{}x{}", width, height);

        let child = std::process::Command::new(&env.binary)
            .arg("explorer")
            .arg(&desktop_arg)
            .arg(exe)
            .env("WINEPREFIX", &env.prefix)
            .env("WINEDEBUG", "-all")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("failed to spawn Wine for '{}'", exe.display()))?;

        let pid = child.id();
        Ok(Self { child, pid, width, height })
    }

    /// Launch `wallpaper_engine.exe` under Wine with the hook DLL injected.
    ///
    /// The hook DLL (`version.dll`) is picked up from the directory containing
    /// the wp-engine binary. Wine loads it via `WINEDLLOVERRIDES=version=n`.
    ///
    /// Invocation:
    /// ```text
    /// wine explorer /desktop=wallpaper,<w>x<h> wallpaper_engine.exe <wallpaper_path>
    /// ```
    ///
    /// The hook DLL reads `WP_ENGINE_SOCKET` to know where to connect.
    pub fn launch_wallpaper_engine(
        env: &WineEnv,
        we_exe: &Path,
        wallpaper_path: &Path,
        socket_path: &Path,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        let desktop_arg = format!("/desktop=wallpaper,{}x{}", width, height);

        // WINEDLLPATH is Wine's DLL search path (colon-separated Linux paths).
        // When WINEDLLOVERRIDES=version=n is set, Wine looks for version.dll here.
        let hook_dll = find_hook_dll(we_exe);
        let winedllpath = hook_dll
            .as_ref()
            .and_then(|p| p.parent())
            .map(|d| d.to_string_lossy().into_owned())
            .unwrap_or_default();

        let mut cmd = std::process::Command::new(&env.binary);
        cmd.arg("explorer")
            .arg(&desktop_arg)
            .arg(we_exe)
            .arg(wallpaper_path)
            .env("WINEPREFIX", &env.prefix)
            .env("WINEDEBUG", "-all")
            // Load our version.dll instead of Wine's built-in stub
            .env("WINEDLLOVERRIDES", "version=n")
            // Tell the hook DLL where to connect
            .env("WP_ENGINE_SOCKET", socket_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        if !winedllpath.is_empty() {
            cmd.env("WINEDLLPATH", &winedllpath);
        }

        let child = cmd
            .spawn()
            .with_context(|| {
                format!("failed to spawn Wallpaper Engine at '{}'", we_exe.display())
            })?;

        let pid = child.id();
        Ok(Self { child, pid, width, height })
    }

    /// Linux PID of the Wine process.
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Width of the virtual desktop, in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height of the virtual desktop, in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Returns `true` if the child process is still running.
    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Kill the child process immediately (best-effort; errors are silenced).
    pub fn kill(&mut self) {
        let _ = self.child.kill();
    }
}

impl Drop for WineProcess {
    fn drop(&mut self) {
        self.kill();
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Locate `version.dll` (the hook DLL) relative to the current executable,
/// or failing that, next to `we_exe`.
fn find_hook_dll(we_exe: &Path) -> Option<std::path::PathBuf> {
    // 1. Same directory as the wp-engine binary
    if let Ok(exe) = std::env::current_exe() {
        let candidate = exe.parent()?.join("version.dll");
        if candidate.exists() {
            return Some(candidate);
        }
    }

    // 2. Same directory as wallpaper_engine.exe
    let candidate = we_exe.parent()?.join("version.dll");
    if candidate.exists() {
        return Some(candidate);
    }

    None
}
