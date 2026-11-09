use std::{
    process::{Child, Command},
    thread,
    time::{Duration, Instant},
};
use x11rb::{
    connection::Connection,
    protocol::{res::ConnectionExt as _, xproto::*, Event},
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
        ("playerctl", "#!/bin/sh\ncase \"$*\" in status|*' status') printf 'Playing\\n';; *'metadata artist'*) printf 'Artist\\n';; *'metadata title'*) printf 'Track\\n';; *'metadata mpris:length'*) printf '100000000\\n';; *position*) printf '10\\n';; *play-pause*|*next*|*previous*|*play*|*pause*|*stop*) printf 'media-action\\n' >> \"$FIXTURE_ACTIONS\";; *) exit 0;; esac\n"),
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
        // A reloaded media module obtains its first snapshot asynchronously.
        // Retry the click until the module is rendered instead of assuming it
        // exists immediately after the theme's background pixel changes.
        let mut last_click = Instant::now() - Duration::from_secs(1);
        wait(&mut || {
            if children() > baseline_children {
                return true;
            }
            if last_click.elapsed() >= Duration::from_millis(200) {
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
                last_click = Instant::now();
            }
            false
        });
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

#[test]
#[ignore = "requires isolated X server, font and tray support"]
fn tray_embeds_icons_and_survives_selection_loss() {
    let directory = std::env::temp_dir().join(format!("nirubar-tray-{}", std::process::id()));
    std::fs::create_dir_all(directory.join("niruwm")).unwrap();
    std::fs::write(
        directory.join("niruwm/config.toml"),
        "[bar]\nmodules = [\"clock\"]\ntray = true\npopup_timeout_seconds = 0\n",
    )
    .unwrap();
    let (c, screen) = x11rb::connect(None).unwrap();
    let root = c.setup().roots[screen].root;
    let mut bar = Process(
        Command::new(env!("CARGO_BIN_EXE_nirubar"))
            .env("XDG_CONFIG_HOME", &directory)
            .spawn()
            .unwrap(),
    );
    let tray_selection = c
        .intern_atom(false, format!("_NET_SYSTEM_TRAY_S{screen}").as_bytes())
        .unwrap()
        .reply()
        .unwrap()
        .atom;
    let until = Instant::now() + Duration::from_secs(5);
    let mut tray = 0;
    while tray == 0 {
        tray = c
            .get_selection_owner(tray_selection)
            .unwrap()
            .reply()
            .unwrap()
            .owner;
        assert!(Instant::now() < until, "tray selection was not acquired");
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        c.get_window_attributes(tray)
            .unwrap()
            .reply()
            .unwrap()
            .map_state,
        MapState::VIEWABLE,
        "tray manager window must be mapped for XEmbed clients"
    );
    let tray_geometry = c.get_geometry(tray).unwrap().reply().unwrap();
    assert!(
        tray_geometry.width > 1 && tray_geometry.height > 1,
        "tray manager must be the bar window, not a clipped 1x1 selection window"
    );
    let icon = c.generate_id().unwrap();
    c.create_window(
        0,
        icon,
        root,
        0,
        0,
        16,
        16,
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &CreateWindowAux::new().background_pixel(0xff00ff),
    )
    .unwrap();
    c.map_window(icon).unwrap();
    let opcode = c
        .intern_atom(false, b"_NET_SYSTEM_TRAY_OPCODE")
        .unwrap()
        .reply()
        .unwrap()
        .atom;
    c.send_event(
        false,
        tray,
        EventMask::NO_EVENT,
        ClientMessageEvent::new(32, tray, opcode, [x11rb::CURRENT_TIME, 0, icon, 0, 0]),
    )
    .unwrap();
    c.flush().unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        if c.query_tree(icon).unwrap().reply().unwrap().parent == tray {
            break;
        }
        assert!(
            Instant::now() < until,
            "icon was not reparented into the tray"
        );
        thread::sleep(Duration::from_millis(20));
    }
    let info = c
        .intern_atom(false, b"_XEMBED_INFO")
        .unwrap()
        .reply()
        .unwrap()
        .atom;
    let values = c
        .get_property(false, icon, info, AtomEnum::ANY, 0, 2)
        .unwrap()
        .reply()
        .unwrap()
        .value32()
        .unwrap()
        .collect::<Vec<_>>();
    assert_eq!(values, vec![0, 1], "embedded icon needs _XEMBED_INFO");

    // Withdrawing by unmapping must free the slot instead of leaving a gap.
    let dock = |window: u32, pixel: u32| {
        c.create_window(
            0,
            window,
            root,
            0,
            0,
            16,
            16,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new().background_pixel(pixel),
        )
        .unwrap();
        c.map_window(window).unwrap();
        c.send_event(
            false,
            tray,
            EventMask::NO_EVENT,
            ClientMessageEvent::new(32, tray, opcode, [x11rb::CURRENT_TIME, 0, window, 0, 0]),
        )
        .unwrap();
        c.flush().unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            if c.query_tree(window).unwrap().reply().unwrap().parent == tray {
                break;
            }
            assert!(
                Instant::now() < until,
                "icon was not reparented into the tray"
            );
            thread::sleep(Duration::from_millis(20));
        }
    };
    let middle = c.generate_id().unwrap();
    let right = c.generate_id().unwrap();
    dock(middle, 0x00ff00);
    dock(right, 0x0000ff);
    thread::sleep(Duration::from_millis(300));
    let before = c.get_geometry(icon).unwrap().reply().unwrap().x;
    c.unmap_window(middle).unwrap();
    c.flush().unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let after = c.get_geometry(icon).unwrap().reply().unwrap().x;
        if after > before {
            break;
        }
        assert!(
            Instant::now() < until,
            "an unmapped icon kept its slot in the tray"
        );
        thread::sleep(Duration::from_millis(20));
    }

    // Another manager taking the tray selection must not kill a healthy bar.
    let rival = c.generate_id().unwrap();
    c.create_window(
        0,
        rival,
        root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_ONLY,
        0,
        &CreateWindowAux::new(),
    )
    .unwrap();
    c.set_selection_owner(rival, tray_selection, x11rb::CURRENT_TIME)
        .unwrap();
    c.flush().unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while c
        .get_selection_owner(tray_selection)
        .unwrap()
        .reply()
        .unwrap()
        .owner
        != rival
    {
        assert!(
            Instant::now() < until,
            "rival tray did not take the selection"
        );
        thread::sleep(Duration::from_millis(20));
    }
    thread::sleep(Duration::from_millis(100));
    assert!(
        bar.0.try_wait().unwrap().is_none(),
        "losing the tray selection terminated the bar"
    );
    drop(bar);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
