//! Browse + install **client** packs from kingpin.info `server/mods`.
//!
//! Many multiplayer mods ship server binaries separately from the files a
//! client needs (maps, models, `pak`s). This module:
//! 1. Scrapes the mods directory listing for folder names
//! 2. Probes each mod page for `client_files` / Windows archives
//! 3. Marks which mods already exist as folders under the game dir
//! 4. Downloads + extracts client archives on demand
//!
//! Server-only mods (no client pack) are listed but not installable here.

use std::fs::{self, File};
use std::io::{copy, Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

pub const MODS_INDEX_URL: &str =
    "https://www.kingpin.info/download/index.php?dir=kingpin/server/mods/";
pub const MODS_PAGE: &str =
    "https://www.kingpin.info/download/index.php?dir=kingpin%2Fserver%2Fmods%2F";

const SKIP_MODS: &[&str] = &["_broken", "_old", "_oldsource", "_sourceonly"];

/// One row in the Mods catalog (scraped from kingpin.info).
#[derive(Debug, Clone)]
pub struct RemoteMod {
    /// Folder name under `server/mods/` (also the local game/mod dir name).
    pub name: String,
    pub has_client_files: bool,
    pub has_windows_full: bool,
    /// Archive filenames found on the mod page (downloaded on Install).
    pub client_archives: Vec<String>,
    /// True if `game_dir/<name>` already exists locally.
    pub installed: bool,
}

impl RemoteMod {
    pub fn needs_client(&self) -> bool {
        self.has_client_files || self.has_windows_full
    }

    pub fn status_label(&self) -> &'static str {
        if self.installed {
            "Installed"
        } else if self.needs_client() {
            "Client files available"
        } else {
            "Server-only / no client pack"
        }
    }
}

#[derive(Debug, Clone)]
pub enum ModsEvent {
    Progress { phase: String, done: u64, total: u64 },
    Catalog(Result<Vec<RemoteMod>, String>),
    InstallFinished { mod_name: String, result: Result<String, String> },
}

pub fn fetch_catalog_async(game_dir: Option<PathBuf>, tx: mpsc::Sender<ModsEvent>) {
    thread::spawn(move || {
        let result = fetch_catalog(game_dir.as_deref());
        let _ = tx.send(ModsEvent::Catalog(result));
    });
}

pub fn install_mod_async(game_dir: PathBuf, remote: RemoteMod, tx: mpsc::Sender<ModsEvent>) {
    thread::spawn(move || {
        let name = remote.name.clone();
        let result = install_mod_client(&game_dir, &remote, &tx);
        let _ = tx.send(ModsEvent::InstallFinished {
            mod_name: name,
            result,
        });
    });
}

