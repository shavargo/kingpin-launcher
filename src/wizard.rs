//! Patches wizard — guided "missing → fix" steps for a playable install.
//!
//! Steps advance automatically based on [`crate::checklist::Checklist`]:
//! locate folder → verify `kingpin.exe` → install MH patch → Ready.
//! Classic M-Browser remains an optional side action (download page / file install),
//! not a required wizard step.

use std::fs::{self, File};
use std::io::{copy, Cursor, Read};
use std::path::{Path, PathBuf};

/// Direct zip download for MH Kingpin Patch v12 (wsock32.dll + patchcfg.exe).
pub const MH_PATCH_URL: &str = "https://download.kingpin.info/kingpin/patches/unofficial/windows/mh%27s_kingpin_patch/kingpinpatch_v12.zip";
/// Forum page where users grab classic M-Browser (optional).
pub const MBROWSER_PAGE: &str = "https://forum.hambloch.com/kingpin.php?dl=mbrowser";
/// Info/browse page for the MH patch (opened if auto-download fails).
pub const MH_PATCH_INFO: &str = "https://www.kingpin.info/download/index.php?dir=kingpin%2Fpatches%2Funofficial%2Fwindows%2Fmh%27s_kingpin_patch%2F";

/// Current wizard screen driven by checklist state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WizardStep {
    LocateGame,
    VerifyGame,
    InstallMhPatch,
    Ready,
}

impl WizardStep {
    pub fn title(self) -> &'static str {
        match self {
            Self::LocateGame => "1 · Locate Kingpin",
            Self::VerifyGame => "2 · Verify game (v1.21)",
            Self::InstallMhPatch => "3 · Install MH Kingpin Patch",
            Self::Ready => "4 · Ready to play",
        }
    }

    pub fn detail(self) -> &'static str {
        match self {
            Self::LocateGame => {
                "Point at the folder that contains kingpin.exe (Steam or GOG)."
            }
            Self::VerifyGame => {
                "Steam and GOG both ship official 1.21. We only check that kingpin.exe is present."
            }
            Self::InstallMhPatch => {
                "Downloads MH patch v12 (wsock32.dll + patchcfg.exe) into the game folder."
            }
            Self::Ready => {
                "Core setup ready. Open the Browser tab for the live server list, or Play for single-player."
            }
        }
    }
}

/// Pick the wizard step that matches what is still missing.
pub fn current_step(checklist: &crate::checklist::Checklist) -> WizardStep {
    use crate::checklist::ItemStatus;

    if checklist.game_dir.is_none() {
        return WizardStep::LocateGame;
    }
    if checklist.kingpin_exe != ItemStatus::Ok {
        return WizardStep::VerifyGame;
    }
    if !checklist.mh_ok() {
        return WizardStep::InstallMhPatch;
    }
    // M-Browser is optional — Ready after MH patch.
    WizardStep::Ready
}

/// Download MH patch zip and extract into the game folder (needs write access).
pub fn install_mh_patch(game_dir: &Path) -> Result<String, String> {
    if !game_dir.is_dir() {
        return Err("Game folder is not a directory.".into());
    }

    let bytes = download(MH_PATCH_URL)?;
    extract_zip_to(&bytes, game_dir)?;

    let dll = game_dir.join("wsock32.dll");
    let cfg = game_dir.join("patchcfg.exe");
    if !dll.is_file() || !cfg.is_file() {
        return Err(
            "Zip extracted but wsock32.dll / patchcfg.exe still missing. Check write permissions (Run as admin under Program Files).".into(),
        );
    }

    Ok("MH Kingpin Patch v12 installed.".into())
}

pub fn open_mbrowser_download() -> Result<(), String> {
    open::that(MBROWSER_PAGE).map_err(|e| format!("Could not open M-Browser download: {e}"))
}

pub fn open_mh_patch_page() -> Result<(), String> {
    open::that(MH_PATCH_INFO).map_err(|e| format!("Could not open MH patch page: {e}"))
}

/// Try to place a user-selected M-Browser zip or exe into the game dir.
pub fn install_mbrowser_from_path(game_dir: &Path, source: &Path) -> Result<String, String> {
    if !game_dir.is_dir() {
        return Err("Game folder is not a directory.".into());
    }
    if !source.exists() {
        return Err("Selected file does not exist.".into());
    }

    let ext = source
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    if ext == "zip" {
        let bytes = fs::read(source).map_err(|e| e.to_string())?;
        extract_zip_to(&bytes, game_dir)?;
    } else if ext == "exe" {
        let dest = game_dir.join("Kingpin_M_Browser.exe");
        fs::copy(source, &dest).map_err(|e| {
            format!(
                "Copy failed ({e}). Under Program Files you may need to run the launcher as admin."
            )
        })?;
    } else {
        return Err("Pick a .zip or Kingpin_M_Browser.exe.".into());
    }

    let target = game_dir.join("Kingpin_M_Browser.exe");
    if !target.is_file() {
        return Err(
            "Install finished but Kingpin_M_Browser.exe not found in the game folder.".into(),
        );
    }

    Ok("M-Browser installed.".into())
}

/// Blocking HTTP GET of an entire file into memory (MH zip is small).
fn download(url: &str) -> Result<Vec<u8>, String> {
    let response = ureq::get(url)
        .set("User-Agent", "kingpin-launcher/0.1")
        .call()
        .map_err(|e| format!("Download failed: {e}"))?;

    let mut bytes = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Read download failed: {e}"))?;

    if bytes.len() < 100 {
        return Err("Download looked empty — check your network or try the website link.".into());
    }
    Ok(bytes)
}

/// Extract a zip into `dest_dir`, rejecting unsafe paths (`..` / absolute).
fn extract_zip_to(bytes: &[u8], dest_dir: &Path) -> Result<(), String> {
    let reader = Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(reader).map_err(|e| format!("Invalid zip: {e}"))?;

    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| format!("Zip entry error: {e}"))?;
        let name = file
            .enclosed_name()
            .ok_or_else(|| "Unsafe path in zip".to_string())?
            .to_path_buf();

        // Flatten one top-level folder if present; keep simple files at root.
        let out_path = flatten_to_game_dir(dest_dir, &name);

        if file.name().ends_with('/') {
            fs::create_dir_all(&out_path).map_err(|e| e.to_string())?;
            continue;
        }

        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                format!(
                    "Cannot create {}: {e}. Try running as administrator.",
                    parent.display()
                )
            })?;
        }

        let mut outfile = File::create(&out_path).map_err(|e| {
            format!(
                "Cannot write {}: {e}. Try running as administrator.",
                out_path.display()
            )
        })?;
        copy(&mut file, &mut outfile).map_err(|e| e.to_string())?;
    }

    Ok(())
}

/// Map a zip entry path onto the game dir, stripping a redundant top folder when useful.
fn flatten_to_game_dir(dest_dir: &Path, name: &Path) -> PathBuf {
    // If zip has a single root folder, drop it for known patch files at depth 1+.
    let components: Vec<_> = name.components().collect();
    if components.len() > 1 {
        let first = components[0].as_os_str().to_string_lossy().to_ascii_lowercase();
        if first.contains("kingpin") || first.contains("patch") || first == "main" {
            // keep nested main/ etc., but for patch root folders with flat files strip one level
            if components.len() == 2 {
                return dest_dir.join(components[1].as_os_str());
            }
        }
    }
    dest_dir.join(name)
}
