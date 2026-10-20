//! Persistent launcher settings.
//!
//! Stored as `launcher.toml` **next to the executable** (not in AppData) so a
//! portable copy of the `.exe` keeps its game path and last tab when moved.
//! Missing or corrupt files fall back to defaults without failing startup.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// User preferences written between runs.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LauncherConfig {
    /// Absolute path to the Kingpin install folder (`kingpin.exe` parent).
    pub game_path: Option<String>,
    /// Last selected sidebar tab (`play`, `browser`, `mods`, …).
    pub last_tab: Option<String>,
}

impl LauncherConfig {
    /// Path of the TOML file beside the running exe (or `./launcher.toml` as fallback).
    pub fn config_path() -> PathBuf {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("launcher.toml")))
            .unwrap_or_else(|| PathBuf::from("launcher.toml"))
    }

    /// Load from the default path; returns [`Default`] if absent or invalid.
    pub fn load() -> Self {
        let path = Self::config_path();
        Self::load_from(&path)
    }

    /// Load from an explicit path (used by tests / tooling).
    pub fn load_from(path: &Path) -> Self {
        let Ok(raw) = fs::read_to_string(path) else {
            return Self::default();
        };
        toml::from_str(&raw).unwrap_or_default()
    }

    /// Pretty-print TOML to disk, creating the parent directory if needed.
    pub fn save(&self) -> Result<(), String> {
        let path = Self::config_path();
        let raw = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        fs::write(&path, raw).map_err(|e| e.to_string())
    }

    /// Parsed [`PathBuf`] for the configured game folder, if any.
    pub fn game_dir(&self) -> Option<PathBuf> {
        self.game_path.as_ref().map(PathBuf::from)
    }

    /// Remember a newly chosen game folder.
    pub fn set_game_dir(&mut self, dir: PathBuf) {
        self.game_path = Some(dir.to_string_lossy().to_string());
    }
}
