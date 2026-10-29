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
    /// The `pulse` module: a microchip, matching the wifi and battery glyph
    /// family. A tiny area chart read as a coffee cup at bar size.
    pub cpu: &'static str,
    /// The `sun` module: sunrise and sunset.
    pub sun: &'static str,
    /// The `uptime` module: a clock face, distinct from the bar's own clock.
    pub uptime: &'static str,
    /// The `brightness` module.
    pub brightness: &'static str,
    /// The optional display-count module.
    pub display: &'static str,
    /// The work-mode countdown module.
    pub mode: &'static str,
    /// The current NIRUWM layout module.
    pub layout: &'static str,
    /// The optional microphone module.
    pub microphone: &'static str,
    /// The optional Tailscale module.
    pub tailscale: &'static str,
    /// The optional Bluetooth module.
    pub bluetooth: &'static str,
    /// The optional power-profile module.
    pub powerprofile: &'static str,
    /// The optional unread-indicator module.
    pub notifications: &'static str,
    /// The media module's music note.
    pub music: &'static str,
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
            cpu: "\u{f2db}",     // nf-fa-microchip
            sun: "\u{f185}",     // nf-fa-sun_o
            uptime: "\u{f017}",  // nf-fa-clock_o
            brightness: "\u{f185}", // sun; brightness is a display property
            display: "\u{f108}", // nf-fa-desktop
            mode: "\u{f017}",    // clock face
            layout: "\u{f009}",  // nf-fa-th-large
            microphone: "\u{f130}", // nf-fa-microphone
            bluetooth: "\u{f293}", // nf-fa-bluetooth
            tailscale: "\u{f0582}", // nf-md-vpn
            powerprofile: "\u{f0e7}", // bolt
            notifications: "\u{f0f3}", // nf-fa-bell
            music: "\u{f001}",   // nf-fa-music
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
            brightness: "bri",
            display: "disp",
            mode: "mode",
            layout: "layout",
            microphone: "mic",
            bluetooth: "bt",
            tailscale: "ts",
            powerprofile: "pwr",
            notifications: "ntf",
            music: "mus",
        }
    }
}

/// Backlight percentage from `/sys/class/backlight`, if a device exists.
pub fn backlight_percent() -> Option<u32> {
    backlight_in(Path::new("/sys/class/backlight"))
}

fn backlight_in(dir: &Path) -> Option<u32> {
    let mut devices: Vec<_> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .collect();
    devices.sort();
    for device in devices {
        let Some(actual) = std::fs::read_to_string(device.join("brightness"))
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
        else {
            continue;
        };
        let Some(max) = std::fs::read_to_string(device.join("max_brightness"))
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
        else {
            continue;
        };
        if max == 0 {
            continue;
        }
        return Some(((actual.saturating_mul(100) + max / 2) / max).min(100) as u32);
    }
    None
}

/// Aggregate state of every battery in the machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BatteryState {
    Charging,
    Discharging,
    Full,
    /// One pack charges while another discharges.
    Mixed,
    Unknown,
}

/// Battery reading, or `None` on a machine without a battery (a desktop).
#[derive(Clone, Debug, PartialEq)]
pub struct Battery {
    pub percent: u8,
    pub charging: bool,
    pub state: BatteryState,
    /// Number of battery packs aggregated into this reading.
    pub count: usize,
    /// Time to empty or full, when the kernel exposes a power/current rate.
    /// `None` is deliberate: an absent estimate must not look like "0 min".
    pub remaining_minutes: Option<u64>,
}

struct BatteryReading {
    percent: Option<u8>,
    status: String,
    energy: Option<(f64, f64)>,
    charge: Option<(f64, f64)>,
    power: Option<f64>,
    current: Option<f64>,
}

fn read_number(path: &Path) -> Option<f64> {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse::<f64>().ok())
}

