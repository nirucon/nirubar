//! NIRUBAR — the NIRU status bar.
//!
//! Modules are independent: workspaces, network, battery and clock. Which ones
//! appear, and in what order, comes from `[bar] modules` in the shared config.
//! Each can be clicked for a small panel with more detail.
//!
//! The bar runs as a separate process, so a bar crash never takes the window
//! manager with it.

mod config;
mod module;
mod popup;
mod render;
mod status;

use config::{Config, Result, Theme};
use module::{Module, Side, Workspaces};
use std::{collections::HashMap, thread, time::Duration};
use x11rb::{
    connection::Connection,
    protocol::{xproto::*, Event},
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};

/// One tick per second keeps `hh:mm` exact while only changed frames repaint.
const TICK: Duration = Duration::from_secs(1);

/// Horizontal space reserved per drawn module, in characters of padding.
const MODULE_PAD: i16 = 2;

/// One drawn module, with where it landed so a click can open its panel.
struct Placed {
    index: usize,
    x: i16,
    width: i16,
}

struct Bar {
    c: RustConnection,
    root: Window,
    win: Window,
    gc: Gcontext,
    width: u16,
    height: u16,
    theme: Theme,
    cfg: Config,
    atoms: HashMap<&'static str, Atom>,
    renderer: render::Renderer,
    last_frame: String,
    icons: status::Icons,
    modules: Vec<Box<dyn Module>>,
    workspaces: Workspaces,
    /// Hit boxes from the last frame: where each module and workspace cell is.
    placed: Vec<Placed>,
    cells: i16,
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
        let icons = cfg.bar.icons;
        let (c, s) = x11rb::connect(None)?;
        let screen = &c.setup().roots[s];
        let root = screen.root;
        let width = screen.width_in_pixels;

        let mut atoms = HashMap::new();
        for n in [
            "_NET_WM_WINDOW_TYPE",
            "_NET_WM_WINDOW_TYPE_DOCK",
            "_NET_WM_STRUT_PARTIAL",
            "_NET_WM_STRUT",
            "_NET_WM_NAME",
            "_NET_WM_PID",
            "UTF8_STRING",
            "_NET_CURRENT_DESKTOP",
            "_NET_CLIENT_LIST",
            "_NET_WM_DESKTOP",
            "_NET_ACTIVE_WINDOW",
            "_NIRUBAR_SELECTION",
            "_NIRUBAR_REDRAW",
            "_NIRU_CONFIG_SERIAL",
        ] {
            atoms.insert(n, c.intern_atom(false, n.as_bytes())?.reply()?.atom);
        }

        let selection = atoms["_NIRUBAR_SELECTION"];
        if c.get_selection_owner(selection)?.reply()?.owner != 0 {
            return Err("NIRUBAR is already running on this X screen".into());
        }

        let win = c.generate_id()?;
        let gc = c.generate_id()?;
        let mut renderer = render::Renderer::new(&c, root, &cfg, width)?;

