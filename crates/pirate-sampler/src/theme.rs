use nih_plug_egui::egui::{self, Color32, CornerRadius, FontId, Margin, Stroke, Visuals};

// Warm enamel, amber indicators, and cream lettering inspired by vintage equipment.
pub const BG: Color32 = Color32::from_rgb(0x17, 0x13, 0x0F);
pub const PANEL: Color32 = Color32::from_rgb(0x30, 0x27, 0x1E);
pub const CARD: Color32 = Color32::from_rgb(0x41, 0x36, 0x2A);
pub const STROKE: Color32 = Color32::from_rgb(0x78, 0x64, 0x4C);
pub const TEXT: Color32 = Color32::from_rgb(0xF3, 0xE4, 0xC2);
pub const MUTED: Color32 = Color32::from_rgb(0xBA, 0xA5, 0x87);
pub const DISABLED_TEXT: Color32 = Color32::from_rgb(0x78, 0x6B, 0x59);
pub const ACCENT: Color32 = Color32::from_rgb(0xDB, 0x9D, 0x44);
pub const ACCENT_HOVER: Color32 = Color32::from_rgb(0xFF, 0xD1, 0x8A);
pub const DANGER: Color32 = Color32::from_rgb(0xDB, 0x68, 0x4C);

// Surface and line colors
pub const HOVER_FILL: Color32 = Color32::from_rgb(0x58, 0x46, 0x32);
pub const TRACK_FILL: Color32 = Color32::from_rgb(0x1B, 0x18, 0x13);
pub const DISABLED_STROKE: Color32 = Color32::from_rgb(0x55, 0x49, 0x3A);
pub const GRID_LINE: Color32 = Color32::from_rgb(0x40, 0x33, 0x24);
pub const ZERO_LINE: Color32 = Color32::from_rgb(0x6D, 0x58, 0x39);

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

    // Warm highlight on hover
    visuals.widgets.hovered.bg_fill = HOVER_FILL;
    visuals.widgets.hovered.weak_bg_fill = HOVER_FILL;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, ACCENT_HOVER);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.5_f32, TEXT);
    visuals.widgets.hovered.corner_radius = ctrl_radius;

    // Illuminated pressed controls
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

    // Translucent amber selection
    visuals.selection.bg_fill = Color32::from_rgba_unmultiplied(0xDB, 0x9D, 0x44, 40);
    visuals.selection.stroke = Stroke::new(1.0_f32, ACCENT);

    style.visuals = visuals;
    ctx.set_style(style);
}
