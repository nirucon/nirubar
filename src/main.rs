//! NIRUBAR — the NIRU status bar.
//!
//! Modules are independent: workspaces, network, battery and clock. Which ones
//! appear, and in what order, comes from `[bar] modules` in the shared config.
//! Each can be clicked for a small panel with more detail.
//!
//! The bar runs as a separate process, so a bar crash never takes the window
//! manager with it.

mod ambient;
mod av;
mod bluetooth;
mod config;
mod helpers;
mod layout;
mod microphone;
mod module;
mod nirux_update;
mod popup;
mod powerprofile;
mod probe;
mod pulse;
mod recording;
mod render;
mod scroll;
mod sni;
mod status;
mod tray;
mod updates;
mod weather;

use config::{Config, Result, Theme};
use libc::{poll, pollfd as PollFd, read as libc_read, write as libc_write};
use std::sync::{Arc, Mutex};

/// Which monitor this bar belongs to, read with `--monitor`.
#[derive(Clone, Debug)]
struct Monitor {
    x: i16,
    y: i16,
    w: u16,
    name: Option<String>,
}

impl Monitor {
    fn key(&self) -> String {
        match &self.name {
            Some(name) if !name.is_empty() => sanitize(name),
            _ => format!("{}_{}_{}", self.x, self.y, self.w),
        }
    }

    fn matches(&self, line: &str) -> bool {
        let mut fields = line.split('\t');
        let Some(name) = fields.next() else {
            return false;
        };
        let (Some(x), Some(y), Some(w)) = (fields.next(), fields.next(), fields.next()) else {
            return false;
        };
        self.name.as_deref().map(|n| n == name).unwrap_or(false)
            || (self.name.is_none()
                && x.parse::<i16>() == Ok(self.x)
                && y.parse::<i16>() == Ok(self.y)
                && w.parse::<u16>() == Ok(self.w))
    }
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// State published by niruwm for one output in `_NIRUWM_MONITORS`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct MonitorLayout {
    offset: u32,
    count: usize,
    active: u32,
    /// The layout of the workspace this screen shows.
    layout: Option<String>,
}

fn parse_monitor_layout(text: &str, monitor: &Option<Monitor>) -> Option<MonitorLayout> {
    let monitor = monitor.as_ref()?;
    for line in text.lines() {
        if !monitor.matches(line) {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() < 7 {
            continue;
        }
        // An older window manager publishes seven fields and no layout; the
        // bar then keeps the global one.
        let layout = fields
            .get(7)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        return Some(MonitorLayout {
            offset: fields[4].parse().ok()?,
            count: fields[5].parse::<usize>().ok()?.max(1),
            active: fields[6].parse().ok()?,
            layout,
        });
    }
    None
}

/// A monitor bar must keep the width assigned by the WM even when the global
/// root changes size. A standalone bar follows the root.
fn follows_root_width(monitor: &Option<Monitor>, current: u16, next: u16) -> bool {
    monitor.is_none() && current != next
}

/// Read `--monitor X,Y,W` from the command line.
///
/// The window manager passes the geometry rather than a name, so the bar does
/// not need its own RandR query and cannot disagree with the WM about where a
/// screen is.
fn parse_monitor(args: &[String]) -> Result<Option<Monitor>> {
    let Some(pos) = args.iter().position(|a| a == "--monitor") else {
        return Ok(None);
    };
    let value = args.get(pos + 1).ok_or("--monitor needs X,Y,W")?;
    let parts: Vec<&str> = value.split(',').collect();
    if parts.len() != 3 && parts.len() != 4 {
        return Err("--monitor needs X,Y,W or X,Y,W,NAME".into());
    }
    Ok(Some(Monitor {
        x: parts[0].parse()?,
        y: parts[1].parse()?,
        w: parts[2].parse()?,
        name: parts
            .get(3)
            .filter(|n| !n.is_empty())
            .map(|n| n.to_string()),
    }))
}
use module::{Module, Side, Workspaces};
use std::{collections::HashMap, os::fd::AsRawFd as _};
use x11rb::{
    connection::Connection,
    protocol::{xproto::*, Event},
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};

/// Horizontal space reserved per drawn module, in characters of padding.
const MODULE_PAD: i16 = 2;

/// A pipe whose readable end becomes ready when a signal arrives.
static EXIT_PIPE: std::sync::OnceLock<(i32, i32)> = std::sync::OnceLock::new();

extern "C" fn on_exit_signal(_sig: i32) {
    // Writing one byte is async-signal-safe. The value is ignored.
    if let Some((_, write)) = EXIT_PIPE.get() {
        // SAFETY: a 1-byte write to a pipe created before any handler ran.
        unsafe {
            let byte = [1u8];
            let _ = libc_write(*write, byte.as_ptr() as *const _, 1);
        }
    }
}

/// Create the signal pipe. Returns (read, write).
fn install_exit_signal() -> Result<(i32, i32)> {
    let mut fds = [0i32; 2];
    // SAFETY: `fds` is a valid two-element array.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } != 0 {
        return Err("cannot create the signal pipe".into());
    }
    let _ = EXIT_PIPE.set((fds[0], fds[1]));
    unsafe {
        libc::signal(
            libc::SIGTERM,
            on_exit_signal as *const () as libc::sighandler_t,
        );
        libc::signal(
            libc::SIGINT,
            on_exit_signal as *const () as libc::sighandler_t,
        );
    }
    Ok((fds[0], fds[1]))
}

/// True when a signal has arrived on the pipe.
fn exit_requested(read_fd: i32) -> bool {
    let mut byte = [0u8; 1];
    let mut pfd = PollFd {
        fd: read_fd,
        events: 1,
        revents: 0,
    };
    // SAFETY: one valid pollfd with a zero timeout.
    let n = unsafe { poll(&mut pfd, 1, 0) };
    if n > 0 {
        // SAFETY: reading one byte into a one-byte buffer.
        unsafe {
            let _ = libc_read(read_fd, byte.as_mut_ptr() as *mut _, 1);
        }
        return true;
    }
    false
}

/// Wait for either the X socket or the exit pipe.
fn wait_readable2(x_fd: i32, exit_fd: i32, timeout_ms: i32) {
    let mut pfds = [
        PollFd {
            fd: x_fd,
            events: 1,
            revents: 0,
        },
        PollFd {
            fd: exit_fd,
            events: 1,
            revents: 0,
        },
    ];
    // SAFETY: two valid pollfds for the duration of the call.
    unsafe {
        poll(pfds.as_mut_ptr(), 2, timeout_ms);
    }
}

/// True when the event is a key press the panel dismisses on.
fn is_dismiss_key(event: &Event) -> bool {
    matches!(event, Event::KeyPress(_))
}

/// True when an event was delivered to `window`.
fn event_is_from(event: &Event, window: Window) -> bool {
    match event {
        Event::ButtonPress(e) => e.event == window,
        Event::KeyPress(e) => e.event == window,
        Event::Expose(e) => e.window == window,
        _ => false,
    }
}

/// One drawn module, with where it landed so a click can open its panel.
#[derive(Clone, Copy)]
struct Placed {
    index: usize,
    x: i16,
    width: i16,
}

/// Which optional module gives way first when the bar is narrow.
fn module_priority(name: &str) -> i32 {
    match name {
        "clock" => 100,
        "battery" => 90,
        "network" => 85,
        "volume" => 80,
        "bluetooth" => 84,
        "powerprofile" => 83,
        "microphone" => 82,
        "layout" => 72,
        "media" => 70,
        "notifications" => 65,
        "tasks" => 66,
        "updates" => 60,
        "niruxupdate" => 58,
        "brightness" => 55,
        "weather" => 50,
        "sun" => 45,
        "pulse" => 40,
        "uptime" => 30,
        _ => 10,
    }
}

