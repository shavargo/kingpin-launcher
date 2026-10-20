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
//! | Player | [`crate::skins`] + [`crate::autoexec`] (skin / name / extras) |
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
use crate::skins::{self, SkinSelection};
use crate::theme;
use crate::wizard::{self, WizardStep};
use eframe::egui::{self, Align, Color32, Event, Layout, RichText, TextStyle, TextureHandle};
use egui_extras::{Column, TableBuilder};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Sidebar navigation destinations (persisted as strings in `launcher.toml`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Play,
    Browser,
    Mods,
    Player,
    Controls,
    Patches,
}

impl Tab {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Play => "play",
            Self::Browser => "browser",
            Self::Mods => "mods",
            Self::Player => "player",
            Self::Controls => "controls",
            Self::Patches => "patches",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "browser" => Self::Browser,
            "mods" => Self::Mods,
            "player" => Self::Player,
            "controls" => Self::Controls,
            "patches" => Self::Patches,
            _ => Self::Play,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Play => "SINGLE PLAYER",
            Self::Browser => "SERVER BROWSER",
            Self::Mods => "MULTIPLAYER",
            Self::Player => "PLAYER",
            Self::Controls => "OPTIONS",
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
    // --- Controls / Player tabs (shared autoexec.cfg) ---
    controls_game: String,
    controls_text: String,
    controls_dirty: bool,
    controls_loaded_for: Option<String>,
    /// When set, the next key/mouse event rebinds this command.
    controls_listening: Option<String>,
    controls_show_raw: bool,
    // --- Player tab ---
    player_models: Vec<String>,
    player_model_filter: String,
    player_sel: SkinSelection,
    player_name: String,
    player_extras: String,
    player_gender: String,
    player_tex: HashMap<String, TextureHandle>,
    player_tex_error: HashMap<String, String>,
    /// Browser JOIN CTA painted on the foreground layer (avoids pane clip).
    join_cta: Option<(egui::Rect, bool)>,
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
            player_models: Vec::new(),
            player_model_filter: String::new(),
            player_sel: SkinSelection {
                model: "male_thug".into(),
                head: "001".into(),
                body: "001".into(),
                legs: "001".into(),
            },
            player_name: "Player".into(),
            player_extras: "0000".into(),
            player_gender: "male".into(),
            player_tex: HashMap::new(),
            player_tex_error: HashMap::new(),
            join_cta: None,
        };
        // Always check the mirror list on launch (body download only if SHA/ETag changed).
        std::thread::spawn(|| crate::servers::sync_list_cache());
        if app.tab == Tab::Browser {
            app.start_server_refresh();
        }
        if app.tab == Tab::Mods {
            app.start_mods_catalog();
        }
        if app.tab == Tab::Controls || app.tab == Tab::Player {
            app.load_controls();
            if app.tab == Tab::Player {
                app.sync_player_from_autoexec();
            }
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

        theme::paint_backdrop(ctx);
        self.join_cta = None;

        // Single transparent stage — alley is the hero (mockup layout).
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.inner_margin(egui::Margin::ZERO))
            .show(ctx, |ui| {
                let full = ui.max_rect();

                // Borderless window: drag from the top strip / logo band.
                let drag_h = 48.0_f32;
                let drag_rect = egui::Rect::from_min_size(
                    full.min,
                    egui::vec2(full.width() - 44.0, drag_h),
                );
                let drag = ui.interact(
                    drag_rect,
                    egui::Id::new("title_drag"),
                    egui::Sense::click_and_drag(),
                );
                if drag.dragged() {
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }

                ui.scope_builder(egui::UiBuilder::new().max_rect(full), |ui| {
                    // Centered distressed KINGPIN logo (mockup proportions).
                    let logo_w = (full.width() * 0.48).clamp(420.0, 600.0);
                    let logo_h = logo_w * 0.20;
                    let logo_rect = egui::Rect::from_center_size(
                        egui::pos2(full.center().x, full.top() + logo_h * 0.58 + 6.0),
                        egui::vec2(logo_w, logo_h),
                    );
                    theme::paint_logo(ui, logo_rect);

                    // Left riveted nav — mockup order: 4 destinations + QUIT.
                    let nav_w = (full.width() * 0.20).clamp(220.0, 270.0);
                    let nav_top = full.top() + logo_h + 28.0;
                    let nav_rect = egui::Rect::from_min_size(
                        egui::pos2(full.left() + full.width() * 0.035, nav_top),
                        egui::vec2(nav_w, full.height() - (nav_top - full.top()) - 36.0),
                    );
                    ui.scope_builder(egui::UiBuilder::new().max_rect(nav_rect), |ui| {
                        ui.set_min_size(nav_rect.size());
                        // Match mockup: SINGLE PLAYER / MULTIPLAYER / SERVER BROWSER / OPTIONS / QUIT
                        let options_selected = matches!(
                            self.tab,
                            Tab::Controls | Tab::Player | Tab::Patches
                        );
                        for (tab, selected) in [
                            (Tab::Play, self.tab == Tab::Play),
                            (Tab::Mods, self.tab == Tab::Mods),
                            (Tab::Browser, self.tab == Tab::Browser),
                            (Tab::Controls, options_selected),
                        ] {
                            if theme::rivet_nav_button(ui, tab.label(), selected).clicked() {
                                self.select_tab(tab);
                            }
                            ui.add_space(10.0);
                        }
                        ui.add_space(14.0);
                        if theme::rivet_nav_button(ui, "QUIT", false).clicked() {
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });

                    // Content stage to the right of nav (mockup teal pane zone).
                    let content_rect = egui::Rect::from_min_max(
                        egui::pos2(
                            full.left() + nav_w + full.width() * 0.045,
                            full.top() + logo_h + 18.0,
                        ),
                        egui::pos2(full.right() - full.width() * 0.025, full.bottom() - 16.0),
                    );
                    ui.scope_builder(egui::UiBuilder::new().max_rect(content_rect), |ui| {
                        ui.set_min_size(content_rect.size());
                        match self.tab {
                            Tab::Play => self.ui_play(ui),
                            Tab::Browser => self.ui_browser(ui),
                            Tab::Mods => self.ui_mods(ui),
                            Tab::Controls | Tab::Player | Tab::Patches => {
                                self.ui_options_shell(ui);
                            }
                        }
                    });

                    // Status only on errors — mockup has a clean stage.
                    if self.status_is_error && !self.status_message.is_empty() {
                        ui.painter().text(
                            egui::pos2(full.left() + 36.0, full.bottom() - 18.0),
                            egui::Align2::LEFT_BOTTOM,
                            &self.status_message,
                            egui::FontId::new(12.0, egui::FontFamily::Proportional),
                            theme::blood(),
                        );
                    }
                });

                // JOIN sits on the foreground layer so the teal pane never clips it.
                if let Some((join_rect, can_join)) = self.join_cta {
                    let join_id = egui::Id::new("join_cta_fg");
                    let resp = ui.interact(
                        join_rect,
                        join_id,
                        if can_join {
                            egui::Sense::click()
                        } else {
                            egui::Sense::hover()
                        },
                    );
                    let fg = ui.ctx().layer_painter(egui::LayerId::new(
                        egui::Order::Foreground,
                        egui::Id::new("join_cta_layer"),
                    ));
                    theme::paint_join_plate(
                        &fg,
                        join_rect,
                        "JOIN",
                        can_join,
                        resp.hovered(),
                    );
                    if resp.clicked() {
                        self.join_selected();
                    }
                }
            });
    }
}

