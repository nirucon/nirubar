use std::{
    process::{Child, Command},
    thread,
    time::{Duration, Instant},
};
use x11rb::{
    connection::Connection,
    protocol::{res::ConnectionExt as _, xproto::*},
    wrapper::ConnectionExt as _,
};
struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "requires isolated X server with X-Resource, font and xdotool"]
fn popup_and_reload_cycles_release_server_pixmaps() {
    use std::os::unix::fs::PermissionsExt;
    let directory = std::env::temp_dir().join(format!("nirubar-pixmaps-{}", std::process::id()));
    std::fs::create_dir_all(directory.join("niruwm")).unwrap();
    std::fs::create_dir_all(directory.join("bin")).unwrap();
    let actions = directory.join("actions");
    std::fs::write(&actions, "").unwrap();
    for (program, script) in [
        ("wpctl", "#!/bin/sh\ncase \"$1\" in get-volume) printf 'Volume: 0.45\\n';; *) printf 'audio-action\\n' >> \"$FIXTURE_ACTIONS\";; esac\n"),
        ("playerctl", "#!/bin/sh\ncase \"$*\" in status) printf 'Playing\\n';; 'metadata artist') printf 'Artist\\n';; 'metadata title') printf 'Track\\n';; 'metadata mpris:length') printf '100000000\\n';; position) printf '10\\n';; *) printf 'media-action\\n' >> \"$FIXTURE_ACTIONS\";; esac\n"),
    ] {
        let path = directory.join("bin").join(program);
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let config = directory.join("niruwm/config.toml");
    let contents = |theme: &str, module: &str| {
        format!("theme = {theme:?}\n[bar]\nmodules = [{module:?}]\nicons = false\ntray = false\npopup_timeout_seconds = 0\n")
    };
    std::fs::write(&config, contents("niru-noir", "volume")).unwrap();
    let (c, screen) = x11rb::connect(None).unwrap();
    let root = c.setup().roots[screen].root;
    let _bar = Process(
        Command::new(env!("CARGO_BIN_EXE_nirubar"))
            .env("XDG_CONFIG_HOME", &directory)
            .env("FIXTURE_ACTIONS", &actions)
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    directory.join("bin").display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .spawn()
            .unwrap(),
    );
    let selection = c
        .intern_atom(false, b"_NIRUBAR_SELECTION")
        .unwrap()
        .reply()
        .unwrap()
        .atom;
    let wait = |condition: &mut dyn FnMut() -> bool| {
        let until = Instant::now() + Duration::from_secs(5);
        while !condition() {
            assert!(Instant::now() < until, "bar condition timed out");
            thread::sleep(Duration::from_millis(20));
        }
    };
    let mut window = 0;
    wait(&mut || {
        window = c
            .get_selection_owner(selection)
            .unwrap()
            .reply()
            .unwrap()
            .owner;
        window != 0
    });
    thread::sleep(Duration::from_millis(100));
    let pixmap_bytes = || {
        let reply = c
            .res_query_client_pixmap_bytes(window)
            .unwrap()
            .reply()
            .unwrap();
        u64::from(reply.bytes) | (u64::from(reply.bytes_overflow) << 32)
    };
    let before = pixmap_bytes();
    assert!(before > 0, "X-Resource did not account for the bar buffer");
    let children = || c.query_tree(root).unwrap().reply().unwrap().children.len();
    let baseline_children = children();
    let width = c.get_geometry(window).unwrap().reply().unwrap().width;
    let pixel = || {
        c.get_image(ImageFormat::Z_PIXMAP, window, 0, 0, 1, 1, u32::MAX)
            .unwrap()
            .reply()
            .unwrap()
            .data
    };
    for cycle in 0..4 {
        c.send_event(
            false,
            window,
            EventMask::BUTTON_PRESS,
            ButtonPressEvent {
                response_type: BUTTON_PRESS_EVENT,
                detail: 1,
                sequence: 0,
                time: x11rb::CURRENT_TIME,
                root,
                event: window,
                child: 0,
                root_x: (width - 40) as i16,
                root_y: 10,
                event_x: (width - 40) as i16,
                event_y: 10,
                state: KeyButMask::default(),
                same_screen: true,
            },
        )
        .unwrap();
        c.flush().unwrap();
        wait(&mut || children() > baseline_children);
        assert!(Command::new("xdotool")
            .args(["key", "Escape"])
            .status()
            .unwrap()
            .success());
        wait(&mut || children() == baseline_children);
        assert_eq!(
            pixmap_bytes(),
            before,
            "closed popup leaked X-server pixel storage"
        );
        let old = pixel();
        std::fs::write(
            &config,
            contents(
                if cycle % 2 == 0 {
                    "niru-nord"
                } else {
                    "niru-noir"
                },
                if cycle % 2 == 0 { "media" } else { "volume" },
            ),
        )
        .unwrap();
        let serial = c
            .intern_atom(false, b"_NIRU_CONFIG_SERIAL")
            .unwrap()
            .reply()
            .unwrap()
            .atom;
        c.change_property32(
            PropMode::REPLACE,
            root,
            serial,
            AtomEnum::CARDINAL,
            &[cycle + 1],
        )
        .unwrap();
        c.flush().unwrap();
        wait(&mut || pixel() != old);
        assert_eq!(
            pixmap_bytes(),
            before,
            "reload leaked the previous bar buffer"
        );
    }
    assert_eq!(
        std::fs::read_to_string(actions).unwrap(),
        "",
        "opening information changed audio/playback"
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
#[ignore = "requires isolated X server, font and xdotool"]
fn popup_uses_monitor_coordinates_focus_and_restores_focus() {
    let directory = std::env::temp_dir().join(format!("nirubar-popup-{}", std::process::id()));
    std::fs::create_dir_all(directory.join("niruwm")).unwrap();
    std::fs::write(
        directory.join("niruwm/config.toml"),
        "[bar]\nmodules = [\"clock\"]\ntray = false\npopup_timeout_seconds = 0\n",
    )
    .unwrap();
    let (c, screen) = x11rb::connect(None).unwrap();
    let root = c.setup().roots[screen].root;
    let bar = Process(
        Command::new(env!("CARGO_BIN_EXE_nirubar"))
            .env("XDG_CONFIG_HOME", &directory)
            .args(["--monitor", "800,100,800"])
            .spawn()
            .unwrap(),
    );
    let selection = c
        .intern_atom(false, b"_NIRUBAR_SELECTION")
        .unwrap()
        .reply()
        .unwrap()
        .atom;
    let until = Instant::now() + Duration::from_secs(5);
    let mut win = 0;
    while win == 0 {
        win = c
            .get_selection_owner(selection)
            .unwrap()
            .reply()
            .unwrap()
            .owner;
        assert!(Instant::now() < until, "bar did not start");
        thread::sleep(Duration::from_millis(20));
    }
    thread::sleep(Duration::from_millis(100));
    let spy = c.generate_id().unwrap();
    c.create_window(
        0,
        spy,
        root,
        0,
        0,
        10,
        10,
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &CreateWindowAux::new(),
    )
    .unwrap();
    c.map_window(spy).unwrap();
    c.set_input_focus(InputFocus::PARENT, spy, x11rb::CURRENT_TIME)
        .unwrap();
    let baseline = c.query_tree(root).unwrap().reply().unwrap().children.len();
    c.send_event(
        false,
        win,
        EventMask::BUTTON_PRESS,
        ButtonPressEvent {
            response_type: BUTTON_PRESS_EVENT,
            detail: 1,
            sequence: 0,
            time: x11rb::CURRENT_TIME,
            root,
            event: win,
            child: 0,
            root_x: 1540,
            root_y: 110,
            event_x: 740,
            event_y: 10,
            state: KeyButMask::default(),
            same_screen: true,
        },
    )
    .unwrap();
    c.flush().unwrap();
    let mut focused = spy;
    let until = Instant::now() + Duration::from_secs(5);
    while focused == spy || focused == 0 || focused == root {
        focused = c.get_input_focus().unwrap().reply().unwrap().focus;
        assert!(Instant::now() < until, "popup never received focus");
        thread::sleep(Duration::from_millis(20));
    }
    let geometry = c.get_geometry(focused).unwrap().reply().unwrap();
    assert_eq!(geometry.width, 340);
    assert!(
        geometry.x >= 800 && geometry.x + 340 <= 1600,
        "popup escaped its monitor: {geometry:?}"
    );
    assert!(
        geometry.y >= 100,
        "popup used screen, not bar, coordinates: {geometry:?}"
    );
    assert!(Command::new("xdotool")
        .args(["key", "Escape"])
        .status()
        .unwrap()
        .success());
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        if c.query_tree(root).unwrap().reply().unwrap().children.len() == baseline {
            break;
        }
        assert!(Instant::now() < until, "popup did not close");
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        c.get_input_focus().unwrap().reply().unwrap().focus,
        spy,
        "popup did not restore the previous focus"
    );
    drop(bar);
    std::fs::remove_dir_all(directory).unwrap();
}
#[test]
#[ignore = "requires isolated X server and a TrueType font"]
fn dock_strut_render_and_singleton() {
    let dir = std::env::temp_dir().join(format!("nirubar-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let _bar = Process(
        Command::new(env!("CARGO_BIN_EXE_nirubar"))
            .env("XDG_CONFIG_HOME", &dir)
            .spawn()
            .unwrap(),
    );
    let (c, _s) = x11rb::connect(None).unwrap();
    let selection = c
        .intern_atom(false, b"_NIRUBAR_SELECTION")
        .unwrap()
        .reply()
        .unwrap()
        .atom;
    let end = Instant::now() + Duration::from_secs(5);
    let win = loop {
        let w = c
            .get_selection_owner(selection)
            .unwrap()
            .reply()
            .unwrap()
            .owner;
        if w != 0 {
            break w;
        }
        assert!(Instant::now() < end);
        thread::sleep(Duration::from_millis(20));
    };
    thread::sleep(Duration::from_millis(100));
    let a = c.get_window_attributes(win).unwrap().reply().unwrap();
    assert_eq!(a.map_state, MapState::VIEWABLE);
    let strut = c
        .intern_atom(false, b"_NET_WM_STRUT_PARTIAL")
        .unwrap()
        .reply()
        .unwrap()
        .atom;
    let p = c
        .get_property(false, win, strut, AtomEnum::CARDINAL, 0, 12)
        .unwrap()
        .reply()
        .unwrap();
    assert_eq!(p.value32().unwrap().nth(2), Some(28));
    let g = c.get_geometry(win).unwrap().reply().unwrap();
    let image = c
        .get_image(
            ImageFormat::Z_PIXMAP,
            win,
            0,
            0,
            g.width,
            g.height,
            u32::MAX,
        )
        .unwrap()
        .reply()
        .unwrap();
    if let Ok(path) = std::env::var("NIRU_CAPTURE_PATH") {
        std::fs::write(path, &image.data).unwrap();
    }
    assert!(image.data.chunks(4).any(|b| b != [0x10, 0x0f, 0x0f, 0]));
    let duplicate = Command::new(env!("CARGO_BIN_EXE_nirubar"))
        .env("XDG_CONFIG_HOME", &dir)
        .output()
        .unwrap();
    assert!(!duplicate.status.success());
    assert!(String::from_utf8_lossy(&duplicate.stderr).contains("already running"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
#[ignore = "requires isolated X server and a TrueType font"]
fn progress_only_changes_repaint_a_paused_short_title() {
    use std::os::unix::fs::PermissionsExt;
    let directory = std::env::temp_dir().join(format!("nirubar-progress-{}", std::process::id()));
    std::fs::create_dir_all(directory.join("bin")).unwrap();
    std::fs::create_dir_all(directory.join("config/niruwm")).unwrap();
    std::fs::write(
        directory.join("config/niruwm/config.toml"),
        "[bar]\nmodules = [\"media\"]\nicons = false\ntray = false\n",
    )
    .unwrap();
    std::fs::write(directory.join("position"), "10").unwrap();
    let helper = directory.join("bin/playerctl");
    std::fs::write(&helper, "#!/bin/sh\ncase \"$*\" in\nstatus) echo Paused;;\n'metadata artist') echo Artist;;\n'metadata title') echo Track;;\nposition) cat \"$FIXTURE_POSITION\";;\n'metadata mpris:length') echo 100000000;;\nesac\n").unwrap();
    std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let _bar = Process(
        Command::new(env!("CARGO_BIN_EXE_nirubar"))
            .env("XDG_CONFIG_HOME", directory.join("config"))
            .env("FIXTURE_POSITION", directory.join("position"))
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    directory.join("bin").display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .spawn()
            .unwrap(),
    );
    let (c, _) = x11rb::connect(None).unwrap();
    let selection = c
        .intern_atom(false, b"_NIRUBAR_SELECTION")
        .unwrap()
        .reply()
        .unwrap()
        .atom;
    let end = Instant::now() + Duration::from_secs(5);
    let win = loop {
        let win = c
            .get_selection_owner(selection)
            .unwrap()
            .reply()
            .unwrap()
            .owner;
        if win != 0 {
            break win;
        }
        assert!(Instant::now() < end);
        thread::sleep(Duration::from_millis(20));
    };
    thread::sleep(Duration::from_millis(200));
    let geometry = c.get_geometry(win).unwrap().reply().unwrap();
    let image = || {
        c.get_image(
            ImageFormat::Z_PIXMAP,
            win,
            0,
            0,
            geometry.width,
            geometry.height,
            u32::MAX,
        )
        .unwrap()
        .reply()
        .unwrap()
        .data
    };
    let before = image();
    std::fs::write(directory.join("position"), "80").unwrap();
    thread::sleep(Duration::from_millis(2200));
    assert_ne!(
        before,
        image(),
        "progress change was omitted from frame invalidation"
    );
    drop(_bar);
    std::fs::remove_dir_all(directory).unwrap();
}
