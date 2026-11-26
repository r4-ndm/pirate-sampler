//! Waveform plot rendering, region selection hit-testing, edge resizing,
//! and direct drag-to-DAW interaction.

use nih_plug_egui::egui::{self, Color32};

use crate::clips::drag_out_selection;
use crate::editor::{pause_playback_off_lock, toggle_playback, EditorState, LoopSlide};
use crate::theme;

const EDGE_GRAB_PX: f32 = 8.0;
/// Right-button travel below this is a click (toggle playback) rather than a pan drag.
const SEC_CLICK_SLOP_PX: f32 = 4.0;

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
    is_empty: bool,
    _paused: bool,
) {
    let (plot_rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), card_h),
        egui::Sense::click_and_drag(),
    );
    let painter = ui.painter_at(plot_rect);

    painter.rect_filled(
        plot_rect,
        egui::CornerRadius::ZERO,
        Color32::from_rgb(19, 17, 12),
    );
    draw_bezel(ui, &painter, plot_rect);

    let mid = plot_rect.center().y;

    let num_buckets = ((plot_rect.width() / 7.0) as usize).clamp(1, 512);
    let (view_start, view_end) = ed.view_range;
    let visible_start = view_start.max(0.0);
    let visible_end = view_end.min(1.0);
    let visible_fraction = ((visible_end - visible_start) / (view_end - view_start)).max(0.0);
    let visible_buckets = ((num_buckets as f32 * visible_fraction).ceil() as usize).max(1);
    let (peaks, playhead) =
        waveform_snapshot(ed, num_buckets, visible_buckets, visible_start, visible_end);

    let glass_painter = painter.with_clip_rect(plot_rect.shrink(13.0));
    draw_peaks(&glass_painter, plot_rect, ed.view_range, &peaks, is_empty);
    // A low-opacity reflection sits over the LEDs, not over selection handles.
    let glass = plot_rect.shrink(13.0);
    if glass.is_positive() {
        glass_painter.add(egui::Shape::convex_polygon(
            vec![
                glass.left_top(),
                glass.right_top(),
                egui::pos2(glass.right(), glass.top() + glass.height() * 0.16),
                egui::pos2(glass.left(), glass.top() + glass.height() * 0.32),
            ],
            Color32::from_rgba_unmultiplied(255, 235, 195, 8),
            egui::Stroke::NONE,
        ));
    }

    let hovering_edge = handle_pointer_interaction(ui, &response, plot_rect, ed);

    if let Some((a, b)) = ed.selection {
        draw_selection_overlay(&painter, plot_rect, mid, a, b, hovering_edge, ed.view_range);
    }

    draw_playhead(&painter, plot_rect, ed.view_range, playhead);

    if ed.freeze_view {
        draw_state_stamp(&painter, plot_rect, "FROZEN");
    }
}

/// Scale the bezel's edges independently so its corners retain their proportions.
fn draw_bezel(ui: &egui::Ui, painter: &egui::Painter, rect: egui::Rect) {
    let id = egui::Id::new("waveform_bezel_texture");
    let texture = match ui
        .ctx()
        .data(|data| data.get_temp::<egui::TextureHandle>(id))
    {
        Some(texture) => texture,
        None => {
            // Size comes from the PNG header rather than being hardcoded here,
            // so re-exporting the bezel at another resolution needs no code change.
            let decoded = image::load_from_memory(include_bytes!(
                "../../../assets/waveform-bezel.png"
            ))
            .expect("waveform-bezel.png failed to decode")
            .to_rgba8();
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [decoded.width() as usize, decoded.height() as usize],
                decoded.as_raw(),
            );
            let texture =
                ui.ctx()
                    .load_texture("waveform_bezel", image, egui::TextureOptions::LINEAR);
            ui.ctx()
                .data_mut(|data| data.insert_temp(id, texture.clone()));
            texture
        }
    };
    let edge = 13.0_f32.min(rect.width() * 0.5).min(rect.height() * 0.5);
    let xs = [
        rect.left(),
        rect.left() + edge,
        rect.right() - edge,
        rect.right(),
    ];
    let ys = [
        rect.top(),
        rect.top() + edge,
        rect.bottom() - edge,
        rect.bottom(),
    ];
    let uv = [0.0, 32.0 / 192.0, 160.0 / 192.0, 1.0];
    for row in 0..3 {
        for col in 0..3 {
            if row == 1 && col == 1 {
                continue;
            }
            painter.image(
                texture.id(),
                egui::Rect::from_min_max(
                    egui::pos2(xs[col], ys[row]),
                    egui::pos2(xs[col + 1], ys[row + 1]),
                ),
                egui::Rect::from_min_max(
                    egui::pos2(uv[col], uv[row]),
                    egui::pos2(uv[col + 1], uv[row + 1]),
                ),
                Color32::WHITE,
            );
        }
    }
}

