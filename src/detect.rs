//! Locate a Kingpin install on disk.
//!
//! Resolution order (first hit wins):
//! 1. Path saved in `launcher.toml` (if it still contains `kingpin.exe`)
//! 2. Steam libraries via `libraryfolders.vdf`
//! 3. Common GOG / Galaxy folders (+ per-user `GOG Games`)
//! 4. Hard-coded default Steam/GOG paths as a last resort
//!
//! Steam VDF parsing is intentionally naive (string scan for `"path"`) — we
//! only need library roots, not a full Valve KeyValues parser.

use std::fs;
use std::path::{Path, PathBuf};

const STEAM_VDF_CANDIDATES: &[&str] = &[
    r"C:\Program Files (x86)\Steam\steamapps\libraryfolders.vdf",
    r"C:\Program Files\Steam\steamapps\libraryfolders.vdf",
];

const DEFAULT_STEAM_KINGPIN: &str =
    r"C:\Program Files (x86)\Steam\steamapps\common\Kingpin";

/// Common GOG / Galaxy install locations (GOG also ships v1.21).
const GOG_KINGPIN_CANDIDATES: &[&str] = &[
    r"C:\GOG Games\Kingpin - Life of Crime",
    r"C:\GOG Games\Kingpin",
    r"C:\Program Files (x86)\GOG Galaxy\Games\Kingpin - Life of Crime",
    r"C:\Program Files (x86)\GOG Galaxy\Games\Kingpin",
    r"C:\Program Files\GOG Galaxy\Games\Kingpin - Life of Crime",
    r"C:\Program Files\GOG Galaxy\Games\Kingpin",
];

/// True if `dir` looks like a Kingpin root (`kingpin.exe` present).
pub fn is_kingpin_dir(dir: &Path) -> bool {
    dir.join("kingpin.exe").is_file()
}

/// Resolve a usable Kingpin directory: saved → Steam → GOG → known defaults.
pub fn resolve_game_dir(saved: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = saved {
        if is_kingpin_dir(path) {
            return Some(path.to_path_buf());
        }
    }

    if let Some(found) = find_steam_kingpin() {
        return Some(found);
    }

    if let Some(found) = find_gog_kingpin() {
        return Some(found);
    }

    for fallback in [DEFAULT_STEAM_KINGPIN]
        .into_iter()
        .chain(GOG_KINGPIN_CANDIDATES.iter().copied())
    {
        let path = PathBuf::from(fallback);
        if is_kingpin_dir(&path) {
            return Some(path);
        }
    }

    None
}

/// Probe well-known GOG install folders (machine-wide and under `%USERPROFILE%`).
pub fn find_gog_kingpin() -> Option<PathBuf> {
    for candidate in GOG_KINGPIN_CANDIDATES {
        let path = PathBuf::from(candidate);
        if is_kingpin_dir(&path) {
            return Some(path);
        }
    }

    // Per-user default: %USERPROFILE%\GOG Games\...
    if let Some(home) = std::env::var_os("USERPROFILE") {
        for name in ["Kingpin - Life of Crime", "Kingpin"] {
            let path = PathBuf::from(&home).join("GOG Games").join(name);
            if is_kingpin_dir(&path) {
                return Some(path);
            }
        }
    }

    None
}

/// Walk Steam library folders from `libraryfolders.vdf` looking for Kingpin.
pub fn find_steam_kingpin() -> Option<PathBuf> {
    for vdf in STEAM_VDF_CANDIDATES {
        let vdf_path = Path::new(vdf);
        if !vdf_path.is_file() {
            continue;
        }
        let Ok(text) = fs::read_to_string(vdf_path) else {
            continue;
        };
        for library_root in parse_library_paths(&text) {
            let candidate = library_root.join("steamapps").join("common").join("Kingpin");
            if is_kingpin_dir(&candidate) {
                return Some(candidate);
            }
            // Some VDFs already point at the steamapps parent
            let alt = library_root.join("common").join("Kingpin");
            if is_kingpin_dir(&alt) {
                return Some(alt);
            }
        }
    }
    None
}

/// Pull `"path"` values from libraryfolders.vdf (simple scan, not a full VDF parser).
fn parse_library_paths(vdf: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for line in vdf.lines() {
        let trimmed = line.trim();
        // "path"		"D:\\SteamLibrary"
        if !trimmed.to_ascii_lowercase().contains("\"path\"") {
            continue;
        }
        let parts: Vec<&str> = trimmed.split('"').collect();
        // ["", "path", "\t\t", "D:\\SteamLibrary", ""]
        if parts.len() >= 4 {
            let raw = parts[3].replace("\\\\", "\\");
            if !raw.is_empty() {
                paths.push(PathBuf::from(raw));
            }
        }
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_library_paths() {
        let sample = r#"
"libraryfolders"
{
	"0"
	{
		"path"		"C:\\Program Files (x86)\\Steam"
		"label"		""
	}
	"1"
	{
		"path"		"D:\\Games\\SteamLibrary"
	}
}
"#;
        let paths = parse_library_paths(sample);
        assert_eq!(paths.len(), 2);
        assert!(paths[0].to_string_lossy().contains("Steam"));
        assert!(paths[1].to_string_lossy().contains("SteamLibrary"));
    }
}
