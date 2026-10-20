//! Read / write Kingpin `autoexec.cfg` and manage common key binds.
//!
//! # Marker block
//! Edits from the Controls tab are wrapped between [`MARKER_BEGIN`] and
//! [`MARKER_END`] so we can rewrite our section without destroying the rest of
//! the user's autoexec (aliases, custom scripts, etc.).
//!
//! # Games / mods
//! Each mod folder (`main`, `hitmen`, `ctf`, …) can have its own `autoexec.cfg`.
//! [`list_mod_dirs`] discovers those folders for the game dropdown.
//!
//! Bind lines follow Quake 2 syntax: `bind KEY command`.

use std::fs;
use std::path::{Path, PathBuf};

/// Start of the launcher-managed section inside `autoexec.cfg`.
pub const MARKER_BEGIN: &str = "// --- kingpin-launcher controls begin ---";
/// End of the launcher-managed section inside `autoexec.cfg`.
pub const MARKER_END: &str = "// --- kingpin-launcher controls end ---";

/// Common actions shown in the Controls UI (engine command → friendly label).
pub const CONTROL_ACTIONS: &[(&str, &str)] = &[
    ("+forward", "Forward"),
    ("+back", "Back"),
    ("+moveleft", "Strafe left"),
    ("+moveright", "Strafe right"),
    ("+moveup", "Jump"),
    ("+movedown", "Crouch"),
    ("+speed", "Walk / run"),
    ("+attack", "Attack"),
    ("zoom", "Zoom / ADS"),
    ("+activate", "Use / interact"),
    ("reload", "Reload"),
    ("holster", "Holster"),
    ("flashlight", "Flashlight"),
    ("weapnext", "Next weapon"),
    ("weapprev", "Previous weapon"),
    ("inven", "Inventory"),
    ("messagemode", "Chat"),
    ("messagemode2", "Team chat"),
    ("key1", "Key / pipe slot 1"),
    ("key3", "Key slot 3"),
];

pub fn autoexec_path(game_dir: &Path, game: &str) -> PathBuf {
    game_dir.join(game).join("autoexec.cfg")
}

const SKIP_DIRS: &[&str] = &[
    "bin",
    "docs",
    "drivers",
    "help",
    "manual",
    "redist",
    "steamapps",
    "_broken",
    "_old",
    "_oldsource",
    "_sourceonly",
];

/// First-level folders that look like Kingpin game/mod dirs (main, hitmen, ctf, …).
pub fn list_mod_dirs(game_dir: &Path) -> Vec<String> {
    let mut games = Vec::new();
    let Ok(entries) = fs::read_dir(game_dir) else {
        return games;
    };
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        if !ft.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || SKIP_DIRS.iter().any(|s| name.eq_ignore_ascii_case(s)) {
            continue;
        }
        if name.eq_ignore_ascii_case("main") || looks_like_mod_dir(&entry.path()) {
            if !games.iter().any(|g| g == &name) {
                games.push(name);
            }
        }
    }
    if !games.iter().any(|g| g.eq_ignore_ascii_case("main")) {
        games.insert(0, "main".into());
    }
    games.sort_by(|a, b| {
        let a_main = a.eq_ignore_ascii_case("main");
        let b_main = b.eq_ignore_ascii_case("main");
        b_main
            .cmp(&a_main)
            .then_with(|| a.to_lowercase().cmp(&b.to_lowercase()))
    });
    games
}

fn looks_like_mod_dir(path: &Path) -> bool {
    if path.join("autoexec.cfg").is_file() || path.join("config.cfg").is_file() {
        return true;
    }
    let Ok(entries) = fs::read_dir(path) else {
        return false;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if name.ends_with(".pak")
            || name.ends_with(".dll")
            || name == "maps"
            || name == "models"
            || name == "sound"
            || name == "sprites"
        {
            return true;
        }
    }
    false
}

/// Shared `set` cvars that are safe to push into every mod autoexec.
pub const SHARED_CVARS: &[&str] = &[
    "showfps",
    "vid_size",
    "vid_fullscreen",
    "vid_nativefs",
    "cl_adjustfov",
    "gl_maxtexsize",
    "r_maxfps",
    "name",
    "skin",
    "extras",
    "gender",
    "gender_auto",
];

