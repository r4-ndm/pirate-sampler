use std::sync::Arc;

use nih_plug::prelude::*;
use nih_plug_egui::{
    create_egui_editor,
    egui::{self, Color32},
    EguiState,
};
use parking_lot::Mutex;
use pirate_sampler_dsp::{write_selection_wav, BufferDuration, RingBuffer, Selection};

use crate::params::PirateSamplerParams;
use crate::theme;

// Window and layout geometry constants (840 x 400 compact side-panel layout)
pub const WINDOW_W: f32 = 840.0;
pub const WINDOW_H: f32 = 400.0;
pub const SIDE_W: f32 = 176.0;
pub const MIN_WAVEFORM_W: f32 = 320.0;
pub const LEFT_MARGIN: i8 = 12;
pub const RIGHT_GAP: i8 = 8;
pub const TOP_MARGIN: i8 = 10;
pub const BOTTOM_MARGIN: i8 = 10;
pub const HORIZONTAL_MARGINS: f32 = (LEFT_MARGIN as f32) + (RIGHT_GAP as f32);
pub const VERTICAL_MARGINS: f32 = (TOP_MARGIN as f32) + (BOTTOM_MARGIN as f32);
pub const WAVEFORM_H: f32 = WINDOW_H - VERTICAL_MARGINS;

// Side-panel block heights for deterministic overflow calculation
const H_BRAND: f32 = 20.0;
const H_STATUS_STRIP: f32 = 14.0;
const H_SOURCE_TOGGLE: f32 = 54.0; // 24 + 6 + 24
const H_BUFFER: f32 = 34.0; // 14 label + 2 gap + 18 slider
const H_TRANSPORT: f32 = 108.0; // 4 * 24 + 3 * 4
const H_MORE_BTN: f32 = 24.0;
const H_EXPORT: f32 = 28.0;
const H_METER: f32 = 24.0; // 14 label + 2 gap + 8 bar
const H_STATUS_LINE: f32 = 14.0;
const SECTION_GAP: f32 = 6.0;

pub const FULL_STACK_NEEDED_H: f32 = H_BRAND
    + H_STATUS_STRIP
    + H_SOURCE_TOGGLE
    + H_BUFFER
    + H_TRANSPORT
    + H_EXPORT
    + H_METER
    + H_STATUS_LINE
    + 7.0 * SECTION_GAP;

pub const TIER1_STACK_NEEDED_H: f32 = H_BRAND
    + H_STATUS_STRIP
    + H_SOURCE_TOGGLE
    + H_BUFFER
    + H_EXPORT
    + H_MORE_BTN
    + H_METER
    + 6.0 * SECTION_GAP;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioInputSource {
    SystemAudio,
    Microphone,
}

pub struct EditorState {
    pub ring: Arc<Mutex<RingBuffer>>,
    /// Selection as normalized 0..1 over the filled buffer (start, end).
    pub selection: Option<(f32, f32)>,
    pub drag_anchor: Option<f32>,
    pub status: String,
    pub freeze_view: bool,
    pub frozen_peaks: Option<Vec<(f32, f32)>>,
    pub input_source: AudioInputSource,
    pub active_links: Vec<(String, String)>,
}

impl EditorState {
    pub fn new(ring: Arc<Mutex<RingBuffer>>) -> Self {
        Self {
            ring,
            selection: None,
            drag_anchor: None,
            status: "Ready - System Audio".to_string(),
            freeze_view: false,
            frozen_peaks: None,
            input_source: AudioInputSource::SystemAudio,
            active_links: Vec::new(),
        }
    }
}

pub fn create(
    params: Arc<PirateSamplerParams>,
    editor_state: Arc<EguiState>,
    shared: Arc<Mutex<EditorState>>,
) -> Option<Box<dyn Editor>> {
    debug_assert!(SIDE_W + MIN_WAVEFORM_W + HORIZONTAL_MARGINS <= WINDOW_W);
    debug_assert!((WAVEFORM_H - (WINDOW_H - VERTICAL_MARGINS)).abs() < f32::EPSILON);

    create_egui_editor(
        editor_state,
        (),
        |egui_ctx, _| {
            theme::apply(egui_ctx);
        },
        move |egui_ctx, setter, _state| {
            theme::apply(egui_ctx);

            // Frameless window: strip the WM decorations as soon as the host window is known.
            // No-op inside a plugin host or on non-X11 platforms.
            #[cfg(target_os = "linux")]
            if let Some(window_id) = nih_plug_egui::parent_x11_window() {
                crate::chrome::attach(window_id, WINDOW_W as u32, WINDOW_H as u32);
            }
            // Keep following the pointer while a drag-to-move or grip-resize is in progress.
            crate::chrome::pump_move();
            crate::chrome::pump_resize();
            egui_ctx.request_repaint_after(std::time::Duration::from_millis(33));

            let mut ed = shared.lock();

            // Live engine metrics read directly from the ring buffer every frame
            let (filled_frames, capacity_frames, sr, is_empty, (peak_l, peak_r)) = {
                let mut ring = ed.ring.lock();
                let p = ring.read_and_decay_peak();
                (
                    ring.filled_frames(),
                    ring.capacity_frames(),
                    ring.sample_rate(),
                    ring.is_empty(),
                    p,
                )
            };
            let cap_secs = capacity_frames as f32 / sr.max(1.0);
            let peak = peak_l.max(peak_r);
            let db = if peak > 1e-4 {
                20.0 * peak.log10()
            } else {
                -96.0
            };

            // Update peak-hold in egui temp memory for the 3 px peak-hold tick
            let hold_id = egui::Id::new("pirate_peak_hold_norm");
            let cur_norm = ((db + 55.0) / 55.0).clamp(0.0, 1.0);
            let peak_hold_norm = egui_ctx.data_mut(|d| {
                let prev = d.get_temp::<f32>(hold_id).unwrap_or(0.0);
                let next = if cur_norm >= prev {
                    cur_norm
                } else {
                    (prev - 0.008).max(cur_norm)
                };
                d.insert_temp(hold_id, next);
                next
            });

            // Keyboard shortcuts
            if egui_ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                ed.selection = None;
            }
            if egui_ctx.input(|i| i.key_pressed(egui::Key::H)) {
                ed.freeze_view = !ed.freeze_view;
                if !ed.freeze_view {
                    ed.frozen_peaks = None;
                }
            }

            let num_shortcuts = [
                (egui::Key::Num1, 0.10),
                (egui::Key::Num2, 0.20),
                (egui::Key::Num3, 0.30),
                (egui::Key::Num4, 0.40),
                (egui::Key::Num5, 0.50),
                (egui::Key::Num6, 0.60),
                (egui::Key::Num7, 0.70),
                (egui::Key::Num8, 0.80),
                (egui::Key::Num9, 0.90),
                (egui::Key::Num0, 1.00),
            ];
            for (key, frac) in num_shortcuts {
                if egui_ctx.input(|i| i.key_pressed(key)) {
                    ed.selection = Some((1.0 - frac, 1.0));
                }
            }

            let copy_shortcut = egui_ctx.input(|i| {
                (i.modifiers.command || i.modifiers.ctrl) && i.key_pressed(egui::Key::C)
            });
            if copy_shortcut && ed.selection.is_some() {
                let msg = copy_selection_to_clipboard(&ed, egui_ctx);
                ed.status = msg;
            }

            let paused = params.pause.value();
            let buf_pct = ((filled_frames as f32 / capacity_frames.max(1) as f32) * 100.0)
                .clamp(0.0, 100.0)
                .round() as u32;
            let sr_khz = sr / 1000.0;

            // 1. RIGHT SIDE PANEL (fixed 176 px, non-resizable)
            egui::SidePanel::right("side_panel")
                .resizable(false)
                .exact_width(SIDE_W)
                .show_separator_line(false)
                .frame(
                    egui::Frame::new()
                        .fill(theme::PANEL)
                        .inner_margin(egui::Margin::symmetric(10, 6)),
                )
                .show(egui_ctx, |ui| {
                    ui.set_clip_rect(ui.max_rect());
                    draw_side_panel(
                        ui,
                        egui_ctx,
                        setter,
                        &params,
                        &mut ed,
                        sr_khz,
                        buf_pct,
                        db,
                        cur_norm,
                        peak_hold_norm,
                    );
                });

            // 2. CENTRAL PANEL: Waveform card spans full window height minus 20 px margins
            egui::CentralPanel::default()
                .frame(
                    egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin {
                        left: LEFT_MARGIN,
                        right: RIGHT_GAP,
                        top: 0,
                        bottom: 0,
                    }),
                )
                .show(egui_ctx, |ui| {
                    let wave_card_h = ui.available_height().max(60.0);
                    draw_waveform_card(
                        ui,
                        &mut ed,
                        wave_card_h,
                        filled_frames,
                        cap_secs,
                        sr,
                        is_empty,
                        paused,
                    );
                });

            // Resize grip: bottom-right corner of the window, drawn above both panels.
            let screen = egui_ctx.screen_rect();
            egui::Area::new(egui::Id::new("resize_grip"))
                .order(egui::Order::Foreground)
                .fixed_pos(egui::pos2(screen.right() - 20.0, screen.bottom() - 20.0))
                .show(egui_ctx, |ui| {
                    let (rect, resp) =
                        ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::drag());
                    if resp.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeNwSe);
                    }
                    if resp.drag_started()
                        || (resp.contains_pointer() && ui.input(|i| i.pointer.any_pressed()))
                    {
                        crate::chrome::begin_resize();
                    }
                    let colour = if resp.hovered() || resp.dragged() {
                        theme::TEXT
                    } else {
                        theme::MUTED
                    };
                    let stroke = egui::Stroke::new(1.2_f32, colour);
                    let painter = ui.painter();
                    let corner = egui::pos2(rect.right() - 3.0, rect.bottom() - 3.0);
                    painter.line_segment(
                        [egui::pos2(corner.x - 9.0, corner.y), egui::pos2(corner.x, corner.y - 9.0)],
                        stroke,
                    );
                    painter.line_segment(
                        [egui::pos2(corner.x - 5.0, corner.y), egui::pos2(corner.x, corner.y - 5.0)],
                        stroke,
                    );
                });
        },
    )
}

