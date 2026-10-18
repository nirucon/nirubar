//! Modules: the pieces the bar can show.
//!
//! Each module is independent and can be switched on or off in the config.
//! A module produces text (and optionally a popup with more detail) and knows
//! its own width, so adding one later does not touch the layout code.

use crate::{config, status};
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
        let b = status::battery()?;
        Some(("Battery".into(), status::battery_detail(&b)))
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
        let link = status::network()?;
        Some(("Network".into(), status::network_detail(link)))
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
