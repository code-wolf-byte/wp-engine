mod engine;
mod platform;
mod render;
mod ui;
mod workshop;

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use render::{FrameSource, RenderSettings, WallpaperContent};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use workshop::Wallpaper;

#[derive(Parser)]
#[command(
    name = "wp-engine",
    about = "Wallpaper Engine client for Wayland",
    long_about = "Browse and apply Steam Workshop wallpapers on Wayland desktops.\n\
                  Run without arguments to open the graphical interface."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// List available Workshop wallpapers (CLI)
    List {
        #[arg(long, help = "Filter by type: video, scene, web, application")]
        r#type: Option<String>,
    },
    /// Apply a wallpaper by Steam Workshop ID (CLI, blocks until Ctrl-C)
    Set {
        id: String,
    },
    /// Apply any image file as wallpaper (CLI, blocks until Ctrl-C)
    SetFile {
        path: PathBuf,
    },
    /// Show metadata for a Workshop item (CLI)
    Info {
        id: String,
    },
    /// List available GPU adapters (PAL probe)
    Probe,
    /// Inspect (and optionally extract) a Wallpaper Engine PKG archive
    PkgInfo {
        path: PathBuf,
        /// Dump all contained files to this directory
        #[arg(long)]
        dump: Option<PathBuf>,
    },
}

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        // No subcommand → open GUI
        None => run_ui(),
        Some(Command::List { r#type }) => cmd_list(r#type),
        Some(Command::Set { id }) => cmd_set(&id),
        Some(Command::SetFile { path }) => cmd_set_file(&path),
        Some(Command::Info { id }) => cmd_info(&id),
        Some(Command::Probe)       => cmd_probe(),
        Some(Command::PkgInfo { path, dump }) => cmd_pkg_info(&path, dump.as_deref()),
    };

    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

// ── GUI ───────────────────────────────────────────────────────────────────────

fn run_ui() -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("wp-engine")
            .with_inner_size([1100.0, 700.0])
            .with_min_inner_size([800.0, 500.0]),
        ..Default::default()
    };

    eframe::run_native(
        "wp-engine",
        options,
        Box::new(|cc| Ok(Box::new(ui::WpApp::new(cc)))),
    )
    .map_err(|e| anyhow!("GUI error: {e}"))
}

// ── CLI commands ──────────────────────────────────────────────────────────────

fn cmd_list(type_filter: Option<String>) -> Result<()> {
    let wallpapers = workshop::scan_wallpapers();

    if wallpapers.is_empty() {
        println!("No Wallpaper Engine Workshop items found.");
        println!();
        println!(
            "Expected path: ~/.local/share/Steam/steamapps/workshop/content/{}/",
            workshop::WALLPAPER_ENGINE_APP_ID
        );
        return Ok(());
    }

    let items: Vec<&Wallpaper> = wallpapers
        .iter()
        .filter(|w| {
            type_filter
                .as_deref()
                .map(|f| w.wallpaper_type().to_string() == f)
                .unwrap_or(true)
        })
        .collect();

    println!("{:<20} {:<12} {}", "ID", "Type", "Title");
    println!("{}", "─".repeat(64));
    for w in &items {
        println!("{:<20} {:<12} {}", w.workshop_id, w.wallpaper_type(), w.title());
    }
    println!("\n{} wallpaper(s)", items.len());
    Ok(())
}

fn cmd_info(id: &str) -> Result<()> {
    let w = workshop::find_by_id(id)
        .ok_or_else(|| anyhow!("workshop item '{id}' not found"))?;

    println!("ID:          {}", w.workshop_id);
    println!("Title:       {}", w.title());
    println!("Type:        {}", w.wallpaper_type());
    println!("Path:        {}", w.path.display());
    if let Some(f) = w.wallpaper_file() {
        println!("File:        {} {}", f.display(), if f.exists() { "" } else { "(MISSING)" });
    }
    if let Some(tags) = &w.project.tags {
        if !tags.is_empty() {
            println!("Tags:        {}", tags.join(", "));
        }
    }
    Ok(())
}