#[allow(clippy::too_many_arguments)]
fn draw_side_panel(
    ui: &mut egui::Ui,
    egui_ctx: &egui::Context,
    setter: &ParamSetter,
    params: &Arc<PirateSamplerParams>,
    ed: &mut EditorState,
    sr_khz: f32,
    buf_pct: u32,
    db: f32,
    cur_norm: f32,
    peak_hold_norm: f32,
) {
    ui.spacing_mut().item_spacing = egui::vec2(6.0, SECTION_GAP);
    let avail_h = ui.available_height();
    let full_w = ui.available_width();

    // Panel background is a window drag handle: this responder is registered first,
    // so every control drawn afterwards sits on top of it and keeps its own clicks.
    let bg_drag = ui.interact(
        ui.max_rect(),
        ui.id().with("panel_bg_drag"),
        egui::Sense::drag(),
    );
    if bg_drag.drag_started()
        || (bg_drag.contains_pointer() && ui.input(|i| i.pointer.any_pressed()))
    {
        // Anchor on the press itself, not after egui's drag threshold, so the window
        // ends exactly under the pointer when the button is released.
        crate::chrome::begin_move();
    }

    // Overflow calculation:
    // - overflow_tier1: avail_h < FULL_STACK_NEEDED_H -> move Transport (5) + Status line (8) into "More ▾"
    // - overflow_tier2: avail_h < TIER1_STACK_NEEDED_H -> also move Buffer slider (4) into "More ▾"
    let overflow_tier1 = avail_h < FULL_STACK_NEEDED_H;
    let overflow_tier2 = avail_h < TIER1_STACK_NEEDED_H;
    // Below this the panel drops to a slim header plus a couple of transport rows.
    let compact = avail_h < 120.0;
    let skull_only = avail_h < 200.0;

    // 1. Brand: 20 px vector skull mark + "PIRATE SAMPLER" in 11 px tracked caps.
    //    The row doubles as a window drag handle, with a small minimize button pinned right.
    if ui.available_height() >= if compact { 48.0_f32 } else { H_BRAND } {
        let brand_h = if compact { 16.0 } else { H_BRAND };
        let (brand_rect, _) =
            ui.allocate_exact_size(egui::vec2(full_w, brand_h), egui::Sense::hover());
        let badge_size = if compact { 14.0 } else { 20.0 };
        let badge_rect = egui::Rect::from_min_size(
            egui::pos2(brand_rect.left(), brand_rect.center().y - badge_size * 0.5),
            egui::vec2(badge_size, badge_size),
        );
        draw_logo_mark(ui.painter(), badge_rect);
        if !compact && !skull_only {
            ui.painter().text(
                egui::pos2(badge_rect.right() + 8.0, brand_rect.center().y),
                egui::Align2::LEFT_CENTER,
                "PIRATE SAMPLER",
                theme::font_small(),
                theme::ACCENT_HOVER,
            );
        }

        // Drag zone: the whole brand row except the minimize button column.
        let min_btn = egui::Rect::from_center_size(
            egui::pos2(brand_rect.right() - 8.0, brand_rect.center().y),
            egui::vec2(if compact { 14.0 } else { 16.0 }, if compact { 14.0 } else { 16.0 }),
        );
        let drag_rect = egui::Rect::from_min_max(
            brand_rect.min,
            egui::pos2((min_btn.left() - 4.0).max(brand_rect.left()), brand_rect.max.y),
        );
        let drag_resp = ui.interact(drag_rect, ui.id().with("brand_drag"), egui::Sense::drag());
        if drag_resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        if drag_resp.drag_started()
            || (drag_resp.contains_pointer() && ui.input(|i| i.pointer.any_pressed()))
        {
            crate::chrome::begin_move();
        }

        // Minimize button: 16 px square, thin minus glyph.
        if draw_minimize_button(ui, min_btn, "minimize_btn") {
            crate::chrome::minimize();
        }
    }

    // 2. Status strip: live sample rate and buffer fill from ring
    if !compact && ui.available_height() >= H_STATUS_STRIP {
        let sr_label = if (sr_khz - sr_khz.round()).abs() < 0.05 {
            format!("{:.0} kHz", sr_khz)
        } else {
            format!("{:.1} kHz", sr_khz)
        };
        let strip_text = format!("{} · {}% buf", sr_label, buf_pct);
        let (strip_rect, _) =
            ui.allocate_exact_size(egui::vec2(full_w, H_STATUS_STRIP), egui::Sense::hover());
        ui.painter().text(
            egui::pos2(strip_rect.left(), strip_rect.center().y),
            egui::Align2::LEFT_CENTER,
            strip_text,
            theme::font_small(),
            theme::MUTED,
        );
    }

    // 3. Source toggle: two full-width stacked segments ("System Audio", "Microphone"), 6 px gap
    if !compact && ui.available_height() >= H_SOURCE_TOGGLE {
        draw_stacked_source_toggle(ui, ed, full_w);
    }

    // 4. Buffer slider (inline unless overflow_tier2)
    if !compact && !overflow_tier2 && ui.available_height() >= H_BUFFER {
        draw_buffer_section(ui, setter, params, full_w);
    }

    // 5. Transport buttons (inline unless overflow_tier1)
    if compact {
        draw_compact_transport(ui, setter, params, ed, full_w);
    } else if !overflow_tier1 && ui.available_height() >= H_TRANSPORT {
        draw_transport_group(ui, egui_ctx, setter, params, ed, full_w);
    }

    // 6. Export WAV: full-width primary button
    let has_sel = ed.selection.is_some();
    if !compact && ui.available_height() >= H_EXPORT {
        if draw_export_button(ui, full_w, H_EXPORT, has_sel) {
            ed.status = export_selection(ed);
        }
    }

    // Overflow "More ▾" menu button pinned above the meter when overflow_tier1 is active
    if overflow_tier1 && ui.available_height() >= H_MORE_BTN {
        draw_more_menu(
            ui,
            egui_ctx,
            setter,
            params,
            ed,
            full_w,
            overflow_tier2,
        );
    }

    // 7. Meter: "dB" label, thin horizontal meter with 3 px peak-hold tick, numeric value
    if !compact && ui.available_height() >= H_METER {
        draw_meter_section(ui, full_w, db, cur_norm, peak_hold_norm);
    }

    // 8. Status message: single line, 10 px muted, truncated with ellipsis (inline unless overflow_tier1)
    if !compact && !overflow_tier1 && ui.available_height() >= H_STATUS_LINE {
        draw_single_line_status(ui, full_w, &ed.status);
    }

    // Whatever panel space is left (and never a button) is a window drag handle.
    let rest = ui.available_rect_before_wrap();
    if rest.height() >= 4.0 && rest.width() >= 20.0 {
        let resp = ui.interact(rest, ui.id().with("panel_drag"), egui::Sense::drag());
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        if resp.drag_started() {
            crate::chrome::begin_move();
        }
    }
}

