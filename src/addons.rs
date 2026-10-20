//! Download + install community maps/models from kingpin.info.
//!
//! # What this installs
//! Large official community packs hosted on kingpin.info (map-pack-n-yo-ass,
//! Bagman maps, player models). Total size is ~2+ GB — downloads stream to a
//! temp cache with progress events for the UI.
//!
//! # Markers
//! After a successful install we write `main/kingpin-launcher-content.json` so
//! later launches can skip the expensive disk scan. [`is_installed`] also
//! accepts a heuristic scan (enough `.bsp` / player dirs / legacy `pak8.pak`)
//! for users who installed packs manually.
//!
//! Archives may be `.zip` or `.rar`/`.7z` (7-Zip CLI required for the latter).

use std::fs::{self, File};
use std::io::{copy, Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// Browse page for the packs we install.
pub const CONTENT_INDEX_URL: &str =
    "https://www.kingpin.info/download/index.php?dir=kingpin/main/maps/map-packs/";

/// Marker written after a successful install (stock Kingpin does not have this).
pub const CONTENT_MARKER_REL: &str = "main/kingpin-launcher-content.json";
pub const CONTENT_MARKER_VERSION: &str = "kingpin-info-maps-models-v1";

/// Rough total for the progress bar before Content-Length is known (~2.3 GB).
pub const CONTENT_EXPECTED_BYTES: u64 = 2_300_000_000;

struct PackFile {
    dir: &'static str,
    file: &'static str,
    /// Approximate size for progress weighting when Content-Length is missing.
    approx_bytes: u64,
}

/// Community map/model packs from kingpin.info.
const PACKS: &[PackFile] = &[
    // map-pack-n-yo-ass (1999–2007)
    PackFile {
        dir: "kingpin/main/maps/map-packs/map-pack-n-yo-ass/all_maps_1999-2007/",
        file: "bcs_map_pack_n_yo_ass-1.zip",
        approx_bytes: 128_500_000,
    },
    PackFile {
        dir: "kingpin/main/maps/map-packs/map-pack-n-yo-ass/all_maps_1999-2007/",
        file: "bcs_map_pack_n_yo_ass-2.zip",
        approx_bytes: 130_600_000,
    },
    PackFile {
        dir: "kingpin/main/maps/map-packs/map-pack-n-yo-ass/all_maps_1999-2007/",
        file: "bcs_map_pack_n_yo_ass-3.zip",
        approx_bytes: 69_000_000,
    },
    PackFile {
        dir: "kingpin/main/maps/map-packs/map-pack-n-yo-ass/all_maps_1999-2007/",
        file: "bcs_map_pack_n_yo_ass-4.zip",
        approx_bytes: 82_800_000,
    },
    PackFile {
        dir: "kingpin/main/maps/map-packs/map-pack-n-yo-ass/all_maps_1999-2007/",
        file: "bcs_map_pack_n_yo_ass-5.zip",
        approx_bytes: 180_000_000,
    },
    PackFile {
        dir: "kingpin/main/maps/map-packs/map-pack-n-yo-ass/all_maps_1999-2007/",
        file: "bcs_map_pack_n_yo_ass-6.zip",
        approx_bytes: 81_300_000,
    },
    PackFile {
        dir: "kingpin/main/maps/map-packs/map-pack-n-yo-ass/all_maps_1999-2007/",
        file: "bcs_map_pack_n_yo_ass-7.zip",
        approx_bytes: 83_000_000,
    },
    PackFile {
        dir: "kingpin/main/maps/map-packs/map-pack-n-yo-ass/all_maps_1999-2007/",
        file: "bcs_map_pack_n_yo_ass-8.zip",
        approx_bytes: 88_700_000,
    },
    PackFile {
        dir: "kingpin/main/maps/map-packs/map-pack-n-yo-ass/all_maps_1999-2007/",
        file: "bcs_map_pack_n_yo_ass-9.zip",
        approx_bytes: 122_300_000,
    },
    PackFile {
        dir: "kingpin/main/maps/map-packs/map-pack-n-yo-ass/all_maps_1999-2007/",
        file: "bcs_map_pack_n_yo_ass-10.zip",
        approx_bytes: 127_200_000,
    },
    PackFile {
        dir: "kingpin/main/maps/map-packs/map-pack-n-yo-ass/all_maps_1999-2007/",
        file: "bcs_map_pack_n_yo_ass-11.zip",
        approx_bytes: 87_200_000,
    },
    // map-pack-n-yo-ass (DM 2007–2017)
    PackFile {
        dir: "kingpin/main/maps/map-packs/map-pack-n-yo-ass/death_match_maps_2007-2017/",
        file: "bcs_map_pack_n_yo_ass-12.zip",
        approx_bytes: 65_400_000,
    },
    PackFile {
        dir: "kingpin/main/maps/map-packs/map-pack-n-yo-ass/death_match_maps_2007-2017/",
        file: "bcs_map_pack_n_yo_ass-13.zip",
        approx_bytes: 128_800_000,
    },
    // Bagman maps (called out by the map-pack readme)
    PackFile {
        dir: "kingpin/main/maps/map-packs/",
        file: "bagman_mappack.zip",
        approx_bytes: 490_000_000,
    },
    // Player models
    PackFile {
        dir: "kingpin/main/models/player_models/",
        file: "players_model_fix.zip",
        approx_bytes: 530_000_000,
    },
    PackFile {
        dir: "kingpin/main/models/",
        file: "kingpin_modeladdon.zip",
        approx_bytes: 310_000,
    },
];

#[derive(Debug, Clone)]
pub enum AddonEvent {
    Progress {
        phase: String,
        done: u64,
        total: u64,
    },
    Finished(Result<String, String>),
}

/// Heuristic snapshot of what community content is already on disk.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ContentDetected {
    pub maps: bool,
    pub map_files: u32,
    pub player_models: bool,
    pub player_model_dirs: u32,
    /// Old all-in-one `pak8.pak` installs count as complete.
    pub legacy_pak8: bool,
}

