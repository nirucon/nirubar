//! Read-only BlueZ snapshots through the suite's shared CLI. The X11 thread
//! never waits for BlueZ or bluetoothctl; one bounded worker handles provider I/O.
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub mac: String,
    pub name: String,
    pub connected: bool,
    pub battery: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub adapter: Option<String>,
    pub powered: bool,
    pub devices: Vec<Device>,
    pub error: Option<String>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            adapter: None,
            powered: false,
            devices: Vec::new(),
            error: Some("checking Bluetooth".into()),
        }
    }
}

fn parse(text: &str) -> Snapshot {
    let value = |key: &str| {
        text.lines()
            .filter_map(|line| line.split_once('='))
            .find_map(|(name, value)| (name == key).then_some(value))
    };
    if value("state") != Some("ready") {
        return Snapshot {
            error: Some(value("reason").unwrap_or("Bluetooth unavailable").into()),
            ..Snapshot::default()
        };
    }
    let mut devices = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("device=") else {
            continue;
        };
        let fields: Vec<&str> = rest.split('\t').collect();
        if fields.len() != 4 || fields[0].is_empty() {
            continue;
        }
        devices.push(Device {
            mac: fields[0].into(),
            name: if fields[1].is_empty() {
                "Unknown device".into()
            } else {
                fields[1].into()
            },
            connected: fields[2] == "true",
            battery: fields[3].parse().ok(),
        });
    }
    Snapshot {
        adapter: value("adapter")
            .filter(|a| !a.is_empty())
            .map(str::to_string),
        powered: value("powered") == Some("true"),
        devices,
        error: None,
    }
}

fn query() -> Snapshot {
    match crate::probe::run(
        "niructl",
        &["bluetooth", "status"],
        &[],
        Duration::from_secs(6),
        32 * 1024,
    ) {
        Some(out) if out.status.success() => parse(&String::from_utf8_lossy(&out.stdout)),
        _ => Snapshot {
            error: Some("niructl bluetooth status did not respond".into()),
            ..Snapshot::default()
        },
    }
}

/// BlueZ's adapter and paired devices from its object manager: one call
/// when BlueZ signals a change (power, a connection, a battery level), no
/// helper process and no polling.
fn from_objects(objects: &crate::objects::Objects) -> Snapshot {
    use crate::objects::{flag, number, text};
    const ADAPTER: &str = "org.bluez.Adapter1";
    const DEVICE: &str = "org.bluez.Device1";
    let adapter = objects.values().find(|o| o.contains_key(ADAPTER));
    let Some(adapter) = adapter else {
        return Snapshot {
            adapter: None,
            error: None,
            ..Snapshot::default()
        };
    };
    let mut devices: Vec<Device> = objects
        .values()
        .filter(|o| flag(o, DEVICE, "Paired"))
        .map(|o| {
            let name = [text(o, DEVICE, "Alias"), text(o, DEVICE, "Name")]
                .into_iter()
                .find(|n| !n.trim().is_empty())
                .unwrap_or("Unknown device")
                .to_string();
            Device {
                mac: text(o, DEVICE, "Address").to_string(),
                name,
                connected: flag(o, DEVICE, "Connected"),
                battery: number(o, "org.bluez.Battery1", "Percentage").map(|n| n.min(100) as u32),
            }
        })
        .collect();
    devices.sort_by(|a, b| b.connected.cmp(&a.connected).then(a.name.cmp(&b.name)));
    Snapshot {
        adapter: Some(text(adapter, ADAPTER, "Address").to_string()).filter(|a| !a.is_empty()),
        powered: flag(adapter, ADAPTER, "Powered"),
        devices,
        error: None,
    }
}

pub enum Monitor {
    Watched(crate::objects::Watched<Snapshot>),
    /// The suite's CLI, for the tests (NIRUBAR_BLUETOOTH_CLI=1).
    Cli(crate::module::Cached<Snapshot>),
}

impl Monitor {
    pub fn new() -> Self {
        if std::env::var_os("NIRUBAR_BLUETOOTH_CLI").is_some() {
            return Self::Cli(crate::module::Cached::new(Duration::from_secs(2), || {
                Some(query())
            }));
        }
        Self::Watched(crate::objects::Watched::new(
            "org.bluez",
            "/",
            Duration::from_secs(120),
            |reader| {
                Some(match reader.objects("org.bluez", "/") {
                    Some(objects) => from_objects(&objects),
                    None => Snapshot {
                        adapter: None,
                        error: None,
                        ..Snapshot::default()
                    },
                })
            },
        ))
    }

