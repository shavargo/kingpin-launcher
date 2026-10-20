//! Mockup-accurate rain-noir chrome (plates + logo cropped from the reference).

use egui::{
    Color32, ColorImage, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Id,
    LayerId, Margin, Rect, Shadow, Stroke, Style, TextStyle, TextureHandle, TextureOptions,
    Visuals,
};

const ALLEY: &[u8] = include_bytes!("../assets/chrome/alley_clean_1440.jpg");
const LOGO: &[u8] = include_bytes!("../assets/chrome/logo.png");
const BTN_IDLE: &[u8] = include_bytes!("../assets/chrome/btn_idle.png");
const JOIN: &[u8] = include_bytes!("../assets/chrome/join.png");
const FONT_BOLD: &[u8] = include_bytes!("../assets/chrome/AgencyFB-Bold.ttf");
const FONT_REG: &[u8] = include_bytes!("../assets/chrome/AgencyFB-Reg.ttf");

const ASPHALT: Color32 = Color32::from_rgb(4, 5, 7);
const BLOOD: Color32 = Color32::from_rgb(140, 28, 24);
const BLOOD_HI: Color32 = Color32::from_rgb(200, 52, 40);
const AMBER: Color32 = Color32::from_rgb(255, 176, 48);
const TEAL: Color32 = Color32::from_rgb(72, 220, 210);
const TEAL_DIM: Color32 = Color32::from_rgb(28, 120, 118);
const TEXT: Color32 = Color32::from_rgb(236, 230, 218);
const MUTED: Color32 = Color32::from_rgb(160, 155, 145);