/// JSON written to [`CONTENT_MARKER_REL`] after install or successful scan.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ContentMarker {
    pub version: String,
    pub source: String,
    pub packs: usize,
    #[serde(default)]
    pub detected: Option<ContentDetected>,
    #[serde(default)]
    pub scanned_at: Option<String>,
}

impl ContentDetected {
    /// Enough community content to treat maps/models as present.
    pub fn looks_installed(&self) -> bool {
        (self.maps && self.player_models) || self.legacy_pak8
    }
}

/// True if the marker matches our version, a legacy txt marker exists, or the scan looks good.
pub fn is_installed(game_dir: &Path) -> bool {
    if read_marker(game_dir)
        .map(|m| m.version == CONTENT_MARKER_VERSION)
        .unwrap_or(false)
    {
        return true;
    }
    // Legacy .txt marker from an earlier build
    let legacy = game_dir.join("main/kingpin-launcher-content.txt");
    if fs::read_to_string(legacy)
        .map(|t| t.contains(CONTENT_MARKER_VERSION))
        .unwrap_or(false)
    {
        return true;
    }
    // Already on disk from an older pack / manual install
    scan_installed_content(game_dir).looks_installed()
}

pub fn read_marker(game_dir: &Path) -> Option<ContentMarker> {
    let text = fs::read_to_string(game_dir.join(CONTENT_MARKER_REL)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Walk the Kingpin folder and report what community content looks present.
pub fn scan_installed_content(game_dir: &Path) -> ContentDetected {
    let map_files = count_files_with_ext(&game_dir.join("main/maps"), &["bsp", "BSP"]);
    let player_model_dirs = count_subdirs(&game_dir.join("main/players"))
        .max(count_subdirs(&game_dir.join("players")));
    let legacy_pak8 = game_dir.join("main/pak8.pak").is_file();

    ContentDetected {
        maps: map_files >= 50 || legacy_pak8,
        map_files,
        player_models: player_model_dirs >= 15 || legacy_pak8,
        player_model_dirs,
        legacy_pak8,
    }
}

/// Scan disk and write/update `kingpin-launcher-content.json`.
pub fn sync_marker_from_disk(game_dir: &Path) -> Result<ContentMarker, String> {
    let detected = scan_installed_content(game_dir);
    if !detected.looks_installed() {
        return Err(format!(
            "Not enough content found yet (maps: {}, player models: {}, pak8: {}). Install maps & models first.",
            detected.map_files, detected.player_model_dirs, detected.legacy_pak8
        ));
    }
    let marker = ContentMarker {
        version: CONTENT_MARKER_VERSION.into(),
        source: if detected.legacy_pak8 {
            "detected-legacy-or-manual".into()
        } else {
            "detected".into()
        },
        packs: PACKS.len(),
        detected: Some(detected),
        scanned_at: Some(timestamp_now()),
    };
    write_marker_json(game_dir, &marker)?;
    Ok(marker)
}

fn timestamp_now() -> String {
    // Local wall clock without pulling in chrono.
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("unix:{secs}")
}

fn count_files_with_ext(dir: &Path, exts: &[&str]) -> u32 {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    let mut n = 0u32;
    let mut stack: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    while let Some(path) = stack.pop() {
        if path.is_dir() {
            if let Ok(kids) = fs::read_dir(&path) {
                stack.extend(kids.flatten().map(|e| e.path()));
            }
            continue;
        }
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if exts.iter().any(|x| ext.eq_ignore_ascii_case(x)) {
                n = n.saturating_add(1);
            }
        }
    }
    n
}

fn count_subdirs(dir: &Path) -> u32 {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .count() as u32
}

pub fn pack_count() -> usize {
    PACKS.len()
}

pub fn find_7z() -> Option<PathBuf> {
    let candidates = [
        PathBuf::from(r"C:\Program Files\7-Zip\7z.exe"),
        PathBuf::from(r"C:\Program Files (x86)\7-Zip\7z.exe"),
        PathBuf::from(r"C:\7-Zip\7z.exe"),
    ];
    candidates.into_iter().find(|p| p.is_file()).or_else(|| {
        which("7z.exe").or_else(|| which("7z"))
    })
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

pub fn install_async(game_dir: PathBuf, tx: mpsc::Sender<AddonEvent>) {
    thread::spawn(move || {
        let result = install_blocking(&game_dir, &tx);
        let _ = tx.send(AddonEvent::Finished(result));
    });
}

fn download_url(dir: &str, file: &str) -> String {
    format!("https://www.kingpin.info/download/index.php?dir={dir}&file={file}")
}

fn install_blocking(game_dir: &Path, tx: &mpsc::Sender<AddonEvent>) -> Result<String, String> {
    if !game_dir.is_dir() {
        return Err("Game folder does not exist.".into());
    }
    if !game_dir.join("kingpin.exe").is_file() {
        return Err("kingpin.exe not found in game folder.".into());
    }

    let cache_dir = std::env::temp_dir().join("kingpin-launcher").join("content");
    fs::create_dir_all(&cache_dir).map_err(|e| e.to_string())?;

    let total_approx: u64 = PACKS.iter().map(|p| p.approx_bytes).sum();
    let mut bytes_done_before: u64 = 0;
    let n = PACKS.len();

    for (i, pack) in PACKS.iter().enumerate() {
        let dest = cache_dir.join(pack.file);
        let url = download_url(pack.dir, pack.file);
        let label = format!("({}/{}) {}", i + 1, n, pack.file);

        download_file(
            &url,
            &dest,
            pack.approx_bytes,
            bytes_done_before,
            total_approx,
            &label,
            tx,
        )?;

        let _ = tx.send(AddonEvent::Progress {
            phase: format!("Extracting {label}"),
            done: bytes_done_before + pack.approx_bytes / 2,
            total: total_approx,
        });

        extract_archive(&dest, game_dir)?;
        bytes_done_before = bytes_done_before.saturating_add(pack.approx_bytes);

        let _ = tx.send(AddonEvent::Progress {
            phase: format!("Installed {label}"),
            done: bytes_done_before.min(total_approx),
            total: total_approx,
        });
    }

    let detected = scan_installed_content(game_dir);
    let marker = ContentMarker {
        version: CONTENT_MARKER_VERSION.into(),
        source: "kingpin.info".into(),
        packs: n,
        detected: Some(detected),
        scanned_at: Some(timestamp_now()),
    };
    write_marker_json(game_dir, &marker)?;

    if !read_marker(game_dir).is_some_and(|m| m.version == CONTENT_MARKER_VERSION) {
        return Err(
            "Install finished but the content marker is missing. Under Program Files, run as Administrator and try again."
                .into(),
        );
    }

    Ok(format!(
        "Installed {n} kingpin.info packs (maps + models). Mod-specific clients are still under Mods."
    ))
}

fn write_marker_json(game_dir: &Path, marker: &ContentMarker) -> Result<(), String> {
    let path = game_dir.join(CONTENT_MARKER_REL);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            format!(
                "Cannot create {}: {e}. Try running as Administrator.",
                parent.display()
            )
        })?;
    }
    let body = serde_json::to_string_pretty(marker).map_err(|e| e.to_string())?;
    fs::write(&path, body).map_err(|e| {
        format!(
            "Cannot write {}: {e}. Try running as Administrator.",
            path.display()
        )
    })?;
    let _ = fs::remove_file(game_dir.join("main/kingpin-launcher-content.txt"));
    Ok(())
}

