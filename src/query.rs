//! Live server queries over Quake 2 / Kingpin connectionless UDP.
//!
//! Flow:
//! 1. [`refresh_live_async`] loads seed addresses from the GitHub mirror
//!    (game / connect ports)
//! 2. Each address is queried in parallel with `\xff\xff\xff\xffstatus`
//!    — try **query port** (game − 10) first, then the game port
//! 3. Responses are parsed as infostrings + optional player lines
//! 4. Results are sorted online-first, then by ping / players / name
//!
//! Offline seeds still appear in the table (timeout) so the user can see
//! known hosts even when unreachable.

use crate::servers::{GameServer, PlayerRow, ServerKind};
use std::collections::BTreeMap;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// Quake 2 out-of-band packet header.
const OOB: [u8; 4] = [0xff, 0xff, 0xff, 0xff];
/// Per-server UDP timeout — keep short so a full refresh stays snappy.
const QUERY_TIMEOUT: Duration = Duration::from_millis(1200);

/// Background: fetch address list, query all hosts, send results on `tx`.
pub fn refresh_live_async(tx: mpsc::Sender<Result<Vec<GameServer>, String>>) {
	thread::spawn(move || {
		let result = (|| {
			let seeds = crate::servers::fetch_address_list()?;
			Ok(query_many(&seeds))
		})();
		let _ = tx.send(result);
	});
}

/// Query every address concurrently (one thread each) and sort for display.
pub fn query_many(addresses: &[String]) -> Vec<GameServer> {
	let (tx, rx) = mpsc::channel();
	let timeout = QUERY_TIMEOUT;

	thread::scope(|scope| {
		for addr in addresses {
			let addr = addr.clone();
			let tx = tx.clone();
			scope.spawn(move || {
				let result = query_one(&addr, timeout);
				let _ = tx.send(result);
			});
		}
		drop(tx);
	});

	let mut out: Vec<GameServer> = rx.into_iter().collect();
	out.sort_by(|a, b| {
		b.online
			.cmp(&a.online)
			.then_with(|| {
				a.ping_ms
					.unwrap_or(u32::MAX)
					.cmp(&b.ping_ms.unwrap_or(u32::MAX))
			})
			.then_with(|| b.players.cmp(&a.players))
			.then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
	});
	out
}

/// Candidate UDP targets for a mirrored **game** port address.
///
/// Kingpin masters/browsers use query port = game − 10 for `status`;
/// some dedicated builds also answer on the game port. We try query first.
fn status_targets(game_address: &str) -> Vec<String> {
	let mut out = Vec::with_capacity(2);
	if let Some((host, port_s)) = game_address.rsplit_once(':') {
		if let Ok(port) = port_s.parse::<u16>() {
			if port >= 10 {
				out.push(format!("{}:{}", host, port - 10));
			}
		}
	}
	out.push(game_address.to_string());
	out
}

/// Send `status` to one host and parse the reply (or return an offline stub).
///
/// `address` is the **game / connect** port from the mirror. Join uses it as-is.
pub fn query_one(address: &str, timeout: Duration) -> GameServer {
	let kind = crate::servers::classify_address(address);
	let fallback = GameServer {
		address: address.to_string(),
		name: address.to_string(),
		map: String::new(),
		players: 0,
		max_players: 0,
		ping_ms: None,
		kind,
		online: false,
		game: String::new(),
		rules: BTreeMap::new(),
		player_rows: Vec::new(),
	};

	let Ok(sock) = UdpSocket::bind("0.0.0.0:0") else {
		return fallback;
	};
	let _ = sock.set_read_timeout(Some(timeout));
	let _ = sock.set_write_timeout(Some(timeout));

	let mut packet = Vec::with_capacity(16);
	packet.extend_from_slice(&OOB);
	packet.extend_from_slice(b"status");

	for target in status_targets(address) {
		let Ok(mut addrs) = target.to_socket_addrs() else {
			continue;
		};
		let Some(dest): Option<SocketAddr> = addrs.next() else {
			continue;
		};

		let t0 = Instant::now();
		if sock.send_to(&packet, dest).is_err() {
			continue;
		}

		let mut buf = [0u8; 16_384];
		let Ok((n, _)) = sock.recv_from(&mut buf) else {
			continue;
		};
		let ping_ms = t0.elapsed().as_millis().min(u128::from(u32::MAX)) as u32;

		if let Some(server) = parse_status_response(address, kind, ping_ms, &buf[..n]) {
			return server;
		}
	}

	fallback
}