pub fn apply(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "agency_bold".into(),
        FontData::from_owned(FONT_BOLD.to_vec()).into(),
    );
    fonts.font_data.insert(
        "agency_reg".into(),
        FontData::from_owned(FONT_REG.to_vec()).into(),
    );
    // Prefer Agency FB for all proportional UI (condensed industrial look).
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "agency_bold".to_owned());
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(1, "agency_reg".to_owned());
    ctx.set_fonts(fonts);

    let mut style = Style::default();
    let mut visuals = Visuals::dark();
    visuals.window_fill = ASPHALT;
    visuals.panel_fill = Color32::TRANSPARENT;
    visuals.extreme_bg_color = ASPHALT;
    visuals.faint_bg_color = Color32::from_rgba_unmultiplied(8, 10, 12, 100);
    visuals.override_text_color = Some(TEXT);
    for w in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        w.corner_radius = CornerRadius::ZERO;
    }
    visuals.widgets.noninteractive.bg_stroke = Stroke::NONE;
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, MUTED);
    visuals.widgets.inactive.bg_fill = Color32::from_rgba_unmultiplied(14, 12, 10, 160);
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(70, 60, 50));
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    visuals.widgets.hovered.bg_fill = Color32::from_rgba_unmultiplied(28, 24, 18, 180);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, AMBER);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, AMBER);
    visuals.widgets.active.bg_fill = BLOOD;
    visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, BLOOD_HI);
    visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, TEXT);
    visuals.widgets.open.bg_fill = Color32::from_rgba_unmultiplied(6, 18, 20, 220);
    visuals.widgets.open.bg_stroke = Stroke::new(1.0_f32, TEAL);
    visuals.selection.bg_fill = Color32::from_rgba_unmultiplied(255, 176, 48, 35);
    visuals.selection.stroke = Stroke::new(1.0_f32, AMBER);
    visuals.hyperlink_color = AMBER;
    visuals.warn_fg_color = AMBER;
    visuals.error_fg_color = BLOOD_HI;
    visuals.window_stroke = Stroke::NONE;
    visuals.window_shadow = Shadow::NONE;
    visuals.popup_shadow = Shadow::NONE;
    visuals.window_corner_radius = CornerRadius::ZERO;
    visuals.menu_corner_radius = CornerRadius::ZERO;

    style.visuals = visuals;
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(10.0, 6.0);
    style.spacing.window_margin = Margin::same(0);
    style.text_styles.insert(
        TextStyle::Small,
        FontId::new(14.0, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Body,
        FontId::new(16.0, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Button,
        FontId::new(16.0, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Heading,
        FontId::new(24.0, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Monospace,
        FontId::new(15.0, FontFamily::Monospace),
    );
    ctx.set_style(style);

    let _ = alley_tex(ctx);
    let _ = logo_tex(ctx);
    let _ = btn_idle_tex(ctx);
    let _ = join_tex(ctx);
}

fn decode(
    ctx: &egui::Context,
    id: &'static str,
    bytes: &[u8],
    punch_luma_below: Option<u16>,
) -> TextureHandle {
    let key = Id::new(id);
    if let Some(t) = ctx.data(|d| d.get_temp::<TextureHandle>(key)) {
        return t;
    }
    let rgba = image::load_from_memory(bytes).unwrap().to_rgba8();
    let (w, h) = (rgba.width() as usize, rgba.height() as usize);
    let mut raw = rgba.into_raw();
    if let Some(thresh) = punch_luma_below {
        for px in raw.chunks_exact_mut(4) {
            let luma = (px[0] as u16 + px[1] as u16 + px[2] as u16) / 3;
            if luma < thresh {
                px[3] = 0;
            }
        }
    }
    let img = ColorImage::from_rgba_unmultiplied([w, h], &raw);
    let tex = ctx.load_texture(id, img, TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(key, tex.clone()));
    tex
}

fn alley_tex(ctx: &egui::Context) -> TextureHandle {
    decode(ctx, "chrome_alley", ALLEY, None)
}
fn logo_tex(ctx: &egui::Context) -> TextureHandle {
    // Punch near-black so only distressed letterforms remain (no rain/lamp box).
    decode(ctx, "chrome_logo", LOGO, Some(18))
}
fn btn_idle_tex(ctx: &egui::Context) -> TextureHandle {
    decode(ctx, "chrome_btn_idle", BTN_IDLE, None)
}
fn join_tex(ctx: &egui::Context) -> TextureHandle {
    // Key out residual black matte around the blood plate (baked JOIN label stays).
    decode(ctx, "chrome_join", JOIN, Some(14))
}

pub fn paint_backdrop(ctx: &egui::Context) {
    let screen = ctx.screen_rect();
    let painter = ctx.layer_painter(LayerId::background());
    painter.rect_filled(screen, 0.0, ASPHALT);
    let tex = alley_tex(ctx);
    let size = tex.size_vec2();
    if size.x > 1.0 {
        let scale = (screen.width() / size.x).max(screen.height() / size.y) * 1.02;
        let draw = egui::vec2(size.x * scale, size.y * scale);
        let origin = egui::pos2(
            screen.center().x - draw.x * 0.5,
            screen.center().y - draw.y * 0.45,
        );
        painter.image(
            tex.id(),
            Rect::from_min_size(origin, draw),
            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    }
    painter.rect_filled(screen, 0.0, Color32::from_rgba_unmultiplied(0, 0, 0, 35));
}

pub fn paint_logo(ui: &mut egui::Ui, rect: Rect) {
    let tex = logo_tex(ui.ctx());
    ui.painter().image(
        tex.id(),
        rect,
        Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        Color32::WHITE,
    );
}

pub fn rivet_nav_button(ui: &mut egui::Ui, label: &str, selected: bool) -> egui::Response {
    let size = egui::vec2(ui.available_width().max(210.0), 52.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let hovered = response.hovered();
    let painter = ui.painter();
    let round = CornerRadius::same(3);

    // Procedural riveted plate only — cropped mockup plates still ghost baked labels.
    let fill = if selected {
        Color32::from_rgb(42, 30, 22)
    } else if hovered {
        Color32::from_rgb(36, 30, 26)
    } else {
        Color32::from_rgb(22, 20, 18)
    };
    painter.rect_filled(rect, round, fill);
    // Soft metal grain via blank plate (very low alpha — rim/rivets only).
    let tex = btn_idle_tex(ui.ctx());
    painter.image(
        tex.id(),
        rect,
        Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        Color32::from_rgba_unmultiplied(255, 255, 255, 40),
    );
    // Double pinstripe bevel (mockup)
    let rim = if selected {
        Color32::from_rgb(210, 150, 50)
    } else {
        Color32::from_rgb(78, 68, 58)
    };
    painter.rect_stroke(
        rect,
        round,
        Stroke::new(2.0_f32, rim),
        egui::StrokeKind::Inside,
    );
    painter.rect_stroke(
        rect.shrink(4.0),
        CornerRadius::same(2),
        Stroke::new(1.0_f32, Color32::from_rgb(14, 12, 10)),
        egui::StrokeKind::Inside,
    );
    // Corner rivets
    let rivet = Color32::from_rgb(120, 108, 92);
    let rivet_hi = Color32::from_rgb(175, 162, 140);
    let pad = 8.0_f32;
    for (x, y) in [
        (rect.left() + pad, rect.top() + pad),
        (rect.right() - pad, rect.top() + pad),
        (rect.left() + pad, rect.bottom() - pad),
        (rect.right() - pad, rect.bottom() - pad),
    ] {
        painter.circle_filled(egui::pos2(x, y), 3.2, rivet);
        painter.circle_filled(egui::pos2(x - 0.8, y - 0.8), 1.2, rivet_hi);
    }

    if selected {
        // Amber selected glow — soft bloom then hard rim (mockup).
        for (expand, a) in [(8.0_f32, 28u8), (5.0, 55), (2.0, 110)] {
            painter.rect_stroke(
                rect.expand(expand),
                CornerRadius::same(5),
                Stroke::new(2.5_f32, Color32::from_rgba_unmultiplied(255, 176, 48, a)),
                egui::StrokeKind::Outside,
            );
        }
        painter.rect_filled(
            rect.shrink(6.0),
            CornerRadius::same(2),
            Color32::from_rgba_unmultiplied(255, 160, 40, 18),
        );
    }

    let color = if selected {
        AMBER
    } else if hovered {
        Color32::from_rgb(255, 245, 220)
    } else {
        Color32::from_rgb(236, 228, 214)
    };
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        FontId::new(18.0, FontFamily::Proportional),
        color,
    );
    response
}

pub fn teal_pane(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) -> egui::Response {
    let r = ui.available_rect_before_wrap();
    // Soft outer bloom onto wet alley (mockup cyan puddle glow) — keep light so cobbles read.
    for (expand, a) in [(14.0_f32, 10u8), (8.0, 18), (3.0, 32)] {
        ui.painter().rect_filled(
            r.expand(expand),
            6.0,
            Color32::from_rgba_unmultiplied(48, 210, 200, a),
        );
    }
    egui::Frame::new()
        // Glass teal — alley cobbles must show through (lower fill alpha than earlier builds).
        .fill(Color32::from_rgba_unmultiplied(0, 55, 58, 48))
        .stroke(Stroke::new(2.0_f32, TEAL))
        .corner_radius(CornerRadius::same(5))
        .inner_margin(Margin::symmetric(16, 12))
        .show(ui, add_contents)
        .response
}

pub fn join_button(ui: &mut egui::Ui, label: &str, enabled: bool) -> egui::Response {
    // Asset is 245×85 — keep that aspect so the baked JOIN plate is not squashed.
    let size = egui::vec2(248.0, 86.0);
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    paint_join_plate(ui.painter(), rect, label, enabled, response.hovered());
    response
}

/// Paint the mockup blood JOIN/LAUNCH plate into an absolute rect (any layer).
pub fn paint_join_plate(
    painter: &egui::Painter,
    rect: Rect,
    label: &str,
    enabled: bool,
    hovered: bool,
) {
    let use_baked = label.eq_ignore_ascii_case("JOIN");
    if use_baked {
        let tex = join_tex(painter.ctx());
        let tint = if !enabled {
            Color32::from_rgba_unmultiplied(130, 110, 110, 170)
        } else if hovered {
            Color32::from_rgba_unmultiplied(255, 235, 225, 255)
        } else {
            Color32::WHITE
        };
        painter.image(
            tex.id(),
            rect,
            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            tint,
        );
        return;
    }
    let fill = if !enabled {
        Color32::from_rgb(50, 28, 28)
    } else if hovered {
        Color32::from_rgb(130, 36, 30)
    } else {
        Color32::from_rgb(100, 26, 22)
    };
    painter.rect_filled(rect, CornerRadius::same(2), fill);
    painter.rect_stroke(
        rect,
        CornerRadius::same(2),
        Stroke::new(
            2.5_f32,
            if enabled {
                BLOOD_HI
            } else {
                Color32::from_rgb(80, 50, 50)
            },
        ),
        egui::StrokeKind::Inside,
    );
    painter.rect_stroke(
        rect.shrink(4.0),
        CornerRadius::same(1),
        Stroke::new(1.0_f32, Color32::from_rgb(40, 12, 12)),
        egui::StrokeKind::Inside,
    );
    let rivet = Color32::from_rgb(150, 75, 65);
    let pad = 9.0_f32;
    for (x, y) in [
        (rect.left() + pad, rect.top() + pad),
        (rect.right() - pad, rect.top() + pad),
        (rect.left() + pad, rect.bottom() - pad),
        (rect.right() - pad, rect.bottom() - pad),
    ] {
        painter.circle_filled(egui::pos2(x, y), 3.2, rivet);
    }
    let tc = if enabled {
        Color32::from_rgb(245, 235, 220)
    } else {
        Color32::from_rgb(120, 100, 100)
    };
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        FontId::new(28.0, FontFamily::Proportional),
        tc,
    );
}

pub fn blood() -> Color32 {
    BLOOD
}
pub fn yellow() -> Color32 {
    AMBER
}
pub fn amber() -> Color32 {
    AMBER
}
pub fn teal() -> Color32 {
    TEAL
}
pub fn teal_dim() -> Color32 {
    TEAL_DIM
}
pub fn muted() -> Color32 {
    MUTED
}
pub fn text() -> Color32 {
    TEXT
}
pub fn ready_green() -> Color32 {
    Color32::from_rgb(72, 170, 120)
}
pub fn ase_ping_color(online: bool, ping_ms: Option<u32>) -> Color32 {
    if !online {
        return MUTED;
    }
    match ping_ms.unwrap_or(999) {
        0..=79 => Color32::from_rgb(64, 210, 150),
        80..=149 => AMBER,
        150..=249 => Color32::from_rgb(210, 120, 40),
        _ => BLOOD_HI,
    }
}
pub fn ase_panel() -> Color32 {
    Color32::from_rgba_unmultiplied(2, 28, 30, 175)
}
pub fn nav_button(ui: &mut egui::Ui, label: &str, selected: bool) -> egui::Response {
    rivet_nav_button(ui, label, selected)
}
pub fn primary_button(ui: &mut egui::Ui, label: &str, enabled: bool) -> egui::Response {
    join_button(ui, label, enabled)
}
