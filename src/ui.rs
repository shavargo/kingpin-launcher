//! Main egui application shell.
//!
//! [`KingpinLauncherApp`] owns all UI state and wires the sidebar tabs to the
//! rest of the crate:
//!
//! | Tab | Backend |
//! |-----|---------|
//! | Play | [`crate::actions::launch_single_player`] |
//! | Browser | [`crate::servers`] + [`crate::query`] (async refresh) |
//! | Mods | [`crate::mods`] catalog / install jobs |
//! | Controls | [`crate::autoexec`] |
//! | Patches | [`crate::checklist`] + [`crate::wizard`] + [`crate::addons`] |
//!
//! Long-running work (UDP queries, downloads) runs on background threads and
//! posts results through `mpsc` channels polled each frame so the UI stays
//! responsive. Window size is clamped every frame because Windows maximize
//! ignores egui's `max_inner_size`.

use crate::actions;
use crate::addons::{self, AddonEvent};
use crate::autoexec;
use crate::checklist::{Checklist, ItemStatus};
use crate::config::LauncherConfig;
use crate::detect::{self, is_kingpin_dir};
use crate::mods::{self, ModsEvent, RemoteMod};
use crate::query;
use crate::servers::{self, GameServer, ServerKind};
use crate::theme;
use crate::wizard::{self, WizardStep};
use eframe::egui::{self, Align, Color32, Event, Layout, RichText, TextStyle};
use egui_extras::{Column, TableBuilder};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Sidebar navigation destinations (persisted as strings in `launcher.toml`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Play,
    Browser,
    Mods,
    Controls,
    Patches,
}

impl Tab {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Play => "play",
            Self::Browser => "browser",
            Self::Mods => "mods",
            Self::Controls => "controls",
            Self::Patches => "patches",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "browser" => Self::Browser,
            "mods" => Self::Mods,
            "controls" => Self::Controls,
            "patches" => Self::Patches,
            _ => Self::Play,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Play => "PLAY",
            Self::Browser => "BROWSER",
            Self::Mods => "MODS",
            Self::Controls => "CONTROLS",
            Self::Patches => "PATCHES",
        }
    }
}

/// Filter chips on the Mods tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModsFilter {
    NeedsClient,
    Missing,
    Installed,
    All,
}

/// Filter chips on the Browser tab (Kingpin ports vs everything).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrowserFilter {
    Kingpin,
    All,
}

/// Sortable columns in the ASE-style server table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortCol {
    Ping,
    Players,
    Name,
    Map,
    Game,
    Address,
}

/// Root application state for the egui frame loop.
pub struct KingpinLauncherApp {
    config: LauncherConfig,
    tab: Tab,
    checklist: Checklist,
    /// Footer status line (ok or error styling via `status_is_error`).
    status_message: String,
    status_is_error: bool,
    /// True while a server refresh (or other shared busy work) is in flight.
    busy: bool,
    // --- Browser ---
    servers: Vec<GameServer>,
    servers_loaded: bool,
    server_filter: BrowserFilter,
    selected_address: Option<String>,
    sort_col: SortCol,
    sort_asc: bool,
    hide_empty: bool,
    hide_full: bool,
    online_only: bool,
    /// When true, re-query about every 30s while the Browser tab is visible.
    auto_refresh: bool,
    server_search: String,
    last_refresh: Option<Instant>,
    refresh_rx: Option<mpsc::Receiver<Result<Vec<GameServer>, String>>>,
    // --- Maps/models installer (Patches) ---
    addon_rx: Option<mpsc::Receiver<AddonEvent>>,
    addon_busy: bool,
    addon_phase: String,
    addon_done: u64,
    addon_total: u64,
    // --- Mods tab ---
    remote_mods: Vec<RemoteMod>,
    mods_loaded: bool,
    mods_busy: bool,
    mods_phase: String,
    mods_filter: ModsFilter,
    mods_rx: Option<mpsc::Receiver<ModsEvent>>,
    installing_mod: Option<String>,
    // --- Controls tab ---
    controls_game: String,
    controls_text: String,
    controls_dirty: bool,
    controls_loaded_for: Option<String>,
    /// When set, the next key/mouse event rebinds this command.
    controls_listening: Option<String>,
    controls_show_raw: bool,
}

impl KingpinLauncherApp {
    /// Construct app state: theme, config, auto-detect game dir, warm server list cache.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&cc.egui_ctx);
        // Required for `include_image!` (header logo PNG).
        egui_extras::install_image_loaders(&cc.egui_ctx);

        let mut config = LauncherConfig::load();
        let saved = config.game_dir();
        // Prefer a still-valid saved path; otherwise search Steam/GOG.
        if let Some(dir) = detect::resolve_game_dir(saved.as_deref()) {
            config.set_game_dir(dir);
            let _ = config.save();
        }

        let tab = config
            .last_tab
            .as_deref()
            .map(Tab::from_str)
            .unwrap_or(Tab::Play);

        let checklist = Checklist::evaluate(config.game_dir().as_deref());