fn draw_peaks(
    painter: &egui::Painter,
    plot_rect: egui::Rect,
    view_range: (f32, f32),
    peaks: &[(f32, f32)],
    is_empty: bool,
) {
    let visible_start = view_range.0.max(0.0);
    let visible_end = view_range.1.min(1.0);

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
        // Each time bucket is a bottom-up column of horizontal LED segments.
        let pitch = 6.0;
        let height = (plot_rect.height() - 28.0).max(0.0);
        let segment_width = (plot_rect.width()
            / num_columns_for_view(peaks.len(), visible_start, visible_end, view_range))
        .min(5.0);
        for (i, (l, r)) in peaks.iter().enumerate() {
            let t = visible_start + (i as f32 + 0.5) / n * (visible_end - visible_start);
            let x = x_at_t(t, plot_rect, view_range);
            let raw_amp = l.max(*r).clamp(0.0, 1.0);
            let amplitude = if max_peak > 1e-6 {
                raw_amp / max_peak
            } else {
                0.0
            };
            let rows = (amplitude * height / pitch).floor() as i32;
            for row in 0..=rows {
                let y = plot_rect.bottom() - 14.0 - row as f32 * pitch;
                let color = if row as f32 * pitch > height * 0.8 {
                    theme::DANGER
                } else {
                    theme::ACCENT
                };
                painter.line_segment(
                    [
                        egui::pos2(x - segment_width * 0.5, y),
                        egui::pos2(x + segment_width * 0.5, y),
                    ],
                    egui::Stroke::new(2.0_f32, color),
                );
            }
        }
    }
}

fn num_columns_for_view(count: usize, start: f32, end: f32, view: (f32, f32)) -> f32 {
    let fraction = ((end - start) / (view.1 - view.0)).max(f32::EPSILON);
    (count as f32 / fraction).max(1.0)
}