        let background = Theme::pixel(&theme.background)?;
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
                .background_pixel(background)
                .event_mask(
                    EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::STRUCTURE_NOTIFY,
                ),
        )?;
        c.create_gc(
            gc,
            win,
            &CreateGCAux::new()
                .foreground(Theme::pixel(&theme.foreground)?)
                .background(background),
        )?;
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
            atoms["_NET_WM_PID"],
            AtomEnum::CARDINAL,
            &[std::process::id()],
        )?;
        c.change_property8(
            PropMode::REPLACE,
            win,
            atoms["_NET_WM_NAME"],
            atoms["UTF8_STRING"],
            b"NIRUBAR",
        )?;
        set_strut(&c, win, &atoms, cfg.bar_height, width)?;
        c.set_selection_owner(win, selection, x11rb::CURRENT_TIME)?;
        c.change_window_attributes(
            root,
            &ChangeWindowAttributesAux::new()
                .event_mask(EventMask::PROPERTY_CHANGE | EventMask::STRUCTURE_NOTIFY),
        )?;
        c.map_window(win)?;
        c.flush()?;

        renderer.resize(&c, root, width, cfg.bar_height)?;
        let icons = crate::choose_icons(&renderer, icons);
        let modules = module::build(&cfg.bar, icons);
        let workspaces = Workspaces::new(&cfg.bar);
        let cells = cfg.bar.cell_width as i16;
        Ok(Self {
            c,
            root,
            win,
            gc,
            width,
            height: cfg.bar_height,
            theme,
            cfg,
            atoms,
            renderer,
            last_frame: String::new(),
            icons,
            modules,
            workspaces,
            placed: Vec::new(),
            cells,
        })
    }

    /// Collect the EWMH state the bar displays.
    fn snapshot(&mut self) {
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
        self.workspaces.update(tag, occupied);
    }

    fn title(&self) -> String {
        if !self.cfg.bar.show_title {
            return String::new();
        }
        let Some(w) = self
            .values(self.root, "_NET_ACTIVE_WINDOW")
            .first()
            .copied()
        else {
            return String::new();
        };
        let name = self
            .c
            .get_property(
                false,
                w,
                self.atom("_NET_WM_NAME"),
                self.atom("UTF8_STRING"),
                0,
                256,
            )
            .ok()
            .and_then(|r| r.reply().ok())
            .map(|r| r.value)
            .unwrap_or_default();
        let name = if name.is_empty() {
            self.c
                .get_property(false, w, AtomEnum::WM_NAME, AtomEnum::ANY, 0, 256)
                .ok()
                .and_then(|r| r.reply().ok())
                .map(|r| r.value)
                .unwrap_or_default()
        } else {
            name
        };
        String::from_utf8_lossy(&name)
            .chars()
            .take(self.cfg.bar.title_max)
            .collect()
    }

    fn compose(&mut self) -> Result<()> {
        let bg = Theme::pixel(&self.theme.background)?;
        let fg = Theme::pixel(&self.theme.foreground)?;
        let muted = Theme::pixel(&self.theme.muted)?;
        let border = Theme::pixel(&self.theme.border)?;
        self.renderer.clear(&self.c, self.gc, bg)?;
        self.placed.clear();

        // Workspaces are cells, drawn first on the left.
        let cell = self.cells;
        let show_workspaces = self.cfg.bar.modules.iter().any(|m| m == "workspaces");
        if show_workspaces {
            for i in 0..9usize {
                let x = (i as i16) * cell;
                let active = i == self.workspaces.active as usize;
                if active {
                    self.renderer.fill(
                        &self.c,
                        self.gc,
                        Rectangle {
                            x,
                            y: 0,
                            width: cell as u16,
                            height: self.height,
                        },
                        border,
                    )?;
                }
                let (colour, cell_bg) = if active {
                    (fg, border)
                } else if self.workspaces.occupied[i] {
                    (fg, bg)
                } else {
                    (muted, bg)
                };
                let label = self.workspaces.names[i].clone();
                let text_x = x + (cell - self.renderer.width(&label)) / 2;
                self.renderer
                    .text(&self.c, self.gc, text_x, &label, colour, cell_bg, x + cell)?;
            }
        }

        // Right-aligned status modules, in config order.
        let right_edge = i16::try_from(self.width).unwrap_or(i16::MAX) - 10;
        let mut texts: Vec<(usize, String)> = Vec::new();
        for (index, m) in self.modules.iter_mut().enumerate() {
            if m.side() != Side::Right {
                continue;
            }
            let text = m.render();
            if !text.is_empty() {
                texts.push((index, text));
            }
        }
        let mut x = right_edge;
        let mut placed: Vec<Placed> = Vec::new();
        for (index, text) in texts.iter().rev() {
            let w = self.renderer.width(text);
            x -= w;
            self.renderer
                .text(&self.c, self.gc, x, text, fg, bg, right_edge)?;
            placed.push(Placed {
                index: *index,
                x,
                width: w,
            });
            x -= self.renderer.width(&self.cfg.bar.separator) + self.renderer.advance * MODULE_PAD;
        }
        placed.reverse();
        self.placed = placed;

        // The window title, clipped to the space that is left.
        let title = self.title();
        if !title.is_empty() {
            let start = self.cells * 9;
            let end = x - self.renderer.advance * 2;
            if end > start {
                self.renderer
                    .text(&self.c, self.gc, start, &title, muted, bg, end)?;
            }
        }

        self.c.flush()?;
        Ok(())
    }

    fn draw(&mut self) -> Result<()> {
        self.snapshot();
        let frame = self.frame_key();
        if frame == self.last_frame {
            return Ok(());
        }
        self.compose()?;
        self.renderer
            .present(&self.c, self.gc, self.win, self.width)?;
        self.c.flush()?;
        self.last_frame = frame;
        Ok(())
    }

    /// Everything the frame depends on. A changed key means a redraw.
    fn frame_key(&mut self) -> String {
        let tag = self.workspaces.active;
        let occupied: Vec<bool> = self.workspaces.occupied.to_vec();
        let modules: Vec<String> = self.modules.iter_mut().map(|m| m.render()).collect();
        let title = self.title();
        format!("{tag}:{occupied:?}:{modules:?}:{title}")
    }

    /// Open the detail panel for whatever was clicked.
    fn click(&mut self, x: i16, y: i16) -> Result<()> {
        let _ = y;
        if self.cfg.bar.modules.iter().any(|m| m == "workspaces") && x < self.cells * 9 {
            let tag = (x / self.cells).clamp(0, 8) as u32;
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
            self.last_frame.clear();
            return self.draw();
        }
        if !self.cfg.bar.popups {
            return Ok(());
        }
        let hit = self
            .placed
            .iter()
            .find(|p| x >= p.x && x < p.x + p.width)
            .map(|p| p.index);
        let Some(index) = hit else {
            return Ok(());
        };
        let left = self.placed.iter().find(|p| p.index == index).map(|p| p.x);
        let content = self.modules.get_mut(index).and_then(|m| m.popup());
        if let (Some((title, lines)), Some(left)) = (content, left) {
            popup::show(
                &self.c,
                self.root,
                &self.theme,
                self.height,
                self.cfg.font_size,
                self.cfg.font_path.as_deref(),
                left,
                &title,
                &lines,
            )?;
            self.last_frame.clear();
        }
        Ok(())
    }

    fn reload(&mut self) -> Result<()> {
        let cfg = Config::load()?;
        let theme = Theme::load(&cfg.theme)?;
        self.renderer = render::Renderer::new(&self.c, self.root, &cfg, self.width)?;
        self.c.configure_window(
            self.win,
            &ConfigureWindowAux::new().height(u32::from(cfg.bar_height)),
        )?;
        set_strut(&self.c, self.win, &self.atoms, cfg.bar_height, self.width)?;
        self.c.change_window_attributes(
            self.win,
            &ChangeWindowAttributesAux::new().background_pixel(Theme::pixel(&theme.background)?),
        )?;
        self.theme = theme;
        self.height = cfg.bar_height;
        self.icons = crate::choose_icons(&self.renderer, cfg.bar.icons);
        self.modules = module::build(&cfg.bar, self.icons);
        self.workspaces = Workspaces::new(&cfg.bar);
        self.cells = cfg.bar.cell_width as i16;
        self.cfg = cfg;
        self.last_frame.clear();
        self.draw()
    }

    fn run(&mut self) -> Result<()> {
        self.draw()?;
        thread::spawn({
            let (timer, _) = x11rb::connect(None)?;
            let win = self.win;
            let atom = self.atoms["_NIRUBAR_REDRAW"];
            move || loop {
                thread::sleep(TICK);
                if timer
                    .send_event(
                        false,
                        win,
                        EventMask::NO_EVENT,
                        ClientMessageEvent::new(32, win, atom, [0; 5]),
                    )
                    .is_err()
                    || timer.flush().is_err()
                {
                    break;
                }
            }
        });
        loop {
            let event = self.c.wait_for_event()?;
            match event {
                Event::ButtonPress(e) => {
                    if e.event_x >= 0 {
                        self.click(e.event_x, e.event_y)?;
                    }
                }
                Event::ConfigureNotify(e) if e.window == self.root && e.width != self.width => {
                    self.width = e.width;
                    self.renderer
                        .resize(&self.c, self.root, self.width, self.height)?;
                    self.c.configure_window(
                        self.win,
                        &ConfigureWindowAux::new().width(u32::from(self.width)),
                    )?;
                    set_strut(&self.c, self.win, &self.atoms, self.height, self.width)?;
                    self.last_frame.clear();
                    self.draw()?;
                }
                Event::Expose(_) => {
                    self.last_frame.clear();
                    self.draw()?;
                }
                Event::PropertyNotify(e) if e.atom == self.atom("_NIRU_CONFIG_SERIAL") => {
                    if let Err(e) = self.reload() {
                        eprintln!("Reload rejected: {e}");
                    }
                }
                Event::PropertyNotify(_) | Event::ClientMessage(_) => {
                    self.last_frame.clear();
                    self.draw()?;
                }
                Event::DestroyNotify(e) if e.window == self.win => break,
                Event::SelectionClear(_) => break,
                _ => (),
            }
        }
        Ok(())
    }
}