        let mut app = Self {
            config,
            tab,
            checklist,
            status_message: String::new(),
            status_is_error: false,
            busy: false,
            servers: Vec::new(),
            servers_loaded: false,
            server_filter: BrowserFilter::Kingpin,
            selected_address: None,
            sort_col: SortCol::Ping,
            sort_asc: true,
            hide_empty: false,
            hide_full: false,
            online_only: true,
            auto_refresh: true,
            server_search: String::new(),
            last_refresh: None,
            refresh_rx: None,
            addon_rx: None,
            addon_busy: false,
            addon_phase: String::new(),
            addon_done: 0,
            addon_total: 0,
            remote_mods: Vec::new(),
            mods_loaded: false,
            mods_busy: false,
            mods_phase: String::new(),
            mods_filter: ModsFilter::NeedsClient,
            mods_rx: None,
            installing_mod: None,
            controls_game: "main".into(),
            controls_text: String::new(),
            controls_dirty: false,
            controls_loaded_for: None,
            controls_listening: None,
            controls_show_raw: false,
        };
        // Always check the mirror list on launch (body download only if SHA/ETag changed).
        std::thread::spawn(|| crate::servers::sync_list_cache());
        if app.tab == Tab::Browser {
            app.start_server_refresh();
        }
        if app.tab == Tab::Mods {
            app.start_mods_catalog();
        }
        if app.tab == Tab::Controls {
            app.load_controls();
        }
        app
    }

    fn load_controls(&mut self) {
        let Some(dir) = self.config.game_dir() else {
            self.controls_text.clear();
            self.controls_loaded_for = None;
            self.controls_dirty = false;
            return;
        };
        match autoexec::load(&dir, &self.controls_game) {
            Ok(text) => {
                self.controls_text = text;
                self.controls_loaded_for = Some(format!("{}::{}", dir.display(), self.controls_game));
                self.controls_dirty = false;
                self.controls_listening = None;
            }
            Err(e) => {
                self.set_err(e);
                self.controls_text.clear();
                self.controls_loaded_for = None;
            }
        }
    }

    fn save_controls(&mut self) {
        let Some(dir) = self.config.game_dir() else {
            self.set_err("Set the Kingpin folder under Patches first.");
            return;
        };
        match autoexec::save(&dir, &self.controls_game, &self.controls_text) {
            Ok(()) => {
                self.controls_dirty = false;
                self.set_ok(format!(
                    "Saved {}/autoexec.cfg",
                    self.controls_game
                ));
            }
            Err(e) => self.set_err(e),
        }
    }

    fn apply_controls_to_all_mods(&mut self) {
        let Some(dir) = self.config.game_dir() else {
            self.set_err("Set the Kingpin folder under Patches first.");
            return;
        };
        if self.controls_text.trim().is_empty() {
            self.set_err("autoexec.cfg is empty — nothing to apply.");
            return;
        }
        match autoexec::apply_to_all_mods(&dir, &self.controls_text) {
            Ok(result) => {
                self.controls_dirty = false;
                if !result.errors.is_empty() {
                    let preview = result.errors.into_iter().take(2).collect::<Vec<_>>().join("; ");
                    self.set_err(format!(
                        "Merged into {} folders; some failed ({preview}). Try running as Administrator.",
                        result.written
                    ));
                    return;
                }
                let skip = if result.skipped_binds > 0 {
                    format!(
                        " Skipped {} mod-specific key conflict(s).",
                        result.skipped_binds
                    )
                } else {
                    String::new()
                };
                self.set_ok(format!(
                    "Merged shared binds/display into {} mod folders without overwriting mod-specific lines.{}",
                    result.written, skip
                ));
            }
            Err(e) => self.set_err(e),
        }
    }

    fn ensure_controls_loaded(&mut self) {
        let Some(dir) = self.config.game_dir() else {
            return;
        };
        let key = format!("{}::{}", dir.display(), self.controls_game);
        if self.controls_loaded_for.as_deref() != Some(key.as_str()) {
            self.load_controls();
        }
    }

    fn start_mods_catalog(&mut self) {
        if self.mods_busy {
            return;
        }
        self.mods_busy = true;
        self.mods_phase = "Loading mod list from kingpin.info…".into();
        let (tx, rx) = mpsc::channel();
        self.mods_rx = Some(rx);
        mods::fetch_catalog_async(self.config.game_dir(), tx);
    }

    fn start_mod_install(&mut self, remote: RemoteMod) {
        if self.mods_busy {
            return;
        }
        let Some(dir) = self.config.game_dir() else {
            self.set_err("Set the game folder in Patches first.");
            self.tab = Tab::Patches;
            return;
        };
        self.mods_busy = true;
        self.installing_mod = Some(remote.name.clone());
        self.mods_phase = format!("Installing {}…", remote.name);
        let (tx, rx) = mpsc::channel();
        self.mods_rx = Some(rx);
        mods::install_mod_async(dir, remote, tx);
    }

    fn poll_mods_job(&mut self, ctx: &egui::Context) {
        let Some(rx) = self.mods_rx.as_ref() else {
            return;
        };
        let mut catalog = None;
        let mut install = None;
        while let Ok(ev) = rx.try_recv() {
            match ev {
                ModsEvent::Progress { phase, done, total } => {
                    if total > 0 {
                        self.mods_phase = format!(
                            "{phase} ({:.0}%)",
                            (done as f64 / total as f64) * 100.0
                        );
                    } else {
                        self.mods_phase = phase;
                    }
                }
                ModsEvent::Catalog(result) => catalog = Some(result),
                ModsEvent::InstallFinished { mod_name, result } => {
                    install = Some((mod_name, result));
                }
            }
        }
        if let Some(result) = catalog {
            self.mods_busy = false;
            self.mods_rx = None;
            self.mods_loaded = true;
            match result {
                Ok(list) => {
                    let n = list.len();
                    let with_client = list.iter().filter(|m| m.needs_client()).count();
                    self.remote_mods = list;
                    self.set_ok(format!("Loaded {n} mods ({with_client} with client packs)."));
                }
                Err(e) => self.set_err(e),
            }
        }
        if let Some((name, result)) = install {
            self.mods_busy = false;
            self.mods_rx = None;
            self.installing_mod = None;
            match result {
                Ok(msg) => {
                    if let Some(m) = self.remote_mods.iter_mut().find(|m| m.name == name) {
                        m.installed = true;
                    }
                    // Refresh installed flags from disk
                    if let Some(dir) = self.config.game_dir() {
                        for m in &mut self.remote_mods {
                            m.installed = mods::is_mod_installed(&dir, &m.name);
                        }
                    }
                    self.set_ok(msg);
                }
                Err(e) => self.set_err(e),
            }
        } else if self.mods_busy {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
    }

    fn scan_and_write_content_marker(&mut self) {
        let Some(dir) = self.config.game_dir() else {
            self.set_err("Set the Kingpin folder under Patches first.");
            return;
        };
        match addons::sync_marker_from_disk(&dir) {
            Ok(marker) => {
                self.refresh();
                let det = marker.detected.as_ref();
                self.set_ok(format!(
                    "Wrote main/kingpin-launcher-content.json (maps: {}, models: {}).",
                    det.map(|d| d.map_files).unwrap_or(0),
                    det.map(|d| d.player_model_dirs).unwrap_or(0)
                ));
            }
            Err(e) => self.set_err(e),
        }
    }

    fn start_kpaddons_install(&mut self) {
        if self.addon_busy {
            return;
        }
        let Some(dir) = self.config.game_dir() else {
            self.set_err("Set the game folder first.");
            return;
        };
        if addons::find_7z().is_none() {
            self.set_err(
                "7-Zip not found. Install it from https://www.7-zip.org/ then try again.",
            );
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.addon_rx = Some(rx);
        self.addon_busy = true;
        self.addon_phase = "Starting…".into();
        self.addon_done = 0;
        self.addon_total = addons::CONTENT_EXPECTED_BYTES;
        self.set_ok(format!(
            "Installing {} kingpin.info packs (maps + models)…",
            addons::pack_count()
        ));
        addons::install_async(dir, tx);
    }

    fn poll_addon_job(&mut self, ctx: &egui::Context) {
        let Some(rx) = self.addon_rx.as_ref() else {
            return;
        };
        let mut finished = None;
        while let Ok(ev) = rx.try_recv() {
            match ev {
                AddonEvent::Progress { phase, done, total } => {
                    self.addon_phase = phase;
                    self.addon_done = done;
                    self.addon_total = total;
                }
                AddonEvent::Finished(result) => {
                    finished = Some(result);
                }
            }
        }
        if let Some(result) = finished {
            self.addon_busy = false;
            self.addon_rx = None;
            match result {
                Ok(msg) => {
                    self.refresh();
                    self.set_ok(msg);
                }
                Err(e) => self.set_err(e),
            }
        } else if self.addon_busy {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
    }

    fn refresh(&mut self) {
        self.checklist = Checklist::evaluate(self.config.game_dir().as_deref());
    }

    fn set_ok(&mut self, msg: impl Into<String>) {
        self.status_message = msg.into();
        self.status_is_error = false;
    }

    fn set_err(&mut self, msg: impl Into<String>) {
        self.status_message = msg.into();
        self.status_is_error = true;
    }

    fn pick_game_folder(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Select Kingpin folder (contains kingpin.exe)")
            .pick_folder()
        {
            if is_kingpin_dir(&path) {
                self.config.set_game_dir(path);
                let _ = self.config.save();
                self.refresh();
                self.set_ok("Game folder set.");
            } else {
                self.set_err("That folder has no kingpin.exe.");
            }
        }
    }

    fn install_mh(&mut self) {
        let Some(dir) = self.config.game_dir() else {
            self.set_err("Set the game folder first.");
            return;
        };
        self.busy = true;
        match wizard::install_mh_patch(&dir) {
            Ok(msg) => {
                self.refresh();
                self.set_ok(msg);
            }
            Err(e) => self.set_err(e),
        }
        self.busy = false;
    }

    fn install_mbrowser_file(&mut self) {
        let Some(dir) = self.config.game_dir() else {
            self.set_err("Set the game folder first.");
            return;
        };
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Select Kingpin_M_Browser.exe or its zip")
            .add_filter("M-Browser", &["exe", "zip"])
            .pick_file()
        {
            match wizard::install_mbrowser_from_path(&dir, &path) {
                Ok(msg) => {
                    self.refresh();
                    self.set_ok(msg);
                }
                Err(e) => self.set_err(e),
            }
        }
    }
}

impl KingpinLauncherApp {
    /// Windows maximize bypasses max_inner_size — snap back if we ever exceed bounds.
    /// Clamp the native window to [`crate::WIN_MIN`] / [`crate::WIN_MAX`] each frame.
    fn enforce_window_bounds(ctx: &egui::Context) {
        use crate::{WIN_MAX, WIN_MIN};

        let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        if maximized {
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                WIN_MAX[0],
                WIN_MAX[1],
            )));
            return;
        }

        if let Some(size) = ctx.input(|i| i.viewport().inner_rect.map(|r| r.size())) {
            let w = size.x.clamp(WIN_MIN[0], WIN_MAX[0]);
            let h = size.y.clamp(WIN_MIN[1], WIN_MAX[1]);
            if (w - size.x).abs() > 0.5 || (h - size.y).abs() > 0.5 {
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(w, h)));
            }
        }
    }
}