pub fn fetch_catalog(game_dir: Option<&Path>) -> Result<Vec<RemoteMod>, String> {
    let html = http_get_text(MODS_INDEX_URL)?;
    let mut names = parse_subdir_names(&html, "kingpin/server/mods/");
    names.retain(|n| !SKIP_MODS.contains(&n.as_str()));
    names.sort();

    // Probe mod pages in parallel (directory listings are small).
    let (tx, rx) = mpsc::channel();
    thread::scope(|scope| {
        for name in names {
            let tx = tx.clone();
            let game_dir = game_dir.map(|p| p.to_path_buf());
            scope.spawn(move || {
                let result = summarize_mod(&name).map(|summary| {
                    let installed = game_dir
                        .as_deref()
                        .is_some_and(|dir| is_mod_installed(dir, &name));
                    RemoteMod {
                        name,
                        has_client_files: summary.has_client_files,
                        has_windows_full: summary.has_windows_full,
                        client_archives: summary.archives,
                        installed,
                    }
                });
                let _ = tx.send(result);
            });
        }
        drop(tx);
    });

    let mut out = Vec::new();
    let mut errors = Vec::new();
    for item in rx {
        match item {
            Ok(m) => out.push(m),
            Err(e) => errors.push(e),
        }
    }

    if out.is_empty() {
        return Err(if errors.is_empty() {
            "No mods found.".into()
        } else {
            errors.join("; ")
        });
    }

    out.sort_by(|a, b| {
        b.needs_client()
            .cmp(&a.needs_client())
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(out)
}

struct ModSummary {
    has_client_files: bool,
    has_windows_full: bool,
    archives: Vec<String>,
}

fn summarize_mod(name: &str) -> Result<ModSummary, String> {
    let url = format!(
        "https://www.kingpin.info/download/index.php?dir=kingpin/server/mods/{name}/"
    );
    let html = http_get_text(&url)?;
    let subs = parse_subdir_names(&html, &format!("kingpin/server/mods/{name}/"));
    let has_client_files = subs.iter().any(|s| s.eq_ignore_ascii_case("client_files"));
    let has_windows_full = subs
        .iter()
        .any(|s| s.eq_ignore_ascii_case("windows_full_install"));

    let mut archives = Vec::new();
    if has_client_files {
        archives.extend(list_archives(name, "client_files")?);
    }
    if archives.is_empty() && has_windows_full {
        archives.extend(list_archives(name, "windows_full_install")?);
    }

    Ok(ModSummary {
        has_client_files,
        has_windows_full,
        archives,
    })
}

fn list_archives(mod_name: &str, folder: &str) -> Result<Vec<String>, String> {
    let url = format!(
        "https://www.kingpin.info/download/index.php?dir=kingpin/server/mods/{mod_name}/{folder}/"
    );
    let html = http_get_text(&url)?;
    Ok(parse_file_names(&html)
        .into_iter()
        .filter(|f| is_client_pack_file(f))
        .collect())
}

fn is_client_pack_file(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".zip")
        || lower.ends_with(".rar")
        || lower.ends_with(".7z")
        || lower.ends_with(".exe")
}

pub fn is_mod_installed(game_dir: &Path, mod_name: &str) -> bool {
    let candidates = [
        mod_name.to_string(),
        mod_name.replace('-', "_"),
        mod_name.replace('_', "-"),
        mod_name.trim_end_matches("_mod").to_string(),
        format!("{mod_name}_mod"),
    ];
    for name in candidates {
        if name.is_empty() {
            continue;
        }
        let path = game_dir.join(&name);
        if path.is_dir() {
            // Ignore empty placeholder dirs
            if fs::read_dir(&path).ok().and_then(|mut d| d.next()).is_some() {
                return true;
            }
        }
    }
    false
}

fn install_mod_client(
    game_dir: &Path,
    remote: &RemoteMod,
    tx: &mpsc::Sender<ModsEvent>,
) -> Result<String, String> {
    if !game_dir.join("kingpin.exe").is_file() {
        return Err("kingpin.exe not found — set the game folder in Patches.".into());
    }
    if remote.client_archives.is_empty() {
        return Err("No client archive listed for this mod.".into());
    }

    let folder = if remote.has_client_files {
        "client_files"
    } else {
        "windows_full_install"
    };

    let cache = std::env::temp_dir()
        .join("kingpin-launcher")
        .join("mods")
        .join(&remote.name);
    fs::create_dir_all(&cache).map_err(|e| e.to_string())?;

    let total = remote.client_archives.len() as u64;
    let mut launched_installer = false;
    for (i, file) in remote.client_archives.iter().enumerate() {
        let _ = tx.send(ModsEvent::Progress {
            phase: format!("Downloading {} ({}/{})", file, i + 1, total),
            done: i as u64,
            total,
        });

        let url = download_url(&remote.name, folder, file);
        let dest = cache.join(file);
        download_to_file(&url, &dest, tx)?;

        let _ = tx.send(ModsEvent::Progress {
            phase: format!("Installing {file}"),
            done: i as u64,
            total,
        });
        match extract_archive(&dest, game_dir)? {
            PackInstall::Extracted => {}
            PackInstall::InstallerLaunched => launched_installer = true,
        }
    }

    if launched_installer {
        return Ok(format!(
            "Launched {} installer — finish setup (target {}), then Refresh list.",
            remote.name,
            game_dir.display()
        ));
    }

    if !is_mod_installed(game_dir, &remote.name) {
        // Some zips only drop into main/ — still OK if extract succeeded.
        return Ok(format!(
            "{} client files extracted into the Kingpin folder.",
            remote.name
        ));
    }

    Ok(format!("{} client files installed.", remote.name))
}

enum PackInstall {
    Extracted,
    InstallerLaunched,
}

fn download_url(mod_name: &str, folder: &str, file: &str) -> String {
    format!(
        "https://www.kingpin.info/download/index.php?dir=kingpin/server/mods/{mod_name}/{folder}/&file={file}"
    )
}

fn download_to_file(url: &str, dest: &Path, tx: &mpsc::Sender<ModsEvent>) -> Result<(), String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(30))
        .timeout_read(Duration::from_secs(300))
        .build();
    let response = agent
        .get(url)
        .set("User-Agent", "kingpin-launcher/0.1")
        .call()
        .map_err(|e| format!("Download failed: {e}"))?;

    let total = response
        .header("Content-Length")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    let mut reader = response.into_reader();
    let partial = dest.with_extension("partial");
    let mut file = File::create(&partial).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut done = 0u64;
    let mut last = 0u64;
    loop {
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        done += n as u64;
        if total > 0 && done - last >= 512 * 1024 {
            last = done;
            let _ = tx.send(ModsEvent::Progress {
                phase: format!("Downloading… {}", dest.file_name().unwrap().to_string_lossy()),
                done,
                total,
            });
        }
    }
    file.flush().map_err(|e| e.to_string())?;
    drop(file);
    if done < 64 {
        let _ = fs::remove_file(&partial);
        return Err("Download too small — file may be missing.".into());
    }
    fs::rename(&partial, dest).map_err(|e| e.to_string())?;
    Ok(())
}

