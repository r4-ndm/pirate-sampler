//! Native X11 XDND (X Drag and Drop v5) drag-out session.
//!
//! Deep module that hides the entire X11 XDND protocol behind a single entry point: [`start_file_drag`].
//!
//!Unlike external popup wrappers (`ripdrag`, `dragon`) or button-grab tools (`drop`), this
//! implementation operates grab-lessly on a background thread using `x11rb`:
//! 1. It tracks the user's already-held left mouse button (`Button1`) via `QueryPointer`.
//! 2. It walks the X11 window hierarchy under the cursor to find the deepest `XdndAware` window (e.g. Bitwig Studio).
//! 3. It answers `SelectionRequest` for both `TARGETS` and `text/uri-list` during the hover phase so hosts
//!    like Bitwig Studio render live drop-target previews.
//! 4. The instant the user releases `Button1`, it fires `XdndDrop` and completes the `XdndFinished` handshake.

use std::path::PathBuf;

/// Spawns a non-blocking native X11 XDND drag session for `file_path` that follows
/// the currently held mouse button and drops on release.
pub fn start_file_drag(file_path: PathBuf) {
    #[cfg(target_os = "linux")]
    std::thread::spawn(move || {
        let _ = imp::run_xdnd_session(&file_path);
    });
}

#[cfg(target_os = "linux")]
mod imp {
    use std::path::Path;
    use std::time::Duration;

    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{
        Atom, AtomEnum, ClientMessageEvent, ConnectionExt as _, CreateWindowAux, EventMask,
        KeyButMask, PropMode, SelectionNotifyEvent, SelectionRequestEvent, Window, WindowClass,
    };
    use x11rb::protocol::Event;
    use x11rb::rust_connection::RustConnection;
    use x11rb::wrapper::ConnectionExt as _;

    const XDND_VERSION: u32 = 5;

    struct Atoms {
        xdnd_selection: Atom,
        xdnd_aware: Atom,
        xdnd_enter: Atom,
        xdnd_position: Atom,
        xdnd_status: Atom,
        xdnd_leave: Atom,
        xdnd_drop: Atom,
        xdnd_finished: Atom,
        xdnd_action_copy: Atom,
        xdnd_action_list: Atom,
        mime_urilist: Atom,
        mime_string: Atom,
        mime_utf8: Atom,
        mime_plain: Atom,
        targets: Atom,
    }

    impl Atoms {
        fn intern_all(conn: &RustConnection) -> Option<Self> {
            let intern = |name: &[u8]| -> Option<Atom> {
                conn.intern_atom(false, name)
                    .ok()?
                    .reply()
                    .ok()
                    .map(|r| r.atom)
            };
            Some(Self {
                xdnd_selection: intern(b"XdndSelection")?,
                xdnd_aware: intern(b"XdndAware")?,
                xdnd_enter: intern(b"XdndEnter")?,
                xdnd_position: intern(b"XdndPosition")?,
                xdnd_status: intern(b"XdndStatus")?,
                xdnd_leave: intern(b"XdndLeave")?,
                xdnd_drop: intern(b"XdndDrop")?,
                xdnd_finished: intern(b"XdndFinished")?,
                xdnd_action_copy: intern(b"XdndActionCopy")?,
                xdnd_action_list: intern(b"XdndActionList")?,
                mime_urilist: intern(b"text/uri-list")?,
                mime_string: intern(b"STRING")?,
                mime_utf8: intern(b"UTF8_STRING")?,
                mime_plain: intern(b"text/plain")?,
                targets: intern(b"TARGETS")?,
            })
        }
    }