fn download_file(
    url: &str,
    dest: &Path,
    approx: u64,
    bytes_before: u64,
    total_approx: u64,
    label: &str,
    tx: &mpsc::Sender<AddonEvent>,
) -> Result<(), String> {
    if dest.is_file() {
        if let Ok(meta) = dest.metadata() {
            // Accept cache if it looks complete (≥ 90% of approx, and at least 64 KB).
            if meta.len() >= 64 * 1024 && meta.len() as f64 >= approx as f64 * 0.9 {
                let _ = tx.send(AddonEvent::Progress {
                    phase: format!("Using cached {label}"),
                    done: (bytes_before + meta.len()).min(total_approx),
                    total: total_approx,
                });
                return Ok(());
            }
        }
    }

    let _ = tx.send(AddonEvent::Progress {
        phase: format!("Downloading {label}"),
        done: bytes_before,
        total: total_approx,
    });

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(30))
        .timeout_read(Duration::from_secs(600))
        .build();

    let response = agent
        .get(url)
        .set("User-Agent", "kingpin-launcher/0.1")
        .call()
        .map_err(|e| format!("Download failed ({label}): {e}"))?;

    let file_total = response
        .header("Content-Length")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(approx);

    let mut reader = response.into_reader();
    let partial = dest.with_extension("partial");
    let mut file = File::create(&partial).map_err(|e| {
        format!("Cannot write download cache ({e}): {}", partial.display())
    })?;

    let mut buf = vec![0u8; 256 * 1024];
    let mut done: u64 = 0;
    let mut last_report = 0u64;

    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("Download read error ({label}): {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])
            .map_err(|e| format!("Download write error ({label}): {e}"))?;
        done += n as u64;
        if done - last_report >= 4 * 1024 * 1024 || done == file_total {
            last_report = done;
            let overall = bytes_before.saturating_add(done).min(total_approx);
            let _ = tx.send(AddonEvent::Progress {
                phase: format!("Downloading {label}"),
                done: overall,
                total: total_approx,
            });
        }
    }

    file.flush().map_err(|e| e.to_string())?;
    drop(file);

    if done < 64 * 1024 {
        let _ = fs::remove_file(&partial);
        return Err(format!(
            "Download too small for {label} — file may be missing on kingpin.info."
        ));
    }

    fs::rename(&partial, dest).map_err(|e| format!("Finalize download failed: {e}"))?;
    Ok(())
}

