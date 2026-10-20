//! Kingpin Launcher — entry point.
//!
//! Boots an egui/eframe native window. On Windows release builds we hide the
//! console (`windows_subsystem = "windows"`) so double-clicking the `.exe`
//! does not flash a terminal. Debug builds keep a console for logging.
//!
//! Window size is capped: the Browser table needs width, but OS maximize on
//! Windows ignores `max_inner_size`, so the maximize button is disabled and
//! we clamp size ourselves in the UI update loop.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod actions;
mod addons;
mod autoexec;
mod checklist;
mod config;
mod detect;
mod mods;
mod query;
mod servers;
mod skins;
mod theme;
mod ui;
mod wizard;

use eframe::egui;
use ui::KingpinLauncherApp;

/// Soft floor — 16:9 cinematic stage.
pub const WIN_MIN: [f32; 2] = [1024.0, 576.0];
/// Soft ceiling.
pub const WIN_MAX: [f32; 2] = [1600.0, 900.0];
/// Default matches mockup aspect (16:9).
pub const WIN_DEFAULT: [f32; 2] = [1280.0, 720.0];

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(WIN_DEFAULT)
            .with_min_inner_size(WIN_MIN)
            .with_max_inner_size(WIN_MAX)
            // Mockup is a borderless stage — no Windows title chrome.
            .with_decorations(false)
            .with_maximize_button(false)
            .with_taskbar(true)
            .with_title("Kingpin Launcher")
            .with_icon(load_app_icon()),
        ..Default::default()
    };

    eframe::run_native(
        "Kingpin Launcher",
        options,
        Box::new(|cc| Ok(Box::new(KingpinLauncherApp::new(cc)))),
    )
}

/// Decode the embedded PNG into egui's RGBA icon format for the title bar / taskbar.
fn load_app_icon() -> egui::IconData {
    let image = image::load_from_memory(include_bytes!("../assets/icon.png"))
        .expect("assets/icon.png")
        .into_rgba8();
    let (width, height) = image.dimensions();
    egui::IconData {
        rgba: image.into_raw(),
        width,
        height,
    }
}