/// Small square minimize button with a painted minus glyph. Returns true on click.
fn draw_minimize_button(ui: &mut egui::Ui, rect: egui::Rect, id: &str) -> bool {
    let resp = ui.interact(rect, ui.id().with(id), egui::Sense::click());
    let painter = ui.painter();
    let (fill, stroke) = if resp.hovered() {
        (theme::HOVER_FILL, theme::TEXT)
    } else {
        (Color32::TRANSPARENT, theme::STROKE)
    };
    painter.rect_filled(rect, egui::CornerRadius::same(0), fill);
    painter.rect_stroke(
        rect,
        egui::CornerRadius::same(0),
        egui::Stroke::new(1.0_f32, stroke),
        egui::StrokeKind::Inside,
    );
    let glyph = (rect.width() * 0.28).clamp(2.0, 4.0);
    let cy = rect.center().y;
    painter.line_segment(
        [
            egui::pos2(rect.center().x - glyph, cy),
            egui::pos2(rect.center().x + glyph, cy),
        ],
        egui::Stroke::new(
            1.5_f32,
            if resp.hovered() { theme::TEXT } else { theme::MUTED },
        ),
    );
    resp.clicked()
}

/// Compact transport: two square buttons per row, Pause first. Used when the
/// window is shorter than a full control stack.
fn draw_compact_transport(
    ui: &mut egui::Ui,
    setter: &ParamSetter,
    params: &Arc<PirateSamplerParams>,
    ed: &mut EditorState,
    width: f32,
) {
    let btn_h = 24.0;
    let gap = 6.0;
    let half = ((width - gap) * 0.5).max(24.0);
    let paused = params.pause.value();
    let pause_label = if paused { "Resume" } else { "Pause" };
    let has_sel = ed.selection.is_some();

    let rows: [(&str, &str, bool, bool); 4] = [
        ("tr_pause", pause_label, true, paused),
        ("tr_freeze", "Freeze", true, ed.freeze_view),
        ("tr_copy", "Copy", has_sel, false),
        ("tr_clips", "Clips", true, false),
    ];

    for pair in rows.chunks(2) {
        if ui.available_height() < btn_h {
            break;
        }
        let (row_rect, _) =
            ui.allocate_exact_size(egui::vec2(width, btn_h), egui::Sense::hover());
        for (index, (id, label, enabled, engaged)) in pair.iter().enumerate() {
            let x = row_rect.left() + index as f32 * (half + gap);
            let rect = egui::Rect::from_min_size(
                egui::pos2(x, row_rect.top()),
                egui::vec2(half, btn_h),
            );
            if draw_transport_button_in_rect(ui, rect, id, label, *enabled, *engaged) {
                match *id {
                    "tr_pause" => {
                        setter.begin_set_parameter(&params.pause);
                        setter.set_parameter(&params.pause, !paused);
                        setter.end_set_parameter(&params.pause);
                    }
                    "tr_freeze" => {
                        ed.freeze_view = !ed.freeze_view;
                        if !ed.freeze_view {
                            ed.frozen_peaks = None;
                        }
                    }
                    "tr_copy" => ed.status = copy_selection_to_clipboard(ed, ui.ctx()),
                    _ => ed.status = open_clips_folder(),
                }
            }
        }
    }
}

/// Draws a crisp monochrome vector skull-and-crossbones mark scaled to `badge_rect`.
fn draw_logo_mark(painter: &egui::Painter, badge_rect: egui::Rect) {
    painter.rect_filled(
        badge_rect,
        egui::CornerRadius::same(4),
        theme::CARD,
    );
    painter.rect_stroke(
        badge_rect,
        egui::CornerRadius::same(4),
        egui::Stroke::new(1.0_f32, theme::STROKE),
        egui::StrokeKind::Inside,
    );

    let c = badge_rect.center();
    let scale = (badge_rect.width() / 28.0).clamp(0.5, 1.5);
    let bone_stroke = egui::Stroke::new((1.4 * scale).max(1.0), theme::TEXT);

    let d = 7.5 * scale;
    painter.line_segment(
        [
            egui::pos2(c.x - d, c.y - d + 0.8 * scale),
            egui::pos2(c.x + d, c.y + d - 0.8 * scale),
        ],
        bone_stroke,
    );
    painter.line_segment(
        [
            egui::pos2(c.x - d, c.y + d - 0.8 * scale),
            egui::pos2(c.x + d, c.y - d + 0.8 * scale),
        ],
        bone_stroke,
    );

    let skull_c = egui::pos2(c.x, c.y - 1.2 * scale);
    painter.circle_filled(skull_c, 5.4 * scale, theme::CARD);
    painter.circle_filled(skull_c, 4.5 * scale, theme::TEXT);
    let jaw_rect = egui::Rect::from_center_size(
        egui::pos2(c.x, c.y + 3.2 * scale),
        egui::vec2(4.8 * scale, 3.0 * scale),
    );
    painter.rect_filled(jaw_rect, egui::CornerRadius::same(1), theme::TEXT);

    painter.circle_filled(
        egui::pos2(c.x - 1.7 * scale, c.y - 1.4 * scale),
        (1.2 * scale).max(0.9),
        theme::BG,
    );
    painter.circle_filled(
        egui::pos2(c.x + 1.7 * scale, c.y - 1.4 * scale),
        (1.2 * scale).max(0.9),
        theme::BG,
    );
}

