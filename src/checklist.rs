//! Install readiness checklist.
//!
//! Scans the configured Kingpin folder for the files we care about and decides
//! whether the install is "READY" (can play with the built-in browser) or needs
//! the Patches wizard. Classic M-Browser is tracked but **optional** — the
//! ASE-style Browser tab replaces it for multiplayer.

use std::path::{Path, PathBuf};

/// Presence of a single expected file on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemStatus {
    /// File exists.
    Ok,
    /// File expected but missing.
    Missing,
    /// No game folder configured yet — cannot evaluate.
    Unknown,
}

/// Snapshot of what is installed under the game directory.
#[derive(Debug, Clone)]
pub struct Checklist {
    pub game_dir: Option<PathBuf>,
    pub kingpin_exe: ItemStatus,
    /// MH Kingpin Patch hook DLL (must sit next to `kingpin.exe`).
    pub mh_dll: ItemStatus,
    /// MH patch configuration utility.
    pub mh_patchcfg: ItemStatus,
    /// Optional classic Hambloch M-Browser.
    pub mbrowser: ItemStatus,
    /// Community maps/models packs (marker or heuristic scan).
    pub kpaddons: ItemStatus,
}

impl Checklist {
    /// Build a fresh checklist for `game_dir` (or all-Unknown if `None`).
    ///
    /// As a side effect, if maps/models look installed but the JSON marker is
    /// missing, we write the marker so later runs stay fast.
    pub fn evaluate(game_dir: Option<&Path>) -> Self {
        let Some(dir) = game_dir else {
            return Self {
                game_dir: None,
                kingpin_exe: ItemStatus::Unknown,
                mh_dll: ItemStatus::Unknown,
                mh_patchcfg: ItemStatus::Unknown,
                mbrowser: ItemStatus::Unknown,
                kpaddons: ItemStatus::Unknown,
            };
        };

        Self {
            game_dir: Some(dir.to_path_buf()),
            kingpin_exe: file_status(&dir.join("kingpin.exe")),
            mh_dll: file_status(&dir.join("wsock32.dll")),
            mh_patchcfg: file_status(&dir.join("patchcfg.exe")),
            mbrowser: file_status(&dir.join("Kingpin_M_Browser.exe")),
            kpaddons: {
                // Heal missing marker when content is already on disk.
                if !crate::addons::read_marker(dir)
                    .is_some_and(|m| m.version == crate::addons::CONTENT_MARKER_VERSION)
                    && crate::addons::scan_installed_content(dir).looks_installed()
                {
                    let _ = crate::addons::sync_marker_from_disk(dir);
                }
                if crate::addons::is_installed(dir) {
                    ItemStatus::Ok
                } else {
                    ItemStatus::Missing
                }
            },
        }
    }

    /// True when SP + built-in multiplayer are usable (MH patch present).
    /// M-Browser is not required.
    pub fn is_ready(&self) -> bool {
        self.kingpin_exe == ItemStatus::Ok
            && self.mh_dll == ItemStatus::Ok
            && self.mh_patchcfg == ItemStatus::Ok
    }

    /// Both MH patch files present.
    pub fn mh_ok(&self) -> bool {
        self.mh_dll == ItemStatus::Ok && self.mh_patchcfg == ItemStatus::Ok
    }

    /// Short status string for the footer strip.
    pub fn status_label(&self) -> &'static str {
        if self.game_dir.is_none() {
            "FIX — set game folder"
        } else if self.is_ready() {
            "READY"
        } else {
            "FIX — open Patches"
        }
    }
}

fn file_status(path: &Path) -> ItemStatus {
    if path.is_file() {
        ItemStatus::Ok
    } else {
        ItemStatus::Missing
    }
}
