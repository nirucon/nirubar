//! System status for the bar: battery, network, and the clock.
//!
//! Everything is read from the kernel (`/sys`, `/proc`) so the bar needs no
//! daemon, no D-Bus and no NetworkManager. Each value is cheap enough to read
//! once per redraw; the bar already redraws at most once per second and only
//! when the rendered text changes.

use std::fs;
use std::path::Path;

/// Nerd Font glyphs with plain ASCII fallbacks.
///
/// The fallback matters on a clean install where the preferred font is
/// missing: a missing glyph renders as a blank box, which is worse than text.
pub struct Icons {
    pub battery_full: &'static str,
    pub battery_high: &'static str,
    pub battery_medium: &'static str,
    pub battery_low: &'static str,
    pub battery_empty: &'static str,
    pub battery_charging: &'static str,
    pub wifi: &'static str,
    pub ethernet: &'static str,
    pub wifi_fallback: &'static str,
    pub ethernet_fallback: &'static str,
}

impl Icons {
    /// Nerd Font glyphs.
    ///
    /// All of these live in the Font Awesome block that every Nerd Font
    /// generation ships. Nerd Font 3 moved some Material Design icons (such as
    /// `\u{f796}` for ethernet) into a range older builds lack, so only the
    /// stable block is used here.
    pub fn nerd() -> Self {
        Self {
            battery_full: "\u{f240}",     // nf-fa-battery_full
            battery_high: "\u{f241}",     // battery three quarters
            battery_medium: "\u{f242}",   // battery half
            battery_low: "\u{f243}",      // battery quarter
            battery_empty: "\u{f244}",    // battery empty
            battery_charging: "\u{f0e7}", // bolt
            wifi: "\u{f1eb}",             // nf-fa-wifi
            ethernet: "\u{f0ac}",         // nf-fa-globe (stable across versions)
            wifi_fallback: "wifi",
            ethernet_fallback: "net",
        }
    }

    /// Plain text for terminals or fonts without the glyph range.
    pub fn plain() -> Self {
        Self {
            battery_full: "[##]",
            battery_high: "[#-]",
            battery_medium: "[-#]",
            battery_low: "[- ]",
            battery_empty: "[  ]",
            battery_charging: "+",
            wifi: "wifi",
            ethernet: "net",
            wifi_fallback: "wifi",
            ethernet_fallback: "net",
        }
    }
}

/// Battery reading, or `None` on a machine without a battery (a desktop).
#[derive(Clone, Debug, PartialEq)]
pub struct Battery {
    pub percent: u8,
    pub charging: bool,
}

/// Find the first battery under `/sys/class/power_supply`.
///
/// Returns `None` when there is none, which is the desktop case: the bar then
/// draws nothing at all rather than a misleading empty battery.
pub fn battery() -> Option<Battery> {
    battery_in(Path::new("/sys/class/power_supply"))
}

fn battery_in(dir: &Path) -> Option<Battery> {
    let entries = fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let kind = fs::read_to_string(path.join("type")).ok()?;
        if kind.trim() != "Battery" {
            continue;
        }
        // A battery with no capacity reading (some drivers during boot) is
        // treated as absent rather than shown as 0%.
        let percent = fs::read_to_string(path.join("capacity"))
            .ok()
            .and_then(|s| s.trim().parse::<u8>().ok())?;
        let status = fs::read_to_string(path.join("status"))
            .unwrap_or_default()
            .trim()
            .to_lowercase();
        let charging = matches!(status.as_str(), "charging" | "full");
        return Some(Battery {
            percent: percent.min(100),
            charging,
        });
    }
    None
}

/// Network state: `Wifi`, `Ethernet` or `None`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Link {
    Wifi,
    Ethernet,
}

/// Return the best active link, preferring a wired connection.
///
/// Uses `/sys/class/net`: `operstate == "up"` and a carrier of 1 means the
/// interface is connected. Wi-Fi is recognised by the presence of a
/// `wireless` directory. Loopback and virtual bridges are ignored so a
/// docker0 interface does not look like a network connection.
pub fn network() -> Option<Link> {
    let entries = fs::read_dir("/sys/class/net").ok()?;
    let mut wired = false;
    let mut wireless = false;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name == "lo" || name.starts_with("docker") || name.starts_with("veth") {
            continue;
        }
        let path = entry.path();
        let state = fs::read_to_string(path.join("operstate"))
            .unwrap_or_default()
            .trim()
            .to_string();
        let carrier = fs::read_to_string(path.join("carrier"))
            .unwrap_or_default()
            .trim()
            .to_string();
        let up = state == "up" && (carrier.is_empty() || carrier == "1");
        if !up {
            continue;
        }
        if path.join("wireless").exists() {
            wireless = true;
        } else {
            wired = true;
        }
    }
    if wired {
        Some(Link::Ethernet)
    } else if wireless {
        Some(Link::Wifi)
    } else {
        None
    }
}