/// A tray belongs to the primary monitor so it does not migrate between bars
/// when a session starts. Without a usable primary, keep the old fallback.
fn tray_belongs_here(monitor: Option<&str>, primary: Option<&str>) -> bool {
    match (monitor, primary) {
        (Some(monitor), Some(primary)) => monitor == primary,
        _ => true,
    }
}

/// Popup module id for an SNI menu, distinct from overflow (`usize::MAX`).
const SNI_MENU: usize = usize::MAX - 1;

/// One clickable SNI icon in the last composed frame.
struct SniHit {
    address: String,
    x: i16,
    width: i16,
}

/// The menu currently shown for an SNI item.
struct SniMenu {
    item_address: String,
    menu_path: String,
    rows: Vec<sni::MenuRow>,
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
    /// Where this bar sits, for the strut and for placing popups.
    bar_x: i16,
    bar_y: i16,
    monitor: Option<Monitor>,
    /// System tray, when enabled.
    tray: Option<tray::Tray>,
    /// D-Bus StatusNotifierItem host, on the primary bar only.
    sni: Option<sni::Host>,
    /// Hit boxes for the SNI icons in the last frame.
    sni_hits: Vec<SniHit>,
    /// Context for the open SNI menu, if the current popup is one.
    sni_menu: Option<SniMenu>,
    modules: Vec<Box<dyn Module>>,
    layout_text: Arc<Mutex<String>>,
    workspaces: Workspaces,
    /// True when the running WM published `_NIRUWM_MONITORS`.
    monitor_managed: bool,
    /// Selection that makes this bar unique for its monitor.
    selection: Atom,
    /// Hit boxes from the last frame: where each module and workspace cell is.
    placed: Vec<Placed>,
    /// Overflow indicator hit box, when low-priority modules were hidden.
    overflow: Option<Placed>,
    /// Bounded media/audio controls; reading a panel never uses this.
    helpers: helpers::HelperPool,
    /// Summaries for hidden modules, used by the overflow panel.
    hidden: Vec<String>,
    cells: i16,
    /// The one open panel, if any. Owned by the bar so clicks stay responsive.
    popup: Option<popup::Popup>,
    /// The module texts of the frame being drawn. `frame_key` renders the
    /// modules once and `compose` uses what it produced.
    ///
    /// Rendering twice was a real bug: the media module advances the scroll
    /// position on every render, so the second render returned different text,
    /// the key looked unchanged, the frame was skipped, and the title froze
    /// after one step.
    rendered: Vec<String>,
    /// The window whose title was last shown, and since when.
    ///
    /// With `title_delay_seconds` set, a title is shown only once the focus
    /// has stayed on the same window that long. Switching several windows
    /// then shows no titles at all, which is what the eye wants.
    title_shown: Option<Window>,
    title_since: std::time::Instant,
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

