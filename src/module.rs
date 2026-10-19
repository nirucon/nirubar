//! Modules: the pieces the bar can show.
//!
//! Each module is independent and can be switched on or off in the config.
//! A module produces text (and optionally a popup with more detail) and knows
//! its own width, so adding one later does not touch the layout code.

use crate::{ambient, av, config, status, updates};
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
    /// Whether its text changes without anything happening in the session.
    ///
    /// Only the media title does: it moves while it plays. The bar repaints on
    /// a timer for these modules, and stays idle for every other one.
    fn scrolling(&self) -> bool {
        false
    }
    /// How far the module's own progress is, 0..=1, when it shows one.
    fn progress(&self) -> Option<f32> {
        None
    }
    /// Shapes this module draws in place of its mark, when it has one.
    ///
    /// `None` means "the text is complete, draw it as it is".
    fn mark(&self) -> Option<av::MediaIcons> {
        None
    }
}

/// A workspace cell. Kept separate from the text modules because the bar draws
/// each cell as a block.
pub struct Workspaces {
    pub names: Vec<String>,
    /// Active workspace local to this monitor.
    pub active: u32,
    pub occupied: Vec<bool>,
    /// First global EWMH desktop owned by this monitor.
    pub offset: u32,
}

impl Workspaces {
    pub fn new(cfg: &config::Bar) -> Self {
        let mut names = cfg.tags.clone();
        // Standalone use keeps the historical nine cells; a running WM
        // replaces both names and count from `_NIRUWM_MONITORS`.
        while names.len() < 9 {
            names.push((names.len() + 1).to_string());
        }
        for (i, n) in names.iter_mut().enumerate() {
            if n.is_empty() {
                *n = (i + 1).to_string();
            }
        }
        let occupied = vec![false; names.len()];
        Self {
            names,
            active: 0,
            occupied,
            offset: 0,
        }
    }

