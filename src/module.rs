//! Modules: the pieces the bar can show.
//!
//! Each module is independent and can be switched on or off in the config.
//! A module produces text (and optionally a popup with more detail) and knows
//! its own width, so adding one later does not touch the layout code.

use crate::{av, config, status, updates};
use chrono::Datelike;

/// Where a module sits on the bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// Reserved for modules that belong next to the workspaces.
    #[allow(dead_code)]
    Left,
    Right,
}

/// One piece of the bar.
pub trait Module {
    /// Stable name used in the config.
    #[allow(dead_code)] // used by tests and for future modules
    fn name(&self) -> &'static str;
    /// Which side of the bar it belongs to.
    fn side(&self) -> Side;
    /// Current text, or an empty string when it has nothing to show.
    fn render(&mut self) -> String;
    /// Title and body for the click popup, or `None` when it has no popup.
    fn popup(&mut self) -> Option<(String, Vec<String>)> {
        None
    }
}

/// A workspace cell. Kept separate from the text modules because the bar draws
/// each cell as a block.
pub struct Workspaces {
    pub names: Vec<String>,
    pub active: u32,
    pub occupied: [bool; 9],
}

impl Workspaces {
    pub fn new(cfg: &config::Bar) -> Self {
        let mut names = cfg.tags.clone();
        names.resize(9, String::new());
        for (i, n) in names.iter_mut().enumerate() {
            if n.is_empty() {
                *n = (i + 1).to_string();
            }
        }
        Self {
            names,
            active: 0,
            occupied: [false; 9],
        }
    }

    pub fn update(&mut self, active: u32, occupied: [bool; 9]) {
        self.active = active;
        self.occupied = occupied;
    }
}

pub struct Clock {
    pub format_date: String,
    pub format_time: String,
    pub show: Show,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Show {
    DateAndTime,
    Date,
    Time,
}

impl Clock {
    pub fn new(cfg: &config::Bar) -> Self {
        Self {
            format_date: cfg.date_format.clone(),
            format_time: cfg.time_format.clone(),
            show: match (cfg.show_date, cfg.show_time) {
                (true, true) => Show::DateAndTime,
                (true, false) => Show::Date,
                (false, true) => Show::Time,
                (false, false) => Show::Time,
            },
        }
    }
}

impl Module for Clock {
    fn name(&self) -> &'static str {
        "clock"
    }
    fn side(&self) -> Side {
        Side::Right
    }
    fn render(&mut self) -> String {
        let now = chrono::Local::now();
        let d = now.format(&self.format_date).to_string();
        let t = now.format(&self.format_time).to_string();
        match self.show {
            Show::DateAndTime => format!("{d}  {t}"),
            Show::Date => d,
            Show::Time => t,
        }
    }
    fn popup(&mut self) -> Option<(String, Vec<String>)> {
        Some(crate::popup::calendar(&self.format_date))
    }
}

pub struct Battery {
    pub icons: status::Icons,
}

impl Module for Battery {
    fn name(&self) -> &'static str {
        "battery"
    }
    fn side(&self) -> Side {
        Side::Right
    }
    fn render(&mut self) -> String {
        match status::battery() {
            Some(b) => status::battery_text(&b, &self.icons),
            None => String::new(),
        }
    }
    fn popup(&mut self) -> Option<(String, Vec<String>)> {
        // Always open. A machine without a battery explains that instead of
        // ignoring the click.
        match status::battery() {
            Some(b) => Some(("Battery".into(), status::battery_detail(&b))),
            None => Some((
                "Battery".into(),
                vec!["no battery in this machine".to_string()],
            )),
        }
    }
}

pub struct Network {
    pub icons: status::Icons,
    pub use_icons: bool,
}

impl Module for Network {
    fn name(&self) -> &'static str {
        "network"
    }
    fn side(&self) -> Side {
        Side::Right
    }
    fn render(&mut self) -> String {
        status::network_text(status::network(), &self.icons, self.use_icons)
    }
    fn popup(&mut self) -> Option<(String, Vec<String>)> {
        // Always open, even when offline: a click that does nothing looks
        // broken, and people want to know why the icon is missing.
        match status::network() {
            Some(link) => Some(("Network".into(), status::network_detail(link))),
            None => Some((
                "Network".into(),
                vec![
                    "no wired or wireless connection".to_string(),
                    String::new(),
                    "check the cable or the wireless switch".to_string(),
                ],
            )),
        }
    }
}

