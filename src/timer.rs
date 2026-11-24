//! A focus and egg timer.
//!
//! The state lives in one small file in `$XDG_RUNTIME_DIR`, so the bars on
//! every screen show the same timer, a bar restart keeps it, and
//! `nirubar --timer ...` (a key binding, a script) changes it too. The bar
//! repaints once a second anyway; while the timer is idle its text never
//! changes, so an idle bar stays idle.
//!
//! Settings are in NIRUBAR's own `~/.config/nirubar/config.toml`, not the
//! shared file, so NIRUWM's strict check never sees them:
//!
//! ```toml
//! [timer]
//! minutes = 25    # what a reset timer starts with (1-180)
//! sound = true    # a short sound when it ends
//! dnd = false     # Do Not Disturb while it runs, put back afterwards
//! ```

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const MAX_MINUTES: u32 = 180;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub minutes: u32,
    pub sound: bool,
    pub dnd: bool,
    /// Entries that were ignored, for `nirubar --check-config`.
    pub problems: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            minutes: 25,
            sound: true,
            dnd: false,
            problems: Vec::new(),
        }
    }
}

fn own_config() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        })
        .join("nirubar/config.toml")
}

impl Settings {
    pub fn load() -> Self {
        match std::fs::read_to_string(own_config()) {
            Ok(text) => Self::from_text(&text),
            Err(_) => Self::default(),
        }
    }

    pub fn from_text(text: &str) -> Self {
        let mut out = Self::default();
        let Ok(value) = text.parse::<toml::Value>() else {
            out.problems
                .push("~/.config/nirubar/config.toml is not valid TOML".into());
            return out;
        };
        let Some(table) = value.get("timer").and_then(|t| t.as_table()) else {
            return out;
        };
        for (key, value) in table {
            match (key.as_str(), value) {
                ("minutes", toml::Value::Integer(n)) if (1..=MAX_MINUTES as i64).contains(n) => {
                    out.minutes = *n as u32
                }
                ("sound", toml::Value::Boolean(b)) => out.sound = *b,
                ("dnd", toml::Value::Boolean(b)) => out.dnd = *b,
                ("minutes", _) => out
                    .problems
                    .push(format!("[timer] minutes must be 1 to {MAX_MINUTES}")),
                ("sound" | "dnd", _) => out
                    .problems
                    .push(format!("[timer] {key} must be true or false")),
                _ => out
                    .problems
                    .push(format!("[timer] {key} is not a timer setting")),
            }
        }
        out
    }
}