fn draw_stacked_source_toggle(ui: &mut egui::Ui, ed: &mut EditorState, width: f32) {
    let seg_h = 24.0;
    let gap = 6.0;
    let (container_rect, _) = ui.allocate_exact_size(
        egui::vec2(width, seg_h * 2.0 + gap),
        egui::Sense::hover(),
    );

    let sys_rect = egui::Rect::from_min_size(container_rect.min, egui::vec2(width, seg_h));
    let mic_rect = egui::Rect::from_min_size(
        egui::pos2(container_rect.left(), container_rect.top() + seg_h + gap),
        egui::vec2(width, seg_h),
    );

    let sys_resp = ui.interact(sys_rect, ui.id().with("src_sys"), egui::Sense::click());
    let mic_resp = ui.interact(mic_rect, ui.id().with("src_mic"), egui::Sense::click());

    if sys_resp.hovered() || mic_resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    let painter = ui.painter();
    for (rect, resp, source, label) in [
        (sys_rect, &sys_resp, AudioInputSource::SystemAudio, "System Audio"),
        (mic_rect, &mic_resp, AudioInputSource::Microphone, "Microphone"),
    ] {
        let active = ed.input_source == source;
        if active {
            let fill = if resp.hovered() {
                theme::ACCENT_HOVER
            } else {
                theme::ACCENT
            };
            painter.rect_filled(rect, egui::CornerRadius::same(theme::CONTROL_RADIUS), fill);
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                label,
                theme::font_small(),
                theme::BG,
            );
        } else {
            if resp.hovered() {
                painter.rect_filled(
                    rect,
                    egui::CornerRadius::same(theme::CONTROL_RADIUS),
                    theme::HOVER_FILL,
                );
            }
            let stroke_col = if resp.hovered() {
                theme::ACCENT_HOVER
            } else {
                theme::STROKE
            };
            let text_col = if resp.hovered() {
                theme::TEXT
            } else {
                theme::MUTED
            };
            painter.rect_stroke(
                rect,
                egui::CornerRadius::same(theme::CONTROL_RADIUS),
                egui::Stroke::new(1.0_f32, stroke_col),
                egui::StrokeKind::Inside,
            );
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                label,
                theme::font_small(),
                text_col,
            );
        }
    }

    if sys_resp.clicked() && ed.input_source != AudioInputSource::SystemAudio {
        ed.input_source = AudioInputSource::SystemAudio;
        ed.status = switch_input_source(&mut ed.active_links, AudioInputSource::SystemAudio);
    } else if mic_resp.clicked() && ed.input_source != AudioInputSource::Microphone {
        ed.input_source = AudioInputSource::Microphone;
        ed.status = switch_input_source(&mut ed.active_links, AudioInputSource::Microphone);
    }
}

fn draw_buffer_section(
    ui: &mut egui::Ui,
    setter: &ParamSetter,
    params: &Arc<PirateSamplerParams>,
    width: f32,
) {
    let (sec_rect, _) =
        ui.allocate_exact_size(egui::vec2(width, H_BUFFER), egui::Sense::hover());
    let label_rect = egui::Rect::from_min_size(sec_rect.min, egui::vec2(width, 14.0));
    let slider_rect = egui::Rect::from_min_size(
        egui::pos2(sec_rect.left(), sec_rect.top() + 16.0),
        egui::vec2(width, 18.0),
    );

    let mut secs = params.buffer_seconds.value();
    ui.painter().text(
        egui::pos2(label_rect.left(), label_rect.center().y),
        egui::Align2::LEFT_CENTER,
        "BUFFER",
        theme::font_tiny(),
        theme::MUTED,
    );
    ui.painter().text(
        egui::pos2(label_rect.right(), label_rect.center().y),
        egui::Align2::RIGHT_CENTER,
        format!("{:.0} s", secs),
        theme::font_small(),
        theme::TEXT,
    );

    if draw_buffer_slider_in_rect(ui, slider_rect, &mut secs, 30.0, 600.0) {
        setter.begin_set_parameter(&params.buffer_seconds);
        setter.set_parameter(&params.buffer_seconds, secs);
        setter.end_set_parameter(&params.buffer_seconds);
    }
}

fn draw_buffer_slider_in_rect(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    secs: &mut f32,
    min_s: f32,
    max_s: f32,
) -> bool {
    let handle_r = 6.0;
    let resp = ui.interact(
        rect,
        ui.id().with("monochrome_buf_slider"),
        egui::Sense::click_and_drag(),
    );
    if resp.hovered() || resp.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    let rail_left = rect.left() + handle_r + 1.0;
    let rail_right = rect.right() - handle_r - 1.0;
    let rail_w = (rail_right - rail_left).max(1.0);

    let mut changed = false;
    if resp.clicked() || resp.dragged() {
        if let Some(pos) = resp.interact_pointer_pos() {
            let t = ((pos.x - rail_left) / rail_w).clamp(0.0, 1.0);
            let next = (min_s + t * (max_s - min_s)).round().clamp(min_s, max_s);
            if (next - *secs).abs() >= 0.5 {
                *secs = next;
                changed = true;
            }
        }
    }

    let norm = ((*secs - min_s) / (max_s - min_s)).clamp(0.0, 1.0);
    let handle_x = rail_left + norm * rail_w;
    let cy = rect.center().y;
    let track_h = 4.0;

    let painter = ui.painter();

    // Track #2A2A2A
    let track_rect = egui::Rect::from_min_max(
        egui::pos2(rail_left, cy - track_h * 0.5),
        egui::pos2(rail_right, cy + track_h * 0.5),
    );
    painter.rect_filled(track_rect, egui::CornerRadius::same(2), theme::TRACK_FILL);

    // Active fill white (#FFFFFF)
    let active_rect = egui::Rect::from_min_max(
        egui::pos2(rail_left, cy - track_h * 0.5),
        egui::pos2(handle_x.max(rail_left + 1.5), cy + track_h * 0.5),
    );
    painter.rect_filled(active_rect, egui::CornerRadius::same(2), theme::ACCENT);

    // Square handle with a 1 px black ring (hard edges, like everything else)
    let handle_center = egui::pos2(handle_x, cy);
    let handle_col = if resp.hovered() || resp.dragged() {
        theme::ACCENT_HOVER
    } else {
        theme::ACCENT
    };
    let handle_rect = egui::Rect::from_center_size(
        handle_center,
        egui::vec2(handle_r * 2.0, handle_r * 2.0),
    );
    painter.rect_filled(handle_rect, egui::CornerRadius::same(0), handle_col);
    painter.rect_stroke(
        handle_rect,
        egui::CornerRadius::same(0),
        egui::Stroke::new(1.0_f32, theme::BG),
        egui::StrokeKind::Inside,
    );

    changed
}

fn draw_transport_group(
    ui: &mut egui::Ui,
    egui_ctx: &egui::Context,
    setter: &ParamSetter,
    params: &Arc<PirateSamplerParams>,
    ed: &mut EditorState,
    width: f32,
) {
    let btn_h = 24.0;
    let gap = 4.0;
    let (group_rect, _) =
        ui.allocate_exact_size(egui::vec2(width, H_TRANSPORT), egui::Sense::hover());

    let paused = params.pause.value();
    let pause_label = if paused { "Resume" } else { "Pause" };
    let has_sel = ed.selection.is_some();

    let r0 = egui::Rect::from_min_size(group_rect.min, egui::vec2(width, btn_h));
    let r1 = egui::Rect::from_min_size(
        egui::pos2(group_rect.left(), r0.bottom() + gap),
        egui::vec2(width, btn_h),
    );
    let r2 = egui::Rect::from_min_size(
        egui::pos2(group_rect.left(), r1.bottom() + gap),
        egui::vec2(width, btn_h),
    );
    let r3 = egui::Rect::from_min_size(
        egui::pos2(group_rect.left(), r2.bottom() + gap),
        egui::vec2(width, btn_h),
    );

    if draw_transport_button_in_rect(ui, r0, "tr_pause", pause_label, true, paused) {
        setter.begin_set_parameter(&params.pause);
        setter.set_parameter(&params.pause, !paused);
        setter.end_set_parameter(&params.pause);
    }

    if draw_transport_button_in_rect(ui, r1, "tr_freeze", "Freeze (H)", true, ed.freeze_view) {
        ed.freeze_view = !ed.freeze_view;
        if !ed.freeze_view {
            ed.frozen_peaks = None;
        }
    }

    if draw_transport_button_in_rect(ui, r2, "tr_copy", "Copy", has_sel, false) {
        ed.status = copy_selection_to_clipboard(ed, egui_ctx);
    }

    if draw_transport_button_in_rect(ui, r3, "tr_clips", "Clips", true, false) {
        ed.status = open_clips_folder();
    }
}