impl eframe::App for KingpinLauncherApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        Self::enforce_window_bounds(ctx);
        self.poll_server_refresh(ctx);
        self.poll_addon_job(ctx);
        self.poll_mods_job(ctx);

        if self.tab == Tab::Browser
            && self.auto_refresh
            && !self.busy
            && self
                .last_refresh
                .is_some_and(|t| t.elapsed() >= Duration::from_secs(30))
        {
            self.start_server_refresh();
        }

        theme::paint_brick_backdrop(ctx);

        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add(
                    egui::Image::new(egui::include_image!("../assets/kingpin_logo.png"))
                        .fit_to_exact_size(egui::vec2(280.0, 72.0))
                        .maintain_aspect_ratio(true),
                );
                ui.add_space(12.0);
                ui.with_layout(Layout::left_to_right(Align::BOTTOM), |ui| {
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("LAUNCHER")
                            .color(Color32::from_rgb(160, 150, 130))
                            .size(14.0),
                    );
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .button(RichText::new("Discord").color(theme::yellow()))
                        .on_hover_text(actions::DISCORD_URL)
                        .clicked()
                    {
                        match actions::open_discord() {
                            Ok(()) => self.set_ok("Opened Discord invite."),
                            Err(e) => self.set_err(e),
                        }
                    }
                });
            });
            ui.add_space(4.0);
            ui.separator();
        });

        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.add_space(4.0);
            let ready = self.checklist.is_ready();
            let label = self.checklist.status_label();
            let color = if ready {
                theme::ready_green()
            } else {
                theme::yellow()
            };
            ui.horizontal(|ui| {
                ui.label(RichText::new(label).color(color).strong());
                if let Some(dir) = &self.checklist.game_dir {
                    ui.label(
                        RichText::new(shorten_path(dir, 56))
                            .color(Color32::from_rgb(120, 110, 95))
                            .small(),
                    );
                }
            });
            if !self.status_message.is_empty() {
                let c = if self.status_is_error {
                    theme::blood()
                } else {
                    Color32::from_rgb(180, 170, 150)
                };
                ui.label(RichText::new(&self.status_message).color(c));
            }
            ui.add_space(4.0);
        });

        egui::SidePanel::left("nav")
            .resizable(false)
            .exact_width(140.0)
            .show(ctx, |ui| {
                ui.add_space(12.0);
                for tab in [Tab::Play, Tab::Browser, Tab::Mods, Tab::Controls, Tab::Patches] {
                    let selected = self.tab == tab;
                    let text = if selected {
                        RichText::new(tab.label()).color(theme::yellow()).strong()
                    } else {
                        RichText::new(tab.label()).color(Color32::from_rgb(200, 190, 170))
                    };
                    if ui
                        .add_sized([120.0, 36.0], egui::SelectableLabel::new(selected, text))
                        .clicked()
                    {
                        self.tab = tab;
                        self.config.last_tab = Some(tab.as_str().into());
                        let _ = self.config.save();
                        if tab == Tab::Browser && !self.servers_loaded && !self.busy {
                            self.start_server_refresh();
                        }
                        if tab == Tab::Mods && !self.mods_loaded && !self.mods_busy {
                            self.start_mods_catalog();
                        }
                        if tab == Tab::Controls {
                            self.ensure_controls_loaded();
                        }
                    }
                    ui.add_space(4.0);
                }
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(8.0);
            match self.tab {
                Tab::Play => self.ui_play(ui),
                Tab::Browser => self.ui_browser(ui),
                Tab::Mods => self.ui_mods(ui),
                Tab::Controls => self.ui_controls(ui),
                Tab::Patches => self.ui_patches(ui),
            }
        });
    }
}

impl KingpinLauncherApp {
    fn ui_play(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new("Single Player")
                .size(22.0)
                .color(theme::yellow()),
        );
        ui.label("Launch Kingpin directly into the campaign / local game.");
        ui.add_space(16.0);

        let enabled = self.checklist.kingpin_exe == ItemStatus::Ok && !self.busy;
        if ui
            .add_enabled(
                enabled,
                egui::Button::new(RichText::new("  PLAY SINGLE PLAYER  ").size(18.0).strong())
                    .fill(theme::blood())
                    .min_size(egui::vec2(260.0, 48.0)),
            )
            .clicked()
        {
            if let Some(dir) = self.config.game_dir() {
                match actions::launch_single_player(&dir) {
                    Ok(()) => self.set_ok("Started kingpin.exe"),
                    Err(e) => {
                        self.set_err(e);
                        self.tab = Tab::Patches;
                    }
                }
            } else {
                self.set_err("No game folder — open Patches.");
                self.tab = Tab::Patches;
            }
        }

