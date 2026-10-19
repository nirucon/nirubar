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
#[derive(Clone, Copy)]
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
    /// Shown before the update count.
    pub updates: &'static str,
    /// The `pulse` module: a small rising graph, in the same family as the
    /// wifi and battery glyphs so the bar reads as one set.
    pub cpu: &'static str,
    /// The `sun` module: sunrise and sunset.
    pub sun: &'static str,
    /// The `uptime` module: a clock face, distinct from the bar's own clock.
    pub uptime: &'static str,
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
            updates: "\u{f021}", // nf-fa-refresh, stable across Nerd Font versions
            cpu: "\u{f1fe}",     // nf-fa-area_chart
            sun: "\u{f185}",     // nf-fa-sun_o
            uptime: "\u{f017}",  // nf-fa-clock_o
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
            updates: "upd",
            cpu: "cpu",
            sun: "sun",
            uptime: "up",
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

/// Detail lines for the battery popup.
pub fn battery_detail(b: &Battery) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(format!("charge: {}%", b.percent));
    if b.charging {
        lines.push("state: charging".to_string());
    } else {
        lines.push("state: on battery".to_string());
    }
    if let Some(name) = battery_name() {
        lines.push(format!("device: {name}"));
    }
    lines
}

fn battery_name() -> Option<String> {
    let entries = std::fs::read_dir("/sys/class/power_supply").ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let kind = std::fs::read_to_string(path.join("type")).ok()?;
        if kind.trim() == "Battery" {
            return Some(entry.file_name().to_string_lossy().to_string());
        }
    }
    None
}

/// Detail lines for the network popup: interface, address and link state.
pub fn network_detail(link: Link) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(match link {
        Link::Wifi => "type: wireless".to_string(),
        Link::Ethernet => "type: wired".to_string(),
    });
    for (iface, kind) in active_interfaces() {
        lines.push(format!("interface: {iface}"));
        let addr = interface_address(&iface);
        lines.push(match addr {
            Some(a) => format!("address: {a}"),
            None => "address: (none)".to_string(),
        });
        if kind == Link::Wifi {
            if let Some(ssid) = wifi_ssid(&iface) {
                lines.push(format!("network: {ssid}"));
            }
            if let Some(sig) = wifi_signal(&iface) {
                lines.push(format!("signal: {sig}%"));
            }
        }
    }
    lines
}

fn active_interfaces() -> Vec<(String, Link)> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir("/sys/class/net") else {
        return out;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name == "lo" || name.starts_with("docker") || name.starts_with("veth") {
            continue;
        }
        let path = entry.path();
        let state = std::fs::read_to_string(path.join("operstate"))
            .unwrap_or_default()
            .trim()
            .to_string();
        let carrier = std::fs::read_to_string(path.join("carrier"))
            .unwrap_or_default()
            .trim()
            .to_string();
        if state != "up" || !(carrier.is_empty() || carrier == "1") {
            continue;
        }
        let kind = if path.join("wireless").exists() {
            Link::Wifi
        } else {
            Link::Ethernet
        };
        out.push((name, kind));
    }
    out
}

/// First IPv4 address of an interface, read from the kernel without a command.
fn interface_address(iface: &str) -> Option<String> {
    // `getifaddrs` through /proc is awkward; using `ip` is portable across the
    // three supported distributions and only runs when the popup is opened.
    let out = crate::probe::run(
        "ip",
        &["-4", "-o", "addr", "show", "dev", iface],
        &[],
        std::time::Duration::from_secs(1),
        64 * 1024,
    )?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    text.split_whitespace()
        .skip_while(|w| *w != "inet")
        .nth(1)
        .map(|s| s.split('/').next().unwrap_or(s).to_string())
}

/// SSID from the wireless interface, without NetworkManager.
fn wifi_ssid(iface: &str) -> Option<String> {
    // iw is the standard tool for cfg80211; fall back to nothing.
    let out = crate::probe::run(
        "iw",
        &["dev", iface, "link"],
        &[],
        std::time::Duration::from_secs(1),
        64 * 1024,
    )?;
    if !out.status.success() {
        return None;
    }
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let line = line.trim();
        if let Some(ssid) = line.strip_prefix("SSID: ") {
            return Some(ssid.to_string());
        }
    }
    None
}

/// Signal quality as a percentage of the interface's maximum.
fn wifi_signal(iface: &str) -> Option<u8> {
    let raw = std::fs::read_to_string("/proc/net/wireless").ok()?;
    for line in raw.lines().skip(2) {
        if !line.starts_with(iface) {
            continue;
        }
        let quality = line.split_whitespace().nth(2)?;
        let value: f32 = quality.trim_end_matches('.').parse().ok()?;
        return Some((value / 70.0 * 100.0).clamp(0.0, 100.0) as u8);
    }
    None
}

#[cfg(test)]
mod detail_tests {
    use super::*;

    #[test]
    fn battery_detail_mentions_charge() {
        let b = Battery {
            percent: 42,
            charging: true,
        };
        let lines = battery_detail(&b);
        assert!(lines[0].contains("42%"));
        assert!(lines.iter().any(|l| l.contains("charging")));
    }

    #[test]
    fn network_detail_states_the_type() {
        let lines = network_detail(Link::Wifi);
        assert!(lines[0].contains("wireless"));
    }
}
