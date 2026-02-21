mod renderer;
mod ui;
mod wayland;
mod workshop;

use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use renderer::{FrameSource, WallpaperContent};
use std::path::PathBuf;
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
    let handle = wayland::spawn_wallpaper(frame_source)?;
    println!("Wallpaper active. Press Ctrl-C to exit.");
    handle.wait();
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
    let handle = wayland::spawn_wallpaper(frame_source)?;
    println!("Wallpaper active. Press Ctrl-C to exit.");
    handle.wait();
    Ok(())
}