pub struct ApplyAllResult {
    pub written: usize,
    pub skipped_binds: usize,
    pub notes: Vec<String>,
    pub errors: Vec<String>,
}

/// Merge shared movement binds + display cvars from `source` into every mod folder.
/// Preserves mod-specific lines (`exec`, unique binds, etc.). Skips a shared bind when
/// that key is already used by a non-shared (mod-specific) command.
pub fn apply_to_all_mods(game_dir: &Path, source: &str) -> Result<ApplyAllResult, String> {
    let games = list_mod_dirs(game_dir);
    if games.is_empty() {
        return Err("No mod folders found under the Kingpin directory.".into());
    }

    let mut result = ApplyAllResult {
        written: 0,
        skipped_binds: 0,
        notes: Vec::new(),
        errors: Vec::new(),
    };

    for game in &games {
        let existing = match load(game_dir, game) {
            Ok(t) => t,
            Err(e) => {
                result.errors.push(format!("{game}: {e}"));
                continue;
            }
        };
        let (merged, skipped) = merge_shared_into_mod(source, &existing);
        result.skipped_binds += skipped.len();
        for s in skipped {
            result.notes.push(format!("{game}: {s}"));
        }
        match save(game_dir, game, &merged) {
            Ok(()) => result.written += 1,
            Err(e) => result.errors.push(format!("{game}: {e}")),
        }
    }
    Ok(result)
}

/// Merge shared settings from `source` into an existing mod autoexec.
/// Returns `(merged_text, skip_notes)`.
pub fn merge_shared_into_mod(source: &str, target: &str) -> (String, Vec<String>) {
    let mut out = strip_launcher_markers(target);
    let mut notes = Vec::new();

    for name in SHARED_CVARS {
        if let Some(val) = get_cvar(source, name) {
            out = set_cvar(&out, name, &val);
        }
    }

    for (action, _) in CONTROL_ACTIONS {
        let Some(key) = primary_key_for_action(source, action) else {
            continue;
        };

        // If this key already runs a mod-specific command, leave it alone.
        if let Some(existing_cmd) = command_on_key(&out, &key) {
            if !is_shared_action(&existing_cmd) {
                notes.push(format!(
                    "kept bind {key} \"{existing_cmd}\" (skipped shared {action})"
                ));
                continue;
            }
        }

        out = set_bind(&out, action, &key);
    }

    (out, notes)
}

fn strip_launcher_markers(contents: &str) -> String {
    let lines: Vec<String> = contents.lines().map(|l| l.to_string()).collect();
    if let Some((begin, end)) = find_markers(&lines) {
        let mut out = Vec::new();
        out.extend(lines[..begin].iter().cloned());
        out.extend(lines[end + 1..].iter().cloned());
        return join_lines(&out);
    }
    if contents.trim().is_empty() {
        String::new()
    } else {
        normalize_newlines(contents)
    }
}

fn is_shared_action(cmd: &str) -> bool {
    let cmd = strip_quotes(cmd);
    CONTROL_ACTIONS
        .iter()
        .any(|(action, _)| cmds_equal(&cmd, action))
}

fn command_on_key(contents: &str, key: &str) -> Option<String> {
    parse_binds(contents)
        .into_iter()
        .rev()
        .find(|(k, _)| keys_equal(k, key))
        .map(|(_, cmd)| cmd)
}

pub fn load(game_dir: &Path, game: &str) -> Result<String, String> {
    let path = autoexec_path(game_dir, game);
    if path.is_file() {
        return fs::read_to_string(&path).map_err(|e| format!("Read {}: {e}", path.display()));
    }
    Ok(default_template())
}

pub fn save(game_dir: &Path, game: &str, contents: &str) -> Result<(), String> {
    let dir = game_dir.join(game);
    fs::create_dir_all(&dir).map_err(|e| {
        format!(
            "Cannot create {}: {e}. Try running as Administrator under Program Files.",
            dir.display()
        )
    })?;
    let path = dir.join("autoexec.cfg");
    fs::write(&path, normalize_newlines(contents)).map_err(|e| {
        format!(
            "Cannot write {}: {e}. Try running as Administrator under Program Files.",
            path.display()
        )
    })
}