fn set_strut(
    c: &RustConnection,
    win: Window,
    atoms: &HashMap<&'static str, Atom>,
    height: u16,
    width: u16,
) -> Result<()> {
    c.change_property32(
        PropMode::REPLACE,
        win,
        atoms["_NET_WM_STRUT_PARTIAL"],
        AtomEnum::CARDINAL,
        &[
            0,
            0,
            u32::from(height),
            0,
            0,
            0,
            0,
            0,
            0,
            u32::from(width.saturating_sub(1)),
            0,
            0,
        ],
    )?;
    c.change_property32(
        PropMode::REPLACE,
        win,
        atoms["_NET_WM_STRUT"],
        AtomEnum::CARDINAL,
        &[0, 0, u32::from(height), 0],
    )?;
    Ok(())
}

/// Pick the icon set the font can actually draw, per glyph.
fn choose_icons(renderer: &render::Renderer, want_icons: bool) -> status::Icons {
    if !want_icons {
        return status::Icons::plain();
    }
    let nerd = status::Icons::nerd();
    let plain = status::Icons::plain();
    let ok = |g: &str| g.chars().all(|ch| renderer.has_glyph(ch));
    status::Icons {
        battery_full: if ok(nerd.battery_full) {
            nerd.battery_full
        } else {
            plain.battery_full
        },
        battery_high: if ok(nerd.battery_high) {
            nerd.battery_high
        } else {
            plain.battery_high
        },
        battery_medium: if ok(nerd.battery_medium) {
            nerd.battery_medium
        } else {
            plain.battery_medium
        },
        battery_low: if ok(nerd.battery_low) {
            nerd.battery_low
        } else {
            plain.battery_low
        },
        battery_empty: if ok(nerd.battery_empty) {
            nerd.battery_empty
        } else {
            plain.battery_empty
        },
        battery_charging: if ok(nerd.battery_charging) {
            nerd.battery_charging
        } else {
            plain.battery_charging
        },
        wifi: if ok(nerd.wifi) { nerd.wifi } else { plain.wifi },
        ethernet: if ok(nerd.ethernet) {
            nerd.ethernet
        } else {
            plain.ethernet
        },
        wifi_fallback: plain.wifi_fallback,
        ethernet_fallback: plain.ethernet_fallback,
    }
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

#[cfg(test)]
mod tests {
    #[test]
    fn clock_follows_the_configured_formats() {
        use crate::config::Bar;
        use crate::module::Module;
        let bar = Bar {
            date_format: "%d/%m".into(),
            time_format: "%H.%M".into(),
            ..Default::default()
        };
        let mut c = crate::module::Clock::new(&bar);
        let text = c.render();
        assert_eq!(
            text,
            format!(
                "{}  {}",
                chrono::Local::now().format("%d/%m"),
                chrono::Local::now().format("%H.%M")
            )
        );
    }
}
