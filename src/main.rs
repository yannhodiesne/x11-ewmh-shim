use std::error::Error;
use std::time::Duration;
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ConnectionExt, EventMask, MapState, PropMode, Window,
};
use x11rb::wrapper::ConnectionExt as _;

const MAX_CLIENTS: usize = 64;
const RETRY_SECS: u64 = 1;
const MAX_DEPTH: u8 = 3;

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
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(cookie) = conn.query_tree(win) else {
        return;
    };
    let Ok(tree) = cookie.reply() else {
        return;
    };
    for child in tree.children {
        if out.len() >= MAX_CLIENTS {
            return;
        }
        if is_client(conn, child, pid) {
            out.push(child);
        }
        scan(conn, child, depth + 1, pid, out);
    }
}

fn watch<C: Connection>(conn: &C, win: Window) {
    let mask =
        EventMask::SUBSTRUCTURE_NOTIFY | EventMask::STRUCTURE_NOTIFY | EventMask::PROPERTY_CHANGE;
    let _ = conn
        .change_window_attributes(win, &ChangeWindowAttributesAux::new().event_mask(mask))
        .map(|cookie| cookie.ignore_error());
}

fn watch_tree<C: Connection>(conn: &C, win: Window, depth: u8) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(cookie) = conn.query_tree(win) else {
        return;
    };
    let Ok(tree) = cookie.reply() else {
        return;
    };
    for child in tree.children {
        watch(conn, child);
        watch_tree(conn, child, depth + 1);
    }
}

fn intern<C: Connection>(conn: &C, name: &str) -> Result<Atom, Box<dyn Error>> {
    Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom)
}

fn publish<C: Connection>(
    conn: &C,
    root: Window,
    pid: Atom,
    workarea: Atom,
    client_lists: [Atom; 2],
) -> Result<(), Box<dyn Error>> {
    let geometry = conn.get_geometry(root)?.reply()?;
    conn.change_property32(
        PropMode::REPLACE,
        root,
        workarea,
        AtomEnum::CARDINAL,
        &[0, 0, geometry.width as u32, geometry.height as u32],
    )?
    .ignore_error();

    let mut clients = Vec::new();
    scan(conn, root, 0, pid, &mut clients);
    for list in client_lists {
        conn.change_property32(PropMode::REPLACE, root, list, AtomEnum::WINDOW, &clients)?
            .ignore_error();
    }
    // The requests above are only queued; flush is what notices a dead server.
    conn.flush()?;
    Ok(())
}

fn republish_on(event: &Event, root: Window) -> bool {
    match event {
        Event::CreateNotify(_)
        | Event::DestroyNotify(_)
        | Event::MapNotify(_)
        | Event::UnmapNotify(_)
        | Event::ReparentNotify(_)
        | Event::ConfigureNotify(_) => true,
        // PropertyNotify reaches us only for windows where watch() selected PropertyChangeMask,
        // i.e. a client setting _NET_WM_PID after it mapped. The Xlib docs claim a root
        // SubstructureNotify also reports the root's own properties (Xorg does not); ignoring
        // those keeps the loop from publishing on its own writes.
        Event::PropertyNotify(event) => event.window != root,
        _ => false,
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let (conn, screen) = x11rb::connect(None)?;
    run_on(&conn, screen)
}

fn run_on<C: Connection>(conn: &C, screen: usize) -> Result<(), Box<dyn Error>> {
    let root = conn.setup().roots[screen].root;
    let pid = intern(&conn, "_NET_WM_PID")?;
    let workarea = intern(&conn, "_NET_WORKAREA")?;
    let client_lists = [
        intern(&conn, "_NET_CLIENT_LIST")?,
        intern(&conn, "_NET_CLIENT_LIST_STACKING")?,
    ];

    conn.change_window_attributes(
        root,
        &ChangeWindowAttributesAux::new()
            .event_mask(EventMask::SUBSTRUCTURE_NOTIFY | EventMask::STRUCTURE_NOTIFY),
    )?
    .ignore_error();
    watch_tree(&conn, root, 0);

    publish(&conn, root, pid, workarea, client_lists)?;

    loop {
        let event = conn.wait_for_event()?;
        if let Event::CreateNotify(event) = &event {
            watch(conn, event.window);
        }
        if republish_on(&event, root) {
            publish(conn, root, pid, workarea, client_lists)?;
        }
    }
}

fn main() {
    loop {
        if let Err(e) = run() {
            eprintln!("x11-ewmh-shim: {e}");
        }
        // niri can restart xwayland-satellite; reconnect instead of exiting so `nix run` works too.
        std::thread::sleep(Duration::from_secs(RETRY_SECS));
    }
}

#[cfg(test)]
mod tests;