/// Lines for `nirubar --check-config`.
pub fn check_lines() -> Vec<String> {
    Settings::load()
        .problems
        .into_iter()
        .map(|p| format!("ignored: {p}"))
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Run {
    Idle,
    /// Ends at this Unix time, in seconds.
    Running {
        end: u64,
    },
    /// Paused with this many seconds left.
    Paused {
        left: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub minutes: u32,
    pub run: Run,
    /// Do Not Disturb was off before this timer turned it on.
    pub restore_dnd: bool,
}

impl State {
    fn idle(minutes: u32) -> Self {
        Self {
            minutes,
            run: Run::Idle,
            restore_dnd: false,
        }
    }

    pub fn parse(text: &str, default_minutes: u32) -> Self {
        let field = |key: &str| {
            text.lines()
                .filter_map(|l| l.split_once('='))
                .find_map(|(k, v)| (k == key).then(|| v.trim().to_string()))
        };
        let number = |key: &str| field(key).and_then(|v| v.parse::<u64>().ok());
        let minutes = number("minutes")
            .map(|m| m.clamp(1, u64::from(MAX_MINUTES)) as u32)
            .unwrap_or(default_minutes);
        let run = match field("state").as_deref() {
            Some("running") => number("end").map(|end| Run::Running { end }),
            Some("paused") => number("left").map(|left| Run::Paused { left }),
            _ => None,
        }
        .unwrap_or(Run::Idle);
        Self {
            minutes,
            run,
            restore_dnd: field("restore_dnd").as_deref() == Some("true"),
        }
    }

    pub fn text(&self) -> String {
        let mut out = format!("minutes={}\n", self.minutes);
        match self.run {
            Run::Idle => out.push_str("state=idle\n"),
            Run::Running { end } => out.push_str(&format!("state=running\nend={end}\n")),
            Run::Paused { left } => out.push_str(&format!("state=paused\nleft={left}\n")),
        }
        if self.restore_dnd {
            out.push_str("restore_dnd=true\n");
        }
        out
    }
}

/// The next length up: by 1 below 5 minutes, by 5 up to 60, by 15 above.
pub fn step_up(m: u32) -> u32 {
    let next = if m < 5 {
        m + 1
    } else if m < 60 {
        (m / 5 + 1) * 5
    } else {
        (m / 15 + 1) * 15
    };
    next.min(MAX_MINUTES)
}

pub fn step_down(m: u32) -> u32 {
    let next = if m <= 5 {
        m.saturating_sub(1)
    } else if m <= 60 {
        (m - 1) / 5 * 5
    } else {
        ((m - 1) / 15 * 15).max(60)
    };
    next.max(1)
}

pub fn clock(seconds: u64) -> String {
    let (h, m, s) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

fn state_path() -> PathBuf {
    runtime_dir().join(format!("nirubar-timer-{}", unsafe { libc::getuid() }))
}

/// The timer as it is now; a timer that ran out without any bar to see it is
/// finished on the first read.
pub fn load(settings: &Settings) -> State {
    let state = std::fs::read_to_string(state_path())
        .map(|t| State::parse(&t, settings.minutes))
        .unwrap_or_else(|_| State::idle(settings.minutes));
    if let Run::Running { end } = state.run {
        if now() >= end {
            finish(&state, end, settings);
            return State::idle(state.minutes);
        }
    }
    state
}

fn save(state: &State) {
    let path = state_path();
    let temp = path.with_extension(format!("{}", std::process::id()));
    if std::fs::write(&temp, state.text()).is_ok() {
        let _ = std::fs::rename(&temp, &path);
    }
}

fn spawn(program: &str, args: &[&str]) {
    let _ = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|mut child| {
            // Reap it off the drawing thread.
            std::thread::spawn(move || child.wait());
        });
}

fn on_path(program: &str) -> bool {
    std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .any(|d| !d.is_empty() && std::path::Path::new(d).join(program).is_file())
}

/// Do Not Disturb through `niructl dnd`, like the `dnd` module. Runs on a
/// thread: the provider may take a moment to answer.
fn dnd_on_for(state_minutes: u32) {
    std::thread::spawn(move || {
        let Ok(out) = std::process::Command::new("niructl")
            .args(["dnd", "status"])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
        else {
            return;
        };
        let was_off = String::from_utf8_lossy(&out.stdout).trim() == "dnd off";
        if !was_off {
            return;
        }
        let _ = std::process::Command::new("niructl")
            .args(["dnd", "on"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        // Remember to put it back, unless the timer stopped meanwhile.
        let mut state = State::parse(
            &std::fs::read_to_string(state_path()).unwrap_or_default(),
            state_minutes,
        );
        if state.run == Run::Idle {
            spawn("niructl", &["dnd", "off"]);
        } else {
            state.restore_dnd = true;
            save(&state);
        }
    });
}

fn restore_dnd(state: &State) {
    if state.restore_dnd {
        spawn("niructl", &["dnd", "off"]);
    }
}

/// The end of a run. Every bar sees the end; only the first one to claim
/// this end time tells you.
fn finish(state: &State, end: u64, settings: &Settings) {
    let prefix = format!("nirubar-timer-{}-done-", unsafe { libc::getuid() });
    let dir = runtime_dir();
    // Creating the marker is atomic: one bar wins, the others return.
    if std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join(format!("{prefix}{end}")))
        .is_err()
    {
        return;
    }
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(&prefix) && name != format!("{prefix}{end}") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    save(&State::idle(state.minutes));
    restore_dnd(state);
    if cfg!(test) {
        return;
    }
    let body = match state.minutes {
        1 => "1 minute is up".to_string(),
        m => format!("{m} minutes are up"),
    };
    // The notification stays silent; the timer has its own sound below, so a
    // finished timer is one sound, not the notification sound and another.
    spawn(
        "notify-send",
        &[
            "-a",
            "NIRUX",
            "-u",
            "critical",
            "-i",
            "alarm-symbolic",
            "-h",
            crate::helpers::SUPPRESS_SOUND,
            "Timer",
            &body,
        ],
    );
    // `timer-done` in NIRUX's sound settings (category notifications). A bar
    // without NIRUX plays the freedesktop alarm sound instead.
    if settings.sound && !crate::helpers::play_nirux_sound("timer-done") {
        let sound = [
            "/usr/share/sounds/freedesktop/stereo/alarm-clock-elapsed.oga",
            "/usr/share/sounds/freedesktop/stereo/complete.oga",
        ]
        .into_iter()
        .find(|p| std::path::Path::new(p).is_file());
        let player = ["pw-play", "paplay"].into_iter().find(|p| on_path(p));
        if let (Some(sound), Some(player)) = (sound, player) {
            spawn(player, &[sound]);
        }
    }
}

/// What a control does to the timer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Start, pause or resume.
    Toggle,
    Start,
    Pause,
    Reset,
    Longer,
    Shorter,
    Set(u32),
}

/// `state` after `action` at `now`. Pure, for the tests.
pub fn apply(state: &State, action: Action, now: u64) -> State {
    let mut s = state.clone();
    let start = |s: &mut State| {
        s.run = Run::Running {
            end: now + u64::from(s.minutes) * 60,
        }
    };
    match (action, state.run) {
        (Action::Toggle | Action::Start, Run::Idle) => start(&mut s),
        (Action::Toggle | Action::Start, Run::Paused { left }) => {
            s.run = Run::Running { end: now + left }
        }
        (Action::Toggle | Action::Pause, Run::Running { end }) => {
            s.run = Run::Paused {
                left: end.saturating_sub(now),
            }
        }
        (Action::Start | Action::Pause, _) => {}
        (Action::Reset, _) => {
            s.run = Run::Idle;
            s.restore_dnd = false;
        }
        (Action::Set(m), _) => s.minutes = m.clamp(1, MAX_MINUTES),
        // Idle: choose the length. Running or paused: a minute more or less.
        (Action::Longer, Run::Idle) => s.minutes = step_up(s.minutes),
        (Action::Shorter, Run::Idle) => s.minutes = step_down(s.minutes),
        (Action::Longer, Run::Running { end }) => s.run = Run::Running { end: end + 60 },
        (Action::Shorter, Run::Running { end }) => {
            s.run = Run::Running {
                end: end.saturating_sub(60).max(now + 1),
            }
        }
        (Action::Longer, Run::Paused { left }) => s.run = Run::Paused { left: left + 60 },
        (Action::Shorter, Run::Paused { left }) => {
            s.run = Run::Paused {
                left: left.saturating_sub(60).max(1),
            }
        }
    }
    s
}

/// Change the timer and keep the result.
pub fn act(action: Action, settings: &Settings) -> State {
    let before = load(settings);
    let after = apply(&before, action, now());
    save(&after);
    let started = before.run == Run::Idle && matches!(after.run, Run::Running { .. });
    if started && settings.dnd {
        dnd_on_for(after.minutes);
    }
    if before.run != Run::Idle && after.run == Run::Idle {
        restore_dnd(&before);
    }
    after
}

/// The bar module's view of the timer.
pub struct Timer {
    pub settings: Settings,
    /// After a scroll the chosen length shows for a moment.
    pub adjusted: Option<std::time::Instant>,
}

impl Timer {
    pub fn new() -> Self {
        Self {
            settings: Settings::load(),
            adjusted: None,
        }
    }

    pub fn text(&self, icon: &str, pause: &str) -> String {
        let state = load(&self.settings);
        match state.run {
            Run::Idle
                if self
                    .adjusted
                    .is_some_and(|t| t.elapsed() < Duration::from_secs(3)) =>
            {
                format!("{icon} {} min", state.minutes)
            }
            Run::Idle => icon.to_string(),
            Run::Running { end } => format!("{icon} {}", clock(end.saturating_sub(now()))),
            Run::Paused { left } => format!("{icon} {pause} {}", clock(left)),
        }
    }

    pub fn running(&self) -> bool {
        matches!(load(&self.settings).run, Run::Running { .. })
    }

    pub fn detail(&self) -> Vec<String> {
        let state = load(&self.settings);
        let mut lines = vec![match state.run {
            Run::Idle => format!("ready: {} minutes", state.minutes),
            Run::Running { end } => format!("running: {} left", clock(end.saturating_sub(now()))),
            Run::Paused { left } => format!("paused: {} left", clock(left)),
        }];
        if self.settings.dnd {
            lines.push("Do Not Disturb while it runs".into());
        }
        lines.extend([
            "type minutes, Return starts".into(),
            "Space pauses or resumes".into(),
        ]);
        lines
    }
}

/// `nirubar --timer [status|start|pause|toggle|reset|set MINUTES]`.
pub fn cli(args: &[String]) -> i32 {
    let settings = Settings::load();
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    let action = match words.as_slice() {
        [] | ["status"] => None,
        ["start"] => Some(Action::Start),
        // `start N`: that many minutes, from now (a running timer starts over).
        ["start", m] => match m.parse::<u32>() {
            Ok(m) if (1..=MAX_MINUTES).contains(&m) => {
                act(Action::Reset, &settings);
                act(Action::Set(m), &settings);
                Some(Action::Start)
            }
            _ => {
                eprintln!("nirubar: minutes must be 1 to {MAX_MINUTES}");
                return 2;
            }
        },
        // `add N` / `add -N`: minutes more or less on a running or paused timer.
        ["add", m] => match m.parse::<i32>() {
            Ok(m) if m != 0 && m.unsigned_abs() <= MAX_MINUTES => {
                let step = if m > 0 {
                    Action::Longer
                } else {
                    Action::Shorter
                };
                for _ in 1..m.unsigned_abs() {
                    act(step, &settings);
                }
                Some(step)
            }
            _ => {
                eprintln!("nirubar: add needs a number of minutes");
                return 2;
            }
        },
        ["pause"] => Some(Action::Pause),
        ["toggle"] => Some(Action::Toggle),
        ["reset"] => Some(Action::Reset),
        ["set", m] => match m.parse::<u32>() {
            Ok(m) if (1..=MAX_MINUTES).contains(&m) => Some(Action::Set(m)),
            _ => {
                eprintln!("nirubar: minutes must be 1 to {MAX_MINUTES}");
                return 2;
            }
        },
        _ => {
            eprintln!(
                "Usage: nirubar --timer [status|start [MINUTES]|pause|toggle|reset|set MINUTES|add [-]MINUTES]"
            );
            return 2;
        }
    };
    let state = match action {
        Some(a) => act(a, &settings),
        None => load(&settings),
    };
    match state.run {
        Run::Idle => println!("idle {} min", state.minutes),
        Run::Running { end } => println!("running {}", clock(end.saturating_sub(now()))),
        Run::Paused { left } => println!("paused {}", clock(left)),
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrolling_steps_by_one_five_and_fifteen() {
        let mut m = 1;
        let mut seen = vec![m];
        while m < MAX_MINUTES {
            m = step_up(m);
            seen.push(m);
        }
        assert_eq!(&seen[..6], &[1, 2, 3, 4, 5, 10]);
        assert!(seen.contains(&55) && seen.contains(&60) && seen.contains(&75));
        assert_eq!(*seen.last().unwrap(), 180);
        assert_eq!(step_up(180), 180);
        assert_eq!(step_up(7), 10);
        assert_eq!(step_down(75), 60);
        assert_eq!(step_down(60), 55);
        assert_eq!(step_down(7), 5);
        assert_eq!(step_down(5), 4);
        assert_eq!(step_down(1), 1);
        for m in 1..=MAX_MINUTES {
            assert!(step_down(step_up(m)) <= m, "{m}");
        }
    }

    #[test]
    fn start_pause_resume_and_reset() {
        let s = State::idle(25);
        let s = apply(&s, Action::Toggle, 1000);
        assert_eq!(s.run, Run::Running { end: 1000 + 1500 });
        let s = apply(&s, Action::Toggle, 1100);
        assert_eq!(s.run, Run::Paused { left: 1400 });
        let s = apply(&s, Action::Longer, 1200);
        assert_eq!(s.run, Run::Paused { left: 1460 });
        let s = apply(&s, Action::Toggle, 2000);
        assert_eq!(s.run, Run::Running { end: 3460 });
        let s = apply(&s, Action::Shorter, 3450);
        assert_eq!(s.run, Run::Running { end: 3451 });
        let s = apply(&s, Action::Reset, 3450);
        assert_eq!(s.run, Run::Idle);
        assert_eq!(apply(&s, Action::Longer, 0).minutes, 30);
        assert_eq!(apply(&s, Action::Set(500), 0).minutes, MAX_MINUTES);
    }

    #[test]
    fn the_state_file_round_trips_and_survives_damage() {
        let s = State {
            minutes: 40,
            run: Run::Paused { left: 99 },
            restore_dnd: true,
        };
        assert_eq!(State::parse(&s.text(), 25), s);
        assert_eq!(State::parse("garbage", 25), State::idle(25));
        assert_eq!(
            State::parse("minutes=999\nstate=running\n", 25),
            State::idle(180)
        );
        assert_eq!(clock(65), "01:05");
        assert_eq!(clock(3725), "1:02:05");
    }

    #[test]
    fn settings_are_checked() {
        let s = Settings::from_text("[timer]\nminutes = 50\nsound = false\ndnd = true\n");
        assert_eq!((s.minutes, s.sound, s.dnd), (50, false, true));
        assert!(s.problems.is_empty());
        let s = Settings::from_text("[timer]\nminutes = 0\ncolour = 1\n");
        assert_eq!(s.minutes, 25);
        assert_eq!(s.problems.len(), 2);
        assert_eq!(
            Settings::from_text("[popup_timeout]\nclock = 0\n"),
            Settings::default()
        );
    }
}
