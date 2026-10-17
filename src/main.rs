//! NIRUBAR — the NIRU minimal status bar.
//!
//! Draws clickable NIRUWM workspaces, the Swedish date and the local time.
//! Everything else is deliberately absent: the bar is a status line, not a
//! dashboard. It runs as a separate process so a bar crash never takes the
//! window manager with it.
mod config;
mod render;

use chrono::Datelike;
use config::{Config, Result, Theme};
use std::{collections::HashMap, thread, time::Duration};
use x11rb::{
    connection::Connection,
    protocol::{xproto::*, Event},
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};

/// How often the clock is re-read. One tick per second keeps `hh:mm` exact
/// while the bar itself only repaints when the rendered string changes.
const TICK: Duration = Duration::from_secs(1);

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

        // A single bar per screen. The selection is per X display, so a nested
        // test server and the real session can coexist.
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
        // Identify the bar to the window manager and tools like `wmctrl`.
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

        // The queue is empty, so the first frame can be composed before the
        // window is visible; the renderer is sized for the initial width.
        renderer.resize(&c, root, width, cfg.bar_height)?;
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
        })
    }

    /// Compose the frame on the backing pixmap.
    fn compose(&mut self) -> Result<()> {
        let bg = Theme::pixel(&self.theme.background)?;
        let fg = Theme::pixel(&self.theme.foreground)?;
        let muted = Theme::pixel(&self.theme.muted)?;
        let border = Theme::pixel(&self.theme.border)?;
        self.renderer.clear(&self.c, self.gc, bg)?;

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

        let cell = self.cfg.bar.cell_width as i16;
        for (i, &is_occupied) in occupied.iter().enumerate() {
            let x = (i as i16) * cell;
            let active = i == tag as usize;
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
            let (colour, background) = if active {
                (fg, border)
            } else if is_occupied {
                (fg, bg)
            } else {
                (muted, bg)
            };
            let label = (i + 1).to_string();
            let text_x = x + (cell - self.renderer.advance) / 2;
            self.renderer.text(
                &self.c,
                self.gc,
                text_x,
                &label,
                colour,
                background,
                x + cell,
            )?;
        }

        // Clock, right-aligned. The Swedish date is ISO 8601 (SS-ISO 8601),
        // which is the Swedish standard: 2026-10-17, then hh:mm.
        let now = chrono::Local::now();
        let clock = format!(
            "{}  {}  {}",
            swedish_weekday(now.weekday()),
            now.format("%Y-%m-%d"),
            now.format("%H:%M")
        );
        let clock_width = self.renderer.width(&clock);
        let clock_x = i16::try_from(self.width)
            .unwrap_or(i16::MAX)
            .saturating_sub(clock_width)
            .saturating_sub(10)
            .max(cell * 9);
        self.renderer.text(
            &self.c,
            self.gc,
            clock_x,
            &clock,
            fg,
            bg,
            i16::try_from(self.width).unwrap_or(i16::MAX),
        )?;
        self.c.flush()?;
        Ok(())
    }

    fn draw(&mut self) -> Result<()> {
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

    /// Cheap description of what the visible frame depends on. A changed key
    /// means the bar must be redrawn.
    fn frame_key(&self) -> String {
        let tag = self
            .values(self.root, "_NET_CURRENT_DESKTOP")
            .first()
            .copied()
            .unwrap_or(0);
        let mut occupied = [0u8; 9];
        for w in self.values(self.root, "_NET_CLIENT_LIST") {
            if let Some(t) = self.values(w, "_NET_WM_DESKTOP").first() {
                if *t < 9 {
                    occupied[*t as usize] = 1;
                }
            }
        }
        let clock = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
        format!("{}:{:?}:{}", tag, occupied, clock)
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
        self.cfg = cfg;
        self.last_frame.clear();
        self.draw()
    }

    fn run(&mut self) -> Result<()> {
        // The first tick paints synchronously so the bar is never empty.
        self.draw()?;
        thread::spawn({
            // A watchdog thread keeps `hh:mm` correct without adding a timer
            // file descriptor to the X connection.
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
                Event::ButtonPress(e) if e.event_x >= 0 => {
                    let x = e.event_x;
                    let cell = self.cfg.bar.cell_width as i16;
                    if x < 9 * cell {
                        let tag = (x / cell) as u32;
                        self.c.send_event(
                            false,
                            self.root,
                            // No event mask: deliver the click to the WM
                            // exactly once, regardless of its selected masks.
                            EventMask::NO_EVENT,
                            ClientMessageEvent::new(
                                32,
                                self.root,
                                self.atom("_NET_CURRENT_DESKTOP"),
                                [tag, x11rb::CURRENT_TIME, 0, 0, 0],
                            ),
                        )?;
                        // Optimistically show the new tag; the WM confirms with
                        // its own property update.
                        self.last_frame.clear();
                    }
                    self.draw()?;
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
                    // An expose often follows a resize; the backing pixmap may
                    // be stale, so redraw unconditionally.
                    self.last_frame.clear();
                    self.draw()?;
                }
                // `niructl reload` publishes a new serial on the root window;
                // both the window manager and the bar pick it up.
                Event::PropertyNotify(e) if e.atom == self.atom("_NIRU_CONFIG_SERIAL") => {
                    if let Err(e) = self.reload() {
                        eprintln!("Reload rejected: {e}");
                    }
                }
                Event::PropertyNotify(_) | Event::ClientMessage(_) => self.draw()?,
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

/// Swedish weekday names, kept in code so the bar works without a locale
/// database (minimal containers and Void installs often have no `sv_SE`).
fn swedish_weekday(day: chrono::Weekday) -> &'static str {
    use chrono::Weekday::*;
    match day {
        Mon => "mån",
        Tue => "tis",
        Wed => "ons",
        Thu => "tor",
        Fri => "fre",
        Sat => "lör",
        Sun => "sön",
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
    use super::*;

    #[test]
    fn weekday_names_are_swedish() {
        use chrono::Weekday::*;
        for (day, name) in [
            (Mon, "mån"),
            (Tue, "tis"),
            (Wed, "ons"),
            (Thu, "tor"),
            (Fri, "fre"),
            (Sat, "lör"),
            (Sun, "sön"),
        ] {
            assert_eq!(swedish_weekday(day), name);
        }
    }

    #[test]
    fn clock_uses_iso_date_and_24h_time() {
        let now = chrono::Local::now();
        let text = format!(
            "{}  {}  {}",
            swedish_weekday(now.weekday()),
            now.format("%Y-%m-%d"),
            now.format("%H:%M")
        );
        assert_eq!(text.split("  ").nth(1).unwrap().len(), 10);
        assert_eq!(text.split("  ").nth(2).unwrap().len(), 5);
    }
}