    /// Apply the WM's monitor-local state. Names are optional so a standalone
    /// bar without a WM keeps its configured labels.
    pub fn update(
        &mut self,
        offset: u32,
        active: u32,
        names: Option<Vec<String>>,
        occupied: Vec<bool>,
    ) {
        self.offset = offset;
        if let Some(names) = names {
            if !names.is_empty() {
                self.names = names;
            }
        }
        let count = self.names.len();
        self.active = active.min(count.saturating_sub(1) as u32);
        self.occupied.clear();
        self.occupied.extend(occupied);
        self.occupied.resize(count, false);
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

/// Probe cadence is independent of animation cadence; failures are cached too.
struct Sample<T> {
    checked: Option<std::time::Instant>,
    value: Option<T>,
}

impl<T> Default for Sample<T> {
    fn default() -> Self {
        Self {
            checked: None,
            value: None,
        }
    }
}

impl<T> Sample<T> {
    fn get(&mut self, probe: impl FnOnce() -> Option<T>) -> Option<&T> {
        let now = std::time::Instant::now();
        if self
            .checked
            .map(|t| now.duration_since(t).as_secs() >= 1)
            .unwrap_or(true)
        {
            self.checked = Some(now);
            self.value = probe();
        }
        self.value.as_ref()
    }
}

pub struct Volume {
    pub icons: av::VolumeIcons,
    sample: Sample<av::Volume>,
}

impl Module for Volume {
    fn name(&self) -> &'static str {
        "volume"
    }
    fn side(&self) -> Side {
        Side::Right
    }
    fn render(&mut self) -> String {
        match self.sample.get(av::Volume::read) {
            Some(v) => av::volume_text(v, &self.icons),
            // No audio server answers. Showing the speaker with a dash says
            // "the module is here, the sound is not", which is truer than the
            // silence of a module that vanishes.
            None => format!("{} --", self.icons.muted),
        }
    }
    fn popup(&mut self) -> Option<(String, Vec<String>)> {
        let lines = match av::Volume::read() {
            Some(v) => vec![
                if v.muted {
                    "muted".to_string()
                } else {
                    format!("{}%", v.percent)
                },
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

/// Placeholder for the drawn music mark. The em space keeps the layout honest
/// where the text width is measured; the bar paints shapes over it.
pub struct Media {
    sample: Sample<av::Media>,
    pub icons: av::MediaIcons,
    /// Scrolls a long title instead of cutting it off.
    pub scroller: crate::scroll::Scroller,
    /// Where the track is, 0..=1, for the thin line under the title.
    ///
    /// It follows the track, so it moves even while the title rests at its
    /// beginning: something small that tells you the player is alive.
    pub progress: Option<f32>,
}

impl Module for Media {
    fn name(&self) -> &'static str {
        "media"
    }
    fn side(&self) -> Side {
        Side::Right
    }
    fn render(&mut self) -> String {
        let Some(m) = self.sample.get(av::Media::read).cloned() else {
            // Nothing plays: let the next track start from its beginning, and
            // stop asking the bar to repaint.
            self.scroller.observe("");
            self.progress = None;
            return String::new();
        };
        self.progress = m
            .position
            .and_then(|(pos, len)| (len > 0).then(|| (pos as f32 / len as f32).clamp(0.0, 1.0)));
        let full = m.line(usize::MAX);
        // The scroller compares this string to decide whether the track
        // changed, so the icon and the scroll window must not be part of it.
        // Including them (as the first version did) made every frame look like
        // a new track: the position reset to the start and the title froze.
        self.scroller.observe(&full);
        let window = if self.scroller.needs_scroll(&full) {
            self.scroller.frame(&full)
        } else {
            full.clone()
        };
        // The mark is a fixed-width placeholder here; the bar draws its
        // shapes. A text glyph has a width only the font knows, so a scrolling
        // title would jump as the mark changed or the font fell back.
        let mark = if self.icons.playing_codes.is_empty() {
            // Plain text: the ">" the font can draw.
            if m.playing {
                self.icons.playing
            } else {
                self.icons.paused
            }
        } else {
            // One space where the bar paints its bars. Fixed width, whatever
            // the font does with a glyph.
            " "
        };
        format!("{mark} {window}")
    }
    fn scrolling(&self) -> bool {
        // Playback position is sampled once/second. Only moving text needs
        // the animation cadence; a short playing title can sleep too.
        self.scroller.moving()
    }
    fn mark(&self) -> Option<av::MediaIcons> {
        // Only when the font can draw shapes; otherwise the ">" stays text.
        (!self.icons.playing_codes.is_empty()).then(|| self.icons.clone())
    }

    fn progress(&self) -> Option<f32> {
        self.progress
    }
    fn popup(&mut self) -> Option<(String, Vec<String>)> {
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
                "use player controls or media keys for playback".to_string(),
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

/// How long the machine has been up, and how the memory and disk look.
pub struct Uptime {
    pub icons: status::Icons,
}

impl Module for Uptime {
    fn name(&self) -> &'static str {
        "uptime"
    }
    fn side(&self) -> Side {
        Side::Right
    }
    fn render(&mut self) -> String {
        match ambient::uptime_text() {
            // The clock icon sits before the age, the way the battery icon
            // sits before a percentage: same family, so the bar reads as one
            // set rather than three unrelated strings.
            Some(t) => format!("{} {t}", self.icons.uptime),
            None => String::new(),
        }
    }
    fn popup(&mut self) -> Option<(String, Vec<String>)> {
        let mut lines = Vec::new();
        match ambient::uptime_text() {
            Some(t) => lines.push(format!("up {t}")),
            None => lines.push("cannot read /proc/uptime".to_string()),
        }
        lines.push(String::new());
        lines.push(match crate::pulse::memory_percent() {
            Some(m) => format!("memory in use: {m}%"),
            None => "memory: unknown".to_string(),
        });
        lines.push(match crate::pulse::disk_percent() {
            Some(d) => format!("fullest filesystem: {d}% used"),
            None => "disk: unknown".to_string(),
        });
        Some(("Uptime".into(), lines))
    }
}

#[derive(Default)]
struct PulseState {
    samples: Vec<crate::pulse::Sample>,
    latest: crate::pulse::Sample,
}

/// A small graph of CPU and memory, drawn with block characters.
///
/// Sampling runs on one bounded worker thread: `/proc`, statvfs and filesystem
/// scans never execute inside rendering or input handling.
pub struct Pulse {
    state: std::sync::Arc<std::sync::Mutex<PulseState>>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
    width: usize,
    pub icons: status::Icons,
}

impl Pulse {
    pub fn new(width: usize, icons: status::Icons) -> Self {
        let state = std::sync::Arc::new(std::sync::Mutex::new(PulseState::default()));
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_state = state.clone();
        let worker_stop = stop.clone();
        let worker = std::thread::Builder::new()
            .name("nirubar-load".into())
            .spawn(move || {
                let mut previous = crate::pulse::CpuCounters::read().unwrap_or_default();
                while !worker_stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let now = crate::pulse::CpuCounters::read().unwrap_or_default();
                    let sample = crate::pulse::sample(&now, &previous);
                    previous = now;
                    if let Ok(mut state) = worker_state.lock() {
                        state.latest = sample;
                        state.samples.push(sample);
                        if state.samples.len() > 240 {
                            state.samples.remove(0);
                        }
                    }
                    // Sleep in small steps so Drop does not wait a full second.
                    for _ in 0..20 {
                        if worker_stop.load(std::sync::atomic::Ordering::Relaxed) {
                            return;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                }
            })
            .ok();
        Self {
            state,
            stop,
            worker,
            width: width.clamp(6, 60),
            icons,
        }
    }

    fn latest(&self) -> crate::pulse::Sample {
        self.state.lock().map(|s| s.latest).unwrap_or_default()
    }

    #[cfg(test)]
    fn sample_count(&self) -> usize {
        self.state.lock().map(|s| s.samples.len()).unwrap_or(0)
    }
}

impl Drop for Pulse {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Module for Pulse {
    fn name(&self) -> &'static str {
        "pulse"
    }
    fn side(&self) -> Side {
        Side::Right
    }
    fn render(&mut self) -> String {
        format!(
            "{} {}",
            self.icons.cpu,
            crate::pulse::summary(self.latest())
        )
    }
    fn popup(&mut self) -> Option<(String, Vec<String>)> {
        let sample = self.latest();
        let samples = self
            .state
            .lock()
            .map(|s| s.samples.clone())
            .unwrap_or_default();
        let rows = crate::pulse::graph_rows(&samples, self.width.min(40));
        Some((
            "Load".into(),
            vec![
                format!(
                    "cpu {}%   memory {}%   disk {}%",
                    sample.cpu, sample.memory, sample.disk
                ),
                String::new(),
                rows[0].clone(),
                rows[1].clone(),
                rows[2].clone(),
                String::new(),
                "cpu, memory, fullest filesystem".to_string(),
                "each block covers one second".to_string(),
            ],
        ))
    }
}

/// When the sun rises and sets, from `latitude` and `longitude`.
pub struct Sun {
    pub latitude: f64,
    pub longitude: f64,
    pub icons: status::Icons,
}

impl Module for Sun {
    fn name(&self) -> &'static str {
        "sun"
    }
    fn side(&self) -> Side {
        Side::Right
    }
    fn render(&mut self) -> String {
        // An empty bar is quiet; a missing location is explained in the popup.
        match ambient::sun_times(self.latitude, self.longitude) {
            // The sun carries both times; the arrows say which is which. The
            // sun is the icon users asked for, and pairing it with the times
            // it explains beats a second, unlabelled sun.
            Some((rise, set)) => format!(
                "{} {} {rise} {} {set}",
                self.icons.sun,
                char::from_u32(0x2191).unwrap_or('^'),
                char::from_u32(0x2193).unwrap_or('v')
            ),
            None => String::new(),
        }
    }
    fn popup(&mut self) -> Option<(String, Vec<String>)> {
        let lines = match ambient::sun_times(self.latitude, self.longitude) {
            Some((rise, set)) => vec![
                format!("sunrise {rise}"),
                format!("sunset  {set}"),
                String::new(),
                format!("at {:.1}, {:.1}", self.latitude, self.longitude),
            ],
            None => vec![
                "no sunrise or sunset today".to_string(),
                String::new(),
                "the sun stays up, or stays down, all day".to_string(),
            ],
        };
        Some(("Sun".into(), lines))
    }
}

/// Build the active modules in the order the config lists them.
///
/// An unknown name is reported once and skipped, so a typo in the config does
/// not stop the bar from starting.
pub fn build(full: &config::Config, icons: status::Icons) -> Vec<Box<dyn Module>> {
    let cfg = &full.bar;
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
                sample: Sample::default(),
            })),
            "media" => out.push(Box::new(Media {
                sample: Sample::default(),
                icons: cfg.media_icons(),
                scroller: crate::scroll::Scroller::new(cfg.media_width),
                progress: None,
            })),
            "pulse" => out.push(Box::new(Pulse::new(cfg.graph_width, icons))),
            "uptime" => out.push(Box::new(Uptime { icons })),
            "sun" => out.push(Box::new(Sun {
                latitude: full.weather_latitude,
                longitude: full.weather_longitude,
                icons,
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
                    Some(d) => format!("{d:>3}"),
                    None => "   ".to_string(),
                })
                .collect::<Vec<_>>()
                .join(" "),
        );
    }
    rows
}

#[cfg(test)]
mod tests {
    #[test]
    fn samples_cache_success_and_failure_independently_of_redraws() {
        let mut sample = super::Sample::default();
        let mut calls = 0;
        for _ in 0..100 {
            assert_eq!(
                sample.get(|| {
                    calls += 1;
                    Some(42)
                }),
                Some(&42)
            );
        }
        assert_eq!(calls, 1);
        sample.checked = sample
            .checked
            .map(|t| t - std::time::Duration::from_secs(2));
        assert_eq!(
            sample.get(|| {
                calls += 1;
                None
            }),
            None
        );
        assert_eq!(
            sample.get(|| {
                calls += 1;
                Some(99)
            }),
            None
        );
        assert_eq!(calls, 2);
    }
    use super::*;

