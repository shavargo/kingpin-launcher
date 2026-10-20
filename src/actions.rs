//! Process spawning and external links.
//!
//! All game launches use `stdin`/`stdout`/`stderr` redirected to null so a
//! console window does not appear under the GUI. Working directory is always
//! the Kingpin install root — Quake 2 engine games expect that for `main/`,
//! mods, and relative paths.
//!
//! Multiplayer join builds classic Quake 2 cmdline args:
//! `kingpin.exe +set game <mod> +connect host:port`

use std::path::Path;
use std::process::{Command, Stdio};

/// Community Discord invite opened from the header button.
pub const DISCORD_URL: &str = "https://discord.gg/SWMG6FD";

/// Launch single-player (`kingpin.exe` with no extra args).
pub fn launch_single_player(game_dir: &Path) -> Result<(), String> {
	let exe = game_dir.join("kingpin.exe");
	if !exe.is_file() {
		return Err("kingpin.exe not found — open Patches to fix.".into());
	}
	spawn(&exe, game_dir, &[])
}

/// Join a multiplayer server, optionally switching to a mod via `+set game`.
///
/// Returns an error if the mod folder is missing so the user can install the
/// client pack under Mods before retrying.
pub fn join_server(game_dir: &Path, address: &str, game: &str) -> Result<(), String> {
	let exe = game_dir.join("kingpin.exe");
	if !exe.is_file() {
		return Err("kingpin.exe not found — open Patches to fix.".into());
	}
	if address.trim().is_empty() || !address.contains(':') {
		return Err("Invalid server address.".into());
	}

	let mod_dir = normalize_game_dir(game);
	if !mod_dir.eq_ignore_ascii_case("main") {
		let path = game_dir.join(&mod_dir);
		if !path.is_dir() {
			return Err(format!(
				"Missing mod folder \"{mod_dir}\" — install its client pack under Mods, then Join again."
			));
		}
	}

	let mut args: Vec<&str> = Vec::new();
	if !mod_dir.eq_ignore_ascii_case("main") {
		args.extend_from_slice(&["+set", "game", mod_dir.as_str()]);
	}
	args.extend_from_slice(&["+connect", address]);
	spawn(&exe, game_dir, &args)
}

/// Map server `game` / `gamedir` infostring values to a local folder name.
///
/// Pass the real client folder (`game`/`gamedir`), not display `gamename`.
/// Long names (`"Hitmen Mod"`) take the first token. Known aliases and
/// server-only packs (Botmatch, Monkey) map to `main`.
pub fn normalize_game_dir(raw: &str) -> String {
	let s = raw.trim();
	if s.is_empty() {
		return "main".into();
	}
	let token = s
		.split(|c: char| c.is_whitespace() || c == '/' || c == '\\')
		.next()
		.unwrap_or("main")
		.trim();
	if token.is_empty() {
		return "main".into();
	}
	// Reject path traversal / odd chars from untrusted server strings.
	if !token
		.chars()
		.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
	{
		return "main".into();
	}
	let lower = token.to_ascii_lowercase();
	match lower.as_str() {
		"baseq2" | "kingpin" | "kp" => "main".into(),
		other if other.starts_with("botmatch") || other.starts_with("monkey") => "main".into(),
		other => other.to_string(),
	}
}

/// Path to optional classic M-Browser, if installed.
pub fn mbrowser_exe(game_dir: &Path) -> Result<std::path::PathBuf, String> {
	let exe = game_dir.join("Kingpin_M_Browser.exe");
	if !exe.is_file() {
		return Err("Classic M-Browser not found — optional install is under Patches.".into());
	}
	Ok(exe)
}

/// Launch the optional classic Hambloch M-Browser.
pub fn launch_mbrowser(game_dir: &Path) -> Result<(), String> {
	let exe = mbrowser_exe(game_dir)?;
	spawn(&exe, game_dir, &[])
}

/// Launch MH's `patchcfg.exe` (patch options UI).
pub fn launch_patchcfg(game_dir: &Path) -> Result<(), String> {
	let exe = game_dir.join("patchcfg.exe");
	if !exe.is_file() {
		return Err("patchcfg.exe not found — install the MH patch first.".into());
	}
	spawn(&exe, game_dir, &[])
}

/// Open the Discord invite in the system browser.
pub fn open_discord() -> Result<(), String> {
	open::that(DISCORD_URL).map_err(|e| format!("Could not open Discord: {e}"))
}

/// Reveal the Kingpin folder in Explorer.
pub fn open_game_folder(game_dir: &Path) -> Result<(), String> {
	if !game_dir.is_dir() {
		return Err("Game folder does not exist.".into());
	}
	open::that(game_dir).map_err(|e| format!("Could not open folder: {e}"))
}

/// Spawn a child process detached from our console I/O.
fn spawn(exe: &Path, cwd: &Path, args: &[&str]) -> Result<(), String> {
	Command::new(exe)
		.args(args)
		.current_dir(cwd)
		.stdin(Stdio::null())
		.stdout(Stdio::null())
		.stderr(Stdio::null())
		.spawn()
		.map(|_| ())
		.map_err(|e| format!("Failed to start {}: {e}", exe.display()))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn normalizes_game_dirs() {
		assert_eq!(normalize_game_dir(""), "main");
		assert_eq!(normalize_game_dir("hitmen"), "hitmen");
		assert_eq!(normalize_game_dir("CTF"), "ctf");
		assert_eq!(normalize_game_dir("kingpin"), "main");
		assert_eq!(normalize_game_dir("Hitmen Mod"), "hitmen");
		assert_eq!(normalize_game_dir("Botmatch.v37"), "main");
		assert_eq!(normalize_game_dir("botmatch.v39.5"), "main");
		assert_eq!(normalize_game_dir("Monkey Mod"), "main");
	}
}