/// One locked snapshot of the peaks and playhead for this frame.
///
/// Both reads share one ring lock, and the guard is dropped before interaction can mutate the
/// ring, so repainting never competes with an edit.
fn waveform_snapshot(
    ed: &mut EditorState,
    num_buckets: usize,
    visible_buckets: usize,
    visible_start: f32,
    visible_end: f32,
) -> (Vec<(f32, f32)>, Option<f32>) {
    let freeze = ed.freeze_view;
    let mut frozen = ed.frozen_peaks.take();
    let ring = ed.ring.lock();
    let peaks = if freeze {
        let needs_snap = frozen.as_ref().is_none_or(|v| v.len() != num_buckets);
        if needs_snap {
            frozen = Some(ring.waveform_peaks(num_buckets));
        }
        let snap = frozen.as_ref().expect("snapshot was just populated");
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
        ring.waveform_peaks_range(visible_buckets, visible_start, visible_end)
    };
    let playhead = ring.playhead_fraction();
    drop(ring);
    ed.frozen_peaks = frozen;
    (peaks, playhead)
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
    let secondary_released = input
        .pointer
        .button_released(egui::PointerButton::Secondary);
    // While a selection loops, a drag outside the brace scrubs the captured audio under the fixed
    // loop instead of drawing a new selection.
    let (looping, filled_frames) = {
        let ring = ed.ring.lock();
        (
            ed.selection.is_some() && ring.is_looping(),
            ring.filled_frames(),
        )
    };

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
    } else if hovering_inside_sel || (looping && pointer_in_plot.is_some()) {
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
                ed.edge_drag = false;
                // Never inherit a scrub from a gesture whose release we did not see.
                ed.loop_slide = None;
                if let Some((a, b)) = ed.selection {
                    let (start_n, end_n) = (a.min(b), a.max(b));
                    let x0 = x_at_t(start_n, plot_rect, view);
                    let x1 = x_at_t(end_n, plot_rect, view);
                    if (pos.x - x0).abs() <= EDGE_GRAB_PX {
                        ed.drag_anchor = Some(end_n);
                        ed.selection = Some((t, end_n));
                        ed.edge_drag = true;
                    } else if (pos.x - x1).abs() <= EDGE_GRAB_PX {
                        ed.drag_anchor = Some(start_n);
                        ed.selection = Some((start_n, t));
                        ed.edge_drag = true;
                    } else if pos.x > x0 && pos.x < x1 {
                        // A press inside the brace starts a clip drag for the DAW. A left drag
                        // never draws a new selection on top of an existing one.
                        ed.drag_anchor = None;
                        ed.selection_body_press = true;
                    } else if looping {
                        // Looping: dragging outside the brace moves the captured audio itself, so
                        // new material slides into the loop without stopping or moving the brace.
                        ed.drag_anchor = None;
                        ed.loop_slide = Some(LoopSlide {
                            anchor: t,
                            applied: 0,
                        });
                    } else {
                        // A press outside the brace starts a fresh selection and discards the
                        // old one; `Esc` is the way to clear it and start over.
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
                if ed.loop_slide.is_some() {
                    apply_loop_slide(ed, t, filled_frames);
                } else if let Some(anchor) = ed.drag_anchor {
                    ed.selection = Some((anchor, t));
                    // A running loop follows the brace live, so widening the selection auditions
                    // the wider range without stopping and restarting.
                    if ed.edge_drag && looping {
                        ed.ring.lock().resize_loop(anchor.min(t), anchor.max(t));
                    }
                } else if ed.selection_body_press && input.pointer.is_decidedly_dragging() {
                    ed.status = drag_out_selection(ed);
                    ed.selection_body_press = false;
                }
            }
        }
    }

    // Right button does double duty: a drag pans the view (handled above), while a press and
    // release that never travelled far enough to pan toggles selection playback.
    if secondary_pressed {
        if let Some(pos) = press_pos.or(pointer) {
            if plot_rect.contains(pos) {
                ed.sec_press = Some((pos, false));
            }
        }
    }
    if let (Some((origin, moved)), Some(pos)) = (ed.sec_press, pointer) {
        ed.sec_press = Some((origin, moved || (pos - origin).length() > SEC_CLICK_SLOP_PX));
    }
    if secondary_released {
        if let Some((origin, moved)) = ed.sec_press.take() {
            if !moved && plot_rect.contains(origin) {
                ed.status = toggle_playback(ed);
            }
        }
    }

    // Finish the left-button gesture: drop click-without-drag selections and clear the anchor.
    if primary_released {
        ed.selection_body_press = false;
        ed.edge_drag = false;
        ed.loop_slide = None;
        if ed.drag_anchor.take().is_some() {
            if let Some((a, b)) = ed.selection {
                let span = (view.1 - view.0).max(f32::EPSILON);
                if (b - a).abs() / span * plot_rect.width() < 2.0 {
                    ed.selection = None;
                    // A vanished brace must not leave a loop playing with nothing to show for it.
                    pause_playback_off_lock(ed);
                }
            }
        }
    }

    hovering_edge
}

/// Rotate the captured audio so it follows the pointer under the fixed loop.
///
/// The brace and the playhead stay put; the audio moves by `pointer_t - anchor` frames, wrapping
/// inside the filled capture, and the loop keeps auditioning whatever is underneath it.
fn apply_loop_slide(ed: &mut EditorState, pointer_t: f32, filled: usize) {
    let Some(mut slide) = ed.loop_slide else {
        return;
    };
    if filled == 0 {
        return;
    }
    let desired = ((pointer_t - slide.anchor) * filled as f32).round() as i64;
    let delta = desired - slide.applied;
    if delta == 0 {
        return;
    }
    slide.applied += ed.ring.lock().shift_content(delta);
    ed.loop_slide = Some(slide);
}

