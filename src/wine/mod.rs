//! Wine translation layer — the central hub for running Wallpaper Engine natively.
//!
//! wp-engine acts as a **translation host**:
//! - Launches `wallpaper_engine.exe` under Wine (headless — no WE UI ever appears)
//! - A hook DLL (`version.dll`) injected via `WINEDLLOVERRIDES` intercepts WE's
//!   D3D11 `Present` calls and forwards rendered frames to wp-engine via Unix socket
//! - wp-engine puts received frames directly onto the Wayland background surface
//!
//! # Modules
//! - [`WineEnv`]              — detect installation and WINEPREFIX
//! - [`process::WineProcess`] — launch WE and generic Wine applications
//! - [`ipc::IpcServer`]       — Unix socket server receiving rendered frames
//!
//! # Environment variables honoured
//! - `WINE`       — override the wine binary path (e.g. `/usr/bin/wine64`)
//! - `WINEPREFIX` — override the prefix directory

pub mod ipc;
pub mod process;

use anyhow::{anyhow, bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

// ── Public API ────────────────────────────────────────────────────────────────

/// A validated Wine installation ready to launch Windows applications.
#[derive(Debug, Clone)]
pub struct WineEnv {
    /// Absolute path to the wine binary (`wine` or `wine64`).
    pub binary: PathBuf,
    /// Whether the binary is a 64-bit Wine build.
    pub is_64bit: bool,
    /// The `WINEPREFIX` directory (created on first use by Wine itself).
    pub prefix: PathBuf,
    /// Version string returned by `wine --version` (e.g. `"wine-9.0"`).
    pub version: String,
}

impl WineEnv {
    /// Detect a usable Wine installation on the current system.
    ///
    /// Search order for the binary:
    /// 1. `$WINE` environment variable
    /// 2. `wine64` on `$PATH`
    /// 3. `wine` on `$PATH`
    ///
    /// Search order for the prefix:
    /// 1. `$WINEPREFIX` environment variable
    /// 2. `~/.local/share/wp-engine/wine`
    pub fn detect() -> Result<Self> {
        let binary = find_wine_binary()?;
        let is_64bit = binary
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.contains("64"))
            .unwrap_or(false);

        let version = query_wine_version(&binary)?;
        let prefix = resolve_prefix()?;

        Ok(Self { binary, is_64bit, prefix, version })
    }

    /// Build a [`Command`] that runs `exe` under this Wine environment.
    pub fn command(&self, exe: &Path) -> Command {
        let mut cmd = Command::new(&self.binary);
        cmd.arg(exe)
            .env("WINEPREFIX", &self.prefix)
            .env("WINEDEBUG", "-all");
        cmd
    }

    /// Return a human-readable summary of the detected Wine environment.
    pub fn summary(&self) -> String {
        format!(
            "{} ({}) — prefix: {}",
            self.version,
            self.binary.display(),
            self.prefix.display(),
        )
    }
}

// ── Detection helpers ─────────────────────────────────────────────────────────

fn find_wine_binary() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("WINE") {
        let p = PathBuf::from(&path);
        if !p.exists() {
            bail!(
                "$WINE is set to '{}' but that file does not exist",
                path
            );
        }
        return Ok(p);
    }

    for name in &["wine64", "wine"] {
        if let Some(path) = which(name) {
            return Ok(path);
        }
    }

    bail!(
        "Wine not found — install Wine to use Wallpaper Engine.\n\
         On Arch:   sudo pacman -S wine\n\
         On Ubuntu: sudo apt install wine"
    )
}

fn resolve_prefix() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("WINEPREFIX") {
        return Ok(PathBuf::from(path));
    }

    let base = dirs::data_local_dir()
        .ok_or_else(|| anyhow!("cannot determine XDG_DATA_HOME / ~/.local/share"))?;

    Ok(base.join("wp-engine").join("wine"))
}

fn query_wine_version(binary: &Path) -> Result<String> {
    let output = Command::new(binary)
        .arg("--version")
        .env("WINEDEBUG", "-all")
        .output()
        .with_context(|| format!("failed to run '{}'", binary.display()))?;

    if !output.status.success() {
        bail!(
            "'{}' --version exited with status {}",
            binary.display(),
            output.status
        );
    }

    let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if version.is_empty() {
        bail!("'{}' --version produced no output", binary.display());
    }

    Ok(version)
}

fn which(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|path_var| {
            std::env::split_paths(&path_var).collect::<Vec<_>>().into_iter()
        })
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

// ── Prefix helpers ────────────────────────────────────────────────────────────

/// Return `true` if the WINEPREFIX has been initialised (contains `system.reg`).
pub fn prefix_is_initialised(prefix: &Path) -> bool {
    prefix.join("system.reg").exists()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn which_finds_sh() {
        let found = which("sh");
        assert!(found.is_some(), "expected to find 'sh' on PATH");
        let path = found.unwrap();
        assert!(path.is_absolute());
        assert!(path.is_file());
    }

    #[test]
    fn which_misses_nonexistent() {
        assert!(which("__wp_engine_nonexistent_binary__").is_none());
    }

    #[test]
    fn resolve_prefix_respects_env() {
        std::env::set_var("WINEPREFIX", "/tmp/test-wp-prefix");
        let prefix = resolve_prefix().unwrap();
        std::env::remove_var("WINEPREFIX");
        assert_eq!(prefix, PathBuf::from("/tmp/test-wp-prefix"));
    }

    #[test]
    fn resolve_prefix_default_path() {
        std::env::remove_var("WINEPREFIX");
        let prefix = resolve_prefix().unwrap();
        assert!(
            prefix.ends_with("wp-engine/wine"),
            "unexpected default prefix: {}",
            prefix.display()
        );
    }

    #[test]
    fn prefix_is_initialised_false_for_nonexistent() {
        assert!(!prefix_is_initialised(Path::new("/tmp/__no_such_prefix__")));
    }
}