fn cmd_set(id: &str) -> Result<()> {
    let w = workshop::find_by_id(id)
        .ok_or_else(|| anyhow!("workshop item '{id}' not found"))?;

    // Type support is checked inside from_wallpaper; unsupported types return Err.
    let content = WallpaperContent::from_wallpaper(&w)?;

    let path = w.wallpaper_file().unwrap_or_default();
    println!("Loading: {}", path.display());

    let frame_source = FrameSource::from_content(content)?;
    println!("Applying \"{}\" to all outputs...", w.title());
    let settings = Arc::new(Mutex::new(RenderSettings::default()));
    let handle = platform::detect_platform().spawn_wallpaper(frame_source, settings)?;
    println!("Wallpaper active. Press Ctrl-C to exit.");
    handle.wait();
    Ok(())
}

fn cmd_probe() -> Result<()> {
    // ── 1. List all visible adapters ─────────────────────────────────────────
    let adapters = platform::probe_adapters();
    if adapters.is_empty() {
        println!("No GPU adapters found.");
        return Ok(());
    }
    println!("{:<40} {:<10} {:<12} {}", "Name", "Backend", "Type", "PCI IDs");
    println!("{}", "─".repeat(74));
    for a in &adapters {
        println!(
            "{:<40} {:<10} {:<12} {:04x}:{:04x}",
            a.name, a.backend, a.device_type, a.vendor_id, a.device_id
        );
    }
    println!("\n{} adapter(s) found\n", adapters.len());

    // ── 2. Open the best device to confirm driver access works ───────────────
    print!("Opening best device... ");
    match platform::open_device() {
        Ok(dev) => println!(
            "OK  →  {} ({}, {})",
            dev.info.name, dev.info.backend, dev.info.device_type
        ),
        Err(e) => println!("FAILED: {e}"),
    }

    Ok(())
}

fn cmd_set_file(path: &PathBuf) -> Result<()> {
    if !path.exists() {
        return Err(anyhow!("file not found: {}", path.display()));
    }
    println!("Loading: {}", path.display());
    let content = WallpaperContent::from_path(path)?;
    println!("Applying to all outputs…");
    let frame_source = FrameSource::from_content(content)?;
    let settings = Arc::new(Mutex::new(RenderSettings::default()));
    let handle = platform::detect_platform().spawn_wallpaper(frame_source, settings)?;
    println!("Wallpaper active. Press Ctrl-C to exit.");
    handle.wait();
    Ok(())
}

fn cmd_pkg_info(path: &std::path::Path, dump: Option<&std::path::Path>) -> Result<()> {
    let pkg = engine::Package::from_file(path)?;

    println!("Files   : {}", pkg.len());
    println!();

    // Collect and sort paths for a stable, readable table.
    let mut paths: Vec<&str> = pkg.paths().collect();
    paths.sort_unstable();

    for p in &paths {
        let size_kb = pkg.get(p).map(|b| b.len()).unwrap_or(0) / 1024;
        println!("{size_kb:>6} KB  {p}");
    }

    if let Some(dump_dir) = dump {
        std::fs::create_dir_all(dump_dir)
            .with_context(|| format!("failed to create dump directory: {}", dump_dir.display()))?;

        let mut count = 0usize;
        for p in &paths {
            if let Some(bytes) = pkg.get(p) {
                let dest = dump_dir.join(p);
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent).with_context(|| {
                        format!("failed to create directory: {}", parent.display())
                    })?;
                }
                std::fs::write(&dest, bytes)
                    .with_context(|| format!("failed to write: {}", dest.display()))?;
                count += 1;
            }
        }
        println!("\nExtracted {count} files to {}", dump_dir.display());
    }

    Ok(())
}
