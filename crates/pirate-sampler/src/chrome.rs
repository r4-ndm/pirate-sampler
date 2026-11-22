//! Frameless window chrome for the standalone build.
//!
//! The standalone runs two X11 windows: the outer top-level window (owned by
//! nih-plug's standalone wrapper) and the egui child window inside it. Everything
//! here targets the *outer* window, whose id we get from nih_plug_egui once the
//! editor is spawned.
//!
//! * `attach`     — strip the WM decorations (`_MOTIF_WM_HINTS`), restore app size
//! * `minimize`   — ICCCM `WM_CHANGE_STATE` -> `IconicState`
//! * `begin_move` — start a drag; `pump_move` follows the pointer until release
//!
//! The drag moves the window ourselves instead of asking the WM through
//! `_NET_WM_MOVERESIZE`: KWin rejects that request when the press is not a
//! trusted input event (which is also why it could not be verified with
//! synthetic input), whereas a self-driven move behaves the same to the user.
//!
//! This module is standalone-only: `attach` returns early unless [`crate::is_standalone()`], which
//! leaves the chrome store empty and every other entry point inert. Inside a host the window id we
//! are handed belongs to the DAW, and taking it over would strip the host's decorations and move
//! its window. Every entry point is also a no-op on a non-X11 platform, so the GUI never depends on
//! this module succeeding.

#[cfg(target_os = "linux")]
pub use imp::{
    attach, begin_move, begin_resize, ensure_min_height, minimize, pump_move, pump_resize,
    set_always_on_top,
};

/// Anything that is not X11 (or not supported) gets no-ops.
#[cfg(not(target_os = "linux"))]
mod imp {
    pub fn attach(_window: u32, _width: u32, _height: u32) {}
    pub fn minimize() {}
    pub fn begin_move() {}
    pub fn pump_move() {}
    pub fn begin_resize() {}
    pub fn pump_resize() {}
    pub fn set_always_on_top(_on_top: bool) {}
    pub fn ensure_min_height(_min_h: Option<u32>) {}
}

#[cfg(target_os = "linux")]
mod imp {
    use std::sync::Mutex;

    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{
        ClientMessageEvent, ConfigureWindowAux, ConnectionExt as _, EventMask, KeyButMask,
        PropMode, Window,
    };
    use x11rb::rust_connection::RustConnection;
    use x11rb::wrapper::ConnectionExt as _;

    struct Chrome {
        conn: RustConnection,
        root: Window,
        window: Window,
    }

    /// A drag in progress, in root coordinates.
    #[derive(Clone, Copy)]
    struct MoveState {
        pointer: (i32, i32),
        window: (i32, i32),
        applied: (i32, i32),
    }

    /// A resize in progress, in root coordinates.
    #[derive(Clone, Copy)]
    struct ResizeState {
        pointer: (i32, i32),
        size: (u32, u32),
        applied: (u32, u32),
    }

    /// Keep the window usable: the side panel plus a workable waveform. The height
    /// floor is low on purpose — the panel drops to a single row of controls.
    const MIN_W: i32 = 560;
    const MIN_H: i32 = 44;

    static CHROME: Mutex<Option<Chrome>> = Mutex::new(None);
    static MOVE: Mutex<Option<MoveState>> = Mutex::new(None);
    static RESIZE: Mutex<Option<ResizeState>> = Mutex::new(None);
    static MENU_EXPANDED_FROM: Mutex<Option<u32>> = Mutex::new(None);

    fn intern(conn: &RustConnection, name: &[u8]) -> Option<u32> {
        conn.intern_atom(false, name)
            .ok()?
            .reply()
            .ok()
            .map(|reply| reply.atom)
    }

    fn with_chrome<R>(f: impl FnOnce(&Chrome) -> R) -> Option<R> {
        let guard = CHROME.lock().ok()?;
        guard.as_ref().map(f)
    }

    fn pointer_position(chrome: &Chrome) -> Option<(i32, i32)> {
        let reply = chrome.conn.query_pointer(chrome.root).ok()?.reply().ok()?;
        Some((reply.root_x as i32, reply.root_y as i32))
    }

    /// Pointer position in root coordinates plus whether the left button is down.
    fn pointer_state(chrome: &Chrome) -> Option<((i32, i32), bool)> {
        let reply = chrome.conn.query_pointer(chrome.root).ok()?.reply().ok()?;
        Some((
            (reply.root_x as i32, reply.root_y as i32),
            reply.mask.contains(KeyButMask::BUTTON1),
        ))
    }

