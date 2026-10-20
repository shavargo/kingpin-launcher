//! Kingpin noir visual theme.
//!
//! Dark asphalt panels, muted blood-red accents, nicotine yellow highlights,
//! and a low-opacity tiled brick texture borrowed from kingpin.info community
//! chrome. Applied once at startup via [`apply`]; the brick backdrop is
//! painted every frame behind panels via [`paint_brick_backdrop`].

use egui::{
    Color32, ColorImage, FontFamily, FontId, Id, LayerId, Rect, Stroke, Style, TextStyle,
    TextureHandle, TextureOptions, Visuals,
};

/// Kingpin.info site brick/metal tile (community chrome).
const BRICK_PNG: &[u8] = include_bytes!("../assets/brickbackground.png");

const ASPHALT: Color32 = Color32::from_rgb(18, 16, 14);
/// Logical tile size so the pattern matches kingpin.info grit.
const BRICK_TILE_PX: f32 = 128.0;

/// Install Kingpin noir colors, spacing, and larger default fonts on `ctx`.
pub fn apply(ctx: &egui::Context) {
    let mut style = Style::default();
    let mut visuals = Visuals::dark();

    // Translucent so the brick tile reads through panels without washing out text.
    let panel = Color32::from_rgba_unmultiplied(28, 24, 20, 200);
    let raised = Color32::from_rgb(38, 32, 26);
    let blood = Color32::from_rgb(160, 48, 36);
    let blood_hi = Color32::from_rgb(196, 72, 48);
    let yellow = Color32::from_rgb(196, 160, 72);
    let text = Color32::from_rgb(230, 220, 200);
    let muted = Color32::from_rgb(140, 128, 110);

    visuals.window_fill = ASPHALT;
    visuals.panel_fill = panel;
    visuals.extreme_bg_color = ASPHALT;
    visuals.faint_bg_color = raised;
    visuals.override_text_color = Some(text);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, muted);
    visuals.widgets.inactive.bg_fill = raised;
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, text);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(52, 42, 34);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, yellow);
    visuals.widgets.active.bg_fill = blood;
    visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, text);
    visuals.selection.bg_fill = blood_hi;
    visuals.hyperlink_color = yellow;
    visuals.warn_fg_color = yellow;
    visuals.error_fg_color = blood_hi;

    style.visuals = visuals;
    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    style.spacing.button_padding = egui::vec2(14.0, 10.0);
    // Larger type — default egui sizes are hard on older eyes / high-DPI.
    style.text_styles.insert(
        TextStyle::Small,
        FontId::new(14.0, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Body,
        FontId::new(17.0, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Button,
        FontId::new(17.0, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Heading,
        FontId::new(24.0, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Monospace,
        FontId::new(16.0, FontFamily::Monospace),
    );
    ctx.set_style(style);
}

/// Tile the brick texture behind all panels (low opacity).
///
/// Must paint on [`LayerId::background`] — a custom Background layer can sort
/// *after* panel content and cover the whole UI.
pub fn paint_brick_backdrop(ctx: &egui::Context) {
    let screen = ctx.screen_rect();
    let tex = brick_texture(ctx);
    let size = tex.size_vec2();
    if size.x < 1.0 || size.y < 1.0 {
        return;
    }

    let uv = Rect::from_min_size(
        egui::pos2(0.0, 0.0),
        egui::vec2(screen.width() / BRICK_TILE_PX, screen.height() / BRICK_TILE_PX),
    );

    let tint = Color32::from_rgba_unmultiplied(220, 200, 170, 72);
    let painter = ctx.layer_painter(LayerId::background());
    painter.rect_filled(screen, 0.0, ASPHALT);
    painter.image(tex.id(), screen, uv, tint);
}

/// Lazily decode and upload the brick PNG once; reuse via egui temp storage.
fn brick_texture(ctx: &egui::Context) -> TextureHandle {
    let id = Id::new("kp_brick_texture");
    if let Some(tex) = ctx.data(|d| d.get_temp::<TextureHandle>(id)) {
        return tex;
    }

    let rgba = image::load_from_memory(BRICK_PNG)
        .expect("brickbackground.png")
        .to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let image = ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
    let tex = ctx.load_texture("brickbackground", image, TextureOptions::LINEAR_REPEAT);
    ctx.data_mut(|d| d.insert_temp(id, tex.clone()));
    tex
}

/// Primary action / danger accent (muted blood red).
pub fn blood() -> Color32 {
    Color32::from_rgb(160, 48, 36)
}

/// Highlight / warning accent (nicotine yellow).
pub fn yellow() -> Color32 {
    Color32::from_rgb(196, 160, 72)
}

/// Footer "READY" green.
pub fn ready_green() -> Color32 {
    Color32::from_rgb(96, 150, 90)
}

/// ASE-style ping bands (green / yellow / orange / red / offline grey).
pub fn ase_ping_color(online: bool, ping_ms: Option<u32>) -> Color32 {
    if !online {
        return Color32::from_rgb(110, 100, 90);
    }
    match ping_ms.unwrap_or(999) {
        0..=79 => Color32::from_rgb(72, 200, 90),
        80..=149 => Color32::from_rgb(210, 190, 60),
        150..=249 => Color32::from_rgb(210, 120, 40),
        _ => Color32::from_rgb(200, 55, 45),
    }
}

/// Semi-opaque panel fill used inside the Browser tab.
pub fn ase_panel() -> Color32 {
    Color32::from_rgba_unmultiplied(22, 20, 18, 230)
}
