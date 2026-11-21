//! Waveform plot rendering, region selection hit-testing, edge resizing,
//! and direct drag-to-DAW interaction.

use nih_plug_egui::egui::{self, Color32};

use crate::clips::drag_out_selection;
use crate::editor::EditorState;
use crate::theme;

const EDGE_GRAB_PX: f32 = 8.0;

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
            hovering_edge || response.dragged(),
            filled_frames,
            cap_secs,
            sr,
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
    let peaks = if ed.freeze_view {
        let needs_snap = ed
            .frozen_peaks
            .as_ref()
            .is_none_or(|v| v.len() != num_buckets);
        if needs_snap {
            let snap = ed.ring.lock().waveform_peaks(num_buckets);
            ed.frozen_peaks = Some(snap);
        }
        ed.frozen_peaks.clone().unwrap_or_default()
    } else {
        ed.ring.lock().waveform_peaks(num_buckets)
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
}

fn handle_pointer_interaction(
    ui: &mut egui::Ui,
    response: &egui::Response,
    plot_rect: egui::Rect,
    ed: &mut EditorState,
) -> bool {
    let pointer_pos = response
        .hover_pos()
        .or_else(|| response.interact_pointer_pos());
    let mut hovering_edge = false;
    let mut hovering_inside_sel = false;

    if let (Some(pos), Some((a, b))) = (pointer_pos, ed.selection) {
        if plot_rect.contains(pos) {
            let x0 = plot_rect.left() + a.min(b) * plot_rect.width();
            let x1 = plot_rect.left() + a.max(b) * plot_rect.width();
            if (pos.x - x0).abs() <= EDGE_GRAB_PX || (pos.x - x1).abs() <= EDGE_GRAB_PX {
                hovering_edge = true;
            } else if pos.x > x0 && pos.x < x1 {
                hovering_inside_sel = true;
            }
        }
    }

    if hovering_edge {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    } else if hovering_inside_sel {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    } else if let Some(pos) = pointer_pos {
        if plot_rect.contains(pos) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
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
                if (pos.x - x0).abs() <= EDGE_GRAB_PX {
                    ed.drag_anchor = Some(end_n);
                    ed.selection = Some((t, end_n));
                } else if (pos.x - x1).abs() <= EDGE_GRAB_PX {
                    ed.drag_anchor = Some(start_n);
                    ed.selection = Some((start_n, t));
                } else if pos.x > x0 && pos.x < x1 {
                    ed.status = drag_out_selection(ed);
                    ed.drag_anchor = None;
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
) {
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

    let handle_w = 6.0;
    let handle_h = 28.0_f32.min((plot_rect.height() - 6.0).max(8.0));
    let handle_color = if active_edge {
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
    use super::*;

    #[test]
    fn formats_duration_badge_correctly() {
        assert_eq!(format_mm_ss_mmm(0.0), "00:00.000");
        assert_eq!(format_mm_ss_mmm(1.87), "00:01.870");
        assert_eq!(format_mm_ss_mmm(65.125), "01:05.125");
    }
}
