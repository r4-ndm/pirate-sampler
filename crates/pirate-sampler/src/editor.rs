//! Side-panel controls, layout constants, top-level `egui` frame orchestration,
//! and keyboard shortcuts.

use std::sync::Arc;

use nih_plug::prelude::*;
use nih_plug_egui::{
    create_egui_editor,
    egui::{self, Color32},
    EguiState,
};
use parking_lot::Mutex;
use pirate_sampler_dsp::{BufferDuration, RingBuffer};

use crate::clips::{copy_selection_to_clipboard, export_selection, open_clips_folder};
use crate::params::{PirateSamplerParams, PLAYBACK_GAIN_MAX_DB, PLAYBACK_GAIN_MIN_DB};
use crate::routing::{switch_input_source, AudioInputSource};
use crate::theme;
use crate::waveform::draw_waveform_card;

// Window and layout geometry constants (840 x 400 compact side-panel layout)
pub const WINDOW_W: f32 = crate::window_state::DEFAULT_WINDOW_SIZE.0 as f32;
pub const WINDOW_H: f32 = crate::window_state::DEFAULT_WINDOW_SIZE.1 as f32;
pub const SIDE_W: f32 = 176.0;
pub const MIN_WAVEFORM_W: f32 = 320.0;
pub const LEFT_MARGIN: i8 = 12;
pub const RIGHT_GAP: i8 = 8;
pub const TOP_MARGIN: i8 = 10;
pub const BOTTOM_MARGIN: i8 = 10;
pub const HORIZONTAL_MARGINS: f32 = (LEFT_MARGIN as f32) + (RIGHT_GAP as f32);
pub const VERTICAL_MARGINS: f32 = (TOP_MARGIN as f32) + (BOTTOM_MARGIN as f32);
pub const WAVEFORM_H: f32 = WINDOW_H - VERTICAL_MARGINS;

/// Compact playback-gain knob diameter, shared by both side-panel layouts.
const GAIN_KNOB_SIZE: f32 = 26.0;
const GAIN_KNOB_RADIUS: f32 = GAIN_KNOB_SIZE * 0.5;
/// Right-edge inset used by the buffer row's gain knob.
const GAIN_KNOB_INSET: f32 = 16.0;
/// Width reserved on the buffer row for the gain knob beside the slider.
const BUFFER_GAIN_RESERVE: f32 = 48.0;
/// Audio recorded while a scrub bake runs is parked in a ring this long.
const SCRUB_PARKING_SECONDS: f32 = 2.0;

// Side-panel block heights for deterministic overflow calculation
const H_BRAND: f32 = 24.0;
const H_SOURCE_TOGGLE: f32 = 54.0; // 24 + 6 + 24
const H_BUFFER: f32 = 34.0; // 14 label + 2 gap + 18 slider
const H_TRANSPORT: f32 = 108.0; // 4 * 24 + 3 * 4
const H_MORE_BTN: f32 = 24.0;
const H_EXPORT: f32 = 28.0;
const H_METER: f32 = 24.0; // 14 label + 2 gap + 8 bar
const H_STATUS_LINE: f32 = 14.0;
const SECTION_GAP: f32 = 6.0;

pub const FULL_STACK_NEEDED_H: f32 = H_BRAND
    + H_SOURCE_TOGGLE
    + H_BUFFER
    + H_TRANSPORT
    + H_EXPORT
    + H_METER
    + H_STATUS_LINE
    + 6.0 * SECTION_GAP;

pub const TIER1_STACK_NEEDED_H: f32 =
    H_BRAND + H_SOURCE_TOGGLE + H_BUFFER + H_EXPORT + H_MORE_BTN + H_METER + 5.0 * SECTION_GAP;

/// A left drag outside a playing loop that scrubs the captured audio under the fixed loop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoopSlide {
    /// Pointer position (normalized buffer time) where the drag started.
    pub anchor: f32,
    /// Frames of rotation already applied by this drag.
    pub applied: i64,
}

pub struct EditorState {
    pub ring: Arc<Mutex<RingBuffer>>,
    /// Small stand-in ring used while a scrub bake runs outside the shared lock.
    spare_ring: Option<RingBuffer>,
    /// Selection as normalized 0..1 over the filled buffer (start, end).
    pub selection: Option<(f32, f32)>,
    pub drag_anchor: Option<f32>,
    /// A left press landed inside the selection: a drag from there drags the clip out.
    pub selection_body_press: bool,
    /// A left press landed on a selection edge: the drag resizes the selection, and a running
    /// loop follows the new brace live.
    pub edge_drag: bool,
    /// Set while a drag outside the brace is scrubbing the audio under the loop.
    pub loop_slide: Option<LoopSlide>,
    /// Waveform view range in normalized buffer coordinates; may extend beyond 0.0..=1.0.
    pub view_range: (f32, f32),
    pub is_panning: bool,
    pub pan_start_pos: Option<f32>,
    pub pan_start_range: (f32, f32),
    /// Right-button press origin and whether it has moved far enough to count as a pan.
    pub sec_press: Option<(egui::Pos2, bool)>,
    pub status: String,
    pub freeze_view: bool,
    pub frozen_peaks: Option<Vec<(f32, f32)>>,
    pub input_source: AudioInputSource,
    pub active_links: Vec<(String, String)>,
    pub always_on_top: bool,
}

impl EditorState {
    pub fn new(ring: Arc<Mutex<RingBuffer>>) -> Self {
        Self {
            ring,
            spare_ring: None,
            selection: None,
            drag_anchor: None,
            selection_body_press: false,
            edge_drag: false,
            loop_slide: None,
            view_range: (0.0, 1.0),
            is_panning: false,
            pan_start_pos: None,
            pan_start_range: (0.0, 1.0),
            sec_press: None,
            status: "Ready - System Audio".to_string(),
            freeze_view: false,
            frozen_peaks: None,
            input_source: AudioInputSource::SystemAudio,
            active_links: Vec::new(),
            always_on_top: true,
        }
    }
}