fn battery_reading(path: &Path) -> Option<BatteryReading> {
    let status = fs::read_to_string(path.join("status"))
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    let percent = fs::read_to_string(path.join("capacity"))
        .ok()
        .and_then(|s| s.trim().parse::<u8>().ok())
        .map(|p| p.min(100));
    let energy = read_number(&path.join("energy_now")).zip(read_number(&path.join("energy_full")));
    let charge = read_number(&path.join("charge_now")).zip(read_number(&path.join("charge_full")));
    if percent.is_none() && energy.is_none() && charge.is_none() {
        // During boot some drivers expose a battery node with no reading yet.
        return None;
    }
    Some(BatteryReading {
        percent,
        status,
        energy,
        charge,
        power: read_number(&path.join("power_now")),
        current: read_number(&path.join("current_now")),
    })
}

/// Find and aggregate every battery under `/sys/class/power_supply`.
///
/// A machine without batteries returns `None`, so the bar draws nothing rather
/// than a misleading empty battery.
pub fn battery() -> Option<Battery> {
    battery_in(Path::new("/sys/class/power_supply"))
}

fn battery_in(dir: &Path) -> Option<Battery> {
    let entries = fs::read_dir(dir).ok()?;
    let mut readings = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let kind = fs::read_to_string(path.join("type")).unwrap_or_default();
        if kind.trim() != "Battery" {
            continue;
        }
        if let Some(reading) = battery_reading(&path) {
            readings.push(reading);
        }
    }
    if readings.is_empty() {
        return None;
    }
    let count = readings.len();
    let weighted = |pairs: &[(f64, f64)]| -> Option<u8> {
        let now: f64 = pairs.iter().map(|(now, _)| *now).sum();
        let full: f64 = pairs.iter().map(|(_, full)| *full).sum();
        (full > 0.0).then(|| (now / full * 100.0).round().clamp(0.0, 100.0) as u8)
    };
    let energy: Vec<(f64, f64)> = readings.iter().filter_map(|r| r.energy).collect();
    let charge: Vec<(f64, f64)> = readings.iter().filter_map(|r| r.charge).collect();
    let percent = if energy.len() == count {
        weighted(&energy)
    } else if charge.len() == count {
        weighted(&charge)
    } else {
        let values: Vec<u8> = readings.iter().filter_map(|r| r.percent).collect();
        (!values.is_empty()).then(|| {
            (values.iter().map(|p| u32::from(*p)).sum::<u32>() / values.len() as u32) as u8
        })
    }?;
    let charging = readings.iter().any(|r| r.status == "charging");
    let discharging = readings.iter().any(|r| r.status == "discharging");
    let full = readings
        .iter()
        .all(|r| matches!(r.status.as_str(), "full" | "not charging"));
    let state = match (charging, discharging) {
        (true, true) => BatteryState::Mixed,
        (true, false) => BatteryState::Charging,
        (false, true) => BatteryState::Discharging,
        (false, false) if full => BatteryState::Full,
        _ => BatteryState::Unknown,
    };
    let energy_now: f64 = energy.iter().map(|(now, _)| *now).sum();
    let energy_full: f64 = energy.iter().map(|(_, full)| *full).sum();
    let power: f64 = readings.iter().filter_map(|r| r.power).sum();
    let charge_now: f64 = charge.iter().map(|(now, _)| *now).sum();
    let current: f64 = readings.iter().filter_map(|r| r.current).sum();
    let remaining_minutes = match state {
        BatteryState::Discharging if power > 0.0 && energy_now > 0.0 => {
            Some((energy_now / power * 60.0).round() as u64)
        }
        BatteryState::Discharging if current > 0.0 && charge_now > 0.0 => {
            Some((charge_now / current * 60.0).round() as u64)
        }
        BatteryState::Charging if power > 0.0 && energy_full > energy_now => {
            Some(((energy_full - energy_now) / power * 60.0).round() as u64)
        }
        _ => None,
    };
    Some(Battery {
        percent,
        charging: matches!(state, BatteryState::Charging | BatteryState::Full),
        state,
        count,
        remaining_minutes,
    })
}

/// Network state: `Wifi`, `Ethernet` or `None`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Link {
    Wifi,
    Ethernet,
}