pub fn default_template() -> String {
    format!(
        "{MARKER_BEGIN}\n{}\n{MARKER_END}\n",
        modern_wasd_block()
    )
}

pub fn modern_wasd_block() -> String {
    r#"// Modern FPS controls (override config.cfg each launch)
unbind SPACE
unbind CTRL
unbind w
unbind a
unbind s
unbind d
unbind c
unbind e
unbind MOUSE2
unbind MOUSE3
unbind MWHEELUP
unbind MWHEELDOWN
unbind MWHEEL_U
unbind MWHEEL_D

bind w "+forward"
bind a "+moveleft"
bind s "+back"
bind d "+moveright"
bind SPACE "+moveup"
bind CTRL "+movedown"
bind c "+movedown"
bind SHIFT "+speed"
bind MOUSE1 "+attack"
bind MOUSE2 "zoom"
bind MOUSE3 "+activate"
bind e "+activate"
bind f "+activate"
bind r "reload"
bind g "holster"
bind l "flashlight"
bind q "weapprev"
bind MWHEEL_U "weapnext"
bind MWHEEL_D "weapprev"
bind TAB "inven"
bind t "messagemode"
bind y "messagemode2"
bind v "messagemode2"
bind x "key3"
bind CAPSLOCK "key1""#
        .trim()
        .to_string()
}

/// Keys currently bound to an action command (e.g. `+forward`).
pub fn keys_for_action(contents: &str, action: &str) -> Vec<String> {
    parse_binds(contents)
        .into_iter()
        .filter(|(_, cmd)| cmds_equal(cmd, action))
        .map(|(key, _)| key)
        .collect()
}

pub fn primary_key_for_action(contents: &str, action: &str) -> Option<String> {
    keys_for_action(contents, action).into_iter().next()
}

/// Set the primary bind for an action. Removes other binds to the same command,
/// clears the new key if it was bound elsewhere, and writes into the managed block
/// when markers exist (otherwise appends a bind line).
pub fn set_bind(contents: &str, action: &str, new_key: &str) -> String {
    let new_key = normalize_key_name(new_key);
    let mut lines: Vec<String> = contents.lines().map(|l| l.to_string()).collect();
    let action_norm = strip_quotes(action);

    // Drop existing binds for this action and any bind on the new key.
    lines.retain(|line| {
        if let Some((key, cmd)) = parse_bind_line(line) {
            if cmds_equal(&cmd, &action_norm) || keys_equal(&key, &new_key) {
                return false;
            }
        }
        true
    });

    let bind_line = format!("bind {new_key} \"{action_norm}\"");

    if let Some((begin, end)) = find_markers(&lines) {
        // Insert before end marker; keep other managed content.
        lines.insert(end, bind_line);
        let _ = begin;
    } else if let Some(idx) = lines.iter().rposition(|l| {
        let t = l.trim();
        t.starts_with("bind ") || t.starts_with("unbind ")
    }) {
        lines.insert(idx + 1, bind_line);
    } else {
        if !lines.is_empty() && !lines.last().map(|l| l.trim().is_empty()).unwrap_or(true) {
            lines.push(String::new());
        }
        lines.push(MARKER_BEGIN.into());
        lines.push(bind_line);
        lines.push(MARKER_END.into());
    }

    join_lines(&lines)
}