#[allow(clippy::too_many_arguments)]
fn draw_selection_overlay(
    painter: &egui::Painter,
    plot_rect: egui::Rect,
    mid: f32,
    a: f32,
    b: f32,
    active_edge: bool,
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
        Color32::from_rgba_unmultiplied(0xDB, 0x9D, 0x44, 40),
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
}

fn draw_playhead(
    painter: &egui::Painter,
    plot_rect: egui::Rect,
    view_range: (f32, f32),
    playhead: Option<f32>,
) {
    let Some(t) = playhead else {
        return;
    };
    let (view_start, view_end) = view_range;
    if t < view_start || t > view_end {
        return;
    }
    let x = x_at_t(t, plot_rect, view_range);
    painter.line_segment(
        [
            egui::pos2(x, plot_rect.top() + 1.0),
            egui::pos2(x, plot_rect.bottom() - 1.0),
        ],
        egui::Stroke::new(1.5_f32, theme::ACCENT_HOVER),
    );
    let tri_w = 4.0;
    let tri_h = 6.0_f32.min(plot_rect.height() * 0.4);
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(x - tri_w, plot_rect.top() + 1.0),
            egui::pos2(x + tri_w, plot_rect.top() + 1.0),
            egui::pos2(x, plot_rect.top() + 1.0 + tri_h),
        ],
        theme::ACCENT_HOVER,
        egui::Stroke::NONE,
    ));
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
                    draw_waveform_card(ui, ed, ui.available_height(), false, false);
                });
            });
        }

        /// Record `frames` frames so playback has audio to read.
        fn fill(&mut self, frames: usize) {
            let mut ring = self.ed.ring.lock();
            for i in 0..frames {
                ring.write_frame(i as f32, -(i as f32));
            }
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

    #[test]
    fn press_inside_the_brace_arms_a_clip_drag_instead_of_a_new_selection() {
        let mut h = Harness::new((0.0, 1.0), Some((0.25, 0.5)));
        h.fill(48_000);
        assert!(h.ed.ring.lock().loop_range(0.25, 0.5, 0.25));

        // Well inside the brace, away from both edge handles.
        let inside = egui::pos2(250.0, 150.0);
        h.hover(inside);
        h.button(inside, egui::PointerButton::Primary, true);

        assert_eq!(
            h.ed.selection,
            Some((0.25, 0.5)),
            "a press inside the brace must not draw a new selection"
        );
        assert!(
            h.ed.selection_body_press,
            "a press inside the brace must arm the clip drag-out"
        );
        assert_eq!(h.ed.drag_anchor, None, "no new selection anchor");
        assert_eq!(h.ed.loop_slide, None, "inside the brace must not scrub");
        assert!(h.ed.ring.lock().is_playing(), "the loop must keep running");

        h.button(inside, egui::PointerButton::Primary, false);
        assert!(!h.ed.selection_body_press, "release must disarm the drag");
    }

    #[test]
    fn drag_outside_the_loop_moves_the_audio_under_it() {
        let mut h = Harness::new((0.0, 1.0), Some((0.25, 0.5)));
        h.fill(48_000);
        assert!(h.ed.ring.lock().loop_range(0.25, 0.5, 0.25));

        h.drag(
            egui::pos2(480.0, 150.0),
            egui::pos2(420.0, 150.0),
            egui::PointerButton::Primary,
        );

        assert_eq!(
            h.ed.selection,
            Some((0.25, 0.5)),
            "scrubbing must not move the loop brace"
        );
        let shift = h.ed.ring.lock().shift_frames();
        assert!(
            (-6_000..=-4_000).contains(&shift),
            "dragging left must move earlier audio under the loop, got {shift}"
        );
        assert!(
            h.ed.ring.lock().is_playing(),
            "scrubbing must not stop the loop"
        );
        assert_eq!(h.ed.loop_slide, None, "release must end the scrub");
        assert!(
            h.ed.ring.lock().read_logical(12_000) != Some((12_000.0, -12_000.0)),
            "the loop must now read different audio"
        );
    }

    #[test]
    fn dragging_the_loop_edge_resizes_the_running_loop() {
        let mut h = Harness::new((0.0, 1.0), Some((0.25, 0.5)));
        h.fill(48_000);
        assert!(h.ed.ring.lock().loop_range(0.25, 0.5, 0.25));

        // The right brace sits at x ~= 300; drag it right to widen the loop.
        h.drag(
            egui::pos2(300.0, 150.0),
            egui::pos2(373.0, 150.0),
            egui::PointerButton::Primary,
        );

        let (start, end) = h.ed.selection.expect("the brace is still selected");
        assert!(
            (start - 0.25).abs() < 0.01,
            "the far edge must stay put, got {start}"
        );
        assert!(
            (0.61..0.64).contains(&end),
            "the dragged edge must follow the pointer, got {end}"
        );
        assert!(
            h.ed.ring.lock().is_playing(),
            "resizing must not stop the loop"
        );
        assert!(!h.ed.edge_drag, "release must end the resize");

        // The wider window must reach past the old end (0.5 * 48_000 = 24_000 frames).
        let mut ring = h.ed.ring.lock();
        let mut max_seen = f32::MIN;
        for _ in 0..20_000 {
            if let Some((v, _)) = ring.next_playback_frame() {
                max_seen = max_seen.max(v);
            }
        }
        assert!(
            max_seen > 28_000.0,
            "the widened loop must play past the old end, saw {max_seen}"
        );
    }

    #[test]
    fn collapsing_a_playing_loop_stops_playback() {
        let mut h = Harness::new((0.0, 1.0), Some((0.25, 0.5)));
        h.fill(48_000);
        assert!(h.ed.ring.lock().loop_range(0.25, 0.5, 0.25));

        // The right brace is at x ~= 300; drag it onto the left brace at x ~= 154.
        h.drag(
            egui::pos2(300.0, 150.0),
            egui::pos2(155.5, 150.0),
            egui::PointerButton::Primary,
        );

        assert_eq!(h.ed.selection, None, "a sub-2px brace is discarded");
        assert!(
            !h.ed.ring.lock().is_playing(),
            "a discarded brace must not leave audio looping"
        );
    }

    #[test]
    fn right_click_toggles_playback_without_selection() {
        let mut h = Harness::new((0.0, 1.0), None);
        h.fill(48_000);
        h.hover(START);
        h.button(START, egui::PointerButton::Secondary, true);
        h.button(START, egui::PointerButton::Secondary, false);

        assert!(
            h.ed.ring.lock().is_playing(),
            "a right click must start playback"
        );
        h.button(START, egui::PointerButton::Secondary, true);
        h.button(START, egui::PointerButton::Secondary, false);
        assert!(
            !h.ed.ring.lock().is_playing(),
            "second click stops playback"
        );
        assert_eq!(h.ed.view_range, (0.0, 1.0), "a right click must not pan");
    }

    #[test]
    fn right_click_loops_the_selection_until_clicked_again() {
        let mut h = Harness::new((0.0, 1.0), Some((0.0, 0.8)));
        h.fill(48_000);
        h.hover(START);
        h.button(START, egui::PointerButton::Secondary, true);
        h.button(START, egui::PointerButton::Secondary, false);

        {
            let mut ring = h.ed.ring.lock();
            assert!(ring.is_playing(), "a right click must start playback");
            assert_eq!(ring.playhead_fraction(), Some(0.0));
            for _ in 0..48_000 {
                assert!(ring.next_playback_frame().is_some(), "selection must loop");
            }
            assert!(ring.is_playing());
        }
        h.button(START, egui::PointerButton::Secondary, true);
        h.button(START, egui::PointerButton::Secondary, false);
        assert!(
            !h.ed.ring.lock().is_playing(),
            "second click stops the loop"
        );
    }

    #[test]
    fn right_drag_pans_without_starting_playback() {
        let mut h = Harness::new((0.25, 0.5), None);
        h.fill(48_000);
        h.drag(
            START,
            START + egui::vec2(60.0, 0.0),
            egui::PointerButton::Secondary,
        );

        assert!(
            !h.ed.ring.lock().is_playing(),
            "panning must not start playback"
        );
        assert!(h.ed.view_range.0 < 0.25, "right-drag must still pan");
    }
}
