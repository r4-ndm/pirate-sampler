//! Persisted standalone window size.
//!
//! The standalone window is painted by egui inside an outer X11 window. `EguiState` only knows the
//! initial size, so the custom resize grip records its final geometry here. The next launch reads
//! this file before constructing `EguiState`, which makes both the outer and inner windows open at
//! the remembered size.
//!
//! Keep this separate from the DSP and editor state: it is a tiny GUI preference, not plugin state
//! that a host should save or restore.

use std::path::PathBuf;

pub(crate) const DEFAULT_WINDOW_SIZE: (u32, u32) = (840, 400);
pub(crate) const MIN_WINDOW_SIZE: (u32, u32) = (560, 44);
pub(crate) const MAX_WINDOW_SIZE: (u32, u32) = (16_384, 16_384);

const CONFIG_DIR: &str = "pirate-sampler";
const CONFIG_FILE: &str = "window-size";

/// Read the last saved size, if it exists and is valid.
pub(crate) fn load_window_size() -> Option<(u32, u32)> {
    let path = config_path()?;
    let text = std::fs::read_to_string(path).ok()?;
    parse_window_size(&text)
}

/// Write the current size, ignoring filesystem errors. This runs on the GUI thread only.
pub(crate) fn save_window_size(size: (u32, u32)) {
    let Some(size) = sanitize_window_size(size.0, size.1) else {
        return;
    };
    let Some(path) = config_path() else {
        return;
    };
    let Some(parent) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }

    // Write-and-rename so a crash mid-write cannot leave a truncated preference behind.
    let temp_path = path.with_extension("tmp");
    let contents = format!("{} {}\n", size.0, size.1);
    if std::fs::write(&temp_path, contents).is_ok() {
        let _ = std::fs::rename(temp_path, path);
    }
}

fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join(CONFIG_DIR).join(CONFIG_FILE))
}

fn parse_window_size(text: &str) -> Option<(u32, u32)> {
    let mut fields = text.split_whitespace();
    let width = fields.next()?.parse().ok()?;
    let height = fields.next()?.parse().ok()?;
    if fields.next().is_some() {
        return None;
    }
    sanitize_window_size(width, height)
}

fn sanitize_window_size(width: u32, height: u32) -> Option<(u32, u32)> {
    if width < MIN_WINDOW_SIZE.0 || height < MIN_WINDOW_SIZE.1 {
        return None;
    }
    Some((width.min(MAX_WINDOW_SIZE.0), height.min(MAX_WINDOW_SIZE.1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_saved_size() {
        assert_eq!(parse_window_size("1200 500\n"), Some((1200, 500)));
        assert_eq!(parse_window_size("  560 44 "), Some(MIN_WINDOW_SIZE));
    }

    #[test]
    fn rejects_malformed_or_short_files() {
        assert_eq!(parse_window_size(""), None);
        assert_eq!(parse_window_size("1200"), None);
        assert_eq!(parse_window_size("1200 500 extra"), None);
        assert_eq!(parse_window_size("wide 500"), None);
    }

    #[test]
    fn rejects_sizes_below_the_window_minimum() {
        assert_eq!(parse_window_size("559 400"), None);
        assert_eq!(parse_window_size("840 43"), None);
        assert_eq!(parse_window_size("0 0"), None);
    }

    #[test]
    fn clamps_absurdly_large_sizes() {
        assert_eq!(parse_window_size("999999 999999"), Some(MAX_WINDOW_SIZE));
    }
}
