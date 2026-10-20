//! Server list seed + shared server model for the ASE-style browser.
//!
//! # Why a mirror?
//! Hitting hambloch.com (or community masters) from every launcher install on
//! every refresh looks like abuse and has gotten IPs blocked. We maintain
//! [kingpin-servers-mirror](https://github.com/shavargo/kingpin-servers-mirror),
//! refreshed daily by GitHub Actions from Hambloch + Gamespy/Q2 masters.
//!
//! # Caching
//! Beside the `.exe` we keep `cache/servers.txt` and `cache/servers.sha`
//! (git blob SHA from the GitHub Contents API). On each load we compare SHAs;
//! if unchanged we reuse the file. Prefer the Contents API over
//! `raw.githubusercontent.com` — the raw CDN can lag behind pushes.
//!
//! # Auth
//! All fetches are anonymous (User-Agent only). No GitHub tokens are read or sent.
//!
//! Live name/map/players/ping still come from UDP `status` queries in [`crate::query`].

use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

/// Human-facing page for the mirror repo (Browser "Web list" link).
pub const SERVERS_PAGE_URL: &str = "https://github.com/shavargo/kingpin-servers-mirror";

/// GitHub Contents API — returns git blob `sha` + file body (avoids stale raw CDN).
const SERVERS_API_URL: &str =
	"https://api.github.com/repos/shavargo/kingpin-servers-mirror/contents/servers.txt?ref=main";
/// Fresh CDN fallback when the API is rate-limited.
const SERVERS_JSDELIVR_URL: &str =
	"https://cdn.jsdelivr.net/gh/shavargo/kingpin-servers-mirror@main/servers.txt";
/// Last-resort raw URL (can lag behind pushes by several minutes).
const SERVERS_RAW_URL: &str =
	"https://raw.githubusercontent.com/shavargo/kingpin-servers-mirror/main/servers.txt";

const USER_AGENT: &str = "kingpin-launcher/0.1";

/// Subset of the GitHub Contents API JSON we care about.
#[derive(Debug, Deserialize)]
struct GithubContents {
	sha: String,
	encoding: Option<String>,
	content: Option<String>,
	download_url: Option<String>,
}

/// Coarse game family for Browser filters (by query/game port heuristics).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerKind {
	Kingpin,
	Quake2,
	Other,
}

/// One player from a UDP status player list line.
#[derive(Debug, Clone)]
pub struct PlayerRow {
	pub name: String,
	pub score: i32,
	pub ping: u32,
}

/// One row in the ASE-style server table (seed address + live query fields).
#[derive(Debug, Clone)]
pub struct GameServer {
	/// `ip:port` used for UDP status and `+connect`.
	pub address: String,
	pub name: String,
	pub map: String,
	pub players: u32,
	pub max_players: u32,
	/// Round-trip milliseconds from the status query; `None` if offline.
	pub ping_ms: Option<u32>,
	pub kind: ServerKind,
	pub online: bool,
	/// Mod / game folder hint from the infostring (`gamename` / `game`).
	pub game: String,
	/// Full `\key\value` rules from the status response (Server Info pane).
	pub rules: BTreeMap<String, String>,
	pub player_rows: Vec<PlayerRow>,
}

impl GameServer {
	pub fn players_label(&self) -> String {
		if self.max_players > 0 {
			format!("{}/{}", self.players, self.max_players)
		} else {
			format!("{}", self.players)
		}
	}

	pub fn ping_label(&self) -> String {
		if !self.online {
			return "timeout".into();
		}
		match self.ping_ms {
			Some(ms) => format!("{ms}"),
			None => "—".into(),
		}
	}

	pub fn is_full(&self) -> bool {
		self.max_players > 0 && self.players >= self.max_players
	}

	pub fn is_empty(&self) -> bool {
		self.players == 0
	}
}

/// `cache/` folder next to the executable (portable with the `.exe`).
fn cache_dir() -> PathBuf {
	std::env::current_exe()
		.ok()
		.and_then(|p| p.parent().map(|d| d.join("cache")))
		.unwrap_or_else(|| PathBuf::from("cache"))
}

fn cache_paths() -> (PathBuf, PathBuf) {
	let dir = cache_dir();
	(dir.join("servers.txt"), dir.join("servers.sha"))
}

fn read_cached_sha() -> Option<String> {
	let (_, sha_path) = cache_paths();
	fs::read_to_string(sha_path)
		.ok()
		.map(|s| s.trim().to_string())
		.filter(|s| !s.is_empty())
}

fn read_cached_body() -> Option<String> {
	let (body_path, _) = cache_paths();
	fs::read_to_string(body_path)
		.ok()
		.filter(|s| !s.trim().is_empty())
}

fn write_cache(body: &str, sha: &str) -> Result<(), String> {
	let dir = cache_dir();
	fs::create_dir_all(&dir).map_err(|e| format!("cache dir: {e}"))?;
	let (body_path, sha_path) = cache_paths();
	fs::write(&body_path, body).map_err(|e| format!("cache write: {e}"))?;
	fs::write(&sha_path, sha.trim()).map_err(|e| format!("sha write: {e}"))?;
	Ok(())
}

fn http_get_text(url: &str) -> Result<String, String> {
	ureq::get(url)
		.set("User-Agent", USER_AGENT)
		.set("Accept", "application/vnd.github+json, text/plain, */*")
		.call()
		.map_err(|e| format!("{url}: {e}"))?
		.into_string()
		.map_err(|e| format!("{url}: {e}"))
}