impl KingpinLauncherApp {
    fn select_tab(&mut self, tab: Tab) {
        self.tab = tab;
        self.config.last_tab = Some(tab.as_str().into());
        let _ = self.config.save();
        if tab == Tab::Browser && !self.servers_loaded && !self.busy {
            self.start_server_refresh();
        }
        if tab == Tab::Mods && !self.mods_loaded && !self.mods_busy {
            self.start_mods_catalog();
        }
        if matches!(tab, Tab::Controls | Tab::Player | Tab::Patches) {
            self.ensure_controls_loaded();
            if tab == Tab::Player || tab == Tab::Controls {
                // Keep player fields warm when entering Options.
                if self.tab == Tab::Player {
                    self.sync_player_from_autoexec();
                }
            }
        }
    }

    /// OPTIONS groups Controls / Player / Patches (mockup has one Options plate).
    fn ui_options_shell(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for (tab, label) in [
                (Tab::Controls, "CONTROLS"),
                (Tab::Player, "PLAYER"),
                (Tab::Patches, "PATCHES"),
            ] {
                let on = self.tab == tab;
                let text = if on {
                    RichText::new(label).color(theme::amber()).strong()
                } else {
                    RichText::new(label).color(theme::muted())
                };
                if ui
                    .add(
                        egui::Button::new(text)
                            .fill(if on {
                                Color32::from_rgba_unmultiplied(40, 30, 18, 200)
                            } else {
                                Color32::from_rgba_unmultiplied(10, 12, 14, 160)
                            })
                            .stroke(egui::Stroke::new(
                                1.0_f32,
                                if on {
                                    theme::amber()
                                } else {
                                    theme::teal_dim()
                                },
                            )),
                    )
                    .clicked()
                {
                    self.select_tab(tab);
                    if tab == Tab::Player {
                        self.sync_player_from_autoexec();
                    }
                }
            }
        });
        ui.add_space(10.0);
        match self.tab {
            Tab::Player => self.ui_player(ui),
            Tab::Patches => self.ui_patches(ui),
            _ => self.ui_controls(ui),
        }
    }

    fn ui_play(&mut self, ui: &mut egui::Ui) {
        // Logo already sits top-center on the stage — keep Play content sparse.
        ui.add_space(40.0);
        theme::teal_pane(ui, |ui| {
            ui.set_min_width(ui.available_width().min(520.0));
            ui.vertical_centered(|ui| {
                ui.add_space(12.0);
                ui.label(
                    RichText::new("SINGLE PLAYER")
                        .size(18.0)
                        .color(theme::amber())
                        .strong(),
                );
                ui.label(
                    RichText::new("Launch the campaign. Build your empire.")
                        .color(theme::muted()),
                );
                ui.add_space(20.0);
                let enabled = self.checklist.kingpin_exe == ItemStatus::Ok && !self.busy;
                if theme::join_button(ui, "LAUNCH", enabled).clicked() {
                    if let Some(dir) = self.config.game_dir() {
                        match actions::launch_single_player(&dir) {
                            Ok(()) => self.set_ok("Started kingpin.exe"),
                            Err(e) => {
                                self.set_err(e);
                                self.select_tab(Tab::Patches);
                            }
                        }
                    } else {
                        self.set_err("No game folder — open Patches.");
                        self.select_tab(Tab::Patches);
                    }
                }
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    self.play_status_chip(
                        ui,
                        "GAME",
                        if self.checklist.kingpin_exe == ItemStatus::Ok {
                            "READY"
                        } else {
                            "MISSING"
                        },
                        self.checklist.kingpin_exe == ItemStatus::Ok,
                    );
                    ui.add_space(12.0);
                    self.play_status_chip(
                        ui,
                        "PATCH",
                        if self.checklist.is_ready() {
                            "OK"
                        } else {
                            "SETUP"
                        },
                        self.checklist.is_ready(),
                    );
                });
                ui.add_space(12.0);
            });
        });
    }

    fn play_status_chip(&self, ui: &mut egui::Ui, title: &str, value: &str, ok: bool) {
        egui::Frame::new()
            .fill(Color32::from_rgba_unmultiplied(10, 14, 18, 200))
            .stroke(egui::Stroke::new(1.0_f32, theme::teal_dim()))
            .inner_margin(egui::Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.label(RichText::new(title).color(theme::muted()).size(11.0).strong());
                ui.label(
                    RichText::new(value)
                        .color(if ok {
                            theme::ready_green()
                        } else {
                            theme::amber()
                        })
                        .strong(),
                );
            });
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
                    || s.game_label().to_ascii_lowercase().contains(&q)
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
                SortCol::Game => a
                    .game_label()
                    .to_lowercase()
                    .cmp(&b.game_label().to_lowercase()),
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
        // F5 refreshes; right-click the teal pane for filters (no visible chrome).
        if ui.input(|i| i.key_pressed(egui::Key::F5)) && !self.busy {
            self.start_server_refresh();
        }

        let list: Vec<GameServer> = self
            .filtered_sorted_servers()
            .into_iter()
            .cloned()
            .collect();
        let shown = list.len();
        let online_n = list.iter().filter(|s| s.online).count();
        let players_n: u32 = list.iter().map(|s| s.players).sum();

        // Leave room so JOIN can sit on the pane's bottom-right corner (mockup overlap).
        let list_h = (ui.available_height() - 36.0).max(220.0);
        let row_h = 30.0;
        let mut join_addr: Option<String> = None;
        let mut select_addr: Option<String> = None;
        let mut sort_click: Option<SortCol> = None;

        let pane = theme::teal_pane(ui, |ui| {
            ui.set_min_height(list_h);
            let teal = theme::teal();
            TableBuilder::new(ui)
                .striped(false)
                .sense(egui::Sense::click())
                .cell_layout(Layout::left_to_right(Align::Center))
                .column(Column::remainder().at_least(180.0))
                .column(Column::exact(70.0))
                .column(Column::exact(80.0))
                .column(Column::initial(140.0).at_least(90.0).clip(true))
                .min_scrolled_height(list_h - 28.0)
                .max_scroll_height(list_h - 28.0)
                .header(28.0, |mut header| {
                    for (label, col) in [
                        ("NAME", SortCol::Name),
                        ("PING", SortCol::Ping),
                        ("PLAYERS", SortCol::Players),
                        ("MAP", SortCol::Map),
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
                                            .size(15.0)
                                            .color(Color32::from_rgb(90, 235, 225)),
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
                        let name_color = if selected {
                            theme::amber()
                        } else if server.online {
                            teal
                        } else {
                            theme::muted()
                        };

                        body.row(row_h, |mut row| {
                            row.set_selected(selected);
                            row.col(|ui| {
                                ui.label(RichText::new(&server.name).color(name_color));
                            });
                            row.col(|ui| {
                                ui.label(
                                    RichText::new(server.ping_label())
                                        .monospace()
                                        .color(ping_color),
                                );
                            });
                            row.col(|ui| {
                                ui.label(
                                    RichText::new(server.players_label())
                                        .monospace()
                                        .color(teal),
                                );
                            });
                            row.col(|ui| {
                                ui.label(
                                    RichText::new(if server.map.is_empty() {
                                        "—"
                                    } else {
                                        server.map.as_str()
                                    })
                                    .color(teal),
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
        pane.context_menu(|ui| {
            ui.label(RichText::new("FILTERS").color(theme::amber()).strong());
            ui.add(egui::TextEdit::singleline(&mut self.server_search).hint_text("search"));
            ui.checkbox(&mut self.online_only, "Online only");
            ui.checkbox(&mut self.auto_refresh, "Auto refresh");
            if ui.button("Refresh (F5)").clicked() {
                self.start_server_refresh();
                ui.close_menu();
            }
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

        // JOIN overlaps the teal pane's bottom-right corner (mockup) — painted in update().
        let pane_r = pane.rect;
        let join_size = egui::vec2(230.0, 68.0);
        let join_rect = egui::Rect::from_min_size(
            egui::pos2(
                pane_r.right() - join_size.x + 12.0,
                pane_r.bottom() - join_size.y * 0.55,
            ),
            join_size,
        );
        let can_join = self.checklist.kingpin_exe == ItemStatus::Ok
            && self
                .selected_address
                .as_ref()
                .and_then(|a| self.servers.iter().find(|s| &s.address == a))
                .is_some_and(|s| s.online)
            && !self.busy;
        self.join_cta = Some((join_rect, can_join));
        let _ = (shown, online_n, players_n);
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

    fn ui_player(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new("PLAYER")
                .size(22.0)
                .color(theme::amber())
                .strong(),
        );
        ui.label(
            RichText::new(
                "Name, skin, extras → autoexec.cfg so Player Setup sticks every launch.",
            )
            .color(theme::muted()),
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
        self.ensure_player_models();

        let games = self
            .config
            .game_dir()
            .map(|d| autoexec::list_mod_dirs(&d))
            .unwrap_or_default();

        ui.horizontal_wrapped(|ui| {
            ui.label("Game:");
            egui::ComboBox::from_id_salt("player_game")
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
                                self.player_models.clear();
                                self.sync_player_from_autoexec();
                            }
                        }
                    }
                });

            if ui.button("Reload").clicked() {
                self.load_controls();
                self.sync_player_from_autoexec();
                self.set_ok("Reloaded autoexec.cfg");
            }
            if ui
                .add_enabled(self.controls_dirty, egui::Button::new("Save"))
                .clicked()
            {
                self.apply_player_to_autoexec();
                self.save_controls();
            }
            if ui
                .button("Apply to all mods")
                .on_hover_text(
                    "Merges name/skin/extras/gender (and shared controls cvars) into every mod autoexec.",
                )
                .clicked()
            {
                self.apply_player_to_autoexec();
                self.apply_controls_to_all_mods();
            }
        });

        if self.controls_dirty {
            ui.label(
                RichText::new("Unsaved changes")
                    .color(theme::yellow())
                    .small(),
            );
        }

        ui.add_space(8.0);

        ui.horizontal(|ui| {
            ui.label("Name:");
            if ui
                .add(
                    egui::TextEdit::singleline(&mut self.player_name)
                        .desired_width(180.0)
                        .char_limit(32),
                )
                .changed()
            {
                self.apply_player_to_autoexec();
            }
            ui.label("Gender:");
            egui::ComboBox::from_id_salt("player_gender")
                .selected_text(&self.player_gender)
                .show_ui(ui, |ui| {
                    for g in ["male", "female"] {
                        if ui
                            .selectable_label(self.player_gender == g, g)
                            .clicked()
                        {
                            self.player_gender = g.into();
                            self.apply_player_to_autoexec();
                        }
                    }
                });
        });

        ui.horizontal(|ui| {
            ui.label("Extras:");
            let mut hat = self.player_extras.chars().next().unwrap_or('0');
            let mut cigar = self.player_extras.chars().nth(1).unwrap_or('0');
            let hat_label = match hat {
                '1' => "cap",
                '2' => "fedora",
                '3' => "stetson",
                _ => "none",
            };
            let mut hat_changed = false;
            egui::ComboBox::from_id_salt("player_hat")
                .selected_text(hat_label)
                .show_ui(ui, |ui| {
                    for (ch, label) in [('0', "none"), ('1', "cap"), ('2', "fedora"), ('3', "stetson")]
                    {
                        if ui.selectable_label(hat == ch, label).clicked() {
                            hat = ch;
                            hat_changed = true;
                        }
                    }
                });
            ui.label("Cigar:");
            let mut cigar_changed = false;
            egui::ComboBox::from_id_salt("player_cigar")
                .selected_text(if cigar == '1' { "on" } else { "off" })
                .show_ui(ui, |ui| {
                    for (ch, label) in [('0', "off"), ('1', "on")] {
                        if ui.selectable_label(cigar == ch, label).clicked() {
                            cigar = ch;
                            cigar_changed = true;
                        }
                    }
                });
            if hat_changed || cigar_changed {
                self.player_extras = format!("{hat}{cigar}00");
                self.apply_player_to_autoexec();
            }
            ui.label(
                RichText::new(format!("→ {}", self.player_extras))
                    .small()
                    .monospace()
                    .color(Color32::from_rgb(160, 150, 130)),
            );
        });

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);

        ui.horizontal(|ui| {
            ui.label("Filter:");
            ui.add(
                egui::TextEdit::singleline(&mut self.player_model_filter)
                    .desired_width(160.0)
                    .hint_text("model name…"),
            );
            ui.label("Model:");
            let filter = self.player_model_filter.to_ascii_lowercase();
            let filtered: Vec<String> = self
                .player_models
                .iter()
                .filter(|m| filter.is_empty() || m.to_ascii_lowercase().contains(&filter))
                .cloned()
                .collect();
            let current_model = self.player_sel.model.clone();
            let mut picked_model: Option<String> = None;
            egui::ComboBox::from_id_salt("player_model")
                .selected_text(&current_model)
                .width(220.0)
                .show_ui(ui, |ui| {
                    for m in &filtered {
                        if ui
                            .selectable_label(current_model == *m, m)
                            .clicked()
                        {
                            picked_model = Some(m.clone());
                        }
                    }
                });
            if let Some(m) = picked_model {
                self.set_player_model(m);
            }
            ui.label(
                RichText::new(format!("{} models", self.player_models.len()))
                    .small()
                    .color(Color32::from_rgb(160, 150, 130)),
            );
        });

        let Some(dir) = self.config.game_dir() else {
            return;
        };
        let dirs = skins::resolve_dirs(&dir, &self.controls_game, &self.player_sel.model);
        let (heads, bodies, legs) = if let Some(ref d) = dirs {
            (
                skins::list_skin_ids(d, "head"),
                skins::list_skin_ids(d, "body"),
                skins::list_skin_ids(d, "legs"),
            )
        } else {
            (Vec::new(), Vec::new(), Vec::new())
        };

        ui.horizontal(|ui| {
            self.skin_id_combo(ui, "Head", "player_head", &heads, 0);
            self.skin_id_combo(ui, "Body", "player_body", &bodies, 1);
            self.skin_id_combo(ui, "Legs", "player_legs", &legs, 2);
        });

        ui.label(
            RichText::new(format!("skin \"{}\"", self.player_sel.to_cvar()))
                .monospace()
                .small()
                .color(Color32::from_rgb(160, 150, 130)),
        );

        ui.add_space(10.0);
        ui.label(RichText::new("PREVIEW").color(theme::teal()).strong());
        ui.add_space(4.0);

        let preview_parts = [
            ("head", self.player_sel.head.clone()),
            ("body", self.player_sel.body.clone()),
            ("legs", self.player_sel.legs.clone()),
        ];
        ui.horizontal(|ui| {
            for (part, id) in &preview_parts {
                egui::Frame::new()
                    .fill(Color32::from_rgba_unmultiplied(10, 14, 18, 220))
                    .stroke(egui::Stroke::new(1.0_f32, theme::teal_dim()))
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        ui.vertical(|ui| {
                            ui.label(
                                RichText::new(part.to_ascii_uppercase())
                                    .small()
                                    .strong()
                                    .color(theme::amber()),
                            );
                            let path = dirs
                                .as_ref()
                                .and_then(|d| skins::texture_path(d, part, id));
                            match path {
                                Some(path) => {
                                    let key = path.to_string_lossy().to_string();
                                    self.ensure_preview_tex(ui.ctx(), &key, &path);
                                    if let Some(tex) = self.player_tex.get(&key) {
                                        let max_w = 180.0;
                                        let size = tex.size_vec2();
                                        let scale = (max_w / size.x).min(1.0);
                                        ui.add(
                                            egui::Image::new(tex)
                                                .fit_to_exact_size(size * scale)
                                                .bg_fill(Color32::from_rgb(8, 10, 12)),
                                        );
                                    } else if let Some(err) = self.player_tex_error.get(&key) {
                                        ui.label(
                                            RichText::new(err).small().color(theme::blood()),
                                        );
                                    }
                                }
                                None => {
                                    ui.label(
                                        RichText::new(format!("no {part}_{id}.tga"))
                                            .small()
                                            .color(theme::muted()),
                                    );
                                }
                            }
                        });
                    });
                ui.add_space(12.0);
            }
        });
    }

    fn skin_id_combo(
        &mut self,
        ui: &mut egui::Ui,
        label: &str,
        id_salt: &str,
        options: &[String],
        which: u8,
    ) {
        ui.label(format!("{label}:"));
        let current = match which {
            0 => self.player_sel.head.clone(),
            1 => self.player_sel.body.clone(),
            _ => self.player_sel.legs.clone(),
        };
        let mut changed_to: Option<String> = None;
        egui::ComboBox::from_id_salt(id_salt)
            .selected_text(&current)
            .width(100.0)
            .show_ui(ui, |ui| {
                if options.is_empty() {
                    ui.label("(none found)");
                    return;
                }
                for opt in options {
                    if ui.selectable_label(current == *opt, opt).clicked() {
                        changed_to = Some(opt.clone());
                    }
                }
            });
        if let Some(v) = changed_to {
            match which {
                0 => self.player_sel.head = v,
                1 => self.player_sel.body = v,
                _ => self.player_sel.legs = v,
            }
            self.apply_player_to_autoexec();
        }
    }

    fn ensure_player_models(&mut self) {
        let Some(dir) = self.config.game_dir() else {
            return;
        };
        if !self.player_models.is_empty() {
            return;
        }
        self.player_models = skins::list_models(&dir, &self.controls_game);
        if self.player_models.is_empty() {
            return;
        }
        if !self
            .player_models
            .iter()
            .any(|m| m.eq_ignore_ascii_case(&self.player_sel.model))
        {
            if let Some(first) = self.player_models.first().cloned() {
                self.set_player_model(first);
            }
        }
    }

    fn set_player_model(&mut self, model: String) {
        let Some(dir) = self.config.game_dir() else {
            self.player_sel.model = model;
            return;
        };
        if let Some(sel) = skins::default_selection(&dir, &self.controls_game, &model) {
            self.player_sel = sel;
        } else {
            self.player_sel.model = model;
        }
        if self.player_sel.model.to_ascii_lowercase().starts_with("female") {
            self.player_gender = "female".into();
        } else if self.player_sel.model.to_ascii_lowercase().starts_with("male") {
            self.player_gender = "male".into();
        }
        self.apply_player_to_autoexec();
    }

    fn sync_player_from_autoexec(&mut self) {
        let Some(dir) = self.config.game_dir() else {
            return;
        };
        self.player_models = skins::list_models(&dir, &self.controls_game);

        if let Some(name) = autoexec::get_cvar(&self.controls_text, "name") {
            self.player_name = name;
        }
        if let Some(extras) = autoexec::get_cvar(&self.controls_text, "extras") {
            let mut e = extras;
            while e.len() < 4 {
                e.push('0');
            }
            self.player_extras = e.chars().take(4).collect();
        }
        if let Some(gender) = autoexec::get_cvar(&self.controls_text, "gender") {
            self.player_gender = gender.to_ascii_lowercase();
        }

        if let Some(raw) = autoexec::get_cvar(&self.controls_text, "skin") {
            if let Some(sel) = SkinSelection::parse(&raw) {
                self.player_sel = sel;
                return;
            }
        }
        // Fallback: first installed model
        if let Some(model) = self.player_models.first().cloned() {
            if let Some(sel) = skins::default_selection(&dir, &self.controls_game, &model) {
                self.player_sel = sel;
            }
        }
    }

    fn apply_player_to_autoexec(&mut self) {
        let skin = self.player_sel.to_cvar();
        self.controls_text = autoexec::set_cvar(&self.controls_text, "name", &self.player_name);
        self.controls_text = autoexec::set_cvar(&self.controls_text, "skin", &skin);
        self.controls_text = autoexec::set_cvar(&self.controls_text, "extras", &self.player_extras);
        self.controls_text =
            autoexec::set_cvar(&self.controls_text, "gender", &self.player_gender);
        self.controls_text = autoexec::set_cvar(&self.controls_text, "gender_auto", "1");
        self.controls_dirty = true;
    }

    fn ensure_preview_tex(&mut self, ctx: &egui::Context, key: &str, path: &std::path::Path) {
        if self.player_tex.contains_key(key) || self.player_tex_error.contains_key(key) {
            return;
        }
        match skins::load_preview_image(path, 256) {
            Ok(color) => {
                let tex = ctx.load_texture(
                    format!("skin-{}", key),
                    color,
                    egui::TextureOptions::LINEAR,
                );
                self.player_tex.insert(key.to_string(), tex);
            }
            Err(e) => {
                self.player_tex_error.insert(key.to_string(), e);
            }
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