    /// Strip the window-manager decorations and remember the connection.
    /// Safe to call every frame: it returns immediately once this window is attached.
    pub fn attach(window: u32, width: u32, height: u32) {
        // Only the standalone owns the window it draws into.
        if !crate::is_standalone() {
            return;
        }
        if let Ok(guard) = CHROME.lock() {
            if let Some(chrome) = guard.as_ref() {
                if chrome.window == window {
                    return;
                }
            }
        }
        let Ok((conn, screen_num)) = x11rb::connect(None) else {
            return;
        };
        let root = conn.setup().roots[screen_num].root;

        // _MOTIF_WM_HINTS: flags = MWM_HINTS_DECORATIONS (2), decorations = 0
        if let Some(motif) = intern(&conn, b"_MOTIF_WM_HINTS") {
            let _ = conn.change_property32(
                PropMode::REPLACE,
                window,
                motif,
                motif,
                &[2u32, 0, 0, 0, 0],
            );
        }
        // KWin hands the titlebar's space back to the client when the decorations go
        // away, which would leave the window taller than the layout asks for. Put it
        // back to the app's own size; the egui child follows via the resize hook.
        let _ = conn.configure_window(
            window,
            &ConfigureWindowAux::new().width(width).height(height),
        );
        let _ = conn.flush();

        if let Ok(mut slot) = CHROME.lock() {
            *slot = Some(Chrome { conn, root, window });
        }
        set_always_on_top(true);
    }

    fn send_client_message(chrome: &Chrome, message_type: u32, data: [u32; 5]) {
        let event = ClientMessageEvent::new(32, chrome.window, message_type, data);
        let _ = chrome.conn.send_event(
            false,
            chrome.root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            event,
        );
        let _ = chrome.conn.flush();
    }

    /// Set or clear the EWMH _NET_WM_STATE_ABOVE flag (Always on Top).
    pub fn set_always_on_top(on_top: bool) {
        with_chrome(|chrome| {
            let Some(net_wm_state) = intern(&chrome.conn, b"_NET_WM_STATE") else {
                return;
            };
            let Some(net_wm_state_above) = intern(&chrome.conn, b"_NET_WM_STATE_ABOVE") else {
                return;
            };
            // _NET_WM_STATE action: 0 = _NET_WM_STATE_REMOVE, 1 = _NET_WM_STATE_ADD
            let action = if on_top { 1u32 } else { 0u32 };
            send_client_message(chrome, net_wm_state, [action, net_wm_state_above, 0, 1, 0]);
        });
    }

    /// Temporarily expand the window downward when  is  and the
    /// current window height is smaller than , then restore the original
    /// compact height when  becomes .
    pub fn ensure_min_height(min_h: Option<u32>) {
        with_chrome(|chrome| match min_h {
            Some(req_h) => {
                let already_checked = MENU_EXPANDED_FROM
                    .lock()
                    .ok()
                    .map(|g| g.is_some())
                    .unwrap_or(false);
                if already_checked {
                    return;
                }
                let Ok(cookie) = chrome.conn.get_geometry(chrome.window) else {
                    return;
                };
                let Ok(geom) = cookie.reply() else {
                    return;
                };
                let cur_h = geom.height as u32;
                if cur_h < req_h {
                    if let Ok(mut slot) = MENU_EXPANDED_FROM.lock() {
                        *slot = Some(cur_h);
                    }
                    let _ = chrome
                        .conn
                        .configure_window(chrome.window, &ConfigureWindowAux::new().height(req_h));
                    let _ = chrome.conn.flush();
                } else if let Ok(mut slot) = MENU_EXPANDED_FROM.lock() {
                    // Mark as checked with cur_h so we don't query geometry every frame while open
                    *slot = Some(cur_h);
                }
            }
            None => {
                let prev = MENU_EXPANDED_FROM.lock().ok().and_then(|mut g| g.take());
                if let Some(orig_h) = prev {
                    let Ok(cookie) = chrome.conn.get_geometry(chrome.window) else {
                        return;
                    };
                    let Ok(geom) = cookie.reply() else {
                        return;
                    };
                    if (geom.height as u32) != orig_h {
                        let _ = chrome.conn.configure_window(
                            chrome.window,
                            &ConfigureWindowAux::new().height(orig_h),
                        );
                        let _ = chrome.conn.flush();
                    }
                }
            }
        });
    }