    pub fn snapshot(&self) -> Snapshot {
        match self {
            Self::Watched(w) => w.get(),
            Self::Cli(c) => c.get(),
        }
        .unwrap_or_default()
    }
}

pub fn text(snapshot: &Snapshot, icon: &str) -> String {
    if snapshot.adapter.is_none() {
        return String::new();
    }
    if snapshot.error.is_some() {
        return format!("{icon} --");
    }
    if !snapshot.powered {
        return format!("{icon} off");
    }
    match snapshot.devices.iter().find(|d| d.connected) {
        Some(device) => {
            let name: String = device.name.chars().take(14).collect();
            format!("{icon} {name}")
        }
        None => icon.to_string(),
    }
}

pub fn detail(snapshot: &Snapshot) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(error) = &snapshot.error {
        lines.push(format!("unavailable: {error}"));
    } else if snapshot.adapter.is_none() {
        lines.push("no Bluetooth adapter".into());
    } else {
        lines.push(format!(
            "adapter: {} ({})",
            snapshot.adapter.as_deref().unwrap_or(""),
            if snapshot.powered { "on" } else { "off" }
        ));
        if snapshot.devices.is_empty() {
            lines.push("no paired devices".into());
        }
        for device in &snapshot.devices {
            lines.push(format!(
                "{}{}{}",
                device.name,
                if device.connected {
                    " · connected"
                } else {
                    ""
                },
                device
                    .battery
                    .map(|b| format!(" · {b}%"))
                    .unwrap_or_default()
            ));
        }
    }
    lines.extend([
        String::new(),
        "right-click: power on/off".into(),
        "middle-click: device selector / connect / disconnect".into(),
        "keyboard: nirulauncher --bluetooth".into(),
    ]);
    lines
}

#[cfg(test)]
mod tests {

    #[test]
    fn bluez_objects_become_a_snapshot() {
        let o = crate::objects::parse_fixture(
            "/org/bluez/hci0\torg.bluez.Adapter1\tAddress\ts:AA:BB\n\
             /org/bluez/hci0\torg.bluez.Adapter1\tPowered\tb:true\n\
             /org/bluez/hci0/dev_1\torg.bluez.Device1\tPaired\tb:true\n\
             /org/bluez/hci0/dev_1\torg.bluez.Device1\tAlias\ts:Mouse\n\
             /org/bluez/hci0/dev_1\torg.bluez.Device1\tAddress\ts:11:22\n\
             /org/bluez/hci0/dev_2\torg.bluez.Device1\tPaired\tb:true\n\
             /org/bluez/hci0/dev_2\torg.bluez.Device1\tName\ts:Headphones\n\
             /org/bluez/hci0/dev_2\torg.bluez.Device1\tAddress\ts:33:44\n\
             /org/bluez/hci0/dev_2\torg.bluez.Device1\tConnected\tb:true\n\
             /org/bluez/hci0/dev_2\torg.bluez.Battery1\tPercentage\tn:80\n\
             /org/bluez/hci0/dev_3\torg.bluez.Device1\tAlias\ts:Not paired\n",
        );
        let s = from_objects(&o);
        assert_eq!(s.adapter.as_deref(), Some("AA:BB"));
        assert!(s.powered);
        assert_eq!(s.devices.len(), 2);
        // Connected first.
        assert_eq!(s.devices[0].name, "Headphones");
        assert!(s.devices[0].connected);
        assert_eq!(s.devices[0].battery, Some(80));
        assert_eq!(s.devices[1].mac, "11:22");
        // No adapter: hidden, not an error.
        let none = from_objects(&crate::objects::Objects::new());
        assert!(none.adapter.is_none() && none.error.is_none());
        assert_eq!(text(&none, "B"), "");
    }

    use super::*;

    #[test]
    fn parses_state_and_never_invents_battery_or_connection() {
        let snapshot = parse("state=ready\nadapter=AA:BB\npowered=true\ndevice=CC:DD\tHeadphones\ttrue\t75\ndevice=EE:FF\tKeyboard\tfalse\t\n");
        assert_eq!(text(&snapshot, "bt"), "bt Headphones");
        assert_eq!(snapshot.devices[1].battery, None);
        assert!(!snapshot.devices[1].connected);
        assert_eq!(
            text(&parse("state=ready\nadapter=\npowered=false\n"), "bt"),
            ""
        );
        assert_eq!(
            text(&parse("state=provider-unavailable\nreason=missing\n"), "bt"),
            ""
        );
    }
}