/// Apply duration param to the ring without reallocating (SPEC duration change).
pub fn apply_duration(ring: &mut RingBuffer, seconds: f32) {
    let _ = ring.set_duration_rt_safe(BufferDuration::new(seconds));
}

/// Region the transport auditions: the selection when there is one, else the whole capture.
pub fn playback_bounds(selection: Option<(f32, f32)>) -> (f32, f32) {
    match selection {
        Some((a, b)) => (a.min(b), a.max(b)),
        None => (0.0, 1.0),
    }
}

/// Transport play/pause. Pause while audio is running; otherwise audition the selection (or the
/// whole capture when nothing is selected), resuming from the parked playhead when it still sits
/// inside that region.
///
/// Recording is held still while playback runs, so this auditions exactly what was captured.
pub fn toggle_playback(ed: &mut EditorState) -> String {
    let playing = ed.ring.lock().is_playing();
    if playing {
        pause_playback_off_lock(ed);
        return "Paused".to_string();
    }
    let mut ring = ed.ring.lock();
    let (start, end) = playback_bounds(ed.selection);
    let from = ring
        .playhead_fraction()
        .filter(|pos| *pos >= start && *pos < end)
        .unwrap_or(start);
    let started = if ed.selection.is_some() {
        ring.loop_range(start, end, from)
    } else {
        ring.play_range(from, end)
    };
    if started {
        "Playing".to_string()
    } else {
        "Nothing recorded yet.".to_string()
    }
}

/// Pause playback without holding the ring lock across a scrub bake.
///
/// A scrub rotation touches up to the whole capture. `process()` shares this lock, so doing that
/// under it can stall the audio callback. When a rotation is pending, the audio thread records
/// into a small parking ring while the bake runs unlocked; those frames are appended afterwards.
pub(crate) fn pause_playback_off_lock(ed: &mut EditorState) {
    let mut guard = ed.ring.lock();
    if guard.shift_frames() == 0 {
        guard.pause_playback();
        return;
    }

    let sample_rate = guard.sample_rate();
    let paused = guard.is_paused();
    let mut parking = match ed.spare_ring.take() {
        Some(mut spare) if (spare.sample_rate() - sample_rate).abs() < 1.0 => {
            spare.clear();
            spare
        }
        _ => RingBuffer::new(sample_rate, BufferDuration::new(SCRUB_PARKING_SECONDS)),
    };
    parking.set_paused(paused);
    let mut baked = std::mem::replace(&mut *guard, parking);
    drop(guard);

    baked.bake_scrub();
    baked.pause_playback();

    // Reattach the baked ring and append anything recorded into the parking ring meanwhile.
    let mut guard = ed.ring.lock();
    let parking = &mut *guard;
    let recorded = parking.filled_frames();
    let was_paused = baked.is_paused();
    baked.set_paused(false);
    for i in 0..recorded {
        if let Some((left, right)) = parking.read_logical(i) {
            baked.write_frame(left, right);
        }
    }
    baked.set_paused(was_paused);
    let parking = std::mem::replace(&mut *guard, baked);
    drop(guard);
    ed.spare_ring = Some(parking);
}

pub fn create(
    params: Arc<PirateSamplerParams>,
    editor_state: Arc<EguiState>,
    shared: Arc<Mutex<EditorState>>,
) -> Option<Box<dyn Editor>> {
    const {
        assert!(SIDE_W + MIN_WAVEFORM_W + HORIZONTAL_MARGINS <= WINDOW_W);
        assert!((WAVEFORM_H - (WINDOW_H - VERTICAL_MARGINS)).abs() < f32::EPSILON);
    }

    let initial_window_size = editor_state.size();
    create_egui_editor(
        editor_state,
        (),
        |egui_ctx, _| {
            theme::apply(egui_ctx);
        },
        move |egui_ctx, setter, _state| {
            theme::apply(egui_ctx);

            #[cfg(target_os = "linux")]
            if let Some(window_id) = nih_plug_egui::parent_x11_window() {
                crate::chrome::attach(window_id, initial_window_size.0, initial_window_size.1);
            }
            crate::chrome::pump_move();
            crate::chrome::pump_resize();
            egui_ctx.request_repaint_after(std::time::Duration::from_millis(33));

            let mut ed = shared.lock();

            let (is_empty, (peak_l, peak_r)) = {
                let mut ring = ed.ring.lock();
                let p = ring.read_and_decay_peak();
                (ring.is_empty(), p)
            };
            let peak = peak_l.max(peak_r);
            let db = if peak > 1e-4 {
                20.0 * peak.log10()
            } else {
                -96.0
            };

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

            handle_keyboard_shortcuts(egui_ctx, setter, &params, &mut ed);

            let paused = params.pause.value();

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
                        db,
                        cur_norm,
                        peak_hold_norm,
                    );
                });

            egui::CentralPanel::default()
                .frame(
                    egui::Frame::new()
                        .fill(theme::PANEL)
                        .inner_margin(egui::Margin {
                            left: LEFT_MARGIN,
                            right: RIGHT_GAP,
                            top: 0,
                            bottom: 0,
                        }),
                )
                .show(egui_ctx, |ui| {
                    let wave_card_h = ui.available_height().max(16.0);
                    draw_waveform_card(ui, &mut ed, wave_card_h, is_empty, paused);
                });

            draw_resize_grip(egui_ctx);
        },
    )
}