fn draw_transport_button_in_rect(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    id_salt: &str,
    label: &str,
    enabled: bool,
    engaged: bool,
) -> bool {
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let resp = ui.interact(rect, ui.id().with(id_salt), sense);
    if enabled && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    let painter = ui.painter();
    if engaged {
        let fill = if resp.hovered() {
            theme::ACCENT_HOVER
        } else {
            theme::ACCENT
        };
        painter.rect_filled(rect, egui::CornerRadius::same(theme::CONTROL_RADIUS), fill);
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            theme::font_small(),
            theme::BG,
        );
    } else {
        let fill = if !enabled {
            theme::HOVER_FILL
        } else if resp.hovered() {
            theme::HOVER_FILL
        } else {
            theme::PANEL
        };
        let stroke_col = if !enabled {
            theme::DISABLED_STROKE
        } else if resp.hovered() {
            theme::ACCENT_HOVER
        } else {
            theme::STROKE
        };
        let text_col = if !enabled {
            theme::DISABLED_TEXT
        } else {
            theme::TEXT
        };
        painter.rect_filled(rect, egui::CornerRadius::same(theme::CONTROL_RADIUS), fill);
        painter.rect_stroke(
            rect,
            egui::CornerRadius::same(theme::CONTROL_RADIUS),
            egui::Stroke::new(1.0_f32, stroke_col),
            egui::StrokeKind::Inside,
        );
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            theme::font_small(),
            text_col,
        );
    }

    enabled && resp.clicked()
}

fn draw_export_button(ui: &mut egui::Ui, width: f32, height: f32, enabled: bool) -> bool {
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, height), sense);

    if enabled && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    let painter = ui.painter();
    if enabled {
        let fill = if resp.hovered() {
            theme::ACCENT_HOVER
        } else {
            theme::ACCENT
        };
        painter.rect_filled(rect, egui::CornerRadius::same(theme::CONTROL_RADIUS), fill);
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Export WAV",
            theme::font_body(),
            theme::BG,
        );
    } else {
        // Disabled (no selection) = #1A1A1A fill, #3A3A3A border, #6A6A6A text, no hover, not clickable
        painter.rect_filled(
            rect,
            egui::CornerRadius::same(theme::CONTROL_RADIUS),
            theme::HOVER_FILL,
        );
        painter.rect_stroke(
            rect,
            egui::CornerRadius::same(theme::CONTROL_RADIUS),
            egui::Stroke::new(1.0_f32, theme::DISABLED_STROKE),
            egui::StrokeKind::Inside,
        );
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Export WAV",
            theme::font_body(),
            theme::DISABLED_TEXT,
        );
    }

    enabled && resp.clicked()
}

fn draw_more_menu(
    ui: &mut egui::Ui,
    egui_ctx: &egui::Context,
    setter: &ParamSetter,
    params: &Arc<PirateSamplerParams>,
    ed: &mut EditorState,
    width: f32,
    include_buffer: bool,
) {
    ui.allocate_ui_with_layout(
        egui::vec2(width, H_MORE_BTN),
        egui::Layout::top_down_justified(egui::Align::Center),
        |ui| {
            let menu_resp = ui.menu_button("More   ", |ui| {
                ui.set_min_width(148.0);

                if include_buffer {
                    draw_buffer_section(ui, setter, params, 148.0);
                    ui.separator();
                }

                let paused = params.pause.value();
                let pause_label = if paused { "Resume" } else { "Pause" };
                if ui.button(pause_label).clicked() {
                    setter.begin_set_parameter(&params.pause);
                    setter.set_parameter(&params.pause, !paused);
                    setter.end_set_parameter(&params.pause);
                    ui.close_menu();
                }

                let freeze_label = if ed.freeze_view {
                    "Unfreeze (H)"
                } else {
                    "Freeze (H)"
                };
                if ui.button(freeze_label).clicked() {
                    ed.freeze_view = !ed.freeze_view;
                    if !ed.freeze_view {
                        ed.frozen_peaks = None;
                    }
                    ui.close_menu();
                }

                let has_sel = ed.selection.is_some();
                if ui.add_enabled(has_sel, egui::Button::new("Copy")).clicked() {
                    ed.status = copy_selection_to_clipboard(ed, egui_ctx);
                    ui.close_menu();
                }

                if ui.button("Clips").clicked() {
                    ed.status = open_clips_folder();
                    ui.close_menu();
                }

                ui.separator();
                draw_single_line_status(ui, 148.0, &ed.status);
            });

            // Paint a crisp vector downward chevron (two lines) instead of U+25BE
            let btn_rect = menu_resp.response.rect;
            let cx = btn_rect.center().x + 18.0;
            let cy = btn_rect.center().y + 0.5;
            let chev_stroke = egui::Stroke::new(1.3_f32, theme::TEXT);
            ui.painter().line_segment(
                [egui::pos2(cx - 3.5, cy - 1.8), egui::pos2(cx, cy + 1.8)],
                chev_stroke,
            );
            ui.painter().line_segment(
                [egui::pos2(cx, cy + 1.8), egui::pos2(cx + 3.5, cy - 1.8)],
                chev_stroke,
            );
        },
    );
}

fn draw_meter_section(
    ui: &mut egui::Ui,
    width: f32,
    db: f32,
    cur_norm: f32,
    peak_hold_norm: f32,
) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, H_METER), egui::Sense::hover());
    let painter = ui.painter();

    let db_str = if db > -55.0 {
        format!("{:+.1} dB", db)
    } else {
        "-inf dB".to_string()
    };
    let db_col = if db > -55.0 {
        theme::TEXT
    } else {
        theme::MUTED
    };

    // One row: "dB" label, the meter track, then the numeric value right-aligned.
    // Keeping them on a single line stops the bar from colliding with the value
    // when the panel is short.
    let row = egui::Rect::from_min_size(rect.min, egui::vec2(width, 14.0));
    painter.text(
        egui::pos2(row.left(), row.center().y),
        egui::Align2::LEFT_CENTER,
        "dB",
        theme::font_tiny(),
        theme::MUTED,
    );
    painter.text(
        egui::pos2(row.right(), row.center().y),
        egui::Align2::RIGHT_CENTER,
        db_str,
        theme::font_tiny(),
        db_col,
    );

    let bar_rect = egui::Rect::from_min_max(
        egui::pos2(row.left() + 20.0, row.center().y - 3.0),
        egui::pos2(row.right() - 56.0, row.center().y + 3.0),
    );

    // Thin horizontal meter: white fill over #2A2A2A track, with a 3 px peak-hold tick
    painter.rect_filled(bar_rect, egui::CornerRadius::ZERO, theme::TRACK_FILL);
    if cur_norm > 0.01 {
        let fill_w = (bar_rect.width() * cur_norm).clamp(2.0, bar_rect.width());
        let fill_rect = egui::Rect::from_min_size(bar_rect.min, egui::vec2(fill_w, bar_rect.height()));
        painter.rect_filled(fill_rect, egui::CornerRadius::ZERO, theme::TEXT);
    }

    if peak_hold_norm > 0.02 {
        let tick_w = 3.0;
        let tick_x = (bar_rect.left() + bar_rect.width() * peak_hold_norm - tick_w)
            .clamp(bar_rect.left(), bar_rect.right() - tick_w);
        let tick_rect = egui::Rect::from_min_size(
            egui::pos2(tick_x, bar_rect.top()),
            egui::vec2(tick_w, bar_rect.height()),
        );
        painter.rect_filled(tick_rect, egui::CornerRadius::ZERO, theme::TEXT);
    }
}