        if self.checklist.kingpin_exe != ItemStatus::Ok {
            ui.add_space(12.0);
            ui.label(
                RichText::new("kingpin.exe missing. Use Patches to locate the install.")
                    .color(theme::yellow()),
            );
        }
    }

    /// Kick off a background mirror fetch + UDP status sweep (no-op if one is already running).
    fn start_server_refresh(&mut self) {
        if self.refresh_rx.is_some() {
            return;
        }
        self.busy = true;
        self.set_ok("Querying servers (UDP status)…");
        let (tx, rx) = mpsc::channel();
        self.refresh_rx = Some(rx);
        query::refresh_live_async(tx);
    }

    fn poll_server_refresh(&mut self, ctx: &egui::Context) {
        let Some(rx) = self.refresh_rx.as_ref() else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(list)) => {
                let online = list.iter().filter(|s| s.online).count();
                let total = list.len();
                self.servers = list;
                self.servers_loaded = true;
                self.busy = false;
                self.refresh_rx = None;
                self.last_refresh = Some(Instant::now());
                self.set_ok(format!("Queried {total} servers — {online} online."));
            }
            Ok(Err(e)) => {
                self.servers_loaded = true;
                self.busy = false;
                self.refresh_rx = None;
                self.set_err(e);
            }
            Err(mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.busy = false;
                self.refresh_rx = None;
                self.set_err("Server query thread ended unexpectedly.");
            }
        }
    }

    fn filtered_sorted_servers(&self) -> Vec<&GameServer> {
        let q = self.server_search.trim().to_ascii_lowercase();
        let mut list: Vec<&GameServer> = self
            .servers
            .iter()
            .filter(|s| match self.server_filter {
                BrowserFilter::All => true,
                BrowserFilter::Kingpin => s.kind != ServerKind::Quake2,
            })
            .filter(|s| !self.online_only || s.online)
            .filter(|s| !self.hide_empty || !s.is_empty())
            .filter(|s| !self.hide_full || !s.is_full())
            .filter(|s| {
                if q.is_empty() {
                    return true;
                }
                s.name.to_ascii_lowercase().contains(&q)
                    || s.map.to_ascii_lowercase().contains(&q)
                    || s.game.to_ascii_lowercase().contains(&q)
                    || s.address.to_ascii_lowercase().contains(&q)
            })
            .collect();

        list.sort_by(|a, b| {
            let ord = match self.sort_col {
                SortCol::Ping => a
                    .ping_ms
                    .unwrap_or(u32::MAX)
                    .cmp(&b.ping_ms.unwrap_or(u32::MAX)),
                SortCol::Players => a.players.cmp(&b.players),
                SortCol::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                SortCol::Map => a.map.to_lowercase().cmp(&b.map.to_lowercase()),
                SortCol::Game => a.game.to_lowercase().cmp(&b.game.to_lowercase()),
                SortCol::Address => a.address.cmp(&b.address),
            };
            if self.sort_asc {
                ord
            } else {
                ord.reverse()
            }
        });
        list
    }

    fn toggle_sort(&mut self, col: SortCol) {
        if self.sort_col == col {
            self.sort_asc = !self.sort_asc;
        } else {
            self.sort_col = col;
            self.sort_asc = col == SortCol::Ping || col == SortCol::Name;
        }
    }

    fn ui_browser(&mut self, ui: &mut egui::Ui) {
        let stroke = egui::Stroke::new(1.0_f32, Color32::from_rgb(70, 58, 46));
        let mut do_join = false;
        let mut do_update = false;

        // Title + actions
        egui::Frame::NONE
            .fill(theme::ase_panel())
            .stroke(stroke)
            .inner_margin(egui::Margin::symmetric(8, 6))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Server browser")
                            .strong()
                            .size(18.0)
                            .color(theme::yellow()),
                    );
                    if self.busy {
                        ui.spinner();
                        ui.label(RichText::new("Updating…").color(theme::yellow()));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let can_join = self.checklist.kingpin_exe == ItemStatus::Ok
                            && self
                                .selected_address
                                .as_ref()
                                .and_then(|a| self.servers.iter().find(|s| &s.address == a))
                                .is_some_and(|s| s.online)
                            && !self.busy;
                        if ui
                            .add_enabled(
                                can_join,
                                egui::Button::new(RichText::new("Join server").strong())
                                    .fill(theme::blood()),
                            )
                            .on_hover_text("Double-click a row to join")
                            .clicked()
                        {
                            do_join = true;
                        }
                        if ui
                            .add_enabled(!self.busy, egui::Button::new("Update"))
                            .clicked()
                        {
                            do_update = true;
                        }
                        ui.hyperlink_to("Web list", servers::SERVERS_PAGE_URL);
                        ui.checkbox(&mut self.auto_refresh, "Auto");
                    });
                });

                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Filter").color(theme::yellow()));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.server_search)
                            .desired_width(200.0)
                            .hint_text("name / map / game / ip"),
                    );
                    ui.separator();
                    ui.selectable_value(&mut self.server_filter, BrowserFilter::Kingpin, "Kingpin");
                    ui.selectable_value(&mut self.server_filter, BrowserFilter::All, "All");
                    ui.separator();
                    ui.checkbox(&mut self.online_only, "Hide offline");
                    ui.checkbox(&mut self.hide_empty, "Hide empty");
                    ui.checkbox(&mut self.hide_full, "Hide full");
                });
            });

        if do_update {
            self.start_server_refresh();
        }
        if do_join {
            self.join_selected();
        }

        ui.add_space(4.0);

        let list: Vec<GameServer> = self
            .filtered_sorted_servers()
            .into_iter()
            .cloned()
            .collect();
        let shown = list.len();
        let online_n = list.iter().filter(|s| s.online).count();
        let players_n: u32 = list.iter().map(|s| s.players).sum();

        let available = ui.available_height();
        let status_h = 28.0;
        let detail_h = (available * 0.32).clamp(160.0, 280.0);
        let list_h = (available - detail_h - status_h - 8.0).max(160.0);
        let row_h = 28.0;

        let mut join_addr: Option<String> = None;
        let mut select_addr: Option<String> = None;
        let mut sort_click: Option<SortCol> = None;

        // Server list (ASE main pane)
        egui::Frame::NONE
            .fill(theme::ase_panel())
            .stroke(stroke)
            .inner_margin(egui::Margin::symmetric(4, 4))
            .show(ui, |ui| {
                ui.allocate_ui(egui::vec2(ui.available_width(), list_h), |ui| {
                    let table = TableBuilder::new(ui)
                        .striped(true)
                        .resizable(true)
                        .sense(egui::Sense::click())
                        .cell_layout(Layout::left_to_right(Align::Center))
                        .column(Column::exact(56.0)) // Ping
                        .column(Column::exact(70.0)) // Players
                        .column(Column::remainder().at_least(180.0)) // Server
                        .column(Column::initial(140.0).at_least(90.0).clip(true)) // Map
                        .column(Column::initial(130.0).at_least(80.0).clip(true)) // Game
                        .column(Column::initial(160.0).at_least(120.0).clip(true)) // IP
                        .min_scrolled_height(list_h - 32.0)
                        .max_scroll_height(list_h - 32.0);

                    table
                        .header(26.0, |mut header| {
                            for (col, label) in [
                                (SortCol::Ping, "Ping"),
                                (SortCol::Players, "Players"),
                                (SortCol::Name, "Server Name"),
                                (SortCol::Map, "Map"),
                                (SortCol::Game, "Game"),
                                (SortCol::Address, "IP Address"),
                            ] {
                                header.col(|ui| {
                                    let mark = if self.sort_col == col {
                                        if self.sort_asc {
                                            " ▲"
                                        } else {
                                            " ▼"
                                        }
                                    } else {
                                        ""
                                    };
                                    if ui
                                        .add(
                                            egui::Label::new(
                                                RichText::new(format!("{label}{mark}"))
                                                    .strong()
                                                    .color(theme::yellow()),
                                            )
                                            .sense(egui::Sense::click()),
                                        )
                                        .clicked()
                                    {
                                        sort_click = Some(col);
                                    }
                                });
                            }
                        })
                        .body(|mut body| {
                            for server in &list {
                                let selected =
                                    self.selected_address.as_deref() == Some(&server.address);
                                let ping_color =
                                    theme::ase_ping_color(server.online, server.ping_ms);
                                let addr = server.address.clone();

                                body.row(row_h, |mut row| {
                                    // Full-row highlight (not per-cell selectable_label bars).
                                    row.set_selected(selected);
                                    let name_color = if selected {
                                        theme::yellow()
                                    } else if server.online {
                                        Color32::from_rgb(230, 220, 200)
                                    } else {
                                        Color32::from_rgb(120, 110, 95)
                                    };

                                    row.col(|ui| {
                                        ui.label(
                                            RichText::new(server.ping_label())
                                                .monospace()
                                                .color(ping_color),
                                        );
                                    });
                                    row.col(|ui| {
                                        ui.label(
                                            RichText::new(server.players_label()).monospace(),
                                        );
                                    });
                                    row.col(|ui| {
                                        ui.label(RichText::new(&server.name).color(name_color));
                                    });
                                    row.col(|ui| {
                                        ui.label(&server.map);
                                    });
                                    row.col(|ui| {
                                        let game = if server.game.is_empty() {
                                            "—"
                                        } else {
                                            server.game.as_str()
                                        };
                                        ui.label(game);
                                    });
                                    row.col(|ui| {
                                        ui.label(
                                            RichText::new(&server.address)
                                                .monospace()
                                                .color(Color32::from_rgb(140, 130, 115)),
                                        );
                                    });

                                    let resp = row.response();
                                    if resp.clicked() {
                                        select_addr = Some(addr.clone());
                                    }
                                    if resp.double_clicked() {
                                        join_addr = Some(addr);
                                    }
                                });
                            }
                        });
                });
            });

        if let Some(col) = sort_click {
            self.toggle_sort(col);
        }
        if let Some(addr) = select_addr {
            self.selected_address = Some(addr);
        }
        if let Some(addr) = join_addr {
            self.selected_address = Some(addr);
            self.join_selected();
        }

        ui.add_space(4.0);

        // ASE bottom: Player List | Server Info
        let selected = self
            .selected_address
            .as_ref()
            .and_then(|a| list.iter().find(|s| &s.address == a).cloned())
            .or_else(|| {
                self.selected_address
                    .as_ref()
                    .and_then(|a| self.servers.iter().find(|s| &s.address == a).cloned())
            });

        ui.allocate_ui(egui::vec2(ui.available_width(), detail_h), |ui| {
            ui.columns(2, |cols| {
                egui::Frame::NONE
                    .fill(theme::ase_panel())
                    .stroke(stroke)
                    .inner_margin(egui::Margin::symmetric(6, 4))
                    .show(&mut cols[0], |ui| {
                        ui.label(
                            RichText::new("PLAYER LIST")
                                .strong()
                                .color(theme::yellow()),
                        );
                        ui.separator();
                        egui::ScrollArea::vertical()
                            .id_salt("ase_players")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                match &selected {
                                    Some(server) if !server.player_rows.is_empty() => {
                                        egui::Grid::new("ase_players_grid")
                                            .num_columns(3)
                                            .striped(true)
                                            .spacing([10.0, 2.0])
                                            .show(ui, |ui| {
                                                ui.label(
                                                    RichText::new("Score")
                                                        .strong()
                                                        .color(Color32::from_rgb(160, 150, 130)),
                                                );
                                                ui.label(
                                                    RichText::new("Ping")
                                                        .strong()
                                                        .color(Color32::from_rgb(160, 150, 130)),
                                                );
                                                ui.label(
                                                    RichText::new("Name")
                                                        .strong()
                                                        .color(Color32::from_rgb(160, 150, 130)),
                                                );
                                                ui.end_row();
                                                for p in &server.player_rows {
                                                    ui.label(
                                                        RichText::new(p.score.to_string())
                                                            .monospace(),
                                                    );
                                                    ui.label(
                                                        RichText::new(p.ping.to_string())
                                                            .monospace(),
                                                    );
                                                    ui.label(&p.name);
                                                    ui.end_row();
                                                }
                                            });
                                    }
                                    Some(server) if server.online => {
                                        ui.label(
                                            RichText::new("No player names in status response.")
                                                .color(Color32::from_rgb(140, 128, 110)),
                                        );
                                    }
                                    Some(_) => {
                                        ui.label(
                                            RichText::new("Server not responding.")
                                                .color(theme::blood()),
                                        );
                                    }
                                    None => {
                                        ui.label(
                                            RichText::new("Select a server.")
                                                .color(Color32::from_rgb(140, 128, 110)),
                                        );
                                    }
                                }
                            });
                    });

                egui::Frame::NONE
                    .fill(theme::ase_panel())
                    .stroke(stroke)
                    .inner_margin(egui::Margin::symmetric(6, 4))
                    .show(&mut cols[1], |ui| {
                        ui.label(
                            RichText::new("SERVER INFO")
                                .strong()
                                .color(theme::yellow()),
                        );
                        ui.separator();
                        egui::ScrollArea::vertical()
                            .id_salt("ase_rules")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                match &selected {
                                    Some(server) => {
                                        ui.label(
                                            RichText::new(&server.name)
                                                .strong()
                                                .color(theme::yellow()),
                                        );
                                        ui.label(
                                            RichText::new(&server.address)
                                                .monospace()
                                                .color(Color32::from_rgb(140, 130, 115)),
                                        );
                                        ui.label(format!(
                                            "Map: {}   Game: {}",
                                            if server.map.is_empty() {
                                                "—"
                                            } else {
                                                &server.map
                                            },
                                            if server.game.is_empty() {
                                                "—"
                                            } else {
                                                &server.game
                                            },
                                        ));
                                        ui.add_space(4.0);
                                        if server.rules.is_empty() {
                                            ui.label(
                                                RichText::new("(no cvars)")
                                                    .color(Color32::from_rgb(140, 128, 110)),
                                            );
                                        } else {
                                            egui::Grid::new("ase_rules_grid")
                                                .num_columns(2)
                                                .striped(true)
                                                .spacing([8.0, 2.0])
                                                .show(ui, |ui| {
                                                    for (k, v) in &server.rules {
                                                        ui.label(
                                                            RichText::new(k).color(
                                                                Color32::from_rgb(160, 150, 130),
                                                            ),
                                                        );
                                                        ui.label(v);
                                                        ui.end_row();
                                                    }
                                                });
                                        }
                                    }
                                    None => {
                                        ui.label(
                                            RichText::new("Select a server.")
                                                .color(Color32::from_rgb(140, 128, 110)),
                                        );
                                    }
                                }
                            });
                    });
            });
        });

        // Status strip
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!(
                    "{shown} servers  ·  {online_n} online  ·  {players_n} players"
                ))
                .color(Color32::from_rgb(160, 150, 130)),
            );
            if self.servers_loaded && shown == 0 && !self.busy {
                ui.label(
                    RichText::new("— no matches (clear filter / show offline)")
                        .color(theme::yellow()),
                );
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("M-Browser").clicked() {
                    self.launch_mbrowser();
                }
            });
        });
    }

    /// `+connect` the highlighted Browser row (with `+set game` when a mod folder is required).
    fn join_selected(&mut self) {
        let Some(address) = self.selected_address.clone() else {
            self.set_err("Select a server first.");
            return;
        };
        let server = self.servers.iter().find(|s| s.address == address);
        let name = server
            .map(|s| s.name.clone())
            .unwrap_or_else(|| address.clone());
        let game = server.map(|s| s.game.as_str()).unwrap_or("");
        let mod_dir = actions::normalize_game_dir(game);
        let Some(dir) = self.config.game_dir() else {
            self.set_err("No game folder — open Patches.");
            self.tab = Tab::Patches;
            return;
        };
        match actions::join_server(&dir, &address, game) {
            Ok(()) => {
                let mod_note = if mod_dir.eq_ignore_ascii_case("main") {
                    String::new()
                } else {
                    format!(" [{mod_dir}]")
                };
                self.set_ok(format!("Joining {name}{mod_note} ({address})…"));
            }
            Err(e) => {
                let missing_mod = e.contains("Missing mod folder");
                self.set_err(e);
                if missing_mod {
                    self.tab = Tab::Mods;
                } else {
                    self.tab = Tab::Patches;
                }
            }
        }
    }

    fn launch_mbrowser(&mut self) {
        let Some(dir) = self.config.game_dir() else {
            self.set_err("No game folder — open Patches.");
            self.tab = Tab::Patches;
            return;
        };
        match actions::launch_mbrowser(&dir) {
            Ok(()) => self.set_ok("Started M-Browser"),
            Err(e) => {
                self.set_err(e);
                self.tab = Tab::Patches;
            }
        }
    }

    fn ui_mods(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new("Multiplayer mods")
                .size(22.0)
                .color(theme::yellow()),
        );
        ui.label(
            "Client packs from kingpin.info/server/mods — install only what a server needs beyond the maps/models pack.",
        );
        ui.add_space(6.0);

        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(!self.mods_busy, egui::Button::new("Refresh list"))
                .clicked()
            {
                self.start_mods_catalog();
            }
            ui.selectable_value(&mut self.mods_filter, ModsFilter::NeedsClient, "Has client files");
            ui.selectable_value(&mut self.mods_filter, ModsFilter::Missing, "Missing");
            ui.selectable_value(&mut self.mods_filter, ModsFilter::Installed, "Installed");
            ui.selectable_value(&mut self.mods_filter, ModsFilter::All, "All");
            ui.hyperlink_to("Browse on kingpin.info", mods::MODS_PAGE);
        });

        if self.mods_busy {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new(&self.mods_phase).color(theme::yellow()));
            });
        }

        ui.add_space(6.0);
        let rows: Vec<RemoteMod> = self
            .remote_mods
            .iter()
            .filter(|m| match self.mods_filter {
                ModsFilter::All => true,
                ModsFilter::NeedsClient => m.needs_client(),
                ModsFilter::Missing => m.needs_client() && !m.installed,
                ModsFilter::Installed => m.installed,
            })
            .cloned()
            .collect();

        let mut install_target: Option<RemoteMod> = None;

        let pack_labels: Vec<String> = rows
            .iter()
            .map(|m| {
                if m.client_archives.is_empty() {
                    if m.needs_client() {
                        "(folder empty)".into()
                    } else {
                        "—".into()
                    }
                } else {
                    m.client_archives.join(", ")
                }
            })
            .collect();

        let (mod_w, packs_w, status_w) = ui.fonts(|fonts| {
            let body = TextStyle::Body.resolve(ui.style());
            let small = TextStyle::Small.resolve(ui.style());
            let strong = TextStyle::Body.resolve(ui.style());
            let measure = |text: &str, font: &egui::FontId| {
                fonts
                    .layout_no_wrap(text.to_owned(), font.clone(), Color32::WHITE)
                    .size()
                    .x
            };

            let mut mod_w = measure("Mod", &strong) + 16.0;
            let mut packs_w = measure("Client packs", &strong) + 16.0;
            let mut status_w = measure("Status", &strong) + 16.0;

            for (m, packs) in rows.iter().zip(pack_labels.iter()) {
                mod_w = mod_w.max(measure(&m.name, &body) + 12.0);
                packs_w = packs_w.max(measure(packs, &small) + 12.0);
                status_w = status_w.max(measure(m.status_label(), &body) + 12.0);
            }

            (mod_w.ceil(), packs_w.ceil(), status_w.ceil())
        });

        egui::ScrollArea::vertical()
            .id_salt("mods_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                TableBuilder::new(ui)
                    .striped(true)
                    .resizable(true)
                    .cell_layout(Layout::left_to_right(Align::Center))
                    .column(Column::exact(mod_w))
                    .column(Column::exact(packs_w))
                    .column(Column::exact(status_w))
                    .column(Column::exact(72.0))
                    .column(Column::remainder())
                    .header(24.0, |mut header| {
                        for label in ["Mod", "Client packs", "Status", "", ""] {
                            header.col(|ui| {
                                ui.label(
                                    RichText::new(label).strong().color(theme::yellow()),
                                );
                            });
                        }
                    })
                    .body(|mut body| {
                        for (m, packs) in rows.iter().zip(pack_labels.iter()) {
                            let can_install = m.needs_client()
                                && !m.installed
                                && !m.client_archives.is_empty()
                                && !self.mods_busy
                                && self.checklist.kingpin_exe == ItemStatus::Ok;
                            let status_color = if m.installed {
                                theme::ready_green()
                            } else if m.needs_client() {
                                theme::yellow()
                            } else {
                                Color32::from_rgb(120, 110, 95)
                            };

                            body.row(24.0, |mut row| {
                                row.col(|ui| {
                                    ui.label(RichText::new(&m.name).strong());
                                });
                                row.col(|ui| {
                                    ui.label(RichText::new(packs).small());
                                });
                                row.col(|ui| {
                                    ui.label(
                                        RichText::new(m.status_label()).color(status_color),
                                    );
                                });
                                row.col(|ui| {
                                    if m.installed {
                                        ui.label(RichText::new("OK").color(theme::ready_green()));
                                    } else if ui
                                        .add_enabled(can_install, egui::Button::new("Install").small())
                                        .clicked()
                                    {
                                        install_target = Some(m.clone());
                                    }
                                });
                                row.col(|_ui| {});
                            });
                        }
                    });
            });

        if let Some(remote) = install_target {
            self.start_mod_install(remote);
        }

        if self.mods_loaded && rows.is_empty() && !self.mods_busy {
            ui.label("No mods match this filter.");
        }
        if self.checklist.kingpin_exe != ItemStatus::Ok {
            ui.label(
                RichText::new("Set the Kingpin folder under Patches before installing mods.")
                    .color(theme::yellow()),
            );
        }
    }

    fn ui_controls(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new("Controls (autoexec.cfg)")
                .size(22.0)
                .color(theme::yellow()),
        );
        ui.label(
            "Edits autoexec.cfg so binds apply every launch (overrides config.cfg). Defaults to main/.",
        );
        ui.add_space(8.0);

        if self.config.game_dir().is_none() {
            ui.label(
                RichText::new("Set the Kingpin folder under Patches first.")
                    .color(theme::yellow()),
            );
            return;
        }

        self.ensure_controls_loaded();

        let cfg_path = self
            .config
            .game_dir()
            .map(|d| autoexec::autoexec_path(&d, &self.controls_game));
        if let Some(path) = &cfg_path {
            ui.label(
                RichText::new(path.display().to_string())
                    .small()
                    .color(Color32::from_rgb(160, 150, 130)),
            );
            ui.add_space(4.0);
        }

        let games = self
            .config
            .game_dir()
            .map(|d| autoexec::list_mod_dirs(&d))
            .unwrap_or_default();

        ui.horizontal_wrapped(|ui| {
            ui.label("Game:");
            egui::ComboBox::from_id_salt("controls_game")
                .selected_text(&self.controls_game)
                .show_ui(ui, |ui| {
                    for g in &games {
                        if ui
                            .selectable_label(self.controls_game == *g, g)
                            .clicked()
                        {
                            if self.controls_dirty {
                                self.set_err("Save or reload before switching game folders.");
                            } else {
                                self.controls_game = g.clone();
                                self.load_controls();
                            }
                        }
                    }
                });

            if ui.button("Reload").clicked() {
                self.load_controls();
                self.set_ok("Reloaded autoexec.cfg");
            }
            if ui
                .add_enabled(self.controls_dirty, egui::Button::new("Save"))
                .clicked()
            {
                self.save_controls();
            }
            if ui.button("Modern WASD preset").clicked() {
                self.controls_text = autoexec::apply_modern_wasd(&self.controls_text);
                self.controls_dirty = true;
                self.set_ok("Applied modern WASD preset — click Save to write the file.");
            }
            if ui
                .button("Apply to all mods")
                .on_hover_text(
                    "Merges shared binds + display cvars into every mod folder. Keeps exec lines and mod-specific binds (CTF, Crash, etc.).",
                )
                .clicked()
            {
                self.apply_controls_to_all_mods();
            }
            if let Some(path) = &cfg_path {
                if ui
                    .button("Open folder")
                    .on_hover_text(path.display().to_string())
                    .clicked()
                {
                    if let Some(parent) = path.parent() {
                        let _ = open::that(parent);
                    }
                }
            }
            ui.checkbox(&mut self.controls_show_raw, "Raw editor");
        });

        if self.controls_dirty {
            ui.label(
                RichText::new("Unsaved changes")
                    .color(theme::yellow())
                    .small(),
            );
        }

        ui.add_space(8.0);
        ui.label(RichText::new("Display").strong().color(theme::yellow()));
        ui.horizontal_wrapped(|ui| {
            let mut fps_on = autoexec::showfps_enabled(&self.controls_text);
            if ui.checkbox(&mut fps_on, "FPS meter (showfps)").changed() {
                self.controls_text = autoexec::set_cvar(
                    &self.controls_text,
                    "showfps",
                    if fps_on { "1" } else { "0" },
                );
                self.controls_dirty = true;
            }

            ui.add_space(12.0);
            ui.label("Resolution:");
            let current = autoexec::get_cvar(&self.controls_text, "vid_size")
                .unwrap_or_else(|| "1920x1080".into());
            let mut chosen = current.clone();
            egui::ComboBox::from_id_salt("controls_resolution")
                .selected_text(&chosen)
                .width(140.0)
                .show_ui(ui, |ui| {
                    let mut options: Vec<String> = autoexec::RESOLUTIONS
                        .iter()
                        .map(|s| (*s).to_string())
                        .collect();
                    if !options.iter().any(|r| r == &current) {
                        options.insert(0, current.clone());
                    }
                    for res in options {
                        if ui.selectable_label(chosen == res, &res).clicked() {
                            chosen = res;
                        }
                    }
                });
            if chosen != current {
                self.controls_text =
                    autoexec::set_cvar(&self.controls_text, "vid_size", &chosen);
                self.controls_dirty = true;
            }
        });
        ui.label(
            RichText::new("Writes set showfps / set vid_size into autoexec.cfg — Save to apply.")
                .small()
                .color(Color32::from_rgb(160, 150, 130)),
        );

        ui.add_space(6.0);

        if let Some(action) = self.controls_listening.clone() {
            let label = autoexec::CONTROL_ACTIONS
                .iter()
                .find(|(cmd, _)| *cmd == action)
                .map(|(_, l)| *l)
                .unwrap_or(action.as_str());
            ui.group(|ui| {
                ui.label(
                    RichText::new(format!("Press a key for: {label}"))
                        .color(theme::yellow())
                        .strong(),
                );
                ui.horizontal_wrapped(|ui| {
                    for key in [
                        "MOUSE1", "MOUSE2", "MOUSE3", "MWHEEL_U", "MWHEEL_D", "SPACE", "CTRL",
                        "SHIFT", "ALT", "TAB",
                    ] {
                        if ui.button(key).clicked() {
                            self.controls_text =
                                autoexec::set_bind(&self.controls_text, &action, key);
                            self.controls_dirty = true;
                            self.controls_listening = None;
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        self.controls_listening = None;
                    }
                });
            });

            let mut captured: Option<String> = None;
            ui.input(|input| {
                for event in &input.events {
                    match event {
                        Event::Key {
                            key,
                            pressed: true,
                            modifiers,
                            ..
                        } => {
                            if *key == egui::Key::Escape {
                                captured = Some(String::new());
                                continue;
                            }
                            if let Some(name) = autoexec::egui_key_to_kingpin(*key) {
                                captured = Some(name);
                            } else if let Some(mod_key) =
                                autoexec::egui_modifiers_to_kingpin(modifiers)
                            {
                                captured = Some(mod_key.into());
                            }
                        }
                        Event::MouseWheel {
                            unit: _,
                            delta,
                            modifiers: _,
                        } => {
                            if delta.y > 0.0 {
                                captured = Some("MWHEEL_U".into());
                            } else if delta.y < 0.0 {
                                captured = Some("MWHEEL_D".into());
                            }
                        }
                        _ => {}
                    }
                }
            });

            if let Some(key) = captured {
                if key.is_empty() {
                    self.controls_listening = None;
                } else {
                    self.controls_text = autoexec::set_bind(&self.controls_text, &action, &key);
                    self.controls_dirty = true;
                    self.controls_listening = None;
                }
            }
        }

        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .id_salt("controls_binds")
            .max_height(if self.controls_show_raw { 280.0 } else { 420.0 })
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("controls_grid")
                    .num_columns(3)
                    .spacing([16.0, 6.0])
                    .striped(true)
                    .show(ui, |ui| {
                        ui.label(RichText::new("Action").strong().color(theme::yellow()));
                        ui.label(RichText::new("Key").strong().color(theme::yellow()));
                        ui.label("");
                        ui.end_row();

                        for (cmd, label) in autoexec::CONTROL_ACTIONS {
                            let keys = autoexec::keys_for_action(&self.controls_text, cmd);
                            let key_label = if keys.is_empty() {
                                "—".to_string()
                            } else {
                                keys.join(", ")
                            };
                            ui.label(*label);
                            ui.label(
                                RichText::new(key_label).color(Color32::from_rgb(220, 210, 190)),
                            );
                            if ui
                                .add_enabled(
                                    self.controls_listening.is_none(),
                                    egui::Button::new("Change").small(),
                                )
                                .clicked()
                            {
                                self.controls_listening = Some((*cmd).to_string());
                            }
                            ui.end_row();
                        }
                    });
            });

        if self.controls_show_raw {
            ui.add_space(8.0);
            ui.label(
                RichText::new(format!("{}/autoexec.cfg", self.controls_game))
                    .color(Color32::from_rgb(160, 150, 130)),
            );
            let editor = egui::TextEdit::multiline(&mut self.controls_text)
                .code_editor()
                .desired_width(f32::INFINITY)
                .desired_rows(14);
            if ui.add(editor).changed() {
                self.controls_dirty = true;
            }
        }
    }

    fn ui_patches(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical()
            .id_salt("patches_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.ui_patches_inner(ui);
            });
    }

    fn ui_patches_inner(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new("Setup wizard")
                .size(22.0)
                .color(theme::yellow()),
        );
        ui.label("Get the install Ready: game → MH patch → maps/models. M-Browser is optional.");
        ui.add_space(10.0);

        ui.horizontal(|ui| {
            if ui.button("Browse game folder…").clicked() {
                self.pick_game_folder();
            }
            if ui.button("Refresh").clicked() {
                self.refresh();
                self.set_ok("Checklist refreshed.");
            }
            if ui.button("Open game folder").clicked() {
                if let Some(dir) = self.config.game_dir() {
                    match actions::open_game_folder(&dir) {
                        Ok(()) => self.set_ok("Opened game folder."),
                        Err(e) => self.set_err(e),
                    }
                } else {
                    self.set_err("No game folder set.");
                }
            }
        });

        ui.add_space(12.0);
        self.draw_checklist(ui);
        ui.add_space(14.0);

        let step = wizard::current_step(&self.checklist);
        ui.group(|ui| {
            ui.label(RichText::new(step.title()).strong().color(theme::yellow()));
            ui.label(step.detail());
            ui.add_space(8.0);

            match step {
                WizardStep::LocateGame | WizardStep::VerifyGame => {
                    if ui.button("Browse for kingpin.exe folder…").clicked() {
                        self.pick_game_folder();
                    }
                }
                WizardStep::InstallMhPatch => {
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(!self.busy, egui::Button::new("Download & install MH patch v12"))
                            .clicked()
                        {
                            self.install_mh();
                        }
                        if ui.button("Open MH patch page").clicked() {
                            let _ = wizard::open_mh_patch_page();
                        }
                        if ui
                            .add_enabled(self.checklist.mh_patchcfg == ItemStatus::Ok, egui::Button::new("Open Patch Config"))
                            .clicked()
                        {
                            if let Some(dir) = self.config.game_dir() {
                                match actions::launch_patchcfg(&dir) {
                                    Ok(()) => self.set_ok("Started patchcfg.exe"),
                                    Err(e) => self.set_err(e),
                                }
                            }
                        }
                    });
                }
                WizardStep::Ready => {
                    ui.label(
                        RichText::new(
                            "Core setup ready. Install maps/models from kingpin.info for MP content.",
                        )
                        .color(theme::ready_green()),
                    );
                    ui.horizontal(|ui| {
                        if ui.button("Open Patch Config").clicked() {
                            if let Some(dir) = self.config.game_dir() {
                                let _ = actions::launch_patchcfg(&dir);
                            }
                        }
                        if ui.button("Launch M-Browser").clicked() {
                            self.launch_mbrowser();
                        }
                        if ui.button("Reinstall MH patch").clicked() {
                            self.install_mh();
                        }
                    });
                }
            }
        });

        ui.add_space(14.0);
        ui.group(|ui| {
            ui.label(
                RichText::new("M-Browser (optional)")
                    .strong()
                    .color(theme::yellow()),
            );
            ui.label(
                "Classic Hambloch console browser — optional. Prefer the built-in Browser tab for multiplayer.",
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Open M-Browser download").clicked() {
                    match wizard::open_mbrowser_download() {
                        Ok(()) => self.set_ok("Download the exe, then use Install from file…"),
                        Err(e) => self.set_err(e),
                    }
                }
                if ui.button("Install from file…").clicked() {
                    self.install_mbrowser_file();
                }
                if ui
                    .add_enabled(
                        self.checklist.mbrowser == ItemStatus::Ok,
                        egui::Button::new("Launch M-Browser"),
                    )
                    .clicked()
                {
                    self.launch_mbrowser();
                }
                if self.checklist.mbrowser == ItemStatus::Ok {
                    ui.label(RichText::new("Present").color(theme::ready_green()));
                } else {
                    ui.label(RichText::new("Not installed").color(Color32::from_rgb(160, 150, 130)));
                }
            });
        });

        ui.add_space(14.0);
        ui.group(|ui| {
            ui.label(
                RichText::new("Client content (kingpin.info)")
                    .strong()
                    .color(theme::yellow()),
            );
            ui.label(format!(
                "Downloads {} public packs from kingpin.info: map-pack-n-yo-ass, Bagman maps, and player models (~2.3 GB). Extracts into your Kingpin folder.",
                addons::pack_count()
            ));
            ui.hyperlink_to("Browse packs on kingpin.info", addons::CONTENT_INDEX_URL);
            ui.add_space(6.0);

            let can_install = self.checklist.kingpin_exe == ItemStatus::Ok && !self.addon_busy;
            ui.horizontal(|ui| {
                let label = if self.checklist.kpaddons == ItemStatus::Ok {
                    "Reinstall maps & models"
                } else {
                    "Download & install maps & models"
                };
                if ui
                    .add_enabled(can_install, egui::Button::new(label).fill(theme::blood()))
                    .clicked()
                {
                    self.start_kpaddons_install();
                }
                if ui
                    .add_enabled(
                        self.checklist.kingpin_exe == ItemStatus::Ok,
                        egui::Button::new("Scan installed content"),
                    )
                    .on_hover_text(
                        "Looks at maps/models already in the Kingpin folder and writes main/kingpin-launcher-content.json",
                    )
                    .clicked()
                {
                    self.scan_and_write_content_marker();
                }
                if self.checklist.kpaddons == ItemStatus::Ok {
                    ui.label(RichText::new("Installed").color(theme::ready_green()));
                } else {
                    ui.label(RichText::new("Not installed").color(theme::yellow()));
                }
            });
            if let Some(dir) = self.config.game_dir() {
                let d = addons::scan_installed_content(&dir);
                ui.label(
                    RichText::new(format!(
                        "On disk: {} map .bsp, {} player models{}",
                        d.map_files,
                        d.player_model_dirs,
                        if d.legacy_pak8 { ", legacy pak8.pak" } else { "" }
                    ))
                    .small()
                    .color(Color32::from_rgb(160, 150, 130)),
                );
            }

            if self.addon_busy {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new(&self.addon_phase).color(theme::yellow()));
                });
                if self.addon_total > 0 {
                    let frac = (self.addon_done as f32 / self.addon_total as f32).clamp(0.0, 1.0);
                    let mb_done = self.addon_done as f64 / (1024.0 * 1024.0);
                    let mb_total = self.addon_total as f64 / (1024.0 * 1024.0);
                    ui.add(egui::ProgressBar::new(frac).text(format!(
                        "{mb_done:.0} / {mb_total:.0} MB"
                    )));
                }
                ui.label(
                    RichText::new(
                        "Tip: under Program Files you may need to run the launcher as Administrator.",
                    )
                    .small()
                    .color(Color32::from_rgb(140, 130, 115)),
                );
            }
        });
        ui.add_space(16.0);
    }

    fn draw_checklist(&self, ui: &mut egui::Ui) {
        let rows = [
            ("Game folder", self.checklist.game_dir.is_some()),
            ("kingpin.exe (1.21)", self.checklist.kingpin_exe == ItemStatus::Ok),
            ("MH patch (wsock32.dll)", self.checklist.mh_dll == ItemStatus::Ok),
            ("MH patchcfg.exe", self.checklist.mh_patchcfg == ItemStatus::Ok),
            (
                "M-Browser (optional)",
                self.checklist.mbrowser == ItemStatus::Ok,
            ),
            (
                "Maps & models (kingpin.info)",
                self.checklist.kpaddons == ItemStatus::Ok,
            ),
        ];

        for (label, ok) in rows {
            ui.horizontal(|ui| {
                let (mark, color) = if ok {
                    ("●", theme::ready_green())
                } else {
                    ("○", theme::yellow())
                };
                ui.label(RichText::new(mark).color(color));
                ui.label(label);
            });
        }
    }
}

/// Truncate a path for the footer strip (`…\tail` when longer than `max`).
fn shorten_path(path: &PathBuf, max: usize) -> String {
    let s = path.to_string_lossy();
    if s.len() <= max {
        return s.into_owned();
    }
    let keep = max.saturating_sub(3);
    format!("...{}", &s[s.len().saturating_sub(keep)..])
}
