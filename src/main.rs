use std::time::Duration;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{Atom, AtomEnum, ConnectionExt, MapState, PropMode, Window};
use x11rb::wrapper::ConnectionExt as _;

const POLL_MS: u64 = 300;
const MAX_CLIENTS: usize = 64;

fn is_client<C: Connection>(conn: &C, win: Window, pid: Atom) -> bool {
    let Ok(cookie) = conn.get_window_attributes(win) else {
        return false;
    };
    let Ok(attrs) = cookie.reply() else {
        return false;
    };
    if attrs.map_state != MapState::VIEWABLE || attrs.override_redirect {
        return false;
    }
    let Ok(cookie) = conn.get_property(false, win, pid, AtomEnum::NONE, 0, 1) else {
        return false;
    };
    cookie.reply().is_ok_and(|p| !p.value.is_empty())
}

fn scan<C: Connection>(conn: &C, win: Window, depth: u8, pid: Atom, out: &mut Vec<Window>) {
    if depth > 3 {
        return;
    }
    let Ok(cookie) = conn.query_tree(win) else {
        return;
    };
    let Ok(tree) = cookie.reply() else {
        return;
    };
    for child in tree.children {
        if is_client(conn, child, pid) {
            out.push(child);
        }
        if out.len() < MAX_CLIENTS {
            scan(conn, child, depth + 1, pid, out);
        }
    }
}

fn intern<C: Connection>(conn: &C, name: &str) -> Atom {
    conn.intern_atom(false, name.as_bytes())
        .expect("intern atom")
        .reply()
        .expect("intern atom")
        .atom
}

fn main() {
    let (conn, screen) = match x11rb::connect(None) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("x11-ewmh-shim: cannot open display: {e}");
            std::process::exit(1);
        }
    };

    let root = conn.setup().roots[screen].root;
    let pid = intern(&conn, "_NET_WM_PID");
    let workarea = intern(&conn, "_NET_WORKAREA");
    let client_lists = [
        intern(&conn, "_NET_CLIENT_LIST"),
        intern(&conn, "_NET_CLIENT_LIST_STACKING"),
    ];

    loop {
        if let Ok(cookie) = conn.get_geometry(root) {
            if let Ok(g) = cookie.reply() {
                let _ = conn.change_property32(
                    PropMode::REPLACE,
                    root,
                    workarea,
                    AtomEnum::CARDINAL,
                    &[0, 0, g.width as u32, g.height as u32],
                );
            }
        }
        let mut clients = Vec::new();
        scan(&conn, root, 0, pid, &mut clients);
        for list in client_lists {
            let _ =
                conn.change_property32(PropMode::REPLACE, root, list, AtomEnum::WINDOW, &clients);
        }
        let _ = conn.flush();
        std::thread::sleep(Duration::from_millis(POLL_MS));
    }
}