pub struct Volume {
    pub icons: av::VolumeIcons,
}

impl Module for Volume {
    fn name(&self) -> &'static str {
        "volume"
    }
    fn side(&self) -> Side {
        Side::Right
    }
    fn render(&mut self) -> String {
        match av::Volume::read() {
            Some(v) => av::volume_text(&v, &self.icons),
            // No audio server answers. Showing the speaker with a dash says
            // "the module is here, the sound is not", which is truer than the
            // silence of a module that vanishes.
            None => format!("{} --", self.icons.muted),
        }
    }
    fn popup(&mut self) -> Option<(String, Vec<String>)> {
        // Clicking toggles mute: the most useful thing a volume icon can do.
        av::Volume::toggle_mute();
        let lines = match av::Volume::read() {
            Some(v) => vec![
                if v.muted {
                    "muted".to_string()
                } else {
                    format!("{}%", v.percent)
                },
                String::new(),
                "click to mute or unmute".to_string(),
                String::new(),
                "media keys: volume up, down, mute".to_string(),
            ],
            None => vec![
                "no audio server answers".to_string(),
                String::new(),
                "on Void the session starts PipeWire at login".to_string(),
                "elsewhere check: wpctl status".to_string(),
            ],
        };
        Some(("Volume".into(), lines))
    }
}

pub struct Media {
    pub icons: av::MediaIcons,
    /// Scrolls a long title instead of cutting it off.
    pub scroller: crate::scroll::Scroller,
}

impl Module for Media {
    fn name(&self) -> &'static str {
        "media"
    }
    fn side(&self) -> Side {
        Side::Right
    }
    fn render(&mut self) -> String {
        let Some(m) = av::Media::read() else {
            return String::new();
        };
        let glyph = if m.playing {
            self.icons.playing
        } else {
            self.icons.paused
        };
        let full = m.line(usize::MAX);
        // A short title is shown as it is; a long one scrolls, so the artist
        // and as much of the title as fits are readable in turn.
        self.scroller.observe(&full);
        if self.scroller.needs_scroll(&full) {
            format!("{glyph} {}", self.scroller.frame(&full))
        } else {
            format!("{glyph} {full}")
        }
    }
    fn popup(&mut self) -> Option<(String, Vec<String>)> {
        // Clicking plays or pauses.
        av::Media::toggle();
        let lines = match av::Media::read() {
            Some(m) => vec![
                m.title.clone(),
                m.artist.clone(),
                String::new(),
                if m.playing {
                    "playing".to_string()
                } else {
                    "paused".to_string()
                },
                "click to play or pause".to_string(),
            ],
            None => vec![
                "no player running".to_string(),
                String::new(),
                "a player must support MPRIS, for example mpv, VLC or Firefox".to_string(),
            ],
        };
        Some(("Media".into(), lines))
    }
}

pub struct Updates {
    pub icons: status::Icons,
    pub every: std::time::Duration,
}

impl Module for Updates {
    fn name(&self) -> &'static str {
        "updates"
    }
    fn side(&self) -> Side {
        Side::Right
    }
    fn render(&mut self) -> String {
        match updates::cached(self.every) {
            Some(n) => updates::text(n, self.icons.updates),
            None => String::new(),
        }
    }
    fn popup(&mut self) -> Option<(String, Vec<String>)> {
        let manager = updates::Manager::detect();
        let how = match manager {
            updates::Manager::Xbps => "sudo xbps-install -Su",
            updates::Manager::Pacman => "sudo pacman -Syu",
            updates::Manager::Apt => "sudo apt upgrade",
            updates::Manager::Unknown => "no package manager found",
        };
        Some((
            "Updates".into(),
            vec![
                match updates::query() {
                    Some(0) => "everything is up to date".to_string(),
                    Some(1) => "1 package can be upgraded".to_string(),
                    Some(n) => format!("{n} packages can be upgraded"),
                    None => "cannot read the package database".to_string(),
                },
                String::new(),
                how.to_string(),
            ],
        ))
    }
}