fn handle_keyboard_shortcuts(
    egui_ctx: &egui::Context,
    setter: &ParamSetter,
    params: &Arc<PirateSamplerParams>,
    ed: &mut EditorState,
) {
    if egui_ctx.input(|i| i.pointer.button_pressed(egui::PointerButton::Middle)) {
        let paused = params.pause.value();
        setter.begin_set_parameter(&params.pause);
        setter.set_parameter(&params.pause, !paused);
        setter.end_set_parameter(&params.pause);
        ed.status = if !paused {
            "Paused".to_string()
        } else {
            "Resumed".to_string()
        };
    }
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

    let copy_shortcut = egui_ctx
        .input(|i| (i.modifiers.command || i.modifiers.ctrl) && i.key_pressed(egui::Key::C));
    if copy_shortcut && ed.selection.is_some() {
        ed.status = copy_selection_to_clipboard(ed, egui_ctx);
    }
}

fn draw_resize_grip(egui_ctx: &egui::Context) {
    let screen = egui_ctx.screen_rect();
    egui::Area::new(egui::Id::new("resize_grip"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(screen.right() - 20.0, screen.bottom() - 20.0))
        .show(egui_ctx, |ui| {
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::drag());
            if resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeNwSe);
            }
            if resp.drag_started()
                || (resp.contains_pointer() && ui.input(|i| i.pointer.primary_pressed()))
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
                [
                    egui::pos2(corner.x - 9.0, corner.y),
                    egui::pos2(corner.x, corner.y - 9.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(corner.x - 5.0, corner.y),
                    egui::pos2(corner.x, corner.y - 5.0),
                ],
                stroke,
            );
        });
}

#[allow(clippy::too_many_arguments)]
fn draw_side_panel(
    ui: &mut egui::Ui,
    egui_ctx: &egui::Context,
    setter: &ParamSetter,
    params: &Arc<PirateSamplerParams>,
    ed: &mut EditorState,
    db: f32,
    cur_norm: f32,
    peak_hold_norm: f32,
) {
    ui.spacing_mut().item_spacing = egui::vec2(6.0, SECTION_GAP);
    let avail_h = ui.available_height();
    let full_w = ui.available_width();

    let screen = ui.ctx().screen_rect();
    let grip_excl = egui::Rect::from_min_max(
        egui::pos2(screen.right() - 24.0, screen.bottom() - 24.0),
        screen.max,
    );
    let pointer_in_grip = ui
        .input(|i| i.pointer.interact_pos())
        .is_some_and(|p| grip_excl.contains(p));

    let bg_drag = ui.interact(
        ui.max_rect(),
        ui.id().with("panel_bg_drag"),
        egui::Sense::drag(),
    );
    if !pointer_in_grip && panel_background_starts_move(&bg_drag) {
        crate::chrome::begin_move();
    }

    let overflow_tier1 = avail_h < FULL_STACK_NEEDED_H;
    let overflow_tier2 = avail_h < TIER1_STACK_NEEDED_H;
    let compact = avail_h < 120.0;
    let logo_only = avail_h < 200.0;

    if ui.available_height() >= if compact { 48.0_f32 } else { H_BRAND } {
        let brand_h = if compact { 18.0 } else { H_BRAND };
        let (brand_rect, _) =
            ui.allocate_exact_size(egui::vec2(full_w, brand_h), egui::Sense::hover());
        let (badge_w, badge_h) = if compact { (19.0, 16.0) } else { (26.0, 22.0) };
        let badge_rect = egui::Rect::from_min_size(
            egui::pos2(brand_rect.left(), brand_rect.center().y - badge_h * 0.5),
            egui::vec2(badge_w, badge_h),
        );
        draw_logo_mark(ui, ui.painter(), badge_rect);
        if !compact && !logo_only {
            ui.painter().text(
                egui::pos2(badge_rect.right() + 6.0, brand_rect.center().y),
                egui::Align2::LEFT_CENTER,
                "PIRATE SAMPLER",
                theme::font_small(),
                theme::ACCENT_HOVER,
            );
        }

        let btn_dim = if compact { 14.0 } else { 16.0 };
        let btn_size = egui::vec2(btn_dim, btn_dim);
        let min_btn = egui::Rect::from_center_size(
            egui::pos2(brand_rect.right() - btn_dim * 0.5, brand_rect.center().y),
            btn_size,
        );
        let more_btn = egui::Rect::from_center_size(
            egui::pos2(min_btn.left() - 4.0 - btn_dim * 0.5, brand_rect.center().y),
            btn_size,
        );
        let drag_rect = egui::Rect::from_min_max(
            brand_rect.min,
            egui::pos2(
                (more_btn.left() - 4.0).max(brand_rect.left()),
                brand_rect.max.y,
            ),
        );
        let drag_resp = ui.interact(drag_rect, ui.id().with("brand_drag"), egui::Sense::drag());
        if drag_resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        if drag_resp.drag_started()
            || (drag_resp.contains_pointer() && ui.input(|i| i.pointer.primary_pressed()))
        {
            crate::chrome::begin_move();
        }

        draw_header_more_menu(ui, more_btn, egui_ctx, setter, params, ed);

        if draw_minimize_button(ui, min_btn, "minimize_btn") {
            crate::chrome::minimize();
        }
    }

    // Pinned System Audio and Microphone buttons (always visible: stacked when tall enough, side-by-side when reduced to one button row)
    let has_top_brand = avail_h >= if compact { 48.0_f32 } else { H_BRAND };
    if ui.available_height() >= H_SOURCE_TOGGLE && !compact {
        draw_stacked_source_toggle(ui, ed, full_w);
    } else if ui.available_height() >= 20.0 {
        if !has_top_brand {
            // Center the single control row vertically in the available panel space
            let seg_h = 24.0_f32.min(ui.available_height());
            let rem_h = (ui.available_height() - seg_h).max(0.0);
            if rem_h > 1.0 {
                ui.add_space((rem_h * 0.5).floor());
            }
        }
        draw_side_by_side_source_toggle(ui, setter, params, ed, full_w, !has_top_brand);
    }

    // Buffer slider inline when window is tall enough
    if !compact && !overflow_tier2 && ui.available_height() >= H_BUFFER {
        draw_buffer_section(ui, setter, params, full_w);
    }

    // Original transport buttons inline when window is at full height (else folded into More)
    if !compact && !overflow_tier1 && ui.available_height() >= H_TRANSPORT {
        draw_transport_group(ui, egui_ctx, setter, params, ed, full_w);
    }

    // Export WAV button inline when tall enough
    let has_sel = ed.selection.is_some();
    if !compact
        && !overflow_tier1
        && ui.available_height() >= H_EXPORT
        && draw_export_button(ui, full_w, H_EXPORT, has_sel)
    {
        ed.status = export_selection(ed);
    }

    if !compact && ui.available_height() >= H_METER {
        draw_meter_section(ui, full_w, db, cur_norm, peak_hold_norm);
    }

    if !compact && !overflow_tier1 && ui.available_height() >= H_STATUS_LINE {
        draw_single_line_status(ui, full_w, &ed.status);
    }

    let rest = ui.available_rect_before_wrap();
    if rest.height() >= 4.0 && rest.width() >= 20.0 {
        let resp = ui.interact(rest, ui.id().with("panel_drag"), egui::Sense::drag());
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        if !pointer_in_grip && resp.drag_started() {
            crate::chrome::begin_move();
        }
    }
}

