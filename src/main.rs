mod engine;
mod platform;
mod render;
mod ui;
mod wine;
mod workshop;

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use render::{FrameSource, RenderSettings};
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
    /// Apply a Workshop wallpaper by ID via Wallpaper Engine (CLI, blocks until Ctrl-C)
    Set {
        id: String,
    },
    /// Show metadata for a Workshop item (CLI)
    Info {
        id: String,
    },
    /// List available GPU adapters (system probe)
    Probe,
    /// Inspect (and optionally extract) a Wallpaper Engine PKG archive
    PkgInfo {
        path: PathBuf,
        /// Dump all contained files to this directory
        #[arg(long)]
        dump: Option<PathBuf>,
    },
    /// Probe the Wine environment (binary, version, WINEPREFIX)
    WineProbe,
}

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        None => run_ui(),
        Some(Command::List { r#type }) => cmd_list(r#type),
        Some(Command::Set { id })      => cmd_set(&id),
        Some(Command::Info { id })     => cmd_info(&id),
        Some(Command::Probe)           => cmd_probe(),
        Some(Command::PkgInfo { path, dump }) => cmd_pkg_info(&path, dump.as_deref()),
        Some(Command::WineProbe)       => cmd_wine_probe(),
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
    use wine::{ipc::IpcServer, process::WineProcess, WineEnv};

    let w = workshop::find_by_id(id)
        .ok_or_else(|| anyhow!("workshop item '{id}' not found"))?;

    // Pass the wallpaper directory — WE finds scene.pkg / index.html internally.
    let wallpaper_path = w.path.clone();

    let we_exe = workshop::find_wallpaper_engine_exe()
        .ok_or_else(|| anyhow!("wallpaper_engine.exe not found — install Wallpaper Engine via Steam"))?;

    let env = WineEnv::detect()?;

    let ipc_server = IpcServer::bind()?;
    let socket_path = ipc_server.socket_path.clone();

    println!("Launching Wallpaper Engine for \"{}\"...", w.title());

    let _we_process = WineProcess::launch_wallpaper_engine(
        &env,
        &we_exe,
        &wallpaper_path,
        &socket_path,
        1920,
        1080,
    )?;

    let (tx, rx) = std::sync::mpsc::sync_channel(2);
    wine::ipc::start_frame_receiver(ipc_server, tx);

    println!("Waiting for first frame from hook DLL...");
    let first_frame = rx
        .recv()
        .map_err(|_| anyhow!("hook DLL did not send any frames — check version.dll placement"))?;

    let frame_source = FrameSource::from_ipc(first_frame, rx);
    let settings = Arc::new(Mutex::new(RenderSettings::default()));
    let handle = platform::detect_platform().spawn_wallpaper(frame_source, settings)?;

    println!("Wallpaper active. Press Ctrl-C to exit.");
    handle.wait();
    Ok(())
}

fn cmd_probe() -> Result<()> {
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
    println!("\n{} adapter(s) found", adapters.len());
    Ok(())
}

fn cmd_wine_probe() -> Result<()> {
    print!("Detecting Wine... ");
    match wine::WineEnv::detect() {
        Ok(env) => {
            println!("OK");
            println!("  {}", env.summary());
            println!(
                "  Prefix initialised: {}",
                wine::prefix_is_initialised(&env.prefix)
            );
        }
        Err(e) => {
            println!("NOT FOUND");
            println!("  {e}");
        }
    }

    print!("Locating wallpaper_engine.exe... ");
    match workshop::find_wallpaper_engine_exe() {
        Some(p) => println!("found: {}", p.display()),
        None    => println!("NOT FOUND (install Wallpaper Engine via Steam)"),
    }

    Ok(())
}

fn cmd_pkg_info(path: &std::path::Path, dump: Option<&std::path::Path>) -> Result<()> {
    let pkg = engine::Package::from_file(path)?;

    println!("Files   : {}", pkg.len());
    println!();

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