    #[test]
    fn every_module_in_the_example_config_is_known() {
        // A typo in the example would teach every user the same mistake.
        let text = include_str!("../config.example.toml");
        let value: toml::Value = text.parse().expect("example config parses");
        let list = value["bar"]["modules"]
            .as_array()
            .expect("bar.modules is a list");
        let built = build(&config::Config::default(), status::Icons::plain());
        for name in list {
            let name = name.as_str().expect("module names are strings");
            assert!(
                matches!(
                    name,
                    "workspaces"
                        | "clock"
                        | "battery"
                        | "network"
                        | "volume"
                        | "media"
                        | "updates"
                        | "pulse"
                        | "uptime"
                        | "sun"
                ),
                "example config names an unknown module {name:?}"
            );
        }
        // The example leaves the new modules out, so they must not be built;
        // and a hand-made config that asks for all of them builds each once.
        assert!(built.len() <= list.len());
        let mut all = config::Config::default();
        all.bar.modules = vec![
            "pulse".into(),
            "uptime".into(),
            "sun".into(),
            "clock".into(),
        ];
        assert_eq!(build(&all, status::Icons::plain()).len(), 4);
    }

    #[test]
    fn the_new_modules_always_open_a_panel() {
        let mut pulse = Pulse::new(10, status::Icons::plain());
        pulse.render();
        pulse.render();
        let (title, lines) = pulse.popup().expect("pulse has a panel");
        assert_eq!(title, "Load");
        assert!(lines.iter().any(|l| l.starts_with("cpu ")));

        let (title, lines) = Uptime {
            icons: status::Icons::plain(),
        }
        .popup()
        .expect("uptime has a panel");
        assert_eq!(title, "Uptime");
        assert!(lines[0].starts_with("up "));

        // A locationless sun module shows whatever the equator has today.
        let mut sun = Sun {
            latitude: 0.0,
            longitude: 0.0,
            icons: status::Icons::plain(),
        };
        let _ = sun.render();
        let (_, lines) = sun.popup().expect("sun has a panel");
        assert!(lines[0].starts_with("sunrise") || lines[0].starts_with("no sunrise"));
    }