/// Build the active modules in the order the config lists them.
///
/// An unknown name is reported once and skipped, so a typo in the config does
/// not stop the bar from starting.
pub fn build(cfg: &config::Bar, icons: status::Icons) -> Vec<Box<dyn Module>> {
    let mut out: Vec<Box<dyn Module>> = Vec::new();
    for name in &cfg.modules {
        match name.as_str() {
            "clock" => out.push(Box::new(Clock::new(cfg))),
            "battery" => out.push(Box::new(Battery { icons })),
            "network" => out.push(Box::new(Network {
                icons,
                use_icons: cfg.icons,
            })),
            "updates" => out.push(Box::new(Updates {
                icons,
                every: std::time::Duration::from_secs(u64::from(cfg.update_check_minutes) * 60),
            })),
            "volume" => out.push(Box::new(Volume {
                icons: cfg.volume_icons(),
            })),
            "media" => out.push(Box::new(Media {
                icons: cfg.media_icons(),
                scroller: crate::scroll::Scroller::new(cfg.media_width),
            })),
            // `workspaces` is drawn separately, not as a text module.
            "workspaces" => {}
            other => eprintln!("nirubar: unknown module {other:?} in [bar] modules"),
        }
    }
    out
}

/// Month grid for the calendar popup.
pub fn month_grid(year: i32, month: u32) -> Vec<String> {
    use chrono::{NaiveDate, Weekday};
    let first = NaiveDate::from_ymd_opt(year, month, 1)
        .unwrap_or_else(|| NaiveDate::from_ymd_opt(year, 1, 1).expect("valid date"));
    let mut day = first;
    let mut cells: Vec<Option<u32>> = Vec::new();
    // Monday first.
    let lead = match first.weekday() {
        Weekday::Mon => 0,
        Weekday::Tue => 1,
        Weekday::Wed => 2,
        Weekday::Thu => 3,
        Weekday::Fri => 4,
        Weekday::Sat => 5,
        Weekday::Sun => 6,
    };
    for _ in 0..lead {
        cells.push(None);
    }
    loop {
        cells.push(Some(day.day()));
        match day.succ_opt() {
            Some(next) if next.month() == month => day = next,
            _ => break,
        }
    }
    while cells.len() % 7 != 0 {
        cells.push(None);
    }
    let mut rows = Vec::new();
    for chunk in cells.chunks(7) {
        rows.push(
            chunk
                .iter()
                .map(|c| match c {
                    Some(d) => format!("{d:>2}"),
                    None => "  ".to_string(),
                })
                .collect::<Vec<_>>()
                .join("  "),
        );
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_starts_on_monday() {
        // 2026-10-01 is a Thursday, so the first row has three blanks.
        let rows = month_grid(2026, 10);
        assert!(rows[0].starts_with("        "), "row: {:?}", rows[0]);
        assert!(rows[0].contains(" 1"));
    }

    #[test]
    fn calendar_has_full_weeks() {
        // Seven cells per row, two spaces between them, two characters each:
        // 7 * 2 + 6 * 2 = 26 characters.
        for m in 1..=12 {
            for r in month_grid(2026, m) {
                assert_eq!(r.chars().count(), 26, "row {:?} in month {m}", r);
            }
        }
    }

    #[test]
    fn workspace_labels_default_to_numbers() {
        let cfg = config::Bar::default();
        let ws = Workspaces::new(&cfg);
        assert_eq!(ws.names[0], "1");
        assert_eq!(ws.names[8], "9");
    }

    #[test]
    fn workspace_labels_can_be_renamed() {
        let cfg = config::Bar {
            tags: vec!["web".into(), "code".into(), "chat".into()],
            ..Default::default()
        };
        let ws = Workspaces::new(&cfg);
        assert_eq!(ws.names[0], "web");
        assert_eq!(ws.names[2], "chat");
        // Unset entries keep their number.
        assert_eq!(ws.names[3], "4");
    }

    #[test]
    fn unknown_modules_are_skipped() {
        let cfg = config::Bar {
            modules: vec!["clock".into(), "nonsense".into(), "battery".into()],
            ..Default::default()
        };
        let mods = build(&cfg, status::Icons::plain());
        assert_eq!(mods.len(), 2);
        assert_eq!(mods[0].name(), "clock");
        assert_eq!(mods[1].name(), "battery");
    }
}