fn draw_single_line_status(ui: &mut egui::Ui, width: f32, status: &str) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(width, H_STATUS_LINE), egui::Sense::hover());
    let truncated = truncate_status_to_width(ui.painter(), status, width, theme::font_tiny());
    ui.painter().text(
        egui::pos2(rect.left(), rect.center().y),
        egui::Align2::LEFT_CENTER,
        truncated,
        theme::font_tiny(),
        theme::MUTED,
    );
}

fn truncate_status_to_width(
    painter: &egui::Painter,
    text: &str,
    max_w: f32,
    font: egui::FontId,
) -> String {
    let single_line = text.lines().next().unwrap_or("");
    let full_galley = painter.layout_no_wrap(single_line.to_string(), font.clone(), theme::MUTED);
    if full_galley.size().x <= max_w {
        return single_line.to_string();
    }
    let chars: Vec<char> = single_line.chars().collect();
    let mut lo = 0usize;
    let mut hi = chars.len();
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        let candidate: String = chars[..mid].iter().collect::<String>() + "...";
        let g = painter.layout_no_wrap(candidate, font.clone(), theme::MUTED);
        if g.size().x <= max_w {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    chars[..lo].iter().collect::<String>() + "..."
}

#[allow(clippy::too_many_arguments)]
fn draw_waveform_card(
    ui: &mut egui::Ui,
    ed: &mut EditorState,
    card_h: f32,
    filled_frames: usize,
    cap_secs: f32,
    sr: f32,
    is_empty: bool,
    paused: bool,
) {
    let (card_rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), card_h),
        egui::Sense::click_and_drag(),
    );
    let painter = ui.painter_at(card_rect);

    // The waveform lives directly on the panel surface: no card fill, no frame.
    // Keeping it bare is what makes the window read as one solid element.

    // The plot uses the full height of the window: no inner padding and no ruler
    // strip, so the waveform is the tallest thing in the app.
    let plot_rect = card_rect;

    // Plot area shares the panel surface; only the grid and zero line define it.

    let mid = plot_rect.center().y;
    // Full-scale peaks reach the top and bottom edges (2 px breathing room), so the
    // waveform fills the window instead of floating inside it.
    let half_h = (plot_rect.height() * 0.5 - 2.0).max(6.0);

    // Grey horizontal grid (#242424)
    for &frac in &[-0.75_f32, -0.5, -0.25, 0.25, 0.5, 0.75] {
        let y = mid + frac * half_h;
        painter.line_segment(
            [
                egui::pos2(plot_rect.left() + 1.0, y),
                egui::pos2(plot_rect.right() - 1.0, y),
            ],
            egui::Stroke::new(1.0_f32, theme::GRID_LINE),
        );
    }

    // Time ruler underneath + vertical grid lines

    // Zero line (#444444)
    painter.line_segment(
        [
            egui::pos2(plot_rect.left() + 1.0, mid),
            egui::pos2(plot_rect.right() - 1.0, mid),
        ],
        egui::Stroke::new(1.0_f32, theme::ZERO_LINE),
    );

    // Waveform peaks
    let num_buckets = (plot_rect.width().max(32.0) as usize).clamp(32, 2048);
    let peaks = if ed.freeze_view {
        let needs_snap = ed
            .frozen_peaks
            .as_ref()
            .map_or(true, |v| v.len() != num_buckets);
        if needs_snap {
            let snap = ed.ring.lock().waveform_peaks(num_buckets);
            ed.frozen_peaks = Some(snap);
        }
        ed.frozen_peaks.clone().unwrap_or_default()
    } else {
        ed.ring.lock().waveform_peaks(num_buckets)
    };

    // White envelope fill (alpha gradient from 0.85 to 0.35) + thin white contour stroke
    // Drawn whenever card_h >= 80.0, with amplitude normalised to the plot rect.
    if is_empty || peaks.is_empty() {
        painter.text(
            plot_rect.center(),
            egui::Align2::CENTER_CENTER,
            "Waiting for audio signal...",
            theme::font_body(),
            theme::MUTED,
        );
    } else if card_rect.height() >= 80.0 {
        let n = peaks.len() as f32;
        let max_peak = peaks
            .iter()
            .fold(0.0_f32, |acc, &(l, r)| acc.max(l.max(r)));
        let outer_fill = Color32::from_rgba_unmultiplied(0xFF, 0xFF, 0xFF, 89); // 0.35 alpha
        let mid_fill = Color32::from_rgba_unmultiplied(0xFF, 0xFF, 0xFF, 153); // 0.60 alpha
        let core_fill = Color32::from_rgba_unmultiplied(0xFF, 0xFF, 0xFF, 217); // 0.85 alpha
        let contour_stroke = egui::Stroke::new(1.0_f32, theme::TEXT);

        let mut prev_top: Option<egui::Pos2> = None;
        let mut prev_bot: Option<egui::Pos2> = None;

        for (i, (l, r)) in peaks.iter().enumerate() {
            let x = plot_rect.left() + (i as f32 + 0.5) / n * plot_rect.width();
            let raw_amp = ((*l).max(*r)).clamp(0.0, 1.0);
            let norm_amp = if max_peak > 1e-6 {
                (raw_amp / max_peak).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let bar_h = if norm_amp > 0.001 {
                (norm_amp * half_h).max(1.5)
            } else {
                1.0
            };
            let y0 = mid - bar_h;
            let y1 = mid + bar_h;

            painter.line_segment(
                [egui::pos2(x, y0), egui::pos2(x, y1)],
                egui::Stroke::new(1.25_f32, outer_fill),
            );
            let mid_h = (bar_h * 0.68).max(0.8);
            painter.line_segment(
                [egui::pos2(x, mid - mid_h), egui::pos2(x, mid + mid_h)],
                egui::Stroke::new(1.25_f32, mid_fill),
            );
            let core_h = (bar_h * 0.36).max(0.6);
            painter.line_segment(
                [egui::pos2(x, mid - core_h), egui::pos2(x, mid + core_h)],
                egui::Stroke::new(1.25_f32, core_fill),
            );

            let top_pt = egui::pos2(x, y0);
            let bot_pt = egui::pos2(x, y1);
            if let (Some(pt), Some(pb)) = (prev_top, prev_bot) {
                painter.line_segment([pt, top_pt], contour_stroke);
                painter.line_segment([pb, bot_pt], contour_stroke);
            }
            prev_top = Some(top_pt);
            prev_bot = Some(bot_pt);
        }
    }

    // Pointer interaction & cursor feedback
    let pointer_pos = response.hover_pos().or_else(|| response.interact_pointer_pos());
    let edge_grab_px = 8.0;
    let mut hovering_edge = false;

    if let (Some(pos), Some((a, b))) = (pointer_pos, ed.selection) {
        if plot_rect.contains(pos) {
            let x0 = plot_rect.left() + a.min(b) * plot_rect.width();
            let x1 = plot_rect.left() + a.max(b) * plot_rect.width();
            if (pos.x - x0).abs() <= edge_grab_px || (pos.x - x1).abs() <= edge_grab_px {
                hovering_edge = true;
            }
        }
    }

    if response.dragged() || hovering_edge {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    } else if let Some(pos) = pointer_pos {
        if plot_rect.contains(pos) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
    }

    if let Some(pos) = response.interact_pointer_pos() {
        let t = ((pos.x - plot_rect.left()) / plot_rect.width()).clamp(0.0, 1.0);
        if response.drag_started() && plot_rect.contains(pos) {
            if let Some((a, b)) = ed.selection {
                let start_n = a.min(b);
                let end_n = a.max(b);
                let x0 = plot_rect.left() + start_n * plot_rect.width();
                let x1 = plot_rect.left() + end_n * plot_rect.width();
                if (pos.x - x0).abs() <= edge_grab_px {
                    ed.drag_anchor = Some(end_n);
                    ed.selection = Some((t, end_n));
                } else if (pos.x - x1).abs() <= edge_grab_px {
                    ed.drag_anchor = Some(start_n);
                    ed.selection = Some((start_n, t));
                } else {
                    ed.drag_anchor = Some(t);
                    ed.selection = Some((t, t));
                }
            } else {
                ed.drag_anchor = Some(t);
                ed.selection = Some((t, t));
            }
        } else if response.dragged() {
            if let Some(anchor) = ed.drag_anchor {
                ed.selection = Some((anchor, t));
            }
        } else if response.drag_stopped() {
            if let Some((a, b)) = ed.selection {
                if (b - a).abs() * plot_rect.width() < 2.0 {
                    ed.selection = None;
                }
            }
            ed.drag_anchor = None;
        }
    }

    // Translucent white selection band + 1 px white border + white edge drag handles + mm:ss.mmm badge
    if let Some((a, b)) = ed.selection {
        let start_n = a.min(b);
        let end_n = a.max(b);
        let x0 = plot_rect.left() + start_n * plot_rect.width();
        let x1 = (plot_rect.left() + end_n * plot_rect.width()).max(x0 + 2.0);
        let sel_rect = egui::Rect::from_min_max(
            egui::pos2(x0, plot_rect.top() + 1.0),
            egui::pos2(x1, plot_rect.bottom() - 1.0),
        );

        painter.rect_filled(
            sel_rect,
            egui::CornerRadius::ZERO,
            Color32::from_rgba_unmultiplied(0xFF, 0xFF, 0xFF, 40),
        );
        painter.rect_stroke(
            sel_rect,
            egui::CornerRadius::ZERO,
            egui::Stroke::new(1.0_f32, theme::ACCENT),
            egui::StrokeKind::Inside,
        );

        // White edge drag handles
        let handle_w = 6.0;
        // Never taller than the plot itself, so the handles stay inside a short window.
        let handle_h = 28.0_f32.min((plot_rect.height() - 6.0).max(8.0));
        let handle_color = if hovering_edge || response.dragged() {
            theme::ACCENT_HOVER
        } else {
            theme::ACCENT
        };
        for &edge_x in &[x0, x1] {
            let h_rect = egui::Rect::from_center_size(
                egui::pos2(
                    edge_x.clamp(plot_rect.left() + 3.0, plot_rect.right() - 3.0),
                    mid,
                ),
                egui::vec2(handle_w, handle_h),
            );
            painter.rect_filled(h_rect, egui::CornerRadius::ZERO, handle_color);
            painter.line_segment(
                [
                    egui::pos2(h_rect.center().x, h_rect.top() + 6.0),
                    egui::pos2(h_rect.center().x, h_rect.bottom() - 6.0),
                ],
                egui::Stroke::new(1.0_f32, theme::BG),
            );
        }

        // mm:ss.mmm badge above selection
        let start_frame = (start_n * filled_frames as f32).floor() as usize;
        let end_frame = ((end_n * filled_frames as f32).ceil() as usize).min(filled_frames);
        let sel_frames = end_frame.saturating_sub(start_frame);
        let sel_secs = if filled_frames > 0 {
            sel_frames as f32 / sr.max(1.0)
        } else {
            (end_n - start_n) * cap_secs
        };
        let label_str = format_mm_ss_mmm(sel_secs);
        let galley = painter.layout_no_wrap(label_str, theme::font_small(), theme::TEXT);
        let pill_w = galley.size().x + 14.0;
        let pill_h = 20.0;
        let center_x = ((x0 + x1) * 0.5).clamp(
            plot_rect.left() + pill_w * 0.5 + 6.0,
            plot_rect.right() - pill_w * 0.5 - 6.0,
        );
        let pill_rect = egui::Rect::from_center_size(
            egui::pos2(center_x, plot_rect.top() + 15.0),
            egui::vec2(pill_w, pill_h),
        );
        painter.rect_filled(
            pill_rect,
            egui::CornerRadius::same(theme::CONTROL_RADIUS),
            theme::BG,
        );
        painter.rect_stroke(
            pill_rect,
            egui::CornerRadius::same(theme::CONTROL_RADIUS),
            egui::Stroke::new(1.0_f32, theme::TEXT),
            egui::StrokeKind::Inside,
        );
        painter.galley(
            egui::pos2(
                pill_rect.center().x - galley.size().x * 0.5,
                pill_rect.center().y - galley.size().y * 0.5,
            ),
            galley,
            theme::TEXT,
        );
    }

    // PAUSED / FROZEN stamp in white outline
    if paused || ed.freeze_view {
        let stamp_text = if paused { "PAUSED" } else { "FROZEN" };
        let galley = painter.layout_no_wrap(
            stamp_text.to_string(),
            theme::font_small(),
            theme::TEXT,
        );
        let stamp_w = galley.size().x + 20.0;
        let stamp_h = 24.0;
        let stamp_rect = egui::Rect::from_min_size(
            egui::pos2(plot_rect.right() - stamp_w - 10.0, plot_rect.top() + 10.0),
            egui::vec2(stamp_w, stamp_h),
        );
        painter.rect_filled(
            stamp_rect,
            egui::CornerRadius::same(theme::CONTROL_RADIUS),
            theme::BG,
        );
        painter.rect_stroke(
            stamp_rect,
            egui::CornerRadius::same(theme::CONTROL_RADIUS),
            egui::Stroke::new(1.0_f32, theme::TEXT),
            egui::StrokeKind::Inside,
        );
        painter.galley(
            egui::pos2(
                stamp_rect.center().x - galley.size().x * 0.5,
                stamp_rect.center().y - galley.size().y * 0.5,
            ),
            galley,
            theme::TEXT,
        );
    }
}