/// Link, address and route are separate facts. A cable can be plugged in while
/// DHCP has not assigned an address, or an address can exist without a default
/// route; the bar must not call either of those "online".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NetworkState {
    pub link: Link,
    /// A default route exists in the kernel, without asking NetworkManager.
    pub route: bool,
}

/// Wi-Fi presence and link state. Presence is independent of the link: an
/// adapter whose radio is off still has an entry point, otherwise it could
/// never be switched on again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WifiState {
    pub present: bool,
    pub powered: bool,
    pub connected: bool,
    pub hard_blocked: bool,
    pub soft_blocked: bool,
    pub ssid: Option<String>,
    pub signal: Option<u8>,
}

impl WifiState {
    fn absent() -> Self {
        Self {
            present: false,
            powered: false,
            connected: false,
            hard_blocked: false,
            soft_blocked: false,
            ssid: None,
            signal: None,
        }
    }
}

pub fn wifi_present() -> bool {
    wireless_interface().is_some()
}

pub fn wifi_state() -> WifiState {
    let Some(interface) = wireless_interface() else {
        return WifiState::absent();
    };
    let (hard, soft) = wifi_rfkill();
    let link = wifi_link(&interface);
    let powered = !hard && !soft;
    WifiState {
        present: true,
        powered,
        connected: powered && link.0,
        hard_blocked: hard,
        soft_blocked: soft,
        ssid: link.1,
        signal: link.2,
    }
}

/// Wi-Fi text for the bar. Empty on machines with no wireless adapter.
pub fn wifi_text(state: WifiState, icons: &Icons, use_icons: bool) -> String {
    if !state.present {
        return String::new();
    }
    let icon = if use_icons {
        icons.wifi
    } else {
        icons.wifi_fallback
    };
    if state.connected {
        icon.to_string()
    } else {
        format!("{icon} !")
    }
}

pub fn wifi_detail(state: WifiState) -> Vec<String> {
    let mut lines = Vec::new();
    if !state.present {
        return vec!["no wireless adapter in this machine".to_string()];
    }
    lines.push(if state.hard_blocked {
        "radio: blocked by a hardware switch".to_string()
    } else if state.soft_blocked {
        "radio: off".to_string()
    } else if state.powered {
        "radio: on".to_string()
    } else {
        "radio: off".to_string()
    });
    if state.connected {
        lines.push(format!(
            "network: {}",
            state.ssid.as_deref().unwrap_or("(hidden)")
        ));
        if let Some(signal) = state.signal {
            lines.push(format!("signal: {signal}%"));
        }
    } else if state.powered {
        lines.push("network: not connected".to_string());
    }
    if let Some(interface) = wireless_interface() {
        lines.push(format!("interface: {interface}"));
    }
    lines.push(String::new());
    lines.push("open the Wi-Fi selector to connect".to_string());
    lines
}

fn wireless_interface() -> Option<String> {
    let entries = fs::read_dir("/sys/class/net").ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with("p2p-dev-") {
            continue;
        }
        if entry.path().join("wireless").exists() {
            return Some(name);
        }
    }
    None
}

fn wifi_rfkill() -> (bool, bool) {
    let mut hard = false;
    let mut soft = false;
    let Ok(entries) = fs::read_dir("/sys/class/rfkill") else {
        return (false, false);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let kind = fs::read_to_string(path.join("type")).unwrap_or_default();
        if kind.trim() != "wlan" {
            continue;
        }
        hard |= fs::read_to_string(path.join("hard"))
            .map(|v| v.trim() == "1")
            .unwrap_or(false);
        soft |= fs::read_to_string(path.join("soft"))
            .map(|v| v.trim() == "1")
            .unwrap_or(false);
    }
    (hard, soft)
}