#[ignore = "requires isolated X server, font and xdotool"]
fn one_bar_per_monitor_shares_a_screen_and_uses_global_desktops() {
    let directory = std::env::temp_dir().join(format!("nirubar-monitors-{}", std::process::id()));
    std::fs::create_dir_all(directory.join("niruwm")).unwrap();
    std::fs::write(
        directory.join("niruwm/config.toml"),
        "[bar]\nmodules = [\"workspaces\", \"clock\"]\ntray = false\npopup_timeout_seconds = 0\n",
    )
    .unwrap();
    let (c, screen) = x11rb::connect(None).unwrap();
    let root = c.setup().roots[screen].root;
    // Stand in for niruwm: publish the monitor contract and receive commands.
    c.change_window_attributes(
        root,
        &ChangeWindowAttributesAux::new()
            .event_mask(EventMask::SUBSTRUCTURE_REDIRECT | EventMask::STRUCTURE_NOTIFY),
    )
    .unwrap()
    .check()
    .unwrap();
    let monitors = c
        .intern_atom(false, b"_NIRUWM_MONITORS")
        .unwrap()
        .reply()
        .unwrap()
        .atom;
    let names = c
        .intern_atom(false, b"_NET_DESKTOP_NAMES")
        .unwrap()
        .reply()
        .unwrap()
        .atom;
    let utf8 = c
        .intern_atom(false, b"UTF8_STRING")
        .unwrap()
        .reply()
        .unwrap()
        .atom;
    let layout = "left\t0\t0\t640\t0\t3\t1\nright\t640\t0\t640\t3\t2\t0\n";
    c.change_property8(PropMode::REPLACE, root, monitors, utf8, layout.as_bytes())
        .unwrap();
    c.change_property8(PropMode::REPLACE, root, names, utf8, b"a\0b\0c\0d\0e\0")
        .unwrap();
    c.flush().unwrap();

    let start = |key: &str| {
        Process(
            Command::new(env!("CARGO_BIN_EXE_nirubar"))
                .env("XDG_CONFIG_HOME", &directory)
                .args([
                    "--monitor",
                    if key == "left" {
                        "0,0,640,left"
                    } else {
                        "640,0,640,right"
                    },
                ])
                .spawn()
                .unwrap(),
        )
    };
    let _left = start("left");
    let _right = start("right");
    let selection = |key: &str| {
        c.intern_atom(false, format!("_NIRUBAR_SELECTION_{key}").as_bytes())
            .unwrap()
            .reply()
            .unwrap()
            .atom
    };
    let left_sel = selection("left");
    let right_sel = selection("right");
    let until = Instant::now() + Duration::from_secs(5);
    let (mut left, mut right) = (0, 0);
    while left == 0 || right == 0 {
        left = c
            .get_selection_owner(left_sel)
            .unwrap()
            .reply()
            .unwrap()
            .owner;
        right = c
            .get_selection_owner(right_sel)
            .unwrap()
            .reply()
            .unwrap()
            .owner;
        assert!(Instant::now() < until,
            "both monitor bars did not start: left_owner={left} right_owner={right} left_sel={left_sel} right_sel={right_sel} left_alive={} right_alive={} logs={:?}/{:?}",
            std::path::Path::new(&format!("/proc/{}", _left.0.id())).exists(),
            std::path::Path::new(&format!("/proc/{}", _right.0.id())).exists(),
            std::fs::read_to_string(directory.join("left.log")).unwrap_or_default(),
            std::fs::read_to_string(directory.join("right.log")).unwrap_or_default());
        thread::sleep(Duration::from_millis(20));
    }
    assert_ne!(left, right);
    assert_eq!(c.get_geometry(left).unwrap().reply().unwrap().width, 640);
    assert_eq!(c.get_geometry(right).unwrap().reply().unwrap().width, 640);
    // A second bar for the same monitor is still refused.
    let duplicate = Command::new(env!("CARGO_BIN_EXE_nirubar"))
        .env("XDG_CONFIG_HOME", &directory)
        .args(["--monitor", "0,0,640,left"])
        .output()
        .unwrap();
    assert!(!duplicate.status.success());
    assert!(String::from_utf8_lossy(&duplicate.stderr).contains("already running on monitor"));

    // Click the second workspace cell of the right monitor: global desktop 4.
    c.send_event(
        false,
        right,
        EventMask::BUTTON_PRESS,
        ButtonPressEvent {
            response_type: BUTTON_PRESS_EVENT,
            detail: 1,
            sequence: 0,
            time: x11rb::CURRENT_TIME,
            root,
            event: right,
            child: 0,
            root_x: 675,
            root_y: 10,
            event_x: 35,
            event_y: 10,
            state: KeyButMask::default(),
            same_screen: true,
        },
    )
    .unwrap();
    c.flush().unwrap();
    let command = c
        .intern_atom(false, b"_NIRUWM_COMMAND")
        .unwrap()
        .reply()
        .unwrap()
        .atom;
    let until = Instant::now() + Duration::from_secs(2);
    let mut saw = false;
    while !saw {
        while let Some(event) = c.poll_for_event().unwrap() {
            if let x11rb::protocol::Event::ClientMessage(e) = event {
                if e.type_ == command && e.data.as_data32()[0] == 9 {
                    assert_eq!(e.data.as_data32()[1], 4, "click used a monitor-local tag");
                    saw = true;
                }
            }
        }
        assert!(
            Instant::now() < until,
            "monitor workspace click sent no global command"
        );
        thread::sleep(Duration::from_millis(10));
    }
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
#[ignore = "requires isolated X server, font and xdotool"]
fn bottom_bar_reserves_the_bottom_edge_and_opens_popups_above() {
    let directory = std::env::temp_dir().join(format!("nirubar-bottom-{}", std::process::id()));
    std::fs::create_dir_all(directory.join("niruwm")).unwrap();
    std::fs::write(
        directory.join("niruwm/config.toml"),
        "[bar]\nmodules = [\"clock\"]\ntray = false\npopup_timeout_seconds = 0\n",
    )
    .unwrap();
    let (c, screen) = x11rb::connect(None).unwrap();
    let root = c.setup().roots[screen].root;
    let width = c.setup().roots[screen].width_in_pixels;
    let height = c.setup().roots[screen].height_in_pixels;
    let width32 = u32::from(width);
    let height32 = u32::from(height);
    let monitor = format!("0,0,{width},{height},test");
    let bar = Process(
        Command::new(env!("CARGO_BIN_EXE_nirubar"))
            .env("XDG_CONFIG_HOME", &directory)
            .args(["--monitor", &monitor, "--position", "bottom"])
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
        assert!(Instant::now() < until, "bottom bar did not start");
        thread::sleep(Duration::from_millis(20));
    }
    thread::sleep(Duration::from_millis(100));
    let geometry = c.get_geometry(win).unwrap().reply().unwrap();
    assert_eq!(
        geometry.y as u32,
        height32 - u32::from(geometry.height),
        "bar was not placed on the bottom edge"
    );
    let strut = c
        .intern_atom(false, b"_NET_WM_STRUT_PARTIAL")
        .unwrap()
        .reply()
        .unwrap()
        .atom;
    let values: Vec<u32> = c
        .get_property(false, win, strut, AtomEnum::CARDINAL, 0, 12)
        .unwrap()
        .reply()
        .unwrap()
        .value32()
        .unwrap()
        .collect();
    assert_eq!(values.get(2), Some(&0), "bottom bar reserved the top");
    assert_eq!(
        values.get(3),
        Some(&28),
        "bottom bar did not reserve the bottom"
    );
    assert_eq!(
        values.get(10),
        Some(&0),
        "bottom strut starts at the wrong x"
    );
    assert_eq!(
        values.get(11),
        Some(&(width32.saturating_sub(1))),
        "bottom strut ends at the wrong x"
    );
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
            root_x: (width - 40) as i16,
            root_y: (height - 18) as i16,
            event_x: (width - 40) as i16,
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
        assert!(Instant::now() < until, "bottom popup never received focus");
        thread::sleep(Duration::from_millis(20));
    }
    let popup = c.get_geometry(focused).unwrap().reply().unwrap();
    assert!(
        popup.y + i16::try_from(popup.height).unwrap() <= geometry.y,
        "popup did not open above the bottom bar: {popup:?} bar={geometry:?}"
    );
    assert!(popup.y >= 0, "popup escaped the top of the screen");
    assert!(Command::new("xdotool")
        .args(["key", "Escape"])
        .status()
        .unwrap()
        .success());
    drop(bar);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
#[ignore = "requires isolated X server and a TrueType font"]
fn scratchpad_module_lists_slots_and_toggles_them_through_niruwm() {
    let directory = std::env::temp_dir().join(format!("nirubar-scratch-{}", std::process::id()));
    std::fs::create_dir_all(directory.join("niruwm")).unwrap();
    std::fs::write(
        directory.join("niruwm/config.toml"),
        "[bar]\nmodules = [\"scratchpad\"]\ntray = false\npopup_timeout_seconds = 0\n",
    )
    .unwrap();
    let (c, screen) = x11rb::connect(None).unwrap();
    let root = c.setup().roots[screen].root;
    let width = c.setup().roots[screen].width_in_pixels;
    let atom = |name: &str| {
        c.intern_atom(false, name.as_bytes())
            .unwrap()
            .reply()
            .unwrap()
            .atom
    };
    let state = atom("_NIRUWM_SCRATCHPADS");
    let utf8 = atom("UTF8_STRING");
    c.delete_property(root, state).unwrap();
    c.flush().unwrap();
    let bar = Process(
        Command::new(env!("CARGO_BIN_EXE_nirubar"))
            .env("XDG_CONFIG_HOME", &directory)
            .spawn()
            .unwrap(),
    );
    let selection = atom("_NIRUBAR_SELECTION");
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
    thread::sleep(Duration::from_millis(200));
    // The window the window manager would describe as scratchpad 3.
    let pad = c.generate_id().unwrap();
    c.create_window(
        0,
        pad,
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
    c.change_property8(PropMode::REPLACE, pad, atom("_NET_WM_NAME"), utf8, b"notes")
        .unwrap();
    // Become the "window manager" that receives the bar's command.
    c.change_window_attributes(
        root,
        &ChangeWindowAttributesAux::new().event_mask(EventMask::SUBSTRUCTURE_REDIRECT),
    )
    .unwrap()
    .check()
    .unwrap();
    c.change_property8(
        PropMode::REPLACE,
        root,
        state,
        utf8,
        format!("3\thidden\t{pad}\tkitty kitty\n").as_bytes(),
    )
    .unwrap();
    c.flush().unwrap();
    thread::sleep(Duration::from_millis(300));
    let click = |x: i16, y: i16, event: Window, event_x: i16, event_y: i16| {
        c.send_event(
            false,
            event,
            EventMask::BUTTON_PRESS,
            ButtonPressEvent {
                response_type: BUTTON_PRESS_EVENT,
                detail: 1,
                sequence: 0,
                time: x11rb::CURRENT_TIME,
                root,
                event,
                child: 0,
                root_x: x,
                root_y: y,
                event_x,
                event_y,
                state: KeyButMask::default(),
                same_screen: true,
            },
        )
        .unwrap();
        c.flush().unwrap();
    };
    let baseline = c.query_tree(root).unwrap().reply().unwrap().children.len();
    let x = width as i16 - 14;
    click(x, 10, win, x, 10);
    let until = Instant::now() + Duration::from_secs(5);
    let panel = loop {
        let children = c.query_tree(root).unwrap().reply().unwrap().children;
        if children.len() >= baseline + 2 {
            break *children.last().unwrap();
        }
        assert!(Instant::now() < until, "the scratchpad panel did not open");
        thread::sleep(Duration::from_millis(20));
    };
    let g = c.get_geometry(panel).unwrap().reply().unwrap();
    assert_eq!(g.width, 340, "{g:?}");
    // The first action row starts one title line below the top.
    let line = (15.0f32 * 1.5).ceil() as i16;
    let row_y = g.y + 10 + line + line / 2;
    click(g.x + 40, row_y, panel, 40, row_y - g.y);
    let until = Instant::now() + Duration::from_secs(5);
    let data = loop {
        if let Some(Event::ClientMessage(e)) = c.poll_for_event().unwrap() {
            if e.type_ == atom("_NIRUWM_COMMAND") {
                break e.data.as_data32();
            }
        }
        assert!(
            Instant::now() < until,
            "no command reached the window manager"
        );
        thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(&data[..3], &[15, 3, 0], "toggle scratchpad 3");
    drop(bar);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
#[ignore = "requires isolated X server and font"]
fn an_urgent_window_colours_only_its_own_screens_workspace_cell() {
    let directory = std::env::temp_dir().join(format!("nirubar-urgent-{}", std::process::id()));
    std::fs::create_dir_all(directory.join("niruwm")).unwrap();
    std::fs::write(
        directory.join("niruwm/config.toml"),
        "theme = \"niru-noir\"\n[bar]\nmodules = [\"workspaces\", \"clock\"]\ntray = false\npopup_timeout_seconds = 0\n",
    )
    .unwrap();
    let (c, screen) = x11rb::connect(None).unwrap();
    let root = c.setup().roots[screen].root;
    let atom = |name: &str| {
        c.intern_atom(false, name.as_bytes())
            .unwrap()
            .reply()
            .unwrap()
            .atom
    };
    let utf8 = atom("UTF8_STRING");
    // Stand in for niruwm: two screens of three and two workspaces.
    let layout = "left\t0\t0\t640\t0\t3\t0\nright\t640\t0\t640\t3\t2\t0\n";
    c.change_property8(
        PropMode::REPLACE,
        root,
        atom("_NIRUWM_MONITORS"),
        utf8,
        layout.as_bytes(),
    )
    .unwrap();
    c.change_property8(
        PropMode::REPLACE,
        root,
        atom("_NET_DESKTOP_NAMES"),
        utf8,
        b"a\0b\0c\0d\0e\0",
    )
    .unwrap();
    // One window on global desktop 4: the right screen's second workspace.
    let w = c.generate_id().unwrap();
    c.create_window(
        0,
        w,
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
    c.change_property32(
        PropMode::REPLACE,
        w,
        atom("_NET_WM_DESKTOP"),
        AtomEnum::CARDINAL,
        &[4],
    )
    .unwrap();
    c.change_property32(
        PropMode::REPLACE,
        root,
        atom("_NET_CLIENT_LIST"),
        AtomEnum::WINDOW,
        &[w],
    )
    .unwrap();
    c.flush().unwrap();
    let start = |geometry: &str| {
        Process(
            Command::new(env!("CARGO_BIN_EXE_nirubar"))
                .env("XDG_CONFIG_HOME", &directory)
                .args(["--monitor", geometry])
                .spawn()
                .unwrap(),
        )
    };
    let _left = start("0,0,640,left");
    let _right = start("640,0,640,right");
    // The colour at (x, 3) on the root window.
    let pixel = |x: i16| -> u32 {
        let image = c
            .get_image(ImageFormat::Z_PIXMAP, root, x, 3, 1, 1, !0)
            .unwrap()
            .reply()
            .unwrap();
        let d = &image.data;
        (u32::from(d[2]) << 16) | (u32::from(d[1]) << 8) | u32::from(d[0])
    };
    let background = 0x0f0f10;
    let wait_for = |what: &str, check: &dyn Fn() -> bool| {
        let until = Instant::now() + Duration::from_secs(5);
        while !check() {
            assert!(Instant::now() < until, "{what}");
            thread::sleep(Duration::from_millis(50));
        }
    };
    // Cells are about 30 px wide: the right screen's second cell is near
    // x = 640 + 45, the left screen's second cell near x = 45.
    wait_for("the right bar draws", &|| pixel(640 + 45) == background);
    c.change_property32(
        PropMode::REPLACE,
        root,
        atom("_NIRUWM_URGENT"),
        AtomEnum::WINDOW,
        &[w],
    )
    .unwrap();
    c.flush().unwrap();
    wait_for("the urgent cell takes the attention colour", &|| {
        pixel(640 + 45) == 0xd7b36a
    });
    assert_eq!(pixel(640 + 75), background, "only that cell");
    assert_eq!(pixel(45), background, "the other screen's bar is unmarked");
    c.change_property32(
        PropMode::REPLACE,
        root,
        atom("_NIRUWM_URGENT"),
        AtomEnum::WINDOW,
        &[],
    )
    .unwrap();
    c.flush().unwrap();
    wait_for("the mark goes away", &|| pixel(640 + 45) == background);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
#[ignore = "requires isolated X server and font"]
fn the_mode_module_shows_studio_mode_from_the_root_property() {
    let directory = std::env::temp_dir().join(format!("nirubar-studio-{}", std::process::id()));
    std::fs::create_dir_all(directory.join("niruwm")).unwrap();
    std::fs::write(
        directory.join("niruwm/config.toml"),
        "theme = \"niru-noir\"\n[bar]\nmodules = [\"mode\"]\ntray = false\npopup_timeout_seconds = 0\n",
    )
    .unwrap();
    let (c, screen) = x11rb::connect(None).unwrap();
    let root = c.setup().roots[screen].root;
    let width = c.setup().roots[screen].width_in_pixels as i16;
    let atom = |name: &str| {
        c.intern_atom(false, name.as_bytes())
            .unwrap()
            .reply()
            .unwrap()
            .atom
    };
    let studio = atom("_NIRUWM_STUDIO");
    let utf8 = atom("UTF8_STRING");
    c.change_property8(PropMode::REPLACE, root, studio, utf8, b"off")
        .unwrap();
    c.flush().unwrap();
    let _bar = Process(
        Command::new(env!("CARGO_BIN_EXE_nirubar"))
            .env("XDG_CONFIG_HOME", &directory)
            .env("XDG_STATE_HOME", directory.join("state"))
            .spawn()
            .unwrap(),
    );
    // Pixels in the right-hand 240 px of the bar that are not background.
    let ink = || -> usize {
        let Ok(reply) = c
            .get_image(ImageFormat::Z_PIXMAP, root, width - 240, 2, 236, 20, !0)
            .unwrap()
            .reply()
        else {
            return 0;
        };
        reply
            .data
            .chunks(4)
            .filter(|p| {
                (u32::from(p[2]) << 16 | u32::from(p[1]) << 8 | u32::from(p[0])) != 0x0f0f10
            })
            .count()
    };
    let wait_for = |what: &str, check: &dyn Fn() -> bool| {
        let until = Instant::now() + Duration::from_secs(5);
        while !check() {
            assert!(Instant::now() < until, "{what}");
            thread::sleep(Duration::from_millis(50));
        }
    };
    let background = || {
        let image = c
            .get_image(ImageFormat::Z_PIXMAP, root, width - 2, 3, 1, 1, !0)
            .unwrap()
            .reply()
            .unwrap();
        let d = &image.data;
        (u32::from(d[2]) << 16) | (u32::from(d[1]) << 8) | u32::from(d[0])
    };
    wait_for("the bar draws", &|| background() == 0x0f0f10);
    thread::sleep(Duration::from_millis(300));
    assert_eq!(
        ink(),
        0,
        "no work mode and no studio mode: the module is empty"
    );
    c.change_property8(PropMode::REPLACE, root, studio, utf8, b"on")
        .unwrap();
    c.flush().unwrap();
    wait_for("studio mode is shown", &|| ink() > 20);
    c.change_property8(PropMode::REPLACE, root, studio, utf8, b"off")
        .unwrap();
    c.flush().unwrap();
    wait_for("studio mode is gone again", &|| ink() == 0);
    c.delete_property(root, studio).unwrap();
    c.flush().unwrap();
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
#[ignore = "requires isolated X server, font and xdotool"]
fn the_clock_popup_marks_and_lists_events_from_a_local_ics_feed() {
    use std::io::{Read, Write};
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let directory = std::env::temp_dir().join(format!("nirubar-calendar-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(directory.join("niruwm")).unwrap();
    std::fs::create_dir_all(directory.join("nirubar")).unwrap();
    std::fs::write(
        directory.join("niruwm/config.toml"),
        "theme = \"niru-noir\"\n[bar]\nmodules = [\"clock\"]\ntray = false\npopup_timeout_seconds = 0\n",
    )
    .unwrap();
    // A one-shot HTTP server on loopback with an all-day event today.
    let today = chrono::Local::now().format("%Y%m%d").to_string();
    let feed = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:x11@test\r\nDTSTART;VALUE=DATE:{today}\r\nSUMMARY:X11 fixture event\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0u8; 2048];
        let n = stream.read(&mut request).unwrap();
        let head = String::from_utf8_lossy(&request[..n]).to_string();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/calendar\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{feed}",
            feed.len()
        )
        .unwrap();
        head
    });
    let secrets = directory.join("nirubar/calendars.toml");
    std::fs::write(
        &secrets,
        format!("[[calendar]]\nname = \"Test\"\nurl = \"http://127.0.0.1:{port}/private-x11secret/basic.ics\"\ncolor = \"#ff00ff\"\n"),
    )
    .unwrap();
    std::fs::set_permissions(&secrets, std::fs::Permissions::from_mode(0o600)).unwrap();
    let check = Command::new(env!("CARGO_BIN_EXE_nirubar"))
        .arg("--check-config")
        .env("XDG_CONFIG_HOME", &directory)
        .output()
        .unwrap();
    let report = String::from_utf8_lossy(&check.stdout);
    assert!(report.contains("Calendars: 1 (Test)"), "{report}");
    assert!(!report.contains("x11secret"), "check output names no URL");

    let (c, screen) = x11rb::connect(None).unwrap();
    let root = c.setup().roots[screen].root;
    let width = c.setup().roots[screen].width_in_pixels;
    let height = c.setup().roots[screen].height_in_pixels;
    let bar = Process(
        Command::new(env!("CARGO_BIN_EXE_nirubar"))
            .env("XDG_CONFIG_HOME", &directory)
            .env("XDG_CACHE_HOME", directory.join("cache"))
            .env("XDG_STATE_HOME", directory.join("state"))
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let head = server.join().unwrap();
    assert!(
        head.starts_with("GET /private-x11secret/basic.ics"),
        "{head}"
    );
    let cache = directory.join("cache/nirubar/calendar/0-test.json");
    let until = Instant::now() + Duration::from_secs(5);
    while !cache.exists() {
        assert!(Instant::now() < until, "the feed is cached");
        thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(std::fs::metadata(&cache).unwrap().mode() & 0o777, 0o600);
    assert!(!std::fs::read_to_string(&cache)
        .unwrap()
        .contains("x11secret"));
    let magenta = || -> usize {
        let Ok(reply) = c
            .get_image(ImageFormat::Z_PIXMAP, root, 0, 0, width, height, !0)
            .unwrap()
            .reply()
        else {
            return 0;
        };
        reply
            .data
            .chunks(4)
            .filter(|p| p[2] == 0xff && p[1] == 0 && p[0] == 0xff)
            .count()
    };
    let until = Instant::now() + Duration::from_secs(8);
    loop {
        // The popup opens on the clock's next click once the data is in.
        let x = (width - 20).to_string();
        assert!(Command::new("xdotool")
            .args(["mousemove", &x, "10", "click", "1"])
            .status()
            .unwrap()
            .success());
        thread::sleep(Duration::from_millis(400));
        if magenta() > 20 {
            break;
        }
        assert!(
            Instant::now() < until,
            "the popup shows the calendar colour"
        );
        // Close it again (or open it, when the click closed it).
        thread::sleep(Duration::from_millis(200));
    }
    assert!(Command::new("xdotool")
        .args(["key", "Escape"])
        .status()
        .unwrap()
        .success());
    drop(bar);
    std::fs::remove_dir_all(directory).unwrap();
}

/// Click the bar at `x` with the real pointer (the bar sits at the top).
fn click_at(x: u16) {
    let x = x.to_string();
    assert!(Command::new("xdotool")
        .args(["mousemove", &x, "10", "click", "1"])
        .status()
        .unwrap()
        .success());
}

fn bump_config_serial(c: &impl Connection, root: Window, serial: u32) {
    let atom = c
        .intern_atom(false, b"_NIRU_CONFIG_SERIAL")
        .unwrap()
        .reply()
        .unwrap()
        .atom;
    c.change_property32(PropMode::REPLACE, root, atom, AtomEnum::CARDINAL, &[serial])
        .unwrap();
    c.flush().unwrap();
}

#[test]
#[ignore = "requires isolated X server, font and xdotool"]
fn reading_panels_stay_open_past_the_global_timeout_and_quick_panels_close() {
    let directory = std::env::temp_dir().join(format!("nirubar-timeouts-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(directory.join("niruwm")).unwrap();
    std::fs::create_dir_all(directory.join("nirubar")).unwrap();
    let config = directory.join("niruwm/config.toml");
    let shared = |module: &str| {
        format!("theme = \"niru-noir\"\n[bar]\nmodules = [{module:?}]\nicons = false\ntray = false\npopup_timeout_seconds = 1\n")
    };
    std::fs::write(&config, shared("clock")).unwrap();
    let (c, screen) = x11rb::connect(None).unwrap();
    let root = c.setup().roots[screen].root;
    let width = c.setup().roots[screen].width_in_pixels;
    let selection = c
        .intern_atom(false, b"_NIRUBAR_SELECTION")
        .unwrap()
        .reply()
        .unwrap()
        .atom;
    let _bar = Process(
        Command::new(env!("CARGO_BIN_EXE_nirubar"))
            .env("XDG_CONFIG_HOME", &directory)
            .env("XDG_CACHE_HOME", directory.join("cache"))
            .env("XDG_STATE_HOME", directory.join("state"))
            .spawn()
            .unwrap(),
    );
    let until = Instant::now() + Duration::from_secs(5);
    while c
        .get_selection_owner(selection)
        .unwrap()
        .reply()
        .unwrap()
        .owner
        == 0
    {
        assert!(Instant::now() < until, "the bar starts");
        thread::sleep(Duration::from_millis(20));
    }
    thread::sleep(Duration::from_millis(300));
    let children = || c.query_tree(root).unwrap().reply().unwrap().children.len();
    let baseline = children();
    // Open the module's panel; retry while the bar is still painting.
    let open = || {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            click_at(width - 20);
            thread::sleep(Duration::from_millis(250));
            if children() > baseline {
                return Instant::now();
            }
            assert!(Instant::now() < until, "a click opens the panel");
        }
    };
    let closes_within = |limit: Duration| {
        let until = Instant::now() + limit;
        while Instant::now() < until {
            if children() == baseline {
                return true;
            }
            thread::sleep(Duration::from_millis(50));
        }
        false
    };

    // The calendar is a reading panel: the 1 s global timeout does not apply.
    open();
    assert!(
        !closes_within(Duration::from_millis(3000)),
        "the calendar closed on the global timeout"
    );
    assert!(Command::new("xdotool")
        .args(["key", "Escape"])
        .status()
        .unwrap()
        .success());
    assert!(
        closes_within(Duration::from_secs(2)),
        "Escape closes a reading panel"
    );

    // Uptime is a quick panel: it closes after about a second.
    std::fs::write(&config, shared("uptime")).unwrap();
    bump_config_serial(&c, root, 1);
    thread::sleep(Duration::from_millis(500));
    let opened = open();
    assert!(
        closes_within(Duration::from_secs(3)),
        "the quick panel kept the global timeout"
    );
    assert!(
        opened.elapsed() >= Duration::from_millis(700),
        "closed too early"
    );

    // An override in nirubar's own file beats the reading-panel default and
    // is picked up by a reload; NIRUWM's shared file stays untouched.
    std::fs::write(
        directory.join("nirubar/config.toml"),
        "[popup_timeout]\ncalendar = 1\nuptime = 0\n",
    )
    .unwrap();
    std::fs::write(&config, shared("clock")).unwrap();
    bump_config_serial(&c, root, 2);
    thread::sleep(Duration::from_millis(500));
    open();
    assert!(
        closes_within(Duration::from_secs(3)),
        "the calendar override closes the panel"
    );
    std::fs::write(&config, shared("uptime")).unwrap();
    bump_config_serial(&c, root, 3);
    thread::sleep(Duration::from_millis(500));
    open();
    assert!(
        !closes_within(Duration::from_millis(2500)),
        "uptime = 0 keeps the quick panel open"
    );
    // A click on the same module closes it.
    click_at(width - 20);
    assert!(closes_within(Duration::from_secs(2)));
    std::fs::remove_dir_all(directory).unwrap();
}

/// A loopback ICS server that answers every request and records its path.
fn ics_server() -> (u16, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = seen.clone();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut request = [0u8; 2048];
            let n = stream.read(&mut request).unwrap_or(0);
            let head = String::from_utf8_lossy(&request[..n]).to_string();
            let path = head.split(' ').nth(1).unwrap_or_default().to_string();
            let today = chrono::Local::now().format("%Y%m%d").to_string();
            let feed = format!(
                "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:{path}@test\r\nDTSTART;VALUE=DATE:{today}\r\nSUMMARY:Reload fixture\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
            );
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/calendar\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{feed}",
                feed.len()
            );
            log.lock().unwrap().push(path);
        }
    });
    (port, seen)
}

#[test]
#[ignore = "requires isolated X server"]
fn calendars_toml_is_reread_on_reload_and_when_the_file_changes() {
    use std::os::unix::fs::PermissionsExt;
    let directory = std::env::temp_dir().join(format!("nirubar-calreload-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(directory.join("niruwm")).unwrap();
    std::fs::create_dir_all(directory.join("nirubar")).unwrap();
    std::fs::write(
        directory.join("niruwm/config.toml"),
        "theme = \"niru-noir\"\n[bar]\nmodules = [\"clock\"]\ntray = false\n",
    )
    .unwrap();
    let (port, seen) = ics_server();
    let secrets = directory.join("nirubar/calendars.toml");
    let entry = |name: &str| {
        format!("[[calendar]]\nname = {name:?}\nurl = \"http://127.0.0.1:{port}/private-{name}/feed.ics\"\n")
    };
    // Write the way an editor does: a new file renamed over the old one.
    let save = |text: &str| {
        let tmp = directory.join("nirubar/calendars.toml.tmp");
        std::fs::write(&tmp, text).unwrap();
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::rename(&tmp, &secrets).unwrap();
    };
    let wait_for = |path: &str, limit: Duration| {
        let until = Instant::now() + limit;
        loop {
            if seen.lock().unwrap().iter().any(|p| p == path) {
                return;
            }
            assert!(
                Instant::now() < until,
                "{path} was not fetched: {:?}",
                seen.lock().unwrap()
            );
            thread::sleep(Duration::from_millis(50));
        }
    };
    let (c, screen) = x11rb::connect(None).unwrap();
    let root = c.setup().roots[screen].root;
    let start = |check: &str| {
        Process(
            Command::new(env!("CARGO_BIN_EXE_nirubar"))
                .env("XDG_CONFIG_HOME", &directory)
                .env("XDG_CACHE_HOME", directory.join("cache"))
                .env("XDG_STATE_HOME", directory.join("state"))
                .env("NIRUBAR_FILE_CHECK_SECONDS", check)
                .spawn()
                .unwrap(),
        )
    };

    // 1. `niructl reload` (the root serial) alone: the file watcher is off.
    let bar = start("0");
    thread::sleep(Duration::from_millis(1500));
    save(&entry("alpha"));
    thread::sleep(Duration::from_millis(1500));
    assert!(
        seen.lock().unwrap().is_empty(),
        "nothing fetched before reload"
    );
    bump_config_serial(&c, root, 11);
    wait_for("/private-alpha/feed.ics", Duration::from_secs(3));
    let alpha_cache = directory.join("cache/nirubar/calendar/0-alpha.json");
    let until = Instant::now() + Duration::from_secs(3);
    while !alpha_cache.exists() {
        assert!(Instant::now() < until, "the first feed is cached");
        thread::sleep(Duration::from_millis(50));
    }
    drop(bar);
    // A bar killed in the middle of a fetch leaves its lock behind; the next
    // bar must not wait two minutes for it.
    std::fs::write(
        directory.join("cache/nirubar/calendar/fetch.lock"),
        "2147483646",
    )
    .unwrap();

    // 2. No reload: the file change alone. The cached feed is not fetched
    //    again; the added one is, at once.
    let bar = start("1");
    thread::sleep(Duration::from_millis(1500));
    save(&format!("{}{}", entry("alpha"), entry("beta")));
    wait_for("/private-beta/feed.ics", Duration::from_secs(4));
    let cache = directory.join("cache/nirubar/calendar/1-beta.json");
    let until = Instant::now() + Duration::from_secs(3);
    while !cache.exists() {
        assert!(Instant::now() < until, "the new feed is cached");
        thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(
        seen.lock()
            .unwrap()
            .iter()
            .filter(|p| p.contains("alpha"))
            .count(),
        1,
        "a cached, unchanged feed is not fetched again"
    );
    // 3. A file anyone can read is refused again without a restart.
    std::fs::set_permissions(&secrets, std::fs::Permissions::from_mode(0o644)).unwrap();
    let check = Command::new(env!("CARGO_BIN_EXE_nirubar"))
        .arg("--check-config")
        .env("XDG_CONFIG_HOME", &directory)
        .output()
        .unwrap();
    let report = String::from_utf8_lossy(&check.stdout);
    assert!(report.contains("mode 0644"), "{report}");
    assert!(!report.contains("private-"), "{report}");
    drop(bar);
    std::fs::remove_dir_all(directory).unwrap();
}