fn extract_archive(archive: &Path, dest_dir: &Path) -> Result<(), String> {
    let ext = archive
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    if ext == "zip" {
        return extract_zip(archive, dest_dir);
    }

    let seven = find_7z().ok_or_else(|| {
        "7-Zip required for .rar/.7z packs. Install from https://www.7-zip.org/".to_string()
    })?;
    let out_arg = format!("-o{}", dest_dir.display());
    let status = Command::new(seven)
        .args(["x", &archive.to_string_lossy(), &out_arg, "-y", "-bb0"])
        .status()
        .map_err(|e| format!("7-Zip failed: {e}"))?;
    if !status.success() {
        return Err(format!(
            "7-Zip extract failed ({status}). Try running as Administrator under Program Files."
        ));
    }
    Ok(())
}

fn extract_zip(archive: &Path, dest_dir: &Path) -> Result<(), String> {
    let bytes = fs::read(archive).map_err(|e| e.to_string())?;
    let mut zip =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("Invalid zip: {e}"))?;
    for i in 0..zip.len() {
        let mut file = zip.by_index(i).map_err(|e| e.to_string())?;
        let name = file
            .enclosed_name()
            .ok_or_else(|| "Unsafe path in zip".to_string())?
            .to_path_buf();
        let out = dest_dir.join(name);
        if file.name().ends_with('/') {
            fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                format!(
                    "Cannot create {}: {e}. Try running as Administrator.",
                    parent.display()
                )
            })?;
        }
        let mut outfile = File::create(&out).map_err(|e| {
            format!(
                "Cannot write {}: {e}. Try running as Administrator.",
                out.display()
            )
        })?;
        copy(&mut file, &mut outfile).map_err(|e| e.to_string())?;
    }
    Ok(())
}