    /// ICCCM minimize: ask the WM to iconify the window.
    pub fn minimize() {
        with_chrome(|chrome| {
            let Some(wm_change_state) = intern(&chrome.conn, b"WM_CHANGE_STATE") else {
                return;
            };
            // IconicState = 3
            send_client_message(chrome, wm_change_state, [3, 0, 0, 0, 0]);
        });
    }

    /// Remember where the pointer and the window were when the drag started.
    pub fn begin_move() {
        if let Ok(guard) = RESIZE.lock() {
            if guard.is_some() {
                // Never start moving the window while a resize is active.
                return;
            }
        }
        if let Ok(guard) = MOVE.lock() {
            if guard.is_some() {
                // A second draggable zone starting mid-drag must not re-base the anchor.
                return;
            }
        }
        with_chrome(|chrome| {
            let Some(pointer) = pointer_position(chrome) else {
                return;
            };
            let Ok(origin) = chrome
                .conn
                .translate_coordinates(chrome.window, chrome.root, 0, 0)
                .ok()
                .and_then(|cookie| cookie.reply().ok())
                .ok_or(())
            else {
                return;
            };
            if let Ok(mut slot) = MOVE.lock() {
                *slot = Some(MoveState {
                    pointer,
                    window: (origin.dst_x as i32, origin.dst_y as i32),
                    applied: (origin.dst_x as i32, origin.dst_y as i32),
                });
            }
        });
    }

    /// Remember where the pointer and the window size were when the grip was pressed.
    pub fn begin_resize() {
        if let Ok(mut exp) = MENU_EXPANDED_FROM.lock() {
            *exp = None;
        }
        if let Ok(mut move_slot) = MOVE.lock() {
            // Cancel any move that panel_bg_drag may have started in the same frame.
            *move_slot = None;
        }
        if let Ok(guard) = RESIZE.lock() {
            if guard.is_some() {
                return;
            }
        }
        with_chrome(|chrome| {
            let Some(pointer) = pointer_position(chrome) else {
                return;
            };
            let Ok(cookie) = chrome.conn.get_geometry(chrome.window) else {
                return;
            };
            let Ok(geometry) = cookie.reply() else {
                return;
            };
            let size = (geometry.width as u32, geometry.height as u32);
            if let Ok(mut slot) = RESIZE.lock() {
                *slot = Some(ResizeState {
                    pointer,
                    size,
                    applied: size,
                });
            }
        });
    }

    /// Follow the pointer while the grip is held, resizing the window from its top-left.
    pub fn pump_resize() {
        let mut state = match RESIZE.lock() {
            Ok(guard) => match *guard {
                Some(state) => state,
                None => return,
            },
            Err(_) => return,
        };

        with_chrome(|chrome| {
            let Some((pointer, down)) = pointer_state(chrome) else {
                return;
            };
            let width = (state.size.0 as i32 + (pointer.0 - state.pointer.0)).max(MIN_W) as u32;
            let height = (state.size.1 as i32 + (pointer.1 - state.pointer.1)).max(MIN_H) as u32;
            let target = (width, height);
            if target != state.applied {
                let _ = chrome.conn.configure_window(
                    chrome.window,
                    &ConfigureWindowAux::new().width(width).height(height),
                );
                let _ = chrome.conn.flush();
                state.applied = target;
                if let Ok(mut slot) = RESIZE.lock() {
                    *slot = Some(state);
                }
            }
            if !down {
                if let Ok(mut slot) = RESIZE.lock() {
                    *slot = None;
                }
            }
        });
    }

    /// Follow the pointer while the left button is held; release ends the drag.
    pub fn pump_move() {
        let mut state = match MOVE.lock() {
            Ok(guard) => match *guard {
                Some(state) => state,
                None => return,
            },
            Err(_) => return,
        };

        with_chrome(|chrome| {
            let Some((pointer, down)) = pointer_state(chrome) else {
                return;
            };
            let target = (
                state.window.0 + (pointer.0 - state.pointer.0),
                state.window.1 + (pointer.1 - state.pointer.1),
            );
            // Re-issuing an identical move makes KWin drift the frame, so only send
            // the position when it actually changed. The final move is applied even
            // on the release frame, so the window lands exactly under the pointer.
            if target != state.applied {
                let _ = chrome.conn.configure_window(
                    chrome.window,
                    &ConfigureWindowAux::new().x(target.0).y(target.1),
                );
                let _ = chrome.conn.flush();
                state.applied = target;
                if let Ok(mut slot) = MOVE.lock() {
                    *slot = Some(state);
                }
            }
            if !down {
                if let Ok(mut slot) = MOVE.lock() {
                    *slot = None;
                }
            }
        });
    }
}