/// The clock text: ISO date and `hh:mm`, as used by NIRUWM.
///
/// The date is deliberately shown without a weekday: NIRUWM asked for
/// `2026-10-17  21:16`.
pub fn clock(now: chrono::DateTime<chrono::Local>) -> String {
    format!("{}  {}", now.format("%Y-%m-%d"), now.format("%H:%M"))
}

/// Format the battery segment, e.g. ` 87%` or `+ 42%` while charging.
pub fn battery_text(b: &Battery, icons: &Icons) -> String {
    let icon = if b.charging {
        icons.battery_charging
    } else {
        match b.percent {
            90..=100 => icons.battery_full,
            65..=89 => icons.battery_high,
            35..=64 => icons.battery_medium,
            10..=34 => icons.battery_low,
            _ => icons.battery_empty,
        }
    };
    format!("{icon} {}%", b.percent)
}

/// Format the network segment, or an empty string when offline.
pub fn network_text(link: Option<Link>, icons: &Icons, use_icons: bool) -> String {
    match link {
        Some(Link::Wifi) => if use_icons {
            icons.wifi
        } else {
            icons.wifi_fallback
        }
        .to_string(),
        Some(Link::Ethernet) => if use_icons {
            icons.ethernet
        } else {
            icons.ethernet_fallback
        }
        .to_string(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmpdir(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("nirubar-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn no_battery_on_a_desktop() {
        let dir = tmpdir("nobat");
        assert_eq!(battery_in(&dir), None);
    }

    #[test]
    fn reads_battery_capacity_and_status() {
        let dir = tmpdir("bat");
        let bat = dir.join("BAT0");
        fs::create_dir_all(&bat).unwrap();
        fs::write(bat.join("type"), "Battery\n").unwrap();
        fs::write(bat.join("capacity"), "73\n").unwrap();
        fs::write(bat.join("status"), "Discharging\n").unwrap();
        assert_eq!(
            battery_in(&dir),
            Some(Battery {
                percent: 73,
                charging: false
            })
        );
        fs::write(bat.join("status"), "Charging\n").unwrap();
        assert!(battery_in(&dir).unwrap().charging);
    }

    #[test]
    fn ignores_mains_and_unreadable_batteries() {
        let dir = tmpdir("mains");
        let ac = dir.join("AC");
        fs::create_dir_all(&ac).unwrap();
        fs::write(ac.join("type"), "Mains\n").unwrap();
        fs::write(ac.join("capacity"), "100\n").unwrap();
        assert_eq!(battery_in(&dir), None);
    }

    #[test]
    fn clock_has_no_weekday() {
        let now = chrono::Local::now();
        let text = clock(now);
        assert_eq!(text.len(), 17, "YYYY-MM-DD + two spaces + HH:MM");
        assert!(!text.contains("lör"));
        assert!(text.contains(':'));
    }

    #[test]
    fn battery_icons_track_level() {
        let icons = Icons::nerd();
        let at = |p: u8, c: bool| {
            battery_text(
                &Battery {
                    percent: p,
                    charging: c,
                },
                &icons,
            )
        };
        assert!(at(100, false).starts_with(icons.battery_full));
        assert!(at(70, false).starts_with(icons.battery_high));
        assert!(at(50, false).starts_with(icons.battery_medium));
        assert!(at(20, false).starts_with(icons.battery_low));
        assert!(at(5, false).starts_with(icons.battery_empty));
        assert!(at(50, true).starts_with(icons.battery_charging));
        assert!(at(87, false).ends_with("87%"));
    }

    #[test]
    fn offline_renders_nothing() {
        let icons = Icons::plain();
        assert_eq!(network_text(None, &icons, true), "");
    }

    #[test]
    fn plain_fallbacks_are_not_glyphs() {
        let icons = Icons::plain();
        assert!(icons.wifi.is_ascii());
        assert!(icons.ethernet.is_ascii());
        assert!(icons.battery_full.is_ascii());
    }
}
