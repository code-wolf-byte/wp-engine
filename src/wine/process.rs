//! Wine child-process management.

use anyhow::{Context, Result};
use std::io::BufRead;
use std::path::Path;
use std::process::{Child, Stdio};

use super::WineEnv;

pub struct WineProcess {
    child: Child,
    pid: u32,
    width: u32,
    height: u32,
}

impl WineProcess {
    /// Launch a generic Windows executable inside a Wine virtual desktop.
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
    pub fn launch_wallpaper_engine(
        env: &WineEnv,
        we_exe: &Path,
        wallpaper_path: &Path,
        socket_path: &Path,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        let desktop_arg = format!("/desktop=wallpaper,{}x{}", width, height);

        // ── Find hook DLL first so we can install it into the prefix ─────────
        let hook_dll = find_hook_dll(we_exe);

        match &hook_dll {
            Some(p) => eprintln!("[wp-engine]   version.dll   : {} (found)", p.display()),
            None    => eprintln!("[wp-engine]   version.dll   : NOT FOUND — hook will not load"),
        }

        // ── Ensure WINEPREFIX is initialised and hook DLL is installed ────────
        ensure_prefix_ready(env, hook_dll.as_deref())?;

        // ── Log launch parameters ─────────────────────────────────────────────
        eprintln!("[wp-engine] launching Wallpaper Engine");
        eprintln!("[wp-engine]   wine binary   : {}", env.binary.display());
        eprintln!("[wp-engine]   we_exe        : {}", we_exe.display());
        eprintln!("[wp-engine]   wallpaper_path: {}", wallpaper_path.display());
        eprintln!("[wp-engine]   socket_path   : {}", socket_path.display());
        eprintln!("[wp-engine]   desktop       : {}", desktop_arg);
        eprintln!("[wp-engine]   WINEPREFIX    : {}", env.prefix.display());

        // Convert Linux path to Wine's Z: drive Windows path.
        // e.g. /home/user/... → Z:\home\user\...
        let win_wallpaper_path = format!(
            "Z:{}",
            wallpaper_path.to_string_lossy().replace('/', "\\")
        );

        let mut cmd = std::process::Command::new(&env.binary);
        cmd.arg("explorer")
            .arg(&desktop_arg)
            .arg(we_exe)
            .arg("-file").arg(&win_wallpaper_path)
            .arg("-width").arg(width.to_string())
            .arg("-height").arg(height.to_string())
            .arg("-silent")
            .env("WINEPREFIX", &env.prefix)
            // Show fixme + error channel only — enough to spot loading failures
            // without the usual wall of noise.
            .env("WINEDEBUG", "fixme-all,err+loaddll,err+module,err+relay")
            // n,b = native first, then builtin fallback.
            // 32-bit Wine subprocesses that can't load the 64-bit DLL will fall
            // back to Wine's own builtin version.dll stub gracefully.
            // version=n,b  — load our hook DLL (native) before Wine's stub
            // d3d11=n,dxgi=n — force DXVK's native DLLs over Wine's WineD3D
            //                  (DXVK uses Vulkan, bypassing EGL/DRI2 which
            //                   fails with NVIDIA on Wayland)
            .env("WINEDLLOVERRIDES", "version=n,b;d3d11=n;dxgi=n")
            .env("WP_ENGINE_SOCKET", socket_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped()); // capture so we can forward with a [wine] prefix

        let mut child = cmd
            .spawn()
            .with_context(|| {
                format!("failed to spawn Wallpaper Engine at '{}'", we_exe.display())
            })?;

        let pid = child.id();
        eprintln!("[wp-engine] WE process spawned (pid {pid})");

        // Forward Wine's stderr to our stderr on a background thread.
        if let Some(stderr) = child.stderr.take() {
            std::thread::spawn(move || {
                let reader = std::io::BufReader::new(stderr);
                for line in reader.lines().map_while(Result::ok) {
                    eprintln!("[wine] {line}");
                }
                eprintln!("[wine] stderr closed");
            });
        }

        Ok(Self { child, pid, width, height })
    }

    pub fn pid(&self) -> u32 { self.pid }
    pub fn width(&self) -> u32 { self.width }
    pub fn height(&self) -> u32 { self.height }

    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
    }
}

impl Drop for WineProcess {
    fn drop(&mut self) {
        eprintln!("[wp-engine] killing WE process (pid {})", self.pid);
        self.kill();
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Ensure the WINEPREFIX directory exists, is initialised, and (if we have
/// the hook DLL) has `version.dll` installed into `system32`.
///
/// `wineboot --init` is run **without** `WINEDLLOVERRIDES` so that Wine's own
/// internal setup processes (msiexec, services) can load the real version.dll
/// from their built-in search paths without interference.
fn ensure_prefix_ready(env: &WineEnv, hook_dll: Option<&Path>) -> Result<()> {
    // 1. Create prefix directory if it doesn't exist.
    if !env.prefix.exists() {
        eprintln!("[wp-engine] creating WINEPREFIX at {}", env.prefix.display());
        std::fs::create_dir_all(&env.prefix)
            .with_context(|| format!("failed to create WINEPREFIX at {}", env.prefix.display()))?;
    }

    // 2. Run wineboot if the prefix has never been initialised.
    //    This creates drive_c/windows/system32 and the registry files.
    if !super::prefix_is_initialised(&env.prefix) {
        eprintln!("[wp-engine] initialising WINEPREFIX (first-time setup, may take 30–60 s)…");
        let status = std::process::Command::new(&env.binary)
            .arg("wineboot")
            .arg("--init")
            .env("WINEPREFIX", &env.prefix)
            .env("WINEDEBUG", "-all")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .context("failed to run wine wineboot --init")?;
        eprintln!("[wp-engine] wineboot finished (exit: {:?})", status.code());
    }

    // 3. Install the hook DLL into system32 so Wine finds it for every process
    //    (including Wine's own internal ones that don't inherit WINEDLLPATH).
    if let Some(src) = hook_dll {
        let sys32 = env.prefix.join("drive_c/windows/system32");
        if sys32.exists() {
            let dest = sys32.join("version.dll");
            eprintln!(
                "[wp-engine] installing hook DLL into prefix system32: {}",
                dest.display()
            );
            std::fs::copy(src, &dest)
                .with_context(|| format!("failed to copy version.dll to {}", dest.display()))?;
        } else {
            eprintln!(
                "[wp-engine] warn: {} does not exist — hook DLL not installed",
                sys32.display()
            );
        }
    }

    Ok(())
}

fn find_hook_dll(we_exe: &Path) -> Option<std::path::PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        eprintln!("[wp-engine]   current_exe   : {}", exe.display());
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("version.dll");
            eprintln!("[wp-engine]   dll candidate : {}", candidate.display());
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    if let Some(dir) = we_exe.parent() {
        let candidate = dir.join("version.dll");
        eprintln!("[wp-engine]   dll candidate : {}", candidate.display());
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}