    #[test]
    fn calendar_starts_on_monday() {
        // 2026-10-01 is a Thursday, so the first row has three blanks.
        let rows = month_grid(2026, 10);
        assert!(rows[0].starts_with("        "), "row: {:?}", rows[0]);
        assert!(rows[0].contains(" 1"));
    }

    #[test]
    fn calendar_has_full_weeks() {
        // Seven cells per row, one space between them, three characters each:
        // 7 * 3 + 6 = 27 characters, matching the Mon..Sun header.
        for m in 1..=12 {
            for r in month_grid(2026, m) {
                assert_eq!(r.chars().count(), 27, "row {:?} in month {m}", r);
            }
        }
    }

    #[test]
    fn pulse_samples_once_per_second_not_once_per_repaint() {
        let mut pulse = Pulse::new(10, status::Icons::plain());
        let before = pulse.sample_count();
        for _ in 0..50 {
            let _ = pulse.render();
        }
        assert!(
            pulse.sample_count() <= before + 1,
            "fast repaints multiplied /proc and filesystem samples"
        );
    }

    #[test]
    fn workspace_labels_default_to_numbers() {
        let cfg = config::Bar::default();
        let ws = Workspaces::new(&cfg);
        assert_eq!(ws.names[0], "1");
        assert_eq!(ws.names[8], "9");
    }

    #[test]
    fn monitor_workspaces_are_not_hardcoded_to_nine() {
        let cfg = config::Bar::default();
        let mut ws = Workspaces::new(&cfg);
        ws.update(
            3,
            1,
            Some(vec!["web".into(), "code".into()]),
            vec![true, false],
        );
        assert_eq!(ws.offset, 3);
        assert_eq!(ws.active, 1);
        assert_eq!(ws.names, vec!["web", "code"]);
        assert_eq!(ws.occupied, vec![true, false]);
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
        let cfg = config::Config {
            bar: config::Bar {
                modules: vec!["clock".into(), "nonsense".into(), "battery".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        let mods = build(&cfg, status::Icons::plain());
        assert_eq!(mods.len(), 2);
        assert_eq!(mods[0].name(), "clock");
        assert_eq!(mods[1].name(), "battery");
    }
}
