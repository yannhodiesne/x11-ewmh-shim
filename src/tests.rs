use super::{MAX_CLIENTS, MAX_DEPTH, intern, publish, republish_on, run_on, scan};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ConnectionExt, CreateWindowAux, ExposeEvent, MapNotifyEvent, PropMode,
    Property, PropertyNotifyEvent, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

static DISPLAY_SEQ: AtomicU32 = AtomicU32::new(0);

/// A throwaway Xvfb plus a connection to it, so every test starts from an empty root window.
struct Server {
    conn: RustConnection,
    display: String,
    root: Window,
    pid: Atom,
    workarea: Atom,
    lists: [Atom; 2],
    num: u32,
    log: String,
    child: Child,
}

impl Server {
    fn new() -> Self {
        // Skip display numbers something else already holds, otherwise Xvfb refuses to start.
        let num = loop {
            let n = 90 + std::process::id() % 900 + DISPLAY_SEQ.fetch_add(1, Ordering::Relaxed);
            let taken = [format!("/tmp/.X11-unix/X{n}"), format!("/tmp/.X{n}-lock")];
            if taken.iter().all(|p| !std::path::Path::new(p).exists()) {
                break n;
            }
        };
        let display = format!(":{num}");
        let socket = format!("/tmp/.X11-unix/X{num}");
        // Xvfb only makes this directory as root, and two of them racing on mkdir fails one.
        let _ = std::fs::create_dir_all("/tmp/.X11-unix");
        let log = format!("/tmp/xvfb-{num}.log");
        let child = Command::new("Xvfb")
            .args([
                display.as_str(),
                "-screen",
                "0",
                "640x480x24",
                "-nolisten",
                "tcp",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::from(
                std::fs::File::create(&log).expect("cannot write the Xvfb log"),
            ))
            .spawn()
            .expect("these tests need Xvfb on PATH (nix develop, or nix-shell -p xorg.xvfb)");
        for _ in 0..100 {
            if std::path::Path::new(&socket).exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        if !std::path::Path::new(&socket).exists() {
            panic!(
                "Xvfb never created {socket}:\n{}",
                std::fs::read_to_string(&log).unwrap_or_default()
            );
        }
        // The socket file shows up before Xvfb starts accepting, so retry the handshake.
        let mut connection = None;
        for _ in 0..40 {
            if let Ok(pair) = RustConnection::connect(Some(display.as_str())) {
                connection = Some(pair);
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let (conn, screen) = connection.expect("Xvfb created its socket but never accepted");
        let root = conn.setup().roots[screen].root;
        let pid = intern(&conn, "_NET_WM_PID").unwrap();
        let workarea = intern(&conn, "_NET_WORKAREA").unwrap();
        let lists = [
            intern(&conn, "_NET_CLIENT_LIST").unwrap(),
            intern(&conn, "_NET_CLIENT_LIST_STACKING").unwrap(),
        ];
        Self {
            conn,
            display,
            root,
            pid,
            workarea,
            lists,
            num,
            log,
            child,
        }
    }

    /// A second connection, the way the shim would have its own.
    fn connect(&self) -> (RustConnection, usize) {
        RustConnection::connect(Some(self.display.as_str())).unwrap()
    }

    /// Run the shim's loop against this server, detached, on its own connection.
    fn spawn_loop(&self) {
        let (conn, screen) = self.connect();
        std::thread::spawn(move || {
            let _ = run_on(&conn, screen);
        });
    }

    /// Make a window the way a game client would: pid property, then map.
    fn client(&self, parent: Window, with_pid: bool, mapped: bool, redirect: bool) -> Window {
        let win = self.conn.generate_id().unwrap();
        self.conn
            .create_window(
                0,
                win,
                parent,
                0,
                0,
                10,
                10,
                0,
                WindowClass::INPUT_OUTPUT,
                x11rb::NONE,
                &CreateWindowAux::new().override_redirect(u32::from(redirect)),
            )
            .unwrap();
        if with_pid {
            self.set_pid(win);
        }
        if mapped {
            self.conn.map_window(win).unwrap();
        }
        self.conn.sync().unwrap();
        win
    }

    fn set_pid(&self, win: Window) {
        self.conn
            .change_property32(
                PropMode::REPLACE,
                win,
                self.pid,
                AtomEnum::CARDINAL,
                &[42u32],
            )
            .unwrap()
            .ignore_error();
    }

    fn root_prop(&self, atom: Atom, kind: AtomEnum) -> Vec<u32> {
        let reply = self
            .conn
            .get_property(false, self.root, atom, kind, 0, 256)
            .unwrap()
            .reply()
            .unwrap();
        reply
            .value
            .as_chunks::<4>()
            .0
            .iter()
            .map(|bytes| u32::from_ne_bytes(*bytes))
            .collect()
    }

    fn wait_in_list(&self, atom: Atom, want: Window) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if self.root_prop(atom, AtomEnum::WINDOW).contains(&want) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("{want:#x} never appeared in the client list");
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(format!("/tmp/.X11-unix/X{}", self.num));
        let _ = std::fs::remove_file(format!("/tmp/.X{}-lock", self.num));
        let _ = std::fs::remove_file(&self.log);
    }
}

#[test]
fn only_mapped_windows_with_a_pid_are_clients() {
    let server = Server::new();
    let client = server.client(server.root, true, true, false);
    server.client(server.root, false, true, false); // no _NET_WM_PID
    server.client(server.root, true, false, false); // never mapped
    server.client(server.root, true, true, true); // override_redirect popup

    let mut found = Vec::new();
    scan(&server.conn, server.root, 0, server.pid, &mut found);
    assert_eq!(found, [client]);
}

#[test]
fn nested_clients_are_found_up_to_max_depth() {
    let server = Server::new();
    let mut deepest = server.root;
    for _ in 0..=MAX_DEPTH {
        deepest = server.client(deepest, true, true, false);
    }
    let too_deep = server.client(deepest, true, true, false);

    let mut found = Vec::new();
    scan(&server.conn, server.root, 0, server.pid, &mut found);
    assert!(
        found.contains(&deepest),
        "depth {} must be scanned",
        MAX_DEPTH + 1
    );
    assert!(
        !found.contains(&too_deep),
        "below MAX_DEPTH must be ignored"
    );
}

#[test]
fn the_client_list_is_capped_at_max_clients() {
    let server = Server::new();
    for _ in 0..MAX_CLIENTS + 5 {
        server.client(server.root, true, true, false);
    }

    let mut found = Vec::new();
    scan(&server.conn, server.root, 0, server.pid, &mut found);
    assert_eq!(found.len(), MAX_CLIENTS);
}

#[test]
fn publish_writes_the_workarea_and_both_client_lists() {
    let server = Server::new();
    let client = server.client(server.root, true, true, false);

    publish(
        &server.conn,
        server.root,
        server.pid,
        server.workarea,
        server.lists,
    )
    .unwrap();

    assert_eq!(
        server.root_prop(server.workarea, AtomEnum::CARDINAL),
        [0, 0, 640, 480]
    );
    for list in server.lists {
        assert_eq!(server.root_prop(list, AtomEnum::WINDOW), [client]);
    }
}

#[test]
fn only_events_that_change_the_client_list_trigger_a_publish() {
    let root = 0x100;
    let property_change = |window: Window| {
        Event::PropertyNotify(PropertyNotifyEvent {
            response_type: 0,
            sequence: 0,
            window,
            atom: 0,
            time: 0,
            state: Property::NEW_VALUE,
        })
    };

    // Our own root writes come back as PropertyNotify on the root; ignoring them is what
    // stops the event loop from spinning on itself.
    assert!(!republish_on(&property_change(root), root));
    // A client that sets _NET_WM_PID after mapping has to be picked up.
    assert!(republish_on(&property_change(root + 1), root));
    assert!(republish_on(
        &Event::MapNotify(MapNotifyEvent {
            response_type: 0,
            sequence: 0,
            event: 2,
            window: 2,
            override_redirect: false,
        }),
        root
    ));
    assert!(!republish_on(
        &Event::Expose(ExposeEvent {
            response_type: 0,
            sequence: 0,
            window: 2,
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            count: 0,
        }),
        root
    ));
}

#[test]
fn the_loop_reacts_to_events_and_reports_a_dead_server() {
    let mut server = Server::new();
    let (conn, screen) = server.connect();
    let looped = std::thread::spawn(move || run_on(&conn, screen).is_err());

    let first = server.client(server.root, true, true, false);
    server.wait_in_list(server.lists[0], first);

    // Nothing is polled: a deleted property stays deleted until an event wakes the loop up.
    server
        .conn
        .delete_property(server.root, server.workarea)
        .unwrap();
    server.conn.sync().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        server
            .root_prop(server.workarea, AtomEnum::CARDINAL)
            .is_empty()
    );

    let second = server.client(server.root, true, true, false);
    server.wait_in_list(server.lists[1], second);

    server.child.kill().unwrap();
    assert!(looped.join().unwrap(), "a dead server must end the loop");
}

#[test]
fn a_client_that_sets_its_pid_after_mapping_is_still_listed() {
    let server = Server::new();
    server.spawn_loop();

    let decoy = server.client(server.root, true, true, false);
    server.wait_in_list(server.lists[0], decoy); // the loop is up before the window appears

    let win = server.client(server.root, false, true, false); // mapped, but no _NET_WM_PID
    std::thread::sleep(Duration::from_millis(200)); // let the loop select input on the window
    server.set_pid(win);

    server.wait_in_list(server.lists[0], win);
}

#[test]
fn a_window_nested_under_a_client_is_listed_too() {
    let server = Server::new();
    server.spawn_loop();

    let decoy = server.client(server.root, true, true, false);
    server.wait_in_list(server.lists[0], decoy); // the loop is up and watching new windows

    let top = server.client(server.root, false, true, false);
    std::thread::sleep(Duration::from_millis(200)); // let it select input on the new window
    let nested = server.client(top, true, true, false); // pid on the nested window

    server.wait_in_list(server.lists[0], nested);
}
