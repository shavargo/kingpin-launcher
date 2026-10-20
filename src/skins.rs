//! Player model / skin discovery and TGA previews for the Player tab.
//!
//! Kingpin stores custom models under `<game>/players/<model>/`. Stock thug /
//! runt / bitch skins often live under `models/actors/...` via `skinfolder.txt`.
//! The `skin` cvar is `"model/head body legs"` (three independent skin IDs).

use egui::{Color32, ColorImage};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// One resolved skin string ready for `set skin "..."`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkinSelection {
    pub model: String,
    pub head: String,
    pub body: String,
    pub legs: String,
}

impl SkinSelection {
    pub fn to_cvar(&self) -> String {
        format!("{}/{} {} {}", self.model, self.head, self.body, self.legs)
    }

    pub fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim().trim_matches('"');
        let (model, rest) = raw.split_once('/')?;
        let parts: Vec<&str> = rest.split_whitespace().collect();
        if model.is_empty() || parts.len() != 3 {
            return None;
        }
        Some(Self {
            model: model.to_string(),
            head: parts[0].to_string(),
            body: parts[1].to_string(),
            legs: parts[2].to_string(),
        })
    }
}

/// Folders that hold head/body/legs TGA skins for a model.
#[derive(Debug, Clone)]
pub struct SkinDirs {
    /// `players/<model>` directory (model meshes / extras live here).
    #[allow(dead_code)]
    pub player_dir: PathBuf,
    pub head: PathBuf,
    pub body: PathBuf,
    pub legs: PathBuf,
}

/// List `players/*` dirs that look like Kingpin player models.
pub fn list_models(game_dir: &Path, game: &str) -> Vec<String> {
    let mut names = BTreeSet::new();
    for root in player_search_roots(game_dir, game) {
        let Ok(entries) = fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if !ft.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let path = entry.path();
            if path.join("head.mdx").is_file()
                || path.join("body.mdx").is_file()
                || path.join("legs.mdx").is_file()
            {
                names.insert(name);
            }
        }
    }
    names.into_iter().collect()
}

/// Resolve where TGA skins live for `model` (player folder + optional skinfolder).
pub fn resolve_dirs(game_dir: &Path, game: &str, model: &str) -> Option<SkinDirs> {
    let player_dir = find_player_dir(game_dir, game, model)?;
    let mut head = player_dir.clone();
    let mut body = player_dir.clone();
    let mut legs = player_dir.clone();

    if let Ok(text) = fs::read_to_string(player_dir.join("skinfolder.txt")) {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
                continue;
            }
            let Some((key, rel)) = line.split_once(':') else {
                continue;
            };
            let rel = rel.trim().replace('\\', "/");
            if rel.is_empty() {
                continue;
            }
            let abs = resolve_rel_dir(game_dir, game, &rel);
            match key.trim().to_ascii_lowercase().as_str() {
                "head" => head = abs,
                "body" => body = abs,
                "legs" => legs = abs,
                _ => {}
            }
        }
    }

    Some(SkinDirs {
        player_dir,
        head,
        body,
        legs,
    })
}

/// Skin IDs available for one body part (`head`, `body`, or `legs`).
pub fn list_skin_ids(dirs: &SkinDirs, part: &str) -> Vec<String> {
    let dir = match part {
        "head" => &dirs.head,
        "body" => &dirs.body,
        "legs" => &dirs.legs,
        _ => return Vec::new(),
    };
    let mut ids = BTreeSet::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let prefix = format!("{part}_");
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let lower = name.to_ascii_lowercase();
        if !lower.ends_with(".tga") {
            continue;
        }
        let stem = &lower[..lower.len() - 4];
        let Some(id) = stem.strip_prefix(&prefix) else {
            continue;
        };
        if id.is_empty() {
            continue;
        }
        // Pain / damage variants — not selectable in Player Setup.
        if id.ends_with("p1") || id.ends_with("p2") {
            continue;
        }
        // Keep original casing from filename after prefix.
        let raw_stem = &name[..name.len() - 4];
        let id_raw = &raw_stem[prefix.len()..];
        ids.insert(id_raw.to_string());
    }
    ids.into_iter().collect()
}

