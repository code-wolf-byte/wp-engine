use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Wallpaper Engine Steam App ID
pub const WALLPAPER_ENGINE_APP_ID: &str = "431960";

/// Wallpaper type as reported in project.json.
/// Wallpaper Engine uses mixed capitalisation across versions, so we alias both.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum WallpaperType {
    #[serde(alias = "Video")]
    Video,
    #[serde(alias = "Scene")]
    Scene,
    #[serde(alias = "Web")]
    Web,
    #[serde(alias = "Application")]
    Application,
    #[serde(other)]
    Unknown,
}

impl std::fmt::Display for WallpaperType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WallpaperType::Video => write!(f, "video"),
            WallpaperType::Scene => write!(f, "scene"),
            WallpaperType::Web => write!(f, "web"),
            WallpaperType::Application => write!(f, "application"),
            WallpaperType::Unknown => write!(f, "unknown"),
        }
    }
}

/// Parsed project.json from a Wallpaper Engine Workshop item
#[derive(Debug, Clone, Deserialize)]
pub struct Project {
    pub title: Option<String>,
    pub file: Option<String>,
    pub preview: Option<String>,
    #[serde(rename = "type")]
    pub wallpaper_type: Option<WallpaperType>,
    pub tags: Option<Vec<String>>,
    pub description: Option<String>,
    pub contentrating: Option<String>,
}

/// A resolved Workshop wallpaper entry (project.json + its directory)
#[derive(Debug, Clone)]
pub struct Wallpaper {
    /// Steam Workshop item ID (directory name)
    pub workshop_id: String,
    /// Root directory of this wallpaper
    pub path: PathBuf,
    pub project: Project,
}

impl Wallpaper {
    /// Absolute path to the main wallpaper file (video, image, html, etc.)
    pub fn wallpaper_file(&self) -> Option<PathBuf> {
        self.project.file.as_ref().map(|f| self.path.join(f))
    }

    /// Absolute path to the preview image
    pub fn preview_file(&self) -> Option<PathBuf> {
        self.project.preview.as_ref().map(|f| self.path.join(f))
    }

    pub fn title(&self) -> &str {
        self.project.title.as_deref().unwrap_or("<untitled>")
    }

    pub fn wallpaper_type(&self) -> &WallpaperType {
        self.project
            .wallpaper_type
            .as_ref()
            .unwrap_or(&WallpaperType::Unknown)
    }
}

/// Locate Steam library roots by reading libraryfolders.vdf
fn find_steam_library_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();

    let candidates = [
        dirs::home_dir().map(|h| h.join(".local/share/Steam")),
        dirs::home_dir().map(|h| h.join(".steam/steam")),
    ];

    for candidate in candidates.into_iter().flatten() {
        if !candidate.exists() {
            continue;
        }
        // Resolve symlinks so ~/.steam/steam (→ ~/.local/share/Steam) isn't scanned twice
        let real = std::fs::canonicalize(&candidate).unwrap_or(candidate);
        if !roots.contains(&real) {
            roots.push(real);
        }
    }

    roots
}

/// Return all Workshop content directories for Wallpaper Engine across all Steam libraries
pub fn find_workshop_dirs() -> Vec<PathBuf> {
    find_steam_library_roots()
        .into_iter()
        .map(|root| {
            root.join("steamapps")
                .join("workshop")
                .join("content")
                .join(WALLPAPER_ENGINE_APP_ID)
        })
        .filter(|p| p.exists())
        .collect()
}

/// Load and parse a project.json inside a wallpaper directory
fn load_project(wallpaper_dir: &Path) -> Result<Project> {
    let project_path = wallpaper_dir.join("project.json");
    let data = std::fs::read_to_string(&project_path)
        .with_context(|| format!("reading {}", project_path.display()))?;
    serde_json::from_str(&data).with_context(|| format!("parsing {}", project_path.display()))
}

/// Scan all workshop directories and return every parseable wallpaper
#[tracing::instrument(target = "workshop", level = "debug")]
pub fn scan_wallpapers() -> Vec<Wallpaper> {
    let workshop_dirs = find_workshop_dirs();
    tracing::debug!(target: "workshop", dirs = workshop_dirs.len(), "scanning workshop roots");
    let mut wallpapers = Vec::new();

    for workshop_dir in workshop_dirs {
        tracing::trace!(target: "workshop", dir = %workshop_dir.display(), "reading workshop directory");
        let entries = match std::fs::read_dir(&workshop_dir) {
            Ok(e) => e,
            Err(err) => {
                tracing::warn!(target: "workshop", "cannot read {}: {}", workshop_dir.display(), err);
                continue;
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let workshop_id = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            match load_project(&path) {
                Ok(project) => {
                    tracing::trace!(target: "workshop", id = %workshop_id, title = project.title.as_deref().unwrap_or(""), "loaded wallpaper");
                    wallpapers.push(Wallpaper {
                        workshop_id,
                        path,
                        project,
                    });
                }
                Err(err) => {
                    tracing::warn!(target: "workshop", "skipping {}: {}", workshop_id, err);
                }
            }
        }
    }

    // Sort by title for consistent output
    wallpapers.sort_by(|a, b| a.title().cmp(b.title()));
    tracing::debug!(target: "workshop", count = wallpapers.len(), "workshop scan complete");
    wallpapers
}

/// Find a single wallpaper by its Workshop ID
pub fn find_by_id(id: &str) -> Option<Wallpaper> {
    scan_wallpapers().into_iter().find(|w| w.workshop_id == id)
}