pub fn apply_modern_wasd(contents: &str) -> String {
    let block = modern_wasd_block();
    let lines: Vec<String> = contents.lines().map(|l| l.to_string()).collect();
    if let Some((begin, end)) = find_markers(&lines) {
        let mut out = Vec::new();
        out.extend(lines[..begin + 1].iter().cloned());
        out.extend(block.lines().map(|l| l.to_string()));
        out.extend(lines[end..].iter().cloned());
        return join_lines(&out);
    }

    // Strip known control binds, then append a managed block.
    let known: Vec<&str> = CONTROL_ACTIONS.iter().map(|(c, _)| *c).collect();
    let mut out: Vec<String> = lines
        .into_iter()
        .filter(|line| {
            if let Some((_, cmd)) = parse_bind_line(line) {
                return !known.iter().any(|a| cmds_equal(&cmd, a));
            }
            true
        })
        .collect();
    if !out.is_empty() && !out.last().map(|l| l.trim().is_empty()).unwrap_or(true) {
        out.push(String::new());
    }
    out.push(MARKER_BEGIN.into());
    out.extend(block.lines().map(|l| l.to_string()));
    out.push(MARKER_END.into());
    join_lines(&out)
}

pub fn parse_binds(contents: &str) -> Vec<(String, String)> {
    contents
        .lines()
        .filter_map(parse_bind_line)
        .collect()
}

/// Common `vid_size` values for the Controls dropdown.
pub const RESOLUTIONS: &[&str] = &[
    "1280x720",
    "1366x768",
    "1600x900",
    "1920x1080",
    "1920x1200",
    "2560x1440",
    "2560x1600",
    "3440x1440",
    "3840x2160",
];

pub fn get_cvar(contents: &str, name: &str) -> Option<String> {
    let mut found = None;
    for line in contents.lines() {
        if let Some((n, v)) = parse_set_line(line) {
            if n.eq_ignore_ascii_case(name) {
                found = Some(v);
            }
        }
    }
    found
}

pub fn showfps_enabled(contents: &str) -> bool {
    match get_cvar(contents, "showfps").as_deref() {
        Some("1") | Some("true") | Some("True") | Some("TRUE") => true,
        _ => false,
    }
}

/// Insert or replace `set name "value"` (last existing line wins; duplicates removed).
pub fn set_cvar(contents: &str, name: &str, value: &str) -> String {
    let set_line = format!("set {name} \"{value}\"");
    let mut lines: Vec<String> = contents.lines().map(|l| l.to_string()).collect();
    let mut replaced = false;
    let mut out = Vec::with_capacity(lines.len() + 1);
    for line in lines.drain(..) {
        if let Some((n, _)) = parse_set_line(&line) {
            if n.eq_ignore_ascii_case(name) {
                if !replaced {
                    out.push(set_line.clone());
                    replaced = true;
                }
                continue;
            }
        }
        out.push(line);
    }
    if !replaced {
        // Prefer inserting after other set lines, before bind/unbind blocks.
        if let Some(idx) = out.iter().rposition(|l| parse_set_line(l).is_some()) {
            out.insert(idx + 1, set_line);
        } else if let Some(idx) = out.iter().position(|l| {
            let t = l.trim();
            t.starts_with("bind ") || t.starts_with("unbind ") || t == MARKER_BEGIN
        }) {
            out.insert(idx, set_line);
            out.insert(idx + 1, String::new());
        } else {
            if !out.is_empty() && !out.last().map(|l| l.trim().is_empty()).unwrap_or(true) {
                out.push(String::new());
            }
            out.push(set_line);
        }
    }
    join_lines(&out)
}

fn parse_set_line(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    if trimmed.starts_with("//") {
        return None;
    }
    let rest = trimmed.strip_prefix("set ")?.trim();
    let mut parts = rest.splitn(2, char::is_whitespace);
    let name = parts.next()?.trim().to_string();
    let value = strip_quotes(parts.next()?.trim());
    if name.is_empty() {
        return None;
    }
    Some((name, value))
}

fn parse_bind_line(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    if trimmed.starts_with("//") {
        return None;
    }
    let rest = trimmed.strip_prefix("bind ")?.trim();
    let mut parts = rest.splitn(2, char::is_whitespace);
    let key = parts.next()?.trim().to_string();
    let cmd_raw = parts.next()?.trim();
    let cmd = strip_quotes(cmd_raw);
    if key.is_empty() || cmd.is_empty() {
        return None;
    }
    Some((key, cmd))
}