/// Read the link without NetworkManager. `iw` is the kernel's own interface;
/// `/proc/net/wireless` supplies the signal when `iw` is unavailable.
fn wifi_link(interface: &str) -> (bool, Option<String>, Option<u8>) {
    let mut ssid = None;
    let mut signal = signal_from_proc(interface);
    let mut connected = false;
    for program in ["/sbin/iw", "/usr/bin/iw", "/usr/sbin/iw"] {
        if !Path::new(program).exists() {
            continue;
        }
        let Some(output) = crate::probe::run(
            program,
            &["dev", interface, "link"],
            &[],
            std::time::Duration::from_secs(1),
            64 * 1024,
        ) else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let line = line.trim();
            if let Some(value) = line.strip_prefix("SSID: ") {
                ssid = Some(value.to_string());
                connected = true;
            } else if let Some(value) = line.strip_prefix("signal: ") {
                signal = value
                    .split_whitespace()
                    .next()
                    .and_then(|v| v.parse::<i32>().ok())
                    .map(|dbm| ((dbm + 90) * 100 / 60).clamp(0, 100) as u8);
            } else if line.contains("Not connected") {
                connected = false;
            }
        }
        break;
    }
    (connected, ssid, signal)
}

fn signal_from_proc(interface: &str) -> Option<u8> {
    let raw = fs::read_to_string("/proc/net/wireless").ok()?;
    for line in raw.lines().skip(2) {
        if !line.starts_with(interface) {
            continue;
        }
        let quality = line.split_whitespace().nth(2)?;
        let value: f32 = quality.trim_end_matches('.').parse().ok()?;
        return Some((value / 70.0 * 100.0).clamp(0.0, 100.0) as u8);
    }
    None
}

/// Return the best active link, preferring a wired connection.
///
/// Uses `/sys/class/net`: `operstate == "up"` and a carrier of 1 means the
/// interface is connected. Wi-Fi is recognised by the presence of a
/// `wireless` directory. Loopback and virtual bridges are ignored so a
/// docker0 interface does not look like a network connection.
pub fn network() -> Option<NetworkState> {
    let ifaces = active_interfaces();
    let link = if ifaces.iter().any(|(_, kind)| *kind == Link::Ethernet) {
        Link::Ethernet
    } else if ifaces.iter().any(|(_, kind)| *kind == Link::Wifi) {
        Link::Wifi
    } else {
        return None;
    };
    Some(NetworkState {
        link,
        route: default_route(),
    })
}