/// Absolute path to the TGA for a part + skin id, if it exists.
pub fn texture_path(dirs: &SkinDirs, part: &str, id: &str) -> Option<PathBuf> {
    let dir = match part {
        "head" => &dirs.head,
        "body" => &dirs.body,
        "legs" => &dirs.legs,
        _ => return None,
    };
    for ext in ["tga", "TGA"] {
        let path = dir.join(format!("{part}_{id}.{ext}"));
        if path.is_file() {
            return Some(path);
        }
    }
    // Case-insensitive fallback scan (helps odd packs / mixed casing).
    let Ok(entries) = fs::read_dir(dir) else {
        return None;
    };
    let want = format!("{part}_{id}");
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let lower = name.to_ascii_lowercase();
        if !lower.ends_with(".tga") {
            continue;
        }
        let stem = &name[..name.len() - 4];
        if stem.eq_ignore_ascii_case(&want) {
            let path = entry.path();
            if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

/// Decode a Kingpin skin TGA/PCX into an egui [`ColorImage`] (downscaled for UI).
pub fn load_preview_image(path: &Path, max_side: u32) -> Result<ColorImage, String> {
    let img = image::open(path).map_err(|e| format!("Open {}: {e}", path.display()))?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    if w == 0 || h == 0 {
        return Err("empty image".into());
    }

    let scale = (max_side as f32 / w.max(h) as f32).min(1.0);
    let (tw, th) = if scale < 1.0 {
        (
            ((w as f32) * scale).round().max(1.0) as u32,
            ((h as f32) * scale).round().max(1.0) as u32,
        )
    } else {
        (w, h)
    };

    let resized = if tw != w || th != h {
        image::imageops::resize(&rgba, tw, th, image::imageops::FilterType::Triangle)
    } else {
        rgba
    };

    let pixels: Vec<Color32> = resized
        .pixels()
        .map(|p| Color32::from_rgba_unmultiplied(p[0], p[1], p[2], p[3]))
        .collect();

    Ok(ColorImage {
        size: [tw as usize, th as usize],
        pixels,
        ..Default::default()
    })
}

/// Pick a sensible default selection for a model (first shared id, or first each).
pub fn default_selection(game_dir: &Path, game: &str, model: &str) -> Option<SkinSelection> {
    let dirs = resolve_dirs(game_dir, game, model)?;
    let heads = list_skin_ids(&dirs, "head");
    let bodies = list_skin_ids(&dirs, "body");
    let legs = list_skin_ids(&dirs, "legs");
    if heads.is_empty() && bodies.is_empty() && legs.is_empty() {
        return Some(SkinSelection {
            model: model.into(),
            head: "001".into(),
            body: "001".into(),
            legs: "001".into(),
        });
    }
    // Prefer a matching triple if one exists.
    for id in &heads {
        if bodies.iter().any(|b| b == id) && legs.iter().any(|l| l == id) {
            return Some(SkinSelection {
                model: model.into(),
                head: id.clone(),
                body: id.clone(),
                legs: id.clone(),
            });
        }
    }
    Some(SkinSelection {
        model: model.into(),
        head: heads.first().cloned().unwrap_or_else(|| "001".into()),
        body: bodies.first().cloned().unwrap_or_else(|| "001".into()),
        legs: legs.first().cloned().unwrap_or_else(|| "001".into()),
    })
}

fn player_search_roots(game_dir: &Path, game: &str) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let mod_players = game_dir.join(game).join("players");
    if mod_players.is_dir() {
        roots.push(mod_players);
    }
    if !game.eq_ignore_ascii_case("main") {
        let main_players = game_dir.join("main").join("players");
        if main_players.is_dir() {
            roots.push(main_players);
        }
    }
    roots
}

fn find_player_dir(game_dir: &Path, game: &str, model: &str) -> Option<PathBuf> {
    for root in player_search_roots(game_dir, game) {
        let path = root.join(model);
        if path.is_dir() {
            return Some(path);
        }
        // Case-insensitive match
        let Ok(entries) = fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.eq_ignore_ascii_case(model) && entry.path().is_dir() {
                return Some(entry.path());
            }
        }
    }
    None
}

fn resolve_rel_dir(game_dir: &Path, game: &str, rel: &str) -> PathBuf {
    let rel = rel.trim_start_matches('/');
    let in_mod = game_dir.join(game).join(rel);
    if in_mod.is_dir() {
        return in_mod;
    }
    let in_main = game_dir.join("main").join(rel);
    if in_main.is_dir() {
        return in_main;
    }
    // Prefer mod path even if missing (caller handles empty listings).
    in_mod
}