fn strip_quotes(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

fn cmds_equal(a: &str, b: &str) -> bool {
    strip_quotes(a).eq_ignore_ascii_case(&strip_quotes(b))
}

fn keys_equal(a: &str, b: &str) -> bool {
    normalize_key_name(a).eq_ignore_ascii_case(&normalize_key_name(b))
}

fn find_markers(lines: &[String]) -> Option<(usize, usize)> {
    let begin = lines.iter().position(|l| l.trim() == MARKER_BEGIN)?;
    let end = lines.iter().rposition(|l| l.trim() == MARKER_END)?;
    if end > begin {
        Some((begin, end))
    } else {
        None
    }
}

fn join_lines(lines: &[String]) -> String {
    let mut s = lines.join("\n");
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

fn normalize_newlines(s: &str) -> String {
    let mut out = s.replace("\r\n", "\n").replace('\r', "\n");
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

pub fn normalize_key_name(key: &str) -> String {
    let k = key.trim();
    match k.to_ascii_uppercase().as_str() {
        "SPACE" | "SPACEBAR" => "SPACE".into(),
        "CTRL" | "CONTROL" | "LCTRL" | "RCTRL" => "CTRL".into(),
        "ALT" | "LALT" | "RALT" => "ALT".into(),
        "SHIFT" | "LSHIFT" | "RSHIFT" => "SHIFT".into(),
        "ENTER" | "RETURN" => "ENTER".into(),
        "ESC" | "ESCAPE" => "ESC".into(),
        "TAB" => "TAB".into(),
        "CAPSLOCK" | "CAPS_LOCK" | "CAPS" => "CAPSLOCK".into(),
        "UP" | "ARROWUP" | "U_ARROW" => "U_ARROW".into(),
        "DOWN" | "ARROWDOWN" | "D_ARROW" => "D_ARROW".into(),
        "LEFT" | "ARROWLEFT" | "L_ARROW" => "L_ARROW".into(),
        "RIGHT" | "ARROWRIGHT" | "R_ARROW" => "R_ARROW".into(),
        "MOUSE1" | "MOUSE_LEFT" | "LEFTMOUSE" => "MOUSE1".into(),
        "MOUSE2" | "MOUSE_RIGHT" | "RIGHTMOUSE" => "MOUSE2".into(),
        "MOUSE3" | "MOUSE_MIDDLE" | "MIDDLEMOUSE" => "MOUSE3".into(),
        "MOUSE4" => "MOUSE4".into(),
        "MOUSE5" => "MOUSE5".into(),
        "MWHEELUP" | "MWHEEL_U" | "WHEELUP" => "MWHEEL_U".into(),
        "MWHEELDOWN" | "MWHEEL_D" | "WHEELDOWN" => "MWHEEL_D".into(),
        "BACKSPACE" | "B_SPACE" => "B_SPACE".into(),
        "DELETE" | "DEL" => "DEL".into(),
        "INSERT" | "INS" => "INS".into(),
        "HOME" => "HOME".into(),
        "END" => "END".into(),
        "PAGEUP" | "PGUP" => "PGUP".into(),
        "PAGEDOWN" | "PGDN" => "PGDN".into(),
        "PAUSE" => "PAUSE".into(),
        other if other.len() == 1 => other.to_ascii_lowercase(),
        other if other.starts_with('F') && other[1..].chars().all(|c| c.is_ascii_digit()) => {
            other.to_string()
        }
        _ => k.to_string(),
    }
}

/// Map an egui key event to a Kingpin bind name.
pub fn egui_key_to_kingpin(key: egui::Key) -> Option<String> {
    use egui::Key;
    let name = match key {
        Key::Space => "SPACE",
        Key::Enter => "ENTER",
        Key::Escape => "ESC",
        Key::Tab => "TAB",
        Key::Backspace => "B_SPACE",
        Key::Delete => "DEL",
        Key::Insert => "INS",
        Key::Home => "HOME",
        Key::End => "END",
        Key::PageUp => "PGUP",
        Key::PageDown => "PGDN",
        Key::ArrowUp => "U_ARROW",
        Key::ArrowDown => "D_ARROW",
        Key::ArrowLeft => "L_ARROW",
        Key::ArrowRight => "R_ARROW",
        Key::F1 => "F1",
        Key::F2 => "F2",
        Key::F3 => "F3",
        Key::F4 => "F4",
        Key::F5 => "F5",
        Key::F6 => "F6",
        Key::F7 => "F7",
        Key::F8 => "F8",
        Key::F9 => "F9",
        Key::F10 => "F10",
        Key::F11 => "F11",
        Key::F12 => "F12",
        Key::A => "a",
        Key::B => "b",
        Key::C => "c",
        Key::D => "d",
        Key::E => "e",
        Key::F => "f",
        Key::G => "g",
        Key::H => "h",
        Key::I => "i",
        Key::J => "j",
        Key::K => "k",
        Key::L => "l",
        Key::M => "m",
        Key::N => "n",
        Key::O => "o",
        Key::P => "p",
        Key::Q => "q",
        Key::R => "r",
        Key::S => "s",
        Key::T => "t",
        Key::U => "u",
        Key::V => "v",
        Key::W => "w",
        Key::X => "x",
        Key::Y => "y",
        Key::Z => "z",
		Key::Num0 => "0",
		Key::Num1 => "1",
		Key::Num2 => "2",
		Key::Num3 => "3",
		Key::Num4 => "4",
		Key::Num5 => "5",
		Key::Num6 => "6",
		Key::Num7 => "7",
		Key::Num8 => "8",
		Key::Num9 => "9",
        Key::Minus => "-",
        Key::Equals => "=",
        Key::OpenBracket => "[",
        Key::CloseBracket => "]",
        Key::Backslash => "\\",
        Key::Semicolon => ";",
        Key::Quote => "'",
        Key::Comma => ",",
        Key::Period => ".",
        Key::Slash => "/",
        Key::Backtick => "`",
        _ => return None,
    };
    Some(normalize_key_name(name))
}

pub fn egui_modifiers_to_kingpin(modifiers: &egui::Modifiers) -> Option<&'static str> {
    if modifiers.ctrl {
        Some("CTRL")
    } else if modifiers.shift {
        Some("SHIFT")
    } else if modifiers.alt {
        Some("ALT")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_rebind() {
        let src = r#"
bind w "+forward"
bind MOUSE1 "+attack"
"#;
        assert_eq!(primary_key_for_action(src, "+forward").as_deref(), Some("w"));
        let next = set_bind(src, "+forward", "e");
        assert_eq!(primary_key_for_action(&next, "+forward").as_deref(), Some("e"));
        assert!(keys_for_action(&next, "+forward").len() == 1);
    }

    #[test]
    fn modern_preset_adds_markers() {
        let out = apply_modern_wasd("// comment\n");
        assert!(out.contains(MARKER_BEGIN));
        assert!(out.contains("bind w \"+forward\""));
    }

    #[test]
    fn set_and_read_cvars() {
        let src = "set showfps \"0\"\nset vid_size \"1920x1080\"\n";
        assert!(!showfps_enabled(src));
        let next = set_cvar(src, "showfps", "1");
        assert!(showfps_enabled(&next));
        let next = set_cvar(&next, "vid_size", "2560x1440");
        assert_eq!(get_cvar(&next, "vid_size").as_deref(), Some("2560x1440"));
    }

    #[test]
    fn merge_preserves_mod_specific_binds_and_exec() {
        let source = r#"
set showfps "1"
set vid_size "2560x1440"
bind w "+forward"
bind e "+activate"
"#;
        let target = r#"
exec ctf.run
// DO NOT MODIFY
bind e "push;wait;+activate"
bind z "cloak"
"#;
        let (merged, notes) = merge_shared_into_mod(source, target);
        assert!(merged.contains("exec ctf.run"));
        assert!(merged.contains("push;wait;+activate"));
        assert!(merged.to_ascii_lowercase().contains("cloak"));
        assert!(merged.contains("bind w \"+forward\""));
        assert_eq!(get_cvar(&merged, "showfps").as_deref(), Some("1"));
        assert_eq!(get_cvar(&merged, "vid_size").as_deref(), Some("2560x1440"));
        assert!(notes.iter().any(|n| n.contains("skipped shared +activate")));
    }
}