    /// Create the bar.
    ///
    /// `monitor` places the bar at the top of one monitor instead of the whole
    /// X screen. With several screens the window manager starts one bar per
    /// monitor, each with its own workspace cells.
    fn new(monitor: Option<Monitor>) -> Result<Self> {
        let cfg = Config::load()?;
        let theme = Theme::load(&cfg.theme)?;
        let icons = cfg.bar.icons;
        let (c, s) = x11rb::connect(None)?;
        let screen = &c.setup().roots[s];
        let root = screen.root;
        // A bar on one monitor covers that monitor's width and sits at its top.
        let (bar_x, bar_y, width) = match &monitor {
            Some(m) => (m.x, m.y, m.w),
            None => (0, 0, screen.width_in_pixels),
        };

        let mut atoms = HashMap::new();
        for n in [
            "_NET_WM_WINDOW_TYPE",
            "_NET_WM_WINDOW_TYPE_DOCK",
            "_NET_WM_STRUT_PARTIAL",
            "_NET_WM_STRUT",
            "_NET_WM_NAME",
            "_NET_WM_PID",
            "_NET_WM_WINDOW_OPACITY",
            "UTF8_STRING",
            "_NET_CURRENT_DESKTOP",
            "_NET_CLIENT_LIST",
            "_NET_WM_DESKTOP",
            "_NET_ACTIVE_WINDOW",
            "_NET_DESKTOP_NAMES",
            "_NIRUWM_MONITORS",
            "_NIRUWM_LAYOUT",
            "_NIRUWM_COMMAND",
            "_NIRUBAR_SELECTION",
            "_NIRUBAR_REDRAW",
            "_NIRU_CONFIG_SERIAL",
            "_NIRUWM_COMMAND",
        ] {
            atoms.insert(n, c.intern_atom(false, n.as_bytes())?.reply()?.atom);
        }

        // One bar per monitor, but they share the X screen. The legacy
        // selection is kept as a best-effort "a bar is present" marker.
        let monitor_key = monitor
            .as_ref()
            .map(Monitor::key)
            .unwrap_or_else(|| "screen".into());
        let selection = c
            .intern_atom(
                false,
                format!("_NIRUBAR_SELECTION_{monitor_key}").as_bytes(),
            )?
            .reply()?
            .atom;
        if c.get_selection_owner(selection)?.reply()?.owner != 0 {
            return Err(format!("NIRUBAR is already running on monitor {monitor_key}").into());
        }

        let win = c.generate_id()?;
        let gc = c.generate_id()?;
        let mut renderer = render::Renderer::new(&c, root, &cfg, width)?;

        let background = Theme::pixel(&theme.background)?;
        c.create_window(
            0,
            win,
            root,
            bar_x,
            bar_y,
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
        // The bar itself paints opaque pixels; transparency comes from the
        // compositor reading this property. Absent means fully opaque.
        if let Some(opacity) = cfg.bar.effective_opacity() {
            c.change_property32(
                PropMode::REPLACE,
                win,
                atoms["_NET_WM_WINDOW_OPACITY"],
                AtomEnum::CARDINAL,
                &[opacity],
            )?;
        }
        set_strut(&c, win, &atoms, cfg.bar_height, width, bar_x, bar_y == 0)?;
        c.set_selection_owner(win, selection, x11rb::CURRENT_TIME)?;
        let legacy = atoms["_NIRUBAR_SELECTION"];
        if c.get_selection_owner(legacy)?.reply()?.owner == 0 {
            c.set_selection_owner(win, legacy, x11rb::CURRENT_TIME)?;
        }
        c.change_window_attributes(
            root,
            // Only property and structure events. Selecting BUTTON_PRESS here
            // would also claim SubstructureRedirect on the root, which is the
            // window manager's lock: the bar would then stop the WM from
            // starting at all. Clicks outside an open panel are caught by the
            // panel's own catch-all window instead.
            &ChangeWindowAttributesAux::new()
                .event_mask(EventMask::PROPERTY_CHANGE | EventMask::STRUCTURE_NOTIFY),
        )?;
        c.map_window(win)?;
        c.flush()?;

        renderer.resize(&c, root, width, cfg.bar_height)?;
        // A tray only when asked for, since it owns an X selection that
        // another tray program may already hold. Only the primary monitor
        // owns it: otherwise the first bar to start wins and the tray moves
        // between screens from one login to the next.
        let primary = status::display_outputs()
            .and_then(|outputs| outputs.into_iter().find(|output| output.primary))
            .map(|output| output.name);
        let monitor_name = monitor.as_ref().and_then(|m| m.name.as_deref());
        let owns_tray = cfg.bar.tray && tray_belongs_here(monitor_name, primary.as_deref());
        let tray = if owns_tray {
            match tray::Tray::new(&c, root, s, win) {
                Ok(t) => Some(t),
                Err(e) => {
                    eprintln!("nirubar: tray: {e}");
                    None
                }
            }
        } else {
            None
        };
        // StatusNotifierItem is independent of the XEmbed selection: an app
        // may register even when another program already owns the X tray.
        let sni = if owns_tray {
            match sni::Host::new() {
                Ok(host) => host,
                Err(e) => {
                    eprintln!("nirubar: sni: {e}");
                    None
                }
            }
        } else {
            None
        };
        let icons = crate::choose_icons(&renderer, icons);
        // Resolve the volume and media glyphs against the loaded font, the
        // same way the bar's own icons are chosen. Without this a fresh start
        // used the defaults and drew plain text, while a reload looked right.
        let mut cfg = cfg;
        cfg.bar.volume_icons = Some(av::volume_icons(|ch| renderer.has_glyph(ch), cfg.bar.icons));
        cfg.bar.media_icons = Some(av::media_icons(|ch| renderer.has_glyph(ch), cfg.bar.icons));
        let layout_text = Arc::new(Mutex::new(String::new()));
        // The bar's own screen travels into the modules: only the display
        // module needs it, to name which screen this is.
        let screen = module::Screen {
            name: monitor.as_ref().and_then(|m| m.name.clone()),
        };
        let modules = module::build(&cfg, icons, layout_text.clone(), screen);
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
            bar_x,
            bar_y,
            monitor,
            icons,
            tray,
            sni,
            sni_hits: Vec::new(),
            sni_menu: None,
            modules,
            layout_text,
            rendered: Vec::new(),
            monitor_managed: false,
            selection,
            workspaces,
            placed: Vec::new(),
            overflow: None,
            hidden: Vec::new(),
            helpers: helpers::HelperPool::new(4),
            cells,
            popup: None,
            title_shown: None,
            title_since: std::time::Instant::now(),
        })
    }

    /// Collect the EWMH state the bar displays.
    fn snapshot(&mut self) {
        let current_layout = self
            .c
            .get_property(
                false,
                self.root,
                self.atom("_NIRUWM_LAYOUT"),
                self.atom("UTF8_STRING"),
                0,
                64,
            )
            .ok()
            .and_then(|r| r.reply().ok())
            .map(|r| String::from_utf8_lossy(&r.value).trim().to_string())
            .unwrap_or_default();
        if !current_layout.is_empty() {
            if let Ok(mut value) = self.layout_text.lock() {
                *value = current_layout;
            }
        }
        let layout_text = self
            .c
            .get_property(
                false,
                self.root,
                self.atom("_NIRUWM_MONITORS"),
                self.atom("UTF8_STRING"),
                0,
                4096,
            )
            .ok()
            .and_then(|r| r.reply().ok())
            .map(|r| String::from_utf8_lossy(&r.value).to_string())
            .unwrap_or_default();
        let layout = parse_monitor_layout(&layout_text, &self.monitor);
        self.monitor_managed = layout.is_some();
        // The manager publishes each screen's own workspace layout in
        // `_NIRUWM_MONITORS`, so this bar shows what its screen is tiling with,
        // not whichever screen has focus. A newer manager adds an eighth field;
        // without it the global `_NIRUWM_LAYOUT` above stands.
        if let Some(own) = layout.as_ref().and_then(|l| l.layout.clone()) {
            if let Ok(mut value) = self.layout_text.lock() {
                *value = own;
            }
        }
        let fallback_active = self
            .values(self.root, "_NET_CURRENT_DESKTOP")
            .first()
            .copied()
            .unwrap_or(0);
        let (offset, count, active) = layout
            .as_ref()
            .map(|l| (l.offset, l.count, l.active))
            .unwrap_or((0, self.workspaces.names.len().max(1), fallback_active));

        let names = layout.as_ref().and_then(|_| {
            let reply = self
                .c
                .get_property(
                    false,
                    self.root,
                    self.atom("_NET_DESKTOP_NAMES"),
                    self.atom("UTF8_STRING"),
                    0,
                    (count * 256) as u32,
                )
                .ok()?
                .reply()
                .ok()?;
            let all: Vec<String> = reply
                .value
                .split(|b| *b == 0)
                .filter(|v| !v.is_empty())
                .map(|v| String::from_utf8_lossy(v).to_string())
                .collect();
            let start = offset as usize;
            all.get(start..start + count).map(|s| s.to_vec())
        });

        let mut occupied = vec![false; count];
        let clients = self
            .c
            .get_property(
                false,
                self.root,
                self.atom("_NET_CLIENT_LIST"),
                AtomEnum::WINDOW,
                0,
                4096,
            )
            .ok()
            .and_then(|r| r.reply().ok())
            .and_then(|r| r.value32().map(|v| v.collect::<Vec<_>>()))
            .unwrap_or_default();
        for w in clients {
            if let Some(t) = self.values(w, "_NET_WM_DESKTOP").first() {
                if *t >= offset && *t < offset + count as u32 {
                    occupied[(*t - offset) as usize] = true;
                }
            }
        }
        self.workspaces.update(offset, active, names, occupied);
    }

    fn title(&mut self) -> String {
        if !self.cfg.bar.show_title {
            return String::new();
        }
        if self.title_allowed().is_none() {
            return String::new();
        }
        self.title_text()
    }

    /// The active window's title, without touching the delay timer.
    fn title_text(&self) -> String {
        let Some(w) = self
            .values(self.root, "_NET_ACTIVE_WINDOW")
            .first()
            .copied()
            .filter(|w| *w != 0)
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

    /// The active window, if the title should be shown right now.
    fn title_allowed(&mut self) -> Option<Window> {
        let active = self
            .values(self.root, "_NET_ACTIVE_WINDOW")
            .first()
            .copied()
            .filter(|w| *w != 0);
        match active {
            Some(w) if self.title_shown == Some(w) => {}
            other => {
                self.title_shown = other;
                self.title_since = std::time::Instant::now();
            }
        }
        let w = active?;
        let delay = std::time::Duration::from_secs(u64::from(self.cfg.bar.title_delay_seconds));
        (self.title_since.elapsed() >= delay).then_some(w)
    }

    fn compose(&mut self) -> Result<()> {
        let bg = Theme::pixel(&self.theme.background)?;
        let fg = Theme::pixel(&self.theme.foreground)?;
        let muted = Theme::pixel(&self.theme.muted)?;
        let border = Theme::pixel(&self.theme.border)?;
        let accent = Theme::pixel(&self.theme.focus)?;
        let sni_items = self
            .sni
            .as_ref()
            .map(sni::Host::snapshot)
            .unwrap_or_default();
        let sni_slot = (sni::ICON_SIZE + sni::GAP) as i16;
        let sni_width = i16::try_from(sni_items.len())
            .unwrap_or(i16::MAX)
            .saturating_mul(sni_slot);
        self.renderer.clear(&self.c, self.gc, bg)?;
        self.placed.clear();
        self.sni_hits.clear();

        // Workspaces are cells, drawn first on the left.
        let cell = self.cells;
        let show_workspaces = self.cfg.bar.modules.iter().any(|m| m == "workspaces");
        if show_workspaces {
            for i in 0..self.workspaces.names.len() {
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
                } else if self.workspaces.occupied.get(i).copied().unwrap_or(false) {
                    (fg, bg)
                } else {
                    (muted, bg)
                };
                let label = self.workspaces.names[i].clone();
                let text_x = x + (cell - self.renderer.width(&label)) / 2;
                self.renderer
                    .text(&self.c, self.gc, text_x, &label, colour, cell_bg, x + cell)?;
                if !active && self.workspaces.occupied.get(i).copied().unwrap_or(false) {
                    let dot = 3;
                    self.renderer.fill(
                        &self.c,
                        self.gc,
                        Rectangle {
                            x: x + (cell - dot) / 2,
                            y: self.height as i16 - 4,
                            width: dot as u16,
                            height: 2,
                        },
                        accent,
                    )?;
                }
            }
        }

        // Right-aligned status modules, in config order. The tray is the first
        // item on the right, so it reads before the status text. SNI and XEmbed
        // share one strip: SNI on the left, XEmbed on the right. It takes part
        // in the same fit: a narrow bar hides low-priority modules and keeps an
        // overflow indicator reachable. Workspace cells never move.
        const TRAY_ITEM: usize = usize::MAX;
        let tray_width = self.tray.as_ref().map(|t| t.width()).unwrap_or(0) as i16;
        let strip_width = sni_width.saturating_add(tray_width);
        let right_edge = i16::try_from(self.width).unwrap_or(i16::MAX) - 10;
        let workspace_width = if show_workspaces {
            self.cells * self.workspaces.names.len() as i16
        } else {
            0
        };
        let mut texts: Vec<(usize, String)> = Vec::new();
        let mut marks: Vec<Option<crate::av::MediaIcons>> = Vec::new();
        let mut progress: Vec<Option<f32>> = Vec::new();
        for (index, m) in self.modules.iter().enumerate() {
            if m.side() != Side::Right {
                continue;
            }
            let text = match self.rendered.get(index) {
                Some(t) => t.clone(),
                None => continue,
            };
            if !text.is_empty() {
                texts.push((index, text));
                marks.push(m.mark());
                progress.push(m.progress());
            }
        }
        let gap = self.renderer.advance * MODULE_PAD;
        let mut items: Vec<layout::Item> = texts
            .iter()
            .map(|(index, text)| layout::Item {
                index: *index,
                width: self.renderer.width(text),
                priority: module_priority(self.modules[*index].name()),
            })
            .collect();
        if strip_width > 0 {
            items.insert(
                0,
                layout::Item {
                    index: TRAY_ITEM,
                    width: strip_width,
                    priority: 95,
                },
            );
        }
        let overflow_width = self.renderer.width("+99");
        let fit = layout::fit(workspace_width, right_edge, gap, overflow_width, &items);
        let mut placements: Vec<Option<(i16, i16)>> = vec![None; texts.len()];
        let mut tray_x = None;
        for placement in &fit.visible {
            if placement.index == TRAY_ITEM {
                tray_x = Some(placement.x);
                continue;
            }
            let slot = texts
                .iter()
                .position(|(index, _)| *index == placement.index);
            if let Some(slot) = slot {
                placements[slot] = Some((placement.x, placement.width));
            }
        }

        let separator = self.cfg.bar.separator.clone();
        let mut placed: Vec<Placed> = Vec::new();
        for (slot, (index, text)) in texts.iter().enumerate().rev() {
            let Some((x, w)) = placements[slot] else {
                continue;
            };
            if text.chars().count() == 1 {
                self.renderer.single_glyph(
                    &self.c,
                    self.gc,
                    x,
                    text.chars().next().unwrap(),
                    fg,
                    bg,
                )?;
            } else {
                self.renderer
                    .text(&self.c, self.gc, x, text, fg, bg, right_edge)?;
            }
            // The media module's mark is drawn, not typed: three bars of the
            // theme's foreground, sitting where its placeholder is.
            if let Some(bars) = marks.get(slot).and_then(|m| m.clone()) {
                self.draw_mark(x, bars, fg, bg)?;
            }
            // A hairline under the module: a faint track and a brighter part
            // up to the position.
            if let Some(share) = progress.get(slot).copied().flatten() {
                let y = (self.height as i16) - 3;
                self.renderer.fill(
                    &self.c,
                    self.gc,
                    Rectangle {
                        x,
                        y,
                        width: w.max(1) as u16,
                        height: 1,
                    },
                    border,
                )?;
                let filled = (f32::from(w.max(1) as u16) * share).round() as u16;
                if filled > 0 {
                    self.renderer.fill(
                        &self.c,
                        self.gc,
                        Rectangle {
                            x,
                            y,
                            width: filled,
                            height: 1,
                        },
                        muted,
                    )?;
                }
            }
            placed.push(Placed {
                index: *index,
                x,
                width: w,
            });
            if !separator.is_empty() && w > 0 && !text.starts_with(' ') {
                // A hairline at the midpoint of the gap, so both sides have
                // the same breathing room.
                self.renderer.fill(
                    &self.c,
                    self.gc,
                    Rectangle {
                        x: x - gap,
                        y: (self.height as i16) / 5,
                        width: 1,
                        height: (self.height / 2).max(4),
                    },
                    border,
                )?;
            }
        }
        placed.sort_by_key(|p| p.x);
        self.placed = placed;

        self.hidden = fit
            .hidden
            .iter()
            .filter_map(|slot| {
                let index = items.get(*slot)?.index;
                if index == TRAY_ITEM {
                    return None;
                }
                let (_, text) = texts.iter().find(|(candidate, _)| *candidate == index)?;
                let name = self.modules[index].name();
                let text = text.trim();
                if text.is_empty() {
                    Some(format!("{name}: (no data)"))
                } else {
                    Some(format!("{name}: {text}"))
                }
            })
            .collect();
        self.overflow = fit.overflow.map(|(x, width)| {
            let label = format!("+{}", self.hidden.len());
            self.renderer
                .text(&self.c, self.gc, x, &label, fg, bg, x + width)
                .ok();
            Placed {
                index: usize::MAX,
                x,
                width,
            }
        });
        if self.overflow.is_none() {
            self.hidden.clear();
        }

        // The window title, clipped to the space that is left.
        //
        // It is drawn here and nowhere else: a second call would compute the
        // title-delay timer a second time and could turn it off again.
        let title = if self.cfg.bar.show_title && self.title_shown.is_some() {
            self.title_text()
        } else {
            String::new()
        };
        if !title.is_empty() {
            let first_x = tray_x
                .into_iter()
                .chain(self.placed.iter().map(|p| p.x))
                .min()
                .or_else(|| self.overflow.map(|p| p.x))
                .unwrap_or(right_edge);
            let start = workspace_width;
            let end = first_x - gap * 2;
            if end > start {
                self.renderer
                    .text(&self.c, self.gc, start, &title, muted, bg, end)?;
            }
        }

        if let Some(x) = tray_x {
            let icon_y =
                (i16::try_from(self.height).unwrap_or(0) - sni::ICON_SIZE as i16).max(0) / 2;
            let mut icon_x = x;
            for item in &sni_items {
                if let Some(icon) = &item.icon {
                    self.renderer.argb(
                        &self.c,
                        self.gc,
                        icon_x,
                        icon_y,
                        sni::ICON_SIZE,
                        icon.width,
                        icon.height,
                        &icon.pixels,
                        bg,
                    )?;
                } else {
                    // A one-character badge keeps an iconless application
                    // visible without ever guessing at a missing pixmap.
                    self.renderer.fill(
                        &self.c,
                        self.gc,
                        Rectangle {
                            x: icon_x,
                            y: icon_y,
                            width: sni::ICON_SIZE,
                            height: sni::ICON_SIZE,
                        },
                        border,
                    )?;
                    let mut bytes = [0u8; 4];
                    let label = item.initial().encode_utf8(&mut bytes);
                    let width = self.renderer.width(label);
                    let text_x = icon_x + (sni::ICON_SIZE as i16 - width).max(0) / 2;
                    self.renderer.text(
                        &self.c,
                        self.gc,
                        text_x,
                        label,
                        fg,
                        border,
                        icon_x + sni::ICON_SIZE as i16,
                    )?;
                }
                self.sni_hits.push(SniHit {
                    address: item.address.clone(),
                    x: icon_x,
                    width: sni_slot,
                });
                icon_x = icon_x.saturating_add(sni_slot);
            }
        }
        if let Some(t) = &self.tray {
            match tray_x {
                Some(x) => t.layout(&self.c, 0, x + strip_width, self.height)?,
                None => t.hide(&self.c)?,
            }
        }

        self.c.flush()?;
        Ok(())
    }

    fn draw(&mut self) -> Result<()> {
        for failure in self.helpers.reap() {
            helpers::notify_failure(&failure);
        }
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
        self.rendered = modules.clone();
        let progress: Vec<Option<u16>> = self
            .modules
            .iter()
            .map(|m| {
                m.progress()
                    .map(|p| (p.clamp(0.0, 1.0) * 1000.0).round() as u16)
            })
            .collect();
        let title = self.title();
        let tray_width = self.tray.as_ref().map(|t| t.width()).unwrap_or(0);
        let sni_key = self
            .sni
            .as_ref()
            .map(sni::Host::frame_key)
            .unwrap_or_default();
        // The module text is part of the key, and the media module's text
        // changes on every scroll step, so a title in motion asks for a redraw
        // by itself. Width and tray size take part because they decide which
        // modules fit and whether the overflow indicator exists. SNI identity,
        // status, title and icon bytes take part so a tray change is painted
        // on the next idle tick.
        format!(
            "{width}:{tray_width}:{sni_key}:{tag}:{occupied:?}:{modules:?}:{progress:?}:{title}",
            width = self.width
        )
    }

    /// Draw the music mark as shapes, in the cell of its placeholder.
    ///
    /// Three bars of equal width and different heights, the middle one shorter,
    /// so the mark reads as sound at any font size and never changes width when
    /// the player starts or stops. The placeholder text underneath is painted
    /// over with the background colour first: its em space is wider than the
    /// drawn mark, and a leftover sliver of a glyph looks like a second mark.
    fn draw_mark(&self, x: i16, bars: crate::av::MediaIcons, fg: u32, bg: u32) -> Result<()> {
        let adv = self.renderer.advance.max(1);
        let line = self.renderer.line_height.max(1);
        let width = (adv / 3).max(2);
        let heights = [line * 6 / 10, line * 4 / 10, line * 6 / 10];
        let bottom = (line / 2) + line * 2 / 5;
        let total = width * 3;
        let left = x + (adv - total).max(0) / 2;
        // Clear the placeholder cell.
        self.renderer.fill(
            &self.c,
            self.gc,
            Rectangle {
                x,
                y: 0,
                width: adv as u16,
                height: self.height,
            },
            bg,
        )?;
        for (i, h) in heights.iter().enumerate() {
            let bh = (*h).max(2) as u16;
            self.renderer.fill(
                &self.c,
                self.gc,
                Rectangle {
                    x: left + i as i16 * width,
                    y: bottom - i16::try_from(bh).unwrap_or(2),
                    width: width as u16,
                    height: bh,
                },
                fg,
            )?;
        }
        let _ = bars;
        Ok(())
    }

    /// Run an explicit non-left control on the module under the pointer.
    fn control(&mut self, x: i16, input: module::Input) -> Result<()> {
        let Some(index) = self
            .placed
            .iter()
            .find(|p| x >= p.x && x < p.x + p.width)
            .map(|p| p.index)
        else {
            return Ok(());
        };
        let Some(control) = self.modules.get_mut(index).and_then(|m| m.control(input)) else {
            return Ok(());
        };
        if let Some((program, args)) = control.command {
            let result = if control.long_lived {
                self.helpers.spawn_ui(&control.key, &program, &args)
            } else {
                self.helpers.spawn(&control.key, &program, &args)
            };
            if let Err(error) = result {
                helpers::notify_failure(&error);
            }
        }
        // Player selection has no command but changes what the next read uses.
        self.last_frame.clear();
        self.draw()?;
        Ok(())
    }

    /// Open the existing control view for a module that has one.
    fn controls(&mut self, index: usize) -> Result<()> {
        let Some(control) = self.modules.get(index).and_then(|module| module.controls()) else {
            return Ok(());
        };
        if let Some((program, args)) = control.command {
            let result = if control.long_lived {
                self.helpers.spawn_ui(&control.key, &program, &args)
            } else {
                self.helpers.spawn(&control.key, &program, &args)
            };
            if let Err(error) = result {
                helpers::notify_failure(&error);
            }
        }
        self.last_frame.clear();
        self.draw()?;
        Ok(())
    }

    /// Find the SNI item under a bar x coordinate. The address, not the
    /// snapshot index, identifies it: an asynchronous add or remove between a
    /// paint and a click must not activate the wrong application.
    fn sni_target(&self, x: i16) -> Option<(usize, sni::Item)> {
        let hit = self
            .sni_hits
            .iter()
            .find(|hit| x >= hit.x && x < hit.x + hit.width)?;
        let snapshot = self.sni.as_ref()?.snapshot();
        snapshot
            .into_iter()
            .enumerate()
            .find(|(_, item)| item.address == hit.address)
    }

    fn sni_left(
        &mut self,
        index: usize,
        item: sni::Item,
        hit_x: i16,
        root_x: i16,
        root_y: i16,
    ) -> Result<()> {
        if self
            .popup
            .as_ref()
            .is_some_and(|popup| popup.module == SNI_MENU)
            && self
                .sni_menu
                .as_ref()
                .is_some_and(|menu| menu.item_address == item.address)
        {
            return self.close_popup();
        }
        self.close_popup()?;
        if sni::left_click_opens_menu(&item) {
            return self.open_sni_menu(&item, hit_x);
        }
        if let Some(host) = &self.sni {
            host.activate(index, false, root_x, root_y);
        }
        Ok(())
    }

    fn sni_right(
        &mut self,
        index: usize,
        item: sni::Item,
        hit_x: i16,
        root_x: i16,
        root_y: i16,
    ) -> Result<()> {
        self.close_popup()?;
        if item.menu.is_some() {
            return self.open_sni_menu(&item, hit_x);
        }
        if let Some(host) = &self.sni {
            host.activate(index, true, root_x, root_y);
        }
        Ok(())
    }

    fn open_sni_menu(&mut self, item: &sni::Item, hit_x: i16) -> Result<()> {
        let Some(menu) = item.menu.as_ref() else {
            return Ok(());
        };
        let Some(menu_path) = item.menu_path.clone() else {
            return Ok(());
        };
        let rows = sni::menu_rows(menu);
        let actions: Vec<popup::ActionRow> = rows
            .iter()
            .map(|row| popup::ActionRow {
                label: row.label.clone(),
                enabled: row.enabled,
                separator: row.separator,
            })
            .collect();
        let title = if !item.title.is_empty() {
            item.title.clone()
        } else if !item.id.is_empty() {
            item.id.clone()
        } else {
            "Tray menu".to_string()
        };
        let popup = popup::Popup::open(
            &self.c,
            self.root,
            &self.theme,
            self.height,
            self.cfg.font_size,
            self.cfg.font_path.as_deref(),
            SNI_MENU,
            self.bar_x,
            self.bar_y,
            self.width,
            self.win,
            hit_x,
            &title,
            &[],
            &actions,
            false,
            self.cfg.bar.popup_timeout_seconds,
        )?;
        self.sni_menu = Some(SniMenu {
            item_address: item.address.clone(),
            menu_path,
            rows,
        });
        self.popup = Some(popup);
        Ok(())
    }

    fn sni_action(&mut self, index: usize) -> Result<()> {
        let action = self.sni_menu.take().and_then(|menu| {
            menu.rows
                .get(index)
                .map(|row| (menu.item_address, menu.menu_path, row.id))
        });
        self.close_popup()?;
        if let Some((address, menu_path, id)) = action {
            if let Some(host) = &self.sni {
                host.menu_action(&address, &menu_path, id);
            }
        }
        Ok(())
    }

    /// Open the detail panel for whatever was clicked.
    /// Handle a click on the bar.
    ///
    /// With a panel open, the same module toggles it and any other spot
    /// closes it; a click on a different module then opens that one.
    fn click(&mut self, x: i16) -> Result<()> {
        if self.cfg.bar.modules.iter().any(|m| m == "workspaces")
            && x < self.cells * self.workspaces.names.len() as i16
        {
            self.close_popup()?;
            let tag = (x / self.cells)
                .clamp(0, self.workspaces.names.len().saturating_sub(1) as i16)
                as u32;
            return self.select_workspace(tag);
        }
        if let Some(overflow) = self.overflow {
            if x >= overflow.x && x < overflow.x + overflow.width {
                if let Some(open) = &self.popup {
                    if open.module == usize::MAX {
                        return self.close_popup();
                    }
                }
                self.close_popup()?;
                let lines = self.hidden.clone();
                self.popup = Some(popup::Popup::open(
                    &self.c,
                    self.root,
                    &self.theme,
                    self.height,
                    self.cfg.font_size,
                    self.cfg.font_path.as_deref(),
                    usize::MAX,
                    self.bar_x,
                    self.bar_y,
                    self.width,
                    self.win,
                    overflow.x,
                    "Hidden modules",
                    &lines,
                    &[],
                    false,
                    self.cfg.bar.popup_timeout_seconds,
                )?);
                return Ok(());
            }
        }
        if let Some(index) = self
            .placed
            .iter()
            .find(|p| x >= p.x && x < p.x + p.width)
            .map(|p| p.index)
        {
            if self
                .modules
                .get(index)
                .is_some_and(|module| module.name() == "wifi")
            {
                self.close_popup()?;
                return self.controls(index);
            }
        }
        if !self.cfg.bar.popups {
            return Ok(());
        }
        let hit = self
            .placed
            .iter()
            .find(|p| x >= p.x && x < p.x + p.width)
            .map(|p| (p.index, p.x));
        let Some((index, left)) = hit else {
            self.close_popup()?;
            return Ok(());
        };
        // Clicking the module that opened the open panel closes it.
        if let Some(open) = &self.popup {
            if open.module == index {
                return self.close_popup();
            }
        }
        self.close_popup()?;
        let controls = self
            .modules
            .get(index)
            .and_then(|module| module.controls())
            .is_some();
        let content = self.modules.get_mut(index).and_then(|m| m.popup());
        if let Some((title, lines)) = content {
            self.popup = Some(popup::Popup::open(
                &self.c,
                self.root,
                &self.theme,
                self.height,
                self.cfg.font_size,
                self.cfg.font_path.as_deref(),
                index,
                self.bar_x,
                self.bar_y,
                self.width,
                self.win,
                left,
                &title,
                &lines,
                &[],
                controls,
                self.cfg.bar.popup_timeout_seconds,
            )?);
        }
        Ok(())
    }

    fn select_workspace(&mut self, tag: u32) -> Result<()> {
        let global = self.workspaces.offset + tag;
        let atom = if self.monitor_managed {
            // Monitor-targeted command; the WM resolves the global desktop.
            self.atom("_NIRUWM_COMMAND")
        } else {
            self.atom("_NET_CURRENT_DESKTOP")
        };
        let data = if self.monitor_managed {
            [9, global, 0, 0, 0]
        } else {
            [global, x11rb::CURRENT_TIME, 0, 0, 0]
        };
        self.c.send_event(
            false,
            self.root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            ClientMessageEvent::new(32, self.root, atom, data),
        )?;
        self.c.flush()?;
        self.last_frame.clear();
        self.draw()
    }

    /// Reclaim an icon's slot as soon as it stops being embedded. A client may
    /// destroy, unmap or reparent its icon away when it withdraws; waiting for
    /// DestroyNotify alone leaves blank places behind.
    fn drop_tray_icon(&mut self, icon: Window) -> Result<()> {
        let known = self
            .tray
            .as_ref()
            .is_some_and(|tray| tray.icons.contains(&icon));
        if !known {
            return Ok(());
        }
        if let Some(tray) = self.tray.as_mut() {
            tray.remove(icon);
        }
        self.last_frame.clear();
        self.draw()
    }

    fn close_popup(&mut self) -> Result<()> {
        self.sni_menu = None;
        if let Some(p) = self.popup.take() {
            p.destroy(&self.c, self.root)?;
        }
        Ok(())
    }

    fn reload(&mut self) -> Result<()> {
        let cfg = Config::load()?;
        let theme = Theme::load(&cfg.theme)?;
        let renderer = render::Renderer::new(&self.c, self.root, &cfg, self.width)?;
        self.close_popup()?;
        self.renderer.release(&self.c)?;
        self.renderer = renderer;
        self.c.configure_window(
            self.win,
            &ConfigureWindowAux::new().height(u32::from(cfg.bar_height)),
        )?;
        set_strut(
            &self.c,
            self.win,
            &self.atoms,
            cfg.bar_height,
            self.width,
            self.bar_x,
            self.bar_y == 0,
        )?;
        self.c.change_window_attributes(
            self.win,
            &ChangeWindowAttributesAux::new().background_pixel(Theme::pixel(&theme.background)?),
        )?;
        self.theme = theme;
        self.height = cfg.bar_height;
        self.icons = crate::choose_icons(&self.renderer, cfg.bar.icons);
        let mut cfg = cfg;
        cfg.bar.volume_icons = Some(av::volume_icons(
            |c| self.renderer.has_glyph(c),
            cfg.bar.icons,
        ));
        cfg.bar.media_icons = Some(av::media_icons(
            |c| self.renderer.has_glyph(c),
            cfg.bar.icons,
        ));
        let screen = module::Screen {
            name: self.monitor.as_ref().and_then(|m| m.name.clone()),
        };
        self.modules = module::build(&cfg, self.icons, self.layout_text.clone(), screen);
        self.workspaces = Workspaces::new(&cfg.bar);
        self.cells = cfg.bar.cell_width as i16;
        self.cfg = cfg;
        self.last_frame.clear();
        self.draw()
    }

    fn run(&mut self) -> Result<()> {
        // Exit on SIGTERM and SIGINT.
        //
        // The session script stops the bar with SIGTERM at logout. Without a
        // handler the process ignores it while it waits for X events, so the
        // bar outlived the session. The handler sets a flag and writes to a
        // pipe the event loop watches, which is the portable way to interrupt
        // a blocking X read.
        let (read_fd, _write_fd) = install_exit_signal()?;
        self.draw()?;
        loop {
            // Sleep until X has work, but never longer than a second while a
            // panel is open: that is how its timeout is enforced without a
            // second thread.
            if exit_requested(read_fd) {
                self.close_popup()?;
                break;
            }
            let event = match self.c.poll_for_event()? {
                Some(e) => e,
                None => {
                    if self.popup.as_ref().map(|p| p.expired()).unwrap_or(false) {
                        self.close_popup()?;
                    }
                    // A moving title needs the frame rate; everything else
                    // can wait for an event or the next second. A bar that
                    // repaints all the time burns a core for nothing.
                    let timeout = if self.popup.is_some() {
                        250
                    } else if self.modules.iter().any(|m| m.scrolling()) {
                        33
                    } else {
                        1000
                    };
                    wait_readable2(self.c.stream().as_raw_fd(), read_fd, timeout);
                    // Nothing arrived, so nothing below this point runs, and
                    // the bar used to go to sleep with a scrolling title in the
                    // middle of a move. Paint on the timer as well.
                    self.draw()?;
                    continue;
                }
            };
            // Arrow keys navigate an open calendar panel. Handle this before
            // the generic popup handler, which closes on every key press.
            if let Event::KeyPress(e) = &event {
                if let Some(module) = self.popup.as_ref().map(|p| p.module) {
                    let delta = match e.detail {
                        113 | 112 => Some(-1), // Left / PageUp
                        114 | 117 => Some(1),  // Right / PageDown
                        _ => None,
                    };
                    if let Some(delta) = delta {
                        if self
                            .modules
                            .get_mut(module)
                            .is_some_and(|m| m.popup_navigate(delta))
                        {
                            let left = self
                                .placed
                                .iter()
                                .find(|p| p.index == module)
                                .map(|p| p.x)
                                .unwrap_or(0);
                            self.close_popup()?;
                            self.click(left + 1)?;
                            continue;
                        }
                    }
                }
            }
            // A click on the panel itself: keep it open.
            if let Some(p) = &self.popup {
                // Escape is grabbed at the root, so it arrives with the root as
                // its window, not the panel's. A click inside the panel has the
                // panel as its window. Both belong to the open panel.
                if event_is_from(&event, p.window()) && matches!(event, Event::Expose(_)) {
                    p.repaint(&self.c, self.root, &self.theme)?;
                    continue;
                }
                if event_is_from(&event, p.window())
                    || event_is_from(&event, p.catch_window())
                    || event_is_from(&event, self.root) && is_dismiss_key(&event)
                {
                    let module = p.module;
                    match p.handle(&event) {
                        popup::Outcome::Close => self.close_popup()?,
                        popup::Outcome::Controls => {
                            self.close_popup()?;
                            self.controls(module)?;
                        }
                        popup::Outcome::Action(index) => self.sni_action(index)?,
                        popup::Outcome::Keep => {}
                    }
                    continue;
                }
            }
            match event {
                // A reload must not depend on where the command came from: the
                // window manager bumps the serial, `niructl` may deliver it
                // directly to the root, and a bar on its own must obey both.
                Event::PropertyNotify(e) if e.atom == self.atom("_NIRU_CONFIG_SERIAL") => {
                    match self.reload() {
                        Ok(()) => eprintln!("nirubar: reloaded"),
                        Err(e) => eprintln!("nirubar: reload rejected: {e}"),
                    }
                }
                Event::ButtonPress(e) => {
                    if e.event_x >= 0 {
                        if let Some((index, item)) = self.sni_target(e.event_x) {
                            match e.detail {
                                1 => self.sni_left(index, item, e.event_x, e.root_x, e.root_y)?,
                                3 => self.sni_right(index, item, e.event_x, e.root_x, e.root_y)?,
                                _ => {}
                            }
                            continue;
                        }
                    }
                    let control = match e.detail {
                        2 => Some(module::Input::Middle),
                        3 => Some(module::Input::Right),
                        4 => Some(module::Input::ScrollUp),
                        5 => Some(module::Input::ScrollDown),
                        _ => None,
                    };
                    if let Some(input) = control {
                        if e.event_x >= 0 {
                            self.control(e.event_x, input)?;
                        }
                        continue;
                    }
                    if let Some(p) = self.popup.take() {
                        self.sni_menu = None;
                        // Clicking the bar under a panel toggles or replaces it.
                        let same_module = self
                            .placed
                            .iter()
                            .find(|pl| e.event_x >= pl.x && e.event_x < pl.x + pl.width)
                            .map(|pl| pl.index == p.module)
                            .unwrap_or(false);
                        p.destroy(&self.c, self.root)?;
                        if same_module {
                            continue;
                        }
                    }
                    if e.event_x >= 0 {
                        self.click(e.event_x)?;
                    }
                }
                // A standalone bar follows the root width. A monitor bar keeps
                // the width the WM assigned: a global display change (mirror,
                // rotate, mode) must not turn every per-monitor bar into a
                // full-desktop bar.
                Event::ConfigureNotify(e)
                    if e.window == self.root
                        && follows_root_width(&self.monitor, self.width, e.width) =>
                {
                    self.close_popup()?;
                    self.width = e.width;
                    self.renderer
                        .resize(&self.c, self.root, self.width, self.height)?;
                    self.c.configure_window(
                        self.win,
                        &ConfigureWindowAux::new().width(u32::from(self.width)),
                    )?;
                    set_strut(
                        &self.c,
                        self.win,
                        &self.atoms,
                        self.height,
                        self.width,
                        self.bar_x,
                        self.bar_y == 0,
                    )?;
                    self.last_frame.clear();
                    self.draw()?;
                }
                Event::Expose(_) => {
                    self.last_frame.clear();
                    self.draw()?;
                }
                Event::NoExposure(_) => {}
                Event::PropertyNotify(_) => {
                    self.last_frame.clear();
                    self.draw()?;
                }
                Event::ClientMessage(e) => {
                    // `niructl reload` and `niructl theme` reach a bar that
                    // runs without a window manager as well; the window
                    // manager normally turns them into the serial bump
                    // below, and this path covers running the bar alone.
                    let reload = e.type_ == self.atom("_NIRUWM_COMMAND")
                        && matches!(e.data.as_data32()[0], 4);
                    if reload {
                        match self.reload() {
                            Ok(()) => eprintln!("nirubar: reloaded"),
                            Err(e) => eprintln!("nirubar: reload rejected: {e}"),
                        }
                        self.last_frame.clear();
                        self.draw()?;
                        continue;
                    }
                    let handled = self
                        .tray
                        .as_mut()
                        .map(|t| t.handle(&self.c, &e))
                        .unwrap_or(false);
                    self.last_frame.clear();
                    if handled {
                        self.draw()?;
                    }
                }
                Event::DestroyNotify(e) => {
                    self.drop_tray_icon(e.window)?;
                    if e.window == self.win {
                        break;
                    }
                }
                Event::UnmapNotify(e) => {
                    // Hidden icons must not keep their place in the tray.
                    self.drop_tray_icon(e.window)?;
                }
                Event::ReparentNotify(e) => {
                    // A client that withdraws reparents its icon away from us.
                    if e.parent != self.win {
                        self.drop_tray_icon(e.window)?;
                    }
                }
                Event::SelectionClear(e) => {
                    // Losing the tray is not fatal: another tray may take over
                    // while the bar itself remains useful. Only losing our own
                    // singleton selection ends this bar.
                    let tray_lost = self
                        .tray
                        .as_ref()
                        .is_some_and(|t| t.selection == e.selection);
                    if tray_lost {
                        if let Some(t) = &self.tray {
                            t.release(&self.c);
                        }
                        self.tray = None;
                        self.last_frame.clear();
                        self.draw()?;
                    } else if e.selection == self.selection {
                        // Another bar legitimately took over this monitor slot.
                        break;
                    }
                    // Best-effort legacy marker: losing it is harmless.
                }
                _ => (),
            }
        }
        Ok(())
    }
}

