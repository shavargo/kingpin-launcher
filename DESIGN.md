# Kingpin Launcher — Design

## Summary

Windows-native (egui/eframe) launcher for *Kingpin: Life of Crime*: single-player, ASE-style server browser, mods, patch checklist, and controls editor.

## UI

| Tab | Role |
|-----|------|
| Play | Launch `kingpin.exe` |
| Browser | Seed from [kingpin-servers-mirror](https://github.com/shavargo/kingpin-servers-mirror), live UDP status, Join |
| Mods | kingpin.info mod catalog + client pack install |
| Patches | Locate install → verify 1.21 → MH patch → optional M-Browser → maps/models |
| Controls | `autoexec.cfg` editor |
| Discord | Always-visible invite |

**Look:** Kingpin noir — dark asphalt, muted blood red, nicotine yellow, brick texture.

## Non-goals

- Not rewriting MH’s `wsock32` patch or M-Browser
- Not a full mod authoring suite
- Not Linux-first for v1

## Architecture

| Module | Responsibility |
|--------|----------------|
| `detect` | Steam/GOG path discovery |
| `checklist` | Ready/missing status for exe, MH patch, optional M-Browser |
| `actions` | Launch SP / join server / patchcfg / Discord |
| `servers` / `query` | Mirror list cache + UDP status |
| `wizard` / `addons` / `mods` | Downloads and installs |
| `autoexec` | Controls tab |
| `ui` / `theme` | egui shell |

## Data (next to the exe)

- `launcher.toml` — game path, last tab
- `cache/servers.txt` + `cache/servers.sha` — mirrored address list (git blob SHA)

No accounts, no telemetry, no GitHub credentials.
