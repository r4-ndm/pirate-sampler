//! Waveform plot rendering, region selection hit-testing, edge resizing,
//! and direct drag-to-DAW interaction.

use nih_plug_egui::egui::{self, Color32};

use crate::clips::drag_out_selection;
use crate::editor::EditorState;
use crate::theme;

const EDGE_GRAB_PX: f32 = 8.0;

/// Pixel x for a normalized buffer position inside the visible `view` window.
fn x_at_t(t: f32, plot_rect: egui::Rect, view: (f32, f32)) -> f32 {
    let span = (view.1 - view.0).max(f32::EPSILON);
    plot_rect.left() + ((t - view.0) / span) * plot_rect.width()
}

/// Normalized buffer position for a pixel x inside the visible `view` window.
fn t_at_x(x: f32, plot_rect: egui::Rect, view: (f32, f32)) -> f32 {
    let span = (view.1 - view.0).max(f32::EPSILON);
    view.0 + ((x - plot_rect.left()) / plot_rect.width()) * span
}

#[allow(clippy::too_many_arguments)]
pub fn draw_waveform_card(
    ui: &mut egui::Ui,
    ed: &mut EditorState,
    card_h: f32,
    filled_frames: usize,
    cap_secs: f32,
    sr: f32,
    is_empty: bool,
    _paused: bool,
) {
    let (plot_rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), card_h),
        egui::Sense::click_and_drag(),
    );
    let painter = ui.painter_at(plot_rect);

    let mid = plot_rect.center().y;
    let half_h = (plot_rect.height() * 0.5 - 2.0).max(6.0);

    draw_peaks(&painter, plot_rect, ed, mid, half_h, is_empty);

    let hovering_edge = handle_pointer_interaction(ui, &response, plot_rect, ed);

    if let Some((a, b)) = ed.selection {
        draw_selection_overlay(
            &painter,
            plot_rect,
            mid,
            a,
            b,
            hovering_edge,
            filled_frames,
            cap_secs,
            sr,
            ed.view_range,
        );
    }

    if ed.freeze_view {
        draw_state_stamp(&painter, plot_rect, "FROZEN");
    }
}