/// Reserve screen space for the bar.
///
/// Only a bar on the top edge reserves space: `_NET_WM_STRUT` has no way to
/// describe a bar in the middle of a tall virtual screen, and a monitor below
/// the primary one must not take space from the primary screen's work area.
fn set_strut(
    c: &RustConnection,
    win: Window,
    atoms: &HashMap<&'static str, Atom>,
    height: u16,
    width: u16,
    x: i16,
    top_edge: bool,
) -> Result<()> {
    c.change_property32(
        PropMode::REPLACE,
        win,
        atoms["_NET_WM_STRUT_PARTIAL"],
        AtomEnum::CARDINAL,
        &[
            0,
            0,
            if top_edge { u32::from(height) } else { 0 },
            0,
            0,
            0,
            0,
            0,
            x.max(0) as u32,
            (x.max(0) as u32) + u32::from(width.saturating_sub(1)),
            0,
            0,
        ],
    )?;
    c.change_property32(
        PropMode::REPLACE,
        win,
        atoms["_NET_WM_STRUT"],
        AtomEnum::CARDINAL,
        &[0, 0, if top_edge { u32::from(height) } else { 0 }, 0],
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
        updates: if ok(nerd.updates) {
            nerd.updates
        } else {
            plain.updates
        },
        cpu: if ok(nerd.cpu) { nerd.cpu } else { plain.cpu },
        sun: if ok(nerd.sun) { nerd.sun } else { plain.sun },
        uptime: if ok(nerd.uptime) {
            nerd.uptime
        } else {
            plain.uptime
        },
        brightness: if ok(nerd.brightness) {
            nerd.brightness
        } else {
            plain.brightness
        },
        display: if ok(nerd.display) {
            nerd.display
        } else {
            plain.display
        },
        mode: if ok(nerd.mode) { nerd.mode } else { plain.mode },
        layout: if ok(nerd.layout) {
            nerd.layout
        } else {
            plain.layout
        },
        microphone: if ok(nerd.microphone) {
            nerd.microphone
        } else {
            plain.microphone
        },
        bluetooth: if ok(nerd.bluetooth) {
            nerd.bluetooth
        } else {
            plain.bluetooth
        },
        powerprofile: if ok(nerd.powerprofile) {
            nerd.powerprofile
        } else {
            plain.powerprofile
        },
        notifications: if ok(nerd.notifications) {
            nerd.notifications
        } else {
            plain.notifications
        },
        music: if ok(nerd.music) {
            nerd.music
        } else {
            plain.music
        },
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = args.first().map(String::as_str).unwrap_or_default();
    if arg == "--version" {
        println!("NIRUBAR {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if arg == "--reset-config" {
        if args.len() != 1 {
            eprintln!("Usage: nirubar --reset-config");
            std::process::exit(2);
        }
        match config::reset_bar() {
            Ok((path, backup)) => {
                if let Some(backup) = backup {
                    println!("Backup: {}", backup.display());
                }
                println!(
                    "Reset [bar] in {}. Restart with: niructl restart-bar",
                    path.display()
                );
                return;
            }
            Err(e) => {
                eprintln!("nirubar: {e}");
                std::process::exit(1)
            }
        }
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
    let monitor = match parse_monitor(&args) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("nirubar: {e}");
            std::process::exit(2)
        }
    };
    if monitor.is_none() && !args.is_empty() {
        eprintln!("Usage: nirubar [--version|--check-config|--reset-config] [--monitor X,Y,W]");
        std::process::exit(2)
    }
    if let Err(e) = Bar::new(monitor).and_then(|mut b| b.run()) {
        eprintln!("nirubar: {e}");
        std::process::exit(1)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_tray_follows_the_primary_monitor() {
        assert!(super::tray_belongs_here(Some("DP-4"), Some("DP-4")));
        assert!(!super::tray_belongs_here(Some("DP-5"), Some("DP-4")));
        assert!(super::tray_belongs_here(None, Some("DP-4")));
        assert!(super::tray_belongs_here(Some("DP-4"), None));
    }

    #[test]
    fn a_monitor_bar_ignores_global_root_resizes() {
        let monitor = Some(super::Monitor {
            x: 0,
            y: 0,
            w: 2560,
            name: None,
        });
        assert!(!super::follows_root_width(&monitor, 2560, 5120));
        assert!(!super::follows_root_width(&monitor, 2560, 2560));
        assert!(super::follows_root_width(&None, 2560, 5120));
        assert!(!super::follows_root_width(&None, 2560, 2560));
    }

    #[test]
    fn monitor_argument_sets_the_geometry() {
        let args = vec!["--monitor".to_string(), "10,20,800".to_string()];
        let m = super::parse_monitor(&args).unwrap().unwrap();
        assert_eq!((m.x, m.y, m.w), (10, 20, 800));
    }

    #[test]
    fn monitor_argument_accepts_an_output_name() {
        let args = vec!["--monitor".to_string(), "10,20,800,eDP-1".to_string()];
        let m = super::parse_monitor(&args).unwrap().unwrap();
        assert_eq!(m.name.as_deref(), Some("eDP-1"));
        assert_eq!(m.key(), "eDP-1");
    }

    #[test]
    fn monitor_layout_selects_the_bar_output() {
        let text = "left\t0\t0\t800\t0\t3\t1\nright\t800\t0\t800\t3\t2\t0\n";
        let left = Some(super::Monitor {
            x: 0,
            y: 0,
            w: 800,
            name: Some("left".into()),
        });
        let right = Some(super::Monitor {
            x: 800,
            y: 0,
            w: 800,
            name: Some("right".into()),
        });
        assert_eq!(
            super::parse_monitor_layout(text, &left),
            Some(super::MonitorLayout {
                offset: 0,
                count: 3,
                active: 1,
                layout: None,
            })
        );
        assert_eq!(
            super::parse_monitor_layout(text, &right),
            Some(super::MonitorLayout {
                offset: 3,
                count: 2,
                active: 0,
                layout: None,
            })
        );
        assert_eq!(super::parse_monitor_layout(text, &None), None);
    }

    /// With two screens the bars must not share one layout. A newer manager
    /// adds the layout of the screen's own workspace as the eighth field.
    #[test]
    fn each_screen_reports_its_own_workspace_layout() {
        let text = "left\t0\t0\t800\t0\t3\t1\ttile\nright\t800\t0\t800\t3\t2\t0\tmonocle\n";
        let left = Some(super::Monitor {
            x: 0,
            y: 0,
            w: 800,
            name: Some("left".into()),
        });
        let right = Some(super::Monitor {
            x: 800,
            y: 0,
            w: 800,
            name: Some("right".into()),
        });
        assert_eq!(
            super::parse_monitor_layout(text, &left).and_then(|l| l.layout),
            Some("tile".to_string())
        );
        assert_eq!(
            super::parse_monitor_layout(text, &right).and_then(|l| l.layout),
            Some("monocle".to_string())
        );
    }

    #[test]
    fn monitor_argument_needs_three_numbers() {
        let args = vec!["--monitor".to_string(), "10,20".to_string()];
        assert!(super::parse_monitor(&args).is_err());
    }

    #[test]
    fn no_arguments_means_the_default_geometry() {
        assert!(super::parse_monitor(&[]).unwrap().is_none());
    }

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
