//! How long each panel stays open.
//!
//! `[bar] popup_timeout_seconds` in the shared NIRUWM file is the timeout for
//! quick-glance panels (volume, network, battery, ...). Panels people read,
//! such as the calendar or the rain timeline, stay open until dismissed. Any
//! panel can be given its own timeout in NIRUBAR's own file:
//!
//! ```toml
//! # ~/.config/nirubar/config.toml
//! [popup_timeout]
//! calendar = 0   # or clock: the clock's month view
//! volume = 2
//! ```
//!
//! The overrides live in `~/.config/nirubar/config.toml`, not in the shared
//! file, so NIRUWM's strict config check never sees them.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// Panels that are read rather than glanced at. They stay open until a click
/// or Escape closes them, unless `[popup_timeout]` says otherwise.
pub const READING_PANELS: &[&str] = &["clock", "weather", "tasks", "pulse", "notifications"];

/// Names `[popup_timeout]` accepts besides the `[bar] modules` names.
/// `calendar` is the clock's panel; `tray` is a tray application's menu and
/// `overflow` the list of modules that did not fit.
const EXTRA_NAMES: &[&str] = &["calendar", "tray", "overflow"];

/// Every module name a panel can belong to.
const MODULES: &[&str] = &[
    "clock",
    "battery",
    "display",
    "mode",
    "tasks",
    "layout",
    "quicksettings",
    "nirusec",
    "agent",
    "share",
    "tailscale",
    "network",
    "wifi",
    "updates",
    "niruxupdate",
    "disk",
    "volume",
    "powerprofile",
    "bluetooth",
    "privacy",
    "recording",
    "scratchpad",
    "dictation",
    "microphone",
    "media",
    "pulse",
    "uptime",
    "brightness",
    "weather",
    "notifications",
    "dnd",
    "sun",
];

/// The longest timeout an override may ask for (one day).
const MAX_SECONDS: i64 = 86_400;

/// Per-panel overrides from `[popup_timeout]`, keyed by module name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Overrides {
    seconds: BTreeMap<String, u32>,
    /// Entries that were ignored, for `nirubar --check-config`.
    pub problems: Vec<String>,
}

impl Overrides {
    /// Read `[popup_timeout]` from `~/.config/nirubar/config.toml`. A missing
    /// file or table means no overrides; a bad entry is skipped and reported.
    pub fn load() -> Self {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
            });
        let Ok(text) = std::fs::read_to_string(base.join("nirubar/config.toml")) else {
            return Self::default();
        };
        match text.parse::<toml::Value>() {
            Ok(value) => Self::from_value(&value),
            // The other sections of this file report their own errors; a
            // broken file simply has no overrides.
            Err(_) => Self::default(),
        }
    }

    /// Overrides from a parsed `~/.config/nirubar/config.toml`.
    pub fn from_value(value: &toml::Value) -> Self {
        let mut out = Self::default();
        let Some(entry) = value.get("popup_timeout") else {
            return out;
        };
        let Some(table) = entry.as_table() else {
            out.problems
                .push("[popup_timeout] must be a table of module = seconds".into());
            return out;
        };
        for (name, seconds) in table {
            if !MODULES.contains(&name.as_str()) && !EXTRA_NAMES.contains(&name.as_str()) {
                out.problems
                    .push(format!("[popup_timeout] {name}: unknown module, ignored"));
                continue;
            }
            match seconds.as_integer() {
                Some(n) if (0..=MAX_SECONDS).contains(&n) => {
                    // `calendar` is another name for the clock's panel.
                    let key = if name == "calendar" { "clock" } else { name };
                    out.seconds.insert(key.to_string(), n as u32);
                }
                _ => out.problems.push(format!(
                    "[popup_timeout] {name}: must be whole seconds from 0 to {MAX_SECONDS}, ignored"
                )),
            }
        }
        out
    }

    /// The configured overrides, for `--check-config`.
    pub fn entries(&self) -> impl Iterator<Item = (&str, u32)> {
        self.seconds.iter().map(|(k, v)| (k.as_str(), *v))
    }

    /// Seconds before the panel of `module` closes itself; 0 keeps it open
    /// until dismissed. An override wins, then the reading-panel default,
    /// then `[bar] popup_timeout_seconds`.
    pub fn resolve(&self, module: &str, global: u32) -> u32 {
        if let Some(seconds) = self.seconds.get(module) {
            return *seconds;
        }
        if READING_PANELS.contains(&module) {
            return 0;
        }
        global
    }
}