fn panel_background_starts_move(response: &egui::Response) -> bool {
    response.drag_started_by(egui::PointerButton::Primary)
}

fn draw_header_more_menu(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    egui_ctx: &egui::Context,
    setter: &ParamSetter,
    params: &Arc<PirateSamplerParams>,
    ed: &mut EditorState,
) {
    let id = ui.id().with("header_more_btn");
    let resp = ui.interact(rect, id, egui::Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    let popup_id = id.with("popup");
    if resp.clicked() {
        ui.memory_mut(|mem| mem.toggle_popup(popup_id));
    }

    let is_open = ui.memory(|mem| mem.is_popup_open(popup_id));
    crate::chrome::ensure_min_height(if is_open { Some(340) } else { None });

    let painter = ui.painter();
    let (fill, stroke) = if is_open || resp.hovered() {
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

    let tri_col = if is_open || resp.hovered() {
        theme::TEXT
    } else {
        theme::MUTED
    };
    let c = rect.center();
    let tri_w = 3.5_f32;
    let tri_h = 3.2_f32;
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(c.x - tri_w, c.y - tri_h * 0.5),
            egui::pos2(c.x + tri_w, c.y - tri_h * 0.5),
            egui::pos2(c.x, c.y + tri_h * 0.5),
        ],
        tri_col,
        egui::Stroke::NONE,
    ));

    egui::popup::popup_below_widget(
        ui,
        popup_id,
        &resp,
        egui::PopupCloseBehavior::CloseOnClickOutside,
        |ui| {
            let item_w = 156.0_f32;
            ui.set_min_width(item_w);
            ui.set_max_width(item_w);
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 5.0);

            let btn_h = 24.0;

            // 1. Buffer duration slider
            draw_buffer_section(ui, setter, params, item_w);

            ui.separator();

            // 2. All original buttons as a clean vertical list (no scrollbar needed)
            let paused = params.pause.value();
            let pause_label = if paused { "Resume" } else { "Pause" };
            let (r0, _) = ui.allocate_exact_size(egui::vec2(item_w, btn_h), egui::Sense::hover());
            if draw_transport_button_in_rect(ui, r0, "menu_pause", pause_label, true, paused) {
                setter.begin_set_parameter(&params.pause);
                setter.set_parameter(&params.pause, !paused);
                setter.end_set_parameter(&params.pause);
            }

            let freeze_label = if ed.freeze_view {
                "Unfreeze (H)"
            } else {
                "Freeze (H)"
            };
            let (r1, _) = ui.allocate_exact_size(egui::vec2(item_w, btn_h), egui::Sense::hover());
            if draw_transport_button_in_rect(
                ui,
                r1,
                "menu_freeze",
                freeze_label,
                true,
                ed.freeze_view,
            ) {
                ed.freeze_view = !ed.freeze_view;
                if !ed.freeze_view {
                    ed.frozen_peaks = None;
                }
            }

            let has_sel = ed.selection.is_some();
            let (r2, _) = ui.allocate_exact_size(egui::vec2(item_w, btn_h), egui::Sense::hover());
            if draw_transport_button_in_rect(ui, r2, "menu_copy", "Copy", has_sel, false) {
                ed.status = copy_selection_to_clipboard(ed, egui_ctx);
                ui.memory_mut(|mem| mem.close_popup());
            }

            let (r3, _) = ui.allocate_exact_size(egui::vec2(item_w, btn_h), egui::Sense::hover());
            if draw_transport_button_in_rect(ui, r3, "menu_clips", "Clips", true, false) {
                ed.status = open_clips_folder();
                ui.memory_mut(|mem| mem.close_popup());
            }

            if draw_export_button(ui, item_w, H_EXPORT, has_sel) {
                ed.status = export_selection(ed);
                ui.memory_mut(|mem| mem.close_popup());
            }

            ui.separator();

            // 3. Settings
            ui.label(
                egui::RichText::new("SETTINGS")
                    .font(theme::font_tiny())
                    .color(theme::MUTED),
            );
            let aot_label = if ed.always_on_top {
                "Always on Top: ON"
            } else {
                "Always on Top: OFF"
            };
            let (r_aot, _) =
                ui.allocate_exact_size(egui::vec2(item_w, btn_h), egui::Sense::hover());
            if draw_transport_button_in_rect(
                ui,
                r_aot,
                "menu_aot",
                aot_label,
                true,
                ed.always_on_top,
            ) {
                ed.always_on_top = !ed.always_on_top;
                crate::chrome::set_always_on_top(ed.always_on_top);
                ed.status = if ed.always_on_top {
                    "Always on Top: ON".to_string()
                } else {
                    "Always on Top: OFF".to_string()
                };
            }

            ui.separator();
            draw_single_line_status(ui, item_w, &ed.status);
        },
    );
}

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
            if resp.hovered() {
                theme::TEXT
            } else {
                theme::MUTED
            },
        ),
    );
    resp.clicked()
}