/// Wired link only. Used when the bar has a dedicated Wi-Fi segment, so the
/// same network is not represented twice.
pub fn network_wired() -> Option<NetworkState> {
    let wired = active_interfaces()
        .into_iter()
        .any(|(_, kind)| kind == Link::Ethernet);
    wired.then_some(NetworkState {
        link: Link::Ethernet,
        route: default_route(),
    })
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

/// Format the network segment, or an empty string when there is no link.
///
/// A link without a default route gets an explicit `!` suffix rather than the
/// same icon as a routed connection.
pub fn network_text(state: Option<NetworkState>, icons: &Icons, use_icons: bool) -> String {
    let Some(state) = state else {
        return String::new();
    };
    let icon = match state.link {
        Link::Wifi => {
            if use_icons {
                icons.wifi
            } else {
                icons.wifi_fallback
            }
        }
        Link::Ethernet => {
            if use_icons {
                icons.ethernet
            } else {
                icons.ethernet_fallback
            }
        }
    };
    if state.route {
        icon.to_string()
    } else {
        format!("{icon} !")
    }
}

/// A default IPv4 route in `/proc/net/route` (destination 0.0.0.0 with the
/// gateway flag), or a default IPv6 route. No NetworkManager or `ip` process.
fn default_route() -> bool {
    let v4 = fs::read_to_string("/proc/net/route").unwrap_or_default();
    let v6 = fs::read_to_string("/proc/net/ipv6_route").unwrap_or_default();
    default_route_from(&v4, &v6)
}

fn default_route_from(v4: &str, v6: &str) -> bool {
    for line in v4.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() >= 4
            && fields[1] == "00000000"
            && u32::from_str_radix(fields[3], 16).is_ok_and(|flags| flags & 0x2 != 0)
        {
            return true;
        }
    }
    for line in v6.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() >= 2 && fields[0].chars().all(|c| c == '0') && fields[1] == "00" {
            return true;
        }
    }
    false
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
    fn backlight_percentage_reads_the_first_device() {
        let dir = std::env::temp_dir().join(format!("nirubar-backlight-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("intel_backlight")).unwrap();
        std::fs::write(dir.join("intel_backlight/brightness"), "9600\n").unwrap();
        std::fs::write(dir.join("intel_backlight/max_brightness"), "19200\n").unwrap();
        assert_eq!(backlight_in(&dir), Some(50));
        std::fs::remove_dir_all(dir).unwrap();
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
                charging: false,
                state: BatteryState::Discharging,
                count: 1,
                remaining_minutes: None,
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
    fn multi_battery_uses_weighted_energy_and_reports_remaining() {
        let dir = tmpdir("multibat");
        for (name, capacity, now, full, power, status) in [
            ("BAT0", "50", "30", "60", "10", "Discharging"),
            ("BAT1", "75", "90", "120", "10", "Discharging"),
        ] {
            let bat = dir.join(name);
            fs::create_dir_all(&bat).unwrap();
            fs::write(bat.join("type"), "Battery\n").unwrap();
            fs::write(bat.join("capacity"), format!("{capacity}\n")).unwrap();
            fs::write(bat.join("energy_now"), format!("{now}\n")).unwrap();
            fs::write(bat.join("energy_full"), format!("{full}\n")).unwrap();
            fs::write(bat.join("power_now"), format!("{power}\n")).unwrap();
            fs::write(bat.join("status"), format!("{status}\n")).unwrap();
        }
        let aggregate = battery_in(&dir).expect("two batteries aggregate");
        assert_eq!(aggregate.percent, 67);
        assert_eq!(aggregate.count, 2);
        assert_eq!(aggregate.state, BatteryState::Discharging);
        assert_eq!(aggregate.remaining_minutes, Some(360));
    }

    #[test]
    fn absent_battery_estimate_stays_unavailable() {
        let dir = tmpdir("noestimate");
        let bat = dir.join("BAT0");
        fs::create_dir_all(&bat).unwrap();
        fs::write(bat.join("type"), "Battery\n").unwrap();
        fs::write(bat.join("capacity"), "40\n").unwrap();
        fs::write(bat.join("status"), "Discharging\n").unwrap();
        let aggregate = battery_in(&dir).expect("one battery");
        assert_eq!(aggregate.remaining_minutes, None);
    }

    #[test]
    fn mixed_battery_states_are_not_reported_as_charging() {
        let dir = tmpdir("mixedbat");
        for (name, status) in [("BAT0", "Charging"), ("BAT1", "Discharging")] {
            let bat = dir.join(name);
            fs::create_dir_all(&bat).unwrap();
            fs::write(bat.join("type"), "Battery\n").unwrap();
            fs::write(bat.join("capacity"), "50\n").unwrap();
            fs::write(bat.join("status"), format!("{status}\n")).unwrap();
        }
        let aggregate = battery_in(&dir).expect("two batteries");
        assert_eq!(aggregate.state, BatteryState::Mixed);
        assert!(!aggregate.charging);
    }

    #[test]
    fn default_route_is_read_from_the_kernel_tables() {
        let v4 = "Iface\tDestination\tGateway\tFlags\neth0\t00000000\t0102A8C0\t0003\n";
        assert!(default_route_from(v4, ""));
        let no_gateway = "Iface\tDestination\tGateway\tFlags\neth0\t00000000\t00000000\t0001\n";
        assert!(!default_route_from(no_gateway, ""));
        let v6 = "00000000000000000000000000000000 00 00000000000000000000000000000000 00 eth0 00000000 00000001 00000000 00000003\n";
        assert!(default_route_from("", v6));
    }

    #[test]
    fn link_without_route_is_marked_not_online() {
        let icons = Icons::plain();
        let routed = NetworkState {
            link: Link::Ethernet,
            route: true,
        };
        let linked_only = NetworkState {
            link: Link::Ethernet,
            route: false,
        };
        assert_eq!(network_text(Some(routed), &icons, true), icons.ethernet);
        assert_eq!(
            network_text(Some(linked_only), &icons, true),
            format!("{} !", icons.ethernet)
        );
    }

    #[test]
    fn battery_icons_track_level() {
        let icons = Icons::nerd();
        let at = |p: u8, c: bool| {
            battery_text(
                &Battery {
                    percent: p,
                    charging: c,
                    state: if c {
                        BatteryState::Charging
                    } else {
                        BatteryState::Discharging
                    },
                    count: 1,
                    remaining_minutes: None,
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
    fn cpu_uses_the_microchip_not_a_tiny_chart() {
        let glyph = Icons::nerd().cpu.chars().next().map(u32::from);
        assert_eq!(glyph, Some(0xf2db));
    }

    #[test]
    fn parses_display_outputs_and_primary() {
        let text = "Screen 0: minimum 8 x 8, current 3840 x 1080, maximum 32767 x 32767\neDP-1 connected primary 1920x1080+0+0 (normal left inverted right x axis y axis) 293mm x 165mm\nHDMI-1 connected 1920x1080+1920+0 (normal left inverted right x axis y axis) 527mm x 296mm\nDP-1 disconnected (normal left inverted right x axis y axis)\n";
        let outputs = display_outputs_from(text);
        assert_eq!(outputs.len(), 2);
        assert_eq!(outputs[0].name, "eDP-1");
        assert_eq!(outputs[0].resolution, "1920x1080");
        assert!(outputs[0].primary);
        assert_eq!(outputs[1].name, "HDMI-1");
        assert!(!outputs[1].primary);
        // With two screens each bar says which one it is.
        let left = crate::module::Screen {
            name: Some("eDP-1".into()),
        };
        let right = crate::module::Screen {
            name: Some("HDMI-1".into()),
        };
        // File order decides which screen is on the left, as xrandr lists it.
        assert_eq!(display_text(&outputs, "D", &left), "D 2 right");
        assert_eq!(display_text(&outputs, "D", &right), "D 2 left");
        // Stacked screens read above/below instead of left/right.
        let stacked = "DP-1 connected 2560x1440+0+1440 (normal left inverted right x axis y axis)\nDP-2 connected 2560x1440+0+0 (normal left inverted right x axis y axis)\n";
        let stacked = display_outputs_from(stacked);
        let lower = crate::module::Screen {
            name: Some("DP-1".into()),
        };
        let upper = crate::module::Screen {
            name: Some("DP-2".into()),
        };
        // DP-1 sits at y=1440, below DP-2. Each bar names where the other
        // screen is: the lower screen has one above it, the upper one below.
        assert_eq!(display_text(&stacked, "D", &lower), "D 2 above");
        assert_eq!(display_text(&stacked, "D", &upper), "D 2 below");
        // The same for side by side: the right screen has one to its left.
        let side = display_outputs_from(
            "eDP-1 connected 1920x1080+0+0 (normal left inverted right x axis y axis)\n\
             DP-3 connected 2560x1440+1920+0 (normal left inverted right x axis y axis)\n",
        );
        let left = crate::module::Screen {
            name: Some("eDP-1".into()),
        };
        let right = crate::module::Screen {
            name: Some("DP-3".into()),
        };
        assert_eq!(display_text(&side, "D", &left), "D 2 right");
        assert_eq!(display_text(&side, "D", &right), "D 2 left");
        // A screen placed diagonally reads by the vertical difference.
        let corner = display_outputs_from(
            "eDP-1 connected 1920x1080+0+0 (normal left inverted right x axis y axis)\n\
             DP-3 connected 2560x1440+1920+500 (normal left inverted right x axis y axis)\n",
        );
        assert_eq!(display_text(&corner, "D", &left), "D 2 below");
        // One screen does not need a permanent module.
        let single = crate::module::Screen {
            name: Some("eDP-1".into()),
        };
        assert_eq!(display_text(&outputs[..1], "D", &single), "");
        assert_eq!(display_detail(&outputs)[0], "eDP-1: 1920x1080 (primary)");
    }

    #[test]
    fn mode_parser_accepts_active_state() {
        let text = "name=focus\nexpiry=9999999999\ntoken=x\n";
        let mut name = None;
        let mut expiry = None;
        for line in text.lines() {
            if let Some((key, value)) = line.split_once('=') {
                match key {
                    "name" => name = Some(value.to_string()),
                    "expiry" => expiry = value.parse::<u64>().ok(),
                    _ => {}
                }
            }
        }
        assert_eq!(name.as_deref(), Some("focus"));
        assert!(expiry.is_some_and(|expiry| expiry > 0));
    }

    #[test]
    fn wifi_segment_is_empty_without_an_adapter_and_marks_a_dead_link() {
        let icons = Icons::plain();
        assert_eq!(wifi_text(WifiState::absent(), &icons, true), "");
        let off = WifiState {
            present: true,
            powered: false,
            ..WifiState::absent()
        };
        assert_eq!(
            wifi_text(off.clone(), &icons, true),
            format!("{} !", icons.wifi)
        );
        let connected = WifiState {
            present: true,
            powered: true,
            connected: true,
            ssid: Some("Home".into()),
            signal: Some(61),
            ..WifiState::absent()
        };
        assert_eq!(wifi_text(connected.clone(), &icons, true), icons.wifi);
        let detail = wifi_detail(connected);
        assert!(detail.iter().any(|line| line == "network: Home"));
        assert!(detail.iter().any(|line| line == "signal: 61%"));
    }

    #[test]
    fn plain_fallbacks_are_not_glyphs() {
        let icons = Icons::plain();
        assert!(icons.wifi.is_ascii());
        assert!(icons.ethernet.is_ascii());
        assert!(icons.battery_full.is_ascii());
    }
}

/// A connected RandR output. Resolution is the current mode geometry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplayOutput {
    pub name: String,
    pub resolution: String,
    pub primary: bool,
    /// The output's top-left corner on the combined desktop, from
    /// `2560x1440+2560+0`.
    pub position: (i16, i16),
}

fn display_outputs_from(text: &str) -> Vec<DisplayOutput> {
    let mut outputs = Vec::new();
    for line in text.lines() {
        if !line.contains(" connected") {
            continue;
        }
        let mut words = line.split_whitespace();
        let Some(name) = words.next() else { continue };
        let primary = line.contains(" primary ");
        let geometry = line
            .split_whitespace()
            .find(|word| word.contains('x') && word.contains('+'));
        let resolution = geometry
            .and_then(|word| word.split('+').next())
            .unwrap_or("?")
            .to_string();
        let position = geometry.and_then(|word| {
            let (_, rest) = word.split_once('+')?;
            let (x, y) = rest.split_once('+')?;
            Some((x.parse::<i16>().ok()?, y.parse::<i16>().ok()?))
        });
        outputs.push(DisplayOutput {
            name: name.to_string(),
            resolution,
            primary,
            position: position.unwrap_or((0, 0)),
        });
    }
    outputs
}

/// Connected outputs through xrandr. The bar samples this at a slower cadence
/// than battery/network: hotplug is rare and output enumeration is not free.
pub fn display_outputs() -> Option<Vec<DisplayOutput>> {
    let out = crate::probe::run(
        "xrandr",
        &["--query"],
        &[],
        std::time::Duration::from_secs(2),
        64 * 1024,
    )?;
    out.status
        .success()
        .then(|| display_outputs_from(&String::from_utf8_lossy(&out.stdout)))
}

/// Name this bar's screen, and where it sits when there are several.
///
/// With more than one screen the count alone does not say which bar is which,
/// so the bar adds the screen's place: `2 above` when the screens are stacked,
/// `2 right` when they are side by side. A single screen still shows nothing.
pub fn display_text(
    outputs: &[DisplayOutput],
    icon: &str,
    screen: &crate::module::Screen,
) -> String {
    if outputs.len() <= 1 {
        return String::new();
    }
    let own = screen.name.as_deref().and_then(|name| {
        outputs
            .iter()
            .find(|output| output.name == name)
            .map(|output| output.position)
    });
    let count = outputs.len();
    let Some((x, y)) = own else {
        return format!("{icon} {count}");
    };
    // Where the other screens are, counted on each side of this one.
    let mut offsets: Vec<(i32, i32)> = Vec::new();
    for output in outputs {
        let (ox, oy) = output.position;
        if (ox, oy) != (x, y) {
            // Subtract in i32: two screens far apart can overflow i16.
            offsets.push((i32::from(ox) - i32::from(x), i32::from(oy) - i32::from(y)));
        }
    }
    let place = display_place(&offsets);
    if place.is_empty() {
        format!("{icon} {count}")
    } else {
        format!("{icon} {count} {place}")
    }
}

/// Name where this screen sits among the others, from relative positions.
///
/// The label says where the *other* screens are, as the bar has always read
/// it: a screen below the top one shows `2 above`, because the second screen
/// is above it. A screen to the right of another shows `2 left`. A stack that
/// is also offset sideways is `above` or `below` first, since that is the
/// arrangement a reader notices.
fn display_place(offsets: &[(i32, i32)]) -> &'static str {
    let any = |test: &dyn Fn(i32, i32) -> bool| offsets.iter().any(|&(dx, dy)| test(dx, dy));
    if any(&|_, dy| dy < 0) {
        "above"
    } else if any(&|_, dy| dy > 0) {
        "below"
    } else if any(&|dx, _| dx < 0) {
        "left"
    } else if any(&|dx, _| dx > 0) {
        "right"
    } else {
        ""
    }
}

pub fn display_detail(outputs: &[DisplayOutput]) -> Vec<String> {
    if outputs.is_empty() {
        return vec!["no connected outputs".to_string()];
    }
    outputs
        .iter()
        .map(|output| {
            format!(
                "{}: {}{}",
                output.name,
                output.resolution,
                if output.primary { " (primary)" } else { "" }
            )
        })
        .collect()
}

/// Active work mode and the seconds left, read from the shared state file.
pub fn mode_status() -> Option<(String, u64)> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
                .join(".local/state")
        });
    let text = std::fs::read_to_string(base.join("niruwm/mode")).ok()?;
    let mut name = None;
    let mut expiry = None;
    for line in text.lines() {
        if let Some((key, value)) = line.split_once('=') {
            match key {
                "name" => name = Some(value.to_string()),
                "expiry" => expiry = value.parse::<u64>().ok(),
                _ => {}
            }
        }
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    let expiry = expiry?;
    (expiry > now).then(|| (name.unwrap_or_else(|| "mode".into()), expiry - now))
}

/// Detail lines for the battery popup.
pub fn battery_detail(b: &Battery) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(format!("charge: {}%", b.percent));
    lines.push(format!(
        "state: {}",
        match b.state {
            BatteryState::Charging => "charging",
            BatteryState::Discharging => "on battery",
            BatteryState::Full => "full",
            BatteryState::Mixed => "mixed (one pack charging, one discharging)",
            BatteryState::Unknown => "unknown",
        }
    ));
    if b.count > 1 {
        lines.push(format!("batteries: {}", b.count));
    }
    lines.push(match b.remaining_minutes {
        Some(minutes) => format!("remaining: {}h {:02}m", minutes / 60, minutes % 60),
        None => "remaining: unavailable".to_string(),
    });
    lines
}

/// Detail lines for the network popup: link, route, interface and address.
pub fn network_detail(state: NetworkState) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(match state.link {
        Link::Wifi => "type: wireless".to_string(),
        Link::Ethernet => "type: wired".to_string(),
    });
    lines.push("link: up".to_string());
    lines.push(if state.route {
        "route: default present".to_string()
    } else {
        "route: no default route".to_string()
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
            state: BatteryState::Charging,
            count: 2,
            remaining_minutes: Some(35),
        };
        let lines = battery_detail(&b);
        assert!(lines[0].contains("42%"));
        assert!(lines.iter().any(|l| l.contains("charging")));
    }

    #[test]
    fn network_detail_states_the_type() {
        let lines = network_detail(NetworkState {
            link: Link::Wifi,
            route: true,
        });
        assert!(lines[0].contains("wireless"));
        assert!(lines.iter().any(|line| line.contains("default present")));
    }
}
