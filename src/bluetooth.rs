//! Read-only BlueZ snapshots through the suite's shared CLI. The X11 thread
//! never waits for BlueZ or bluetoothctl; one bounded worker handles provider I/O.
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

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

pub struct Monitor {
    state: Arc<Mutex<Snapshot>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Monitor {
    pub fn new() -> Self {
        let state = Arc::new(Mutex::new(Snapshot::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let ws = state.clone();
        let flag = stop.clone();
        let worker = thread::Builder::new()
            .name("nirubar-bluetooth".into())
            .spawn(move || {
                while !flag.load(Ordering::Relaxed) {
                    let snapshot = query();
                    if let Ok(mut state) = ws.lock() {
                        *state = snapshot;
                    }
                    for _ in 0..20 {
                        if flag.load(Ordering::Relaxed) {
                            return;
                        }
                        thread::sleep(Duration::from_millis(100));
                    }
                }
            })
            .ok();
        Self {
            state,
            stop,
            worker,
        }
    }
    pub fn snapshot(&self) -> Snapshot {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
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