fn format_mm_ss_mmm(seconds: f32) -> String {
    let total_ms = (seconds.max(0.0) * 1000.0).round() as u64;
    let mins = total_ms / 60_000;
    let secs = (total_ms % 60_000) / 1000;
    let millis = total_ms % 1000;
    format!("{:02}:{:02}.{:03}", mins, secs, millis)
}

fn resolve_selection(ed: &EditorState) -> Result<(Selection, f32), String> {
    let Some((a, b)) = ed.selection else {
        return Err("No selection active.".to_string());
    };
    let start_n = a.min(b);
    let end_n = a.max(b);
    if (end_n - start_n) < 0.000_1 {
        return Err("Selection too small.".to_string());
    }

    let ring = ed.ring.lock();
    let filled = ring.filled_frames();
    if filled == 0 {
        return Err("Buffer is empty.".to_string());
    }
    let start = (start_n * filled as f32).floor() as usize;
    let end = ((end_n * filled as f32).ceil() as usize).min(filled).max(start + 1);
    let sel = Selection::new(start, end).ok_or_else(|| "Invalid selection bounds.".to_string())?;
    let sr = ring.sample_rate();
    Ok((sel, sr))
}

fn get_clips_dir() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    let clips = base.join("pirate-sampler").join("clips");
    let _ = std::fs::create_dir_all(&clips);
    clips
}

fn open_clips_folder() -> String {
    let dir = get_clips_dir();
    let _ = std::process::Command::new("xdg-open").arg(&dir).spawn();
    format!("Opened {}", dir.display())
}