fn decode_github_contents(meta: &GithubContents) -> Result<String, String> {
	if let (Some(encoding), Some(content)) = (&meta.encoding, &meta.content) {
		if encoding == "base64" {
			let cleaned: String = content.chars().filter(|c| !c.is_whitespace()).collect();
			let bytes = decode_base64(&cleaned)?;
			return String::from_utf8(bytes).map_err(|e| format!("utf-8: {e}"));
		}
	}
	if let Some(url) = &meta.download_url {
		return http_get_text(url);
	}
	Err("GitHub contents response had no usable body.".into())
}

/// Minimal base64 decode (std-only) for GitHub Contents payloads.
fn decode_base64(input: &str) -> Result<Vec<u8>, String> {
	fn val(c: u8) -> Result<u8, String> {
		Ok(match c {
			b'A'..=b'Z' => c - b'A',
			b'a'..=b'z' => c - b'a' + 26,
			b'0'..=b'9' => c - b'0' + 52,
			b'+' => 62,
			b'/' => 63,
			_ => return Err(format!("invalid base64 byte {c}")),
		})
	}
	let bytes = input.as_bytes();
	let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
	let mut i = 0;
	while i + 3 < bytes.len() {
		let (a, b, c, d) = (bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]);
		i += 4;
		if a == b'=' {
			break;
		}
		let av = val(a)?;
		let bv = val(b)?;
		out.push((av << 2) | (bv >> 4));
		if c != b'=' {
			let cv = val(c)?;
			out.push(((bv & 0xf) << 4) | (cv >> 2));
			if d != b'=' {
				let dv = val(d)?;
				out.push(((cv & 0x3) << 6) | dv);
			}
		}
	}
	Ok(out)
}

fn fetch_via_github_api() -> Result<(String, String), String> {
	let raw = http_get_text(SERVERS_API_URL)?;
	let meta: GithubContents =
		serde_json::from_str(&raw).map_err(|e| format!("GitHub API JSON: {e}"))?;
	if meta.sha.is_empty() {
		return Err("GitHub API returned empty sha.".into());
	}
	let body = decode_github_contents(&meta)?;
	if body.trim().is_empty() {
		return Err("GitHub API returned empty servers.txt.".into());
	}
	Ok((meta.sha, body))
}

fn fetch_via_plain_url(url: &str) -> Result<String, String> {
	let body = http_get_text(url)?;
	if body.trim().is_empty() {
		return Err(format!("{url}: empty body"));
	}
	Ok(body)
}

/// Load `servers.txt` text, preferring cache when the remote git blob SHA matches.
///
/// Order: GitHub Contents API → jsDelivr → raw.githubusercontent.com → on-disk cache.
pub fn load_servers_txt() -> Result<String, String> {
	let cached_sha = read_cached_sha();
	let cached_body = read_cached_body();

	match fetch_via_github_api() {
		Ok((sha, body)) => {
			// Unchanged remote — avoid rewriting disk and return the local copy.
			if cached_sha.as_deref() == Some(sha.as_str()) {
				if let Some(body) = cached_body {
					return Ok(body);
				}
			}
			let _ = write_cache(&body, &sha);
			return Ok(body);
		}
		Err(api_err) => {
			// CDN / raw fallbacks when API is rate-limited or unreachable.
			for url in [SERVERS_JSDELIVR_URL, SERVERS_RAW_URL] {
				match fetch_via_plain_url(url) {
					Ok(body) => {
						// No git sha from plain URL — store a content tag so we still refresh later.
						let tag = cached_sha
							.clone()
							.unwrap_or_else(|| format!("fallback-{}", body.len()));
						let _ = write_cache(&body, &tag);
						return Ok(body);
					}
					Err(_) => continue,
				}
			}
			if let Some(body) = cached_body {
				return Ok(body);
			}
			Err(format!("Could not fetch server list: {api_err}"))
		}
	}
}

fn parse_address_list(text: &str) -> Vec<String> {
	let mut addrs = Vec::new();
	for line in text.lines() {
		let line = line.trim();
		if line.is_empty() || line.starts_with('#') {
			continue;
		}
		if let Some(addr) = line.split_whitespace().next() {
			if addr.contains(':') && !addrs.iter().any(|a| a == addr) {
				addrs.push(addr.to_string());
			}
		}
	}
	addrs
}

/// Parse unique `ip:port` seeds from the mirrored list (comments/blank lines skipped).
pub fn fetch_address_list() -> Result<Vec<String>, String> {
	let text = load_servers_txt()?;
	let addrs = parse_address_list(&text);
	if addrs.is_empty() {
		return Err("Server list was empty.".into());
	}
	Ok(addrs)
}

/// Warm the on-disk list cache (SHA check). Safe to call from a background thread at launch.
pub fn sync_list_cache() {
	let _ = load_servers_txt();
}

/// Heuristic: Quake 2 default game port vs typical Kingpin ports.
pub fn classify_address(address: &str) -> ServerKind {
	let port = address
		.rsplit_once(':')
		.and_then(|(_, p)| p.parse::<u16>().ok())
		.unwrap_or(0);
	if port == 27910 {
		ServerKind::Quake2
	} else if port > 0 {
		ServerKind::Kingpin
	} else {
		ServerKind::Other
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn classifies_ports() {
		assert_eq!(classify_address("1.1.1.1:27910"), ServerKind::Quake2);
		assert_eq!(classify_address("1.1.1.1:31510"), ServerKind::Kingpin);
	}

	#[test]
	fn decodes_github_base64() {
		// "hi\n" in base64
		let out = decode_base64("aGkK").unwrap();
		assert_eq!(out, b"hi\n");
	}
}
