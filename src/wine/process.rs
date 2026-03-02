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

        // ── Ensure WINEPREFIX directory exists (Wine refuses to start otherwise) ─
        if !env.prefix.exists() {
            eprintln!("[wp-engine] creating WINEPREFIX at {}", env.prefix.display());
            std::fs::create_dir_all(&env.prefix)
                .with_context(|| format!("failed to create WINEPREFIX at {}", env.prefix.display()))?;
        }

        // WINEDLLPATH: Linux-side directories Wine searches for native DLLs.
        let hook_dll = find_hook_dll(we_exe);
        let winedllpath = hook_dll
            .as_ref()
            .and_then(|p| p.parent())
            .map(|d| d.to_string_lossy().into_owned())
            .unwrap_or_default();

        // ── Log launch parameters ──────────────────────────────────────────────
        eprintln!("[wp-engine] launching Wallpaper Engine");
        eprintln!("[wp-engine]   wine binary   : {}", env.binary.display());
        eprintln!("[wp-engine]   we_exe        : {}", we_exe.display());
        eprintln!("[wp-engine]   wallpaper_path: {}", wallpaper_path.display());
        eprintln!("[wp-engine]   socket_path   : {}", socket_path.display());
        eprintln!("[wp-engine]   desktop       : {}", desktop_arg);
        eprintln!("[wp-engine]   WINEPREFIX    : {}", env.prefix.display());
        match &hook_dll {
            Some(p) => eprintln!("[wp-engine]   version.dll   : {} (found)", p.display()),
            None    => eprintln!("[wp-engine]   version.dll   : NOT FOUND — hook will not load"),
        }
        eprintln!("[wp-engine]   WINEDLLPATH   : {}", if winedllpath.is_empty() { "(unset)" } else { &winedllpath });

        let mut cmd = std::process::Command::new(&env.binary);
        cmd.arg("explorer")
            .arg(&desktop_arg)
            .arg(we_exe)
            .arg(wallpaper_path)
            .env("WINEPREFIX", &env.prefix)
            // Show fixme + error channel only — enough to spot loading failures
            // without the usual wall of noise.
            .env("WINEDEBUG", "fixme-all,err+loaddll,err+module,err+relay")
            .env("WINEDLLOVERRIDES", "version=n")
            .env("WP_ENGINE_SOCKET", socket_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped()); // capture so we can forward with a [wine] prefix

        if !winedllpath.is_empty() {
            cmd.env("WINEDLLPATH", &winedllpath);
        }

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