fn draw_logo_mark(ui: &egui::Ui, painter: &egui::Painter, badge_rect: egui::Rect) {
    // 56 x 48 RGBA buffer rasterized directly with safe margins for retina clarity
    const RAW_LOGO_RGBA: &[u8] = include_bytes!("m-and-bones.rgba");
    let tex_id = egui::Id::new("pirate_sampler_m_bones_logo_tex");
    let cached = ui.ctx().data(|d| d.get_temp::<egui::TextureHandle>(tex_id));
    let texture = match cached {
        Some(handle) => handle,
        None => {
            let image = egui::ColorImage::from_rgba_unmultiplied([56, 48], RAW_LOGO_RGBA);
            let handle = ui.ctx().load_texture(
                "pirate_sampler_m_bones_logo",
                image,
                egui::TextureOptions::LINEAR,
            );
            ui.ctx().data_mut(|d| d.insert_temp(tex_id, handle.clone()));
            handle
        }
    };

    painter.image(
        texture.id(),
        badge_rect,
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        Color32::WHITE,
    );
}

fn draw_stacked_source_toggle(ui: &mut egui::Ui, ed: &mut EditorState, width: f32) {
    let seg_h = 24.0;
    let gap = 6.0;
    let (container_rect, _) =
        ui.allocate_exact_size(egui::vec2(width, seg_h * 2.0 + gap), egui::Sense::hover());

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
        (
            sys_rect,
            &sys_resp,
            AudioInputSource::SystemAudio,
            "System Audio",
        ),
        (
            mic_rect,
            &mic_resp,
            AudioInputSource::Microphone,
            "Microphone",
        ),
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

fn draw_side_by_side_source_toggle(
    ui: &mut egui::Ui,
    setter: &ParamSetter,
    params: &Arc<PirateSamplerParams>,
    ed: &mut EditorState,
    width: f32,
    show_inline_logo: bool,
) {
    let seg_h = 24.0_f32.min(ui.available_height());
    let gap = 6.0;
    let (row_rect, _) = ui.allocate_exact_size(egui::vec2(width, seg_h), egui::Sense::hover());

    // Inline-logo rows right-align both source buttons so the logo badge and gain knob fit to
    // their left; otherwise the pair fills the available width.
    let half = if show_inline_logo {
        seg_h
    } else {
        ((row_rect.width() - gap) * 0.5).max(20.0)
    };
    let source_x = if show_inline_logo {
        // Keep the rightmost button clear of the window's bottom-right resize grip.
        row_rect.right() - 16.0 - 2.0 * half - gap
    } else {
        row_rect.left()
    };

    if show_inline_logo {
        let badge_h = seg_h;
        let badge_w = (seg_h * (52.0 / 44.0)).round();
        // The gain knob sits between the badge and the buttons, with `gap` on either side.
        let badge_rect = egui::Rect::from_min_size(
            egui::pos2(
                source_x - gap - GAIN_KNOB_SIZE - gap - badge_w,
                row_rect.top(),
            ),
            egui::vec2(badge_w, badge_h),
        );
        draw_logo_mark(ui, ui.painter(), badge_rect);
        let drag_resp = ui.interact(
            badge_rect,
            ui.id().with("inline_logo_drag"),
            egui::Sense::drag(),
        );
        if drag_resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        if drag_resp.drag_started()
            || (drag_resp.contains_pointer() && ui.input(|i| i.pointer.primary_pressed()))
        {
            crate::chrome::begin_move();
        }
    }

    let sys_rect = egui::Rect::from_min_size(
        egui::pos2(source_x, row_rect.top()),
        egui::vec2(half, seg_h),
    );
    let mic_rect = egui::Rect::from_min_size(
        egui::pos2(source_x + half + gap, row_rect.top()),
        egui::vec2(half, seg_h),
    );

    let sys_resp = ui.interact(sys_rect, ui.id().with("src_sys_row"), egui::Sense::click());
    let mic_resp = ui.interact(mic_rect, ui.id().with("src_mic_row"), egui::Sense::click());

    if sys_resp.hovered() || mic_resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    if show_inline_logo {
        let gain_center = egui::pos2(
            sys_rect.left() - gap - GAIN_KNOB_RADIUS,
            row_rect.center().y,
        );
        draw_playback_gain(ui, setter, params, gain_center, false);
    }

    let painter = ui.painter();
    for (rect, resp, source, label) in [
        (sys_rect, &sys_resp, AudioInputSource::SystemAudio, "System"),
        (mic_rect, &mic_resp, AudioInputSource::Microphone, "Mic"),
    ] {
        let active = ed.input_source == source;
        if active {
            let fill = if resp.hovered() {
                theme::ACCENT_HOVER
            } else {
                theme::ACCENT
            };
            painter.rect_filled(rect, egui::CornerRadius::same(theme::CONTROL_RADIUS), fill);
            if show_inline_logo {
                draw_compact_source_icon(painter, rect.center(), source, theme::BG);
            } else {
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    label,
                    theme::font_small(),
                    theme::BG,
                );
            }
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
            if show_inline_logo {
                draw_compact_source_icon(painter, rect.center(), source, text_col);
            } else {
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    label,
                    theme::font_small(),
                    text_col,
                );
            }
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

fn draw_compact_source_icon(
    painter: &egui::Painter,
    center: egui::Pos2,
    source: AudioInputSource,
    color: Color32,
) {
    let stroke = egui::Stroke::new(1.5_f32, color);
    match source {
        AudioInputSource::SystemAudio => {
            painter.add(egui::Shape::convex_polygon(
                vec![
                    center + egui::vec2(-6.0, -3.0),
                    center + egui::vec2(-3.0, -3.0),
                    center + egui::vec2(1.0, -6.0),
                    center + egui::vec2(1.0, 6.0),
                    center + egui::vec2(-3.0, 3.0),
                    center + egui::vec2(-6.0, 3.0),
                ],
                color,
                egui::Stroke::NONE,
            ));
            painter.line_segment(
                [
                    center + egui::vec2(4.0, -3.0),
                    center + egui::vec2(6.0, 0.0),
                ],
                stroke,
            );
            painter.line_segment(
                [center + egui::vec2(6.0, 0.0), center + egui::vec2(4.0, 3.0)],
                stroke,
            );
        }
        AudioInputSource::Microphone => {
            painter.rect_stroke(
                egui::Rect::from_center_size(center + egui::vec2(0.0, -2.0), egui::vec2(6.0, 9.0)),
                egui::CornerRadius::same(3),
                stroke,
                egui::StrokeKind::Inside,
            );
            painter.line_segment(
                [
                    center + egui::vec2(-5.0, 0.0),
                    center + egui::vec2(-5.0, 3.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center + egui::vec2(-5.0, 3.0),
                    center + egui::vec2(0.0, 5.0),
                ],
                stroke,
            );
            painter.line_segment(
                [center + egui::vec2(0.0, 5.0), center + egui::vec2(5.0, 3.0)],
                stroke,
            );
            painter.line_segment(
                [center + egui::vec2(5.0, 3.0), center + egui::vec2(5.0, 0.0)],
                stroke,
            );
            painter.line_segment(
                [center + egui::vec2(0.0, 5.0), center + egui::vec2(0.0, 8.0)],
                stroke,
            );
        }
    }
}

fn draw_buffer_section(
    ui: &mut egui::Ui,
    setter: &ParamSetter,
    params: &Arc<PirateSamplerParams>,
    width: f32,
) {
    let (sec_rect, _) = ui.allocate_exact_size(egui::vec2(width, H_BUFFER), egui::Sense::hover());
    let buffer_width = (width - BUFFER_GAIN_RESERVE).max(20.0);
    let label_rect = egui::Rect::from_min_size(sec_rect.min, egui::vec2(buffer_width, 14.0));
    let slider_rect = egui::Rect::from_min_size(
        egui::pos2(sec_rect.left(), sec_rect.top() + 16.0),
        egui::vec2(buffer_width, 18.0),
    );
    let gain_center = egui::pos2(
        sec_rect.right() - GAIN_KNOB_INSET,
        sec_rect.center().y + 3.0,
    );
    draw_playback_gain(ui, setter, params, gain_center, true);

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

fn draw_playback_gain(
    ui: &mut egui::Ui,
    setter: &ParamSetter,
    params: &Arc<PirateSamplerParams>,
    center: egui::Pos2,
    show_label: bool,
) {
    let rect = egui::Rect::from_center_size(center, egui::vec2(GAIN_KNOB_SIZE, GAIN_KNOB_SIZE));
    // The inline and buffer layouts never share a frame, but distinct IDs keep drag state from
    // leaking between them if that ever changes.
    let response = ui.interact(
        rect,
        ui.id().with(("playback_gain", show_label)),
        egui::Sense::click_and_drag(),
    );
    let gain = params.playback_gain_db.value();
    let drag_id = response.id.with("start_gain");
    if response.drag_started_by(egui::PointerButton::Primary) {
        ui.ctx().data_mut(|data| data.insert_temp(drag_id, gain));
        setter.begin_set_parameter(&params.playback_gain_db);
    }
    if response.dragged_by(egui::PointerButton::Primary) {
        let start_gain = ui.ctx().data(|data| data.get_temp::<f32>(drag_id));
        let origin = ui.input(|input| input.pointer.press_origin());
        if let (Some(start_gain), Some(origin), Some(position)) =
            (start_gain, origin, response.interact_pointer_pos())
        {
            let value = playback_gain_from_drag(start_gain, position.y - origin.y);
            setter.set_parameter(&params.playback_gain_db, value);
        }
    }
    if response.drag_stopped_by(egui::PointerButton::Primary) {
        setter.end_set_parameter(&params.playback_gain_db);
        ui.ctx().data_mut(|data| data.remove::<f32>(drag_id));
    }
    if response.double_clicked_by(egui::PointerButton::Primary) {
        setter.begin_set_parameter(&params.playback_gain_db);
        setter.set_parameter(&params.playback_gain_db, 0.0);
        setter.end_set_parameter(&params.playback_gain_db);
    }
    let painter = ui.painter();
    if show_label {
        painter.text(
            egui::pos2(center.x, center.y - 20.0),
            egui::Align2::CENTER_TOP,
            "GAIN",
            theme::font_tiny(),
            theme::MUTED,
        );
    }
    painter.circle_filled(center, 10.0, theme::TRACK_FILL);
    painter.circle_stroke(center, 10.0, egui::Stroke::new(1.0_f32, theme::STROKE));
    let angle = std::f32::consts::PI
        * (0.75
            + (gain - PLAYBACK_GAIN_MIN_DB) / (PLAYBACK_GAIN_MAX_DB - PLAYBACK_GAIN_MIN_DB) * 1.5);
    let tip = center + egui::vec2(angle.cos(), angle.sin()) * 7.0;
    painter.line_segment([center, tip], egui::Stroke::new(2.0_f32, theme::ACCENT));
    response.on_hover_text(format!(
        "Playback gain: {gain:.1} dB\nDrag up/down; double-click to reset"
    ));
}

fn playback_gain_from_drag(start_gain: f32, vertical_distance: f32) -> f32 {
    (start_gain - vertical_distance * 0.5).clamp(PLAYBACK_GAIN_MIN_DB, PLAYBACK_GAIN_MAX_DB)
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

    let track_rect = egui::Rect::from_min_max(
        egui::pos2(rail_left, cy - track_h * 0.5),
        egui::pos2(rail_right, cy + track_h * 0.5),
    );
    painter.rect_filled(track_rect, egui::CornerRadius::same(2), theme::TRACK_FILL);

    let active_rect = egui::Rect::from_min_max(
        egui::pos2(rail_left, cy - track_h * 0.5),
        egui::pos2(handle_x.max(rail_left + 1.5), cy + track_h * 0.5),
    );
    painter.rect_filled(active_rect, egui::CornerRadius::same(2), theme::ACCENT);

    let handle_center = egui::pos2(handle_x, cy);
    let handle_col = if resp.hovered() || resp.dragged() {
        theme::ACCENT_HOVER
    } else {
        theme::ACCENT
    };
    let handle_rect =
        egui::Rect::from_center_size(handle_center, egui::vec2(handle_r * 2.0, handle_r * 2.0));
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

    // Settings button (replaced old Drag -> Bitwig button)
    draw_inline_settings_button(ui, r2, egui_ctx, ed);

    if draw_transport_button_in_rect(ui, r3, "tr_clips", "Clips", true, false) {
        ed.status = open_clips_folder();
    }
}

fn draw_inline_settings_button(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    egui_ctx: &egui::Context,
    ed: &mut EditorState,
) {
    let id = ui.id().with("tr_settings_btn");
    let resp = ui.interact(rect, id, egui::Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    let popup_id = id.with("popup");
    if resp.clicked() {
        ui.memory_mut(|mem| mem.toggle_popup(popup_id));
    }

    let is_open = ui.memory(|mem| mem.is_popup_open(popup_id));
    let painter = ui.painter();
    let fill = if is_open || resp.hovered() {
        theme::HOVER_FILL
    } else {
        theme::PANEL
    };
    let stroke_col = if is_open || resp.hovered() {
        theme::ACCENT_HOVER
    } else {
        theme::STROKE
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
        "Settings",
        theme::font_small(),
        theme::TEXT,
    );

    let cx = rect.right() - 12.0;
    let cy = rect.center().y + 0.5;
    let chev_stroke = egui::Stroke::new(1.2_f32, theme::TEXT);
    painter.line_segment(
        [egui::pos2(cx - 3.0, cy - 1.5), egui::pos2(cx, cy + 1.5)],
        chev_stroke,
    );
    painter.line_segment(
        [egui::pos2(cx, cy + 1.5), egui::pos2(cx + 3.0, cy - 1.5)],
        chev_stroke,
    );

    egui::popup::popup_below_widget(
        ui,
        popup_id,
        &resp,
        egui::PopupCloseBehavior::CloseOnClickOutside,
        |ui| {
            let item_w = 156.0_f32;
            ui.set_min_width(item_w);
            ui.set_max_width(item_w);
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 5.0);
            let btn_h = 24.0;

            let aot_label = if ed.always_on_top {
                "Always on Top: ON"
            } else {
                "Always on Top: OFF"
            };
            let (r_aot, _) =
                ui.allocate_exact_size(egui::vec2(item_w, btn_h), egui::Sense::hover());
            if draw_transport_button_in_rect(
                ui,
                r_aot,
                "inline_aot",
                aot_label,
                true,
                ed.always_on_top,
            ) {
                ed.always_on_top = !ed.always_on_top;
                crate::chrome::set_always_on_top(ed.always_on_top);
                ed.status = if ed.always_on_top {
                    "Always on Top: ON".to_string()
                } else {
                    "Always on Top: OFF".to_string()
                };
            }

            let has_sel = ed.selection.is_some();
            let (r_cp, _) = ui.allocate_exact_size(egui::vec2(item_w, btn_h), egui::Sense::hover());
            if draw_transport_button_in_rect(
                ui,
                r_cp,
                "inline_copy",
                "Copy Selection",
                has_sel,
                false,
            ) {
                ed.status = copy_selection_to_clipboard(ed, egui_ctx);
                ui.memory_mut(|mem| mem.close_popup());
            }
        },
    );
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
        let fill = if !enabled || resp.hovered() {
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

fn draw_meter_section(ui: &mut egui::Ui, width: f32, db: f32, cur_norm: f32, peak_hold_norm: f32) {
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

    painter.rect_filled(bar_rect, egui::CornerRadius::ZERO, theme::TRACK_FILL);
    if cur_norm > 0.01 {
        let fill_w = (bar_rect.width() * cur_norm).clamp(2.0, bar_rect.width());
        let fill_rect =
            egui::Rect::from_min_size(bar_rect.min, egui::vec2(fill_w, bar_rect.height()));
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
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, H_STATUS_LINE), egui::Sense::hover());
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
        let mid = (lo + hi).div_ceil(2);
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use parking_lot::Mutex;
    use pirate_sampler_dsp::RingBuffer;

    use super::*;

    #[test]
    fn gain_drag_does_not_start_panel_move() {
        let ctx = egui::Context::default();
        let knob = egui::Rect::from_min_size(
            egui::pos2(30.0, 30.0),
            egui::vec2(GAIN_KNOB_SIZE, GAIN_KNOB_SIZE),
        );
        let start = knob.center();
        let mut move_started = false;
        let mut knob_dragged = false;
        for events in [
            vec![egui::Event::PointerMoved(start)],
            vec![egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            }],
            vec![egui::Event::PointerMoved(start + egui::vec2(0.0, -15.0))],
        ] {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(200.0, 200.0),
                )),
                events,
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let bg = ui.interact(
                        ui.max_rect(),
                        ui.id().with("panel_bg_drag"),
                        egui::Sense::drag(),
                    );
                    move_started |= panel_background_starts_move(&bg);
                    let response = ui.interact(
                        knob,
                        ui.id().with(("playback_gain", true)),
                        egui::Sense::click_and_drag(),
                    );
                    knob_dragged |= response.dragged_by(egui::PointerButton::Primary);
                });
            });
        }
        assert!(!move_started, "knob press must not start window movement");
        assert!(knob_dragged, "primary drag must reach the gain knob");
    }

    #[test]
    fn playback_gain_drag_uses_total_vertical_distance() {
        assert_eq!(playback_gain_from_drag(0.0, -10.0), 5.0);
        assert_eq!(playback_gain_from_drag(0.0, 10.0), -5.0);
        assert_eq!(playback_gain_from_drag(0.0, -30.0), 15.0);
        assert_eq!(playback_gain_from_drag(0.0, -100.0), 24.0);
        assert_eq!(playback_gain_from_drag(0.0, 200.0), -60.0);
        assert_eq!(playback_gain_from_drag(0.0, 0.5), -0.25);
    }

    /// Editor whose ring holds `frames` frames valued by their logical index.
    fn editor_with_frames(frames: usize) -> EditorState {
        let ring = RingBuffer::with_capacity(48_000.0, (frames * 2).max(2));
        let ed = EditorState::new(Arc::new(Mutex::new(ring)));
        {
            let mut ring = ed.ring.lock();
            for i in 0..frames {
                ring.write_frame(i as f32, -(i as f32));
            }
        }
        ed
    }

    #[test]
    fn transport_toggles_play_and_pause() {
        let mut ed = editor_with_frames(8);
        assert_eq!(toggle_playback(&mut ed), "Playing");
        assert!(ed.ring.lock().is_playing());
        assert_eq!(toggle_playback(&mut ed), "Paused");
        assert!(!ed.ring.lock().is_playing());
    }

    #[test]
    fn pausing_a_scrubbed_loop_keeps_the_rotation_and_releases_the_audio_lock() {
        let mut ed = editor_with_frames(8);
        ed.selection = Some((0.25, 0.5));
        assert_eq!(toggle_playback(&mut ed), "Playing");
        assert_eq!(ed.ring.lock().shift_content(-2), -2);

        assert_eq!(toggle_playback(&mut ed), "Paused");

        let ring = ed.ring.lock();
        assert_eq!(ring.shift_frames(), 0, "the scrub must be baked in");
        assert_eq!(ring.read_logical(2), Some((4.0, -4.0)));
        assert_eq!(ring.read_logical(0), Some((2.0, -2.0)));
        assert_eq!(
            ring.filled_frames(),
            8,
            "the bake must preserve the capture"
        );
        drop(ring);
        assert!(ed.spare_ring.is_some(), "the parking ring is retained");
    }

    #[test]
    fn transport_loops_only_the_selection() {
        let mut ed = editor_with_frames(8);
        ed.selection = Some((0.5, 0.75));
        assert_eq!(toggle_playback(&mut ed), "Playing");
        assert_eq!(ed.ring.lock().next_playback_frame(), Some((4.0, -4.0)));
        assert_eq!(ed.ring.lock().next_playback_frame(), Some((5.0, -5.0)));
        assert_eq!(
            ed.ring.lock().next_playback_frame(),
            Some((4.0, -4.0)),
            "playback must loop to the selection start"
        );
    }

    #[test]
    fn transport_resumes_from_the_parked_playhead() {
        let mut ed = editor_with_frames(8);
        ed.selection = Some((0.5, 1.0));
        assert_eq!(toggle_playback(&mut ed), "Playing");
        assert_eq!(ed.ring.lock().next_playback_frame(), Some((4.0, -4.0)));
        assert_eq!(toggle_playback(&mut ed), "Paused");
        assert_eq!(toggle_playback(&mut ed), "Playing");
        assert_eq!(ed.ring.lock().next_playback_frame(), Some((5.0, -5.0)));
        assert_eq!(ed.ring.lock().next_playback_frame(), Some((6.0, -6.0)));
        assert_eq!(ed.ring.lock().next_playback_frame(), Some((7.0, -7.0)));
        assert_eq!(ed.ring.lock().next_playback_frame(), Some((4.0, -4.0)));
    }

    #[test]
    fn transport_without_audio_reports_nothing_to_play() {
        let mut ed = editor_with_frames(0);
        assert_eq!(toggle_playback(&mut ed), "Nothing recorded yet.");
        assert!(!ed.ring.lock().is_playing());
    }
}