/// Lines for `nirubar --check-config`.
pub fn check_lines(global: u32) -> Vec<String> {
    let overrides = Overrides::load();
    let describe = |s: u32| {
        if s == 0 {
            "until dismissed".to_string()
        } else {
            format!("{s} s")
        }
    };
    let mut out = vec![format!(
        "Panels: {} for quick panels; until dismissed for {}",
        describe(global),
        READING_PANELS.join(", ")
    )];
    let entries: Vec<String> = overrides
        .entries()
        .map(|(name, s)| format!("{name} {}", describe(s)))
        .collect();
    if !entries.is_empty() {
        out.push(format!("Panels: overrides {}", entries.join(", ")));
    }
    for problem in &overrides.problems {
        out.push(format!("Panels: WARNING {problem}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Overrides {
        Overrides::from_value(&text.parse::<toml::Value>().unwrap())
    }

    #[test]
    fn defaults_keep_reading_panels_open_and_use_the_global_timeout_elsewhere() {
        let none = Overrides::default();
        for reading in ["clock", "weather", "tasks", "pulse", "notifications"] {
            assert_eq!(none.resolve(reading, 4), 0, "{reading}");
            assert_eq!(none.resolve(reading, 9), 0, "{reading}");
        }
        for quick in [
            "volume",
            "microphone",
            "network",
            "wifi",
            "battery",
            "bluetooth",
            "media",
            "powerprofile",
            "brightness",
            "updates",
            "uptime",
            "display",
            "dnd",
            "sun",
            "tray",
            "overflow",
        ] {
            assert_eq!(none.resolve(quick, 4), 4, "{quick}");
            assert_eq!(none.resolve(quick, 0), 0, "{quick}");
            assert_eq!(none.resolve(quick, 12), 12, "{quick}");
        }
    }

    #[test]
    fn an_override_beats_both_defaults() {
        let o = parse("[popup_timeout]\ncalendar = 6\nvolume = 2\nbattery = 0\nweather = 30\n");
        assert!(o.problems.is_empty(), "{:?}", o.problems);
        assert_eq!(o.resolve("clock", 4), 6, "calendar names the clock panel");
        assert_eq!(o.resolve("volume", 4), 2);
        assert_eq!(o.resolve("battery", 4), 0);
        assert_eq!(o.resolve("weather", 4), 30);
        assert_eq!(o.resolve("tasks", 4), 0, "untouched reading panel");
        assert_eq!(o.resolve("network", 4), 4, "untouched quick panel");
        let clock = parse("[popup_timeout]\nclock = 8\n");
        assert_eq!(clock.resolve("clock", 4), 8);
    }

    #[test]
    fn bad_entries_are_ignored_and_reported() {
        let o = parse(
            "[popup_timeout]\nvolum = 2\nvolume = -1\nmedia = \"5\"\nbattery = 100000\nwifi = 3\n",
        );
        assert_eq!(o.problems.len(), 4, "{:?}", o.problems);
        assert!(o.problems[0].contains("battery"));
        assert_eq!(o.resolve("volume", 4), 4);
        assert_eq!(o.resolve("media", 4), 4);
        assert_eq!(o.resolve("battery", 4), 4);
        assert_eq!(o.resolve("wifi", 4), 3);
        let not_table = parse("popup_timeout = 3\n");
        assert_eq!(not_table.problems.len(), 1);
        assert_eq!(not_table.resolve("volume", 4), 4);
    }

    #[test]
    fn a_file_without_the_table_changes_nothing() {
        let o = parse("[calendar]\ndays_ahead = 3\n[weather]\nlocation = \"Umeå\"\n");
        assert_eq!(o, Overrides::default());
    }

    #[test]
    fn every_module_the_bar_builds_is_a_known_name() {
        // Keep MODULES in step with module::build.
        let source = include_str!("module.rs");
        let start = source.find("pub fn build(").unwrap();
        let body = &source[start..start + source[start..].find("\n}\n").unwrap()];
        for line in body.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix('"') {
                if let Some((name, tail)) = rest.split_once('"') {
                    if tail.trim_start().starts_with("=>") && name != "workspaces" {
                        assert!(MODULES.contains(&name), "{name} missing from MODULES");
                    }
                }
            }
        }
    }
}
