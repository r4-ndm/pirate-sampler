//! Selection resolution, WAV clip rendering, clipboard copy, save dialog export,
//! and native X11 drag-out orchestration.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use nih_plug_egui::egui;
use pirate_sampler_dsp::{write_selection_wav, RingBuffer, Selection};

use crate::editor::EditorState;
use crate::xdnd;

/// Resolves normalized `(0.0..1.0)` selection bounds from `EditorState` into sample frame indices
/// and the current sample rate.
pub fn resolve_selection(ed: &EditorState) -> Result<(Selection, f32), String> {
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
    let end = ((end_n * filled as f32).ceil() as usize)
        .min(filled)
        .max(start + 1);
    let sel = Selection::new(start, end).ok_or_else(|| "Invalid selection bounds.".to_string())?;
    let sr = ring.sample_rate();
    Ok((sel, sr))
}

/// Returns the user's clip cache directory (`~/.cache/pirate-sampler/clips`), creating it if needed.
pub fn get_clips_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    let clips = base.join("pirate-sampler").join("clips");
    let _ = std::fs::create_dir_all(&clips);
    clips
}

/// Opens the clips cache directory in the system file manager.
pub fn open_clips_folder() -> String {
    let dir = get_clips_dir();
    let _ = Command::new("xdg-open").arg(&dir).spawn();
    format!("Opened {}", dir.display())
}

/// Renders the current selection to a timestamped WAV file in the clips cache directory
/// and updates `latest.wav`. Returns `(filename, full_path)`.
fn render_selection_to_cache(
    ring: &parking_lot::Mutex<RingBuffer>,
    sel: Selection,
    sr: f32,
) -> Result<(String, PathBuf), String> {
    let dir = get_clips_dir();
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let filename = format!("clip_{:.2}s_{}.wav", sel.len() as f32 / sr, timestamp);
    let file_path = dir.join(&filename);

    {
        let ring_guard = ring.lock();
        write_selection_wav(&ring_guard, sel, &file_path)
            .map_err(|e| format!("Failed to render clip: {e}"))?;
    }

    let latest_path = dir.join("latest.wav");
    let _ = std::fs::copy(&file_path, &latest_path);

    Ok((filename, file_path))
}

/// Renders the current selection to a WAV file and immediately starts a grab-less X11 XDND
/// drag session attached to the user's held mouse cursor.
pub fn drag_out_selection(ed: &EditorState) -> String {
    let (sel, sr) = match resolve_selection(ed) {
        Ok(s) => s,
        Err(e) => return e,
    };

    let (filename, file_path) = match render_selection_to_cache(&ed.ring, sel, sr) {
        Ok(res) => res,
        Err(e) => return e,
    };

    xdnd::start_file_drag(file_path);
    format!("Drag {}", filename)
}

/// Renders the current selection to the clips directory and copies its `file://` URI
/// to the system clipboard (X11, Wayland, and egui).
pub fn copy_selection_to_clipboard(ed: &EditorState, ctx: &egui::Context) -> String {
    let (sel, sr) = match resolve_selection(ed) {
        Ok(s) => s,
        Err(e) => return e,
    };

    let (filename, file_path) = match render_selection_to_cache(&ed.ring, sel, sr) {
        Ok(res) => res,
        Err(e) => return e,
    };

    let canonical = file_path.canonicalize().unwrap_or(file_path);
    let uri = format!("file://{}\r\n", canonical.display());
    let path_str = canonical.display().to_string();

    ctx.copy_text(uri.clone());

    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        let _ = Command::new("wl-copy")
            .args(["-t", "text/uri-list", &uri])
            .spawn();
    }

    if std::env::var_os("DISPLAY").is_some() {
        for args in [
            &["-selection", "clipboard", "-t", "text/uri-list"][..],
            &["-selection", "clipboard"][..],
        ] {
            let payload = if args.contains(&"text/uri-list") {
                uri.as_bytes()
            } else {
                path_str.as_bytes()
            };
            if let Ok(mut child) = Command::new("xclip")
                .args(args)
                .stdin(Stdio::piped())
                .spawn()
            {
                if let Some(mut stdin) = child.stdin.take() {
                    use std::io::Write;
                    let _ = stdin.write_all(payload);
                }
            }
        }
    }

    format!("Copied {}", filename)
}

/// Opens a native save-file dialog (`rfd`) to export the selected audio region to a user-chosen path.
pub fn export_selection(ed: &EditorState) -> String {
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
