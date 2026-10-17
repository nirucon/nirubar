use std::{
    process::{Child, Command},
    thread,
    time::{Duration, Instant},
};
use x11rb::protocol::xproto::*;
struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
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
