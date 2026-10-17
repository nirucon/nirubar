mod config;
mod render;
use config::{Config, Result, Theme};
use std::{collections::HashMap, fs, thread, time::Duration};
use x11rb::{
    connection::Connection,
    protocol::{xproto::*, Event},
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};
struct Bar {
    c: RustConnection,
    root: Window,
    win: Window,
    gc: Gcontext,
    width: u16,
    height: u16,
    theme: Theme,
    atoms: HashMap<&'static str, Atom>,
    title_win: Option<Window>,
    renderer: render::Renderer,
}
impl Bar {
    fn atom(&self, n: &str) -> Atom {
        self.atoms[n]
    }
    fn values(&self, w: Window, n: &str) -> Vec<u32> {
        self.c
            .get_property(false, w, self.atom(n), AtomEnum::ANY, 0, 64)
            .ok()
            .and_then(|r| r.reply().ok())
            .and_then(|r| r.value32().map(|v| v.collect()))
            .unwrap_or_default()
    }
    fn new() -> Result<Self> {
        let cfg = Config::load()?;
        let theme = Theme::load(&cfg.theme)?;
        let (c, s) = x11rb::connect(None)?;
        let root = c.setup().roots[s].root;
        let width = c.setup().roots[s].width_in_pixels;
        let mut atoms = HashMap::new();
        for n in [
            "_NET_WM_WINDOW_TYPE",
            "_NET_WM_WINDOW_TYPE_DOCK",
            "_NET_WM_STRUT_PARTIAL",
            "_NET_WM_STRUT",
            "_NET_WM_NAME",
            "UTF8_STRING",
            "_NET_CURRENT_DESKTOP",
            "_NET_ACTIVE_WINDOW",
            "_NET_CLIENT_LIST",
            "_NET_WM_DESKTOP",
            "_NIRUBAR_TICK",
            "_NIRUBAR_SELECTION",
            "_NIRU_CONFIG_SERIAL",
        ] {
            atoms.insert(n, c.intern_atom(false, n.as_bytes())?.reply()?.atom);
        }
        let selection = atoms["_NIRUBAR_SELECTION"];
        if c.get_selection_owner(selection)?.reply()?.owner != 0 {
            return Err("nirubar already running on this X screen".into());
        }
        let win = c.generate_id()?;
        let gc = c.generate_id()?;
        let renderer = render::Renderer::new(&c, root, &cfg)?;

        c.create_window(
            0,
            win,
            root,
            0,
            0,
            width,
            cfg.bar_height,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new()
                .background_pixel(Theme::pixel(&theme.background)?)
                .event_mask(
                    EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::STRUCTURE_NOTIFY,
                ),
        )?;
        c.create_gc(
            gc,
            win,
            &CreateGCAux::new()
                .foreground(Theme::pixel(&theme.foreground)?)
                .background(Theme::pixel(&theme.background)?),
        )?;
        c.set_selection_owner(win, selection, x11rb::CURRENT_TIME)?;
        c.change_property32(
            PropMode::REPLACE,
            win,
            atoms["_NET_WM_WINDOW_TYPE"],
            AtomEnum::ATOM,
            &[atoms["_NET_WM_WINDOW_TYPE_DOCK"]],
        )?;
        c.change_property32(
            PropMode::REPLACE,
            win,
            atoms["_NET_WM_STRUT_PARTIAL"],
            AtomEnum::CARDINAL,
            &[
                0,
                0,
                u32::from(cfg.bar_height),
                0,
                0,
                0,
                0,
                0,
                0,
                u32::from(width) - 1,
                0,
                0,
            ],
        )?;
        c.change_property32(
            PropMode::REPLACE,
            win,
            atoms["_NET_WM_STRUT"],
            AtomEnum::CARDINAL,
            &[0, 0, u32::from(cfg.bar_height), 0],
        )?;
        c.change_property8(
            PropMode::REPLACE,
            win,
            atoms["_NET_WM_NAME"],
            atoms["UTF8_STRING"],
            b"NIRUBAR",
        )?;
        c.change_window_attributes(
            root,
            &ChangeWindowAttributesAux::new()
                .event_mask(EventMask::PROPERTY_CHANGE | EventMask::STRUCTURE_NOTIFY),
        )?;
        c.map_window(win)?;
        c.flush()?;
        let tick = atoms["_NIRUBAR_TICK"];
        thread::spawn(move || {
            let Ok((timer, _)) = x11rb::connect(None) else {
                return;
            };
            loop {
                thread::sleep(Duration::from_secs(5));
                if timer
                    .send_event(
                        false,
                        win,
                        EventMask::NO_EVENT,
                        ClientMessageEvent::new(32, win, tick, [0; 5]),
                    )
                    .is_err()
                    || timer.flush().is_err()
                {
                    break;
                }
            }
        });
        Ok(Self {
            c,
            root,
            win,
            gc,
            width,
            height: cfg.bar_height,
            theme,
            atoms,
            title_win: None,
            renderer,
        })
    }
    fn text(&self, x: i16, text: &str, color: &str, bg: &str) -> Result<()> {
        let available = self.width.saturating_sub(x.max(0) as u16);
        self.renderer.draw(
            &self.c,
            self.win,
            self.gc,
            x,
            self.height,
            available,
            text,
            color,
            bg,
        )
    }
    fn draw(&mut self) -> Result<()> {
        self.c.clear_area(false, self.win, 0, 0, 0, 0)?;
        let tag = self
            .values(self.root, "_NET_CURRENT_DESKTOP")
            .first()
            .copied()
            .unwrap_or(0);
        let mut occupied = [false; 9];
        for w in self.values(self.root, "_NET_CLIENT_LIST") {
            if let Some(t) = self.values(w, "_NET_WM_DESKTOP").first() {
                if *t < 9 {
                    occupied[*t as usize] = true;
                }
            }
        }
        for (i, is_occupied) in occupied.iter().enumerate() {
            let x = i as i16 * 30;
            if i == tag as usize {
                self.c.change_gc(
                    self.gc,
                    &ChangeGCAux::new().foreground(Theme::pixel(&self.theme.border)?),
                )?;
                self.c.poly_fill_rectangle(
                    self.win,
                    self.gc,
                    &[Rectangle {
                        x,
                        y: 0,
                        width: 28,
                        height: self.height,
                    }],
                )?;
            }
            self.text(
                x + 10,
                &(i + 1).to_string(),
                if i == tag as usize || *is_occupied {
                    &self.theme.foreground
                } else {
                    &self.theme.muted
                },
                if i == tag as usize {
                    &self.theme.border
                } else {
                    &self.theme.background
                },
            )?;
        }
        let active = self
            .values(self.root, "_NET_ACTIVE_WINDOW")
            .first()
            .copied();
        if active != self.title_win {
            if let Some(w) = self.title_win {
                let _ = self.c.change_window_attributes(
                    w,
                    &ChangeWindowAttributesAux::new().event_mask(EventMask::NO_EVENT),
                );
            }
            if let Some(w) = active {
                self.c.change_window_attributes(
                    w,
                    &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
                )?;
            }
            self.title_win = active;
        }
        let status = status();
        let status_width = self.renderer.width(&status) + 16;
        let sx = (self.width as i16 - status_width).max(280);
        if let Some(w) = active {
            let title = self
                .c
                .get_property(
                    false,
                    w,
                    self.atom("_NET_WM_NAME"),
                    self.atom("UTF8_STRING"),
                    0,
                    256,
                )?
                .reply()?
                .value;
            let title = if title.is_empty() {
                self.c
                    .get_property(false, w, AtomEnum::WM_NAME, AtomEnum::ANY, 0, 256)?
                    .reply()?
                    .value
            } else {
                title
            };
            let max = (sx - 296).max(0) as usize / self.renderer.advance.ceil() as usize;
            self.text(
                286,
                &String::from_utf8_lossy(&title)
                    .chars()
                    .take(max)
                    .collect::<String>(),
                &self.theme.muted,
                &self.theme.background,
            )?;
        }
        self.text(sx, &status, &self.theme.foreground, &self.theme.background)?;
        self.c.flush()?;
        Ok(())
    }
    fn reload(&mut self) -> Result<()> {
        let cfg = Config::load()?;
        let theme = Theme::load(&cfg.theme)?;
        let renderer = render::Renderer::new(&self.c, self.root, &cfg)?;
        self.c.configure_window(
            self.win,
            &ConfigureWindowAux::new().height(u32::from(cfg.bar_height)),
        )?;
        self.c.change_property32(
            PropMode::REPLACE,
            self.win,
            self.atom("_NET_WM_STRUT_PARTIAL"),
            AtomEnum::CARDINAL,
            &[
                0,
                0,
                u32::from(cfg.bar_height),
                0,
                0,
                0,
                0,
                0,
                0,
                u32::from(self.width) - 1,
                0,
                0,
            ],
        )?;
        self.c.change_property32(
            PropMode::REPLACE,
            self.win,
            self.atom("_NET_WM_STRUT"),
            AtomEnum::CARDINAL,
            &[0, 0, u32::from(cfg.bar_height), 0],
        )?;
        self.c.change_window_attributes(
            self.win,
            &ChangeWindowAttributesAux::new().background_pixel(Theme::pixel(&theme.background)?),
        )?;
        self.theme = theme;
        self.renderer = renderer;
        self.height = cfg.bar_height;
        self.draw()
    }
    fn run(&mut self) -> Result<()> {
        self.draw()?;
        loop {
            let event = self.c.wait_for_event()?;
            match event {
                Event::ButtonPress(e) if e.event_x >= 0 && e.event_x < 270 => {
                    let tag = e.event_x as u32 / 30;
                    self.c.send_event(
                        false,
                        self.root,
                        EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                        ClientMessageEvent::new(
                            32,
                            self.root,
                            self.atom("_NET_CURRENT_DESKTOP"),
                            [tag, x11rb::CURRENT_TIME, 0, 0, 0],
                        ),
                    )?;
                    self.c.flush()?;
                }
                Event::ConfigureNotify(e) if e.window == self.root => {
                    self.width = e.width;
                    self.c.configure_window(
                        self.win,
                        &ConfigureWindowAux::new().width(u32::from(self.width)),
                    )?;
                    self.draw()?;
                }
                Event::PropertyNotify(e) if e.atom == self.atom("_NIRU_CONFIG_SERIAL") => {
                    if let Err(e) = self.reload() {
                        eprintln!("Reload rejected: {e}");
                    }
                }
                Event::PropertyNotify(_) | Event::Expose(_) | Event::ClientMessage(_) => {
                    if let Err(e) = self.draw() {
                        eprintln!("bar redraw: {e}");
                    }
                }
                Event::DestroyNotify(e) if e.window == self.win => break,
                Event::SelectionClear(_) => break,
                _ => (),
            }
        }
        Ok(())
    }
}
fn status() -> String {
    let mem = fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let value = |key: &str| {
        mem.lines()
            .find(|l| l.starts_with(key))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|n| n.parse::<u64>().ok())
            .unwrap_or(0)
    };
    let used = value("MemTotal:").saturating_sub(value("MemAvailable:")) / 1024;
    let battery = fs::read_dir("/sys/class/power_supply")
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .find_map(|e| {
            let p = e.path();
            if fs::read_to_string(p.join("type")).ok()?.trim() != "Battery" {
                return None;
            }
            let level = fs::read_to_string(p.join("capacity")).ok()?;
            Some(format!(" | BAT {}%", level.trim()))
        })
        .unwrap_or_default();
    format!(
        "MEM {}M{} | {}",
        used,
        battery,
        chrono::Local::now().format("%Y-%m-%d %H:%M")
    )
}
fn main() {
    let arg = std::env::args().nth(1).unwrap_or_default();
    if arg == "--version" {
        println!("NIRUBAR {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if arg == "--check-config" {
        match Config::load().and_then(|c| Theme::load(&c.theme)) {
            Ok(t) => {
                println!("Configuration OK · {}", t.name);
                return;
            }
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1)
            }
        }
    }
    if !arg.is_empty() {
        eprintln!("Usage: nirubar [--version|--check-config]");
        std::process::exit(2)
    }
    if let Err(e) = Bar::new().and_then(|mut b| b.run()) {
        eprintln!("nirubar: {e}");
        std::process::exit(1)
    }
}