fn extract_archive(archive: &Path, dest_dir: &Path) -> Result<PackInstall, String> {
    let ext = archive
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    if ext == "zip" {
        extract_zip(archive, dest_dir)?;
        return Ok(PackInstall::Extracted);
    }

    if ext == "exe" {
        return install_exe_pack(archive, dest_dir);
    }

    // rar / 7z via 7-Zip
    let seven = crate::addons::find_7z().ok_or_else(|| {
        "7-Zip required to extract .rar/.7z client packs. Install from https://www.7-zip.org/"
            .to_string()
    })?;
    extract_with_7z(&seven, archive, dest_dir)?;
    Ok(PackInstall::Extracted)
}

fn install_exe_pack(archive: &Path, dest_dir: &Path) -> Result<PackInstall, String> {
    // Many Kingpin "client" EXEs are self-extracting archives 7-Zip can unpack silently.
    if let Some(seven) = crate::addons::find_7z() {
        if extract_with_7z(&seven, archive, dest_dir).is_ok() {
            return Ok(PackInstall::Extracted);
        }
    }

    // Otherwise launch the installer and let the user finish the wizard.
    Command::new(archive)
        .current_dir(dest_dir)
        .spawn()
        .map_err(|e| format!("Could not launch {}: {e}", archive.display()))?;
    Ok(PackInstall::InstallerLaunched)
}

fn extract_with_7z(seven: &Path, archive: &Path, dest_dir: &Path) -> Result<(), String> {
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
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("Invalid zip: {e}"))?;
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

fn http_get_text(url: &str) -> Result<String, String> {
    ureq::get(url)
        .set("User-Agent", "kingpin-launcher/0.1")
        .call()
        .map_err(|e| format!("HTTP failed ({url}): {e}"))?
        .into_string()
        .map_err(|e| format!("Read failed: {e}"))
}

fn parse_subdir_names(html: &str, prefix: &str) -> Vec<String> {
    // Links like: dir=kingpin/server/mods/hitmen/  or URL-encoded
    let mut names = Vec::new();
    let encoded_prefix = prefix.replace('/', "%2F");
    for raw in html.split("dir=") {
        let chunk = raw.split(['"', '&', ' ', '>']).next().unwrap_or("");
        let decoded = percent_decode(chunk);
        let path = if decoded.starts_with(prefix) {
            decoded
        } else if chunk.to_ascii_lowercase().contains(&encoded_prefix.to_ascii_lowercase()) {
            percent_decode(chunk)
        } else {
            continue;
        };
        let rest = path.trim_start_matches(prefix).trim_matches('/');
        if rest.is_empty() || rest.contains('/') {
            continue;
        }
        if !names.iter().any(|n| n == rest) {
            names.push(rest.to_string());
        }
    }
    names
}

fn parse_file_names(html: &str) -> Vec<String> {
    let mut names = Vec::new();
    for part in html.split("&file=").chain(html.split("?file=").skip(1)) {
        let name = part.split(['"', '&', ' ', '>', '?']).next().unwrap_or("");
        let name = percent_decode(name);
        if name.is_empty() || name.contains("..") || !name.contains('.') {
            continue;
        }
        if !names.iter().any(|n| n == &name) {
            names.push(name);
        }
    }
    // Fallback: encoded links still use file=
    if names.is_empty() {
        for part in html.split("file=") {
            let name = part.split(['"', '&', ' ', '>', '?']).next().unwrap_or("");
            let name = percent_decode(name);
            let lower = name.to_ascii_lowercase();
            if !(is_client_pack_file(&name) || lower.ends_with(".txt")) {
                continue;
            }
            if !names.iter().any(|n| n == &name) {
                names.push(name);
            }
        }
    }
    names
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let h = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(hex) = h {
                if let Ok(v) = u8::from_str_radix(hex, 16) {
                    out.push(v);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mod_names_from_index_snippet() {
        let html = r#"
        <a href="/download/index.php?dir=kingpin/server/mods/hitmen/">hitmen</a>
        <a href="/download/index.php?dir=kingpin/server/mods/gunrace/">gunrace</a>
        <a href="/download/index.php?dir=kingpin/server/mods/_old/">_old</a>
        "#;
        let names = parse_subdir_names(html, "kingpin/server/mods/");
        assert!(names.contains(&"hitmen".into()));
        assert!(names.contains(&"gunrace".into()));
    }

    #[test]
    fn parses_file_param() {
        let html = r#"<a href="/download/index.php?dir=kingpin/server/mods/hitmen/client_files/&file=hitmenclient.zip">"#;
        let files = parse_file_names(html);
        assert_eq!(files, vec!["hitmenclient.zip"]);
    }

    #[test]
    fn accepts_exe_client_packs() {
        assert!(is_client_pack_file("Blood_Money_4.0.6_Client_Files.exe"));
        assert!(is_client_pack_file("pack.ZIP"));
        assert!(!is_client_pack_file("readme.txt"));
    }
}