    pub fn run_xdnd_session(file_path: &Path) -> Option<()> {
        let canonical = file_path
            .canonicalize()
            .unwrap_or_else(|_| file_path.to_path_buf());
        let urilist = format!("file://{}\r\n", canonical.display());
        let strlist = format!("file://{}", canonical.display());

        let (conn, screen_num) = x11rb::connect(None).ok()?;
        let root = conn.setup().roots[screen_num].root;
        let win = conn.generate_id().ok()?;

        conn.create_window(
            x11rb::COPY_FROM_PARENT as u8,
            win,
            root,
            -100,
            -100,
            1,
            1,
            0,
            WindowClass::INPUT_ONLY,
            x11rb::COPY_FROM_PARENT,
            &CreateWindowAux::new(),
        )
        .ok()?;

        let atoms = Atoms::intern_all(&conn)?;
        conn.set_selection_owner(win, atoms.xdnd_selection, x11rb::CURRENT_TIME)
            .ok()?;
        conn.change_property32(
            PropMode::REPLACE,
            win,
            atoms.xdnd_action_list,
            AtomEnum::ATOM,
            &[atoms.xdnd_action_copy],
        )
        .ok()?;
        conn.flush().ok()?;

        let mut current_target: Window = x11rb::NONE;
        let mut last_pos = (-1i32, -1i32);
        let mut target_accepted = false;
        let mut dropped = false;
        let mut post_drop_ticks = 0u32;

        loop {
            while let Ok(Some(event)) = conn.poll_for_event() {
                match event {
                    Event::SelectionRequest(req) => {
                        handle_selection_request(&conn, &atoms, &req, &urilist, &strlist);
                    }
                    Event::ClientMessage(msg) => {
                        let data = msg.data.as_data32();
                        if msg.type_ == atoms.xdnd_status {
                            if data[0] == current_target {
                                target_accepted = (data[1] & 1) != 0;
                            }
                        } else if msg.type_ == atoms.xdnd_finished {
                            let _ = conn.destroy_window(win);
                            let _ = conn.flush();
                            return Some(());
                        }
                    }
                    _ => {}
                }
            }

            if dropped {
                post_drop_ticks += 1;
                if post_drop_ticks > 150 {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }

            let Ok(ptr_cookie) = conn.query_pointer(root) else {
                break;
            };
            let Ok(ptr) = ptr_cookie.reply() else {
                break;
            };

            let rx = ptr.root_x as i32;
            let ry = ptr.root_y as i32;
            let button1_down = ptr.mask.contains(KeyButMask::BUTTON1);

            let new_target = find_xdnd_aware(&conn, root, win, rx, ry, atoms.xdnd_aware);
            if new_target != current_target {
                if current_target != x11rb::NONE {
                    send_leave(&conn, win, current_target, &atoms);
                }
                current_target = new_target;
                target_accepted = false;
                if current_target != x11rb::NONE {
                    send_enter(&conn, win, current_target, &atoms);
                    send_position(&conn, win, current_target, &atoms, rx, ry);
                    last_pos = (rx, ry);
                }
                let _ = conn.flush();
            } else if current_target != x11rb::NONE && ((rx, ry) != last_pos || !target_accepted) {
                send_position(&conn, win, current_target, &atoms, rx, ry);
                last_pos = (rx, ry);
                let _ = conn.flush();
            }

            if !button1_down {
                if current_target != x11rb::NONE {
                    send_position(&conn, win, current_target, &atoms, rx, ry);
                    let _ = conn.flush();

                    for _ in 0..20 {
                        if target_accepted {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(5));
                        while let Ok(Some(event)) = conn.poll_for_event() {
                            match event {
                                Event::SelectionRequest(req) => {
                                    handle_selection_request(
                                        &conn, &atoms, &req, &urilist, &strlist,
                                    );
                                }
                                Event::ClientMessage(msg) if msg.type_ == atoms.xdnd_status => {
                                    let data = msg.data.as_data32();
                                    if data[0] == current_target {
                                        target_accepted = (data[1] & 1) != 0;
                                    }
                                }
                                _ => {}
                            }
                        }
                        send_position(&conn, win, current_target, &atoms, rx, ry);
                        let _ = conn.flush();
                    }

                    if target_accepted {
                        send_drop(&conn, win, current_target, &atoms);
                        let _ = conn.flush();
                        dropped = true;
                        continue;
                    } else {
                        send_leave(&conn, win, current_target, &atoms);
                        let _ = conn.flush();
                    }
                }
                break;
            }

            std::thread::sleep(Duration::from_millis(8));
        }

        let _ = conn.destroy_window(win);
        let _ = conn.flush();
        Some(())
    }

    fn find_xdnd_aware(
        conn: &RustConnection,
        root: Window,
        self_win: Window,
        x: i32,
        y: i32,
        xdnd_aware: Atom,
    ) -> Window {
        let mut cur = root;
        let mut aware_win = x11rb::NONE;

        while cur != x11rb::NONE && cur != self_win {
            if let Ok(cookie) = conn.get_property(false, cur, xdnd_aware, AtomEnum::ANY, 0, 1) {
                if let Ok(reply) = cookie.reply() {
                    if let Some(version) = reply.value32().and_then(|mut it| it.next()) {
                        if version >= 3 {
                            aware_win = cur;
                        }
                    } else if reply.value.first().copied().unwrap_or(0) >= 3 {
                        aware_win = cur;
                    }
                }
            }
            let Ok(trans_cookie) = conn.translate_coordinates(root, cur, x as i16, y as i16) else {
                break;
            };
            let Ok(trans) = trans_cookie.reply() else {
                break;
            };
            cur = trans.child;
        }
        aware_win
    }

    fn handle_selection_request(
        conn: &RustConnection,
        atoms: &Atoms,
        req: &SelectionRequestEvent,
        urilist: &str,
        strlist: &str,
    ) {
        let mut prop = if req.property != x11rb::NONE {
            req.property
        } else {
            req.target
        };

        if req.target == atoms.targets {
            let supported = [
                atoms.targets,
                atoms.mime_urilist,
                atoms.mime_string,
                atoms.mime_utf8,
                atoms.mime_plain,
            ];
            let _ = conn.change_property32(
                PropMode::REPLACE,
                req.requestor,
                prop,
                AtomEnum::ATOM,
                &supported,
            );
        } else if req.target == atoms.mime_urilist {
            let _ = conn.change_property8(
                PropMode::REPLACE,
                req.requestor,
                prop,
                req.target,
                urilist.as_bytes(),
            );
        } else if req.target == atoms.mime_string
            || req.target == atoms.mime_utf8
            || req.target == atoms.mime_plain
        {
            let _ = conn.change_property8(
                PropMode::REPLACE,
                req.requestor,
                prop,
                req.target,
                strlist.as_bytes(),
            );
        } else {
            prop = x11rb::NONE;
        }

        let notify = SelectionNotifyEvent {
            response_type: x11rb::protocol::xproto::SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: req.time,
            requestor: req.requestor,
            selection: req.selection,
            target: req.target,
            property: prop,
        };
        let _ = conn.send_event(false, req.requestor, EventMask::NO_EVENT, notify);
        let _ = conn.flush();
    }

    fn send_enter(conn: &RustConnection, src: Window, target: Window, atoms: &Atoms) {
        let ev = ClientMessageEvent::new(
            32,
            target,
            atoms.xdnd_enter,
            [
                src,
                XDND_VERSION << 24,
                atoms.mime_urilist,
                atoms.mime_string,
                0,
            ],
        );
        let _ = conn.send_event(false, target, EventMask::NO_EVENT, ev);
    }

    fn send_position(
        conn: &RustConnection,
        src: Window,
        target: Window,
        atoms: &Atoms,
        x: i32,
        y: i32,
    ) {
        let packed_xy = ((x as u32) << 16) | ((y as u32) & 0xFFFF);
        let ev = ClientMessageEvent::new(
            32,
            target,
            atoms.xdnd_position,
            [
                src,
                0,
                packed_xy,
                x11rb::CURRENT_TIME,
                atoms.xdnd_action_copy,
            ],
        );
        let _ = conn.send_event(false, target, EventMask::NO_EVENT, ev);
    }

    fn send_leave(conn: &RustConnection, src: Window, target: Window, atoms: &Atoms) {
        let ev = ClientMessageEvent::new(32, target, atoms.xdnd_leave, [src, 0, 0, 0, 0]);
        let _ = conn.send_event(false, target, EventMask::NO_EVENT, ev);
    }

    fn send_drop(conn: &RustConnection, src: Window, target: Window, atoms: &Atoms) {
        let ev = ClientMessageEvent::new(
            32,
            target,
            atoms.xdnd_drop,
            [src, 0, x11rb::CURRENT_TIME, 0, 0],
        );
        let _ = conn.send_event(false, target, EventMask::NO_EVENT, ev);
    }
}