fn copy_selection_to_clipboard(ed: &EditorState, ctx: &egui::Context) -> String {
    let (sel, sr) = match resolve_selection(ed) {
        Ok(s) => s,
        Err(e) => return e,
    };

    let dir = get_clips_dir();
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let filename = format!("clip_{:.2}s_{}.wav", sel.len() as f32 / sr, timestamp);
    let file_path = dir.join(&filename);

    let ring = ed.ring.lock();
    if let Err(e) = write_selection_wav(&ring, sel, &file_path) {
        return format!("Failed to render clip: {e}");
    }
    drop(ring);

    let latest_path = dir.join("latest.wav");
    let _ = std::fs::copy(&file_path, &latest_path);

    let canonical = file_path.canonicalize().unwrap_or(file_path);
    let uri = format!("file://{}\r\n", canonical.display());
    let path_str = canonical.display().to_string();

    ctx.copy_text(uri.clone());

    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        let _ = std::process::Command::new("wl-copy")
            .args(["-t", "text/uri-list", &uri])
            .spawn();
    }

    if std::env::var_os("DISPLAY").is_some() {
        if let Ok(mut child) = std::process::Command::new("xclip")
            .args(["-selection", "clipboard", "-t", "text/uri-list"])
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                use std::io::Write;
                let _ = stdin.write_all(uri.as_bytes());
            }
        }
        if let Ok(mut child) = std::process::Command::new("xclip")
            .args(["-selection", "clipboard"])
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                use std::io::Write;
                let _ = stdin.write_all(path_str.as_bytes());
            }
        }
    }

    format!("Copied {}", filename)
}

fn export_selection(ed: &EditorState) -> String {
    let (sel, sr) = match resolve_selection(ed) {
        Ok(s) => s,
        Err(e) => return e,
    };

    let default_name = format!("pirate-sampler-{:.0}s.wav", sel.len() as f32 / sr);

    let path = rfd::FileDialog::new()
        .set_title("Export selection")
        .set_file_name(&default_name)
        .add_filter("WAV", &["wav"])
        .save_file();

    let Some(path) = path else {
        return "Export cancelled.".to_string();
    };

    let ring = ed.ring.lock();
    match write_selection_wav(&ring, sel, &path) {
        Ok(()) => format!("Wrote {}", path.display()),
        Err(e) => format!("Export failed: {e}"),
    }
}

/// Apply duration param to the ring without reallocating (SPEC duration change).
pub fn apply_duration(ring: &mut RingBuffer, seconds: f32) {
    let _ = ring.set_duration_rt_safe(BufferDuration::new(seconds));
}

/// Dynamically switch audio inputs (PipeWire / JACK standalone mode).
pub fn switch_input_source(
    active_links: &mut Vec<(String, String)>,
    source: AudioInputSource,
) -> String {
    let inputs = match std::process::Command::new("pw-link").arg("-i").output() {
        Ok(o) => String::from_utf8_lossy(&o.stdout).to_string(),
        Err(_) => {
            return "Standalone PipeWire/JACK required".to_string()
        }
    };

    if !inputs.contains("pirate_sampler:input_1") {
        return "Host track routing active".to_string();
    }

    for (src, dst) in active_links.drain(..) {
        let _ = std::process::Command::new("pw-link")
            .args(["-d", &src, &dst])
            .output();
    }

    if let Ok(links_output) = std::process::Command::new("pw-link").arg("-l").output() {
        let text = String::from_utf8_lossy(&links_output.stdout);
        let mut cur_src = "";
        for line in text.lines() {
            let trimmed = line.trim();
            if !line.starts_with(' ') && !line.starts_with('\t') {
                cur_src = trimmed;
            } else if trimmed.starts_with("|->") {
                let target = trimmed.trim_start_matches("|->").trim();
                if target.starts_with("pirate_sampler:") || cur_src.starts_with("pirate_sampler:")
                {
                    let _ = std::process::Command::new("pw-link")
                        .args(["-d", cur_src, target])
                        .output();
                }
            }
        }
    }

    let outputs = match std::process::Command::new("pw-link").arg("-o").output() {
        Ok(o) => String::from_utf8_lossy(&o.stdout).to_string(),
        Err(_) => return "Failed to list audio sources".to_string(),
    };

    match source {
        AudioInputSource::SystemAudio => {
            let default_sink = std::process::Command::new("pactl")
                .arg("get-default-sink")
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_default();

            let mut mon_l = None;
            let mut mon_r = None;
            for line in outputs.lines() {
                let trimmed = line.trim();
                let is_default_monitor = !default_sink.is_empty()
                    && trimmed.starts_with(&default_sink)
                    && trimmed.contains("monitor");
                if (is_default_monitor && trimmed.contains("FL"))
                    || trimmed.contains("analog-stereo:monitor_FL")
                    || (trimmed.ends_with(":monitor_FL") && mon_l.is_none())
                {
                    mon_l = Some(trimmed.to_string());
                }
                if (is_default_monitor && trimmed.contains("FR"))
                    || trimmed.contains("analog-stereo:monitor_FR")
                    || (trimmed.ends_with(":monitor_FR") && mon_r.is_none())
                {
                    mon_r = Some(trimmed.to_string());
                }
            }

            if let (Some(l), Some(r)) = (mon_l, mon_r) {
                let _ = std::process::Command::new("pw-link")
                    .args(["-P", &l, "pirate_sampler:input_1"])
                    .output();
                let _ = std::process::Command::new("pw-link")
                    .args(["-P", &r, "pirate_sampler:input_2"])
                    .output();
                active_links.push((l.clone(), "pirate_sampler:input_1".to_string()));
                active_links.push((r.clone(), "pirate_sampler:input_2".to_string()));
                "Capturing System Audio".to_string()
            } else {
                "Monitor port not found".to_string()
            }
        }
        AudioInputSource::Microphone => {
            let default_mic_name = std::process::Command::new("pactl")
                .arg("get-default-source")
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_default();

            let mut mic_ports = Vec::new();
            for line in outputs.lines() {
                let trimmed = line.trim();
                if (trimmed.contains("capture") || trimmed.contains("input"))
                    && !trimmed.contains("monitor")
                    && !trimmed.contains("Midi")
                {
                    if !default_mic_name.is_empty() && trimmed.starts_with(&default_mic_name) {
                        mic_ports.insert(0, trimmed.to_string());
                    } else {
                        mic_ports.push(trimmed.to_string());
                    }
                }
            }

            if mic_ports.is_empty() {
                "No microphone detected".to_string()
            } else if mic_ports.len() == 1 || mic_ports[0].contains("mono") {
                let port = mic_ports[0].clone();
                let _ = std::process::Command::new("pw-link")
                    .args(["-P", &port, "pirate_sampler:input_1"])
                    .output();
                let _ = std::process::Command::new("pw-link")
                    .args(["-P", &port, "pirate_sampler:input_2"])
                    .output();
                active_links.push((port.clone(), "pirate_sampler:input_1".to_string()));
                active_links.push((port, "pirate_sampler:input_2".to_string()));
                "Capturing Microphone (Mono)".to_string()
            } else {
                let p1 = mic_ports[0].clone();
                let p2 = mic_ports[1].clone();
                let _ = std::process::Command::new("pw-link")
                    .args(["-P", &p1, "pirate_sampler:input_1"])
                    .output();
                let _ = std::process::Command::new("pw-link")
                    .args(["-P", &p2, "pirate_sampler:input_2"])
                    .output();
                active_links.push((p1, "pirate_sampler:input_1".to_string()));
                active_links.push((p2, "pirate_sampler:input_2".to_string()));
                "Capturing Microphone (Stereo)".to_string()
            }
        }
    }
}
