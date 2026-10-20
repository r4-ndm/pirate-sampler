use nih_plug_egui::egui::{self, Color32, CornerRadius, FontId, Margin, Stroke, Visuals};

// Pure monochrome greyscale palette — zero hue anywhere
pub const BG: Color32 = Color32::from_rgb(0x00, 0x00, 0x00);
pub const PANEL: Color32 = Color32::from_rgb(0x0B, 0x0B, 0x0B);
pub const CARD: Color32 = Color32::from_rgb(0x14, 0x14, 0x14);
pub const STROKE: Color32 = Color32::from_rgb(0x2E, 0x2E, 0x2E);
pub const TEXT: Color32 = Color32::from_rgb(0xFF, 0xFF, 0xFF);
pub const MUTED: Color32 = Color32::from_rgb(0x9A, 0x9A, 0x9A);
pub const DISABLED_TEXT: Color32 = Color32::from_rgb(0x6A, 0x6A, 0x6A);
pub const ACCENT: Color32 = Color32::from_rgb(0xFF, 0xFF, 0xFF);
pub const ACCENT_HOVER: Color32 = Color32::from_rgb(0xE8, 0xE8, 0xE8);
pub const DANGER: Color32 = Color32::from_rgb(0xFF, 0xFF, 0xFF);

// Additional monochrome surface/line constants
pub const HOVER_FILL: Color32 = Color32::from_rgb(0x1A, 0x1A, 0x1A);
pub const TRACK_FILL: Color32 = Color32::from_rgb(0x2A, 0x2A, 0x2A);
pub const DISABLED_STROKE: Color32 = Color32::from_rgb(0x3A, 0x3A, 0x3A);
pub const GRID_LINE: Color32 = Color32::from_rgb(0x24, 0x24, 0x24);
pub const ZERO_LINE: Color32 = Color32::from_rgb(0x44, 0x44, 0x44);

// Geometry tokens (hard-edged instrument panel: every corner is square)
pub const CARD_RADIUS: u8 = 0;
pub const CONTROL_RADIUS: u8 = 0;
pub const PAD_SM: i8 = 8;
pub const PAD_MD: i8 = 10;

// Typography hierarchy (built-in font only)
pub fn font_title() -> FontId {
    FontId::proportional(15.0)
}

pub fn font_body() -> FontId {
    FontId::proportional(12.5)
}

pub fn font_small() -> FontId {
    FontId::proportional(11.0)
}

pub fn font_tiny() -> FontId {
    FontId::proportional(10.0)
}

pub fn apply(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(6.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    style.spacing.window_margin = Margin::same(PAD_SM);
    style.spacing.slider_width = 140.0;

    let mut visuals = Visuals::dark();
    visuals.override_text_color = Some(TEXT);
    visuals.slider_trailing_fill = true;
    visuals.panel_fill = BG;
    visuals.window_fill = PANEL;
    visuals.extreme_bg_color = BG;
    visuals.faint_bg_color = PANEL;
    visuals.window_corner_radius = CornerRadius::same(CARD_RADIUS);
    visuals.window_stroke = Stroke::new(1.0_f32, STROKE);

    let ctrl_radius = CornerRadius::same(CONTROL_RADIUS);

    visuals.widgets.noninteractive.bg_fill = CARD;
    visuals.widgets.noninteractive.weak_bg_fill = PANEL;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, STROKE);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, MUTED);
    visuals.widgets.noninteractive.corner_radius = ctrl_radius;

    visuals.widgets.inactive.bg_fill = CARD;
    visuals.widgets.inactive.weak_bg_fill = CARD;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, STROKE);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    visuals.widgets.inactive.corner_radius = ctrl_radius;

    // Hover: #E8E8E8 stroke on a #1A1A1A fill
    visuals.widgets.hovered.bg_fill = HOVER_FILL;
    visuals.widgets.hovered.weak_bg_fill = HOVER_FILL;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, ACCENT_HOVER);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.5_f32, TEXT);
    visuals.widgets.hovered.corner_radius = ctrl_radius;

    // Active/pressed widgets: white fill with BLACK text
    visuals.widgets.active.bg_fill = ACCENT;
    visuals.widgets.active.weak_bg_fill = ACCENT;
    visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    visuals.widgets.active.fg_stroke = Stroke::new(1.5_f32, BG);
    visuals.widgets.active.corner_radius = ctrl_radius;

    visuals.widgets.open.bg_fill = HOVER_FILL;
    visuals.widgets.open.weak_bg_fill = HOVER_FILL;
    visuals.widgets.open.bg_stroke = Stroke::new(1.0_f32, ACCENT_HOVER);
    visuals.widgets.open.fg_stroke = Stroke::new(1.0_f32, TEXT);
    visuals.widgets.open.corner_radius = ctrl_radius;

    // selection.bg_fill = white at alpha 40; selection.stroke = white 1 px
    visuals.selection.bg_fill = Color32::from_rgba_unmultiplied(0xFF, 0xFF, 0xFF, 40);
    visuals.selection.stroke = Stroke::new(1.0_f32, ACCENT);

    style.visuals = visuals;
    ctx.set_style(style);
}