fn draw_peaks(
    painter: &egui::Painter,
    plot_rect: egui::Rect,
    ed: &mut EditorState,
    mid: f32,
    half_h: f32,
    is_empty: bool,
) {
    let num_buckets = (plot_rect.width().max(32.0) as usize).clamp(32, 2048);
    let (view_start, view_end) = ed.view_range;
    let visible_start = view_start.max(0.0);
    let visible_end = view_end.min(1.0);
    let visible_fraction = ((visible_end - visible_start) / (view_end - view_start)).max(0.0);
    let visible_buckets = ((num_buckets as f32 * visible_fraction).ceil() as usize).max(1);
    let peaks = if ed.freeze_view {
        let needs_snap = ed
            .frozen_peaks
            .as_ref()
            .is_none_or(|v| v.len() != num_buckets);
        if needs_snap {
            let snap = ed.ring.lock().waveform_peaks(num_buckets);
            ed.frozen_peaks = Some(snap);
        }
        let snap = ed.frozen_peaks.as_ref().unwrap();
        if snap.is_empty() {
            Vec::new()
        } else {
            (0..visible_buckets)
                .map(|i| {
                    let t = visible_start
                        + (i as f32 + 0.5) / visible_buckets as f32 * (visible_end - visible_start);
                    snap[((t * snap.len() as f32) as usize).min(snap.len() - 1)]
                })
                .collect()
        }
    } else {
        ed.ring
            .lock()
            .waveform_peaks_range(visible_buckets, visible_start, visible_end)
    };

    if is_empty || peaks.is_empty() {
        painter.text(
            plot_rect.center(),
            egui::Align2::CENTER_CENTER,
            "Waiting for audio signal...",
            theme::font_body(),
            theme::MUTED,
        );
    } else {
        let n = peaks.len() as f32;
        let max_peak = peaks.iter().fold(0.0_f32, |acc, &(l, r)| acc.max(l.max(r)));
        let outer_fill = Color32::from_rgba_unmultiplied(0xFF, 0xFF, 0xFF, 89);
        let mid_fill = Color32::from_rgba_unmultiplied(0xFF, 0xFF, 0xFF, 153);
        let core_fill = Color32::from_rgba_unmultiplied(0xFF, 0xFF, 0xFF, 217);
        let contour_stroke = egui::Stroke::new(1.0_f32, theme::TEXT);

        let mut prev_top: Option<egui::Pos2> = None;
        let mut prev_bot: Option<egui::Pos2> = None;

        for (i, (l, r)) in peaks.iter().enumerate() {
            let t = visible_start + (i as f32 + 0.5) / n * (visible_end - visible_start);
            let x = x_at_t(t, plot_rect, ed.view_range);
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
}

fn handle_pointer_interaction(
    ui: &mut egui::Ui,
    response: &egui::Response,
    plot_rect: egui::Rect,
    ed: &mut EditorState,
) -> bool {
    // Read the pointer from global input instead of `response.hover_pos()`: egui clears a
    // widget's hover flag while a button is held and the pointer is over another widget (or
    // over this widget before the click is granted), so hover-derived positions go stale
    // mid-gesture. `interact_pos` stays valid for as long as the pointer is in the window.
    let input = ui.input(|i| i.clone());
    let view = ed.view_range;
    let pointer = input.pointer.interact_pos();
    let pointer_in_plot = pointer.filter(|p| plot_rect.contains(*p));
    let press_pos = input.pointer.press_origin();
    let primary_pressed = input.pointer.button_pressed(egui::PointerButton::Primary);
    let primary_down = input.pointer.button_down(egui::PointerButton::Primary);
    let primary_released = input.pointer.button_released(egui::PointerButton::Primary);
    let secondary_pressed = input.pointer.button_pressed(egui::PointerButton::Secondary);
    let secondary_down = input.pointer.button_down(egui::PointerButton::Secondary);

    let mut hovering_edge = false;
    let mut hovering_inside_sel = false;
    if let (Some(pos), Some((a, b))) = (pointer_in_plot, ed.selection) {
        let x0 = x_at_t(a.min(b), plot_rect, view);
        let x1 = x_at_t(a.max(b), plot_rect, view);
        if (pos.x - x0).abs() <= EDGE_GRAB_PX || (pos.x - x1).abs() <= EDGE_GRAB_PX {
            hovering_edge = true;
        } else if pos.x > x0 && pos.x < x1 {
            hovering_inside_sel = true;
        }
    }

    if ed.is_panning {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    } else if hovering_edge {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    } else if hovering_inside_sel {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    } else if pointer_in_plot.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    }

    // Zoom with scroll wheel (cursor‑centered)
    if let Some(pos) = pointer_in_plot {
        let scroll = ui.input(|i| i.raw_scroll_delta.y);
        if scroll != 0.0 && plot_rect.contains(pos) {
            let cursor_t = ((pos.x - plot_rect.left()) / plot_rect.width()).clamp(0.0, 1.0);
            let factor = if scroll > 0.0 { 0.85 } else { 1.15 };
            let (start, end) = ed.view_range;
            let range = end - start;
            let new_range = (range * factor).clamp(0.01, 1.0);
            let cursor_abs = start + cursor_t * range;
            let new_start =
                (cursor_abs - cursor_t * new_range).clamp(-0.9 * new_range, 1.0 - 0.1 * new_range);
            let new_end = new_start + new_range;
            ed.view_range = (new_start, new_end);
        }
    }

    // Double‑click resets zoom to full view
    if response.double_clicked() && pointer_in_plot.is_some() {
        ed.view_range = (0.0, 1.0);
    }

    // Right‑click drag pans the zoomed window through the buffer. Selection work is bound to
    // the left button, so the right button is free to pan anywhere on the plot, including
    // over an existing selection edge.

    // Start pan from wherever the press landed.
    if secondary_pressed {
        if let Some(pos) = press_pos.or(pointer) {
            if plot_rect.contains(pos) {
                ed.is_panning = true;
                ed.pan_start_pos = Some((pos.x - plot_rect.left()) / plot_rect.width());
                ed.pan_start_range = ed.view_range;
            }
        }
    }

    // Continue pan: the pointer is free to leave the plot while the button is held. A drag
    // across `fraction` of the plot width shifts the view by that fraction of its own span,
    // so the sample under the pointer stays under the pointer.
    if let (true, Some(start_t), Some(pos)) = (ed.is_panning, ed.pan_start_pos, pointer) {
        let cur_t = (pos.x - plot_rect.left()) / plot_rect.width();
        let (orig_start, orig_end) = ed.pan_start_range;
        let range = (orig_end - orig_start).clamp(0.0, 1.0);
        // Keep at least 10% of the view overlapping the recording, even at full zoom.
        let new_start =
            (orig_start - (cur_t - start_t) * range).clamp(-0.9 * range, 1.0 - 0.1 * range);
        ed.view_range = (new_start, new_start + range);
    }

    // End pan whenever the button is no longer held, so a release over another widget can
    // never leave the editor stuck in panning mode.
    if ed.is_panning && !secondary_down {
        ed.is_panning = false;
        ed.pan_start_pos = None;
    }

    // Selection editing is left-button only. It is driven by explicit primary-button events
    // instead of `Response::drag_*`, because egui reports a widget drag for *any* pointer
    // button, which is how right-drags used to draw a selection as well.
    if !ed.is_panning {
        if primary_pressed {
            if let Some(pos) = press_pos.filter(|p| {
                plot_rect.contains(*p) && (0.0..=1.0).contains(&t_at_x(p.x, plot_rect, view))
            }) {
                let t = t_at_x(pos.x, plot_rect, view).clamp(0.0, 1.0);
                ed.selection_body_press = false;
                if let Some((a, b)) = ed.selection {
                    let (start_n, end_n) = (a.min(b), a.max(b));
                    let x0 = x_at_t(start_n, plot_rect, view);
                    let x1 = x_at_t(end_n, plot_rect, view);
                    if (pos.x - x0).abs() <= EDGE_GRAB_PX {
                        ed.drag_anchor = Some(end_n);
                        ed.selection = Some((t, end_n));
                    } else if (pos.x - x1).abs() <= EDGE_GRAB_PX {
                        ed.drag_anchor = Some(start_n);
                        ed.selection = Some((start_n, t));
                    } else if pos.x > x0 && pos.x < x1 {
                        // Pressed inside the selection: dragging from here drags the clip out.
                        ed.drag_anchor = None;
                        ed.selection_body_press = true;
                    } else {
                        ed.drag_anchor = Some(t);
                        ed.selection = Some((t, t));
                    }
                } else {
                    ed.drag_anchor = Some(t);
                    ed.selection = Some((t, t));
                }
            }
        } else if primary_down {
            if let Some(pos) = pointer {
                let t = t_at_x(pos.x, plot_rect, view).clamp(0.0, 1.0);
                if let Some(anchor) = ed.drag_anchor {
                    ed.selection = Some((anchor, t));
                } else if ed.selection_body_press && input.pointer.is_decidedly_dragging() {
                    ed.status = drag_out_selection(ed);
                    ed.selection_body_press = false;
                }
            }
        }
    }

    // Finish the left-button gesture: drop click-without-drag selections and clear the anchor.
    if primary_released {
        ed.selection_body_press = false;
        if ed.drag_anchor.take().is_some() {
            if let Some((a, b)) = ed.selection {
                let span = (view.1 - view.0).max(f32::EPSILON);
                if (b - a).abs() / span * plot_rect.width() < 2.0 {
                    ed.selection = None;
                }
            }
        }
    }

    hovering_edge
}

#[allow(clippy::too_many_arguments)]
fn draw_selection_overlay(
    painter: &egui::Painter,
    plot_rect: egui::Rect,
    mid: f32,
    a: f32,
    b: f32,
    active_edge: bool,
    filled_frames: usize,
    cap_secs: f32,
    sr: f32,
    view_range: (f32, f32),
) {
    let start_n = a.min(b);
    let end_n = a.max(b);
    let (view_start, view_end) = view_range;
    if end_n < view_start || start_n > view_end {
        return;
    }
    let x0 =
        plot_rect.left() + ((start_n - view_start) / (view_end - view_start)) * plot_rect.width();
    let x1 = (plot_rect.left()
        + ((end_n - view_start) / (view_end - view_start)) * plot_rect.width())
    .max(x0 + 2.0);
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

    let handle_w = 6.0;
    let handle_h = 28.0_f32.min((plot_rect.height() - 6.0).max(8.0));
    let handle_color = if active_edge {
        theme::ACCENT_HOVER
    } else {
        theme::ACCENT
    };
    for &edge_x in &[x0, x1] {
        if edge_x < plot_rect.left() || edge_x > plot_rect.right() {
            continue;
        }
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
    let pill_h = 20.0_f32;
    let center_x = ((x0 + x1) * 0.5).clamp(
        plot_rect.left() + pill_w * 0.5 + 6.0,
        plot_rect.right() - pill_w * 0.5 - 6.0,
    );
    let pill_y = if plot_rect.height() < 40.0 {
        mid
    } else {
        plot_rect.top() + 15.0
    };
    let pill_rect = egui::Rect::from_center_size(
        egui::pos2(center_x, pill_y),
        egui::vec2(pill_w, pill_h.min(plot_rect.height() - 2.0)),
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

fn draw_state_stamp(painter: &egui::Painter, plot_rect: egui::Rect, stamp_text: &str) {
    let galley = painter.layout_no_wrap(stamp_text.to_string(), theme::font_small(), theme::TEXT);
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

pub fn format_mm_ss_mmm(seconds: f32) -> String {
    let total_ms = (seconds.max(0.0) * 1000.0).round() as u64;
    let mins = total_ms / 60_000;
    let secs = (total_ms % 60_000) / 1000;
    let millis = total_ms % 1000;
    format!("{:02}:{:02}.{:03}", mins, secs, millis)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use parking_lot::Mutex;
    use pirate_sampler_dsp::{BufferDuration, RingBuffer};

    use super::*;

    const START: egui::Pos2 = egui::pos2(300.0, 150.0);

    /// Headless egui harness: drives draw_waveform_card with synthetic pointer input.
    struct Harness {
        ctx: egui::Context,
        ed: EditorState,
    }

    impl Harness {
        fn new(view: (f32, f32), selection: Option<(f32, f32)>) -> Self {
            let ring = RingBuffer::with_max_duration(48_000.0, BufferDuration::new(1.0));
            let mut ed = EditorState::new(Arc::new(Mutex::new(ring)));
            ed.view_range = view;
            ed.selection = selection;
            Self {
                ctx: egui::Context::default(),
                ed,
            }
        }

        fn frame(&mut self, events: Vec<egui::Event>) {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600.0, 300.0),
                )),
                events,
                ..Default::default()
            };
            let ctx = self.ctx.clone();
            let ed = &mut self.ed;
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    draw_waveform_card(
                        ui,
                        ed,
                        ui.available_height(),
                        48_000,
                        1.0,
                        48_000.0,
                        false,
                        false,
                    );
                });
            });
        }

        fn hover(&mut self, pos: egui::Pos2) {
            self.frame(vec![egui::Event::PointerMoved(pos)]);
            self.frame(vec![egui::Event::PointerMoved(pos)]);
        }

        fn button(&mut self, pos: egui::Pos2, button: egui::PointerButton, pressed: bool) {
            self.frame(vec![egui::Event::PointerButton {
                pos,
                button,
                pressed,
                modifiers: egui::Modifiers::default(),
            }]);
        }

        fn drag(&mut self, from: egui::Pos2, to: egui::Pos2, button: egui::PointerButton) {
            self.hover(from);
            self.button(from, button, true);
            for step in 1..=4 {
                let t = step as f32 / 4.0;
                self.frame(vec![egui::Event::PointerMoved(from + (to - from) * t)]);
            }
            self.button(to, button, false);
        }
    }

    #[test]
    fn formats_duration_badge_correctly() {
        assert_eq!(format_mm_ss_mmm(0.0), "00:00.000");
        assert_eq!(format_mm_ss_mmm(1.87), "00:01.870");
        assert_eq!(format_mm_ss_mmm(65.125), "01:05.125");
    }

    #[test]
    fn right_drag_pans_zoomed_view() {
        // A selection covering the buffer must not block panning.
        let mut h = Harness::new((0.25, 0.5), Some((0.0, 1.0)));
        h.drag(
            START,
            START + egui::vec2(60.0, 0.0),
            egui::PointerButton::Secondary,
        );

        // 60 px of a ~600 px plot shifts a 25%-wide view by roughly 0.025.
        let start = h.ed.view_range.0;
        assert!(
            (0.21..0.24).contains(&start),
            "right-drag should pan by the drag distance, got {start}"
        );
        assert!(
            (h.ed.view_range.1 - start - 0.25).abs() < 1e-4,
            "panning must keep the zoom level, got {:?}",
            h.ed.view_range
        );
        assert!(!h.ed.is_panning, "release must end panning");
        assert_eq!(
            h.ed.selection,
            Some((0.0, 1.0)),
            "panning must not touch the selection"
        );
    }

    #[test]
    fn right_drag_pans_full_view_in_both_directions() {
        for dx in [-80.0, 80.0] {
            let mut h = Harness::new((0.0, 1.0), None);
            h.drag(
                START,
                START + egui::vec2(dx, 0.0),
                egui::PointerButton::Secondary,
            );
            assert!(
                h.ed.view_range.0 * dx < 0.0,
                "view should follow the drag: {:?}",
                h.ed.view_range
            );
            assert!((h.ed.view_range.1 - h.ed.view_range.0 - 1.0).abs() < 1e-5);
            assert_eq!(h.ed.selection, None);
        }
    }

    #[test]
    fn right_drag_never_draws_a_selection() {
        let mut h = Harness::new((0.25, 0.5), None);
        h.drag(
            START,
            START + egui::vec2(80.0, 0.0),
            egui::PointerButton::Secondary,
        );

        assert_eq!(h.ed.selection, None, "right-drag must not select");
        assert!(h.ed.view_range.0 < 0.25, "right-drag must still pan");
    }

    #[test]
    fn left_drag_selects_audio_after_panning() {
        let mut h = Harness::new((-0.2, 0.8), None);
        h.drag(
            egui::pos2(200.0, 150.0),
            egui::pos2(380.0, 150.0),
            egui::PointerButton::Primary,
        );
        let (a, b) = h.ed.selection.expect("left drag should select");
        assert!(
            (0.1..0.2).contains(&a),
            "selection start must match the panned view: {a}"
        );
        assert!(
            (0.4..0.5).contains(&b),
            "selection end must match the panned view: {b}"
        );
    }

    #[test]
    fn left_click_without_drag_leaves_no_selection() {
        let mut h = Harness::new((0.0, 1.0), None);
        h.hover(START);
        h.button(START, egui::PointerButton::Primary, true);
        h.button(START, egui::PointerButton::Primary, false);

        assert_eq!(h.ed.selection, None, "a bare left click must not select");
    }

    #[test]
    fn left_drag_draws_a_selection() {
        let mut h = Harness::new((0.0, 1.0), None);
        h.drag(
            egui::pos2(200.0, 150.0),
            egui::pos2(380.0, 150.0),
            egui::PointerButton::Primary,
        );

        let (a, b) = h.ed.selection.expect("left drag should select");
        assert!(
            b - a > 0.1,
            "selection should follow the drag, got {a}..{b}"
        );
        assert_eq!(h.ed.view_range, (0.0, 1.0), "selecting must not pan");
        assert!(!h.ed.selection_body_press);
    }
}