/// Parse a Quake 2 `print\n\\key\\value...` status payload into a [`GameServer`].
pub fn parse_status_response(
	address: &str,
	kind: ServerKind,
	ping_ms: u32,
	raw: &[u8],
) -> Option<GameServer> {
	let payload = strip_oob(raw)?;
	let text = String::from_utf8_lossy(payload);
	let text = text.trim_start_matches('\0');

	// Accept both `print\n` wrappers and bare infostrings.
	let body = if let Some(rest) = text.strip_prefix("print\n") {
		rest
	} else if let Some(rest) = text.strip_prefix("print\r\n") {
		rest
	} else if text.starts_with('\\') {
		text
	} else {
		return None;
	};

	let mut lines = body.lines();
	let info_line = lines.next().unwrap_or("").trim();
	let rules = parse_infostring(info_line);

	let mut player_rows = Vec::new();
	for line in lines {
		let line = line.trim();
		if line.is_empty() {
			continue;
		}
		if let Some(row) = parse_player_line(line) {
			player_rows.push(row);
		}
	}

	let name = rules
		.get("hostname")
		.cloned()
		.unwrap_or_else(|| address.to_string());
	let map = rules
		.get("mapname")
		.or_else(|| rules.get("map"))
		.cloned()
		.unwrap_or_default();
	let max_players = rules
		.get("maxclients")
		.or_else(|| rules.get("maxplayers"))
		.and_then(|v| v.parse().ok())
		.unwrap_or(0);
	let players = if !player_rows.is_empty() {
		player_rows.len() as u32
	} else {
		rules
			.get("clients")
			.or_else(|| rules.get("players"))
			.and_then(|v| v.parse().ok())
			.unwrap_or(player_rows.len() as u32)
	};
	// Client folder for `+set game` — prefer real `game`/`gamedir`.
	// `gamename` is a display string (e.g. Botmatch.v37) and is often
	// server-only; clients still run `main`. Shown via `GameServer::game_label`.
	let game = rules
		.get("game")
		.or_else(|| rules.get("gamedir"))
		.cloned()
		.unwrap_or_default();

	Some(GameServer {
		address: address.to_string(),
		name,
		map,
		players,
		max_players,
		ping_ms: Some(ping_ms),
		kind,
		online: true,
		game,
		rules,
		player_rows,
	})
}

fn strip_oob(raw: &[u8]) -> Option<&[u8]> {
	if raw.len() >= 4 && raw[..4] == OOB {
		Some(&raw[4..])
	} else {
		Some(raw)
	}
}

/// Parse `\key\value\key\value…` Quake infostrings into a map.
fn parse_infostring(s: &str) -> BTreeMap<String, String> {
	let mut map = BTreeMap::new();
	let s = s.trim().trim_start_matches('\\');
	if s.is_empty() {
		return map;
	}
	let parts: Vec<&str> = s.split('\\').collect();
	let mut i = 0;
	while i + 1 < parts.len() {
		let key = parts[i].to_string();
		let val = parts[i + 1].to_string();
		if !key.is_empty() {
			map.insert(key, val);
		}
		i += 2;
	}
	map
}

/// Player lines look like: `score ping "name"`.
fn parse_player_line(line: &str) -> Option<PlayerRow> {
	let mut parts = line.splitn(3, char::is_whitespace);
	let score: i32 = parts.next()?.parse().ok()?;
	let ping: u32 = parts.next()?.parse().ok()?;
	let rest = parts.next()?.trim();
	let name = if rest.starts_with('"') && rest.ends_with('"') && rest.len() >= 2 {
		rest[1..rest.len() - 1].to_string()
	} else {
		rest.trim_matches('"').to_string()
	};
	Some(PlayerRow { name, score, ping })
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn parses_info_and_players() {
		let raw = b"\xff\xff\xff\xffprint\n\\hostname\\Test Server\\mapname\\kpdm1\\maxclients\\16\\game\\main\n10 45 \"PlayerOne\"\n0 12 \"PlayerTwo\"\n";
		let s = parse_status_response("1.2.3.4:31510", ServerKind::Kingpin, 40, raw).unwrap();
		assert!(s.online);
		assert_eq!(s.name, "Test Server");
		assert_eq!(s.map, "kpdm1");
		assert_eq!(s.max_players, 16);
		assert_eq!(s.players, 2);
		assert_eq!(s.player_rows[0].name, "PlayerOne");
		assert_eq!(s.ping_ms, Some(40));
	}

	#[test]
	fn status_targets_prefer_query_port() {
		assert_eq!(
			status_targets("203.212.17.27:31211"),
			vec![
				"203.212.17.27:31201".to_string(),
				"203.212.17.27:31211".to_string(),
			]
		);
	}
}
